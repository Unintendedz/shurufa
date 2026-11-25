//! 手动置顶候选（`pinned_candidates`）与在线动态候选归位规则。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::journal::open_journal;
use crate::error::Result;
use crate::types::{CandidateSource, WordItem};

/// 在线动态候选座位归位：云候选排第 1 槽，AI 候选按原序排在列表末尾，不挪动本地候选。
pub fn rehome_online_candidates(
    candidates: &mut Vec<WordItem>,
    keep_dynamic_candidate_positions: bool,
) {
    if keep_dynamic_candidate_positions
        || candidates.is_empty()
        || !candidates
            .iter()
            .any(|item| item.source.takes_online_seat())
    {
        return;
    }
    let original = std::mem::take(candidates);
    let (retained, dynamic): (Vec<_>, Vec<_>) = original
        .into_iter()
        .partition(|item| !item.source.takes_online_seat());
    *candidates = retained;

    let mut ai = Vec::new();
    for item in dynamic {
        if item.source == CandidateSource::CloudSuggestion {
            let index = 1.min(candidates.len());
            candidates.insert(index, item);
        } else {
            ai.push(item);
        }
    }
    candidates.append(&mut ai);
}

/// 每个输入上下文记录唯一一条手动置顶候选；再次置顶覆盖前值。
pub fn record_pinned_candidate(user_db: &Path, context: &str, value: &str) -> Result<()> {
    let journal = open_journal(user_db)?;
    journal
        .prepare_cached(
            "INSERT INTO pinned_candidates(context_key,value) VALUES(?1,?2) ON CONFLICT(context_key) DO UPDATE SET value=excluded.value,updated_at=unixepoch()",
        )?
        .execute(params![context, value])?;
    Ok(())
}

/// 在已打开的日志连接上读取指定上下文的手动置顶词。
pub(crate) fn pinned_value(journal: &Connection, context: &str) -> Result<Option<String>> {
    Ok(journal
        .prepare_cached("SELECT value FROM pinned_candidates WHERE context_key=?1")?
        .query_row(params![context], |row| row.get(0))
        .optional()?)
}

/// 判断给定词是否为该上下文的手动置顶词；从不创建日志文件。
pub fn is_pinned_candidate(user_db: &Path, context: &str, value: &str) -> bool {
    pinned_candidate_for(user_db, context).as_deref() == Some(value)
}

/// 读取当前上下文手动置顶的词；日志不存在或读取失败时返回 `None`，绝不创建日志文件。
pub fn pinned_candidate_for(user_db: &Path, context: &str) -> Option<String> {
    if context.is_empty() || !user_db.try_exists().unwrap_or(false) {
        return None;
    }
    let journal = open_journal(user_db).ok()?;
    pinned_value(&journal, context).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::super::journal::test_support::{item, Dir};
    use super::*;

    fn words(items: &[WordItem]) -> Vec<&str> {
        items.iter().map(|item| item.word.as_str()).collect()
    }

    fn online(word: &str, source: CandidateSource) -> WordItem {
        WordItem::new("", word, 1, source, "")
    }

    #[test]
    fn ordinary_rows_skip_dynamic_partition_when_no_online_candidates_exist() {
        let mut list = vec![item("ni", "甲", 3), item("ni", "乙", 2)];
        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            rehome_online_candidates(&mut list, false);
        });
        assert_eq!(words(&list), ["甲", "乙"]);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn online_rows_return_to_cloud_and_tail_slots() {
        let mut list = vec![
            item("ni", "甲", 3),
            item("ni", "乙", 2),
            online("智", CandidateSource::AiSuggestion),
            item("ni", "丙", 1),
            online("云", CandidateSource::CloudSuggestion),
        ];
        rehome_online_candidates(&mut list, false);
        assert_eq!(words(&list), ["甲", "云", "乙", "丙", "智"]);
    }

    #[test]
    fn a_later_pin_replaces_the_earlier_one() {
        let dir = Dir::new();
        let journal = dir.journal();
        record_pinned_candidate(&journal, "ni'hao", "拟好").unwrap();
        assert!(is_pinned_candidate(&journal, "ni'hao", "拟好"));
        record_pinned_candidate(&journal, "ni'hao", "你好").unwrap();
        assert!(!is_pinned_candidate(&journal, "ni'hao", "拟好"));
        assert!(is_pinned_candidate(&journal, "ni'hao", "你好"));
        assert!(!is_pinned_candidate(&journal, "ni", "你好"));
    }
}
