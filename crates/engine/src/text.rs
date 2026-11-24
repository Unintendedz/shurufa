//! UTF-8 与汉字文本辅助函数，供词网格、重排器与 `neural_v2` 上下文窗口共用。

/// 判断码点是否为汉字（含 U+3007、CJK 基本区与扩展区至 U+323AF）。
pub fn is_han_code_point(code_point: u32) -> bool {
    matches!(code_point, 0x3007 | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F | 0x30000..=0x323AF)
}

pub fn is_han(character: char) -> bool {
    is_han_code_point(character as u32)
}

/// 返回 `text` 中第一个汉字的切片，无汉字时为空。
pub fn first_han_char(text: &str) -> &str {
    text.char_indices()
        .find(|(_, c)| is_han(*c))
        .map_or("", |(at, c)| &text[at..at + c.len_utf8()])
}

/// 返回 `text` 中最后一个汉字的切片，无汉字时为空。
pub fn last_han_char(text: &str) -> &str {
    text.char_indices()
        .rev()
        .find(|(_, c)| is_han(*c))
        .map_or("", |(at, c)| &text[at..at + c.len_utf8()])
}

/// 按 UTF-8 首字节统计字符数（保留与旧 C++ `count_han_chars` 一致的计数语义，双拼等调用方将其用作字符计数）。
pub fn count_han_chars(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut at = 0;
    while at < bytes.len() {
        let lead = bytes[at];
        at += if lead >= 0xF0 {
            4
        } else if lead >= 0xE0 {
            3
        } else if lead >= 0xC0 {
            2
        } else {
            1
        };
        count += 1;
    }
    count
}

pub fn count_utf8_chars(text: &str) -> usize {
    text.chars().count()
}

/// 非空且每个字符均为汉字。
pub fn is_all_han(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_han)
}

/// 含汉字且不止一个字符：「只出单字」要去掉的词和整句（`你好`、`T恤`）。单个汉字、英文、表情和颜文字都不算。
pub fn is_han_phrase(text: &str) -> bool {
    text.chars().nth(1).is_some() && text.chars().any(is_han)
}

/// 返回 `text` 末尾至多 `count` 个 Unicode 标量值的切片；`count == 0` 时返回空串。
pub fn last_characters(text: &str, count: usize) -> &str {
    if count == 0 {
        return "";
    }
    match text.char_indices().rev().nth(count - 1) {
        Some((at, _)) => &text[at..],
        None => text,
    }
}

/// 返回 `text` 末尾至多 `max_chars` 个 Unicode 标量值的完整上下文窗口切片，
/// 完整保留中文标点、空格、ASCII 字母、数字与表情符号，供 `neural_v2` 因果语言模型（最多 512 字符）消费。
/// `trailing_han_clause` 仍保留给 octagram `.gram` 尾窗与旧字级模型。
#[allow(dead_code)]
pub fn recent_context_window(text: &str, max_chars: usize) -> &str {
    last_characters(text, max_chars)
}

/// 返回 `text` 末尾连续汉字子句的切片；若末字符不是汉字（如标点、空格、ASCII、表情符号）或无汉字则返回 `""`。
/// 旧字级句子模型 vocab 不含空格、标点和表情符号，剥离末尾非汉字前缀可防止 `<unk>` 词元污染前文语境。
pub fn trailing_han_clause(text: &str) -> &str {
    let mut start = text.len();
    for (at, ch) in text.char_indices().rev() {
        if is_han(ch) {
            start = at;
        } else {
            break;
        }
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_the_reference_rules() {
        assert_eq!(first_han_char("a你好"), "你");
        assert_eq!(last_han_char("你好b"), "好");
        assert_eq!(first_han_char("abc"), "");
        assert_eq!(count_han_chars("a你𠀀"), 3);
        assert_eq!(last_characters("a你好", 2), "你好");
        assert_eq!(last_characters("a你好", 0), "");
        assert_eq!(last_characters("好", 5), "好");
        assert!(is_han_phrase("你好"));
        assert!(is_han_phrase("T恤"));
        assert!(!is_han_phrase("你"));
        assert!(!is_han_phrase("𠀀"));
        assert!(!is_han_phrase("GitHub"));
        assert!(!is_han_phrase("(^_^)"));
        assert!(is_all_han("你好"));
        assert!(!is_all_han("你a"));
        assert_eq!(trailing_han_clause("🧐 培养"), "培养");
        assert_eq!(trailing_han_clause("你好，世界"), "世界");
        assert_eq!(trailing_han_clause("你好，"), "");
        assert_eq!(trailing_han_clause("ASCII only"), "");
        assert_eq!(trailing_han_clause(""), "");
        assert_eq!(trailing_han_clause("难道说没有"), "难道说没有");
    }

    #[test]
    fn recent_context_window_preserves_punctuation_spaces_ascii_and_digits() {
        let mixed = "我昨天花了 128.5 元，买了一台 M3 Max！今天打算继续写 Rust 2024 代码，";
        assert_eq!(recent_context_window(mixed, 512), mixed);
        assert_eq!(recent_context_window(mixed, 0), "");
        assert_eq!(recent_context_window("你好，世界！", 4), "，世界！");
        assert_eq!(recent_context_window("🧐 培养，", 5), "🧐 培养，");
        assert_eq!(recent_context_window("Hello, 世界 42!", 6), "世界 42!");
        // 超长混合上下文按字符边界截断至 512 字符且不截断多字节 UTF-8。
        let long = "甲乙，ab 12！".repeat(80);
        let window = recent_context_window(&long, 512);
        assert_eq!(window.chars().count(), 512);
        assert!(long.ends_with(window));
    }
}
