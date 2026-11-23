//! Shared text predicates used by client-core input boundaries.

/// 与现有设置界面的 JavaScript `trim` 一致：去掉首尾空白，包含 BOM，不包含 U+0085。
pub fn trim_ui_whitespace(value: &str) -> &str {
    value.trim_matches(|c| {
        matches!(c, '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
    })
}

/// Whether `value` contains a control character other than the line-oriented controls accepted
/// by multi-line text fields.
pub fn has_disallowed_control(value: &str) -> bool {
    has_disallowed_control_with_options(value, true)
}

/// Whether `value` contains a disallowed control character with an explicit line-whitespace policy.
pub fn has_disallowed_control_with_options(value: &str, allow_whitespace: bool) -> bool {
    let allowed = if allow_whitespace {
        &['\n', '\r', '\t'][..]
    } else {
        &[]
    };
    has_disallowed_control_with_allowed(value, allowed)
}

/// Whether `value` contains a control character outside the supplied allowed set.
pub fn has_disallowed_control_with_allowed(value: &str, allowed: &[char]) -> bool {
    value
        .chars()
        .any(|character| character.is_control() && !allowed.contains(&character))
}

/// Whether `value` contains a control character other than line-feed or carriage-return.
pub fn has_disallowed_control_with_line_breaks(value: &str) -> bool {
    has_disallowed_control_with_allowed(value, &['\n', '\r'])
}

/// Whether a single-line field fits its byte bound and contains no controls.
pub fn is_bounded_text(value: &str, maximum_bytes: usize) -> bool {
    is_bounded_text_with_options(value, maximum_bytes, false)
}

/// Whether `value` fits its byte bound and contains no control characters outside the selected whitespace policy.
pub fn is_bounded_text_with_options(
    value: &str,
    maximum_bytes: usize,
    allow_whitespace: bool,
) -> bool {
    value.len() <= maximum_bytes && !has_disallowed_control_with_options(value, allow_whitespace)
}

/// Whether `value` fits both byte and Unicode scalar bounds and contains no controls.
pub fn is_bounded_text_with_chars(
    value: &str,
    maximum_bytes: usize,
    maximum_characters: usize,
) -> bool {
    is_bounded_text(value, maximum_bytes) && value.chars().count() <= maximum_characters
}

/// Whether `value` fits a UTF-16 code-unit bound.
pub fn is_bounded_utf16(value: &str, maximum_units: usize) -> bool {
    value.encode_utf16().count() <= maximum_units
}

/// Validate an ASCII hexadecimal value with an exact byte length.
pub fn is_ascii_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Validate a CSS-style hexadecimal color with one of the accepted digit lengths.
pub fn is_hex_color(value: &str, digits: &[usize]) -> bool {
    value.strip_prefix('#').is_some_and(|hex| {
        digits.contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Validate a non-empty ASCII digit string.
pub fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether a value contains only ASCII letters.
pub fn is_ascii_alphabetic(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_alphabetic())
}

/// Whether a value contains only visible ASCII characters (U+0021 through U+007E).
pub fn is_ascii_graphic(value: &str) -> bool {
    value.bytes().all(|byte| (33..=126).contains(&byte))
}

/// Whether a value contains only lowercase ASCII letters.
pub fn is_ascii_lowercase(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Whether a value contains only ASCII letters, digits, and dashes.
pub fn is_ascii_alphanumeric_dash(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Whether a value contains only ASCII letters, digits, dashes, and underscores.
pub fn is_ascii_identifier(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Whether a non-empty ASCII identifier fits the supplied byte bound.
pub fn is_bounded_ascii_identifier(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= maximum_bytes && is_ascii_identifier(value)
}

/// Whether a value contains only ASCII letters, digits, dots, dashes, and underscores.
pub fn is_ascii_identifier_with_dots(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// Whether a value contains only lowercase ASCII letters, digits, dots, dashes, and underscores.
pub fn is_ascii_lowercase_identifier_with_dots(value: &str) -> bool {
    value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
    })
}

pub fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether a single-line field fits its character bound and contains no controls.
pub fn is_bounded_chars(value: &str, maximum_characters: usize) -> bool {
    is_bounded_chars_with_options(value, maximum_characters, false)
}

/// Whether `value` fits its character bound and contains no control characters outside the selected whitespace policy.
pub fn is_bounded_chars_with_options(
    value: &str,
    maximum_characters: usize,
    allow_whitespace: bool,
) -> bool {
    value.chars().count() <= maximum_characters
        && !has_disallowed_control_with_options(value, allow_whitespace)
}

/// Whether `value` fits a character bound and contains no NUL character.
pub fn is_bounded_chars_without_nul(value: &str, maximum_characters: usize) -> bool {
    value.chars().count() <= maximum_characters && !value.contains('\0')
}

/// Whether an ASCII byte is an RFC 3986 URI unreserved character.
pub fn is_ascii_uri_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// 皮肤、插件包等文件夹名的形状：1 到 64 字节，首字符是 ASCII 字母或数字，其余是小写 ASCII 标识符字符或点。
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && is_ascii_lowercase_identifier_with_dots(id)
}

/// 判断字符是否属于 CJK 统一表意文字（基本区、扩展 A–G 及兼容表意文字区）。
pub fn is_han_character(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            | 0x30000..=0x3134f
    )
}

/// 判断相邻两个字符是否构成「汉字 ↔ ASCII 字母/数字」的空格插入边界。
pub fn is_han_ascii_alphanumeric_boundary(left: char, right: char) -> bool {
    (is_han_character(left) && right.is_ascii_alphanumeric())
        || (left.is_ascii_alphanumeric() && is_han_character(right))
}

/// 在 `text` 中汉字与 ASCII 字母、数字相邻处各插入一个半角空格；无边界时返回 `None` 且不分配内存。
///
/// 供键盘上屏中英文加空格（`chinese_english_spacing`）与跨次上屏策略共用；假名、谚文、全角字母与 ASCII 标点均不触发。
pub fn space_han_ascii_alphanumeric(text: &str) -> Option<String> {
    let mut pairs = text.chars().zip(text.chars().skip(1));
    if !pairs.any(|(left, right)| is_han_ascii_alphanumeric_boundary(left, right)) {
        return None;
    }
    let mut spaced = String::with_capacity(text.len() + 8);
    let mut previous = None;
    for character in text.chars() {
        if previous.is_some_and(|previous| is_han_ascii_alphanumeric_boundary(previous, character))
        {
            spaced.push(' ');
        }
        spaced.push(character);
        previous = Some(character);
    }
    Some(spaced)
}

/// 识别 URL 协议前缀的字符长度（大小写不敏感）。
fn url_scheme_prefix_len(chars: &[char], start: usize) -> Option<usize> {
    const SCHEMES: &[&str] = &[
        "https://", "http://", "wss://", "ws://", "ftp://", "mailto:", "file://",
    ];
    for scheme in SCHEMES {
        let len = scheme.len();
        if start + len > chars.len() {
            continue;
        }
        let matches = scheme.bytes().enumerate().all(|(offset, expected)| {
            chars[start + offset].to_ascii_lowercase() == char::from(expected)
        });
        if matches {
            return Some(len);
        }
    }
    None
}

/// 从 `start` 起扫描完整 ASCII URL 片段，保护其内部字符不被拆散，并避开尾随句末标点。
fn scan_url_end(chars: &[char], start: usize) -> Option<usize> {
    let prefix_len = url_scheme_prefix_len(chars, start)?;
    let mut end = start + prefix_len;
    let mut paren_depth: usize = 0;
    let mut bracket_depth: usize = 0;
    while end < chars.len() {
        let ch = chars[end];
        if !ch.is_ascii_graphic() {
            break;
        }
        match ch {
            '<' | '>' | '"' | '\'' | '`' => break,
            '(' => paren_depth += 1,
            ')' => {
                if paren_depth == 0 {
                    break;
                }
                paren_depth -= 1;
            }
            '[' => bracket_depth += 1,
            ']' => {
                if bracket_depth == 0 {
                    break;
                }
                bracket_depth -= 1;
            }
            '{' | '}' => break,
            _ => {}
        }
        end += 1;
    }
    // 去掉 URL 尾部误吞的半角句末标点（例如 `见https://example.com.` 中的句点）。
    while end > start + prefix_len && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?') {
        end -= 1;
    }
    (end > start + prefix_len).then_some(end)
}

/// 标准 Pangu 中英混排空格：在汉字与 ASCII 字母/数字、数字百分号后接汉字、汉字后接美元金额或长参数 `--flag` 处插入单个半角空格。
///
/// 不豁免中文量词（如 `3 个`、`8 月 4 日`、`50% 的` 均加空格），同时完整保留每行行首 `[ \t]+` 缩进，并保护 URL、邮箱、代码标识符（`snake_case`/`camelCase`/`v1.2.3`/`--flag`）及数字单位（`50%`、`3.5s`）内部不被拆散；若文本无需修改则返回 `None` 且不分配结果串。
pub fn pangu_space(text: &str) -> Option<String> {
    if !text.chars().any(is_han_character) {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut changed = false;
    let mut index = 0;
    while index < chars.len() {
        if let Some(url_end) = scan_url_end(&chars, index) {
            if index > 0 && is_han_character(chars[index - 1]) && !out.ends_with(' ') {
                out.push(' ');
                changed = true;
            }
            for &ch in &chars[index..url_end] {
                out.push(ch);
            }
            if url_end < chars.len() && is_han_character(chars[url_end]) && !out.ends_with(' ') {
                out.push(' ');
                changed = true;
            }
            index = url_end;
            continue;
        }
        let ch = chars[index];
        if index > 0 && !out.ends_with(' ') {
            let prev = chars[index - 1];
            let need_space = is_han_ascii_alphanumeric_boundary(prev, ch)
                || (prev == '%'
                    && index >= 2
                    && chars[index - 2].is_ascii_digit()
                    && is_han_character(ch))
                || (is_han_character(prev)
                    && ch == '$'
                    && chars.get(index + 1).is_some_and(char::is_ascii_digit))
                || (is_han_character(prev)
                    && ch == '-'
                    && chars.get(index + 1) == Some(&'-')
                    && chars
                        .get(index + 2)
                        .is_some_and(char::is_ascii_alphanumeric));
            if need_space {
                out.push(' ');
                changed = true;
            }
        }
        out.push(ch);
        index += 1;
    }
    changed.then_some(out)
}

/// 执行标准 Pangu 空格并返回规整后的文本；无需变动时直接克隆原文。
pub fn apply_pangu_space(text: &str) -> String {
    pangu_space(text).unwrap_or_else(|| text.to_owned())
}

#[cfg(test)]
mod pangu_tests {
    use super::*;

    #[test]
    fn han_ascii_alphanumeric_helper_preserves_keyboard_spacing_contract() {
        assert_eq!(
            space_han_ascii_alphanumeric("用Rust写插件").as_deref(),
            Some("用 Rust 写插件")
        );
        assert_eq!(
            space_han_ascii_alphanumeric("800人").as_deref(),
            Some("800 人")
        );
        assert_eq!(
            space_han_ascii_alphanumeric("第3版").as_deref(),
            Some("第 3 版")
        );
        assert_eq!(space_han_ascii_alphanumeric("𠀀A").as_deref(), Some("𠀀 A"));
        assert_eq!(space_han_ascii_alphanumeric("用 Rust 写"), None);
        assert_eq!(space_han_ascii_alphanumeric("中-A"), None);
        assert_eq!(space_han_ascii_alphanumeric("50%的"), None);
        assert_eq!(space_han_ascii_alphanumeric("かなA"), None);
        assert_eq!(space_han_ascii_alphanumeric("한A"), None);
    }

    #[test]
    fn pangu_spaces_cjk_latin_digits_percent_currency_and_flags() {
        assert_eq!(
            pangu_space("用Rust写10个接口").as_deref(),
            Some("用 Rust 写 10 个接口")
        );
        assert_eq!(pangu_space("8月4日上线").as_deref(), Some("8 月 4 日上线"));
        assert_eq!(
            pangu_space("准确率84.55%以上的样本").as_deref(),
            Some("准确率 84.55% 以上的样本")
        );
        assert_eq!(
            pangu_space("耗时3.5s左右").as_deref(),
            Some("耗时 3.5s 左右")
        );
        assert_eq!(
            pangu_space("预算$400左右").as_deref(),
            Some("预算 $400 左右")
        );
        assert_eq!(
            pangu_space("启用--reasoning-budget参数").as_deref(),
            Some("启用 --reasoning-budget 参数")
        );
    }

    #[test]
    fn pangu_protects_urls_emails_code_identifiers_versions_and_indentation() {
        assert_eq!(
            pangu_space("打开https://example.com/v1/api?k=1看看").as_deref(),
            Some("打开 https://example.com/v1/api?k=1 看看")
        );
        assert_eq!(
            pangu_space("联系user_1+tag@example.com即可").as_deref(),
            Some("联系 user_1+tag@example.com 即可")
        );
        assert_eq!(
            pangu_space("检查account_identity和useState字段").as_deref(),
            Some("检查 account_identity 和 useState 字段")
        );
        assert_eq!(
            pangu_space("升级到v1.2.3版本").as_deref(),
            Some("升级到 v1.2.3 版本")
        );
        assert_eq!(
            pangu_space("访问https://example.com/docs/看看").as_deref(),
            Some("访问 https://example.com/docs/ 看看")
        );
        // 完整保留 Git diff 与嵌套列表的行首缩进。
        assert_eq!(
            pangu_space("    + 新增Rust模块\n\t子项v2说明").as_deref(),
            Some("    + 新增 Rust 模块\n\t子项 v2 说明")
        );
        assert_eq!(pangu_space("中-A"), None);
    }

    #[test]
    fn pangu_is_idempotent_and_returns_none_when_unchanged() {
        for already_clean in [
            "",
            "你好世界",
            "hello world v1.2.3 50%",
            "用 Rust 写 10 个接口，准确率 95% 以上",
            "打开 https://example.com/v1 看看",
            "    保留四空格缩进",
        ] {
            assert_eq!(pangu_space(already_clean), None, "{already_clean}");
        }
        let once = apply_pangu_space("升级v1.2.3后准确率95%以上");
        assert_eq!(once, "升级 v1.2.3 后准确率 95% 以上");
        assert_eq!(pangu_space(&once), None);
    }
}
