//! 宿主侧文本与表情目录薄包装函数。

use std::path::Path;

use super::options::{runtime_paths, EngineOptions};
use crate::assets;
use crate::error::Result;
use crate::local::catalog::{self, EmojiCatalogItem, EmojiCatalogSlice, EmojiSymbolGroup};
use crate::shuangpin::hints::ShuangpinKeyHint;

/// 基于当前代次 `msime-pinyin.db` 将汉字串转为拼音。
pub fn hanzi_to_pinyin(options: &EngineOptions, text: &str) -> String {
    crate::dictionary::hanzi::hanzi_to_pinyin(
        &runtime_paths(options).dictionary(assets::MAIN_DICTIONARY),
        text,
    )
}

/// 一批联系人姓名共用词库连接与整词查询，顺序与输入一致。
pub fn hanzi_to_pinyin_batch(options: &EngineOptions, texts: &[&str]) -> Vec<String> {
    crate::dictionary::hanzi::hanzi_to_pinyin_batch(
        &runtime_paths(options).dictionary(assets::MAIN_DICTIONARY),
        texts,
    )
}

pub fn normalize_full_pinyin(input: &str, expected_syllables: usize) -> String {
    crate::pinyin::normalize::normalize_full_pinyin(input, expected_syllables)
}

pub fn shuangpin_key_hints(profile: &str) -> Vec<ShuangpinKeyHint> {
    crate::shuangpin::hints::shuangpin_key_hints(profile)
}

pub fn shuangpin_zero_initials(profile: &str) -> Vec<(&'static str, &'static str)> {
    crate::shuangpin::hints::shuangpin_zero_initials(profile)
}

pub fn emoji_catalog_filtered_page(
    resources: &str,
    search: &str,
    category: &str,
    group: &str,
    offset: usize,
    limit: u16,
    parent: &str,
) -> Result<Vec<EmojiCatalogItem>> {
    let slice = catalog::read_emoji_catalog_slice(
        Path::new(resources),
        search,
        category,
        group,
        offset,
        usize::from(limit),
        parent,
        true,
    )?;
    Ok(slice.items)
}

pub fn emoji_catalog_slice(
    resources: &str,
    search: &str,
    category: &str,
    group: &str,
    offset: usize,
    limit: u16,
    parent: &str,
) -> Result<EmojiCatalogSlice> {
    catalog::read_emoji_catalog_slice(
        Path::new(resources),
        search,
        category,
        group,
        offset,
        usize::from(limit),
        parent,
        false,
    )
}

pub fn emoji_symbol_groups(resources: &str) -> Result<Vec<EmojiSymbolGroup>> {
    catalog::emoji_symbol_groups(Path::new(resources))
}

pub fn emoji_catalog_groups(resources: &str, category: &str) -> Result<Vec<String>> {
    catalog::emoji_catalog_groups(Path::new(resources), category)
}
