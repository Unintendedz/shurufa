//! 第 1–5 位固定排位（`fixed_candidate_positions`）、手动置顶（`pinned_candidates`）与在线动态候选归位规则。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::journal::{open_journal, UNSTORABLE_ENTRY};
use crate::error::{EngineError, Result};
use crate::types::{CandidateSource, WordItem};

/// 当固定排位的词未出现在当前候选中时，按 `(entry_key, value)` 从词库补找的回调。
pub type FindCandidate<'a> = dyn FnMut(&str, &str) -> Option<WordItem> + 'a;

const FIXED_SLOT_COUNT: usize = 5;

/// 将 `(context, entry_key, value)` 固定到第 `1..=5` 位；该槽位原持有者会被顶出，并同步清理 `pinned_candidates` 冲突。
#[cfg(test)]
pub fn set_fixed_position(
    user_db: &Path,
    context: &str,
    entry_key: &str,
    value: &str,
    position: u8,
) -> Result<()> {
    set_fixed_position_with_pin_context(user_db, context, None, entry_key, value, position)
}

/// 写入第 `1..=5` 位固定排位，并同时清理 `context` 与可选 `pin_context` 在 `pinned_candidates` 中的冲突置顶。
pub(crate) fn set_fixed_position_with_pin_context(
    user_db: &Path,
    context: &str,
    pin_context: Option<&str>,
    entry_key: &str,
    value: &str,
    position: u8,
) -> Result<()> {
    if !(1..=FIXED_SLOT_COUNT as u8).contains(&position)
        || context.is_empty()
        || entry_key.is_empty()
        || value.is_empty()
    {
        return Err(EngineError::invalid(UNSTORABLE_ENTRY));
    }
    let journal = open_journal(user_db)?;
    journal.execute_batch("BEGIN IMMEDIATE")?;
    journal
        .prepare_cached(
            "DELETE FROM fixed_candidate_positions WHERE context_key=?1 AND position=?2",
        )?
        .execute(params![context, i32::from(position)])?;
    journal
        .prepare_cached(
            "INSERT INTO fixed_candidate_positions(context_key,entry_key,value,position) VALUES(?1,?2,?3,?4) \
             ON CONFLICT(context_key,entry_key,value) DO UPDATE SET position=excluded.position",
        )?
        .execute(params![context, entry_key, value, i32::from(position)])?;
    if position == 1 {
        journal
            .prepare_cached("DELETE FROM pinned_candidates WHERE context_key=?1 AND value!=?2")?
            .execute(params![context, value])?;
        if let Some(extra) = pin_context.filter(|extra| !extra.is_empty() && *extra != context) {
            journal
                .prepare_cached("DELETE FROM pinned_candidates WHERE context_key=?1 AND value!=?2")?
                .execute(params![extra, value])?;
        }
    } else {
        journal
            .prepare_cached("DELETE FROM pinned_candidates WHERE context_key=?1 AND value=?2")?
            .execute(params![context, value])?;
        if let Some(extra) = pin_context.filter(|extra| !extra.is_empty() && *extra != context) {
            journal
                .prepare_cached("DELETE FROM pinned_candidates WHERE context_key=?1 AND value=?2")?
                .execute(params![extra, value])?;
        }
    }
    journal.execute_batch("COMMIT")?;
    Ok(())
}

/// 取消指定候选的固定排位，并同步清除同词在 `pinned_candidates` 中的手动置顶。
#[cfg(test)]
pub fn clear_fixed_position(
    user_db: &Path,
    context: &str,
    entry_key: &str,
    value: &str,
) -> Result<()> {
    clear_fixed_position_with_pin_context(user_db, context, None, entry_key, value)
}

/// 取消指定候选的固定排位，并在同事务内清理 `context` 与可选 `pin_context` 中的手动置顶记录。
pub(crate) fn clear_fixed_position_with_pin_context(
    user_db: &Path,
    context: &str,
    pin_context: Option<&str>,
    entry_key: &str,
    value: &str,
) -> Result<()> {
    let journal = open_journal(user_db)?;
    journal.execute_batch("BEGIN IMMEDIATE")?;
    journal
        .prepare_cached(
            "DELETE FROM fixed_candidate_positions WHERE context_key=?1 AND entry_key=?2 AND value=?3",
        )?
        .execute(params![context, entry_key, value])?;
    journal
        .prepare_cached("DELETE FROM pinned_candidates WHERE context_key=?1 AND value=?2")?
        .execute(params![context, value])?;
    if let Some(extra) = pin_context.filter(|extra| !extra.is_empty() && *extra != context) {
        journal
            .prepare_cached("DELETE FROM pinned_candidates WHERE context_key=?1 AND value=?2")?
            .execute(params![extra, value])?;
    }
    journal.execute_batch("COMMIT")?;
    Ok(())
}

/// 判断 `(context, entry_key, value)` 是否记录了第 1–5 位固定排位。
#[cfg(test)]
pub fn is_fixed(user_db: &Path, context: &str, entry_key: &str, value: &str) -> bool {
    let Ok(journal) = open_journal(user_db) else {
        return false;
    };
    journal
        .prepare_cached(
            "SELECT 1 FROM fixed_candidate_positions WHERE context_key=?1 AND entry_key=?2 AND value=?3",
        )
        .and_then(|mut statement| statement.exists(params![context, entry_key, value]))
        .unwrap_or(false)
}

/// 按槽位升序读取当前上下文的所有固定排位记录 `(entry_key, value, position)`。
fn fixed_rows(user_db: &Path, context: &str) -> Result<Vec<(String, String, u8)>> {
    let journal = open_journal(user_db)?;
    let mut statement = journal.prepare_cached(
        "SELECT entry_key,value,position FROM fixed_candidate_positions WHERE context_key=?1 ORDER BY position",
    )?;
    let rows = statement.query_map(params![context], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (entry_key, value, pos) = row?;
        let Ok(position) = u8::try_from(pos) else {
            continue;
        };
        if !(1..=FIXED_SLOT_COUNT as u8).contains(&position) {
            continue;
        }
        if result.is_empty() {
            result.reserve_exact(FIXED_SLOT_COUNT);
        }
        result.push((entry_key, value, position));
    }
    Ok(result)
}

fn fixed_row_contains(rows: &[(WordItem, u8)], word: &str) -> bool {
    rows.iter().any(|(item, _)| item.word == word)
}

/// 仅对在线动态候选（云/AI）执行座位归位：云候选落第 1 槽，AI 候选按原序排在列表末尾。
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

    let mut cloud_index = 1.min(candidates.len());
    let mut ai = Vec::new();
    for item in dynamic {
        if matches!(
            item.source,
            CandidateSource::CloudSuggestion | CandidateSource::CloudPending
        ) {
            let insert_at = cloud_index.min(candidates.len());
            candidates.insert(insert_at, item);
            cloud_index = insert_at + 1;
        } else {
            ai.push(item);
        }
    }
    candidates.append(&mut ai);
}

/// 将当前上下文固定的第 1–5 位候选移动到目标席位，并标记 `fixed_position = 1..=5`；日志不存在时不创建。
#[cfg(test)]
pub fn apply_fixed_positions(
    user_db: &Path,
    context: &str,
    candidates: &mut Vec<WordItem>,
    include_missing: bool,
    find_candidate: Option<&mut FindCandidate<'_>>,
    keep_dynamic_candidate_positions: bool,
) {
    if context.is_empty() || candidates.is_empty() {
        return;
    }
    let journal_exists = user_db.try_exists().unwrap_or(false);
    apply_fixed_positions_with_state(
        user_db,
        context,
        candidates,
        include_missing,
        find_candidate,
        keep_dynamic_candidate_positions,
        journal_exists,
    );
}

/// 复用日志存在性检查的内部实现：无固定行且无在线行时零额外堆分配直接返回。
pub(crate) fn apply_fixed_positions_with_state(
    user_db: &Path,
    context: &str,
    candidates: &mut Vec<WordItem>,
    include_missing: bool,
    mut find_candidate: Option<&mut FindCandidate<'_>>,
    keep_dynamic_candidate_positions: bool,
    journal_exists: bool,
) {
    if context.is_empty() || candidates.is_empty() {
        return;
    }
    let fixed = if journal_exists {
        let Ok(fixed) = fixed_rows(user_db, context) else {
            return;
        };
        fixed
    } else {
        Vec::new()
    };

    let mut dynamic_candidates = Vec::new();
    if !keep_dynamic_candidate_positions
        && candidates
            .iter()
            .any(|item| item.source.takes_online_seat())
    {
        let original = std::mem::take(candidates);
        let (retained, dynamic): (Vec<_>, Vec<_>) = original
            .into_iter()
            .partition(|item| !item.source.takes_online_seat());
        *candidates = retained;
        dynamic_candidates = dynamic;
    }
    if fixed.is_empty() && dynamic_candidates.is_empty() {
        return;
    }

    let mut rows: Vec<(WordItem, u8)> = Vec::new();
    if !fixed.is_empty() {
        let candidate_by_word: HashMap<&str, &WordItem> = candidates
            .iter()
            .map(|item| (item.word.as_str(), item))
            .collect();
        rows.reserve(fixed.len());
        for (entry_key, value, position) in fixed {
            let found = match candidate_by_word.get(value.as_str()) {
                Some(existing) => Some((*existing).clone()),
                None if include_missing => find_candidate
                    .as_deref_mut()
                    .and_then(|find| find(&entry_key, &value)),
                None => None,
            };
            if let Some(mut item) = found {
                item.fixed_position = position;
                if position == 1 {
                    item.pinned_first = true;
                }
                rows.push((item, position));
            }
        }
        if !rows.is_empty() {
            candidates.retain(|item| !fixed_row_contains(&rows, &item.word));
        }
    }

    let mut cloud = Vec::new();
    let mut ai = Vec::new();
    for item in dynamic_candidates {
        if matches!(
            item.source,
            CandidateSource::CloudSuggestion | CandidateSource::CloudPending
        ) {
            cloud.push(item);
        } else {
            ai.push(item);
        }
    }

    let has_pre_fixed = candidates.iter().any(|item| item.fixed_position > 0);
    if rows.is_empty() && !has_pre_fixed {
        let mut cloud_index = 1.min(candidates.len());
        while cloud_index < candidates.len()
            && (candidates[cloud_index].pinned_first
                || candidates[cloud_index].source == CandidateSource::Contact)
        {
            cloud_index += 1;
        }
        for item in cloud {
            let insert_at = cloud_index.min(candidates.len());
            candidates.insert(insert_at, item);
            cloud_index = insert_at + 1;
        }
        candidates.append(&mut ai);
        return;
    }

    if has_pre_fixed {
        let remaining = std::mem::take(candidates);
        for item in remaining {
            if item.fixed_position > 0 {
                let pos = item.fixed_position.min(FIXED_SLOT_COUNT as u8);
                rows.push((item, pos));
            } else {
                candidates.push(item);
            }
        }
        rows.sort_by_key(|(_, pos)| *pos);
    }

    let total = candidates.len() + rows.len() + cloud.len();
    if total == 0 {
        *candidates = ai;
        return;
    }

    let mut slots: Vec<Option<WordItem>> = (0..total).map(|_| None).collect();
    for (item, position) in rows {
        let target = usize::from(position.saturating_sub(1)).min(total - 1);
        let seat = (target..total)
            .find(|&idx| slots[idx].is_none())
            .or_else(|| slots.iter().position(|slot| slot.is_none()));
        if let Some(seat) = seat {
            slots[seat] = Some(item);
        }
    }

    let mut ordinary = std::collections::VecDeque::from(std::mem::take(candidates));
    while ordinary
        .front()
        .is_some_and(|item| item.pinned_first || item.source == CandidateSource::Contact)
    {
        if let (Some(item), Some(seat)) = (
            ordinary.pop_front(),
            slots.iter().position(|slot| slot.is_none()),
        ) {
            slots[seat] = Some(item);
        } else {
            break;
        }
    }

    if slots[0].is_none() {
        if let Some(first_ordinary) = ordinary.pop_front() {
            slots[0] = Some(first_ordinary);
        }
    }

    for item in cloud {
        let seat = (1..total)
            .find(|&idx| slots[idx].is_none())
            .or_else(|| slots.iter().position(|slot| slot.is_none()));
        if let Some(seat) = seat {
            slots[seat] = Some(item);
        }
    }

    for item in ordinary {
        if let Some(seat) = slots.iter().position(|slot| slot.is_none()) {
            slots[seat] = Some(item);
        }
    }

    *candidates = slots.into_iter().flatten().collect();
    candidates.append(&mut ai);
}

/// 每个输入上下文记录唯一一条手动置顶候选；再次置顶覆盖前值，并同步移除 `fixed_candidate_positions` 中的旧第 1 位或同词固定冲突。
#[cfg(test)]
pub fn record_pinned_candidate(user_db: &Path, context: &str, value: &str) -> Result<()> {
    record_pinned_candidate_with_position_context(user_db, context, None, value)
}

/// 记录手动置顶并清理可选 `position_context` 下同一词汇或第 1 位的旧固定记录。
pub(crate) fn record_pinned_candidate_with_position_context(
    user_db: &Path,
    context: &str,
    position_context: Option<&str>,
    value: &str,
) -> Result<()> {
    let journal = open_journal(user_db)?;
    journal
        .prepare_cached(
            "INSERT INTO pinned_candidates(context_key,value) VALUES(?1,?2) \
             ON CONFLICT(context_key) DO UPDATE SET value=excluded.value,updated_at=unixepoch()",
        )?
        .execute(params![context, value])?;
    journal
        .prepare_cached(
            "DELETE FROM fixed_candidate_positions WHERE context_key=?1 AND (value=?2 OR position=1)",
        )?
        .execute(params![context, value])?;
    if let Some(extra) = position_context.filter(|extra| !extra.is_empty() && *extra != context) {
        journal
            .prepare_cached(
                "DELETE FROM fixed_candidate_positions WHERE context_key=?1 AND (value=?2 OR position=1)",
            )?
            .execute(params![extra, value])?;
    }
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
    fn fixed_word_scan_avoids_temporary_hash_allocation() {
        let rows = vec![(item("ni", "甲", 1), 1u8), (item("ni", "乙", 1), 2u8)];
        let (found, allocations) =
            crate::ime::personal_rerank::allocations::count(|| fixed_row_contains(&rows, "乙"));
        assert!(found);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn ordinary_rows_skip_dynamic_partition_when_no_online_candidates_exist() {
        let dir = Dir::new();
        let journal = dir.journal();
        let mut list = vec![item("ni", "甲", 3), item("ni", "乙", 2)];

        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            apply_fixed_positions(&journal, "ni", &mut list, false, None, false);
        });

        assert_eq!(words(&list), ["甲", "乙"]);
        assert_eq!(
            allocations, 0,
            "ordinary rows allocated {allocations} temporary buffers"
        );
    }

    #[test]
    fn slots_are_validated_unique_cleared_and_synced_with_pinned_candidates() {
        let dir = Dir::new();
        let journal = dir.journal();
        for position in [0u8, 6u8] {
            assert!(set_fixed_position(&journal, "ni'hao", "ni'hao", "拟好", position).is_err());
        }
        assert!(set_fixed_position(&journal, "", "ni'hao", "拟好", 1).is_err());

        set_fixed_position(&journal, "ni'hao", "ni'hao", "拟好", 1).unwrap();
        assert!(is_fixed(&journal, "ni'hao", "ni'hao", "拟好"));
        // 同一槽位抢占淘汰前任。
        set_fixed_position(&journal, "ni'hao", "ni'hao", "你好", 1).unwrap();
        assert!(!is_fixed(&journal, "ni'hao", "ni'hao", "拟好"));

        // 从置顶改到第 3 位不会保留旧的 pinned_candidates 鬼影；再次改置顶也不会保留第 3 位。
        record_pinned_candidate(&journal, "ni'hao", "你好").unwrap();
        assert!(is_pinned_candidate(&journal, "ni'hao", "你好"));
        assert!(!is_fixed(&journal, "ni'hao", "ni'hao", "你好"));

        set_fixed_position(&journal, "ni'hao", "ni'hao", "你好", 3).unwrap();
        assert!(!is_pinned_candidate(&journal, "ni'hao", "你好"));
        assert!(is_fixed(&journal, "ni'hao", "ni'hao", "你好"));

        let mut list = vec![
            item("ni'hao", "甲", 3),
            item("ni'hao", "乙", 2),
            item("ni'hao", "你好", 1),
        ];
        apply_fixed_positions(&journal, "ni'hao", &mut list, false, None, false);
        assert_eq!(words(&list), ["甲", "乙", "你好"]);
        assert_eq!(list[2].fixed_position, 3);

        clear_fixed_position(&journal, "ni'hao", "ni'hao", "你好").unwrap();
        assert!(!is_fixed(&journal, "ni'hao", "ni'hao", "你好"));

        // 右键「取消固定」同样可以清除手动置顶（pinned_candidates）。
        record_pinned_candidate(&journal, "ni'hao", "拟好").unwrap();
        assert!(is_pinned_candidate(&journal, "ni'hao", "拟好"));
        clear_fixed_position(&journal, "ni'hao", "ni'hao", "拟好").unwrap();
        assert!(!is_pinned_candidate(&journal, "ni'hao", "拟好"));
    }

    #[test]
    fn fixed_rows_move_by_word_and_online_rows_return_to_their_slots() {
        let dir = Dir::new();
        let journal = dir.journal();
        set_fixed_position(&journal, "ni", "ni", "丙", 1).unwrap();
        set_fixed_position(&journal, "ni", "ni", "戊", 2).unwrap();
        set_fixed_position(&journal, "ni", "ni", "庚", 5).unwrap();
        let mut list = vec![
            item("ni", "甲", 6),
            online("云", CandidateSource::CloudSuggestion),
            online("智", CandidateSource::AiSuggestion),
            item("ni", "乙", 5),
            item("ni", "丙", 4),
        ];
        let mut found =
            |key: &str, word: &str| (key == "ni" && word == "戊").then(|| item("ni", "戊", 1));
        apply_fixed_positions(&journal, "ni", &mut list, true, Some(&mut found), false);
        // 丙固定第 1 位，戊固定第 2 位，云候选让位顺延至第 3 位（下标 2），AI 在最后。
        assert_eq!(words(&list), ["丙", "戊", "云", "甲", "乙", "智"]);
        assert_eq!(list[0].fixed_position, 1);
        assert_eq!(list[1].fixed_position, 2);
    }

    #[test]
    fn fixed_third_slot_stays_pinned_while_cloud_and_ai_route_around_it() {
        let dir = Dir::new();
        let journal = dir.journal();
        set_fixed_position(&journal, "ni'hao", "ni'hao", "拟好", 3).unwrap();
        for online_source in [
            CandidateSource::CloudPending,
            CandidateSource::CloudSuggestion,
        ] {
            let mut list = vec![
                item("ni'hao", "你好", 10),
                item("ni'hao", "拟好", 9),
                item("ni'hao", "泥好", 8),
                online("☁", online_source),
                online("智", CandidateSource::AiSuggestion),
            ];
            apply_fixed_positions(&journal, "ni'hao", &mut list, false, None, false);
            assert_eq!(words(&list), ["你好", "☁", "拟好", "泥好", "智"]);
            assert_eq!(list[2].fixed_position, 3);
        }
    }

    #[test]
    fn fixed_rows_reserve_the_five_available_slots() {
        let dir = Dir::new();
        let journal = dir.journal();
        for position in 1..=5u8 {
            set_fixed_position(&journal, "ni", "ni", &format!("字{position}"), position).unwrap();
        }
        let rows = fixed_rows(&journal, "ni").unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows.capacity(), 5);
    }

    #[test]
    fn empty_fixed_rows_do_not_reserve_unused_slots() {
        let dir = Dir::new();
        let journal = dir.journal();
        let rows = fixed_rows(&journal, "ni").unwrap();
        assert!(rows.is_empty());
        assert_eq!(rows.capacity(), 0);
    }

    #[test]
    fn a_missing_journal_fixes_nothing_and_is_not_created() {
        let dir = Dir::new();
        let journal = dir.journal();
        let mut list = vec![
            item("ni", "甲", 3),
            item("ni", "乙", 2),
            online("智", CandidateSource::AiSuggestion),
            item("ni", "丙", 1),
            online("云", CandidateSource::CloudSuggestion),
        ];
        apply_fixed_positions(&journal, "ni", &mut list, true, None, false);
        assert_eq!(words(&list), ["甲", "云", "乙", "丙", "智"]);
        assert!(!is_pinned_candidate(&journal, "ni", "甲"));
        assert!(!journal.exists());
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
