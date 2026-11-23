//! 共享 Rust 确定性语音后处理管线（无网络、无状态纯函数）。
//!
//! 负责在关闭 LLM 润色、润色超时降级以及润色清洗后兜底时，提供稳定一致的文本质量：
//! 1. 保留行首 `[ \t]+` 缩进的空白清洗与块边界伪句号单字结巴/句尾孤立语气词清理；
//! 2. 纯标点/语气词、复读死循环与短音频静音幻觉拦截；
//! 3. 拆散字母缩写、拆分品牌与口述符号（下划线、域名点）合并；
//! 4. 中文语境标点与直引号 `""`→`“”` 规整（保护 URL、时间、小数、代码与 Git diff）；
//! 5. 双边界保守智能数字 ITN（日期、年份、百分比、货币、版本号小数、`ESI幺`、`>=10` 精确计数，绝对保护自然中文小数字与成语）；
//! 6. 跨语种个人词表确定性替换（最高优先级）+ 内置高频技术词大小写规范表；
//! 7. 标准 Pangu 中英空格 + 双正交句末句号省略律（IM 全长抑制 / 非 IM 短句抑制 / 翻译保留 `.`）。

use super::numbers;
use super::vocabulary::{self, VocabularyTerm};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 复用下沉至 `msime_client_base::text` 的汉字与 Pangu 空格原语，并对外重新导出供宿主层共用。
pub use msime_client_base::text::{
    apply_pangu_space, is_han_ascii_alphanumeric_boundary, is_han_character, pangu_space,
    space_han_ascii_alphanumeric,
};

/// 宿主应用场景分类，与上下文路由及 C ABI 契约保持逐字节一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Im,
    TerminalIde,
    AiChat,
    BrowserDoc,
    Mail,
    #[default]
    General,
}

impl AppCategory {
    /// 返回与 C ABI JSON 契约一致的稳定小写标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Im => "im",
            Self::TerminalIde => "terminal_ide",
            Self::AiChat => "ai_chat",
            Self::BrowserDoc => "browser_doc",
            Self::Mail => "mail",
            Self::General => "general",
        }
    }
}

/// 语音处理模式，与 C ABI 契约一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    #[default]
    Dictation,
    RewriteSelection,
    Translate,
}

/// 双正交句末句号策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrailingPeriodPolicy {
    #[default]
    Auto,
    Keep,
    StripShort,
    StripIm,
}

fn default_true() -> bool {
    true
}

/// 确定性语音后处理输入请求，字段与 `msime_client_voice_postprocess` JSON 契约完全对齐。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostprocessInput {
    pub text: String,
    #[serde(default)]
    pub model_kind: String,
    #[serde(default)]
    pub audio_ms: Option<u64>,
    #[serde(default)]
    pub mode: VoiceMode,
    #[serde(default)]
    pub app_category: AppCategory,
    #[serde(default = "default_true")]
    pub enable_pangu_spacing: bool,
    /// 为 `Some(true)` 时强制启用保守 ITN；为 `Some(false)` 时强制跳过；缺省时按 `model_kind` 判定。
    #[serde(default)]
    pub enable_number_itn: Option<bool>,
    #[serde(default = "default_true")]
    pub enable_tech_casing: bool,
    #[serde(default)]
    pub trailing_period_policy: TrailingPeriodPolicy,
    #[serde(default)]
    pub vocabulary: Vec<VocabularyTerm>,
}

impl Default for PostprocessInput {
    fn default() -> Self {
        Self {
            text: String::new(),
            model_kind: String::new(),
            audio_ms: None,
            mode: VoiceMode::Dictation,
            app_category: AppCategory::General,
            enable_pangu_spacing: true,
            enable_number_itn: None,
            enable_tech_casing: true,
            trailing_period_policy: TrailingPeriodPolicy::Auto,
            vocabulary: Vec::new(),
        }
    }
}

/// 确定性语音后处理输出，与 `msime_client_voice_postprocess` 响应 `value` 对齐。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostprocessOutput {
    pub text: String,
    pub dismissed: bool,
    pub dismissed_reason: Option<String>,
    pub changed: bool,
}

/// 短音频静音幻觉判定的时长阈值（毫秒）。
const SHORT_SILENCE_HALLUCINATION_MAX_MS: u64 = 2_500;
/// 非 IM 场景允许省略句末单个 `。` 的最大有效字符数。
const SHORT_SENTENCE_MAX_EFFECTIVE_CHARS: usize = 15;

/// 执行完整确定性语音后处理管线。
pub fn postprocess(input: &PostprocessInput) -> PostprocessOutput {
    // 1. 空白规范化（保留每行行首 `[ \t]+` 缩进，仅清理非法控制符与行尾空白）。
    let normalized = normalize_whitespace_preserving_indent(&input.text);
    if normalized.trim().is_empty() {
        return PostprocessOutput {
            text: String::new(),
            dismissed: true,
            dismissed_reason: Some("empty".to_owned()),
            changed: !input.text.is_empty(),
        };
    }

    let document_is_diff = looks_like_git_diff(&normalized);

    // 2. 清理块边界伪句号单字结巴与句尾孤立语气词残片（非 diff/代码文档）。
    let mut text = if document_is_diff {
        normalized
    } else {
        let unstuttered = clean_chunk_boundary_stutter(&normalized);
        strip_trailing_isolated_fillers(&unstuttered)
    };

    // 3. 幻觉与纯噪音判定（先于语义改写执行）。
    if is_filler_or_punctuation_only(&text) {
        return PostprocessOutput {
            text: String::new(),
            dismissed: true,
            dismissed_reason: Some("filler_only".to_owned()),
            changed: true,
        };
    }
    if is_short_silence_hallucination(&text, input.audio_ms) {
        return PostprocessOutput {
            text: String::new(),
            dismissed: true,
            dismissed_reason: Some("silence_hallucination".to_owned()),
            changed: true,
        };
    }
    match handle_repetition_loop(&text, input.audio_ms) {
        RepetitionOutcome::Dismiss => {
            return PostprocessOutput {
                text: String::new(),
                dismissed: true,
                dismissed_reason: Some("repetition_loop".to_owned()),
                changed: true,
            };
        }
        RepetitionOutcome::Keep(cleaned) => {
            text = cleaned;
        }
    }

    // 收集用户词表覆盖项（小写集合），确保内置品牌合并与大小写表的优先级严格低于用户词表。
    let user_overridden = collect_user_overridden_keys(&input.vocabulary);

    // 4. 拆散缩写、拆分品牌与口述符号合并（按行保护行首缩进与代码/diff 行）。
    text = transform_lines_preserving_indent(&text, document_is_diff, |body, protected| {
        if protected {
            return body.to_owned();
        }
        let step1 = repair_spoken_symbols(body);
        let step2 = merge_split_initialisms(&step1, &user_overridden);
        merge_split_brands(&step2, &user_overridden)
    });

    // 5. 中英标点与中文弯引号规整（保护 URL、小数、时间、CLI、纯英文与代码/diff 行）。
    text = transform_lines_preserving_indent(&text, document_is_diff, |body, protected| {
        if protected {
            return body.to_owned();
        }
        let punct = normalize_punctuation_line(body, input.mode);
        normalize_chinese_straight_quotes(&punct, input.mode, input.app_category)
    });

    // 6. 双边界保守智能数字 ITN（仅在启用或模型不自带 ITN 时执行，跳过代码/diff 行）。
    if should_run_itn(input) {
        text = transform_lines_preserving_indent(&text, document_is_diff, |body, protected| {
            if protected {
                body.to_owned()
            } else {
                apply_conservative_itn(body)
            }
        });
    }

    // 7. 跨语种用户词表确定性替换（最高优先级）+ 内置高频技术词大小写规范表。
    if !input.vocabulary.is_empty() {
        text = transform_lines_preserving_indent(&text, document_is_diff, |body, protected| {
            if protected {
                body.to_owned()
            } else {
                vocabulary::apply_deterministic_terms(body, &input.vocabulary)
            }
        });
    }
    if input.enable_tech_casing {
        let doc_has_han = text.chars().any(is_han_character);
        text = transform_lines_preserving_indent(&text, document_is_diff, |body, protected| {
            if protected {
                body.to_owned()
            } else {
                apply_builtin_tech_casing(body, &user_overridden, doc_has_han, input.app_category)
            }
        });
    }

    // 8. 标准 Pangu 中英空格（保留行首 `[ \t]+` 缩进，跳过受保护代码/围栏/diff 行，仅对正文主体处理并去行尾空格）。
    if input.enable_pangu_spacing {
        text = transform_lines_preserving_indent(&text, document_is_diff, |body, protected| {
            if protected {
                body.to_owned()
            } else {
                apply_pangu_space(body)
            }
        });
    }

    // 9. 双正交句末句号省略律（IM 场景全长去 `。`；非 IM 仅单段 <=15 有效字且无句中停顿去 `。`；Translate 保留 `.`）。
    if !document_is_diff {
        text = apply_trailing_period_policy(
            &text,
            input.trailing_period_policy,
            input.app_category,
            input.mode,
        );
    }

    let changed = text != input.text;
    PostprocessOutput {
        text,
        dismissed: false,
        dismissed_reason: None,
        changed,
    }
}

fn should_run_itn(input: &PostprocessInput) -> bool {
    match input.enable_number_itn {
        Some(true) => true,
        Some(false) => false,
        None => {
            let kind = input.model_kind.trim();
            kind.is_empty()
                || numbers::applies_to_kind(kind)
                || kind.eq_ignore_ascii_case("textstream")
                || kind.eq_ignore_ascii_case("qwen3_asr")
                || kind.eq_ignore_ascii_case("mlx_qwen3_asr")
        }
    }
}

fn collect_user_overridden_keys(terms: &[VocabularyTerm]) -> HashSet<String> {
    let mut keys = HashSet::with_capacity(terms.len() * 2);
    for term in terms {
        let canonical = term.canonical.trim();
        if !canonical.is_empty() {
            keys.insert(canonical.to_ascii_lowercase());
        }
        for alias in &term.aliases {
            let alias = alias.trim();
            if !alias.is_empty() {
                keys.insert(alias.to_ascii_lowercase());
            }
        }
    }
    keys
}

/// 拆分每行的行首缩进（仅 `[ \t]+`）与正文主体，严格保留行首缩进且只裁掉行尾空白。
fn split_leading_indent(line: &str) -> (&str, &str) {
    let indent_len = line
        .bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count();
    (&line[..indent_len], &line[indent_len..])
}

fn is_horizontal_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\u{00a0}' | '\u{3000}')
}

fn trim_end_horizontal(text: &str) -> &str {
    text.trim_end_matches(is_horizontal_space)
}

/// 空白规范化：统一换行符、剥离非法控制符与不可见占位符（`U+FFFC`/`U+FEFF`/零宽字符）、保留每行行首 `[ \t]+` 缩进、仅裁剪行尾空白与首尾纯空行。
fn normalize_whitespace_preserving_indent(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                cleaned.push('\n');
            }
            '\n' | '\t' => cleaned.push(ch),
            // 清理 QQ/富文本占位符、BOM、零宽字符与其余控制字符。
            '\u{fffc}' | '\u{feff}' | '\u{200b}'..='\u{200d}' | '\u{2060}' => {}
            c if c.is_control() => {}
            '\u{00a0}' => cleaned.push(' '),
            c => cleaned.push(c),
        }
    }

    let document_is_diff = looks_like_git_diff(&cleaned);
    let mut lines: Vec<String> = Vec::new();
    let mut blank_run = 0usize;
    let mut in_code_fence = false;

    for raw_line in cleaned.split('\n') {
        let is_fence_delim = raw_line.trim_start().starts_with("```");
        if raw_line.trim().is_empty() {
            if in_code_fence {
                lines.push(String::new());
                continue;
            }
            // 丢弃文首纯空行，并把文内连续 3 个以上换行折叠为单个空行 `\n\n`。
            if !lines.is_empty() && blank_run == 0 {
                lines.push(String::new());
            }
            blank_run += 1;
            continue;
        }
        blank_run = 0;
        let (indent, rest) = split_leading_indent(raw_line);
        let body = trim_end_horizontal(rest);
        let protected = in_code_fence
            || is_fence_delim
            || is_protected_code_or_diff_line(raw_line, document_is_diff);
        if is_fence_delim {
            in_code_fence = !in_code_fence;
        }
        let normalized_body = if protected {
            body.to_owned()
        } else {
            collapse_interior_spaces(body)
        };
        let mut line = String::with_capacity(indent.len() + normalized_body.len());
        line.push_str(indent);
        line.push_str(&normalized_body);
        lines.push(line);
    }

    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// 折叠正文内部的连续半角空格为单个空格（不触碰行首缩进）。
fn collapse_interior_spaces(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut prev_space = false;
    for ch in body.chars() {
        if ch == ' ' || ch == '\u{3000}' {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    trim_end_horizontal(&out).to_owned()
}

fn looks_like_git_diff(text: &str) -> bool {
    let mut diff_markers = 0usize;
    let mut has_plus = false;
    let mut has_minus = false;
    let mut plus_minus_lines = 0usize;
    for line in text.lines() {
        if line.starts_with("diff --git ")
            || line.starts_with("@@ ")
            || line.starts_with("+++ ")
            || line.starts_with("--- ")
        {
            diff_markers += 1;
        } else if line.starts_with("+  ") || line.starts_with("+\t") {
            has_plus = true;
            plus_minus_lines += 1;
        } else if line.starts_with("-  ") || line.starts_with("-\t") {
            has_minus = true;
            plus_minus_lines += 1;
        } else if line.starts_with('+') && !line.starts_with("+ ") {
            has_plus = true;
            plus_minus_lines += 1;
        } else if line.starts_with('-') && !line.starts_with("- ") {
            has_minus = true;
            plus_minus_lines += 1;
        }
    }
    diff_markers >= 1 || (has_plus && has_minus) || plus_minus_lines >= 3
}

fn is_protected_code_or_diff_line(raw_line: &str, document_is_diff: bool) -> bool {
    if raw_line.starts_with('\t')
        || raw_line.starts_with("    ")
        || raw_line.starts_with("```")
        || raw_line.starts_with("diff --git ")
        || raw_line.starts_with("@@ ")
        || raw_line.starts_with("+++ ")
        || raw_line.starts_with("--- ")
        || raw_line.starts_with("+  ")
        || raw_line.starts_with("-  ")
        || raw_line.starts_with("+\t")
        || raw_line.starts_with("-\t")
    {
        return true;
    }
    if document_is_diff
        && (raw_line.starts_with('+') || raw_line.starts_with('-') || raw_line.starts_with(' '))
    {
        return true;
    }
    false
}

/// 按行执行正文变换：保留行首 `[ \t]+` 缩进并追踪围栏代码块，仅对正文主体变换并裁剪行尾空白。
fn transform_lines_preserving_indent(
    text: &str,
    document_is_diff: bool,
    mut transform: impl FnMut(&str, bool) -> String,
) -> String {
    if !text.contains('\n') {
        let (indent, rest) = split_leading_indent(text);
        let protected = text.trim_start().starts_with("```")
            || is_protected_code_or_diff_line(text, document_is_diff);
        let transformed = transform(trim_end_horizontal(rest), protected);
        let mut out = String::with_capacity(indent.len() + transformed.len());
        out.push_str(indent);
        out.push_str(trim_end_horizontal(&transformed));
        return out;
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut in_code_fence = false;
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            continue;
        }
        let is_fence_delim = line.trim_start().starts_with("```");
        let protected = in_code_fence
            || is_fence_delim
            || is_protected_code_or_diff_line(line, document_is_diff);
        if is_fence_delim {
            in_code_fence = !in_code_fence;
        }
        let (indent, rest) = split_leading_indent(line);
        let transformed = transform(trim_end_horizontal(rest), protected);
        out.push_str(indent);
        out.push_str(trim_end_horizontal(&transformed));
    }
    out
}

/// 允许在句中合并的块边界单字结巴前缀表（防止误合并正常跨句同字，如 `我喜欢吃鱼。鱼肉很鲜`）。
const STUTTER_PREFIX_WORDS: &[&str] = &[
    "上去", "上来", "上面", "下去", "下来", "下面", "可以", "可能", "自己", "自动", "哪个", "哪里",
    "哪儿", "这个", "这里", "这样", "这些", "那个", "那里", "那样", "那些", "就是", "因为", "所以",
    "但是", "如果", "或者", "然后", "应该", "需要", "已经", "正在", "比如", "特别", "直接", "确实",
    "基本", "突然", "我们", "你们", "他们", "她们", "它们", "要把", "要是", "要求", "能够", "想要",
    "没有", "不是", "不能", "不会", "不要", "不用", "还是", "还有", "也是", "也有", "都是", "只是",
    "只有", "一个", "一下", "一起", "一直", "一般", "一定",
];

/// 清理流式块边界产生的伪句号单字结巴与伪断词（如 `上。上去`→`上去`、`可。可以`→`可以`、`自。自己`→`自己`、`要。把`→`要把`、`突然。之间`→`突然之间`）。
fn clean_chunk_boundary_stutter(text: &str) -> String {
    // 先处理明确的固定块边界伪断词。
    let pre = text.replace("突然。之间", "突然之间");
    let chars: Vec<char> = pre.chars().collect();
    if chars.len() < 3 {
        return pre;
    }
    let mut out = String::with_capacity(pre.len());
    let mut index = 0;
    while index < chars.len() {
        // 检查 `<X>。<X><Y>` 单字结巴（X、Y 均为汉字且 X != Y）。
        if index + 3 < chars.len()
            && is_han_character(chars[index])
            && chars[index + 1] == '。'
            && chars[index + 2] == chars[index]
            && is_han_character(chars[index + 3])
            && chars[index + 3] != chars[index]
        {
            let isolated = index == 0 || is_clause_boundary_char(chars[index - 1]);
            let pair: String = [chars[index + 2], chars[index + 3]].into_iter().collect();
            if isolated || STUTTER_PREFIX_WORDS.contains(&pair.as_str()) {
                // 跳过冗余单字与伪句号，直接从 `index + 2` 的完整词继续。
                index += 2;
                continue;
            }
        }
        // 检查孤立单字 `要。把`（位于句首或停顿标点后）→ `要把`。
        if index + 2 < chars.len()
            && chars[index] == '要'
            && chars[index + 1] == '。'
            && chars[index + 2] == '把'
            && (index == 0 || is_clause_boundary_char(chars[index - 1]))
        {
            out.push('要');
            index += 2;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn is_clause_boundary_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '，' | '。' | '？' | '！' | '；' | '：' | '、' | ',' | '.' | '?' | '!' | ';' | ':'
        )
}

/// 清理句尾孤立的语气词残片（如 `。嗯。`、`。啊。`、`。呃。`、`？嗯。`）。
fn strip_trailing_isolated_fillers(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.split('\n') {
        lines.push(strip_trailing_isolated_fillers_line(line));
    }
    lines.join("\n")
}

fn strip_trailing_isolated_fillers_line(line: &str) -> String {
    const TRAILING_FILLERS: &[&str] = &["嗯嗯", "呃呃", "嗯", "啊", "呃", "哦", "哎", "额", "唉"];
    let mut current = line.to_owned();
    loop {
        let trimmed = trim_end_horizontal(&current);
        let (without_end_dot, had_end_dot) = if let Some(rest) = trimmed.strip_suffix('。') {
            (rest, true)
        } else if let Some(rest) = trimmed.strip_suffix('.') {
            if !rest.ends_with('.') {
                (rest, true)
            } else {
                (trimmed, false)
            }
        } else {
            (trimmed, false)
        };
        let without_end_dot = trim_end_horizontal(without_end_dot);
        let mut stripped = false;
        for filler in TRAILING_FILLERS {
            let Some(before_filler) = without_end_dot.strip_suffix(filler) else {
                continue;
            };
            let before_trimmed = trim_end_horizontal(before_filler);
            let Some(sep) = before_trimmed.chars().last() else {
                // 整句仅有语气词，留给后续 `filler_only` 判定，不在此抹空。
                continue;
            };
            let sep_len = sep.len_utf8();
            let prefix_before_sep = &before_trimmed[..before_trimmed.len() - sep_len];
            if prefix_before_sep.trim().is_empty() {
                continue;
            }
            if matches!(sep, '。' | '？' | '！' | '?' | '!') {
                current = before_trimmed.to_owned();
                stripped = true;
                break;
            }
            if matches!(sep, '，' | ',') {
                current = if had_end_dot {
                    format!("{prefix_before_sep}。")
                } else {
                    prefix_before_sep.to_owned()
                };
                stripped = true;
                break;
            }
            if before_filler.len() != before_trimmed.len() && had_end_dot {
                // 例如 `这样吧 嗯。` → `这样吧。`
                current = format!("{before_trimmed}。");
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    current
}

fn is_cjk_or_ascii_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。'
                | '？'
                | '！'
                | '；'
                | '：'
                | '、'
                | '（'
                | '）'
                | '《'
                | '》'
                | '【'
                | '】'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '…'
                | '—'
                | '·'
                | '～'
                | '「'
                | '」'
                | '『'
                | '』'
        )
}

/// 判定文本是否仅由标点或纯单字语气词组成（如 `嗯。`、`啊`、`呃`、`哦`、`哎`）。
fn is_filler_or_punctuation_only(text: &str) -> bool {
    let stripped: String = text
        .chars()
        .filter(|ch| !ch.is_whitespace() && !is_cjk_or_ascii_punctuation(*ch))
        .collect();
    if stripped.is_empty() {
        return true;
    }
    let chars: Vec<char> = stripped.chars().collect();
    // 单字纯语气词（排除 `哈` 等真实短回复）。
    if chars.len() == 1
        && matches!(
            chars[0],
            '嗯' | '啊' | '呃' | '哦' | '哎' | '额' | '唉' | '噢' | '唔'
        )
    {
        return true;
    }
    // 连续 2~3 个纯犹豫音（如 `呃呃`、`额额`）。
    if (2..=3).contains(&chars.len()) && chars.iter().all(|ch| matches!(ch, '呃' | '额' | '唔'))
    {
        return true;
    }
    let lower = stripped.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "um" | "uh" | "erm" | "hmm" | "hm" | "mm" | "ah" | "oh" | "eh"
    )
}

/// 短音频（`< 2500ms`）静音幻觉黑名单判定。
fn is_short_silence_hallucination(text: &str, audio_ms: Option<u64>) -> bool {
    let Some(ms) = audio_ms else {
        return false;
    };
    if ms >= SHORT_SILENCE_HALLUCINATION_MAX_MS {
        return false;
    }
    let trimmed = text
        .trim()
        .trim_matches(|ch: char| ch.is_whitespace() || is_cjk_or_ascii_punctuation(ch));
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.starts_with("字幕由") || trimmed.starts_with("字幕制作") {
        return true;
    }
    const ZH_BLACKLIST: &[&str] = &[
        "字幕由",
        "字幕组",
        "中文字幕",
        "感谢观看",
        "谢谢观看",
        "感谢收看",
        "谢谢收看",
        "请订阅",
        "请点赞",
        "欢迎订阅",
        "订阅频道",
        "点赞关注",
        "请不吝点赞",
    ];
    if ZH_BLACKLIST.contains(&trimmed) {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    const EN_BLACKLIST: &[&str] = &[
        "thank you for watching",
        "thanks for watching",
        "please subscribe",
        "subscribe to my channel",
        "like and subscribe",
        "subtitles by",
        "captioned by",
        "subtitles provided by",
        "amara.org",
    ];
    EN_BLACKLIST
        .iter()
        .any(|phrase| lower == *phrase || lower.starts_with(phrase))
}

enum RepetitionOutcome {
    Dismiss,
    Keep(String),
}

/// 检测连续重复 `>= 4` 次的编码器复读幻觉；占比 `>= 60%` 且无有效前缀时判 `repetition_loop`，否则压缩尾部复读。
fn handle_repetition_loop(text: &str, audio_ms: Option<u64>) -> RepetitionOutcome {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 4 {
        return RepetitionOutcome::Keep(text.to_owned());
    }

    // 扫描最长的连续重复片段（允许重复单元间夹杂单个逗号/句号/空格）。
    let mut best: Option<(usize, usize, usize, usize)> = None; // (start, unit_end, run_end, covered)
    for start in 0..chars.len() {
        let max_unit = ((chars.len() - start) / 4).min(40);
        for unit_len in 1..=max_unit {
            let unit = &chars[start..start + unit_len];
            if is_exempt_repetition_unit(unit) {
                continue;
            }
            let (count, run_end) = count_consecutive_repeats(&chars, start, unit_len);
            if count >= 4 {
                let covered = run_end - start;
                if best.is_none_or(|(_, _, _, best_cov)| covered > best_cov) {
                    best = Some((start, start + unit_len, run_end, covered));
                }
            }
        }
    }

    let Some((start, unit_end, run_end, covered)) = best else {
        return RepetitionOutcome::Keep(text.to_owned());
    };

    let ratio_ge_60 = covered * 100 >= chars.len() * 60;
    let prefix_meaningful = chars[..start]
        .iter()
        .filter(|&&ch| !ch.is_whitespace() && !is_cjk_or_ascii_punctuation(ch))
        .count();
    let suffix_meaningful = chars[run_end..]
        .iter()
        .filter(|&&ch| !ch.is_whitespace() && !is_cjk_or_ascii_punctuation(ch))
        .count();

    if ratio_ge_60
        && (prefix_meaningful < 4 && suffix_meaningful < 4
            || (audio_ms.is_some_and(|ms| ms < 3_000) && covered * 100 >= chars.len() * 80))
    {
        return RepetitionOutcome::Dismiss;
    }

    // 有实词内容时将复读段压缩为单次出现，保留有效正文。
    if ratio_ge_60 || run_end + 3 >= chars.len() {
        let mut compressed = String::with_capacity(text.len());
        for &ch in &chars[..unit_end] {
            compressed.push(ch);
        }
        for &ch in &chars[run_end..] {
            compressed.push(ch);
        }
        return RepetitionOutcome::Keep(compressed);
    }

    RepetitionOutcome::Keep(text.to_owned())
}

fn is_exempt_repetition_unit(unit: &[char]) -> bool {
    if unit
        .iter()
        .all(|ch| ch.is_whitespace() || is_cjk_or_ascii_punctuation(*ch))
    {
        return true;
    }
    if unit.len() == 1 {
        let ch = unit[0];
        // 避免误伤大数字 `100000` 及自然笑声/拟声词 `哈哈哈哈`。
        if ch.is_ascii_digit()
            || matches!(ch, '哈' | '呵' | '嘿' | '嘻' | '哇' | '呜' | '呀' | '啊')
        {
            return true;
        }
    }
    false
}

fn count_consecutive_repeats(chars: &[char], start: usize, unit_len: usize) -> (usize, usize) {
    let unit = &chars[start..start + unit_len];
    let mut count = 1usize;
    let mut pos = start + unit_len;
    while pos < chars.len() {
        let mut next = pos;
        // 仅允许单字分隔符（空格或常见停顿标点）隔开的复读，如 `好的，好的，好的，好的`。
        while next < chars.len()
            && next - pos < 2
            && matches!(chars[next], ' ' | '，' | '。' | '、' | ',' | '.')
        {
            next += 1;
        }
        if next + unit_len <= chars.len() && &chars[next..next + unit_len] == unit {
            count += 1;
            pos = next + unit_len;
        } else {
            break;
        }
    }
    (count, pos)
}

/// 合并口述下划线、短横杠、斜杠标识符（如 `Account下划线Identity` → `account_identity`、
/// `Qwen3 杠 ASR` → `Qwen3-ASR`、`src 斜杠 main` → `src/main`）及明确域名点号（`Max点儿example点儿com` → `max.example.com`）。
fn repair_spoken_symbols(text: &str) -> String {
    let after_underscore = repair_spoken_underscores(text);
    let after_hyphen =
        repair_spoken_ascii_connector(&after_underscore, &["短横杠", "横杠", "杠"], '-');
    let after_slash = repair_spoken_ascii_connector(&after_hyphen, &["斜杠"], '/');
    repair_spoken_domain_dots(&after_slash)
}

/// 仅当两侧都是 ASCII 字母/数字片段时，把口述连接词（如「杠」「横杠」「斜杠」）合并为对应半角符号；
/// 自然中文词（如「抬杠」「杠杆」「某某杠某某」）因两侧非 ASCII 片段而完全不受影响。
fn repair_spoken_ascii_connector(text: &str, spoken_tokens: &[&str], symbol: char) -> String {
    if !spoken_tokens.iter().any(|tok| text.contains(tok)) {
        return text.to_owned();
    }
    let token_chars: Vec<Vec<char>> = spoken_tokens
        .iter()
        .map(|tok| tok.chars().collect())
        .collect();
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphanumeric() {
            let start = i;
            let mut end = i;
            while end < chars.len()
                && (chars[end].is_ascii_alphanumeric()
                    || chars[end] == symbol
                    || chars[end] == '_'
                    || chars[end] == '.')
            {
                end += 1;
            }
            let mut segments: Vec<String> = vec![chars[start..end].iter().collect()];
            let mut cursor = end;
            let mut matched_any = false;
            loop {
                let mut probe = cursor;
                while probe < chars.len() && chars[probe] == ' ' {
                    probe += 1;
                }
                let matched_len = token_chars.iter().find_map(|tok| {
                    if probe + tok.len() <= chars.len()
                        && tok
                            .iter()
                            .enumerate()
                            .all(|(off, &c)| chars[probe + off] == c)
                    {
                        Some(tok.len())
                    } else {
                        None
                    }
                });
                let Some(tok_len) = matched_len else {
                    break;
                };
                probe += tok_len;
                while probe < chars.len() && chars[probe] == ' ' {
                    probe += 1;
                }
                if probe < chars.len() && chars[probe].is_ascii_alphanumeric() {
                    let seg_start = probe;
                    while probe < chars.len()
                        && (chars[probe].is_ascii_alphanumeric()
                            || chars[probe] == symbol
                            || chars[probe] == '_'
                            || chars[probe] == '.')
                    {
                        probe += 1;
                    }
                    segments.push(chars[seg_start..probe].iter().collect());
                    cursor = probe;
                    matched_any = true;
                    continue;
                }
                break;
            }
            if matched_any {
                let sep = symbol.to_string();
                out.push_str(&segments.join(&sep));
                i = cursor;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn repair_spoken_underscores(text: &str) -> String {
    if !text.contains("下划线") {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphanumeric() {
            let start = i;
            let mut end = i;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let mut segments: Vec<String> = vec![chars[start..end].iter().collect()];
            let mut cursor = end;
            let mut matched_any = false;
            loop {
                let mut probe = cursor;
                while probe < chars.len() && chars[probe] == ' ' {
                    probe += 1;
                }
                if probe + 3 <= chars.len()
                    && chars[probe] == '下'
                    && chars[probe + 1] == '划'
                    && chars[probe + 2] == '线'
                {
                    probe += 3;
                    while probe < chars.len() && chars[probe] == ' ' {
                        probe += 1;
                    }
                    if probe < chars.len() && chars[probe].is_ascii_alphanumeric() {
                        let seg_start = probe;
                        while probe < chars.len()
                            && (chars[probe].is_ascii_alphanumeric() || chars[probe] == '_')
                        {
                            probe += 1;
                        }
                        segments.push(chars[seg_start..probe].iter().collect());
                        cursor = probe;
                        matched_any = true;
                        continue;
                    }
                }
                break;
            }
            if matched_any {
                let joined = segments
                    .into_iter()
                    .map(|seg| seg.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("_");
                out.push_str(&joined);
                i = cursor;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn repair_spoken_domain_dots(text: &str) -> String {
    if !text.contains('点') {
        return text.to_owned();
    }
    const TLDS: &[&str] = &["com", "org", "dev", "io", "ai", "cn", "net"];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphanumeric() {
            let start = i;
            let mut cursor = i;
            while cursor < chars.len()
                && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '-')
            {
                cursor += 1;
            }
            let mut labels: Vec<String> = vec![chars[start..cursor].iter().collect()];
            let mut end_cursor = cursor;
            let mut valid_domain = false;
            loop {
                let mut probe = end_cursor;
                while probe < chars.len() && chars[probe] == ' ' {
                    probe += 1;
                }
                if probe < chars.len() && chars[probe] == '点' {
                    probe += 1;
                    if probe < chars.len() && chars[probe] == '儿' {
                        probe += 1;
                    }
                    while probe < chars.len() && chars[probe] == ' ' {
                        probe += 1;
                    }
                    if probe < chars.len() && chars[probe].is_ascii_alphanumeric() {
                        let lbl_start = probe;
                        while probe < chars.len()
                            && (chars[probe].is_ascii_alphanumeric() || chars[probe] == '-')
                        {
                            probe += 1;
                        }
                        let label: String = chars[lbl_start..probe].iter().collect();
                        let lower = label.to_ascii_lowercase();
                        labels.push(label);
                        end_cursor = probe;
                        if TLDS.contains(&lower.as_str()) {
                            valid_domain = true;
                            break;
                        }
                        if labels.len() >= 4 {
                            break;
                        }
                        continue;
                    }
                }
                break;
            }
            if valid_domain {
                let domain = labels
                    .into_iter()
                    .map(|lbl| lbl.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(".");
                out.push_str(&domain);
                i = end_cursor;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 合并拆散字母缩写与点分缩写（如 `A P I K`/`APIK`→`API Key`、`A P I`→`API`、`B Y O K`→`BYOK`、`P.S.W`→`PSW`、`A.I`→`AI`）。
fn merge_split_initialisms(text: &str, user_overridden: &HashSet<String>) -> String {
    // 显式高收益缩写映射（按长度降序排列，严格执行 ASCII 词边界校验）。
    const EXPLICIT_PATTERNS: &[(&str, &str)] = &[
        ("open a p i", "OpenAPI"),
        ("open a.p.i", "OpenAPI"),
        ("h t t p s", "HTTPS"),
        ("h.t.t.p.s", "HTTPS"),
        ("a p i key", "API Key"),
        ("a.p.i.k", "API Key"),
        ("a p i k", "API Key"),
        ("b y o k", "BYOK"),
        ("b.y.o.k", "BYOK"),
        ("h t t p", "HTTP"),
        ("h.t.t.p", "HTTP"),
        ("h t m l", "HTML"),
        ("h.t.m.l", "HTML"),
        ("j s o n", "JSON"),
        ("j.s.o.n", "JSON"),
        ("y a m l", "YAML"),
        ("y.a.m.l", "YAML"),
        ("s a a s", "SaaS"),
        ("s.a.a.s", "SaaS"),
        ("g g u f", "GGUF"),
        ("g.g.u.f", "GGUF"),
        ("api k", "API Key"),
        ("a p i", "API"),
        ("a.p.i", "API"),
        ("c l i", "CLI"),
        ("c.l.i", "CLI"),
        ("o p s", "OPS"),
        ("o.p.s", "OPS"),
        ("b f f", "BFF"),
        ("b.f.f", "BFF"),
        ("v p s", "VPS"),
        ("v.p.s", "VPS"),
        ("m v p", "MVP"),
        ("m.v.p", "MVP"),
        ("m c p", "MCP"),
        ("m.c.p", "MCP"),
        ("s s h", "SSH"),
        ("s.s.h", "SSH"),
        ("p d f", "PDF"),
        ("p.d.f", "PDF"),
        ("c s v", "CSV"),
        ("c.s.v", "CSV"),
        ("u r l", "URL"),
        ("u.r.l", "URL"),
        ("d n f", "DNF"),
        ("d.n.f", "DNF"),
        ("d n s", "DNS"),
        ("d.n.s", "DNS"),
        ("p s w", "PSW"),
        ("p.s.w", "PSW"),
        ("s q l", "SQL"),
        ("s.q.l", "SQL"),
        ("l l m", "LLM"),
        ("l.l.m", "LLM"),
        ("a s r", "ASR"),
        ("a.s.r", "ASR"),
        ("v a d", "VAD"),
        ("v.a.d", "VAD"),
        ("t t s", "TTS"),
        ("t.t.s", "TTS"),
        ("s t t", "STT"),
        ("s.t.t", "STT"),
        ("s d k", "SDK"),
        ("s.d.k", "SDK"),
        ("m l x", "MLX"),
        ("m.l.x", "MLX"),
        ("c p u", "CPU"),
        ("c.p.u", "CPU"),
        ("g p u", "GPU"),
        ("g.p.u", "GPU"),
        ("r a m", "RAM"),
        ("r.a.m", "RAM"),
        ("s p a", "SPA"),
        ("s.p.a", "SPA"),
        ("apik", "API Key"),
        ("a.i.", "AI"),
        ("a.i", "AI"),
        ("u.i", "UI"),
        ("u.x", "UX"),
        ("p.r", "PR"),
        ("c.i", "CI"),
        ("c.d", "CD"),
        ("i.d", "ID"),
    ];

    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|ch| ch.to_ascii_lowercase()).collect();
    let mut first_pass = String::with_capacity(text.len());
    let mut i = 0;

    'outer: while i < chars.len() {
        for &(pat, replacement) in EXPLICIT_PATTERNS {
            if user_overridden.contains(&replacement.to_ascii_lowercase())
                || user_overridden.contains(pat)
            {
                continue;
            }
            let pat_len = pat.len();
            let end = i + pat_len;
            if end > chars.len() {
                continue;
            }
            let matched = pat
                .bytes()
                .enumerate()
                .all(|(off, b)| lower[i + off] as u32 == u32::from(b));
            if !matched {
                continue;
            }
            if !has_ascii_word_boundaries(&chars, i, end) {
                continue;
            }
            // `O P S` 若紧跟版本号（如 `O P S 4.5`），留给 LLM 结合语境判 `Opus`。
            if replacement == "OPS" && followed_by_version_number(&chars, end) {
                continue;
            }
            first_pass.push_str(replacement);
            i = end;
            continue 'outer;
        }
        first_pass.push(chars[i]);
        i += 1;
    }

    // 通用规则：合并 2~6 个以空格隔开的独立大写单字母（如 `A I`→`AI`、`U I`→`UI`、`G P T`→`GPT`、`A W S`→`AWS`）。
    merge_spaced_single_capitals(&first_pass, user_overridden)
}

fn followed_by_version_number(chars: &[char], pos: usize) -> bool {
    let mut cursor = pos;
    while cursor < chars.len() && chars[cursor] == ' ' {
        cursor += 1;
    }
    let digit_start = cursor;
    while cursor < chars.len() && chars[cursor].is_ascii_digit() {
        cursor += 1;
    }
    cursor > digit_start
        && chars.get(cursor) == Some(&'.')
        && chars.get(cursor + 1).is_some_and(char::is_ascii_digit)
}

fn merge_spaced_single_capitals(text: &str, user_overridden: &HashSet<String>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if is_standalone_capital(&chars, i) {
            let mut letters = vec![chars[i]];
            let mut cursor = i + 1;
            while letters.len() < 6
                && cursor + 1 < chars.len()
                && chars[cursor] == ' '
                && is_standalone_capital(&chars, cursor + 1)
            {
                letters.push(chars[cursor + 1]);
                cursor += 2;
            }
            if letters.len() >= 2 {
                let merged: String = letters.iter().collect();
                let merged_lower = merged.to_ascii_lowercase();
                // `O P S` 紧跟版本号（如 `O P S 4.5`）时留给 LLM 结合语境判 `Opus`。
                if merged == "OPS" && followed_by_version_number(&chars, cursor) {
                    out.push(chars[i]);
                    i += 1;
                    continue;
                }
                // 2 字母序列仅允许已知无歧义缩写，防止误并英语 `"I A..."` 或孤立大写标签。
                let allowed_two = matches!(
                    merged.as_str(),
                    "AI" | "UI"
                        | "UX"
                        | "PR"
                        | "CI"
                        | "CD"
                        | "ID"
                        | "OS"
                        | "IP"
                        | "DB"
                        | "QA"
                        | "PM"
                        | "OK"
                        | "JS"
                        | "TS"
                        | "IO"
                        | "ML"
                        | "DL"
                        | "CV"
                        | "RL"
                        | "IM"
                );
                if (letters.len() >= 3 || allowed_two) && !user_overridden.contains(&merged_lower) {
                    out.push_str(&merged);
                    i = cursor;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_standalone_capital(chars: &[char], index: usize) -> bool {
    if !chars[index].is_ascii_uppercase() {
        return false;
    }
    let left_ok = index == 0 || !is_ascii_ident_char(chars[index - 1]);
    let right_ok = index + 1 >= chars.len() || !is_ascii_ident_char(chars[index + 1]);
    left_ok && right_ok
}

fn is_ascii_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn has_ascii_word_boundaries(chars: &[char], start: usize, end: usize) -> bool {
    let left_ok = start == 0 || !is_ascii_ident_char(chars[start - 1]);
    let right_ok = end >= chars.len() || !is_ascii_ident_char(chars[end]);
    left_ok && right_ok
}

/// 合并拆分品牌与常见拼写变体（优先级低于用户词表）。
fn merge_split_brands(text: &str, user_overridden: &HashSet<String>) -> String {
    const BRAND_MERGES: &[(&str, &str)] = &[
        ("postgre sql", "PostgreSQL"),
        ("java script", "JavaScript"),
        ("type script", "TypeScript"),
        ("open router", "OpenRouter"),
        ("cloud flare", "Cloudflare"),
        ("play wright", "Playwright"),
        ("deep seek", "DeepSeek"),
        ("mini max", "MiniMax"),
        ("chat gpt", "ChatGPT"),
        ("open code", "OpenCode"),
        ("open api", "OpenAPI"),
        ("fast api", "FastAPI"),
        ("graph ql", "GraphQL"),
        ("swift ui", "SwiftUI"),
        ("sql lite", "SQLite"),
        ("s q lite", "SQLite"),
        ("open ai", "OpenAI"),
        ("git hub", "GitHub"),
        ("git lab", "GitLab"),
        ("we chat", "WeChat"),
        ("my sql", "MySQL"),
        ("next js", "Next.js"),
        ("node js", "Node.js"),
        ("vue js", "Vue.js"),
        ("mac os", "macOS"),
        ("i os", "iOS"),
    ];

    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|ch| ch.to_ascii_lowercase()).collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    'outer: while i < chars.len() {
        for &(pat, canonical) in BRAND_MERGES {
            if user_overridden.contains(&canonical.to_ascii_lowercase())
                || user_overridden.contains(pat)
            {
                continue;
            }
            let pat_len = pat.len();
            let end = i + pat_len;
            if end > chars.len() {
                continue;
            }
            let matched = pat
                .bytes()
                .enumerate()
                .all(|(off, b)| lower[i + off] as u32 == u32::from(b));
            if matched && has_ascii_word_boundaries(&chars, i, end) {
                out.push_str(canonical);
                i = end;
                continue 'outer;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 单行中英标点规整：覆盖 C++ `tidy_local_transcript` 全量规则，保护小数、时间、URL、CLI 参数与纯英文句。
fn normalize_punctuation_line(line: &str, mode: VoiceMode) -> String {
    let chars: Vec<char> = line.chars().collect();
    let line_has_han = chars.iter().copied().any(is_han_character);
    let convert_to_fullwidth = line_has_han && mode != VoiceMode::Translate;

    // 预标记 URL 区间，避免改坏 URL 中的 `:`, `/`, `?`, `.`, `,`。
    let protected_spans = find_url_spans(&chars);
    let in_protected = |idx: usize| -> bool {
        protected_spans
            .iter()
            .any(|&(start, end)| idx >= start && idx < end)
    };

    let mut stage1 = String::with_capacity(line.len() + 8);
    for i in 0..chars.len() {
        let ch = chars[i];
        if in_protected(i) {
            stage1.push(ch);
            continue;
        }
        if convert_to_fullwidth {
            let prev = i.checked_sub(1).and_then(|p| chars.get(p).copied());
            let next = chars.get(i + 1).copied();
            let prev_non_space = (0..i).rev().find(|&p| chars[p] != ' ').map(|p| chars[p]);
            let next_non_space = ((i + 1)..chars.len())
                .find(|&p| chars[p] != ' ')
                .map(|p| chars[p]);
            match ch {
                ',' => {
                    // 保护千分位数字 `1,000`。
                    let between_digits = prev.is_some_and(|c| c.is_ascii_digit())
                        && next.is_some_and(|c| c.is_ascii_digit());
                    if !between_digits
                        && (prev_non_space.is_some_and(is_han_character)
                            || next_non_space.is_some_and(is_han_character))
                    {
                        stage1.push('，');
                        continue;
                    }
                }
                ';' => {
                    if prev_non_space.is_some_and(is_han_character)
                        || next_non_space.is_some_and(is_han_character)
                    {
                        stage1.push('；');
                        continue;
                    }
                }
                ':' => {
                    // 保护时间 `10:30`、端口 `8080`、Rust 路径 `::`。
                    let time_or_scope = prev == Some(':')
                        || next == Some(':')
                        || (prev.is_some_and(|c| c.is_ascii_digit())
                            && next.is_some_and(|c| c.is_ascii_digit()));
                    if !time_or_scope && prev_non_space.is_some_and(is_han_character) {
                        stage1.push('：');
                        continue;
                    }
                }
                '?' => {
                    if prev_non_space.is_some_and(is_han_character)
                        || next_non_space.is_some_and(is_han_character)
                        || next_non_space.is_none()
                    {
                        stage1.push('？');
                        continue;
                    }
                }
                '!' => {
                    let is_not_eq = next == Some('=');
                    if !is_not_eq
                        && (prev_non_space.is_some_and(is_han_character)
                            || next_non_space.is_some_and(is_han_character)
                            || next_non_space.is_none())
                    {
                        stage1.push('！');
                        continue;
                    }
                }
                '.' => {
                    let part_of_ellipsis = prev == Some('.') || next == Some('.');
                    let decimal_or_ident = prev.is_some_and(|c| c.is_ascii_alphanumeric())
                        && next.is_some_and(|c| c.is_ascii_alphanumeric());
                    let list_marker = prev.is_some_and(|c| c.is_ascii_digit())
                        && next == Some(' ')
                        && chars[..i].iter().all(char::is_ascii_digit);
                    if !part_of_ellipsis
                        && !decimal_or_ident
                        && !list_marker
                        && (prev.is_some_and(is_han_character)
                            || (next_non_space.is_none() && line_has_han))
                    {
                        stage1.push('。');
                        continue;
                    }
                }
                _ => {}
            }
        }
        // 英文句间缺空格补齐（如 `monday.Today` → `monday. Today`，继承 `tidy_local_transcript`）。
        if matches!(ch, '.' | '?' | '!')
            && i > 0
            && chars[i - 1].is_ascii_lowercase()
            && chars.get(i + 1).is_some_and(char::is_ascii_uppercase)
        {
            stage1.push(ch);
            stage1.push(' ');
            continue;
        }
        stage1.push(ch);
    }

    // 去除中文标点两侧的多余半角空格，并合并重复标点（保留 `……` 与 `...`）。
    let s_chars: Vec<char> = stage1.chars().collect();
    let mut out = String::with_capacity(stage1.len());
    for i in 0..s_chars.len() {
        let ch = s_chars[i];
        if ch == ' ' {
            let after_cjk_punct = i > 0 && is_cjk_spacing_strip_punct(s_chars[i - 1]);
            let before_cjk_punct =
                i + 1 < s_chars.len() && is_cjk_spacing_strip_punct(s_chars[i + 1]);
            let repeated_space = out.ends_with(' ');
            if after_cjk_punct || before_cjk_punct || repeated_space {
                continue;
            }
        }
        if matches!(ch, '，' | '。' | '、' | '；' | '：') && out.ends_with(ch) {
            continue;
        }
        // 合并 `，。` 为 `。`。
        if ch == '。' && out.ends_with('，') {
            out.pop();
        }
        out.push(ch);
    }
    trim_end_horizontal(&out).to_owned()
}

fn is_cjk_spacing_strip_punct(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。'
            | '？'
            | '！'
            | '、'
            | '：'
            | '；'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '（'
            | '）'
            | '《'
            | '》'
            | '…'
    )
}

fn find_url_spans(chars: &[char]) -> Vec<(usize, usize)> {
    const SCHEMES: &[&str] = &[
        "https://", "http://", "wss://", "ws://", "ftp://", "mailto:", "file://",
    ];
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut matched_len = None;
        for scheme in SCHEMES {
            let len = scheme.len();
            if i + len <= chars.len()
                && scheme
                    .bytes()
                    .enumerate()
                    .all(|(off, b)| chars[i + off].to_ascii_lowercase() == char::from(b))
            {
                matched_len = Some(len);
                break;
            }
        }
        if let Some(prefix_len) = matched_len {
            let mut end = i + prefix_len;
            while end < chars.len()
                && chars[end].is_ascii_graphic()
                && !matches!(chars[end], '<' | '>' | '"' | '\'' | '`')
            {
                end += 1;
            }
            while end > i + prefix_len
                && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?')
            {
                end -= 1;
            }
            if end > i + prefix_len {
                spans.push((i, end));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    spans
}

/// 在非终端/非翻译的中文语境下，将成对包裹中文内容的 ASCII 直引号 `"..."` 规范为 `“...”`（保护代码/JSON/CLI 属性）。
fn normalize_chinese_straight_quotes(
    line: &str,
    mode: VoiceMode,
    app_category: AppCategory,
) -> String {
    if mode == VoiceMode::Translate
        || app_category == AppCategory::TerminalIde
        || !line.contains('"')
        || !line.chars().any(is_han_character)
    {
        return line.to_owned();
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            // 保护赋值/参数/转义引号（如 `msg="你好"`、`\"你好\"`、`--title="你好"`）。
            let left_guarded = i > 0
                && (chars[i - 1] == '='
                    || chars[i - 1] == '\\'
                    || chars[i - 1].is_ascii_alphanumeric());
            if !left_guarded {
                // 查找同行的配对闭合直引号。
                let mut close = None;
                let mut probe = i + 1;
                while probe < chars.len() {
                    if chars[probe] == '"' && chars[probe - 1] != '\\' {
                        close = Some(probe);
                        break;
                    }
                    probe += 1;
                }
                if let Some(end) = close {
                    // 保护 JSON 键值对（如 `"键": 1`）。
                    let right_is_json_colon = ((end + 1)..chars.len())
                        .find(|&p| chars[p] != ' ')
                        .is_some_and(|p| chars[p] == ':');
                    let inner = &chars[i + 1..end];
                    if !right_is_json_colon
                        && !inner.is_empty()
                        && inner.iter().copied().any(is_han_character)
                    {
                        out.push('“');
                        for &c in inner {
                            out.push(c);
                        }
                        out.push('”');
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 双边界保守智能数字 ITN：覆盖日期、安全守卫的 4/2 位口语年份、百分比、美元/人民币货币、版本号小数、`ESI幺`、以及 `>=10` 明确计数；绝对保护自然中文小数字、约数与成语。
pub fn apply_conservative_itn(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // 1. 字母数字编码中的 `幺`（如 `ESI幺`→`ESI1`）及尾随孤立单字编号（如 `ESI幺到ESI五`→`ESI1到ESI5`）。
    let step1 = convert_alphanumeric_code_digits(text);
    // 2. 美元货币（`十刀`/`九点九刀`/`四百美金`/`三USD`/`七万 USD` → `$10`/`$9.9`/`$400`/`$3`/`$70,000`）。
    let step2 = convert_usd_currency(&step1);
    // 3. 版本号与受守卫的小数（`版本二点零`→`版本 2.0`、`GPT五点四`→`GPT 5.4`、`九点九七六九八七`→`9.976987`）。
    let step3 = convert_guarded_decimals(&step2);
    // 4. `10..=19` 补充技术/时间单位及明确 ASCII 计量单位（如 `四GB`→`4GB`、`十二周`→`12周`、`十倍`→`10倍`）。
    let step4 = convert_extended_unit_counts(&step3);
    // 5. 屏蔽易误判为年份的两位约数区间（如 `三五年`、`五八年`），再调用 `numbers::convert` 完成日期、年份、百分数、时刻与整数转换。
    let (shielded, restore_map) = shield_approximate_year_ranges(&step4);
    let converted = numbers::convert(&shielded);
    restore_placeholders(&converted, &restore_map)
}

fn zh_digit(ch: char) -> Option<u64> {
    match ch {
        '零' | '〇' => Some(0),
        '一' | '幺' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

fn zh_place(ch: char) -> Option<u64> {
    match ch {
        '十' => Some(10),
        '百' => Some(100),
        '千' => Some(1_000),
        _ => None,
    }
}

fn is_zh_numeral(ch: char) -> bool {
    zh_digit(ch).is_some() || zh_place(ch).is_some() || ch == '万' || ch == '亿'
}

fn parse_zh_section(chars: &[char]) -> Option<u64> {
    if chars.is_empty() {
        return None;
    }
    let mut total = 0u64;
    let mut pending: Option<u64> = None;
    let mut last_place = 10_000u64;
    let mut zero_after_place = false;
    for (idx, &ch) in chars.iter().enumerate() {
        if let Some(place) = zh_place(ch) {
            if place >= last_place {
                return None;
            }
            let count = match pending.take() {
                Some(d) => d,
                None if place == 10 && idx == 0 => 1,
                None => return None,
            };
            total += count * place;
            last_place = place;
            zero_after_place = false;
        } else {
            let d = zh_digit(ch)?;
            if pending.is_some() {
                return None;
            }
            if d == 0 {
                if zero_after_place || last_place == 10_000 {
                    return None;
                }
                zero_after_place = true;
            } else {
                pending = Some(d);
            }
        }
    }
    match pending {
        None if zero_after_place => None,
        None => Some(total),
        Some(d) if last_place == 10_000 => Some(d),
        Some(d) if zero_after_place || last_place == 10 => Some(total + d),
        Some(d) => Some(total + d * last_place / 10),
    }
}

fn parse_zh_cardinal(chars: &[char]) -> Option<u64> {
    if let [only] = chars {
        if zh_digit(*only) == Some(0) {
            return Some(0);
        }
    }
    for (unit_char, unit) in [('亿', 100_000_000u64), ('万', 10_000u64)] {
        if let Some(split) = chars.iter().position(|c| *c == unit_char) {
            let rest = &chars[split + 1..];
            if rest.contains(&unit_char) {
                return None;
            }
            let high = parse_zh_cardinal(&chars[..split])?;
            let low = match rest {
                [] => 0,
                [single] if zh_digit(*single).is_some_and(|d| d > 0) => {
                    zh_digit(*single)? * (unit / 10)
                }
                ['零', tail @ ..] => parse_zh_cardinal(tail)?,
                _ => parse_zh_cardinal(rest)?,
            };
            if high == 0 || low >= unit {
                return None;
            }
            return high.checked_mul(unit)?.checked_add(low);
        }
    }
    parse_zh_section(chars)
}

fn parse_zh_digit_string(chars: &[char]) -> Option<String> {
    if chars.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(chars.len());
    for &ch in chars {
        let d = zh_digit(ch)?;
        out.push(char::from_digit(d as u32, 10)?);
    }
    Some(out)
}

fn format_with_commas(value: u64) -> String {
    let digits = value.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, byte) in digits.bytes().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(byte as char);
    }
    out
}

fn format_usd_amount(whole: u64, fraction: Option<&str>) -> String {
    if let Some(frac) = fraction {
        format!("${whole}.{frac}")
    } else if whole >= 10_000 {
        format!("${}", format_with_commas(whole))
    } else {
        format!("${whole}")
    }
}

/// 字母编码后接 `幺`→`1` 及明确编号后缀（如 `ESI幺`→`ESI1`、`ESI幺到ESI五`→`ESI1到ESI5`）。
fn convert_alphanumeric_code_digits(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_uppercase() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_uppercase() {
                out.push(chars[i]);
                i += 1;
            }
            let prefix_len = i - start;
            let left_ok = start == 0 || !chars[start - 1].is_ascii_alphabetic();
            // 仅在独立大写字母编号（左边界非英文字母且长度 >= 2，如 `ESI`、`API`）后转换，避免误伤 `macOS`/`iOS` 尾缀。
            if left_ok && prefix_len >= 2 && i < chars.len() {
                if chars[i] == '幺' {
                    while i < chars.len() {
                        if let Some(d) = zh_digit(chars[i]) {
                            // 仅在首字为 `幺` 时续吃纯数字字，排除 `两` 避免误吃 `ESI幺两个`。
                            if chars[i] == '两' {
                                break;
                            }
                            if let Some(ascii) = char::from_digit(d as u32, 10) {
                                out.push(ascii);
                            }
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    continue;
                }
                // 仅在严格编号边界转换单字编号（如 `ESI五`、`ESI三到ESI五`），排除普通数量区间（如 `GPU三到五张`）。
                if matches!(
                    chars[i],
                    '零' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九'
                ) {
                    let next = chars.get(i + 1).copied();
                    let next_after_conn = chars.get(i + 2).copied();
                    let safe_boundary = match next {
                        None => true,
                        Some(c) if !is_han_character(c) => true,
                        Some('到' | '至' | '和' | '与' | '或') => {
                            next_after_conn.is_some_and(|c| c.is_ascii_uppercase())
                        }
                        _ => false,
                    };
                    if safe_boundary {
                        if let Some(ascii) =
                            zh_digit(chars[i]).and_then(|d| char::from_digit(d as u32, 10))
                        {
                            out.push(ascii);
                            i += 1;
                            continue;
                        }
                    }
                }
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 美元货币 ITN：`十刀`/`九点九刀`/`四百美金`/`三USD`/`七万 USD` → `$10`/`$9.9`/`$400`/`$3`/`$70,000`。
fn convert_usd_currency(text: &str) -> String {
    const PRICE_CUES: &[&str] = &[
        "花", "给", "收", "赚", "省", "付", "卖", "买", "要", "才", "只", "约", "大概", "大约",
        "成本", "价格", "费用", "预算", "月费", "总共", "一共", "报价", "只要", "收费", "价值",
    ];
    const KNIFE_FOLLOWERS: &[char] = &['切', '流', '剁', '砍', '子', '片', '法', '锋', '刃', '疤'];

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < chars.len() {
        // 情况 A：中文数字（可含小数点）+ 可选空格 + (美金|美元|刀|USD)
        if is_zh_numeral(chars[i]) {
            let before = i.checked_sub(1).map(|p| chars[p]);
            let approx_before = matches!(before, Some('几' | '数'));
            let mut int_end = i;
            while int_end < chars.len() && is_zh_numeral(chars[int_end]) {
                int_end += 1;
            }
            let mut end = int_end;
            let mut frac: Option<String> = None;
            if chars.get(end) == Some(&'点') {
                let mut frac_end = end + 1;
                while frac_end < chars.len() && zh_digit(chars[frac_end]).is_some() {
                    frac_end += 1;
                }
                if frac_end > end + 1 {
                    frac = parse_zh_digit_string(&chars[end + 1..frac_end]);
                    end = frac_end;
                }
            }
            let mut unit_start = end;
            while unit_start < chars.len() && chars[unit_start] == ' ' {
                unit_start += 1;
            }
            let unit_match = match_usd_unit(&chars, unit_start);
            if let Some((unit_len, is_dao)) = unit_match.filter(|_| !approx_before) {
                if let Some(whole) = parse_zh_cardinal(&chars[i..int_end]) {
                    let after_unit = chars.get(unit_start + unit_len).copied();
                    let dao_ok = !is_dao
                        || (!after_unit.is_some_and(|c| KNIFE_FOLLOWERS.contains(&c))
                            && (whole >= 5
                                || frac.is_some()
                                || ends_with_any_cue(&chars[..i], PRICE_CUES)));
                    if dao_ok {
                        out.push_str(&format_usd_amount(whole, frac.as_deref()));
                        i = unit_start + unit_len;
                        continue;
                    }
                }
            }
        }

        // 情况 B：ASCII 数字（如 `3 USD`、`400美金`）+ 可选空格 + (美金|美元|USD)
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_alphanumeric()) {
            let num_start = i;
            let mut num_end = i;
            while num_end < chars.len() && chars[num_end].is_ascii_digit() {
                num_end += 1;
            }
            let mut frac: Option<String> = None;
            if chars.get(num_end) == Some(&'.')
                && chars.get(num_end + 1).is_some_and(char::is_ascii_digit)
            {
                let frac_start = num_end + 1;
                let mut frac_end = frac_start;
                while frac_end < chars.len() && chars[frac_end].is_ascii_digit() {
                    frac_end += 1;
                }
                frac = Some(chars[frac_start..frac_end].iter().collect());
                num_end = frac_end;
            }
            let mut unit_start = num_end;
            while unit_start < chars.len() && chars[unit_start] == ' ' {
                unit_start += 1;
            }
            if let Some((unit_len, false)) = match_usd_unit(&chars, unit_start) {
                let int_part: String = chars[num_start..num_end]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(whole) = int_part.parse::<u64>() {
                    out.push_str(&format_usd_amount(whole, frac.as_deref()));
                    i = unit_start + unit_len;
                    continue;
                }
            }
        }

        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 匹配美元单位，返回 `(消耗字符数, 是否为口语"刀")`。
fn match_usd_unit(chars: &[char], start: usize) -> Option<(usize, bool)> {
    if start + 2 <= chars.len()
        && ((chars[start] == '美' && chars[start + 1] == '金')
            || (chars[start] == '美' && chars[start + 1] == '元'))
    {
        return Some((2, false));
    }
    if start + 3 <= chars.len()
        && chars[start].eq_ignore_ascii_case(&'u')
        && chars[start + 1].eq_ignore_ascii_case(&'s')
        && chars[start + 2].eq_ignore_ascii_case(&'d')
        && (start + 3 >= chars.len() || !is_ascii_ident_char(chars[start + 3]))
    {
        return Some((3, false));
    }
    if start < chars.len() && chars[start] == '刀' {
        return Some((1, true));
    }
    None
}

fn ends_with_any_cue(prefix: &[char], cues: &[&str]) -> bool {
    let mut end = prefix.len();
    while end > 0 && prefix[end - 1] == ' ' {
        end -= 1;
    }
    let slice = &prefix[..end];
    cues.iter().any(|cue| {
        let cue_chars: Vec<char> = cue.chars().collect();
        slice.len() >= cue_chars.len()
            && slice[slice.len() - cue_chars.len()..]
                .iter()
                .zip(cue_chars.iter())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

/// 受守卫的版本号与小数转换（覆盖 `版本二点零`→`版本 2.0`、`GPT五点四`→`GPT 5.4`、`九点九七六九八七`→`9.976987`、`准确率八十四点五五`→`准确率 84.55`、`延迟三点五毫秒`→`延迟 3.5 毫秒`）。
fn convert_guarded_decimals(text: &str) -> String {
    const VERSION_AND_METRIC_CUES: &[&str] = &[
        "版本",
        "版本号",
        "v",
        "gpt",
        "claude",
        "qwen",
        "opus",
        "sonnet",
        "gemini",
        "deepseek",
        "llama",
        "ios",
        "macos",
        "python",
        "rust",
        "xcode",
        "node",
        "react",
        "next",
        "准确率",
        "召回率",
        "命中率",
        "胜率",
        "延迟",
        "耗时",
        "评分",
        "分数",
        "得分",
        "比例",
        "倍数",
        "系数",
        "阈值",
        "温度",
        "价格",
        "成本",
        "均值",
        "中位数",
    ];
    const CLOCK_CUES: &[&str] = &[
        "凌晨", "早上", "早晨", "上午", "中午", "下午", "傍晚", "晚上", "夜里", "半夜", "今晚",
        "明早", "明晚",
    ];
    const EXTRA_DECIMAL_UNITS: &[&str] = &[
        "毫秒", "分钟", "小时", "天", "个点", "gb", "mb", "kb", "tb", "ms", "khz", "mhz", "ghz",
        "hz", "px", "fps",
    ];
    const SPACE_BEFORE_VERSION_BRANDS: &[&str] = &[
        "gpt", "claude", "qwen", "opus", "sonnet", "gemini", "deepseek", "llama", "ios", "macos",
        "python", "rust", "xcode",
    ];

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < chars.len() {
        // 整段跳过「百分之 + 中文数（含可选小数）」，完整交由 `numbers::convert` 统一转为百分数。
        if i + 3 <= chars.len() && chars[i] == '百' && chars[i + 1] == '分' && chars[i + 2] == '之'
        {
            let mut end = i + 3;
            while end < chars.len() && is_zh_numeral(chars[end]) {
                end += 1;
            }
            if chars.get(end) == Some(&'点') {
                end += 1;
                while end < chars.len() && is_zh_numeral(chars[end]) {
                    end += 1;
                }
            }
            for &ch in &chars[i..end] {
                out.push(ch);
            }
            i = end;
            continue;
        }
        if is_zh_numeral(chars[i]) {
            let int_start = i;
            let mut int_end = i;
            while int_end < chars.len() && is_zh_numeral(chars[int_end]) {
                int_end += 1;
            }
            if chars.get(int_end) == Some(&'点')
                && !ends_with_any_cue(&chars[..int_start], CLOCK_CUES)
                && int_start.checked_sub(1).and_then(|p| chars.get(p)) != Some(&'第')
            {
                let mut cursor = int_end + 1;
                let mut frac_parts: Vec<String> = Vec::new();
                while cursor < chars.len() {
                    let part_start = cursor;
                    while cursor < chars.len() && zh_digit(chars[cursor]).is_some() {
                        cursor += 1;
                    }
                    if cursor == part_start {
                        break;
                    }
                    if let Some(part) = parse_zh_digit_string(&chars[part_start..cursor]) {
                        frac_parts.push(part);
                    } else {
                        break;
                    }
                    if chars.get(cursor) == Some(&'点')
                        && chars.get(cursor + 1).and_then(|&c| zh_digit(c)).is_some()
                    {
                        cursor += 1;
                    } else {
                        break;
                    }
                }
                if !frac_parts.is_empty() {
                    // 保护钟点 `九点零五分` 与成语 `一点一滴`/`一点一点`/`三点一线`。
                    let next_char = chars.get(cursor).copied();
                    let is_clock_minute =
                        next_char == Some('分') && chars.get(cursor + 1).copied() != Some('钟');
                    let is_idiom = frac_parts.len() == 1
                        && frac_parts[0] == "1"
                        && matches!(next_char, Some('滴' | '点' | '线'));
                    if !is_clock_minute && !is_idiom {
                        if let Some(whole) = parse_zh_cardinal(&chars[int_start..int_end]) {
                            let multi_dot = frac_parts.len() >= 2;
                            let long_frac = frac_parts[0].len() >= 2;
                            let has_cue =
                                ends_with_any_cue(&chars[..int_start], VERSION_AND_METRIC_CUES);
                            let has_unit =
                                starts_with_any_unit(&chars[cursor..], EXTRA_DECIMAL_UNITS);
                            if multi_dot || long_frac || has_cue || has_unit {
                                // 若紧跟品牌英文名（如 `GPT五点四`），补一个空格形成 `GPT 5.4`。
                                if ends_with_any_cue(
                                    &chars[..int_start],
                                    SPACE_BEFORE_VERSION_BRANDS,
                                ) && !out.ends_with(' ')
                                {
                                    out.push(' ');
                                }
                                out.push_str(&whole.to_string());
                                for part in &frac_parts {
                                    out.push('.');
                                    out.push_str(part);
                                }
                                i = cursor;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn starts_with_any_unit(slice: &[char], units: &[&str]) -> bool {
    let mut start = 0;
    while start < slice.len() && slice[start] == ' ' {
        start += 1;
    }
    let rest = &slice[start..];
    units.iter().any(|unit| {
        let unit_chars: Vec<char> = unit.chars().collect();
        if rest.len() < unit_chars.len() {
            return false;
        }
        let matched = rest[..unit_chars.len()]
            .iter()
            .zip(unit_chars.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b));
        if !matched {
            return false;
        }
        // ASCII 单位要求右侧词边界，防止 `gb` 误匹配普通单词前缀。
        if unit_chars
            .last()
            .copied()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            rest.get(unit_chars.len())
                .is_none_or(|c| !is_ascii_ident_char(*c))
        } else {
            true
        }
    })
}

/// 补充 `numbers::convert` 未收录的技术/时间单位计数（`>=10` + `周`/`倍`/`行`/`毫秒` 及明确 ASCII 技术单位 `GB`/`MB`/`KB`/`TB`/`ms`/`kHz`/`Hz`/`px`/`fps`）。
fn convert_extended_unit_counts(text: &str) -> String {
    const TECH_ASCII_UNITS: &[&str] = &[
        "gb", "mb", "kb", "tb", "ms", "khz", "mhz", "ghz", "hz", "px", "fps",
    ];
    const EXTRA_TEENS_UNITS: &[&str] = &["周", "倍", "行", "毫秒", "个月"];

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if is_zh_numeral(chars[i]) {
            let before = i.checked_sub(1).map(|p| chars[p]);
            let mut end = i;
            while end < chars.len() && is_zh_numeral(chars[end]) {
                end += 1;
            }
            let approx =
                matches!(before, Some('几' | '数')) || matches!(chars.get(end), Some('几' | '来'));
            if !approx {
                if let Some(value) = parse_zh_cardinal(&chars[i..end]) {
                    let is_tech_ascii = starts_with_any_unit(&chars[end..], TECH_ASCII_UNITS);
                    let is_extra_teen =
                        value >= 10 && starts_with_any_unit(&chars[end..], EXTRA_TEENS_UNITS);
                    if (is_tech_ascii && value >= 1) || is_extra_teen {
                        out.push_str(&value.to_string());
                        if is_tech_ascii && chars.get(end) != Some(&' ') {
                            out.push(' ');
                        }
                        i = end;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 保护易被误转为两位年份的口语约数区间（如 `三五年`、`五八年`），使其不受 `numbers::convert` 年份规则误伤。
fn shield_approximate_year_ranges(text: &str) -> (String, Vec<(String, String)>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len()
            && chars[i + 2] == '年'
            && (i == 0 || !is_zh_numeral(chars[i - 1]))
            && matches!(chars[i], '三' | '四' | '五' | '六' | '七' | '八')
        {
            if let (Some(a), Some(b)) = (zh_digit(chars[i]), zh_digit(chars[i + 1])) {
                if a < b {
                    let original: String = chars[i..i + 3].iter().collect();
                    let token = format!("\u{e000}YR{}\u{e001}", map.len());
                    out.push_str(&token);
                    map.push((token, original));
                    i += 3;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    (out, map)
}

fn restore_placeholders(text: &str, map: &[(String, String)]) -> String {
    if map.is_empty() {
        return text.to_owned();
    }
    let mut result = text.to_owned();
    for (token, original) in map {
        result = result.replace(token, original);
    }
    result
}

/// 内置高频通用技术缩写与品牌大小写规范表（优先级严格低于用户 `voice-vocabulary.json`）。
fn apply_builtin_tech_casing(
    line: &str,
    user_overridden: &HashSet<String>,
    doc_has_han: bool,
    app_category: AppCategory,
) -> String {
    // 1. 全场景无歧义技术缩写与专有品牌。
    const ALWAYS_SAFE_CASING: &[(&str, &str)] = &[
        ("api", "API"),
        ("id", "ID"),
        ("pdf", "PDF"),
        ("http", "HTTP"),
        ("https", "HTTPS"),
        ("ssh", "SSH"),
        ("mcp", "MCP"),
        ("pr", "PR"),
        ("json", "JSON"),
        ("yaml", "YAML"),
        ("cli", "CLI"),
        ("ui", "UI"),
        ("ux", "UX"),
        ("sdk", "SDK"),
        ("sql", "SQL"),
        ("url", "URL"),
        ("dns", "DNS"),
        ("cpu", "CPU"),
        ("gpu", "GPU"),
        ("ram", "RAM"),
        ("llm", "LLM"),
        ("asr", "ASR"),
        ("vad", "VAD"),
        ("tts", "TTS"),
        ("stt", "STT"),
        ("gguf", "GGUF"),
        ("mlx", "MLX"),
        ("html", "HTML"),
        ("css", "CSS"),
        ("csv", "CSV"),
        ("vps", "VPS"),
        ("mvp", "MVP"),
        ("spa", "SPA"),
        ("ci", "CI"),
        ("r2", "R2"),
        ("ai", "AI"),
        ("ok", "OK"),
        ("byok", "BYOK"),
        ("bff", "BFF"),
        ("ops", "OPS"),
        ("jwt", "JWT"),
        ("tls", "TLS"),
        ("ssl", "SSL"),
        ("tcp", "TCP"),
        ("udp", "UDP"),
        ("ipc", "IPC"),
        ("abi", "ABI"),
        ("ffi", "FFI"),
        ("ime", "IME"),
        ("sse", "SSE"),
        ("pcm", "PCM"),
        ("wav", "WAV"),
        ("agc", "AGC"),
        ("rms", "RMS"),
        ("oov", "OOV"),
        ("itn", "ITN"),
        ("cer", "CER"),
        ("mer", "MER"),
        ("deepseek", "DeepSeek"),
        ("openai", "OpenAI"),
        ("openapi", "OpenAPI"),
        ("openrouter", "OpenRouter"),
        ("opencode", "OpenCode"),
        ("next.js", "Next.js"),
        ("node.js", "Node.js"),
        ("vue.js", "Vue.js"),
        ("github", "GitHub"),
        ("gitlab", "GitLab"),
        ("docker", "Docker"),
        ("vercel", "Vercel"),
        ("cloudflare", "Cloudflare"),
        ("playwright", "Playwright"),
        ("typescript", "TypeScript"),
        ("javascript", "JavaScript"),
        ("claude", "Claude"),
        ("codex", "Codex"),
        ("django", "Django"),
        ("gemini", "Gemini"),
        ("chatgpt", "ChatGPT"),
        ("minimax", "MiniMax"),
        ("swiftui", "SwiftUI"),
        ("sqlite", "SQLite"),
        ("postgresql", "PostgreSQL"),
        ("mysql", "MySQL"),
        ("fastapi", "FastAPI"),
        ("graphql", "GraphQL"),
        ("macos", "macOS"),
        ("ios", "iOS"),
        ("ipados", "iPadOS"),
        ("xcode", "Xcode"),
        ("python", "Python"),
        ("cmake", "CMake"),
        ("qwen", "Qwen"),
        ("wechat", "WeChat"),
        ("telegram", "Telegram"),
        ("whatsapp", "WhatsApp"),
        ("iterm2", "iTerm2"),
        ("shurufa", "Shurufa"),
        ("typeless", "Typeless"),
        ("ollama", "Ollama"),
        ("silero", "Silero"),
        ("kubernetes", "Kubernetes"),
        ("linux", "Linux"),
        ("android", "Android"),
        ("harmonyos", "HarmonyOS"),
        ("tauri", "Tauri"),
        ("websocket", "WebSocket"),
        ("raycast", "Raycast"),
        ("homebrew", "Homebrew"),
        ("anthropic", "Anthropic"),
        ("google", "Google"),
        ("microsoft", "Microsoft"),
        ("redis", "Redis"),
        ("nginx", "Nginx"),
        ("kafka", "Kafka"),
        ("webrtc", "WebRTC"),
        ("opencc", "OpenCC"),
        ("coreaudio", "CoreAudio"),
    ];

    // 2. 在中文混排或终端/AI 场景下才规范大小写的双义技术品牌（避免纯英文里误改 `windows`/`swift`/`slack` 等普通词）。
    const CONTEXT_TECH_CASING: &[(&str, &str)] = &[
        ("rust", "Rust"),
        ("swift", "Swift"),
        ("cursor", "Cursor"),
        ("slack", "Slack"),
        ("discord", "Discord"),
        ("notion", "Notion"),
        ("windows", "Windows"),
        ("apple", "Apple"),
        ("react", "React"),
        ("git", "Git"),
        ("markdown", "Markdown"),
        ("whisper", "Whisper"),
        ("tailwind", "Tailwind"),
        ("sublime", "Sublime"),
    ];

    let allow_context_brands =
        doc_has_han || matches!(app_category, AppCategory::TerminalIde | AppCategory::AiChat);

    let chars: Vec<char> = line.chars().collect();
    let protected_spans = find_url_spans(&chars);
    let in_protected = |idx: usize| -> bool {
        protected_spans
            .iter()
            .any(|&(start, end)| idx >= start && idx < end)
    };

    let mut out = String::with_capacity(line.len());
    let mut i = 0;

    while i < chars.len() {
        if in_protected(i) || !chars[i].is_ascii_alphanumeric() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // 扫描完整词元（允许内部包含 `.js` 等点号品牌后缀）。
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        if i + 3 <= chars.len()
            && chars[i] == '.'
            && chars[i + 1].eq_ignore_ascii_case(&'j')
            && chars[i + 2].eq_ignore_ascii_case(&'s')
            && (i + 3 >= chars.len() || !is_ascii_ident_char(chars[i + 3]))
        {
            i += 3;
        }
        let end = i;

        // 跳过路径片段、邮箱、CLI 参数前缀与下划线标识符。
        let prev = start.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(end).copied();
        let in_path_or_flag_or_email = matches!(prev, Some('/' | '@' | '-' | '_' | '.'))
            || matches!(next, Some('@' | '_'))
            || (next == Some('/') && prev == Some('/'));

        let token: String = chars[start..end].iter().collect();
        let lower = token.to_ascii_lowercase();

        // 特殊组合 `ci/cd` → `CI/CD`（避免破坏终端命令 `cd /tmp`）。
        if !in_path_or_flag_or_email
            && lower == "ci"
            && chars.get(end) == Some(&'/')
            && chars
                .get(end + 1)
                .is_some_and(|c| c.eq_ignore_ascii_case(&'c'))
            && chars
                .get(end + 2)
                .is_some_and(|c| c.eq_ignore_ascii_case(&'d'))
            && chars.get(end + 3).is_none_or(|c| !is_ascii_ident_char(*c))
            && !user_overridden.contains("ci/cd")
        {
            out.push_str("CI/CD");
            i = end + 3;
            continue;
        }

        if !in_path_or_flag_or_email && !user_overridden.contains(&lower) {
            if let Some(&(_, canonical)) = ALWAYS_SAFE_CASING.iter().find(|&&(k, _)| k == lower) {
                out.push_str(canonical);
                continue;
            }
            if allow_context_brands {
                if let Some(&(_, canonical)) =
                    CONTEXT_TECH_CASING.iter().find(|&&(k, _)| k == lower)
                {
                    out.push_str(canonical);
                    continue;
                }
            }
        }
        out.push_str(&token);
    }
    out
}

/// 双正交句末句号省略律：
/// - `Translate` 模式（含 IM 英文翻译）：保留英文句点 `.` 与全部句末标点；
/// - `StripIm`（或 `Auto` + `AppCategory::Im`）：含中文时无论长短及单/多段，去除每段及文末单个 `。`（完整保留 `？`/`！`/`……`）；
/// - `StripShort`（或 `Auto` + 非 `Im`）：仅当单段、有效字符 `<= 15` 且无句中停顿标点时去除末尾单个 `。`，其余保留。
fn apply_trailing_period_policy(
    text: &str,
    policy: TrailingPeriodPolicy,
    app_category: AppCategory,
    mode: VoiceMode,
) -> String {
    if mode == VoiceMode::Translate || policy == TrailingPeriodPolicy::Keep {
        return text.to_owned();
    }
    let effective = match policy {
        TrailingPeriodPolicy::Auto => {
            if app_category == AppCategory::Im {
                TrailingPeriodPolicy::StripIm
            } else {
                TrailingPeriodPolicy::StripShort
            }
        }
        other => other,
    };

    match effective {
        TrailingPeriodPolicy::Keep | TrailingPeriodPolicy::Auto => text.to_owned(),
        TrailingPeriodPolicy::StripIm => {
            if !text.chars().any(is_han_character) {
                return text.to_owned();
            }
            text.split('\n')
                .map(|line| line.strip_suffix('。').unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
        }
        TrailingPeriodPolicy::StripShort => {
            if text.contains('\n') {
                return text.to_owned();
            }
            let Some(without_period) = text.strip_suffix('。') else {
                return text.to_owned();
            };
            let has_mid_pause = without_period.chars().any(|ch| {
                matches!(
                    ch,
                    '，' | '、' | '；' | '：' | '。' | '？' | '！' | ',' | ';' | '?' | '!'
                )
            });
            if has_mid_pause {
                return text.to_owned();
            }
            let effective_chars = without_period
                .chars()
                .filter(|ch| !ch.is_whitespace() && !is_cjk_or_ascii_punctuation(*ch))
                .count();
            if effective_chars <= SHORT_SENTENCE_MAX_EFFECTIVE_CHARS {
                without_period.to_owned()
            } else {
                text.to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with(text: &str) -> PostprocessInput {
        PostprocessInput {
            text: text.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn cleans_chunk_boundary_stutter_and_trailing_fillers() {
        assert_eq!(postprocess(&input_with("上。上去看看。")).text, "上去看看");
        assert_eq!(
            postprocess(&input_with("我想上。上去看看。")).text,
            "我想上去看看"
        );
        assert_eq!(postprocess(&input_with("可。可以的。")).text, "可以的");
        assert_eq!(
            postprocess(&input_with("自。自己改一下。")).text,
            "自己改一下"
        );
        assert_eq!(postprocess(&input_with("哪。哪个代理。")).text, "哪个代理");
        assert_eq!(
            postprocess(&input_with("要。把配置改掉。")).text,
            "要把配置改掉"
        );
        assert_eq!(
            postprocess(&input_with("突然。之间断了。")).text,
            "突然之间断了"
        );
        // 正常跨句同字不得误合并。
        assert_eq!(
            postprocess(&input_with("我今天在市场买了鱼。鱼肉非常新鲜。")).text,
            "我今天在市场买了鱼。鱼肉非常新鲜。"
        );
        // 句尾孤立语气词残片清理。
        let mut req = input_with("今天先到这里吧，明天继续讨论细节。嗯。");
        req.app_category = AppCategory::Mail;
        assert_eq!(postprocess(&req).text, "今天先到这里吧，明天继续讨论细节。");
        assert_eq!(
            postprocess(&input_with("明天开会吗？啊。")).text,
            "明天开会吗？"
        );
        assert_eq!(
            postprocess(&input_with("先发给你吧，呃。")).text,
            "先发给你吧"
        );
        let mut keep_filler = input_with("先发给你吧，呃。");
        keep_filler.trailing_period_policy = TrailingPeriodPolicy::Keep;
        assert_eq!(postprocess(&keep_filler).text, "先发给你吧。");
    }

    #[test]
    fn dismisses_filler_only_silence_hallucination_and_repetition_loops() {
        for noise in ["嗯。", "啊", "呃", "哦", "哎", "。。。", "   "] {
            let out = postprocess(&input_with(noise));
            assert!(out.dismissed, "{noise}");
            assert!(out.text.is_empty(), "{noise}");
        }
        assert_eq!(
            postprocess(&input_with("嗯。")).dismissed_reason.as_deref(),
            Some("filler_only")
        );

        let mut short_hal = input_with("字幕由测试组提供");
        short_hal.audio_ms = Some(1_200);
        let out = postprocess(&short_hal);
        assert!(out.dismissed);
        assert_eq!(
            out.dismissed_reason.as_deref(),
            Some("silence_hallucination")
        );

        let mut short_en = input_with("Thank you for watching.");
        short_en.audio_ms = Some(1_800);
        assert_eq!(
            postprocess(&short_en).dismissed_reason.as_deref(),
            Some("silence_hallucination")
        );

        // 长录音里真实说出「感谢观看」不得被误杀。
        let mut long_real = input_with("感谢观看。");
        long_real.audio_ms = Some(6_000);
        assert!(!postprocess(&long_real).dismissed);

        // 整段死循环直接拦截。
        let loop_all = postprocess(&input_with("测试测试测试测试测试"));
        assert!(loop_all.dismissed);
        assert_eq!(
            loop_all.dismissed_reason.as_deref(),
            Some("repetition_loop")
        );

        // 有实词前缀的尾部复读压缩为一次。
        let loop_tail = postprocess(&input_with(
            "我们先把方案整理清楚，然后再推进推进推进推进推进",
        ));
        assert!(!loop_tail.dismissed);
        assert_eq!(loop_tail.text, "我们先把方案整理清楚，然后再推进");

        // 正常数字与笑声不受复读检测误伤。
        assert!(!postprocess(&input_with("预算 100000 元")).dismissed);
        assert!(!postprocess(&input_with("哈哈哈哈太好了")).dismissed);
    }

    #[test]
    fn merges_split_initialisms_brands_and_spoken_symbols() {
        let mut req = input_with(
            "用 A P I K 调 Mini Max 和 Deep Seek，查 Open A P I 文档，看 B Y O K 与 P.S.W 还有 A.I",
        );
        req.app_category = AppCategory::General;
        let out = postprocess(&req);
        assert_eq!(
            out.text,
            "用 API Key 调 MiniMax 和 DeepSeek，查 OpenAPI 文档，看 BYOK 与 PSW 还有 AI"
        );

        assert_eq!(
            postprocess(&input_with("配置APIK字段")).text,
            "配置 API Key 字段"
        );
        assert_eq!(
            postprocess(&input_with("Chat GPT 和 Open Code")).text,
            "ChatGPT 和 OpenCode"
        );
        assert_eq!(
            postprocess(&input_with(
                "检查Account下划线Identity和user下划线id下划线list"
            ))
            .text,
            "检查 account_identity 和 user_id_list"
        );
        assert_eq!(
            postprocess(&input_with("访问Max点儿example点儿com看看")).text,
            "访问 max.example.com 看看"
        );
        // 普通中文含「下划线」与 `O P S 4.5` 不得误改。
        assert_eq!(
            postprocess(&input_with("用下划线分隔字段")).text,
            "用下划线分隔字段"
        );
        assert_eq!(postprocess(&input_with("O P S 平台")).text, "OPS 平台");
        assert_eq!(
            postprocess(&input_with("O P S 4.5 模型")).text,
            "O P S 4.5 模型"
        );
    }

    #[test]
    fn conservative_itn_converts_structured_numbers_and_protects_natural_chinese() {
        assert_eq!(
            postprocess(&input_with("准确率百分之二十")).text,
            "准确率 20%"
        );
        assert_eq!(
            postprocess(&input_with("准确率百分之八十四点五五")).text,
            "准确率 84.55%"
        );
        assert_eq!(
            postprocess(&input_with("利率百分之三点一四")).text,
            "利率 3.14%"
        );
        assert_eq!(
            postprocess(&input_with("三月十五号见")).text,
            "3 月 15 号见"
        );
        assert_eq!(
            postprocess(&input_with("十月三十号上线")).text,
            "10 月 30 号上线"
        );
        assert_eq!(
            postprocess(&input_with("一九年建成的项目")).text,
            "19 年建成的项目"
        );
        assert_eq!(
            postprocess(&input_with("版本二点零发布了")).text,
            "版本 2.0 发布了"
        );
        assert_eq!(
            postprocess(&input_with("GPT五点四表现不错")).text,
            "GPT 5.4 表现不错"
        );
        assert_eq!(
            postprocess(&input_with("成本只要十刀")).text,
            "成本只要 $10"
        );
        assert_eq!(
            postprocess(&input_with("预算四百美金，大概二十人民币")).text,
            "预算 $400，大概 20 人民币"
        );
        assert_eq!(
            postprocess(&input_with("合同金额七万 USD")).text,
            "合同金额 $70,000"
        );
        assert_eq!(
            postprocess(&input_with("每天三十次，还有十四个")).text,
            "每天 30 次，还有 14 个"
        );
        assert_eq!(
            postprocess(&input_with("型号ESI幺到ESI五")).text,
            "型号 ESI1 到 ESI5"
        );
        assert_eq!(
            postprocess(&input_with("型号ESI三到ESI五")).text,
            "型号 ESI3 到 ESI5"
        );
        assert_eq!(
            postprocess(&input_with("在macOS三到五分钟")).text,
            "在 macOS 三到五分钟"
        );
        assert_eq!(
            postprocess(&input_with("支持GPU三到五张")).text,
            "支持 GPU 三到五张"
        );
        assert_eq!(
            postprocess(&input_with("需要四GB内存")).text,
            "需要 4 GB 内存"
        );

        // 绝对保护自然中文小数字、约数与成语。
        for protected in [
            "一个方案",
            "两个问题",
            "三个步骤",
            "第一个需求",
            "等一下再看",
            "一下子就好了",
            "几十万条记录",
            "三心二意",
            "一刀切不行",
            "一两年再说",
            "三五年后再看",
            "百分之百确定",
            "十分有趣",
        ] {
            let mut req = input_with(protected);
            req.trailing_period_policy = TrailingPeriodPolicy::Keep;
            assert_eq!(postprocess(&req).text, protected, "{protected}");
        }
    }

    #[test]
    fn user_vocabulary_overrides_builtin_tech_casing() {
        let mut req = input_with("调用 minimax 和 api，顺便检查 github 与 playwright");
        req.vocabulary = vec![VocabularyTerm {
            canonical: "minimax-custom".to_owned(),
            aliases: vec!["minimax".to_owned()],
            pinyin: String::new(),
            category: "product".to_owned(),
            weight: 200,
        }];
        let out = postprocess(&req);
        assert_eq!(
            out.text,
            "调用 minimax-custom 和 API，顺便检查 GitHub 与 Playwright"
        );
    }

    #[test]
    fn normalizes_chinese_punctuation_and_straight_quotes_while_protecting_code_and_indent() {
        let mut req =
            input_with("点击\"确认\"按钮,然后打开https://example.com:8080/api?k=1,耗时3.14s.");
        req.app_category = AppCategory::BrowserDoc;
        let out = postprocess(&req);
        assert_eq!(
            out.text,
            "点击“确认”按钮，然后打开 https://example.com:8080/api?k=1，耗时 3.14s。"
        );

        // 终端场景与 JSON/赋值语句保留 ASCII 直引号。
        let mut term_req = input_with("执行 echo \"你好\" 和 cd /tmp");
        term_req.app_category = AppCategory::TerminalIde;
        assert_eq!(postprocess(&term_req).text, "执行 echo \"你好\" 和 cd /tmp");

        // P0 缩进保护：Git diff 的 4 空格缩进与前缀必须完整保留。
        let diff =
            "@@ -1,3 +1,3 @@\n    context line\n-    oldValue = \"旧值\"\n+    newValue = \"新值\"";
        let mut diff_req = input_with(diff);
        diff_req.mode = VoiceMode::RewriteSelection;
        assert_eq!(postprocess(&diff_req).text, diff);

        // 多行嵌套列表保留行首缩进。
        let nested = "1. 统一架构规范：\n   保留三空格缩进并补齐Rust测试";
        assert_eq!(
            postprocess(&input_with(nested)).text,
            "1. 统一架构规范：\n   保留三空格缩进并补齐 Rust 测试"
        );

        // 围栏代码块内部未缩进代码行完整受保护，不改大小写、直引号与 Pangu 空格；围栏外正文照常处理。
        let fenced = "```rust\nlet api = \"你好rust\";\n```\n这是后面的rust说明。";
        assert_eq!(
            postprocess(&input_with(fenced)).text,
            "```rust\nlet api = \"你好rust\";\n```\n这是后面的 Rust 说明。"
        );
    }

    #[test]
    fn enforces_orthogonal_trailing_period_policy() {
        // IM 场景中文/混排无论长短及多段，去掉每段末尾单个 `。`，保留 `？`/`！`/`……`。
        let mut im_long = input_with(
            "第一段说明今天已经把核心逻辑梳理完毕，稍后提交验证。\n\n第二段回头再补充一些回归测试细节。",
        );
        im_long.app_category = AppCategory::Im;
        assert_eq!(
            postprocess(&im_long).text,
            "第一段说明今天已经把核心逻辑梳理完毕，稍后提交验证\n\n第二段回头再补充一些回归测试细节"
        );

        let mut im_question = input_with("今天下午四点开会吗？");
        im_question.app_category = AppCategory::Im;
        assert_eq!(postprocess(&im_question).text, "今天下午 4 点开会吗？");

        let mut im_ellipsis = input_with("让我想想……");
        im_ellipsis.app_category = AppCategory::Im;
        assert_eq!(postprocess(&im_ellipsis).text, "让我想想……");

        // IM 翻译模式英文输出保留 `.`。
        let mut im_translate = input_with("Please check the updated config.");
        im_translate.app_category = AppCategory::Im;
        im_translate.mode = VoiceMode::Translate;
        assert_eq!(
            postprocess(&im_translate).text,
            "Please check the updated config."
        );

        // 非 IM 场景：单段 <=15 有效字且无句中停顿去 `。`，长句或有逗号保留 `。`。
        let mut non_im_short = input_with("帮我跑一下回归单测。");
        non_im_short.app_category = AppCategory::TerminalIde;
        assert_eq!(postprocess(&non_im_short).text, "帮我跑一下回归单测");

        let mut non_im_comma = input_with("好的，马上处理。");
        non_im_comma.app_category = AppCategory::TerminalIde;
        assert_eq!(postprocess(&non_im_comma).text, "好的，马上处理。");

        let mut non_im_long = input_with("这是一个超过十五个有效字符的完整技术说明句子。");
        non_im_long.app_category = AppCategory::BrowserDoc;
        assert_eq!(
            postprocess(&non_im_long).text,
            "这是一个超过 15 个有效字符的完整技术说明句子。"
        );

        // Keep 策略无条件保留句末句号。
        let mut keep = input_with("好的。");
        keep.app_category = AppCategory::Im;
        keep.trailing_period_policy = TrailingPeriodPolicy::Keep;
        assert_eq!(postprocess(&keep).text, "好的。");
    }

    #[test]
    fn postprocess_is_idempotent_across_scenarios() {
        let samples = [
            (
                "用 A P I K 调用 Mini Max，三月十五号上线，准确率百分之二十。",
                AppCategory::BrowserDoc,
                VoiceMode::Dictation,
            ),
            (
                "第一段测试完成。\n\n第二段也准备好了。",
                AppCategory::Im,
                VoiceMode::Dictation,
            ),
            (
                "    + 新增Rust检查逻辑\n    - 移除旧逻辑",
                AppCategory::TerminalIde,
                VoiceMode::RewriteSelection,
            ),
            (
                "Please check the Cloudflare R2 API token.",
                AppCategory::Im,
                VoiceMode::Translate,
            ),
        ];
        for (sample, app_category, mode) in samples {
            let mut req = input_with(sample);
            req.app_category = app_category;
            req.mode = mode;
            let once = postprocess(&req);
            req.text = once.text.clone();
            let twice = postprocess(&req);
            assert_eq!(twice.text, once.text, "not idempotent: {sample}");
            assert!(!twice.changed, "second pass mutated: {sample}");
        }
    }

    #[test]
    fn merges_spoken_hyphens_and_slashes_between_ascii_without_touching_chinese() {
        assert_eq!(
            postprocess(&input_with("把 Qwen3 杠 ASR 换成 GPT 横杠 4o")).text,
            "把 Qwen3-ASR 换成 GPT-4o"
        );
        assert_eq!(
            postprocess(&input_with("分支名是 feature 短横杠 voice 杠 fix")).text,
            "分支名是 feature-voice-fix"
        );
        assert_eq!(
            postprocess(&input_with("路径在 src 斜杠 voice 斜杠 main.rs")).text,
            "路径在 src/voice/main.rs"
        );
        assert_eq!(
            postprocess(&input_with(
                "别跟他抬杠，杠杆率太高了，比方说就是某某杠某某"
            ))
            .text,
            "别跟他抬杠，杠杆率太高了，比方说就是某某杠某某"
        );
    }
}
