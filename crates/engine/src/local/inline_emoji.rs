//! Shurufa 精选「中文词 → Emoji」内存映射表（供 `slot_allocator` 在全部重排之后做 O(1) 投影）。
//!
//! 不同于传统按拼音码做 SQLite 前缀扫描的 `local/emoji.rs`，本表只按已解码并完成重排的中文词做精确反查：
//! 全拼、双拼、模糊音与纠错天然一致，零每键 SQLite 锁与文件排序，并通过编译期单测严格守卫词表卫生。

use std::collections::HashMap;
use std::sync::OnceLock;

const CURATED_EMOJI_TSV: &str = include_str!("curated_emoji.tsv");

static CURATED_INDEX: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();

fn table() -> &'static HashMap<&'static str, Vec<&'static str>> {
    CURATED_INDEX.get_or_init(|| {
        let mut map = HashMap::with_capacity(4800);
        for line in CURATED_EMOJI_TSV.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((word, rest)) = trimmed.split_once('\t') else {
                continue;
            };
            let emojis: Vec<&'static str> = rest.split_whitespace().collect();
            if !word.is_empty() && !emojis.is_empty() {
                map.insert(word, emojis);
            }
        }
        map
    })
}

/// 按精确中文词查精选 Emoji 列表（已按中文社交常用序排好）；未命中返回空切片。
pub fn lookup_inline_emojis(word: &str) -> &'static [&'static str] {
    table().get(word).map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CJK_BUTTONS: &[char] = &[
        '🈁', '🈂', '🈷', '🈶', '🈯', '🉐', '🈹', '🈚', '🈲', '🉑', '🈸', '🈴', '🈳', '㊗', '㊙',
        '🈺', '🈵',
    ];

    const SINGLE_CHAR_WHITELIST: &str = "笑哭赞爱心火水钱猫狗猪牛马鸡鱼鸟兔虎龙熊羊猴蛇鼠鸭鹅鹿龟虾蟹花树雨雪云雷星风酒茶肉蛋糖书刀剑车船门锁灯鬼屎困晕吐怒怕汗吻嘘";

    fn strip_vs16(text: &str) -> String {
        text.chars().filter(|&c| c != '\u{FE0F}').collect()
    }

    #[test]
    fn curated_emoji_table_meets_all_hygiene_contracts() {
        let map = table();
        assert!(map.len() >= 4000, "精选表应包含 4000+ 中文词映射");
        for (&word, emojis) in map {
            assert!(
                !emojis.is_empty() && emojis.len() <= 4,
                "{word}: {emojis:?}"
            );
            let char_count = word.chars().count();
            assert!((1..=6).contains(&char_count), "词长越界: {word}");
            if char_count == 1 {
                let ch = word.chars().next().unwrap();
                assert!(
                    SINGLE_CHAR_WHITELIST.contains(ch),
                    "单字 {word} 不在白名单内"
                );
            }
            let mut seen = Vec::new();
            for &emoji in emojis {
                assert!(
                    !emoji
                        .chars()
                        .any(|c| ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)),
                    "{word} 含肤色修饰符: {emoji}"
                );
                assert!(!emoji.contains('\u{20E3}'), "{word} 含带框数字: {emoji}");
                assert!(
                    !emoji.chars().any(|c| CJK_BUTTONS.contains(&c)),
                    "{word} 含日文方块汉字: {emoji}"
                );
                assert!(
                    emoji.chars().any(|c| (c as u32) >= 0x2000),
                    "{word} 含非表情符号: {emoji}"
                );
                if char_count == 1 {
                    assert!(
                        !emoji
                            .chars()
                            .any(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)),
                        "单字 {word} 不得挂国旗: {emoji}"
                    );
                }
                let normalized = strip_vs16(emoji);
                assert!(
                    !seen.contains(&normalized),
                    "{word} 存在变体选择符重复: {emojis:?}"
                );
                seen.push(normalized);
            }
        }
    }

    #[test]
    fn curated_emoji_table_covers_core_chat_words_and_blocks_function_words() {
        assert_eq!(lookup_inline_emojis("思考"), &["🤔"]);
        assert_eq!(lookup_inline_emojis("开心").first().copied(), Some("😄"));
        assert_eq!(lookup_inline_emojis("笑哭").first().copied(), Some("😂"));
        assert_eq!(lookup_inline_emojis("捂脸"), &["🤦"]);
        assert_eq!(lookup_inline_emojis("点赞"), &["👍"]);
        assert_eq!(lookup_inline_emojis("收到").first().copied(), Some("👌"));
        assert_eq!(lookup_inline_emojis("晚安").first().copied(), Some("🌙"));
        assert_eq!(lookup_inline_emojis("摸鱼").first().copied(), Some("🐟"));
        assert_eq!(lookup_inline_emojis("破防").first().copied(), Some("😭"));
        assert_eq!(lookup_inline_emojis("中国"), &["🇨🇳"]);
        for blocked in ["一", "不", "的", "得", "有", "无", "好", "中", "上", "下"] {
            assert!(
                lookup_inline_emojis(blocked).is_empty(),
                "高频虚字/功能字 {blocked} 不应触发内联 Emoji"
            );
        }
    }
}
