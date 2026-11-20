//! 独立个人学习叠加层（Stage 4）：永不改写主词库 `msime-pinyin.db` 静态权重。
//!
//! 用户明确改选（非空格接受默认首选）写入 `msime_user.db` 的追加式事件表
//! `personal_selection_events`，查询时按 30 天半衰期做指数衰减计数 `n_user(w)`，
//! 与静态先验按贝叶斯平滑后验 `alpha * P_static(w) + n_user(w)` 叠加排序：
//! - 单次误选不会击穿核心高频词（`n_user < 1.8` 不得竞争第 1 槽，且核心词先验质量天然更高）；
//! - 首次明确选择冷门专名即可稳定获得首屏召回席位（第 2–4 位）；
//! - 连续 2–3 次明确选择自然竞争第 1 槽，且 30 天半衰期使旧习惯自然回落；
//! - 显式「置顶」（`pinned_candidates`）保持第 1 槽硬约束；
//! - 上屏后紧接退格删光刚上屏的字数时，按事件 ID 确定性回滚该次学习记录。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, TransactionBehavior};

use super::journal::{existing_journal, open_journal, UNSTORABLE_ENTRY};
use super::ranking::candidate_dictionary_key;
use crate::error::{EngineError, Result};
use crate::types::WordItem;

/// 个人选择事件的指数衰减半衰期：30 天（秒）。
pub const HALF_LIFE_SECONDS: f64 = 30.0 * 86_400.0;
/// 超过 180 天（约 6 个半衰期，残余权重 < 0.016）的历史事件在写入时自动裁剪。
pub const RETENTION_SECONDS: i64 = 180 * 86_400;
/// 事件表最大保留条数，防止长期使用无限增长。
pub const MAX_PERSONAL_EVENTS: i64 = 50_000;
/// 贝叶斯平滑静态先验等效伪计数 `alpha`：
/// 跨同码全部静态词分布总质量为 `alpha = 3.0`，单次明确选冷门词（`n_user = 1.0`）
/// 最多只会排在至多 2 个占比超 1/3 的高频静态词之后，自然保证首屏第 2–4 位召回；
/// 对占比 ~90% 的超高频核心词（先验质量 ~2.70），连续 3 次明确选择即可自然反超。
pub const BAYESIAN_PRIOR_ALPHA: f64 = 3.0;
/// 竞争第 1 槽所需的最低衰减明确选择次数（至少 2 次近期明确选择，彻底杜绝单次误选翻首位）。
pub const SLOT_ZERO_MIN_DECAYED_PICKS: f64 = 1.8;

/// 当前 Unix 秒时间戳。
pub fn unix_now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// 追加一条用户明确选词事件，返回自增事件 `id`（供紧接退格回滚使用）。
pub fn record_personal_selection(
    user_db: &Path,
    entry_key: &str,
    value: &str,
    now_unix: i64,
) -> Result<i64> {
    if entry_key.is_empty() || value.is_empty() {
        return Err(EngineError::invalid(UNSTORABLE_ENTRY));
    }
    let journal = open_journal(user_db)?;
    let tx = rusqlite::Transaction::new_unchecked(&journal, TransactionBehavior::Immediate)?;
    tx.prepare_cached(
        "INSERT INTO personal_selection_events(entry_key,value,selected_at) VALUES(?1,?2,?3)",
    )?
    .execute(params![entry_key, value, now_unix])?;
    let event_id = tx.last_insert_rowid();
    // 定期裁剪超过 180 天或超出上限的陈旧事件（仅在每 64 次写入时触发，避免热路径额外扫描）。
    if event_id % 64 == 0 {
        let cutoff = now_unix.saturating_sub(RETENTION_SECONDS);
        tx.prepare_cached("DELETE FROM personal_selection_events WHERE selected_at < ?1")?
            .execute(params![cutoff])?;
        tx.prepare_cached(
            "DELETE FROM personal_selection_events WHERE id <= (SELECT id FROM personal_selection_events ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        )?
        .execute(params![MAX_PERSONAL_EVENTS])?;
    }
    tx.commit()?;
    Ok(event_id)
}

/// 按事件 ID 确定性回滚一次刚写入的个人选词学习事件（如紧接退格删光整段刚上屏词）。
pub fn delete_personal_selection_event(user_db: &Path, event_id: i64) -> Result<bool> {
    let Some(journal) = existing_journal(user_db) else {
        return Ok(false);
    };
    let deleted = journal
        .prepare_cached("DELETE FROM personal_selection_events WHERE id=?1")?
        .execute(params![event_id])?;
    Ok(deleted > 0)
}

/// 计算某个完整拼音键下各词经 30 天半衰期指数衰减后的有效明确选择次数；无日志时不创建文件。
pub fn decayed_counts_for_key(
    user_db: &Path,
    entry_key: &str,
    now_unix: i64,
) -> HashMap<String, f64> {
    let mut counts = HashMap::new();
    if entry_key.is_empty() {
        return counts;
    }
    let Some(journal) = existing_journal(user_db) else {
        return counts;
    };
    let Ok(mut statement) = journal.prepare_cached(
        "SELECT value, selected_at FROM personal_selection_events WHERE entry_key=?1",
    ) else {
        return counts;
    };
    let Ok(rows) = statement.query_map(params![entry_key], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return counts;
    };
    for row in rows.flatten() {
        let (value, selected_at) = row;
        let age = now_unix.saturating_sub(selected_at).max(0) as f64;
        let weight = 0.5f64.powf(age / HALF_LIFE_SECONDS);
        if weight >= 1e-4 {
            *counts.entry(value).or_insert(0.0) += weight;
        }
    }
    counts
}

/// 独立个人叠加层重排的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OverlayOutcome {
    /// 候选顺序发生变化。
    pub moved: bool,
    /// 第 0 位由手动置顶或个人衰减叠加层确立，下游 `personal_context_rerank_order` 不得再拿未改写的静态权重覆盖它。
    pub leader_protected: bool,
}

/// 用独立贝叶斯衰减叠加层重排前段同音词典候选；绝不修改任何 `WordItem::weight`。
///
/// 仅作用于开头连续的同键拼音词典行（跳过整句、前缀补全与混输行）。
pub fn apply_personal_overlay(
    user_db: &Path,
    entry_key: &str,
    pinned_leader: Option<&str>,
    candidates: &mut [WordItem],
    now_unix: i64,
) -> OverlayOutcome {
    if entry_key.is_empty() || candidates.len() < 2 {
        return OverlayOutcome::default();
    }
    // 收集开头连续且与 `entry_key` 完全同音的拼音词典行。
    let group_len = candidates
        .iter()
        .take_while(|item| {
            item.source.is_dictionary()
                && item.scheme.is_pinyin()
                && candidate_dictionary_key(item, entry_key) == entry_key
        })
        .count();
    if group_len < 2 {
        return OverlayOutcome::default();
    }
    let pinned_idx = pinned_leader.and_then(|pin| {
        candidates[..group_len]
            .iter()
            .position(|item| item.word.as_str() == pin)
    });
    let counts = decayed_counts_for_key(user_db, entry_key, now_unix);
    if counts.is_empty() {
        let Some(pin_pos) = pinned_idx else {
            return OverlayOutcome::default();
        };
        if pin_pos > 0 {
            candidates[..=pin_pos].rotate_right(1);
        }
        return OverlayOutcome {
            moved: pin_pos > 0,
            leader_protected: true,
        };
    }
    let total_static_mass: f64 = candidates[..group_len]
        .iter()
        .map(|item| item.weight.max(1) as f64)
        .sum::<f64>()
        .max(1.0);
    let mut posteriors = Vec::with_capacity(group_len);
    let mut user_counts = Vec::with_capacity(group_len);
    for item in &candidates[..group_len] {
        let p_static = item.weight.max(1) as f64 / total_static_mass;
        let n_user = counts.get(item.word.as_str()).copied().unwrap_or(0.0);
        posteriors.push(BAYESIAN_PRIOR_ALPHA * p_static + n_user);
        user_counts.push(n_user);
    }

    let mut leader_idx = pinned_idx.unwrap_or(0);
    let mut overlay_promoted_leader = false;
    if pinned_idx.is_none() {
        let mut best_score = posteriors[0];
        for idx in 1..group_len {
            if user_counts[idx] >= SLOT_ZERO_MIN_DECAYED_PICKS && posteriors[idx] > best_score {
                best_score = posteriors[idx];
                leader_idx = idx;
                overlay_promoted_leader = true;
            }
        }
    }
    let leader_protected = pinned_idx.is_some() || overlay_promoted_leader;

    let mut order: Vec<usize> = Vec::with_capacity(group_len);
    order.push(leader_idx);
    let mut rest: Vec<usize> = (0..group_len).filter(|&idx| idx != leader_idx).collect();
    // 余下同音词按贝叶斯后验降序稳定排布：首次明确选择的冷门专名（`n_user ≈ 1.0`）自动进入首屏前排。
    rest.sort_by(|&a, &b| posteriors[b].total_cmp(&posteriors[a]).then(a.cmp(&b)));
    order.extend(rest);

    if order.iter().enumerate().all(|(seat, &idx)| seat == idx) {
        return OverlayOutcome {
            moved: false,
            leader_protected,
        };
    }
    crate::ordering::apply_order(&mut candidates[..group_len], &order);
    OverlayOutcome {
        moved: true,
        leader_protected,
    }
}

#[cfg(test)]
mod tests {
    use super::super::journal::test_support::{item, Dir};
    use super::*;

    fn words(items: &[WordItem]) -> Vec<&str> {
        items.iter().map(|row| row.word.as_str()).collect()
    }

    #[test]
    fn single_accidental_pick_never_breaks_core_word_and_recalls_rare_proper_noun_to_page_one() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_800_000_000;

        // 模拟 `yi'ge`：「一个」占 90%+ 静态先验，冷门专名「义戈」埋在第 8 位（第 2 页）。
        let mut list = vec![
            item("yi'ge", "一个", 508_964),
            item("yi'ge", "一格", 20_030),
            item("yi'ge", "一哥", 15_000),
            item("yi'ge", "衣阁", 5_000),
            item("yi'ge", "移格", 3_000),
            item("yi'ge", "艺阁", 2_000),
            item("yi'ge", "依格", 1_000),
            item("yi'ge", "义戈", 50),
        ];
        let original_weights: Vec<i64> = list.iter().map(|row| row.weight).collect();

        // 1 次明确选择冷门专名「义戈」：
        let event_id = record_personal_selection(&journal, "yi'ge", "义戈", now).unwrap();
        let outcome = apply_personal_overlay(&journal, "yi'ge", None, &mut list, now);
        assert!(outcome.moved && !outcome.leader_protected);
        // 「一个」稳守首位（单次误选不击穿核心词），「义戈」跃升至第 2 位（首屏召回）！
        assert_eq!(list[0].word, "一个");
        assert_eq!(list[1].word, "义戈");
        // 静态权重逐条保持原值，零污染！
        let mut weights_after: Vec<i64> = list.iter().map(|row| row.weight).collect();
        weights_after.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(weights_after, original_weights);

        // 紧接退格删除后按事件 ID 确定性回滚：「义戈」立即回到原静态位次！
        assert!(delete_personal_selection_event(&journal, event_id).unwrap());
        let mut fresh = vec![
            item("yi'ge", "一个", 508_964),
            item("yi'ge", "一格", 20_030),
            item("yi'ge", "一哥", 15_000),
            item("yi'ge", "衣阁", 5_000),
            item("yi'ge", "移格", 3_000),
            item("yi'ge", "艺阁", 2_000),
            item("yi'ge", "依格", 1_000),
            item("yi'ge", "义戈", 50),
        ];
        assert!(!apply_personal_overlay(&journal, "yi'ge", None, &mut fresh, now).moved);
        assert_eq!(words(&fresh)[7], "义戈");
    }

    #[test]
    fn repeated_picks_compete_for_slot_zero_decay_after_30_days_and_respect_pin() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_800_000_000;

        let make = || {
            vec![
                item("yi'ge", "一个", 508_964),
                item("yi'ge", "一格", 20_030),
                item("yi'ge", "义戈", 50),
            ]
        };

        // 第 2 次明确选择仍不击穿 90% 静态先验核心词（后验 2.0 < 2.88）。
        record_personal_selection(&journal, "yi'ge", "义戈", now).unwrap();
        record_personal_selection(&journal, "yi'ge", "义戈", now + 10).unwrap();
        let mut list = make();
        let out2 = apply_personal_overlay(&journal, "yi'ge", None, &mut list, now + 20);
        assert!(out2.moved && !out2.leader_protected);
        assert_eq!(words(&list), ["一个", "义戈", "一格"]);

        // 第 3 次明确选择（后验 3.0 > 2.88）自然升至首位！
        record_personal_selection(&journal, "yi'ge", "义戈", now + 30).unwrap();
        let mut list = make();
        let out3 = apply_personal_overlay(&journal, "yi'ge", None, &mut list, now + 40);
        assert!(out3.moved && out3.leader_protected);
        assert_eq!(words(&list), ["义戈", "一个", "一格"]);

        // 显式置顶「一个」作为 L4 硬约束，即使「义戈」已有 3 次明确选择也不得抢占第 1 槽。
        let mut pinned = make();
        let out_pin =
            apply_personal_overlay(&journal, "yi'ge", Some("一个"), &mut pinned, now + 40);
        assert!(out_pin.moved && out_pin.leader_protected);
        assert_eq!(words(&pinned), ["一个", "义戈", "一格"]);

        // 30 天半衰期后，3 次旧选择衰减为 1.5（< 1.8 首位门槛），核心词「一个」自然恢复首位，
        // 「义戈」保留在首屏次席。
        let mut decayed = make();
        let out_decay = apply_personal_overlay(
            &journal,
            "yi'ge",
            None,
            &mut decayed,
            now + 30 * 86_400 + 40,
        );
        assert!(out_decay.moved && !out_decay.leader_protected);
        assert_eq!(words(&decayed), ["一个", "义戈", "一格"]);
    }
}
