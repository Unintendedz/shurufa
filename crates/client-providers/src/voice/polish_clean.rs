//! LLM 润色/改写/翻译输出清洗与防幻觉防线（无网络、无状态纯函数）。
//!
//! 统一收口所有模型输出校验逻辑，确保无论对接本地 `llama-server` 还是云端模型，都不会向宿主泄露：
//! - 思考链 `<think>...</think>` 或截断未闭合 `<think>`；
//! - 联合路由标签 `<action>rewrite|dictate</action>`、外层围栏 ```` ``` ````、`<output>`/`<asr_text>` 或冗余首尾引号；
//! - 助手元回复、拒答话术或 IM 场景误加的邮件签名占位符（如 `Best regards, [Your Name]`）；
//! - 复读死循环、异常膨胀/过度塌缩或非预期的繁体中文漂移。

use super::postprocess::{self, AppCategory, PostprocessInput, TrailingPeriodPolicy, VoiceMode};
use super::vocabulary::VocabularyTerm;
use msime_client_base::chinese_conversion::traditional_to_simplified_characters;
use msime_client_base::text::is_han_character;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

fn default_true() -> bool {
    true
}

/// 清洗阶段解析出的最终上屏动作，与 `msime_client_voice_polish_clean` 契约一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedAction {
    ReplaceSelection,
    #[default]
    Insert,
}

/// 润色输出清洗请求，与 `msime_client_voice_polish_clean` JSON 契约对齐。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolishCleanInput {
    pub raw_text: String,
    pub polished_text: String,
    #[serde(default)]
    pub selected_text: Option<String>,
    #[serde(default)]
    pub mode: VoiceMode,
    #[serde(default)]
    pub expect_action_tag: bool,
    #[serde(default)]
    pub app_category: AppCategory,
    #[serde(default = "default_true")]
    pub enable_pangu_spacing: bool,
    /// 期望简体中文时防繁简漂移（原文已为繁体或翻译模式自动跳过）。
    #[serde(default = "default_true")]
    pub enforce_simplified_chinese: bool,
    #[serde(default)]
    pub trailing_period_policy: TrailingPeriodPolicy,
    #[serde(default)]
    pub vocabulary: Vec<VocabularyTerm>,
}

impl Default for PolishCleanInput {
    fn default() -> Self {
        Self {
            raw_text: String::new(),
            polished_text: String::new(),
            selected_text: None,
            mode: VoiceMode::Dictation,
            expect_action_tag: false,
            app_category: AppCategory::General,
            enable_pangu_spacing: true,
            enforce_simplified_chinese: true,
            trailing_period_policy: TrailingPeriodPolicy::Auto,
            vocabulary: Vec::new(),
        }
    }
}

/// 润色输出清洗结果；`accepted == false` 时 `text` 必为空串，宿主应降级使用确定性后处理后的 `raw_text`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolishCleanOutput {
    pub accepted: bool,
    pub text: String,
    pub resolved_action: ResolvedAction,
    pub rejection_reason: Option<String>,
}

fn reject(reason: &'static str) -> PolishCleanOutput {
    PolishCleanOutput {
        accepted: false,
        text: String::new(),
        resolved_action: ResolvedAction::Insert,
        rejection_reason: Some(reason.to_owned()),
    }
}

/// 清洗并校验 LLM 润色/改写/翻译输出。
pub fn clean_polish_output(input: &PolishCleanInput) -> PolishCleanOutput {
    let normalized_nl = input
        .polished_text
        .replace("\r\n", "\n")
        .replace('\r', "\n");

    // 1. 剥离 `<think>...</think>`（跨行、大小写不敏感、多次）；未闭合且无正文判 `unclosed_think`。
    let (after_think, had_unclosed_think) = strip_think_blocks(&normalized_nl);
    if had_unclosed_think && after_think.trim().is_empty() {
        return reject("unclosed_think");
    }

    // 2. 仅在 `RewriteSelection && expect_action_tag` 时采信 `<action>` 标签决定上屏动作；
    // 非 `expect_action_tag` 模式动作恒由 `mode` 锁定，仅当原文不含 `<action>` 时剥离模型开头意外泄漏的标签。
    let source_mentions_action = input.raw_text.to_ascii_lowercase().contains("<action>")
        || input
            .selected_text
            .as_deref()
            .is_some_and(|sel| sel.to_ascii_lowercase().contains("<action>"));
    let (after_action, resolved_action) =
        if input.mode == VoiceMode::RewriteSelection && input.expect_action_tag {
            let (stripped, parsed) = extract_and_strip_action_tag(&after_think);
            (stripped, parsed.unwrap_or(ResolvedAction::Insert))
        } else {
            let stripped = if source_mentions_action {
                after_think
            } else {
                strip_leading_leaked_action_tag(&after_think)
            };
            let fixed_action = if input.mode == VoiceMode::RewriteSelection {
                ResolvedAction::ReplaceSelection
            } else {
                ResolvedAction::Insert
            };
            (stripped, fixed_action)
        };
    let effective_mode =
        if input.mode == VoiceMode::RewriteSelection && resolved_action == ResolvedAction::Insert {
            VoiceMode::Dictation
        } else {
            input.mode
        };

    // 3. 剥离外层包装噪声（XML 包裹标签、代码围栏与冗余成对首尾引号），同时保护正文首行缩进。
    let selected_has_fence = input
        .selected_text
        .as_deref()
        .is_some_and(|sel| sel.contains("```"));
    let mut text = strip_outer_wrappers(
        &after_action,
        effective_mode,
        selected_has_fence,
        &input.raw_text,
        input.selected_text.as_deref(),
    );

    if text.trim().is_empty() {
        return reject("empty_output");
    }

    // 4. 助手元回复与拒答检测（含纯拒答直接拒绝、无害前缀剥离后复核）。
    match filter_assistant_meta_reply(&text, &input.raw_text, input.selected_text.as_deref()) {
        MetaReplyOutcome::Reject => return reject("assistant_meta_reply"),
        MetaReplyOutcome::Accept(cleaned) => text = cleaned,
    }
    if text.trim().is_empty() {
        return reject("assistant_meta_reply");
    }

    // 5. IM 场景清理误生成的邮件落款与 `[Your Name]` 占位符。
    if input.app_category == AppCategory::Im {
        text = strip_im_email_signature(&text, &input.raw_text, input.selected_text.as_deref());
        if text.trim().is_empty() {
            return reject("empty_output");
        }
    }

    // 6. 防复读死循环（检测末尾连续重复 `>= 4` 次的短语/子串）。
    if has_llm_repetition_loop(&text, &input.raw_text) {
        return reject("repetition_loop");
    }

    // 7. 按模式分级长度膨胀/塌缩守卫。
    if let Some(reason) = check_length_guard(
        &text,
        &input.raw_text,
        input.selected_text.as_deref(),
        effective_mode,
    ) {
        return reject(reason);
    }

    // 8. 简体中文锁：防模型意外漂移出繁体字符（原文已含繁体或翻译模式自动豁免）。
    if input.enforce_simplified_chinese && effective_mode != VoiceMode::Translate {
        text = enforce_simplified_lock(&text, &input.raw_text, input.selected_text.as_deref());
    }

    // 9. 补跑幂等确定性后处理（保留行首缩进的 Pangu 空格、技术词保真、保守 ITN、中文弯引号与双正交句号策略）。
    let post = postprocess::postprocess(&PostprocessInput {
        text,
        model_kind: String::new(),
        audio_ms: None,
        mode: effective_mode,
        app_category: input.app_category,
        enable_pangu_spacing: input.enable_pangu_spacing,
        enable_number_itn: Some(effective_mode != VoiceMode::Translate),
        enable_tech_casing: true,
        trailing_period_policy: input.trailing_period_policy,
        vocabulary: input.vocabulary.clone(),
    });
    if post.dismissed || post.text.trim().is_empty() {
        return reject("empty_output");
    }

    PolishCleanOutput {
        accepted: true,
        text: post.text,
        resolved_action,
        rejection_reason: None,
    }
}

/// 剥离所有 `<think>...</think>`（含跨行与大小写变体）及孤立 `</think>`；返回 `(剩余文本, 是否存在未闭合标签)`。
fn strip_think_blocks(text: &str) -> (String, bool) {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut had_unclosed = false;

    while cursor < text.len() {
        if let Some(rel_open) = lower[cursor..].find("<think>") {
            let open_start = cursor + rel_open;
            out.push_str(&text[cursor..open_start]);
            let search_from = open_start + "<think>".len();
            if let Some(rel_close) = lower[search_from..].find("</think>") {
                cursor = search_from + rel_close + "</think>".len();
            } else {
                had_unclosed = true;
                cursor = text.len();
            }
        } else {
            out.push_str(&text[cursor..]);
            break;
        }
    }

    // 清理某些本地模型在 `--reasoning-budget 0` 下偶尔残留的孤立 `</think>`。
    let out_lower = out.to_ascii_lowercase();
    if out_lower.contains("</think>") {
        let mut cleaned = String::with_capacity(out.len());
        let mut pos = 0;
        while pos < out.len() {
            if let Some(rel) = out_lower[pos..].find("</think>") {
                cleaned.push_str(&out[pos..pos + rel]);
                pos += rel + "</think>".len();
            } else {
                cleaned.push_str(&out[pos..]);
                break;
            }
        }
        return (trim_blank_edges(&cleaned), had_unclosed);
    }

    (trim_blank_edges(&out), had_unclosed)
}

/// 仅裁剪首尾的空行（与不含缩进正文的换行），严禁破坏首行有效 `[ \t]+` 缩进。
fn trim_blank_edges(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    if start == lines.len() {
        return String::new();
    }
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |idx| idx + 1);
    lines[start..end].join("\n")
}

/// 解析并剥离所有 `<action>rewrite|dictate</action>` 标签，避免泄漏到上屏文本。
fn extract_and_strip_action_tag(text: &str) -> (String, Option<ResolvedAction>) {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut resolved: Option<ResolvedAction> = None;
    let mut cursor = 0usize;

    while cursor < text.len() {
        if let Some(rel_open) = lower[cursor..].find("<action>") {
            let open_start = cursor + rel_open;
            let body_start = open_start + "<action>".len();
            if let Some(rel_close) = lower[body_start..].find("</action>") {
                let body_end = body_start + rel_close;
                let tag_value = lower[body_start..body_end].trim();
                if resolved.is_none() {
                    resolved = match tag_value {
                        "rewrite" | "replace" | "replace_selection" => {
                            Some(ResolvedAction::ReplaceSelection)
                        }
                        "dictate" | "insert" => Some(ResolvedAction::Insert),
                        _ => None,
                    };
                }
                out.push_str(&text[cursor..open_start]);
                cursor = body_end + "</action>".len();
                // 跳过紧跟在行首标签后面的单个换行，保留下一行的原始缩进。
                if text[cursor..].starts_with('\n') {
                    cursor += 1;
                }
                continue;
            }
        }
        out.push_str(&text[cursor..]);
        break;
    }

    (trim_blank_edges(&out), resolved)
}

/// 在非 `expect_action_tag` 模式下，仅剥离模型开头意外泄漏的单个 `<action>...</action>` 标签，不改变动作判定。
fn strip_leading_leaked_action_tag(text: &str) -> String {
    let trimmed = text.trim_start_matches([' ', '\t', '\n']);
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest_open) = lower.strip_prefix("<action>") {
        if let Some(rel_close) = rest_open.find("</action>") {
            let after_tag = &trimmed["<action>".len() + rel_close + "</action>".len()..];
            let after_tag = after_tag
                .strip_prefix('\n')
                .unwrap_or_else(|| after_tag.trim_start_matches([' ', '\t']));
            return trim_blank_edges(after_tag);
        }
    }
    text.to_owned()
}

/// 剥离外层 `<output>`/`<asr_text>` 等包裹标签、外层代码围栏及冗余成对首尾引号。
fn strip_outer_wrappers(
    text: &str,
    mode: VoiceMode,
    selected_has_fence: bool,
    raw_text: &str,
    selected_text: Option<&str>,
) -> String {
    let mut current = trim_blank_edges(text);
    for _ in 0..3 {
        let before = current.clone();
        current = strip_outer_xml_wrapper(&current);
        current = strip_outer_code_fence(&current, mode, selected_has_fence);
        current = strip_outer_decorative_quotes(&current, raw_text, selected_text);
        current = trim_blank_edges(&current);
        if current == before {
            break;
        }
    }
    current
}

fn strip_outer_xml_wrapper(text: &str) -> String {
    const TAGS: &[&str] = &[
        "output",
        "asr_text",
        "result",
        "polished",
        "text",
        "translation",
    ];
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    for tag in TAGS {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if lower.starts_with(&open)
            && lower.ends_with(&close)
            && trimmed.len() >= open.len() + close.len()
        {
            let inner = &trimmed[open.len()..trimmed.len() - close.len()];
            return trim_blank_edges(inner);
        }
    }
    text.to_owned()
}

fn strip_outer_code_fence(text: &str, mode: VoiceMode, selected_has_fence: bool) -> String {
    if mode == VoiceMode::RewriteSelection && selected_has_fence {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() < 2 {
        return text.to_owned();
    }
    let first = lines[0].trim();
    let last = lines[lines.len() - 1].trim();
    if !first.starts_with("```") || last != "```" {
        return text.to_owned();
    }
    let inner = &lines[1..lines.len() - 1];
    // 内部还含围栏说明正文自身包含多个代码块，不贸然剥除外层。
    if inner.iter().any(|line| line.trim().starts_with("```")) {
        return text.to_owned();
    }
    inner.join("\n")
}

fn strip_outer_decorative_quotes(
    text: &str,
    raw_text: &str,
    selected_text: Option<&str>,
) -> String {
    // 含行首缩进的结构化文本不剥引号。
    if text.starts_with(' ') || text.starts_with('\t') {
        return text.to_owned();
    }
    let pairs = [('"', '"'), ('“', '”'), ('\'', '\''), ('「', '」')];
    for (open, close) in pairs {
        if let Some(inner) = text
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
        {
            let source_had_pair = has_outer_pair(raw_text.trim(), open, close)
                || selected_text.is_some_and(|sel| has_outer_pair(sel.trim(), open, close));
            // 若内部仍有同种引号（如 `"甲"和"乙"`），绝不能误剥外层。
            let inner_has_same = inner.contains(open) || inner.contains(close);
            if !source_had_pair && !inner_has_same && !inner.trim().is_empty() {
                return inner.to_owned();
            }
        }
    }
    text.to_owned()
}

fn has_outer_pair(text: &str, open: char, close: char) -> bool {
    text.starts_with(open) && text.ends_with(close)
}

enum MetaReplyOutcome {
    Reject,
    Accept(String),
}

/// 检测拒答/助手身份声明与元回复前缀（如 `好的，以下是`、`当然，整理如下`、`作为人工智能`、`我无法`、`Here is the`、`Sure, here`）。
fn filter_assistant_meta_reply(
    text: &str,
    raw_text: &str,
    selected_text: Option<&str>,
) -> MetaReplyOutcome {
    let lower = text.to_ascii_lowercase();
    let source_has = |needle: &str| -> bool {
        raw_text.to_ascii_lowercase().contains(needle)
            || selected_text.is_some_and(|sel| sel.to_ascii_lowercase().contains(needle))
    };

    // 1. 硬拒答或 AI 身份声明：直接拒绝并回退确定性原稿。
    const HARD_REFUSALS: &[&str] = &[
        "作为人工智能",
        "作为一个ai",
        "作为一名ai",
        "作为ai助手",
        "作为语言模型",
        "作为大语言模型",
        "我是一个ai",
        "我是人工智能",
        "我无法完成",
        "我无法满足",
        "我无法提供",
        "我无法处理",
        "我无法回答",
        "我不能回答",
        "抱歉，我无法",
        "对不起，我无法",
        "很抱歉，我无法",
        "请提供需要整理的文本",
        "请提供您需要",
        "没有看到需要整理的内容",
        "as an ai",
        "as a language model",
        "as an artificial intelligence",
        "i cannot fulfill",
        "i can't fulfill",
        "i cannot assist",
        "i cannot help with that",
        "i'm sorry, i cannot",
        "i am sorry, i cannot",
        "i'm sorry, i can't",
        "i'm unable to",
        "please provide the text",
    ];
    for pattern in HARD_REFUSALS {
        if lower.contains(pattern) && !source_has(pattern) {
            return MetaReplyOutcome::Reject;
        }
    }

    // 2. 元回复寒暄前缀：若引导语本身表示“回答/建议”则拒绝；否则剥离前缀并校验剩余正文非空。
    const META_PREFIXES: &[&str] = &[
        "好的，以下是",
        "好的,以下是",
        "好的，整理如下",
        "当然，以下是",
        "当然，整理如下",
        "当然,整理如下",
        "以下是整理后的",
        "以下是润色后的",
        "以下是修改后的",
        "以下是翻译后的",
        "以下是翻译结果",
        "整理后的文本如下",
        "润色后的文本如下",
        "润色结果如下",
        "翻译结果如下",
        "here is the ",
        "here's the ",
        "sure, here is",
        "sure, here's",
        "certainly, here is",
    ];
    let trimmed_start = text.trim_start();
    let trimmed_lower = trimmed_start.to_ascii_lowercase();
    for prefix in META_PREFIXES {
        if trimmed_lower.starts_with(prefix) && !source_has(prefix) {
            // 定位引导句末尾的冒号或换行。
            let header_end = trimmed_start.find('\n').unwrap_or(trimmed_start.len());
            let header = &trimmed_start[..header_end];
            let header_lower = header.to_ascii_lowercase();
            // 若模型把语音转写当问题来回答，直接拒收。
            if header_lower.contains("回答")
                || header_lower.contains("解答")
                || header_lower.contains("建议")
                || header_lower.contains("answer")
            {
                return MetaReplyOutcome::Reject;
            }
            let colon_pos = header.find('：').or_else(|| header.find(':'));
            let Some(pos) = colon_pos else {
                return MetaReplyOutcome::Reject;
            };
            let colon_len = if header[pos..].starts_with('：') {
                '：'.len_utf8()
            } else {
                1
            };
            let after_colon = &trimmed_start[pos + colon_len..];
            // 先裁掉冒号后同行水平空白（防 `"Here is the text: Hello"` 残留前导空格），紧跟 `\n` 时保留下下行缩进。
            let after_colon = after_colon.trim_start_matches([' ', '\t', '\u{00a0}', '\u{3000}']);
            let remainder = trim_blank_edges(after_colon);
            if remainder.trim().is_empty() {
                return MetaReplyOutcome::Reject;
            }
            // 递归复核剥离后的正文，防嵌套拒答。
            return filter_assistant_meta_reply(&remainder, raw_text, selected_text);
        }
    }

    MetaReplyOutcome::Accept(text.to_owned())
}

/// IM 场景清理模型误追加的邮件落款与 `[Your Name]` 类占位符。
fn strip_im_email_signature(text: &str, raw_text: &str, selected_text: Option<&str>) -> String {
    let source_has = |needle: &str| -> bool {
        raw_text.to_ascii_lowercase().contains(needle)
            || selected_text.is_some_and(|sel| sel.to_ascii_lowercase().contains(needle))
    };
    const PLACEHOLDERS: &[&str] = &[
        "[your name]",
        "[name]",
        "<your name>",
        "[你的名字]",
        "[您的姓名]",
        "[姓名]",
    ];
    const SIGNOFFS: &[&str] = &[
        "best regards,",
        "best regards",
        "kind regards,",
        "kind regards",
        "warm regards,",
        "warm regards",
        "regards,",
        "sincerely,",
        "thanks and regards,",
        "best,",
        "cheers,",
        "此致",
        "敬礼",
        "此致敬礼",
        "顺祝商祺",
    ];

    let mut lines: Vec<&str> = text.split('\n').collect();
    let mut changed = false;

    loop {
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        let Some(&last) = lines.last() else {
            break;
        };
        let last_lower = last.trim().to_ascii_lowercase();
        let is_placeholder = PLACEHOLDERS
            .iter()
            .any(|p| last_lower.contains(p) && !source_has(p));
        let is_signoff = SIGNOFFS.iter().any(|s| last_lower == *s && !source_has(s));
        // 处理单行合并写法如 `Best regards, [Your Name]`。
        let is_inline_signoff_placeholder = is_placeholder
            || (SIGNOFFS
                .iter()
                .any(|s| last_lower.starts_with(s) && !source_has(s))
                && PLACEHOLDERS.iter().any(|p| last_lower.contains(p)));
        if is_inline_signoff_placeholder || is_signoff {
            lines.pop();
            changed = true;
            continue;
        }
        break;
    }

    if !changed {
        return text.to_owned();
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// 检测 LLM 输出末尾 4~40 字符子串连续重复 `>= 4` 次的复读死循环。
fn has_llm_repetition_loop(text: &str, raw_text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 16 {
        return false;
    }
    let tail_len = chars.len().min(200);
    let tail = &chars[chars.len() - tail_len..];
    let max_unit = (tail.len() / 4).min(40);
    for unit_len in 4..=max_unit {
        let unit = &tail[tail.len() - unit_len..];
        if unit
            .iter()
            .all(|ch| ch.is_whitespace() || ch.is_ascii_punctuation())
        {
            continue;
        }
        let mut count = 1usize;
        let mut pos = tail.len() - unit_len;
        while pos >= unit_len {
            if &tail[pos - unit_len..pos] == unit {
                count += 1;
                pos -= unit_len;
            } else {
                break;
            }
        }
        if count >= 4 {
            let unit_str: String = unit.iter().collect();
            let repeated_4 = unit_str.repeat(4);
            // 原文自身不含该 4 连复读才判为模型循环幻觉。
            if !raw_text.contains(&repeated_4) {
                return true;
            }
        }
    }
    false
}

/// 按模式分级检查输出长度膨胀与过度塌缩。
fn check_length_guard(
    output: &str,
    raw_text: &str,
    selected_text: Option<&str>,
    mode: VoiceMode,
) -> Option<&'static str> {
    let raw_chars = raw_text.trim().chars().count();
    let out_chars = output.trim().chars().count();
    let selected_chars = selected_text.map_or(0, |sel| sel.trim().chars().count());

    match mode {
        VoiceMode::Dictation => {
            // 对齐蓝图 B.2：`max(raw_chars * 2.2 + 40, 120)`，为热词展开、Pangu 空格与列表排版留足安全余量。
            let bloat_limit = (((raw_chars as f64) * 2.2 + 40.0).ceil() as usize).max(120);
            if out_chars > bloat_limit {
                return Some("length_bloat");
            }
            if raw_chars >= 20 && out_chars * 4 < raw_chars && !has_self_deletion_intent(raw_text) {
                return Some("length_collapse");
            }
            None
        }
        VoiceMode::RewriteSelection => {
            let source_chars = raw_chars.max(selected_chars);
            let bloat_limit = source_chars
                .saturating_mul(12)
                .saturating_add(200)
                .max(1_200);
            if out_chars > bloat_limit {
                return Some("length_bloat");
            }
            None
        }
        VoiceMode::Translate => {
            let source_chars = raw_chars.max(selected_chars);
            let bloat_limit = source_chars.saturating_mul(4).saturating_add(120).max(400);
            if out_chars > bloat_limit {
                return Some("length_bloat");
            }
            if raw_chars >= 40 && out_chars < 4 {
                return Some("length_collapse");
            }
            None
        }
    }
}

fn has_self_deletion_intent(raw_text: &str) -> bool {
    const SELF_DELETE_CUES: &[&str] = &[
        "算了",
        "删掉",
        "删除",
        "取消",
        "重说",
        "重新说",
        "前面不要",
        "前面不算",
        "别写了",
        "全部删",
        "不对，重新",
        "不对重新",
        "作废",
        "忘了吧",
        "scratch that",
        "never mind",
        "nevermind",
        "cancel that",
        "delete that",
    ];
    let lower = raw_text.to_ascii_lowercase();
    SELF_DELETE_CUES.iter().any(|cue| lower.contains(cue))
}

/// 属于标准简体规范字但同时出现在 OpenCC `STCharacters` 繁体侧的字形白名单，避免单字反查误改（如 `覆盖`、`著名`、`乾坤`）。
fn is_simplified_valid_ambiguous_char(ch: char) -> bool {
    matches!(
        ch,
        '著' | '覆' | '乾' | '藉' | '徵' | '昇' | '夥' | '釐' | '於'
    )
}

/// 检测文本是否已明确包含无歧义繁体字。
fn contains_strict_traditional(text: &str) -> bool {
    let converted = traditional_to_simplified_characters(text);
    text.chars().zip(converted.chars()).any(|(orig, simp)| {
        orig != simp && is_han_character(orig) && !is_simplified_valid_ambiguous_char(orig)
    })
}

/// 简体中文锁：当原始输入不含繁体时，将模型意外漂移出的无歧义繁体字拉回简体，同时保护原文既有字形与简体合法字。
fn enforce_simplified_lock(text: &str, raw_text: &str, selected_text: Option<&str>) -> String {
    if !text.chars().any(is_han_character) {
        return text.to_owned();
    }
    if contains_strict_traditional(raw_text)
        || selected_text.is_some_and(contains_strict_traditional)
    {
        return text.to_owned();
    }
    let converted = traditional_to_simplified_characters(text);
    if converted == text {
        return text.to_owned();
    }
    let orig_chars: Vec<char> = text.chars().collect();
    let conv_chars: Vec<char> = converted.chars().collect();
    if orig_chars.len() != conv_chars.len() {
        return text.to_owned();
    }
    let mut source_chars: HashSet<char> = raw_text.chars().collect();
    if let Some(sel) = selected_text {
        source_chars.extend(sel.chars());
    }
    let mut out = String::with_capacity(text.len());
    for (orig, simp) in orig_chars.into_iter().zip(conv_chars) {
        if orig != simp
            && is_han_character(orig)
            && !is_simplified_valid_ambiguous_char(orig)
            && !source_chars.contains(&orig)
        {
            out.push(simp);
        } else {
            out.push(orig);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(raw: &str, polished: &str) -> PolishCleanInput {
        PolishCleanInput {
            raw_text: raw.to_owned(),
            polished_text: polished.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn strips_think_blocks_and_rejects_unclosed_think() {
        let out = clean_polish_output(&req(
            "明天下午四点开会",
            "<think>先检查时间。\n再确认措辞。</think>\n明天下午 4 点开会。",
        ));
        assert!(out.accepted);
        assert_eq!(out.text, "明天下午 4 点开会");

        let multi = clean_polish_output(&req(
            "整理一下两个要点",
            "<THINK>推理1</THINK>整理一下两个要点<think>推理2</think>",
        ));
        assert!(multi.accepted);
        assert_eq!(multi.text, "整理一下两个要点");

        let orphan = clean_polish_output(&req("测试孤立标签", "</think>\n测试孤立标签"));
        assert!(orphan.accepted);
        assert_eq!(orphan.text, "测试孤立标签");

        let unclosed = clean_polish_output(&req(
            "帮我整理",
            "<think>正在思考如何整理但被长度上限截断没有闭合",
        ));
        assert!(!unclosed.accepted);
        assert!(unclosed.text.is_empty());
        assert_eq!(unclosed.rejection_reason.as_deref(), Some("unclosed_think"));
    }

    #[test]
    fn resolves_action_tag_and_falls_back_safely_when_missing() {
        let mut rewrite = req("把这段精简一下", "<action>rewrite</action>\n精简后的结果。");
        rewrite.mode = VoiceMode::RewriteSelection;
        rewrite.expect_action_tag = true;
        rewrite.selected_text = Some("这是一段非常冗长需要被精简处理的示例文案内容。".to_owned());
        let out = clean_polish_output(&rewrite);
        assert!(out.accepted);
        assert_eq!(out.resolved_action, ResolvedAction::ReplaceSelection);
        assert_eq!(out.text, "精简后的结果");

        let mut dictate = req(
            "顺便提醒我买咖啡",
            "<action>dictate</action>\n顺便提醒我买咖啡",
        );
        dictate.mode = VoiceMode::RewriteSelection;
        dictate.expect_action_tag = true;
        dictate.selected_text = Some("无关选区内容".to_owned());
        let out = clean_polish_output(&dictate);
        assert!(out.accepted);
        assert_eq!(out.resolved_action, ResolvedAction::Insert);
        assert_eq!(out.text, "顺便提醒我买咖啡");

        // 歧义态缺失 `<action>` 标签时安全回退为 `Insert`，避免误覆写选区。
        let mut missing = req("顺便提醒我买咖啡", "顺便提醒我买咖啡");
        missing.mode = VoiceMode::RewriteSelection;
        missing.expect_action_tag = true;
        missing.selected_text = Some("已有选区".to_owned());
        let out = clean_polish_output(&missing);
        assert!(out.accepted);
        assert_eq!(out.resolved_action, ResolvedAction::Insert);

        // 显式改写模式（`expect_action_tag = false`）恒为 `ReplaceSelection`，即使模型泄漏 `<action>dictate</action>` 也不降级。
        let mut explicit = req("改短一点", "<action>dictate</action>\n改短后的内容");
        explicit.mode = VoiceMode::RewriteSelection;
        explicit.expect_action_tag = false;
        explicit.selected_text = Some("一段待改写的原始文本内容。".to_owned());
        let out = clean_polish_output(&explicit);
        assert!(out.accepted);
        assert_eq!(out.resolved_action, ResolvedAction::ReplaceSelection);
        assert_eq!(out.text, "改短后的内容");

        // 普通听写（`expect_action_tag = false`）恒为 `Insert`，绝不被幻觉 `<action>rewrite</action>` 翻转。
        let leaked_dictation = clean_polish_output(&req(
            "今天天气很好",
            "<action>rewrite</action>\n今天天气很好。",
        ));
        assert!(leaked_dictation.accepted);
        assert_eq!(leaked_dictation.resolved_action, ResolvedAction::Insert);
        assert_eq!(leaked_dictation.text, "今天天气很好");

        // 原文自身提到 `<action>rewrite</action>` 字面量时不得被剥离。
        let literal_action = clean_polish_output(&req(
            "文档里写了<action>rewrite</action>标签",
            "文档里写了 <action>rewrite</action> 标签",
        ));
        assert!(literal_action.accepted);
        assert_eq!(literal_action.resolved_action, ResolvedAction::Insert);
        assert!(literal_action.text.contains("<action>rewrite</action>"));
    }

    #[test]
    fn strips_fences_xml_wrappers_and_decorative_quotes_safely() {
        let fenced = clean_polish_output(&req(
            "帮我把这句话理顺",
            "```markdown\n<output>帮我把这句话理顺。</output>\n```",
        ));
        assert!(fenced.accepted);
        assert_eq!(fenced.text, "帮我把这句话理顺");

        let quoted = clean_polish_output(&req("今天先这样吧", "“今天先这样吧”"));
        assert!(quoted.accepted);
        assert_eq!(quoted.text, "今天先这样吧");

        // 含内部成对引号的句子不得误剥首尾引号。
        let inner_quotes = clean_polish_output(&req("甲和乙的区别", "“甲”和“乙”的区别"));
        assert!(inner_quotes.accepted);
        assert_eq!(inner_quotes.text, "“甲”和“乙”的区别");

        // 选区本身含代码围栏的改写保留围栏且不破坏内部未缩进代码字面量。
        let mut code_rewrite = req("把变量名改清晰", "```rust\nlet api = \"你好rust\";\n```");
        code_rewrite.mode = VoiceMode::RewriteSelection;
        code_rewrite.selected_text = Some("```rust\nlet x = \"你好rust\";\n```".to_owned());
        let out = clean_polish_output(&code_rewrite);
        assert!(out.accepted);
        assert_eq!(out.text, "```rust\nlet api = \"你好rust\";\n```");
    }

    #[test]
    fn detects_assistant_refusals_meta_replies_and_im_email_signatures() {
        let refusal = clean_polish_output(&req(
            "帮我整理这句话",
            "作为人工智能助手，我无法满足您的请求。",
        ));
        assert!(!refusal.accepted);
        assert_eq!(
            refusal.rejection_reason.as_deref(),
            Some("assistant_meta_reply")
        );

        let answer_leak = clean_polish_output(&req(
            "这个错误怎么排查",
            "好的，以下是关于这个问题的回答：首先检查日志。",
        ));
        assert!(!answer_leak.accepted);
        assert_eq!(
            answer_leak.rejection_reason.as_deref(),
            Some("assistant_meta_reply")
        );

        let empty_preamble =
            clean_polish_output(&req("今天测试通过", "Sure, here is the polished text:"));
        assert!(!empty_preamble.accepted);
        assert_eq!(
            empty_preamble.rejection_reason.as_deref(),
            Some("assistant_meta_reply")
        );

        // 无害前缀后跟真实正文时自动剥离前缀（含同行冒号后空格不留前导空格）。
        let stripped = clean_polish_output(&req(
            "今天测试全部通过了",
            "好的，以下是整理后的文本：\n今天测试全部通过了。",
        ));
        assert!(stripped.accepted);
        assert_eq!(stripped.text, "今天测试全部通过了");

        let same_line_preamble = clean_polish_output(&req(
            "hello world",
            "Sure, here is the polished text: Hello world.",
        ));
        assert!(same_line_preamble.accepted);
        assert_eq!(same_line_preamble.text, "Hello world.");

        // IM 场景清除误加的邮件落款与 `[Your Name]` 占位符。
        let mut im_reply = req(
            "礼貌婉拒这个合作邀约",
            "Thanks for reaching out, I'll have to pass this time.\n\nBest regards,\n[Your Name]",
        );
        im_reply.app_category = AppCategory::Im;
        im_reply.mode = VoiceMode::RewriteSelection;
        im_reply.selected_text = Some("Would you like to join our campaign?".to_owned());
        let out = clean_polish_output(&im_reply);
        assert!(out.accepted);
        assert_eq!(
            out.text,
            "Thanks for reaching out, I'll have to pass this time."
        );
    }

    #[test]
    fn enforces_mode_specific_length_guards_and_repetition_loop_rejection() {
        // Dictation 膨胀拦截与合法适度展开（<= 2.2x + 40）放行（用非重复长文触发长度守卫而非复读循环）。
        let bloat = clean_polish_output(&req(
            "帮我查一下",
            "第一段说明当前系统架构的整体背景、核心组件职责、依赖边界以及跨平台共享层的模块划分方式。第二段详细列举配置校验、缓存失效、并发锁治理、重试退避策略与日志脱敏等工程细节。第三段继续补充性能基准、延迟分位数、内存预算与异常降级路径的完整实施方案。",
        ));
        assert!(!bloat.accepted);
        assert_eq!(bloat.rejection_reason.as_deref(), Some("length_bloat"));

        let moderate_expand = clean_polish_output(&req(
            "这个接口有三个改动第一补词表第二修缩进第三加测试最后跑全量验证通过",
            "这个接口本次主要有以下三个方面的改动：\n1. 补齐个人跨语种词表、别名替换与常见技术术语大小写规范。\n2. 修复 Pangu 空格与行级清洗吞掉行首缩进的问题。\n3. 补充完整的边界回归单元测试，并运行全量验证确认通过。",
        ));
        assert!(moderate_expand.accepted);

        // Dictation 无自删意图的塌缩拦截。
        let collapse = clean_polish_output(&req(
            "我们今天下午三点在会议室讨论下个季度的架构重构方案和里程碑安排",
            "好的",
        ));
        assert!(!collapse.accepted);
        assert_eq!(
            collapse.rejection_reason.as_deref(),
            Some("length_collapse")
        );

        // Dictation 含口语自删意图时放行短结果。
        let self_cancel = clean_polish_output(&req(
            "明天下午三点开会讨论部署细节，算了前面全部删掉，改成周四",
            "改成周四",
        ));
        assert!(self_cancel.accepted);
        assert_eq!(self_cancel.text, "改成周四");

        // RewriteSelection 放宽膨胀上限至 `max(source*12+200, 1200)`（构造非重复长扩写，避免命中复读检测）。
        let expanded_rewrite: String = (1..=30)
            .map(|i| format!("第 {i} 条离线运行保障措施涵盖本地权重校验与异常回退机制。"))
            .collect();
        let mut rewrite = req("把这句扩写成完整说明", &expanded_rewrite);
        rewrite.mode = VoiceMode::RewriteSelection;
        rewrite.selected_text = Some("支持离线模式".to_owned());
        assert!(clean_polish_output(&rewrite).accepted);

        // Translate 放宽膨胀上限至 `max(source*4+120, 400)`。
        let mut translate = req(
            "请帮我确认这次发布的配置文件是否已经同步更新完毕",
            "Please help me confirm whether the configuration files for this release have all been synchronized and updated completely.",
        );
        translate.mode = VoiceMode::Translate;
        assert!(clean_polish_output(&translate).accepted);

        // 末尾复读死循环拦截。
        let repeated = clean_polish_output(&req(
            "继续推进这个任务",
            "继续推进这个任务，下一步检查日志，下一步检查日志，下一步检查日志，下一步检查日志，下一步检查日志",
        ));
        assert!(!repeated.accepted);
        assert_eq!(
            repeated.rejection_reason.as_deref(),
            Some("repetition_loop")
        );
    }

    #[test]
    fn locks_simplified_chinese_and_preserves_git_diff_indentation() {
        // 原文为简体，模型漂移出繁体时自动拉回简体（字形级简繁转换）。
        let drifted = clean_polish_output(&req(
            "这个软件的设定问题已经修复",
            "這個軟件的設定問題已經修復。",
        ));
        assert!(drifted.accepted);
        assert_eq!(drifted.text, "这个软件的设定问题已经修复");

        // 简体合法字 `覆`、`著` 不得被误转为 `复`、`着`。
        let valid_simp = clean_polish_output(&req(
            "这个功能会覆盖原有著名配置",
            "这个功能会覆盖原有著名配置。",
        ));
        assert!(valid_simp.accepted);
        assert_eq!(valid_simp.text, "这个功能会覆盖原有著名配置");

        // 原文自身为繁体时尊重原文不强改。
        let orig_trad = clean_polish_output(&req("這個軟體的設定問題", "這個軟體的設定問題。"));
        assert!(orig_trad.accepted);
        assert_eq!(orig_trad.text, "這個軟體的設定問題");

        // 保结构选区改写严格保留 Git diff 4 空格缩进。
        let diff_out = "    context line\n-    old message\n+    translated message";
        let mut diff_req = req(
            "翻译里面的英文注释",
            &format!("<action>rewrite</action>\n{diff_out}"),
        );
        diff_req.mode = VoiceMode::RewriteSelection;
        diff_req.expect_action_tag = true;
        diff_req.selected_text = Some(diff_out.to_owned());
        let cleaned = clean_polish_output(&diff_req);
        assert!(cleaned.accepted);
        assert_eq!(cleaned.resolved_action, ResolvedAction::ReplaceSelection);
        assert_eq!(cleaned.text, diff_out);
    }
}
