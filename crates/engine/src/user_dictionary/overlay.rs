//! 独立个人学习叠加层（Stage 4 / Phase 3 抗劫持升级）：永不改写主词库 `msime-pinyin.db` 静态权重。
//!
//! 用户明确改选（非空格接受默认首选）写入 `msime_user.db` 的追加式事件表
//! `personal_selection_events`（携带粗粒度 `app_scope` 场景桶），查询时按 30 天半衰期做指数衰减计数 `n_user(w)`：
//! - **App 场景桶隔离**：同桶事件按 `1.0` 计权，跨桶事件按 `0.5` 计权，避免 `CodeIde` 学到的工程术语劫持 `ImChat` 高频日常词；
//! - **退格 24 小时衰减负反馈**：上屏后 5 秒内退格删光刚上屏词，既确定性回滚该次学习事件，又写入 `personal_negative_feedback` 标记（24 小时半衰期），重打时即时抑制误选词；
//! - **贝叶斯平滑与防劫持铁律**：单次误选不会击穿核心高频词（`n_user < 1.8` 不得竞争第 1 槽）；首次明确选择冷门专名稳定召回首屏第 2–3 槽；连续 2–3 次明确选择自然竞争第 1 槽，且 30 天半衰期自然回落；显式「置顶」（`pinned_candidates`）保持第 1 槽硬约束；
//! - **Neural v2 统一对数概率接口**：`personal_cache_log_bonus` 输出严格有界（`[-1.5, +1.5]` nats）的个人先验对数增益，供统一概率融合层直接叠加。

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, TransactionBehavior};

use super::journal::{existing_journal, open_journal, UNSTORABLE_ENTRY};
use super::ranking::candidate_dictionary_key;
use crate::error::{EngineError, Result};
use crate::types::WordItem;

/// 个人选择事件的指数衰减半衰期：30 天（秒）。
pub const HALF_LIFE_SECONDS: f64 = 30.0 * 86_400.0;
/// 退格删词负反馈的指数衰减半衰期：24 小时（秒）。
pub const NEGATIVE_FEEDBACK_HALF_LIFE_SECONDS: f64 = 24.0 * 3_600.0;
/// 超过 7 天（7 个半衰期，残余权重 < 0.008）的负反馈标记在写入时自动裁剪。
pub const NEGATIVE_FEEDBACK_RETENTION_SECONDS: i64 = 7 * 86_400;
/// 上屏后触发确定性退格回滚与负反馈的时间窗上限：5 秒。
pub const BACKSPACE_ROLLBACK_WINDOW_SECONDS: i64 = 5;
/// 超过 180 天（约 6 个半衰期，残余权重 < 0.016）的历史事件在写入时自动裁剪。
pub const RETENTION_SECONDS: i64 = 180 * 86_400;
/// 事件表最大保留条数，防止长期使用无限增长。
pub const MAX_PERSONAL_EVENTS: i64 = 50_000;
/// 贝叶斯平滑静态先验等效伪计数 `alpha`：
/// 跨同码全部静态词以榜首权重为锚（`alpha = 3.0`），单次明确选冷门词（`n_user = 1.0`）
/// 稳获首屏第 2–3 位召回而不击穿核心高频词；连续 3 次明确选择可自然竞争第 1 槽。
pub const BAYESIAN_PRIOR_ALPHA: f64 = 3.0;
/// 竞争第 1 槽所需的最低衰减明确选择次数（至少 2 次近期明确选择，彻底杜绝单次误选翻首位）。
pub const SLOT_ZERO_MIN_DECAYED_PICKS: f64 = 1.8;
/// 跨 App 场景桶的历史选词折算权重（同桶 `1.0`，跨桶 `0.5`）。
pub const CROSS_SCOPE_WEIGHT: f64 = 0.5;
/// 单条近期退格负反馈在有效选择计数与后验上的抑制强度（乘以 24 小时衰减系数）。
pub const NEGATIVE_FEEDBACK_WEIGHT: f64 = 1.5;
/// Neural v2 融合层个人对数概率增益/抑制的绝对值上限（单位：nat），防止个人偏置压倒强语境。
pub const MAX_PERSONAL_CACHE_LOG_BONUS: f64 = 1.5;
/// 单条近期退格负反馈在 Neural v2 对数域施加的抑制基准（单位：nat，乘以 24 小时衰减系数）。
const NEGATIVE_FEEDBACK_LOG_PENALTY: f64 = 1.2;

/// 粗粒度应用使用场景桶，用于隔离编程/聊天/写作/搜索之间的个人习惯。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppScope {
    #[default]
    General,
    ImChat,
    CodeIde,
    DocsWriting,
    BrowserSearch,
}

impl AppScope {
    /// 持久化到 SQLite `app_scope` 列的稳定标识符。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::ImChat => "im_chat",
            Self::CodeIde => "code_ide",
            Self::DocsWriting => "docs_writing",
            Self::BrowserSearch => "browser_search",
        }
    }

    /// 从持久化字符串解析场景桶；未知值安全回落为 `General`。
    pub fn from_stored(raw: &str) -> Self {
        match raw {
            "im_chat" => Self::ImChat,
            "code_ide" => Self::CodeIde,
            "docs_writing" => Self::DocsWriting,
            "browser_search" => Self::BrowserSearch,
            _ => Self::General,
        }
    }

    /// 按宿主的 bundle identifier（如 macOS `NSRunningApplication.bundleIdentifier`）映射粗粒度场景桶。
    pub fn from_bundle_id(bundle_id: &str) -> Self {
        let id = bundle_id.trim().to_ascii_lowercase();
        if id.is_empty() {
            return Self::General;
        }
        // 编程 IDE 与终端
        if id.contains("xcode")
            || id.contains("vscode")
            || id.contains("vscodium")
            || id.contains("cursor")
            || id == "com.todesktop.230313mzl4w4u92"
            || id.contains("windsurf")
            || id.contains("jetbrains")
            || id.contains("intellij")
            || id.contains("pycharm")
            || id.contains("clion")
            || id.contains("rustrover")
            || id.contains("goland")
            || id.contains("webstorm")
            || id.contains("dev.zed.zed")
            || id.contains("sublimetext")
            || id.contains("apple.terminal")
            || id.contains("iterm2")
            || id.contains("ghostty")
            || id.contains("dev.warp")
            || id.contains("alacritty")
            || id.contains("wezterm")
            || id.contains("kovidgoyal.kitty")
        {
            return Self::CodeIde;
        }
        // 即时通讯与聊天
        if id.contains("xinwechat")
            || id.contains("wechat")
            || id.contains("tencent.qq")
            || id.contains("slack")
            || id.contains("discord")
            || id.contains("mobilesms")
            || id.contains("apple.ichat")
            || id.contains("telegram")
            || id.contains("feishu")
            || id.contains("lark")
            || id.contains("dingtalk")
            || id.contains("whatsapp")
            || id.contains("naver.line")
        {
            return Self::ImChat;
        }
        // 文档与笔记写作
        if id.contains("notion")
            || id.contains("obsidian")
            || id.contains("iwork.pages")
            || id.contains("microsoft.word")
            || id.contains("apple.notes")
            || id.contains("shinyfrog.bear")
            || id.contains("typora")
            || id.contains("ulysses")
            || id.contains("wpsoffice")
            || id.contains("yuque")
        {
            return Self::DocsWriting;
        }
        // 浏览器与搜索
        if id.contains("apple.safari")
            || id.contains("google.chrome")
            || id.contains("thebrowser.browser")
            || id.contains("mozilla.firefox")
            || id.contains("microsoft.edgemac")
            || id.contains("brave.browser")
            || id.contains("vivaldi")
            || id.contains("operasoftware")
            || id.contains("kagi.kagimacos")
        {
            return Self::BrowserSearch;
        }
        Self::General
    }
}

/// 当前 Unix 秒时间戳。
pub fn unix_now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// 追加一条用户明确选词事件（默认 `AppScope::General`），返回自增事件 `id`（供紧接退格回滚使用）。
pub fn record_personal_selection(
    user_db: &Path,
    entry_key: &str,
    value: &str,
    now_unix: i64,
) -> Result<i64> {
    record_personal_selection_scoped(user_db, entry_key, value, AppScope::General, now_unix)
}

/// 追加一条带 `AppScope` 场景桶的用户明确选词事件，同时清除该桶下更早的同名负反馈标记。
pub fn record_personal_selection_scoped(
    user_db: &Path,
    entry_key: &str,
    value: &str,
    app_scope: AppScope,
    now_unix: i64,
) -> Result<i64> {
    if entry_key.is_empty() || value.is_empty() {
        return Err(EngineError::invalid(UNSTORABLE_ENTRY));
    }
    let scope_str = app_scope.as_str();
    let journal = open_journal(user_db)?;
    let tx = rusqlite::Transaction::new_unchecked(&journal, TransactionBehavior::Immediate)?;
    tx.prepare_cached(
        "INSERT INTO personal_selection_events(entry_key,value,selected_at,app_scope) VALUES(?1,?2,?3,?4)",
    )?
    .execute(params![entry_key, value, now_unix, scope_str])?;
    let event_id = tx.last_insert_rowid();
    // 用户再次明确选择该词时，清除该场景桶下早先残留的退格负反馈标记。
    tx.prepare_cached(
        "DELETE FROM personal_negative_feedback WHERE entry_key=?1 AND value=?2 AND app_scope=?3 AND rejected_at<=?4",
    )?
    .execute(params![entry_key, value, scope_str, now_unix])?;
    // 定期裁剪超过 180 天或超出上限的陈旧事件（仅在每 64 次写入时触发，避免热路径额外扫描）。
    if event_id % 64 == 0 {
        let cutoff = now_unix.saturating_sub(RETENTION_SECONDS);
        tx.prepare_cached("DELETE FROM personal_selection_events WHERE selected_at < ?1")?
            .execute(params![cutoff])?;
        tx.prepare_cached(
            "DELETE FROM personal_selection_events WHERE id <= (SELECT id FROM personal_selection_events ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        )?
        .execute(params![MAX_PERSONAL_EVENTS])?;
        let neg_cutoff = now_unix.saturating_sub(NEGATIVE_FEEDBACK_RETENTION_SECONDS);
        tx.prepare_cached("DELETE FROM personal_negative_feedback WHERE rejected_at < ?1")?
            .execute(params![neg_cutoff])?;
    }
    tx.commit()?;
    Ok(event_id)
}

/// 当用户在 5 秒窗口内退格删光刚上屏词时，确定性撤销对应选词事件（若有）并写入 24 小时半衰期负反馈标记。
/// 超出 5 秒窗口或键值非法时不做任何写入并返回 `Ok(false)`。
pub fn record_backspace_rollback_feedback(
    user_db: &Path,
    event_id: Option<i64>,
    entry_key: &str,
    value: &str,
    app_scope: AppScope,
    committed_at_unix: i64,
    now_unix: i64,
) -> Result<bool> {
    if entry_key.is_empty() || value.is_empty() {
        return Ok(false);
    }
    let elapsed = now_unix.saturating_sub(committed_at_unix);
    if !(0..=BACKSPACE_ROLLBACK_WINDOW_SECONDS).contains(&elapsed) {
        return Ok(false);
    }
    let journal = open_journal(user_db)?;
    let tx = rusqlite::Transaction::new_unchecked(&journal, TransactionBehavior::Immediate)?;
    if let Some(id) = event_id {
        tx.prepare_cached("DELETE FROM personal_selection_events WHERE id=?1")?
            .execute(params![id])?;
    }
    tx.prepare_cached(
        "INSERT INTO personal_negative_feedback(entry_key,value,app_scope,rejected_at) VALUES(?1,?2,?3,?4) \
         ON CONFLICT(entry_key,value,app_scope) DO UPDATE SET rejected_at=excluded.rejected_at",
    )?
    .execute(params![entry_key, value, app_scope.as_str(), now_unix])?;
    let neg_cutoff = now_unix.saturating_sub(NEGATIVE_FEEDBACK_RETENTION_SECONDS);
    tx.prepare_cached("DELETE FROM personal_negative_feedback WHERE rejected_at < ?1")?
        .execute(params![neg_cutoff])?;
    tx.commit()?;
    Ok(true)
}

/// 当某个多音节词键在 `personal_selection_events` 中已有竞争词记录时，记录一次用户以空格接受
/// 当前首选的确认事件，消除「只记改选、不记空格接受默认首位」的选择偏差（默认 `AppScope::General`）。
pub fn record_contested_leader_confirmation(
    user_db: &Path,
    entry_key: &str,
    leader_word: &str,
    now_unix: i64,
) -> Result<bool> {
    record_contested_leader_confirmation_scoped(
        user_db,
        entry_key,
        leader_word,
        AppScope::General,
        now_unix,
    )
}

/// 带 `AppScope` 场景桶的竞争首选空格确认写入。
pub fn record_contested_leader_confirmation_scoped(
    user_db: &Path,
    entry_key: &str,
    leader_word: &str,
    app_scope: AppScope,
    now_unix: i64,
) -> Result<bool> {
    if entry_key.is_empty() || leader_word.is_empty() || !entry_key.contains('\'') {
        return Ok(false);
    }
    let counts = decayed_counts_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    if counts.is_empty() {
        return Ok(false);
    }
    let max_rival = counts
        .iter()
        .filter(|(word, _)| word.as_str() != leader_word)
        .map(|(_, &count)| count)
        .fold(0.0f64, f64::max);
    if max_rival <= 0.0 {
        return Ok(false);
    }
    let leader_count = counts.get(leader_word).copied().unwrap_or(0.0);
    if leader_count >= max_rival + 2.0 {
        return Ok(false);
    }
    record_personal_selection_scoped(user_db, entry_key, leader_word, app_scope, now_unix)?;
    Ok(true)
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

/// 计算某个完整拼音键在指定场景桶下各词的 24 小时衰减退格负反馈强度（同桶 `1.0`、跨桶 `0.5`）。
pub fn decayed_negative_feedback_for_key_scoped(
    user_db: &Path,
    entry_key: &str,
    app_scope: AppScope,
    now_unix: i64,
) -> HashMap<String, f64> {
    let mut penalties = HashMap::new();
    if entry_key.is_empty() {
        return penalties;
    }
    let Some(journal) = existing_journal(user_db) else {
        return penalties;
    };
    let Ok(mut statement) = journal.prepare_cached(
        "SELECT value, app_scope, rejected_at FROM personal_negative_feedback WHERE entry_key=?1",
    ) else {
        return penalties;
    };
    let Ok(rows) = statement.query_map(params![entry_key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    }) else {
        return penalties;
    };
    for row in rows.flatten() {
        let (value, stored_scope, rejected_at) = row;
        let age = now_unix.saturating_sub(rejected_at).max(0) as f64;
        let scope_factor = if AppScope::from_stored(&stored_scope) == app_scope {
            1.0
        } else {
            CROSS_SCOPE_WEIGHT
        };
        let strength = scope_factor * 0.5f64.powf(age / NEGATIVE_FEEDBACK_HALF_LIFE_SECONDS);
        if strength >= 1e-4 {
            let entry = penalties.entry(value).or_insert(0.0);
            if strength > *entry {
                *entry = strength;
            }
        }
    }
    penalties
}

/// 计算某个完整拼音键下各词经 30 天半衰期指数衰减后的有效明确选择次数（默认 `AppScope::General`）；无日志时不创建文件。
pub fn decayed_counts_for_key(
    user_db: &Path,
    entry_key: &str,
    now_unix: i64,
) -> HashMap<String, f64> {
    decayed_counts_for_key_scoped(user_db, entry_key, AppScope::General, now_unix)
}

/// 计算某个完整拼音键在 `app_scope` 下的有效明确选择次数：同桶按 `1.0` 计权、跨桶按 `0.5` 计权，
/// 并扣减 24 小时衰减退格负反馈（结果截断至非负且仅保留 `> 0.0` 项）。
pub fn decayed_counts_for_key_scoped(
    user_db: &Path,
    entry_key: &str,
    app_scope: AppScope,
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
        "SELECT value, selected_at, app_scope FROM personal_selection_events WHERE entry_key=?1",
    ) else {
        return counts;
    };
    let Ok(rows) = statement.query_map(params![entry_key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else {
        return counts;
    };
    for row in rows.flatten() {
        let (value, selected_at, stored_scope) = row;
        let age = now_unix.saturating_sub(selected_at).max(0) as f64;
        let scope_factor = if AppScope::from_stored(&stored_scope) == app_scope {
            1.0
        } else {
            CROSS_SCOPE_WEIGHT
        };
        let weight = scope_factor * 0.5f64.powf(age / HALF_LIFE_SECONDS);
        if weight >= 1e-4 {
            *counts.entry(value).or_insert(0.0) += weight;
        }
    }
    if !counts.is_empty() {
        let negative =
            decayed_negative_feedback_for_key_scoped(user_db, entry_key, app_scope, now_unix);
        if !negative.is_empty() {
            counts.retain(|word, count| {
                if let Some(&neg) = negative.get(word) {
                    *count = (*count - NEGATIVE_FEEDBACK_WEIGHT * neg).max(0.0);
                }
                *count > 1e-4
            });
        }
    }
    counts
}

/// 为 Neural v2 统一融合阶段提供个人贝叶斯缓存对数概率增益/抑制（单位：nat，严格限制在 `[-1.5, +1.5]`）。
///
/// - 无个人事件且无退格负反馈时返回 `0.0`；
/// - 正向增益取平滑对数插值 `ln(1 + n_user / alpha)`（单次同桶选择仅 `+0.29 nat`，跨桶仅 `+0.15 nat`，绝不压倒强语境或核心高频词）；
/// - 5 秒内退格删词写入的 24 小时衰减负反馈施加 `-1.2 * neg_decay` 抑制，重打时即时避让刚被删除的错误词。
pub fn personal_cache_log_bonus(
    user_db: &Path,
    entry_key: &str,
    word: &str,
    app_scope: AppScope,
    now_unix: i64,
) -> f64 {
    if entry_key.is_empty() || word.is_empty() {
        return 0.0;
    }
    let counts = decayed_counts_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    let negative =
        decayed_negative_feedback_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    let pos = counts.get(word).copied().unwrap_or(0.0);
    let neg = negative.get(word).copied().unwrap_or(0.0);
    if pos <= 0.0 && neg <= 0.0 {
        return 0.0;
    }
    let lift = if pos > 0.0 {
        (1.0 + pos / BAYESIAN_PRIOR_ALPHA)
            .ln()
            .min(MAX_PERSONAL_CACHE_LOG_BONUS)
    } else {
        0.0
    };
    let suppress = (NEGATIVE_FEEDBACK_LOG_PENALTY * neg).min(MAX_PERSONAL_CACHE_LOG_BONUS);
    (lift - suppress).clamp(-MAX_PERSONAL_CACHE_LOG_BONUS, MAX_PERSONAL_CACHE_LOG_BONUS)
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

/// 为语境同音竞争计算融合了 Stage 4 衰减个人选词事件的等效静态先验权重（默认 `AppScope::General`）。
pub fn effective_bayesian_weights(
    user_db: &Path,
    entry_key: &str,
    items: &[&WordItem],
    now_unix: i64,
) -> Option<Vec<f64>> {
    effective_bayesian_weights_scoped(user_db, entry_key, items, AppScope::General, now_unix)
}

/// 带 `AppScope` 场景桶与 24 小时退格负反馈的贝叶斯等效先验权重。
pub fn effective_bayesian_weights_scoped(
    user_db: &Path,
    entry_key: &str,
    items: &[&WordItem],
    app_scope: AppScope,
    now_unix: i64,
) -> Option<Vec<f64>> {
    if entry_key.is_empty() || !entry_key.contains('\'') || items.len() < 2 {
        return None;
    }
    let counts = decayed_counts_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    let negative =
        decayed_negative_feedback_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    if counts.is_empty() && negative.is_empty() {
        return None;
    }
    let any_matched = items.iter().any(|item| {
        counts.get(item.word.as_str()).is_some_and(|&n| n > 0.0)
            || negative.get(item.word.as_str()).is_some_and(|&n| n > 0.0)
    });
    if !any_matched {
        return None;
    }
    let leader_weight = items
        .iter()
        .map(|item| item.weight.max(1) as f64)
        .fold(1.0f64, f64::max);
    let user_unit_weight = leader_weight / BAYESIAN_PRIOR_ALPHA;
    Some(
        items
            .iter()
            .map(|item| {
                let base = item.weight.max(1) as f64;
                let n_user = counts.get(item.word.as_str()).copied().unwrap_or(0.0);
                let neg = negative.get(item.word.as_str()).copied().unwrap_or(0.0);
                let adjusted = base + user_unit_weight * n_user;
                let damped = if neg > 0.0 {
                    adjusted * (-NEGATIVE_FEEDBACK_LOG_PENALTY * neg).exp()
                } else {
                    adjusted
                };
                damped.max(1.0)
            })
            .collect(),
    )
}

/// 用独立贝叶斯衰减叠加层重排同音词典候选（默认 `AppScope::General`）；绝不修改任何 `WordItem::weight`。
pub fn apply_personal_overlay(
    user_db: &Path,
    entry_key: &str,
    pinned_leader: Option<&str>,
    candidates: &mut [WordItem],
    now_unix: i64,
) -> OverlayOutcome {
    apply_personal_overlay_scoped(
        user_db,
        entry_key,
        pinned_leader,
        AppScope::General,
        candidates,
        now_unix,
    )
}

/// 带 `AppScope` 场景桶隔离与 24 小时退格负反馈抑制的独立个人叠加层重排；绝不修改任何 `WordItem::weight`。
pub fn apply_personal_overlay_scoped(
    user_db: &Path,
    entry_key: &str,
    pinned_leader: Option<&str>,
    app_scope: AppScope,
    candidates: &mut [WordItem],
    now_unix: i64,
) -> OverlayOutcome {
    if entry_key.is_empty()
        || candidates.len() < 2
        || !is_exact_homophone(&candidates[0], entry_key)
    {
        return OverlayOutcome::default();
    }
    let counts = decayed_counts_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    let negative =
        decayed_negative_feedback_for_key_scoped(user_db, entry_key, app_scope, now_unix);
    if counts.is_empty() && negative.is_empty() && pinned_leader.is_none() {
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
    if counts.is_empty() && negative.is_empty() {
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
    let is_single_char_key =
        !entry_key.contains('\'') && candidates[exact_indices[0]].word.chars().count() == 1;
    let leader_weight = exact_indices
        .iter()
        .map(|&raw_idx| candidates[raw_idx].weight.max(1) as f64)
        .fold(1.0f64, f64::max);
    let mut posteriors = Vec::with_capacity(group_len);
    let mut user_counts = Vec::with_capacity(group_len);
    let mut neg_strengths = Vec::with_capacity(group_len);
    for &raw_idx in &exact_indices {
        let item = &candidates[raw_idx];
        let p_rel = item.weight.max(1) as f64 / leader_weight;
        let n_user = counts.get(item.word.as_str()).copied().unwrap_or(0.0);
        let neg = negative.get(item.word.as_str()).copied().unwrap_or(0.0);
        // 退格负反馈采用“先抵消用户正向增量、余量对静态先验做有界乘性阻尼”的贝叶斯更新：
        // - 误选冷门词后退格，正向计数归零立刻回落后排；
        // - 误退格常用高频字/词（如 `hua -> 化/画/华`）时后验严格保持正值，仅温和下调有限顺位，
        //   绝不跌为负值而被甩到上百个零权生僻字之后，也绝不会被踢出 Tier 1 神经窗口。
        let eff_user = (n_user - NEGATIVE_FEEDBACK_WEIGHT * neg).max(0.0);
        let residual_neg = (neg - n_user / NEGATIVE_FEEDBACK_WEIGHT).clamp(0.0, 2.0);
        let damped_prior = BAYESIAN_PRIOR_ALPHA * p_rel * 0.5_f64.powf(residual_neg);
        posteriors.push(damped_prior + eff_user);
        user_counts.push(n_user);
        neg_strengths.push(neg);
    }

    let mut leader_pos = pinned_pos.unwrap_or(0);
    let mut overlay_promoted_leader = false;
    if pinned_pos.is_none() && !is_single_char_key {
        let mut best_score = posteriors[0];
        let leader_suppressed = neg_strengths[0] >= 0.5;
        // 仅当静态槽 0 只是低频冷门残留词（`weight < 500`）且用户从未选过时，
        // 才允许首轮自造/首 pick（`n_user >= 0.9`）直接升格至槽 0；高频核心词仍严格遵守 `1.8` 次防劫持阈值。
        let weak_static_leader = leader_weight < 500.0 && user_counts[0] < 0.9;
        let mut weak_winner_found = false;
        for pos in 1..group_len {
            let weak_picked =
                weak_static_leader && user_counts[pos] >= 0.9 && neg_strengths[pos] < 0.25;
            let eligible = user_counts[pos] >= SLOT_ZERO_MIN_DECAYED_PICKS
                || weak_picked
                || (leader_suppressed && neg_strengths[pos] < 0.25);
            if !eligible {
                continue;
            }
            let beats_leader = if weak_winner_found
                && !weak_picked
                && user_counts[pos] < SLOT_ZERO_MIN_DECAYED_PICKS
            {
                false
            } else if weak_picked && !weak_winner_found {
                true
            } else {
                posteriors[pos] > best_score
            };
            if beats_leader {
                best_score = posteriors[pos];
                leader_pos = pos;
                weak_winner_found |= weak_picked;
                overlay_promoted_leader =
                    user_counts[pos] >= SLOT_ZERO_MIN_DECAYED_PICKS || weak_picked;
            }
        }
    }
    let leader_protected = pinned_pos.is_some() || overlay_promoted_leader;

    let mut exact_order: Vec<usize> = Vec::with_capacity(group_len);
    exact_order.push(leader_pos);
    let mut rest: Vec<usize> = (0..group_len).filter(|&pos| pos != leader_pos).collect();
    // 余下同音词按贝叶斯后验降序稳定排布：首次明确选择的冷门专名（`n_user ≈ 1.0`）自动进入首屏前排，
    // 近期被退格删除的词受 24 小时衰减负反馈压低至后排。
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
    fn app_scope_maps_bundle_ids_accurately_and_round_trips_storage() {
        for (bundle, expected) in [
            ("com.apple.dt.Xcode", AppScope::CodeIde),
            ("com.microsoft.VSCode", AppScope::CodeIde),
            ("com.todesktop.230313mzl4w4u92", AppScope::CodeIde),
            ("com.jetbrains.rustrover", AppScope::CodeIde),
            ("com.apple.Terminal", AppScope::CodeIde),
            ("com.mitchellh.ghostty", AppScope::CodeIde),
            ("com.tencent.xinWeChat", AppScope::ImChat),
            ("com.tinyspeck.slackmacgap", AppScope::ImChat),
            ("com.hnc.Discord", AppScope::ImChat),
            ("com.apple.MobileSMS", AppScope::ImChat),
            ("ru.keepcoder.Telegram", AppScope::ImChat),
            ("com.electron.lark", AppScope::ImChat),
            ("com.bytedance.feishu", AppScope::ImChat),
            ("notion.id", AppScope::DocsWriting),
            ("md.obsidian", AppScope::DocsWriting),
            ("com.apple.iWork.Pages", AppScope::DocsWriting),
            ("com.microsoft.Word", AppScope::DocsWriting),
            ("com.apple.Safari", AppScope::BrowserSearch),
            ("com.google.Chrome", AppScope::BrowserSearch),
            ("company.thebrowser.Browser", AppScope::BrowserSearch),
            ("org.mozilla.firefox", AppScope::BrowserSearch),
            ("com.apple.finder", AppScope::General),
            ("", AppScope::General),
        ] {
            assert_eq!(
                AppScope::from_bundle_id(bundle),
                expected,
                "bundle={bundle}"
            );
            assert_eq!(AppScope::from_stored(expected.as_str()), expected);
        }
    }

    #[test]
    fn single_accidental_pick_never_breaks_core_word_and_recalls_rare_proper_noun_to_page_one() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_800_000_000;

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

        let event_id = record_personal_selection(&journal, "yi'ge", "义戈", now).unwrap();
        let outcome = apply_personal_overlay(&journal, "yi'ge", None, &mut list, now);
        assert!(outcome.moved && !outcome.leader_protected);
        assert_eq!(list[0].word, "一个");
        assert_eq!(list[1].word, "义戈");
        let mut weights_after: Vec<i64> = list.iter().map(|row| row.weight).collect();
        weights_after.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(weights_after, original_weights);

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

        record_personal_selection(&journal, "yi'ge", "义戈", now).unwrap();
        record_personal_selection(&journal, "yi'ge", "义戈", now + 10).unwrap();
        let mut list = make();
        let out2 = apply_personal_overlay(&journal, "yi'ge", None, &mut list, now + 20);
        assert!(out2.moved && !out2.leader_protected);
        assert_eq!(words(&list), ["一个", "义戈", "一格"]);

        record_personal_selection(&journal, "yi'ge", "义戈", now + 30).unwrap();
        let mut list = make();
        let out3 = apply_personal_overlay(&journal, "yi'ge", None, &mut list, now + 40);
        assert!(out3.moved && out3.leader_protected);
        assert_eq!(words(&list), ["义戈", "一个", "一格"]);

        let mut pinned = make();
        let out_pin =
            apply_personal_overlay(&journal, "yi'ge", Some("一个"), &mut pinned, now + 40);
        assert!(out_pin.moved && out_pin.leader_protected);
        assert_eq!(words(&pinned), ["一个", "义戈", "一格"]);

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

        let mut untouched = make();
        assert!(!apply_personal_overlay(&journal, "deng'lu", None, &mut untouched, now).moved);
        let refs: Vec<&WordItem> = vec![&untouched[0], &untouched[4]];
        assert!(effective_bayesian_weights(&journal, "deng'lu", &refs, now).is_none());

        let mut pinned_only = make();
        let out_pin =
            apply_personal_overlay(&journal, "deng'lu", Some("登陆"), &mut pinned_only, now);
        assert!(out_pin.moved && out_pin.leader_protected);
        assert_eq!(
            words(&pinned_only),
            ["登陆", "登录", "邓禄", "登录时间", "登录名", "等"]
        );

        record_personal_selection(&journal, "deng'lu", "登陆", now).unwrap();
        let mut after1 = make();
        let out1 = apply_personal_overlay(&journal, "deng'lu", None, &mut after1, now + 1);
        assert!(out1.moved && !out1.leader_protected);
        assert_eq!(
            words(&after1),
            ["登录", "登陆", "邓禄", "登录时间", "登录名", "等"]
        );

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
        assert!(weights[0] > weights[1]);
    }

    #[test]
    fn single_char_overlay_recalls_without_hijacking_slot_zero_and_respects_pin() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_800_000_000;

        let make = || {
            vec![
                item("li", "里", 7_329_328),
                item("li", "力", 3_298_612),
                item("li", "理", 1_903_915),
                item("li", "李", 1_462_547),
                item("li", "离", 1_378_185),
                item("li", "立", 1_293_824),
            ]
        };

        for i in 0..5 {
            record_personal_selection(&journal, "li", "李", now + i).unwrap();
        }
        let mut after = make();
        let out = apply_personal_overlay(&journal, "li", None, &mut after, now + 10);
        assert!(out.moved && !out.leader_protected);
        assert_eq!(words(&after), ["里", "李", "力", "理", "离", "立"]);

        let refs: Vec<&WordItem> = after.iter().collect();
        assert!(effective_bayesian_weights(&journal, "li", &refs, now + 10).is_none());

        let mut pinned = make();
        let out_pin = apply_personal_overlay(&journal, "li", Some("李"), &mut pinned, now + 10);
        assert!(out_pin.moved && out_pin.leader_protected);
        assert_eq!(words(&pinned)[0], "李");
    }

    #[test]
    fn multi_char_prior_anchors_to_leader_and_contested_confirmation_bounds_writes() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_800_000_000;

        let make = || {
            vec![
                item("chu'fa", "出发", 593_690),
                item("chu'fa", "处罚", 587_462),
                item("chu'fa", "触发", 550_097),
                item("chu'fa", "楚法", 20_000),
                item("chu'fa", "除法", 10_000),
            ]
        };

        assert!(!record_contested_leader_confirmation(&journal, "chu'fa", "出发", now).unwrap());
        assert!(!journal.exists());

        record_personal_selection(&journal, "chu'fa", "除法", now).unwrap();
        record_personal_selection(&journal, "chu'fa", "除法", now + 1).unwrap();
        let mut after2 = make();
        let out2 = apply_personal_overlay(&journal, "chu'fa", None, &mut after2, now + 2);
        assert!(out2.moved && !out2.leader_protected);
        assert_eq!(words(&after2), ["出发", "处罚", "触发", "除法", "楚法"]);

        let mut written = 0;
        for step in 0..8 {
            if record_contested_leader_confirmation(&journal, "chu'fa", "出发", now + 10 + step)
                .unwrap()
            {
                written += 1;
            }
        }
        assert_eq!(written, 4);

        record_personal_selection(&journal, "chu'fa", "除法", now + 20).unwrap();
        let mut defended = make();
        let out_def = apply_personal_overlay(&journal, "chu'fa", None, &mut defended, now + 21);
        assert!(!out_def.leader_protected);
        assert_eq!(defended[0].word, "出发");
    }

    /// 架构方案 §4.3：50 组确定性合成防劫持回放测试套件，完整验证全部 5 条个人学习防劫持铁律：
    /// - 铁律 1（10 组）：误选 1 次生僻词对高频核心词误劫持率 0%；
    /// - 铁律 2（10 组）：首次明确选冷门专名/人名 100% 稳定进入首屏槽 2–3（下标 1–2）；
    /// - 铁律 3（10 组）：连续 3 次明确选自然竞争第 1 槽且 30 天半衰期后自然回落；
    /// - 铁律 4（10 组）：5 秒内退格删全词 100% 撤销学习并施加 24 小时衰减负反馈抑制；
    /// - 铁律 5（10 组）：`CodeIde` 等桶学到的术语跨桶仅 50% 权重，对 `ImChat` 高频核心词误劫持率 0%。
    #[test]
    fn anti_hijacking_50_scenario_deterministic_replay_suite() {
        let now = 1_800_000_000i64;
        let mut executed = 0usize;

        // ---- 铁律 1（10 组）：单次误选生僻词对高频核心词误劫持率 = 0% ----
        let core_vs_obscure: [(&str, &str, i64, &str, i64); 10] = [
            ("yi'ge", "一个", 509_000, "义戈", 40),
            ("wo'men", "我们", 509_000, "沃门", 35),
            ("xian'zai", "现在", 503_000, "衔载", 25),
            ("shi'jian", "时间", 505_000, "矢箭", 80),
            ("wen'ti", "问题", 505_000, "文蹄", 20),
            ("yin'wei", "因为", 504_000, "音纬", 30),
            ("ke'yi", "可以", 506_000, "刻艺", 45),
            ("mei'you", "没有", 508_000, "梅柚", 50),
            ("zhi'dao", "知道", 504_000, "纸刀", 60),
            ("gong'zuo", "工作", 502_000, "弓座", 30),
        ];
        for (key, core, core_w, obscure, obs_w) in core_vs_obscure {
            let dir = Dir::new();
            let journal = dir.journal();
            let mut list = vec![
                item(key, core, core_w),
                item(key, "次高频", core_w / 10),
                item(key, "中频词", core_w / 50),
                item(key, obscure, obs_w),
            ];
            let orig_weights: Vec<i64> = list.iter().map(|r| r.weight).collect();
            record_personal_selection_scoped(&journal, key, obscure, AppScope::ImChat, now)
                .unwrap();
            let out = apply_personal_overlay_scoped(
                &journal,
                key,
                None,
                AppScope::ImChat,
                &mut list,
                now + 1,
            );
            assert_eq!(
                list[0].word, core,
                "铁律 1 违背：{key} 单次误选劫持了核心词"
            );
            assert!(!out.leader_protected);
            // 静态权重完全零污染，且对数增益严格有界且温和（单次约 +0.29 nat）。
            let bonus = personal_cache_log_bonus(&journal, key, obscure, AppScope::ImChat, now + 1);
            assert!(
                bonus > 0.0 && bonus < 0.35,
                "单次误选对数增益应温和受控：{bonus}"
            );
            let mut after_w: Vec<i64> = list.iter().map(|r| r.weight).collect();
            after_w.sort_unstable_by(|a, b| b.cmp(a));
            assert_eq!(after_w, orig_weights);
            executed += 1;
        }

        // ---- 铁律 2（10 组）：首次明确选埋在第 7–9 位的冷门专名/人名 100% 进入首屏槽 2–3 ----
        let rare_nouns: [(&str, &str, &str, bool); 10] = [
            ("lin'xi", "林溪", "临西", false),
            ("su'ming", "苏明", "宿命", true),
            ("gu'cheng", "顾城", "古城", false),
            ("shen'he", "沈河", "审核", true),
            ("lu'yuan", "陆远", "路缘", false),
            ("ji'ning", "济宁", "机能", false),
            ("qing'he", "清河", "轻荷", true),
            ("ning'bo", "宁波", "凝波", false),
            ("cheng'ze", "程泽", "承泽", false),
            ("yun'qi", "云栖", "运气", true),
        ];
        for (idx, (key, rare, common, with_interleaved)) in rare_nouns.into_iter().enumerate() {
            let dir = Dir::new();
            let journal = dir.journal();
            let mut list = vec![item(key, common, 500_000)];
            if with_interleaved {
                let mut gen = item(key, "拼凑句", -9_000);
                gen.source = crate::types::CandidateSource::Generated;
                list.push(gen);
                list.push(item(&format!("{key}'de"), "前缀补全", 300_000));
            }
            for k in 0..6 {
                list.push(item(
                    key,
                    &format!("普通同音{idx}_{k}"),
                    40_000 - (k as i64) * 5_000,
                ));
            }
            list.push(item(key, rare, 80));

            record_personal_selection_scoped(&journal, key, rare, AppScope::DocsWriting, now)
                .unwrap();
            let out = apply_personal_overlay_scoped(
                &journal,
                key,
                None,
                AppScope::DocsWriting,
                &mut list,
                now + 2,
            );
            assert!(out.moved && !out.leader_protected);
            assert_eq!(list[0].word, common, "铁律 2：首次选冷门词不得抢占第 0 槽");
            let rare_seat = list.iter().position(|r| r.word == rare).unwrap();
            assert!(
                (1..=2).contains(&rare_seat),
                "铁律 2：{key} 首次选冷门专名应进入槽 2–3（下标 1–2），实际下标 {rare_seat}"
            );
            executed += 1;
        }

        // ---- 铁律 3（10 组）：连续 3 次明确选择自然胜出第 1 槽，30 天半衰期后回落 ----
        let repeated_cases: [(&str, &str, i64, &str, i64); 10] = [
            ("deng'lu", "登录", 502_000, "登陆", 5_000),
            ("zuo'ye", "作业", 480_000, "昨夜", 15_000),
            ("ji'lu", "记录", 495_000, "纪录", 20_000),
            ("fu'zhi", "复制", 450_000, "副职", 3_000),
            ("jie'kou", "接口", 420_000, "借口", 40_000),
            ("biao'da", "表达", 460_000, "表大", 500),
            ("fen'xi", "分析", 490_000, "分系", 800),
            ("xiang'mu", "项目", 500_000, "香木", 400),
            ("pei'zhi", "配置", 440_000, "配制", 6_000),
            ("mo'xing", "模型", 470_000, "魔性", 12_000),
        ];
        for (key, core, core_w, target, target_w) in repeated_cases {
            let dir = Dir::new();
            let journal = dir.journal();
            let make = || {
                vec![
                    item(key, core, core_w),
                    item(key, "陪跑词", core_w / 20),
                    item(key, target, target_w),
                ]
            };
            for step in 0..3 {
                record_personal_selection_scoped(
                    &journal,
                    key,
                    target,
                    AppScope::General,
                    now + step * 10,
                )
                .unwrap();
            }
            let mut promoted = make();
            let out3 = apply_personal_overlay_scoped(
                &journal,
                key,
                None,
                AppScope::General,
                &mut promoted,
                now + 30,
            );
            assert!(out3.moved && out3.leader_protected);
            assert_eq!(
                promoted[0].word, target,
                "铁律 3：{key} 连续 3 次选应竞争首位"
            );
            // 显式置顶核心词作为 L4 硬约束优先级高于个人叠加层。
            let mut pinned = make();
            let out_pin = apply_personal_overlay_scoped(
                &journal,
                key,
                Some(core),
                AppScope::General,
                &mut pinned,
                now + 30,
            );
            assert!(out_pin.leader_protected);
            assert_eq!(pinned[0].word, core);
            // 30 天后衰减至 1.5 (< 1.8)，核心词自然恢复第 0 槽，目标词保留首屏次席。
            let mut decayed = make();
            let out_decay = apply_personal_overlay_scoped(
                &journal,
                key,
                None,
                AppScope::General,
                &mut decayed,
                now + 30 * 86_400 + 30,
            );
            assert!(!out_decay.leader_protected);
            assert_eq!(
                decayed[0].word, core,
                "铁律 3：{key} 30 天后核心词应自然回位"
            );
            assert_eq!(decayed[1].word, target);
            executed += 1;
        }

        // ---- 铁律 4（10 组）：5 秒内退格删全词 100% 撤销学习 + 24 小时衰减负反馈抑制 ----
        let rollback_cases: [(&str, &str, &str, i64); 10] = [
            ("yi'ge", "一个", "义戈", 1),
            ("deng'lu", "登录", "登陆", 2),
            ("ji'lu", "记录", "纪录", 3),
            ("chu'fa", "出发", "除法", 4),
            ("zhi'dao", "知道", "指导", 5),
            ("mei'you", "没有", "煤油", 0),
            ("shi'jian", "时间", "事件", 2),
            ("wen'ti", "问题", "文体", 3),
            ("ke'yi", "可以", "刻意", 1),
            ("gong'zuo", "工作", "公座", 4),
        ];
        for (idx, (key, core, wrong, delay_secs)) in rollback_cases.into_iter().enumerate() {
            let dir = Dir::new();
            let journal = dir.journal();
            // 场景偶数：先选 3 次已占首位，第 4 次误选后 5 秒内退格删全词；
            // 场景奇数：单次误选后 5 秒内退格删全词。
            if idx % 2 == 0 {
                for step in 0..3 {
                    record_personal_selection_scoped(
                        &journal,
                        key,
                        wrong,
                        AppScope::General,
                        now - 100 + step,
                    )
                    .unwrap();
                }
            }
            let commit_at = now;
            let event_id = record_personal_selection_scoped(
                &journal,
                key,
                wrong,
                AppScope::General,
                commit_at,
            )
            .unwrap();
            // 超出 5 秒窗口不得触发回滚。
            assert!(!record_backspace_rollback_feedback(
                &journal,
                Some(event_id),
                key,
                wrong,
                AppScope::General,
                commit_at,
                commit_at + BACKSPACE_ROLLBACK_WINDOW_SECONDS + 1,
            )
            .unwrap());
            // 在 5 秒窗口内退格删光：撤销本次学习事件并写入 24h 负反馈标记。
            assert!(record_backspace_rollback_feedback(
                &journal,
                Some(event_id),
                key,
                wrong,
                AppScope::General,
                commit_at,
                commit_at + delay_secs,
            )
            .unwrap());

            let mut list = vec![
                item(key, core, 500_000),
                item(key, "正常备选", 80_000),
                item(key, wrong, 2_000),
            ];
            let check_at = commit_at + delay_secs + 1;
            let out = apply_personal_overlay_scoped(
                &journal,
                key,
                None,
                AppScope::General,
                &mut list,
                check_at,
            );
            assert_eq!(
                list[0].word, core,
                "铁律 4：{key} 退格回滚后核心词必须稳守首位"
            );
            assert!(!out.leader_protected);
            if idx % 2 == 1 {
                // 单次误选撤销后正计数归零，负反馈压低对数增益为负，重打时被抑制在末尾。
                let counts =
                    decayed_counts_for_key_scoped(&journal, key, AppScope::General, check_at);
                assert!(!counts.contains_key(wrong));
                assert_eq!(list[2].word, wrong);
                let bonus_now =
                    personal_cache_log_bonus(&journal, key, wrong, AppScope::General, check_at);
                let bonus_24h = personal_cache_log_bonus(
                    &journal,
                    key,
                    wrong,
                    AppScope::General,
                    check_at + 24 * 3_600,
                );
                assert!(bonus_now < -1.0, "刚回滚后应有明显负反馈抑制：{bonus_now}");
                assert!(
                    (bonus_24h - bonus_now * 0.5).abs() < 0.05,
                    "24 小时半衰期后负反馈应衰减一半：now={bonus_now}, 24h={bonus_24h}"
                );
            } else {
                // 原先 3 次已占首位，退格后扣除 1.5 -> 残余 1.5 (< 1.8)，立即让出首位。
                let counts =
                    decayed_counts_for_key_scoped(&journal, key, AppScope::General, check_at);
                let remain = counts.get(wrong).copied().unwrap_or(0.0);
                assert!(
                    remain < SLOT_ZERO_MIN_DECAYED_PICKS,
                    "铁律 4：退格负反馈应将原首位拉回门槛之下，remain={remain}"
                );
            }
            executed += 1;
        }

        // ---- 铁律 5（10 组）：跨 AppScope 隔离（CodeIde 术语不劫持 ImChat 高频词） ----
        let cross_scope_cases: [(&str, &str, &str, &str, &str); 10] = [
            (
                "com.apple.dt.Xcode",
                "com.tencent.xinWeChat",
                "nei'cun",
                "内存",
                "内衬",
            ),
            (
                "com.microsoft.VSCode",
                "com.tinyspeck.slackmacgap",
                "xian'cheng",
                "线程",
                "现成",
            ),
            (
                "com.todesktop.230313mzl4w4u92",
                "com.hnc.Discord",
                "bian'yi",
                "编译",
                "便宜",
            ),
            (
                "com.jetbrains.rustrover",
                "com.apple.MobileSMS",
                "zhi'zhen",
                "指针",
                "至真",
            ),
            (
                "com.apple.Terminal",
                "ru.keepcoder.Telegram",
                "huan'cun",
                "缓存",
                "换村",
            ),
            (
                "com.mitchellh.ghostty",
                "com.electron.lark",
                "yi'bu",
                "异步",
                "一步",
            ),
            (
                "dev.zed.zed",
                "com.bytedance.feishu",
                "jie'gou",
                "结构",
                "借狗",
            ),
            (
                "com.jetbrains.intellij",
                "com.tencent.xinWeChat",
                "fan'xing",
                "泛型",
                "反省",
            ),
            ("notion.id", "com.google.Chrome", "yin'yong", "引用", "应用"),
            ("md.obsidian", "com.apple.Safari", "suo'yin", "索引", "所因"),
        ];
        for (src_bundle, dst_bundle, key, term, chat_word) in cross_scope_cases {
            let dir = Dir::new();
            let journal = dir.journal();
            let src_scope = AppScope::from_bundle_id(src_bundle);
            let dst_scope = AppScope::from_bundle_id(dst_bundle);
            assert_ne!(src_scope, dst_scope);

            // 在源桶（如 CodeIde）连续明确选术语 3 次：
            for step in 0..3 {
                record_personal_selection_scoped(&journal, key, term, src_scope, now + step)
                    .unwrap();
            }
            // 同桶有效计数 ~3.0，跨桶严格折半 ~1.5 (< 1.8)：
            let src_cnt = decayed_counts_for_key_scoped(&journal, key, src_scope, now + 5)
                .get(term)
                .copied()
                .unwrap_or(0.0);
            let dst_cnt = decayed_counts_for_key_scoped(&journal, key, dst_scope, now + 5)
                .get(term)
                .copied()
                .unwrap_or(0.0);
            assert!((src_cnt - 3.0).abs() < 1e-3);
            assert!((dst_cnt - 1.5).abs() < 1e-3);

            // 在源桶（IDE）里：术语正常夺得第 0 槽！
            let mut in_src = vec![item(key, chat_word, 500_000), item(key, term, 2_000)];
            let out_src =
                apply_personal_overlay_scoped(&journal, key, None, src_scope, &mut in_src, now + 5);
            assert!(out_src.leader_protected);
            assert_eq!(in_src[0].word, term, "源桶内应正常习得术语首位");

            // 切到目标桶（ImChat / BrowserSearch）：跨桶 50% 权重（1.5 < 1.8），绝不劫持日常高频词第 0 槽！
            let mut in_dst = vec![item(key, chat_word, 500_000), item(key, term, 2_000)];
            let out_dst =
                apply_personal_overlay_scoped(&journal, key, None, dst_scope, &mut in_dst, now + 5);
            assert!(!out_dst.leader_protected);
            assert_eq!(
                in_dst[0].word, chat_word,
                "铁律 5 违背：{src_bundle} 学到的「{term}」劫持了 {dst_bundle} 的「{chat_word}」"
            );
            // 源桶对数增益高于目标桶，且均受 MAX_PERSONAL_CACHE_LOG_BONUS 严格约束。
            let bonus_src = personal_cache_log_bonus(&journal, key, term, src_scope, now + 5);
            let bonus_dst = personal_cache_log_bonus(&journal, key, term, dst_scope, now + 5);
            assert!(bonus_src > bonus_dst && bonus_src <= MAX_PERSONAL_CACHE_LOG_BONUS);
            executed += 1;
        }

        assert_eq!(executed, 50, "必须完整执行全部 50 组防劫持回放场景");
    }

    #[test]
    fn backspace_negative_feedback_never_banishes_common_single_chars_behind_rare_glyphs() {
        let dir = Dir::new();
        let journal = dir.journal();
        let now = 1_750_000_000i64;
        for ch in ["画", "化", "华"] {
            assert!(record_backspace_rollback_feedback(
                &journal,
                None,
                "hua",
                ch,
                AppScope::General,
                now,
                now + 2,
            )
            .unwrap());
        }
        let mut list = vec![
            item("hua", "花", 3_746_417),
            item("hua", "话", 3_745_917),
            item("hua", "画", 1_054_361),
            item("hua", "化", 951_401),
            item("hua", "华", 800_451),
            item("hua", "划", 385_366),
            item("hua", "滑", 156_890),
            item("hua", "哗", 68_257),
            item("hua", "桦", 13_149),
            item("hua", "㓰", 0),
            item("hua", "㕦", 0),
            item("hua", "㕲", 0),
        ];
        apply_personal_overlay_scoped(
            &journal,
            "hua",
            None,
            AppScope::General,
            &mut list,
            now + 10,
        );
        let pos = |w: &str| list.iter().position(|it| it.word == w).unwrap();
        // 即使刚退格过 `画`/`化`/`华`，三者仍必须稳居 `滑`/`哗`/`桦` 及零权生僻字之前，绝不掉出首页与 Top-16 神经窗口。
        assert!(pos("画") < pos("滑"));
        assert!(pos("化") < pos("滑"));
        assert!(pos("华") < pos("滑"));
        assert!(pos("化") < pos("㓰"));
    }
}
