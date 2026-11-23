//! 语音润色的宿主场景分类、假选区清洗与两层选区意图路由（纯函数，无 I/O）。

use serde::{Deserialize, Deserializer, Serialize};

/// 宿主应用场景分桶，用于驱动双正交句末句号策略与语气/排版提示词。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    /// 即时聊天软件（微信、QQ、Telegram、Slack、WhatsApp、飞书聊天等）。
    Im,
    /// 终端模拟器与代码编辑器/IDE（iTerm2、Warp、Codex、VS Code、Cursor、OpenCode 等）。
    TerminalIde,
    /// AI 对话客户端与网页 AI 助手（Claude、ChatGPT、Paseo、Cherry Studio 等）。
    AiChat,
    /// 浏览器文档、笔记与知识库（Google Docs、Notion、GitHub、Obsidian 等）。
    BrowserDoc,
    /// 邮件客户端与网页邮箱（Apple Mail、Outlook、Gmail 等）。
    Mail,
    /// 未命中特定分类的通用场景。
    #[default]
    General,
}

impl AppCategory {
    /// 与跨平台 JSON/C ABI 契约一致的蛇形字符串标识。
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

/// 最终生效的语音润色模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoicePolishMode {
    /// 普通听写整理（去口癖、自纠错、专词纠偏、场景化排版与标点）。
    #[default]
    Dictation,
    /// 针对当前选中文本的口述改写、保结构翻译或回复生成。
    RewriteSelection,
    /// 语音翻译（清洗口癖 + 上下文实体消歧 + 自然意译 + 合理排版）。
    Translate,
}

impl VoicePolishMode {
    /// 与跨平台 JSON/C ABI 契约一致的蛇形字符串标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dictation => "dictation",
            Self::RewriteSelection => "rewrite_selection",
            Self::Translate => "translate",
        }
    }
}

/// 调用方请求的润色模式；缺省 `auto` 表示按配置、目标语种与两层选区意图自动路由。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedPolishMode {
    #[default]
    Auto,
    Dictation,
    RewriteSelection,
    Translate,
}

/// `auto` 模式下，当存在有效选区时的两层意图分类结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionIntent {
    /// 明确命中改写/翻译/总结/回复等编辑指令信号，直接走选区改写。
    ExplicitCommand,
    /// 明显与选区无关的独立口述或闲聊，直接走普通听写，防止误改偶然选中的内容。
    PureDictation,
    /// 短口述且难以确定是改写选区还是新听写，交由模型首行输出 `<action>rewrite|dictate</action>` 二次裁决。
    Ambiguous,
}

/// 路由解析输出：最终模式、是否要求模型输出意图标签以及选区意图判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedPolishRoute {
    pub mode: VoicePolishMode,
    pub expect_action_tag: bool,
    pub selection_intent: SelectionIntent,
}

/// 宿主上报的脱敏上下文快照，字段命名与蓝图 C.4 JSON 契约严格对齐。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceContextSnapshot {
    #[serde(deserialize_with = "deserialize_null_default")]
    pub app_bundle_id: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub app_name: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub window_title: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub web_domain: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub preceding_text: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub following_text: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub selected_text: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub visible_screen_text: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub is_secure_field: bool,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub ax_blacklisted: bool,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// 清洗选区文本（纵深防御）：剥离 QQ 常见 `U+FFFC` 对象占位符、`U+FEFF`、零宽字符、
/// 非换行控制字符；若仅剩空白则返回空串，同时保留多行代码/Diff 内部缩进。
pub fn sanitize_selected_text(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|&ch| {
            !matches!(
                ch,
                '\u{FFFC}' | '\u{FEFF}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}'
            ) && (!ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        })
        .collect();
    if cleaned.trim().is_empty() {
        return String::new();
    }
    // 统一将 `\r\n` 与孤立 `\r` 规范为 `\n`，避免 `\r`-only 多行选区被误判单行而丢失首行缩进。
    let normalized = cleaned.replace("\r\n", "\n").replace('\r', "\n");
    // 仅剥离首尾空行，保留代码/Git diff 首行及各行的前导缩进，单行普通文本仍去首尾空白。
    if normalized.contains('\n') {
        let lines: Vec<&str> = normalized.split('\n').collect();
        let first = lines
            .iter()
            .position(|line| !line.trim().is_empty())
            .unwrap_or(0);
        let last = lines
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .unwrap_or(lines.len().saturating_sub(1));
        if first == last {
            normalized.trim().to_owned()
        } else {
            lines[first..=last]
                .iter()
                .map(|line| line.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
        }
    } else {
        normalized.trim().to_owned()
    }
}

/// 按 `web_domain` → `bundle_id` → `app_name`（辅以浏览器 `window_title` 消歧）确定宿主场景分类。
pub fn classify_app(
    bundle_id: &str,
    app_name: &str,
    window_title: &str,
    web_domain: &str,
) -> AppCategory {
    let domain = web_domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Some(category) = classify_web_domain(&domain) {
        return category;
    }

    let bundle = bundle_id.trim().to_ascii_lowercase();
    let title = window_title.trim().to_ascii_lowercase();
    if let Some(category) = classify_bundle_id(&bundle, &title) {
        return category;
    }

    let name = app_name.trim().to_ascii_lowercase();
    if let Some(category) = classify_app_name(&name, &title) {
        return category;
    }

    AppCategory::General
}

fn domain_matches(host: &str, target: &str) -> bool {
    host == target
        || (host.len() > target.len()
            && host.ends_with(target)
            && host.as_bytes()[host.len() - target.len() - 1] == b'.')
}

fn classify_web_domain(host: &str) -> Option<AppCategory> {
    if host.is_empty() {
        return None;
    }

    const AI_CHAT_DOMAINS: &[&str] = &[
        "claude.ai",
        "chatgpt.com",
        "chat.openai.com",
        "chat.deepseek.com",
        "gemini.google.com",
        "aistudio.google.com",
        "poe.com",
        "perplexity.ai",
        "grok.com",
        "kimi.moonshot.cn",
        "kimi.com",
        "doubao.com",
        "yuanbao.tencent.com",
        "chat.qwen.ai",
        "tongyi.aliyun.com",
    ];
    if AI_CHAT_DOMAINS.iter().any(|d| domain_matches(host, d)) {
        return Some(AppCategory::AiChat);
    }

    const MAIL_DOMAINS: &[&str] = &[
        "mail.google.com",
        "outlook.live.com",
        "outlook.office.com",
        "outlook.office365.com",
        "mail.qq.com",
        "mail.163.com",
        "mail.126.com",
        "mail.proton.me",
        "app.fastmail.com",
        "mail.icloud.com",
    ];
    if MAIL_DOMAINS.iter().any(|d| domain_matches(host, d)) {
        return Some(AppCategory::Mail);
    }

    const IM_DOMAINS: &[&str] = &[
        "web.whatsapp.com",
        "web.telegram.org",
        "app.slack.com",
        "discord.com",
        "wx.qq.com",
        "teams.microsoft.com",
        "teams.live.com",
    ];
    if IM_DOMAINS.iter().any(|d| domain_matches(host, d)) {
        return Some(AppCategory::Im);
    }

    const DOC_DOMAINS: &[&str] = &[
        "docs.google.com",
        "sheets.google.com",
        "slides.google.com",
        "notion.so",
        "notion.site",
        "feishu.cn",
        "larksuite.com",
        "github.com",
        "gitlab.com",
        "linear.app",
        "atlassian.net",
        "yuque.com",
        "wolai.com",
        "coda.io",
        "overleaf.com",
        "craft.do",
        "figma.com",
        "cloudflare.com",
        "vercel.com",
    ];
    if DOC_DOMAINS.iter().any(|d| domain_matches(host, d)) {
        return Some(AppCategory::BrowserDoc);
    }

    None
}

fn classify_browser_title(title: &str) -> AppCategory {
    if title.is_empty() {
        return AppCategory::BrowserDoc;
    }
    if [
        "claude",
        "chatgpt",
        "deepseek",
        "gemini",
        "perplexity",
        "poe",
    ]
    .iter()
    .any(|kw| title.contains(kw))
    {
        return AppCategory::AiChat;
    }
    if ["gmail", "outlook", "收件箱", "写邮件", "proton mail"]
        .iter()
        .any(|kw| title.contains(kw))
    {
        return AppCategory::Mail;
    }
    if ["whatsapp", "telegram", "slack", "discord"]
        .iter()
        .any(|kw| title.contains(kw))
    {
        return AppCategory::Im;
    }
    AppCategory::BrowserDoc
}

fn classify_bundle_id(bundle: &str, title: &str) -> Option<AppCategory> {
    if bundle.is_empty() {
        return None;
    }

    if matches!(
        bundle,
        "com.tencent.xinwechat"
            | "com.tencent.qq"
            | "com.tencent.mipadqq"
            | "com.tdesktop.telegram"
            | "ru.keepcoder.telegram"
            | "com.tinyspeck.slackmacgap"
            | "net.whatsapp.whatsapp"
            | "com.whatsapp.whatsapp"
            | "desktop.whatsapp"
            | "com.apple.mobilesms"
            | "com.electron.lark"
            | "com.bytedance.feishu"
            | "com.larksuite.larkapp"
            | "com.hnc.discord"
            | "com.alibaba.dingtalkmac"
            | "com.tencent.weworkmac"
            | "com.tencent.meeting"
            | "org.whispersystems.signal-desktop"
            | "com.microsoft.teams"
            | "com.microsoft.teams2"
    ) {
        return Some(AppCategory::Im);
    }

    if bundle.starts_with("com.jetbrains.")
        || matches!(
            bundle,
            "com.googlecode.iterm2"
                | "dev.warp.warp-stable"
                | "dev.warp.warp-preview"
                | "com.apple.terminal"
                | "com.mitchellh.ghostty"
                | "net.kovidgoyal.kitty"
                | "io.alacritty"
                | "org.alacritty"
                | "co.zeit.hyper"
                | "com.github.wez.wezterm"
                | "com.openai.codex"
                | "com.microsoft.vscode"
                | "com.microsoft.vscodeinsiders"
                | "com.todesktop.230313mzl4w4u92"
                | "com.exafunction.windsurf"
                | "ai.opencode.desktop"
                | "dev.zed.zed"
                | "dev.zed.zed-preview"
                | "com.apple.dt.xcode"
                | "com.sublimetext.4"
                | "com.sublimetext.3"
        )
    {
        return Some(AppCategory::TerminalIde);
    }

    if matches!(
        bundle,
        "com.anthropic.claudefordesktop"
            | "com.openai.chat"
            | "com.openai.atlas"
            | "sh.paseo.desktop"
            | "com.kangfenmao.cherrystudio"
            | "ai.perplexity.mac"
            | "com.electron.ollama"
            | "ai.elementlabs.lmstudio"
    ) {
        return Some(AppCategory::AiChat);
    }

    if matches!(
        bundle,
        "com.apple.mail"
            | "com.microsoft.outlook"
            | "com.readdle.smartemail-mac"
            | "com.freron.mailmate"
            | "org.mozilla.thunderbird"
            | "com.postbox-inc.postboxapp"
            | "com.superhuman.electron"
    ) {
        return Some(AppCategory::Mail);
    }

    if matches!(
        bundle,
        "company.thebrowser.browser"
            | "company.thebrowser.dia"
            | "com.google.chrome"
            | "com.google.chrome.beta"
            | "com.google.chrome.canary"
            | "com.apple.safari"
            | "com.apple.safaritechnologypreview"
            | "org.mozilla.firefox"
            | "org.mozilla.firefoxdeveloperedition"
            | "com.microsoft.edgemac"
            | "com.microsoft.edgemac.beta"
            | "com.brave.browser"
            | "com.vivaldi.vivaldi"
            | "com.operasoftware.opera"
            | "com.kagi.kagimacos"
    ) {
        return Some(classify_browser_title(title));
    }

    if matches!(
        bundle,
        "notion.id"
            | "md.obsidian"
            | "com.apple.notes"
            | "com.apple.iwork.pages"
            | "com.microsoft.word"
            | "com.typora.typora"
            | "abnerworks.typora"
            | "net.shinyfrog.bear"
            | "com.linear"
    ) {
        return Some(AppCategory::BrowserDoc);
    }

    None
}

fn classify_app_name(name: &str, title: &str) -> Option<AppCategory> {
    if name.is_empty() {
        return None;
    }

    if matches!(
        name,
        "wechat"
            | "微信"
            | "qq"
            | "telegram"
            | "slack"
            | "whatsapp"
            | "messages"
            | "信息"
            | "discord"
            | "feishu"
            | "飞书"
            | "lark"
            | "dingtalk"
            | "钉钉"
            | "企业微信"
            | "wecom"
            | "腾讯会议"
            | "tencent meeting"
            | "signal"
    ) {
        return Some(AppCategory::Im);
    }

    if matches!(
        name,
        "iterm2"
            | "iterm"
            | "warp"
            | "terminal"
            | "终端"
            | "ghostty"
            | "kitty"
            | "alacritty"
            | "wezterm"
            | "codex"
            | "visual studio code"
            | "vs code"
            | "code"
            | "cursor"
            | "windsurf"
            | "opencode"
            | "zed"
            | "xcode"
            | "sublime text"
    ) || name.starts_with("intellij")
        || name.starts_with("pycharm")
        || name.starts_with("rustrover")
        || name.starts_with("webstorm")
        || name.starts_with("goland")
        || name.starts_with("clion")
    {
        return Some(AppCategory::TerminalIde);
    }

    if matches!(
        name,
        "claude"
            | "chatgpt"
            | "chatgpt atlas"
            | "paseo"
            | "cherry studio"
            | "cherrystudio"
            | "perplexity"
    ) {
        return Some(AppCategory::AiChat);
    }

    if matches!(
        name,
        "mail" | "邮件" | "outlook" | "spark" | "thunderbird" | "mailmate" | "superhuman"
    ) {
        return Some(AppCategory::Mail);
    }

    if matches!(
        name,
        "arc"
            | "dia"
            | "google chrome"
            | "chrome"
            | "safari"
            | "firefox"
            | "microsoft edge"
            | "edge"
            | "brave"
            | "vivaldi"
    ) {
        return Some(classify_browser_title(title));
    }

    if matches!(
        name,
        "notion" | "obsidian" | "notes" | "备忘录" | "typora" | "pages" | "microsoft word" | "word"
    ) {
        return Some(AppCategory::BrowserDoc);
    }

    None
}

/// 统计非空白字符数，用于区分简短指令与独立长口述。
fn content_char_count(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

/// 在小写文本中按 ASCII 词边界匹配英文指令词，避免 `prefix` 误中 `fix` 等子串误判。
fn contains_ascii_word(lower: &str, word: &str) -> bool {
    let bytes = lower.as_bytes();
    let needle = word.as_bytes();
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    let mut start = 0;
    while let Some(pos) = lower[start..].find(word) {
        let abs = start + pos;
        let end = abs + needle.len();
        let left_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let right_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// 明确指向选中文本的指代语，即使口述稍长也足以判定是在指挥处理选区。
fn has_explicit_selection_reference(raw: &str, lower: &str) -> bool {
    const ZH_DEICTIC: &[&str] = &[
        "把这段",
        "把这句",
        "把这个",
        "把上面",
        "把下面",
        "把选中",
        "把它们",
        "这段话",
        "这句话",
        "这段内容",
        "这段文字",
        "这条消息",
        "这封邮件",
        "回复这条",
        "回复一下这条",
        "帮我回这条",
        "针对这段",
        "针对这条",
    ];
    if ZH_DEICTIC.iter().any(|p| raw.contains(p)) {
        return true;
    }
    const EN_DEICTIC: &[&str] = &[
        "this text",
        "this message",
        "this paragraph",
        "this sentence",
        "this email",
        "selected text",
        "above text",
        "rewrite this",
        "translate this",
        "summarize this",
        "polish this",
        "fix this",
        "reply to this",
        "respond to this",
        "explain this",
        "shorten this",
        "expand this",
        "rephrase this",
        "make this",
        "turn this into",
        "convert this",
    ];
    EN_DEICTIC.iter().any(|p| contains_ascii_word(lower, p))
}

/// 低碰撞的显式编辑/改写/翻译指令信号；日常听写高碰撞词（如“改成/帮我写/回复/simplify/fix the”）
/// 刻意不进入本层，而是降级至 `Ambiguous` 或 `PureDictation`，杜绝 `mode=auto` 覆盖偶然选区。
fn has_explicit_command_signal(raw: &str, lower: &str, char_len: usize) -> bool {
    if has_explicit_selection_reference(raw, lower) {
        return true;
    }

    // 低碰撞纯编辑指令词（适用于中短指令 <= 90 字）。
    const ZH_LOW_COLLISION_COMMANDS: &[&str] = &[
        "改写",
        "重写",
        "润色",
        "精简",
        "缩写",
        "扩写",
        "重新组织",
        "重新表述",
        "换一种说法",
        "换个说法",
        "检查语病",
        "修正语病",
        "改通顺",
        "改顺一点",
        "翻译成",
        "译成",
        "翻成",
        "译为",
        "翻译一下",
        "语气改",
        "语气调",
        "语气换",
        "调一下语气",
        "换个语气",
        "聊天口吻",
        "微信口吻",
        "帮我润",
        "帮我翻",
        "帮我总结",
        "帮我精简",
        "帮我扩写",
        "帮我缩写",
        "帮我缩短",
        "帮我拉长",
        "帮我提炼",
        "去掉格式",
        "不要那么多格式",
        "不要格式",
    ];
    if ZH_LOW_COLLISION_COMMANDS.iter().any(|p| raw.contains(p)) {
        return true;
    }

    // 超短独立指令（<= 10 字）：几乎不可能出现在完整日常陈述句中。
    if char_len <= 10 {
        const ZH_ULTRA_SHORT_COMMANDS: &[&str] = &[
            "翻译",
            "总结",
            "概括",
            "提炼",
            "校对",
            "解释一下",
            "解释",
            "优化一下",
            "列出要点",
            "列成要点",
            "格式化",
            "回他",
            "回她",
            "啥意思",
            "什么意思",
            "改成英文",
            "改成中文",
            "改为英文",
            "改为中文",
        ];
        if ZH_ULTRA_SHORT_COMMANDS.iter().any(|p| raw.contains(p)) {
            return true;
        }
    }

    const EN_LOW_COLLISION_WORDS: &[&str] = &[
        "rewrite",
        "rephrase",
        "paraphrase",
        "translate",
        "summarize",
        "summarise",
        "polish",
        "proofread",
        "shorten",
        "condense",
        "reformat",
    ];
    if EN_LOW_COLLISION_WORDS
        .iter()
        .any(|w| contains_ascii_word(lower, w))
    {
        return true;
    }

    const EN_LOW_COLLISION_PHRASES: &[&str] =
        &["fix grammar", "fix typo", "change tone", "bullet points"];
    if EN_LOW_COLLISION_PHRASES
        .iter()
        .any(|p| contains_ascii_word(lower, p))
    {
        return true;
    }

    // 极短英文独立指令（<= 16 非空字符），避免长技术句子误中通用动词。
    if char_len <= 16 {
        const EN_ULTRA_SHORT_COMMANDS: &[&str] = &[
            "shorter",
            "more polite",
            "more formal",
            "more casual",
            "more concise",
            "explain",
            "expand",
            "elaborate",
        ];
        if EN_ULTRA_SHORT_COMMANDS
            .iter()
            .any(|w| contains_ascii_word(lower, w))
        {
            return true;
        }
    }

    false
}

/// 常见的纯聊天寒暄与独立应答，命中时优先判定为普通听写，绝不误改选区。
fn is_pure_chat_or_greeting(raw: &str, lower: &str) -> bool {
    let zh_compact: String = raw
        .chars()
        .filter(|&c| {
            !c.is_whitespace() && !matches!(c, '。' | '，' | '！' | '？' | '.' | ',' | '!' | '?')
        })
        .collect();
    if zh_compact.is_empty() {
        return true;
    }
    const ZH_PURE_CHAT: &[&str] = &[
        "好的",
        "好",
        "收到",
        "谢谢",
        "感谢",
        "没问题",
        "好的没问题",
        "收到谢谢",
        "好的收到",
        "好的收到谢谢",
        "嗯好的",
        "行",
        "可以",
        "你好",
        "早安",
        "晚安",
        "拜拜",
        "知道了",
        "明白",
        "明白了",
    ];
    if ZH_PURE_CHAT.contains(&zh_compact.as_str()) {
        return true;
    }
    let lower_stripped = lower.trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?' | '。' | '，' | '！' | '？')
    });
    matches!(
        lower_stripped,
        "ok" | "okay"
            | "thanks"
            | "thank you"
            | "got it"
            | "sounds good"
            | "sure"
            | "yes"
            | "no"
            | "hi"
            | "hello"
            | "bye"
    )
}

/// 弱风格/格式/语种修饰线索及降级的高碰撞口语指令词（仅作用于短句）：
/// 交由 LLM 结合 `<selected_text>` 与 `<asr_text>` 首行输出 `<action>rewrite|dictate</action>` 安全裁决。
fn has_ambiguous_edit_cue(raw: &str, lower: &str, char_len: usize) -> bool {
    if char_len <= 25 {
        const ZH_SOFT_CUES: &[&str] = &[
            "正式一点",
            "委婉一点",
            "礼貌一点",
            "客气一点",
            "客气点",
            "简洁一点",
            "精炼一点",
            "专业一点",
            "通顺一点",
            "自然一点",
            "口语一点",
            "柔和一点",
            "简短一点",
            "直接一点",
            "稍微正式",
            "稍微委婉",
            "稍微简洁",
            "稍微口语",
            "更委婉",
            "更正式",
            "更礼貌",
            "更简洁",
            "更口语",
            "太啰嗦",
            "太生硬",
            "太长了",
            "太短了",
            "不够正式",
            "不够委婉",
            "不够简洁",
            "英文版",
            "中文版",
            "日文版",
            "韩文版",
            "书面化",
            "口语化",
            "分点",
            "要点版",
            "换个风格",
            "换种口吻",
            "语气",
            "改成",
            "改为",
            "换成",
            "调整成",
            "整理成",
            "帮我改",
            "帮我写",
            "帮我换",
            "帮我整理",
            "帮我回复",
            "帮我回一下",
            "帮我解释",
            "帮我检查",
            "回复一下",
            "优化一下",
            "总结一下",
            "概括一下",
            "提炼一下",
            "列出要点",
            "列成要点",
            "提炼要点",
        ];
        if ZH_SOFT_CUES.iter().any(|cue| raw.contains(cue)) {
            return true;
        }
    }
    if char_len <= 35 {
        const EN_SOFT_CUES: &[&str] = &[
            "tone",
            "style",
            "english version",
            "chinese version",
            "simpler",
            "simplify",
            "more direct",
            "less formal",
            "too long",
            "too wordy",
            "bulleted",
            "softer",
            "reply",
            "respond",
            "fix the",
            "make it",
            "format",
        ];
        if EN_SOFT_CUES
            .iter()
            .any(|cue| contains_ascii_word(lower, cue))
        {
            return true;
        }
    }
    false
}

/// 两层选区意图路由：在 `mode=auto` 且存在有效选区时区分显式改写指令、普通听写与歧义态。
pub fn detect_selection_intent(raw_text: &str, selected_text: &str) -> SelectionIntent {
    let selected = sanitize_selected_text(selected_text);
    let raw = raw_text.trim();
    if selected.is_empty() || raw.is_empty() {
        return SelectionIntent::PureDictation;
    }

    let char_len = content_char_count(raw);
    let lower = raw.to_ascii_lowercase();

    // 长篇口述（> 90 字）只有明确指代选区时才按改写处理，避免正文中偶提“总结/优化”误伤选区。
    if char_len > 90 {
        return if has_explicit_selection_reference(raw, &lower) {
            SelectionIntent::ExplicitCommand
        } else {
            SelectionIntent::PureDictation
        };
    }

    if has_explicit_command_signal(raw, &lower, char_len) {
        return SelectionIntent::ExplicitCommand;
    }

    // 独立中长口述或纯寒暄应答，直接判定为普通听写。
    if char_len > 45 || is_pure_chat_or_greeting(raw, &lower) {
        return SelectionIntent::PureDictation;
    }

    // 短口述带有风格/语种/格式微调线索时，交由 LLM 输出 <action> 标签二次裁决。
    if has_ambiguous_edit_cue(raw, &lower, char_len) {
        return SelectionIntent::Ambiguous;
    }

    SelectionIntent::PureDictation
}

/// 统一计算最终润色模式与是否期望 `<action>` 裁决标签。
///
/// 规则：
/// 1. 显式 `mode` 优先：`rewrite_selection` 在有效选区非空时走选区改写（空选区自动退回听写）；
///    `translate` 走翻译；`dictation` 走普通听写。
/// 2. `mode=auto` 时：`prompt_id == "zh2en"` 或传入非空 `target_language` 走翻译；
///    否则若启用 `enable_selection_rewrite` 且清洗后的选区非空，经两层选区意图检测路由。
pub fn resolve_polish_mode(
    requested_mode: RequestedPolishMode,
    prompt_id: &str,
    target_language: &str,
    enable_selection_rewrite: bool,
    raw_text: &str,
    selected_text: &str,
) -> ResolvedPolishRoute {
    let cleaned_selected = sanitize_selected_text(selected_text);
    match requested_mode {
        RequestedPolishMode::Dictation => ResolvedPolishRoute {
            mode: VoicePolishMode::Dictation,
            expect_action_tag: false,
            selection_intent: SelectionIntent::PureDictation,
        },
        RequestedPolishMode::Translate => ResolvedPolishRoute {
            mode: VoicePolishMode::Translate,
            expect_action_tag: false,
            selection_intent: SelectionIntent::PureDictation,
        },
        RequestedPolishMode::RewriteSelection => {
            if cleaned_selected.is_empty() {
                ResolvedPolishRoute {
                    mode: VoicePolishMode::Dictation,
                    expect_action_tag: false,
                    selection_intent: SelectionIntent::PureDictation,
                }
            } else {
                ResolvedPolishRoute {
                    mode: VoicePolishMode::RewriteSelection,
                    expect_action_tag: false,
                    selection_intent: SelectionIntent::ExplicitCommand,
                }
            }
        }
        RequestedPolishMode::Auto => {
            if prompt_id.trim().eq_ignore_ascii_case("zh2en") || !target_language.trim().is_empty()
            {
                return ResolvedPolishRoute {
                    mode: VoicePolishMode::Translate,
                    expect_action_tag: false,
                    selection_intent: SelectionIntent::PureDictation,
                };
            }
            if enable_selection_rewrite && !cleaned_selected.is_empty() {
                let intent = detect_selection_intent(raw_text, &cleaned_selected);
                return match intent {
                    SelectionIntent::ExplicitCommand => ResolvedPolishRoute {
                        mode: VoicePolishMode::RewriteSelection,
                        expect_action_tag: false,
                        selection_intent: intent,
                    },
                    SelectionIntent::Ambiguous => ResolvedPolishRoute {
                        mode: VoicePolishMode::RewriteSelection,
                        expect_action_tag: true,
                        selection_intent: intent,
                    },
                    SelectionIntent::PureDictation => ResolvedPolishRoute {
                        mode: VoicePolishMode::Dictation,
                        expect_action_tag: false,
                        selection_intent: intent,
                    },
                };
            }
            ResolvedPolishRoute {
                mode: VoicePolishMode::Dictation,
                expect_action_tag: false,
                selection_intent: SelectionIntent::PureDictation,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_macos_apps_and_browser_domains() {
        // IM 客户端覆盖微信、QQ、Telegram 双 bundle、Slack、飞书、腾讯会议。
        for bundle in [
            "com.tencent.xinWeChat",
            "com.tencent.qq",
            "com.tdesktop.Telegram",
            "ru.keepcoder.Telegram",
            "com.tinyspeck.slackmacgap",
            "com.tencent.meeting",
        ] {
            assert_eq!(
                classify_app(bundle, "", "", ""),
                AppCategory::Im,
                "bundle={bundle}"
            );
        }

        // 终端与 IDE 覆盖 iTerm2、Warp、Codex、VS Code、Cursor、OpenCode、JetBrains。
        for bundle in [
            "com.googlecode.iterm2",
            "dev.warp.Warp-Stable",
            "com.openai.codex",
            "com.microsoft.VSCode",
            "com.todesktop.230313mzl4w4u92",
            "ai.opencode.desktop",
            "com.jetbrains.rustrover",
        ] {
            assert_eq!(
                classify_app(bundle, "", "", ""),
                AppCategory::TerminalIde,
                "bundle={bundle}"
            );
        }

        // AI 对话桌面端覆盖 Claude、ChatGPT、Paseo、Cherry Studio。
        for bundle in [
            "com.anthropic.claudefordesktop",
            "com.openai.chat",
            "sh.paseo.desktop",
            "com.kangfenmao.CherryStudio",
        ] {
            assert_eq!(
                classify_app(bundle, "", "", ""),
                AppCategory::AiChat,
                "bundle={bundle}"
            );
        }

        assert_eq!(
            classify_app("com.apple.mail", "Mail", "", ""),
            AppCategory::Mail
        );

        // 浏览器多标签场景：web_domain 优先于浏览器 bundle_id。
        assert_eq!(
            classify_app(
                "company.thebrowser.Browser",
                "Arc",
                "Synthetic Tab",
                "claude.ai"
            ),
            AppCategory::AiChat
        );
        assert_eq!(
            classify_app(
                "com.google.Chrome",
                "Google Chrome",
                "Inbox",
                "mail.google.com"
            ),
            AppCategory::Mail
        );
        assert_eq!(
            classify_app(
                "com.google.Chrome",
                "Google Chrome",
                "Chat",
                "web.telegram.org"
            ),
            AppCategory::Im
        );
        assert_eq!(
            classify_app("com.apple.Safari", "Safari", "Spec", "docs.google.com"),
            AppCategory::BrowserDoc
        );
        assert_eq!(
            classify_app("com.google.Chrome", "Google Chrome", "PR #42", "github.com"),
            AppCategory::BrowserDoc
        );
        // 浏览器缺少域名时利用窗口标题辅助消歧。
        assert_eq!(
            classify_app(
                "company.thebrowser.Browser",
                "Arc",
                "ChatGPT - Synthetic Thread",
                ""
            ),
            AppCategory::AiChat
        );
        // 无 bundle_id 时回退到 app_name。
        assert_eq!(
            classify_app("", "Cherry Studio", "", ""),
            AppCategory::AiChat
        );
        assert_eq!(
            classify_app("com.synthetic.unknown", "SyntheticApp", "", ""),
            AppCategory::General
        );
    }

    #[test]
    fn sanitizes_qq_object_replacement_and_preserves_multiline_diff_indentation() {
        assert_eq!(sanitize_selected_text("\u{FFFC}"), "");
        assert_eq!(
            sanitize_selected_text(" \u{FFFC}\u{FEFF}\u{200B}\u{200C}\u{200D}\u{2060}\n\t "),
            ""
        );
        assert_eq!(
            sanitize_selected_text(" \u{FFFC}保留正文\u{200B} "),
            "保留正文"
        );
        // 多行 Git diff 选区必须保留行首四空格缩进（含 \r-only 换行规范化）。
        assert_eq!(
            sanitize_selected_text("\n    - old_line\n    + new_line  \n\n"),
            "    - old_line\n    + new_line"
        );
        assert_eq!(
            sanitize_selected_text("\r    - old_cr\r    + new_cr  \r"),
            "    - old_cr\n    + new_cr"
        );
    }

    #[test]
    fn two_layer_selection_intent_separates_command_dictation_and_ambiguous() {
        let sample_selection = "这是一段用于测试的合成草稿内容。";

        // 假选区或空选区一律判为纯听写。
        assert_eq!(
            detect_selection_intent("帮我润色一下", "\u{FFFC}"),
            SelectionIntent::PureDictation
        );
        assert_eq!(
            detect_selection_intent("帮我润色一下", "   "),
            SelectionIntent::PureDictation
        );

        // 显式中文与英文指令。
        for cmd in [
            "帮我把这段润色得更正式一点",
            "精简成三条要点",
            "翻译成英文",
            "回复这条消息说我下午两点有空",
            "不要那么多格式，改成微信聊天口吻",
            "解释一下",
            "Rewrite this in a more polite tone",
            "Translate to Japanese",
            "Summarize in bullet points",
            "Make it shorter and fix grammar",
        ] {
            assert_eq!(
                detect_selection_intent(cmd, sample_selection),
                SelectionIntent::ExplicitCommand,
                "cmd={cmd}"
            );
        }

        // 明显无关的独立口述或闲聊，绝不误改选区。
        for dictation in [
            "好的，收到谢谢",
            "我下午三点到会议室，咱们先对齐一下接口字段",
            "今天上午我们先梳理完整个音频采集管线的缓冲大小，然后把各个模块的接口契约逐条核对清楚，最后再补充一组合成数据回归测试确保边界行为稳定。",
            "刚才那个接口已经优化好了，而且我们也总结了上周的日志，不过今天先不发布，等明天上午把最后两组回归测完再一起合入主干分支。",
            "Sounds good",
        ] {
            assert_eq!(
                detect_selection_intent(dictation, sample_selection),
                SelectionIntent::PureDictation,
                "dictation={dictation}"
            );
        }

        // 英文词边界保护：prefix 不得误匹配 fix。
        assert_eq!(
            detect_selection_intent("We should add a prefix to the cache key", sample_selection),
            SelectionIntent::PureDictation
        );

        // 简短但含风格/语种微调线索或高碰撞口语词 → Ambiguous，交由模型 <action> 二次裁决。
        for ambiguous in [
            "稍微正式一点",
            "英文版",
            "更客气点",
            "太啰嗦了",
            "softer tone",
            "帮我改成委婉一点的回复",
        ] {
            assert_eq!(
                detect_selection_intent(ambiguous, sample_selection),
                SelectionIntent::Ambiguous,
                "ambiguous={ambiguous}"
            );
        }

        // 防破坏性误改回归：日常听写/口语自纠错/AI 提示句绝不能误判为 ExplicitCommand 直接覆盖选区。
        for guard_case in [
            "明天下午三点，不对，改成四点开会",
            "帮我写一个单元测试验证边界条件",
            "我晚点回复你",
            "We should simplify the cache layer",
            "Let's fix the build error first",
        ] {
            assert_ne!(
                detect_selection_intent(guard_case, sample_selection),
                SelectionIntent::ExplicitCommand,
                "不得直接判为 ExplicitCommand 覆盖偶然选区: {guard_case}"
            );
        }
        // “我晚点回复你”既不含显式指令也不含改写线索，应直接判为 PureDictation。
        assert_eq!(
            detect_selection_intent("我晚点回复你", sample_selection),
            SelectionIntent::PureDictation
        );
    }

    #[test]
    fn resolves_modes_respecting_explicit_flags_and_auto_routing() {
        // 显式翻译。
        let r = resolve_polish_mode(
            RequestedPolishMode::Translate,
            "cleanup",
            "",
            true,
            "明天见",
            "选区",
        );
        assert_eq!(r.mode, VoicePolishMode::Translate);
        assert!(!r.expect_action_tag);

        // auto + zh2en 预设走翻译。
        let r = resolve_polish_mode(
            RequestedPolishMode::Auto,
            "zh2en",
            "",
            true,
            "如何配置存储桶",
            "选区",
        );
        assert_eq!(r.mode, VoicePolishMode::Translate);

        // auto + 关闭选区改写开关时，即使说“润色”也不改选区。
        let r = resolve_polish_mode(
            RequestedPolishMode::Auto,
            "cleanup",
            "",
            false,
            "帮我润色一下",
            "一段草稿",
        );
        assert_eq!(r.mode, VoicePolishMode::Dictation);
        assert!(!r.expect_action_tag);

        // auto + 开启选区改写 + 歧义短指令 → RewriteSelection 且 expect_action_tag=true。
        let r = resolve_polish_mode(
            RequestedPolishMode::Auto,
            "cleanup",
            "",
            true,
            "稍微正式一点",
            "一段草稿",
        );
        assert_eq!(r.mode, VoicePolishMode::RewriteSelection);
        assert!(r.expect_action_tag);
        assert_eq!(r.selection_intent, SelectionIntent::Ambiguous);

        // 显式 RewriteSelection 若选区仅含 U+FFFC 假占位符，安全退回普通听写。
        let r = resolve_polish_mode(
            RequestedPolishMode::RewriteSelection,
            "cleanup",
            "",
            false,
            "润色一下",
            "\u{FFFC}",
        );
        assert_eq!(r.mode, VoicePolishMode::Dictation);
        assert!(!r.expect_action_tag);
    }

    #[test]
    fn context_snapshot_deserializes_missing_and_null_fields() {
        let snap: VoiceContextSnapshot = serde_json::from_str(
            r#"{"app_bundle_id":"com.tencent.qq","window_title":null,"selected_text":null,"is_secure_field":null}"#,
        )
        .unwrap();
        assert_eq!(snap.app_bundle_id, "com.tencent.qq");
        assert!(snap.window_title.is_empty());
        assert!(snap.selected_text.is_empty());
        assert!(!snap.is_secure_field);
        assert!(!snap.ax_blacklisted);
    }
}
