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

/// 判断候选是否为 `entry_key` 对应的未纠错、非模糊精确拼音词典同音词。
fn is_exact_homophone(item: &WordItem, entry_key: &str) -> bool {
    item.source.is_dictionary()
        && item.scheme.is_pinyin()
        && !item.fuzzy
        && item.corrected_from.is_empty()
        && candidate_dictionary_key(item, entry_key) == entry_key
}

/// 为语境同音竞争计算融合了 Stage 4 衰减个人选词事件的等效静态先验权重。
///
/// 推导：`alpha * P_static(w) + n_user(w) = (alpha / Z) * (w_static + (Z / alpha) * n_user(w))`；
/// 当所有同音词均无个人选词记录（`n_user == 0`）时返回 `None`，使下游 `ln(weight)` 与冷启动完全等价；
/// 有明确选词记录时既不改写主词库静态权重，又能防止中性前文把刚学会的常用词打回静态次席。
pub fn effective_bayesian_weights(
    user_db: &Path,
    entry_key: &str,
    items: &[&WordItem],
    now_unix: i64,
) -> Option<Vec<f64>> {
    if entry_key.is_empty() || items.len() < 2 {
        return None;
    }
    let counts = decayed_counts_for_key(user_db, entry_key, now_unix);
    if counts.is_empty() {
        return None;
    }
    let any_matched = items
        .iter()
        .any(|item| counts.get(item.word.as_str()).is_some_and(|&n| n > 0.0));
    if !any_matched {
        return None;
    }
    let total_static_mass: f64 = items
        .iter()
        .map(|item| item.weight.max(1) as f64)
        .sum::<f64>()
        .max(1.0);
    let user_unit_weight = total_static_mass / BAYESIAN_PRIOR_ALPHA;
    Some(
        items
            .iter()
            .map(|item| {
                let base = item.weight.max(1) as f64;
                let n_user = counts.get(item.word.as_str()).copied().unwrap_or(0.0);
                (base + user_unit_weight * n_user).max(1.0)
            })
            .collect(),
    )
}

/// 用独立贝叶斯衰减叠加层重排同音词典候选；绝不修改任何 `WordItem::weight`。
///
/// 要求第 0 位本身是 `entry_key` 的精确拼音词典命中；随后跨越中间可能夹入的词网格整句与长词补全，
/// 收集全部精确同音词典行一起参与贝叶斯平滑与手动置顶，避免「登录/登陆」「出发/除法」这类因补全词
/// 夹层而断开的同音词脱离个人学习与置顶控制。
pub fn apply_personal_overlay(
    user_db: &Path,
    entry_key: &str,
    pinned_leader: Option<&str>,
    candidates: &mut [WordItem],
    now_unix: i64,
) -> OverlayOutcome {
    if entry_key.is_empty()
        || candidates.len() < 2
        || !is_exact_homophone(&candidates[0], entry_key)
    {
        return OverlayOutcome::default();
    }
    let counts = decayed_counts_for_key(user_db, entry_key, now_unix);
    if counts.is_empty() && pinned_leader.is_none() {
        return OverlayOutcome::default();
    }
    // 跨越可能夹在同音词之间的整句与长词补全，收集全部精确同音词典行下标。
    let exact_indices: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| is_exact_homophone(item, entry_key).then_some(idx))
        .collect();
    let group_len = exact_indices.len();
    if group_len < 2 {
        return OverlayOutcome::default();
    }
    let pinned_pos = pinned_leader.and_then(|pin| {
        exact_indices
            .iter()
            .position(|&raw_idx| candidates[raw_idx].word.as_str() == pin)
    });
    if counts.is_empty() {
        let Some(pin_pos) = pinned_pos else {
            return OverlayOutcome::default();
        };
        let raw_idx = exact_indices[pin_pos];
        if raw_idx > 0 {
            candidates[..=raw_idx].rotate_right(1);
        }
        return OverlayOutcome {
            moved: raw_idx > 0,
            leader_protected: true,
        };
    }
    let total_static_mass: f64 = exact_indices
        .iter()
        .map(|&raw_idx| candidates[raw_idx].weight.max(1) as f64)
        .sum::<f64>()
        .max(1.0);
    let mut posteriors = Vec::with_capacity(group_len);
    let mut user_counts = Vec::with_capacity(group_len);
    for &raw_idx in &exact_indices {
        let item = &candidates[raw_idx];
        let p_static = item.weight.max(1) as f64 / total_static_mass;
        let n_user = counts.get(item.word.as_str()).copied().unwrap_or(0.0);
        posteriors.push(BAYESIAN_PRIOR_ALPHA * p_static + n_user);
        user_counts.push(n_user);
    }

    let mut leader_pos = pinned_pos.unwrap_or(0);
    let mut overlay_promoted_leader = false;
    if pinned_pos.is_none() {
        let mut best_score = posteriors[0];
        for pos in 1..group_len {
            if user_counts[pos] >= SLOT_ZERO_MIN_DECAYED_PICKS && posteriors[pos] > best_score {
                best_score = posteriors[pos];
                leader_pos = pos;
                overlay_promoted_leader = true;
            }
        }
    }
    let leader_protected = pinned_pos.is_some() || overlay_promoted_leader;

    let mut exact_order: Vec<usize> = Vec::with_capacity(group_len);
    exact_order.push(leader_pos);
    let mut rest: Vec<usize> = (0..group_len).filter(|&pos| pos != leader_pos).collect();
    // 余下同音词按贝叶斯后验降序稳定排布：首次明确选择的冷门专名（`n_user ≈ 1.0`）自动进入首屏前排。
    rest.sort_by(|&a, &b| posteriors[b].total_cmp(&posteriors[a]).then(a.cmp(&b)));
    exact_order.extend(rest);

    let leading_contiguous = exact_indices
        .iter()
        .enumerate()
        .take_while(|&(pos, &raw_idx)| pos == raw_idx)
        .count();
    if leading_contiguous == group_len {
        if exact_order
            .iter()
            .enumerate()
            .all(|(seat, &pos)| seat == pos)
        {
            return OverlayOutcome {
                moved: false,
                leader_protected,
            };
        }
        crate::ordering::apply_order(&mut candidates[..group_len], &exact_order);
        return OverlayOutcome {
            moved: true,
            leader_protected,
        };
    }

    // 存在夹层（整句/长词补全）时：凡是新首位、显式置顶、带个人选词记录或原本即在连续前段的同音词
    // 均连续排在最前，确保一次明确改选即可越过补全词进入首屏前排，且被替代的原首位紧随第 2 位；
    // 中间的非同音夹层保持相对次序，其余未选过的尾部同音词顺次填回后排空位。
    let max_exact_idx = *exact_indices.last().expect("group_len >= 2");
    let mut front_exact = Vec::with_capacity(group_len);
    let mut tail_exact = Vec::new();
    for &pos in &exact_order {
        let promoted_to_front = pos == leader_pos
            || pinned_pos == Some(pos)
            || user_counts[pos] > 0.0
            || exact_indices[pos] < leading_contiguous;
        if promoted_to_front {
            front_exact.push(exact_indices[pos]);
        } else {
            tail_exact.push(exact_indices[pos]);
        }
    }
    let mut full_order = Vec::with_capacity(max_exact_idx + 1);
    full_order.extend_from_slice(&front_exact);
    let mut tail_iter = tail_exact.into_iter();
    let mut exact_cursor = leading_contiguous;
    for raw_idx in leading_contiguous..=max_exact_idx {
        if exact_cursor < group_len && exact_indices[exact_cursor] == raw_idx {
            exact_cursor += 1;
            if let Some(next_tail) = tail_iter.next() {
                full_order.push(next_tail);
            }
        } else {
            full_order.push(raw_idx);
        }
    }
    debug_assert_eq!(full_order.len(), max_exact_idx + 1);
    if full_order
        .iter()
        .enumerate()
        .all(|(seat, &raw_idx)| seat == raw_idx)
    {
        return OverlayOutcome {
            moved: false,
            leader_protected,
        };
    }
    crate::ordering::apply_order(&mut candidates[..=max_exact_idx], &full_order);
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

    #[test]
    fn interleaved_generated_and_longer_phrases_do_not_block_overlay_learning_or_pin() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_800_000_000;

        let make = || {
            let mut gen_row = item("deng'lu", "邓禄", 10);
            gen_row.source = crate::types::CandidateSource::Generated;
            let longer_a = item("deng'lu'shi'jian", "登录时间", 400_000);
            let longer_b = item("deng'lu'ming", "登录名", 200_000);
            let single = item("deng", "等", 800_000);
            vec![
                item("deng'lu", "登录", 502_000),
                gen_row,
                longer_a,
                longer_b,
                item("deng'lu", "登陆", 5_000),
                single,
            ]
        };

        // 未学习、未置顶时保持原序且有效先验权重返回 None。
        let mut untouched = make();
        assert!(!apply_personal_overlay(&journal, "deng'lu", None, &mut untouched, now).moved);
        let refs: Vec<&WordItem> = vec![&untouched[0], &untouched[4]];
        assert!(effective_bayesian_weights(&journal, "deng'lu", &refs, now).is_none());

        // 仅手动置顶夹层后的同音词「登陆」：直接旋至首位且保持「登录」在次席。
        let mut pinned_only = make();
        let out_pin =
            apply_personal_overlay(&journal, "deng'lu", Some("登陆"), &mut pinned_only, now);
        assert!(out_pin.moved && out_pin.leader_protected);
        assert_eq!(
            words(&pinned_only),
            ["登陆", "登录", "邓禄", "登录时间", "登录名", "等"]
        );

        // 1 次明确改选「登陆」：跨过整句与补全夹层进入第 2 位，「登录」稳守第 1 位。
        record_personal_selection(&journal, "deng'lu", "登陆", now).unwrap();
        let mut after1 = make();
        let out1 = apply_personal_overlay(&journal, "deng'lu", None, &mut after1, now + 1);
        assert!(out1.moved && !out1.leader_protected);
        assert_eq!(
            words(&after1),
            ["登录", "登陆", "邓禄", "登录时间", "登录名", "等"]
        );

        // 连续 3 次明确改选「登陆」：跃升至第 1 位，「登录」紧随第 2 位，且有效先验权重超过「登录」。
        record_personal_selection(&journal, "deng'lu", "登陆", now + 2).unwrap();
        record_personal_selection(&journal, "deng'lu", "登陆", now + 3).unwrap();
        let mut after3 = make();
        let out3 = apply_personal_overlay(&journal, "deng'lu", None, &mut after3, now + 4);
        assert!(out3.moved && out3.leader_protected);
        assert_eq!(
            words(&after3),
            ["登陆", "登录", "邓禄", "登录时间", "登录名", "等"]
        );
        let pair_refs: Vec<&WordItem> = vec![&after3[0], &after3[1]];
        let weights = effective_bayesian_weights(&journal, "deng'lu", &pair_refs, now + 4)
            .expect("有个人选择记录时应返回有效贝叶斯权重");
        assert!(
            weights[0] > weights[1],
            "3 次选择后的「登陆」有效权重应高于「登录」: {weights:?}"
        );
    }
}
