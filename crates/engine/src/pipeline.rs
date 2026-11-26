//! Shurufa 单向候选流水线与纯函数边界：
//! `CursorSnapshot -> RecallPool -> UnifiedScorer -> PurePresenter`。
//!
//! 所有输入源、重排打分与展示投影严格单向流动：
//! 1. [`CursorSnapshot`] 冻结当前按键代次的不可变输入上下文（含 `locked_prefix`、左侧上文与右侧同行后文）。
//! 2. [`CandidateId`] / [`CandidateEntry`] 用单一结构化记录取代原先分散的并行向量，彻底消除下标错位。
//! 3. [`RecallPool`] 对多路召回结果按 `(CandidateKind, text)` 稳定去重。
//! 4. [`UnifiedScorer`] 作为唯一打分中心，统一承载 `.gram` 与 `Qwen3-0.6B` 对数线性融合目标，仅支持 `PinToFirst` 与屏蔽删除。
//! 5. [`PurePresenter`] 以纯函数完成精选 Emoji 投影与展示整形，不回改打分器状态。

use std::collections::HashSet;

use crate::neural_v2::{
    fuse_neural_response, neural_context_window, neural_right_context_window, NeuralDecodeRequest,
    NeuralDecodeResponse, NeuralFusionOutcome, NeuralFusionWeights,
};
use crate::ordering::StaticEvidence;
use crate::slot_allocator::{
    allocate_slots, AllocatedSlot, SlotBudget, SlotCandidate, SlotCandidateKind,
};
use crate::types::CandidateSource;

/// 单次会话内已选定并暂存在预编辑区中的词段（`locked_prefix` 组成单元）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LockedSegment {
    pub reading: String,
    pub text: String,
}

/// 每次按键或光标变更时构造的不可变输入上下文快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CursorSnapshot {
    pub generation: u64,
    pub scheme: u8,
    pub raw_input: String,
    pub caret: usize,
    pub locked_prefix: Vec<LockedSegment>,
    pub left_context: String,
    pub right_context: String,
    pub script_conversion: bool,
    pub emoji_enabled: bool,
    pub english_autocomplete_enabled: bool,
    pub page_size: u8,
}

impl CursorSnapshot {
    /// 拼接当前会话内已锁定的前缀汉字文本。
    pub fn locked_prefix_text(&self) -> String {
        let cap: usize = self.locked_prefix.iter().map(|seg| seg.text.len()).sum();
        let mut out = String::with_capacity(cap);
        for seg in &self.locked_prefix {
            out.push_str(&seg.text);
        }
        out
    }

    /// 拼接当前会话内已锁定前缀消耗的原始拼音片段。
    pub fn locked_prefix_reading(&self) -> String {
        let cap: usize = self.locked_prefix.iter().map(|seg| seg.reading.len()).sum();
        let mut out = String::with_capacity(cap);
        for seg in &self.locked_prefix {
            out.push_str(&seg.reading);
        }
        out
    }

    /// 返回送往 Tier 1 神经打分的完整左侧上文：`left_context + locked_prefix_text`，
    /// 按 `NEURAL_CONTEXT_MAX_CHARS` 保留尾部窗口。
    pub fn effective_left_context(&self) -> String {
        if self.locked_prefix.is_empty() {
            return neural_context_window(&self.left_context).to_owned();
        }
        let locked = self.locked_prefix_text();
        if locked.is_empty() {
            return neural_context_window(&self.left_context).to_owned();
        }
        let mut combined = String::with_capacity(self.left_context.len() + locked.len());
        combined.push_str(&self.left_context);
        combined.push_str(&locked);
        neural_context_window(&combined).to_owned()
    }

    /// 返回裁剪到同行 16 字上限的光标右侧后文。
    pub fn effective_right_context(&self) -> &str {
        neural_right_context_window(&self.right_context)
    }
}

/// 基于内容与代次计算的确定性候选身份标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CandidateId(pub u64);

impl CandidateId {
    /// 按代次、候选类别、文本、编码与来源计算 64 位 FNV-1a 内容哈希。
    pub fn for_entry(
        generation: u64,
        kind: CandidateKind,
        text: &str,
        code: &str,
        source: u8,
    ) -> Self {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;
        let mut hash = FNV_OFFSET;
        let mix = |h: &mut u64, bytes: &[u8]| {
            for &b in bytes {
                *h ^= u64::from(b);
                *h = h.wrapping_mul(FNV_PRIME);
            }
        };
        mix(&mut hash, &generation.to_le_bytes());
        mix(&mut hash, &[kind as u8, source]);
        mix(&mut hash, text.as_bytes());
        mix(&mut hash, &[0xff]);
        mix(&mut hash, code.as_bytes());
        Self(hash)
    }
}

/// 候选大类，替代原先零散的来源号分支判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum CandidateKind {
    #[default]
    PrimaryChinese = 0,
    English = 1,
    ProjectedEmoji = 2,
    LocalOrLiteral = 3,
    Online = 4,
}

impl CandidateKind {
    /// 由引擎 `CandidateSource` 序号推导候选大类。
    pub fn from_source(source: u8) -> Self {
        if source == CandidateSource::EnglishDictionary as u8 {
            Self::English
        } else if source == CandidateSource::Emoji as u8 {
            Self::ProjectedEmoji
        } else if source == CandidateSource::CloudSuggestion as u8
            || source == CandidateSource::CloudPending as u8
            || source == CandidateSource::AiSuggestion as u8
            || source == CandidateSource::AiPending as u8
        {
            Self::Online
        } else if matches!(
            SlotCandidateKind::from_source(source),
            SlotCandidateKind::PrimaryChinese
        ) {
            Self::PrimaryChinese
        } else {
            Self::LocalOrLiteral
        }
    }

    /// 映射为 `SlotAllocator` 使用的槽位类别。
    pub fn to_slot_kind(self) -> SlotCandidateKind {
        match self {
            Self::PrimaryChinese => SlotCandidateKind::PrimaryChinese,
            Self::English => SlotCandidateKind::English,
            Self::Online => SlotCandidateKind::Online,
            Self::ProjectedEmoji | Self::LocalOrLiteral => SlotCandidateKind::Other,
        }
    }
}

/// 候选条目的生成来源追踪，替代原先的 `engine_order` + `projected_emojis` 映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateOrigin {
    EngineRow(usize),
    InjectedBeam,
    ProjectedEmoji {
        anchor_word: String,
        emoji: &'static str,
    },
}

impl Default for CandidateOrigin {
    fn default() -> Self {
        Self::EngineRow(0)
    }
}

/// 单一结构化候选条目，承载候选文本、读音、分数分量与稳定来源身份。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CandidateEntry {
    pub id: CandidateId,
    pub kind: CandidateKind,
    pub text: String,
    pub code: String,
    pub annotation: String,
    pub source: u8,
    pub corrected: bool,
    pub answers_key: bool,
    pub pinned_first: bool,
    /// L4 硬约束：用户显式固定的 1-based 候选排位（`0` 表示未固定，`1..=5` 锁定最终下标 `pos - 1`）。
    pub fixed_position: u8,
    pub deletable: bool,
    pub channel_log_prob: f64,
    pub tier0_gram_log_prob: Option<f64>,
    pub tier1_neural_log_prob: Option<f64>,
    pub user_log_prob: f64,
    pub static_evidence: Option<StaticEvidence>,
    pub keyboard_runner_up: bool,
    /// 首词是否源自词典或多字词汇边界（`source.is_dictionary()` 或首词长 `>= 2`），供首屏区分词汇首词与散字起手。
    pub lexical_head: bool,
    pub origin: CandidateOrigin,
}

impl CandidateEntry {
    /// 构造带默认分数与确定性 [`CandidateId`] 的候选条目。
    pub fn new(text: impl Into<String>, code: impl Into<String>, source: u8) -> Self {
        let text = text.into();
        let code = code.into();
        let kind = CandidateKind::from_source(source);
        let id = CandidateId::for_entry(0, kind, &text, &code, source);
        let deletable = kind != CandidateKind::Online
            && kind != CandidateKind::ProjectedEmoji
            && text.chars().nth(1).is_some();
        Self {
            id,
            kind,
            text,
            code,
            annotation: String::new(),
            source,
            corrected: false,
            answers_key: false,
            pinned_first: false,
            fixed_position: 0,
            deletable,
            channel_log_prob: 0.0,
            tier0_gram_log_prob: None,
            tier1_neural_log_prob: None,
            user_log_prob: 0.0,
            static_evidence: None,
            keyboard_runner_up: false,
            lexical_head: false,
            origin: CandidateOrigin::default(),
        }
    }

    /// 返回生效中的 1-based 固定排位（`fixed_position > 0` 优先，其次 `pinned_first` 映射为 `1`）。
    #[inline]
    pub fn effective_fixed_position(&self) -> u8 {
        if self.fixed_position > 0 {
            self.fixed_position
        } else if self.pinned_first {
            1
        } else {
            0
        }
    }

    /// 用当前视图代次刷新 [`CandidateId`]。
    pub fn stamp_generation(&mut self, generation: u64) {
        self.id =
            CandidateId::for_entry(generation, self.kind, &self.text, &self.code, self.source);
    }
}

/// 召回候选池：对多路召回候选执行确定性 `(CandidateKind, text)` 去重，保留高优先级首见项。
#[derive(Debug, Clone, Default)]
pub struct RecallPool {
    pub entries: Vec<CandidateEntry>,
}

impl RecallPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收入一批候选并按 `(kind, text)` 去重。
    pub fn from_candidates(candidates: Vec<CandidateEntry>) -> Self {
        let mut pool = Self {
            entries: Vec::with_capacity(candidates.len()),
        };
        let mut seen: HashSet<(CandidateKind, String)> = HashSet::with_capacity(candidates.len());
        for entry in candidates {
            if seen.insert((entry.kind, entry.text.clone())) {
                pool.entries.push(entry);
            }
        }
        pool
    }

    /// 将去重后的候选移交下一步打分器。
    pub fn into_entries(self) -> Vec<CandidateEntry> {
        self.entries
    }
}

/// 统一概率打分中心：单一对数线性目标同时服务 Tier 0（`.gram`）与 Tier 1（`Qwen3-0.6B`）。
pub struct UnifiedScorer;

impl UnifiedScorer {
    /// 计算单条候选的对数线性综合得分：
    /// `channel + lambda_neural * neural + lambda_stat * rel_stat + lambda_user * user`。
    #[inline]
    pub fn combined_score(
        channel_log_prob: f64,
        rel_stat_log_prob: f64,
        neural_log_prob: f64,
        user_log_prob: f64,
        weights: NeuralFusionWeights,
    ) -> f64 {
        channel_log_prob
            + weights.lambda_neural * neural_log_prob
            + weights.lambda_stat * rel_stat_log_prob
            + weights.lambda_user * user_log_prob
    }

    /// 执行 Tier 0 与 Tier 1（及受限束搜索结果）统一融合排序，严格遵守槽 0 锁与 `PinToFirst`。
    pub fn score(
        total_base_len: usize,
        request: &NeuralDecodeRequest,
        response: &NeuralDecodeResponse,
        weights: NeuralFusionWeights,
        lock_slot_0: bool,
    ) -> NeuralFusionOutcome {
        fuse_neural_response(total_base_len, request, response, weights, lock_slot_0)
    }
}

/// 纯函数展示投影器：在不回改打分器内部状态的前提下，将已排序候选投影为最终展示页序列。
pub struct PurePresenter;

impl PurePresenter {
    /// 根据 [`CursorSnapshot`] 执行内联 Emoji 投影等纯展示变换。
    pub fn present<'p, F>(
        mut scored: Vec<CandidateEntry>,
        cursor: &CursorSnapshot,
        preferred_emoji: F,
    ) -> Vec<CandidateEntry>
    where
        F: Fn(&str) -> Option<&'p str>,
    {
        // 先剥离上一轮投影出的临时 Emoji 席位。
        scored.retain(|entry| !matches!(entry.origin, CandidateOrigin::ProjectedEmoji { .. }));
        if !cursor.emoji_enabled || scored.is_empty() {
            return scored;
        }
        let base: Vec<SlotCandidate<'_>> = scored
            .iter()
            .enumerate()
            .map(|(index, entry)| SlotCandidate {
                id: index,
                text: entry.text.as_str(),
                kind: entry.kind.to_slot_kind(),
                answers_key: entry.answers_key,
                pinned_first: entry.pinned_first,
                fixed_position: entry.effective_fixed_position(),
            })
            .collect();
        let budget = SlotBudget {
            page_size: usize::from(cursor.page_size.max(2)),
            ..SlotBudget::default()
        };
        let allocated = allocate_slots(&base, budget, true, preferred_emoji);
        if allocated.len() == scored.len() {
            return scored;
        }
        let mut old: Vec<Option<CandidateEntry>> = scored.into_iter().map(Some).collect();
        let mut presented = Vec::with_capacity(allocated.len());
        for slot in allocated {
            match slot {
                AllocatedSlot::Base(id) => {
                    if let Some(entry) = old[id].take() {
                        presented.push(entry);
                    }
                }
                AllocatedSlot::ProjectedEmoji { anchor_word, emoji } => {
                    let mut entry = CandidateEntry::new(emoji, "", CandidateSource::Emoji as u8);
                    entry.kind = CandidateKind::ProjectedEmoji;
                    entry.deletable = false;
                    entry.origin = CandidateOrigin::ProjectedEmoji { anchor_word, emoji };
                    entry.stamp_generation(cursor.generation);
                    presented.push(entry);
                }
            }
        }
        presented
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_snapshot_merges_locked_prefix_into_effective_left_context() {
        let snapshot = CursorSnapshot {
            generation: 7,
            scheme: 0,
            raw_input: "paobu".into(),
            caret: 5,
            locked_prefix: vec![
                LockedSegment {
                    reading: "qu".into(),
                    text: "去".into(),
                },
                LockedSegment {
                    reading: "haitan".into(),
                    text: "海滩".into(),
                },
            ],
            left_context: "今天下午我们一起".into(),
            right_context: "然后回家\n下一行".into(),
            script_conversion: true,
            emoji_enabled: true,
            english_autocomplete_enabled: true,
            page_size: 6,
        };
        assert_eq!(snapshot.locked_prefix_text(), "去海滩");
        assert_eq!(snapshot.locked_prefix_reading(), "quhaitan");
        assert_eq!(snapshot.effective_left_context(), "今天下午我们一起去海滩");
        assert_eq!(snapshot.effective_right_context(), "然后回家");

        let mut popped = snapshot.clone();
        let last = popped.locked_prefix.pop().unwrap();
        assert_eq!(last.reading, "haitan");
        assert_eq!(popped.effective_left_context(), "今天下午我们一起去");
    }

    #[test]
    fn recall_pool_deduplicates_preserving_first_entry() {
        let mut first = CandidateEntry::new("假设", "jia'she", 0);
        first.channel_log_prob = 0.0;
        let mut dup = CandidateEntry::new("假设", "jia'she", 8);
        dup.channel_log_prob = -3.0;
        let second = CandidateEntry::new("架设", "jia'she", 0);

        let pool = RecallPool::from_candidates(vec![first.clone(), dup, second.clone()]);
        assert_eq!(pool.entries.len(), 2);
        assert_eq!(pool.entries[0].id, first.id);
        assert_eq!(pool.entries[0].source, 0);
        assert_eq!(pool.entries[1].text, "架设");
    }

    #[test]
    fn pure_presenter_projects_emoji_without_displacing_pinned_or_fixed_seats() {
        let mut pinned = CandidateEntry::new("麻", "ma", 0);
        pinned.answers_key = true;
        pinned.pinned_first = true;
        let mut second = CandidateEntry::new("大笑", "daxiao", 0);
        second.answers_key = true;
        let mut third = CandidateEntry::new("码", "ma", 0);
        third.answers_key = true;
        let mut fourth = CandidateEntry::new("嘛", "ma", 0);
        fourth.answers_key = true;
        fourth.fixed_position = 4;
        let mut fifth = CandidateEntry::new("妈", "ma", 0);
        fifth.answers_key = true;
        let mut sixth = CandidateEntry::new("马", "ma", 0);
        sixth.answers_key = true;

        let cursor = CursorSnapshot {
            generation: 11,
            emoji_enabled: true,
            page_size: 6,
            ..CursorSnapshot::default()
        };
        let presented = PurePresenter::present(
            vec![pinned, second, third, fourth, fifth, sixth],
            &cursor,
            |_| None,
        );
        assert_eq!(presented[0].text, "麻");
        assert!(presented[0].pinned_first);
        assert_eq!(presented[1].text, "大笑");
        assert_eq!(presented[2].kind, CandidateKind::ProjectedEmoji);
        assert_eq!(presented[2].text, "😂");
        assert_eq!(presented[3].text, "嘛");
        assert_eq!(presented[3].fixed_position, 4);
        assert!(matches!(
            presented[2].origin,
            CandidateOrigin::ProjectedEmoji { ref anchor_word, .. } if anchor_word == "大笑"
        ));
    }
}
