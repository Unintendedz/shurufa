//! 候选排序与重排辅助：统一概率融合排序、整句次选读法后移及上下文窗口裁剪。

use crate::SchemeType;

/// 词典命中来源集合：`CandidateSource::Database` 与 `CandidateSource::UserDatabase`。
pub const DICTIONARY_SOURCES: [u8; 2] = [0, 1];

/// `CandidateSource::Generated`：词网格解码出的整句路径。
pub const LATTICE_SOURCE: u8 = 8;

/// 会产生回答完整按键的整句或长词读法的来源集合：`Generated`、`NeuralKeyboard`、`Collocation`、`NeuralV2`、`NeuralConstrained`。
const SENTENCE_READING_SOURCES: [u8; 5] = [LATTICE_SOURCE, 11, 17, 19, 20];

/// `CandidateSource::QuickPhrase`、`Emoji` 与 `Kaomoji`：目录查词按目录原序展示，不参与整句模型比较。
const CATALOG_SOURCES: [u8; 3] = [5, 6, 7];

/// `CandidateSource::AiSuggestion` 与 `AiPending`：在线座位单独摆放，不参与本地整句重排比较。
const AI_SEAT_SOURCES: [u8; 2] = [3, 12];

/// `CandidateSource::EnglishDictionary`：英文混输领衔时保持引擎原序，不参与中文重排。
const ENGLISH_SOURCE: u8 = 4;

/// `CandidateSource::Fallback`：大写字面输入、特殊符号字面行及无匹配兜底原文，不参与中文句子模型评分。
const FALLBACK_SOURCE: u8 = 9;

/// `CandidateSource::MixedJapanese`：历史来源序号保留过滤保护。
const MIXED_JAPANESE_SOURCE: u8 = 13;

/// `neural_v2` 统一概率重排前置判断：不因 `Database` 首位一刀切免重排，仅排除英文/字面领衔及不足两行的列表。
pub fn rerank_needs_model_v2<'a, R: OrderRowSource<'a> + ?Sized>(
    scheme: u8,
    _answered_by_pinyin_fallback: bool,
    rows: &'a R,
) -> bool {
    if !reorders_candidates(scheme) {
        return false;
    }
    if rows.len() < 2 {
        return false;
    }
    let leader = rows.get(0);
    if matches!(leader.source, ENGLISH_SOURCE | MIXED_JAPANESE_SOURCE) {
        return false;
    }
    row_answers_key(scheme, leader)
}

/// `neural_v2` 统一概率融合排序：`score = neural_weight * neural + static_weight * statik + channel`。
/// 当列表已有序或长度非法时返回 `None`。
pub fn neural_v2_fused_order(
    neural_log_probs: &[f64],
    static_log_probs: &[f64],
    channel_log_probs: &[f64],
    neural_weight: f64,
    static_weight: f64,
) -> Option<Vec<usize>> {
    let count = neural_log_probs.len();
    if count < 2 || static_log_probs.len() != count || channel_log_probs.len() != count {
        return None;
    }
    let mut scores = Vec::with_capacity(count);
    for i in 0..count {
        scores.push(
            neural_weight * neural_log_probs[i]
                + static_weight * static_log_probs[i]
                + channel_log_probs[i],
        );
    }
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
    if order.iter().enumerate().all(|(seat, &idx)| seat == idx) {
        None
    } else {
        Some(order)
    }
}

fn scheme_type(scheme: u8) -> Option<SchemeType> {
    SchemeType::from_u8(scheme)
}

/// 当前方案是否允许对候选列表做整句重排与次选后移。
pub fn reorders_candidates(scheme: u8) -> bool {
    scheme_type(scheme).is_none_or(SchemeType::holds_phrase_progress)
}

/// 双拼的纠错整句不抢首选；全拼纠错行允许领先。
fn corrections_may_lead(scheme: u8) -> bool {
    scheme_type(scheme) != Some(SchemeType::Shuangpin)
}

/// 原地应用排列，`order` 将每个新座位映射到旧座位。
pub fn apply_order<T>(items: &mut [T], order: &[usize]) {
    debug_assert_eq!(items.len(), order.len());
    apply_order_with(order, |left, right| items.swap(left, right));
}

/// 按排列的循环调用交换操作，让调用方同步移动多个并行数组。
pub fn apply_order_with(order: &[usize], mut swap: impl FnMut(usize, usize)) {
    if order.iter().enumerate().all(|(seat, row)| seat == *row) {
        return;
    }
    let mut stack_visited = [false; 128];
    let mut heap_visited;
    let visited = if order.len() <= stack_visited.len() {
        &mut stack_visited[..order.len()]
    } else {
        heap_visited = vec![false; order.len()];
        &mut heap_visited[..]
    };
    for start in 0..order.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut current = start;
        while order[current] != start {
            let next = order[current];
            swap(current, next);
            visited[next] = true;
            current = next;
        }
    }
}

/// 维持引擎下标映射表：首次置换前按恒等映射初始化。
pub fn ensure_engine_order(engine_order: &mut Vec<usize>, count: usize) {
    if engine_order.len() == count {
        return;
    }
    engine_order.clear();
    engine_order.extend(0..count);
}

pub fn rotate_to_front<T>(items: &mut [T], index: usize) {
    items[..=index].rotate_right(1);
}

/// 上下文超出模型窗口时每次滑动的字符步长。
pub const RERANK_CONTEXT_STEP: usize = 16;

/// 按固定步长裁剪交给重排器的上文尾部，使候选增长时前缀缓存保持稳定。
pub fn rerank_context(context: &str, window: usize, longest: usize) -> &str {
    let room = window.saturating_sub(longest + 1);
    let keep = room / RERANK_CONTEXT_STEP * RERANK_CONTEXT_STEP;
    let count = context.chars().count();
    if count <= room {
        return context;
    }
    let skip = count - keep;
    context
        .char_indices()
        .nth(skip)
        .map_or("", |(start, _)| &context[start..])
}

/// 排序决策读取的一行候选：文字、来源序号（`CandidateSource`），是否回答整个按键串，是否是纠错后的读法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderRow<'a> {
    pub text: &'a str,
    pub source: u8,
    pub answers_key: bool,
    pub corrected: bool,
}

/// 词网格的自然对数总分，保留整数毫 nat 与字数避免快照丢失精度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticEvidence {
    pub log_prob_milli_nats: i64,
    pub characters: usize,
    pub keyboard_neural: Option<(usize, i32)>,
}

impl StaticEvidence {
    pub const fn new(log_prob_milli_nats: i64, characters: usize) -> Self {
        Self {
            log_prob_milli_nats,
            characters,
            keyboard_neural: None,
        }
    }

    fn mean(self) -> Option<f64> {
        (self.characters > 0)
            .then(|| self.log_prob_milli_nats as f64 / 1000.0 / self.characters as f64)
    }
}

/// 排序决策读取的候选行来源。
pub trait OrderRowSource<'a> {
    fn len(&self) -> usize;
    fn get(&self, index: usize) -> OrderRow<'a>;

    fn static_evidence(&self, _index: usize) -> Option<StaticEvidence> {
        None
    }

    fn keyboard_runner_up(&self, _index: usize) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<'a> OrderRowSource<'a> for [OrderRow<'a>] {
    fn len(&self) -> usize {
        <[OrderRow<'a>]>::len(self)
    }

    fn get(&self, index: usize) -> OrderRow<'a> {
        self[index]
    }
}

impl<'a> OrderRowSource<'a> for Vec<OrderRow<'a>> {
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn get(&self, index: usize) -> OrderRow<'a> {
        self[index]
    }
}

impl<'a, const N: usize> OrderRowSource<'a> for [OrderRow<'a>; N] {
    fn len(&self) -> usize {
        N
    }

    fn get(&self, index: usize) -> OrderRow<'a> {
        self[index]
    }
}

/// 为已有候选行附上可选的静态证据；长度不一致时整份证据失效。
pub struct ScoredOrderRows<'s, R: ?Sized> {
    rows: &'s R,
    evidence: &'s [Option<StaticEvidence>],
}

impl<'s, R: ?Sized> ScoredOrderRows<'s, R> {
    pub fn new(rows: &'s R, evidence: &'s [Option<StaticEvidence>]) -> Self {
        Self { rows, evidence }
    }
}

impl<'a, R: OrderRowSource<'a> + ?Sized> OrderRowSource<'a> for ScoredOrderRows<'_, R> {
    fn len(&self) -> usize {
        self.rows.len()
    }
    fn get(&self, index: usize) -> OrderRow<'a> {
        self.rows.get(index)
    }
    fn static_evidence(&self, index: usize) -> Option<StaticEvidence> {
        if self.evidence.len() != self.rows.len() {
            return None;
        }
        self.evidence.get(index).copied().flatten()
    }
    fn keyboard_runner_up(&self, index: usize) -> bool {
        self.rows.keyboard_runner_up(index)
    }
}

/// 借用快照里的并行候选数组，避免排序前复制一份行数组。
pub struct ParallelOrderRows<'a> {
    texts: &'a [String],
    sources: &'a [u8],
    answers_key: &'a [bool],
    corrected: &'a [bool],
    static_evidence: Option<&'a [Option<StaticEvidence>]>,
    keyboard_runner_ups: Option<&'a [bool]>,
}

impl<'a> ParallelOrderRows<'a> {
    pub fn new(
        texts: &'a [String],
        sources: &'a [u8],
        answers_key: &'a [bool],
        corrected: &'a [bool],
    ) -> Self {
        Self {
            texts,
            sources,
            answers_key,
            corrected,
            static_evidence: None,
            keyboard_runner_ups: None,
        }
    }

    pub fn with_static_evidence(mut self, evidence: &'a [Option<StaticEvidence>]) -> Self {
        self.static_evidence = (evidence.len() == self.texts.len()).then_some(evidence);
        self
    }

    pub fn with_keyboard_runner_ups(mut self, runner_ups: &'a [bool]) -> Self {
        self.keyboard_runner_ups = (runner_ups.len() == self.texts.len()).then_some(runner_ups);
        self
    }
}

impl<'a> OrderRowSource<'a> for ParallelOrderRows<'a> {
    fn len(&self) -> usize {
        self.texts.len()
    }

    fn static_evidence(&self, index: usize) -> Option<StaticEvidence> {
        self.static_evidence?.get(index).copied().flatten()
    }

    fn keyboard_runner_up(&self, index: usize) -> bool {
        self.keyboard_runner_ups
            .is_some_and(|runner_ups| runner_ups.get(index).copied().unwrap_or(false))
    }

    fn get(&self, index: usize) -> OrderRow<'a> {
        OrderRow {
            text: &self.texts[index],
            source: self.sources[index],
            answers_key: self.answers_key[index],
            corrected: self.corrected[index],
        }
    }
}

/// 借用单向候选条目切片参与排序决策，直接复用结构化 [`CandidateEntry`] 字段。
pub struct CandidateOrderRows<'a>(pub &'a [crate::pipeline::CandidateEntry]);

impl<'a> OrderRowSource<'a> for CandidateOrderRows<'a> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn static_evidence(&self, index: usize) -> Option<StaticEvidence> {
        self.0.get(index)?.static_evidence
    }

    fn keyboard_runner_up(&self, index: usize) -> bool {
        self.0
            .get(index)
            .is_some_and(|entry| entry.keyboard_runner_up)
    }

    fn get(&self, index: usize) -> OrderRow<'a> {
        let entry = &self.0[index];
        OrderRow {
            text: &entry.text,
            source: entry.source,
            answers_key: entry.answers_key,
            corrected: entry.corrected,
        }
    }
}

fn row_answers_key(scheme: u8, row: OrderRow<'_>) -> bool {
    row.answers_key
        && !CATALOG_SOURCES.contains(&row.source)
        && !AI_SEAT_SOURCES.contains(&row.source)
        && !matches!(
            row.source,
            ENGLISH_SOURCE | FALLBACK_SOURCE | MIXED_JAPANESE_SOURCE
        )
        && (corrections_may_lead(scheme) || !row.corrected)
}

const RUNNER_UP_MAX_MEAN_GAP_NATS: f64 = 3.8;
const TWO_CHAR_SPLIT_FALLBACK_MAX_MEAN_NATS: f64 = -8.5;
const EXPAND_FIFTH_READING_MAX_MEAN_GAP_NATS: f64 = 1.2;
const TWO_CHAR_DEFERRED_PAGE_BUDGET: usize = 6;

#[allow(clippy::too_many_arguments)]
fn visit_runner_up_order(
    count: usize,
    first_reading: usize,
    keep: usize,
    front_limit: usize,
    expand_front_with_unused_keep: bool,
    deferred_front_limit: Option<usize>,
    is_reading: &impl Fn(usize) -> bool,
    is_deferred_reading: &impl Fn(usize) -> bool,
    is_far_below_leader: &impl Fn(usize) -> bool,
    is_promoted_exact_non_reading: &impl Fn(usize) -> bool,
    mut emit: impl FnMut(usize),
) {
    let primary_reading = |index: usize| is_reading(index) && !is_deferred_reading(index);
    let deferred_reading = |index: usize| is_reading(index) && is_deferred_reading(index);
    for index in 0..first_reading {
        emit(index);
    }
    let mut kept_primary = 0;
    for index in first_reading..count {
        if primary_reading(index) && !is_far_below_leader(index) {
            emit(index);
            kept_primary += 1;
            if kept_primary == keep {
                break;
            }
        }
    }
    let mut kept_deferred = 0;
    if kept_primary < keep {
        for index in first_reading..count {
            if deferred_reading(index) && !is_far_below_leader(index) {
                emit(index);
                kept_deferred += 1;
                if kept_primary + kept_deferred == keep {
                    break;
                }
            }
        }
    }
    let kept_total = kept_primary + kept_deferred;
    let promote_exact_after_budget = kept_total == 0 && deferred_front_limit.is_some();
    let effective_front_limit = if kept_total == 0 {
        if let Some(limit) = deferred_front_limit {
            limit
        } else if expand_front_with_unused_keep {
            front_limit + keep
        } else {
            front_limit
        }
    } else if expand_front_with_unused_keep {
        front_limit + keep.saturating_sub(kept_total)
    } else {
        front_limit
    };
    let mut kept_front = 0;
    for index in (first_reading + 1)..count {
        if is_reading(index) {
            continue;
        }
        if kept_front < effective_front_limit {
            emit(index);
            kept_front += 1;
        } else if promote_exact_after_budget && is_promoted_exact_non_reading(index) {
            emit(index);
        }
    }
    let mut primary_near_seen = 0;
    for index in first_reading..count {
        if primary_reading(index) && !is_far_below_leader(index) {
            if primary_near_seen >= kept_primary {
                emit(index);
            }
            primary_near_seen += 1;
        }
    }
    for index in first_reading..count {
        if primary_reading(index) && is_far_below_leader(index) {
            emit(index);
        }
    }
    let mut deferred_near_seen = 0;
    for index in first_reading..count {
        if deferred_reading(index) && !is_far_below_leader(index) {
            if deferred_near_seen >= kept_deferred {
                emit(index);
            }
            deferred_near_seen += 1;
        }
    }
    for index in first_reading..count {
        if deferred_reading(index) && is_far_below_leader(index) {
            emit(index);
        }
    }
    let mut skipped_front = 0;
    for index in (first_reading + 1)..count {
        if is_reading(index) {
            continue;
        }
        if skipped_front < effective_front_limit {
            skipped_front += 1;
            continue;
        }
        if promote_exact_after_budget && is_promoted_exact_non_reading(index) {
            continue;
        }
        emit(index);
    }
}

pub fn runner_up_order<'a, R: OrderRowSource<'a> + ?Sized>(
    scheme: u8,
    rows: &R,
) -> Option<Vec<usize>> {
    const SENTENCE_SYLLABLES: usize = 2;
    const LONG_SENTENCE_CHARACTERS: usize = 3;
    const SENTENCE_READINGS: usize = 4;
    const FRONT_NON_READING_LIMIT: usize = 2;
    const SHORT_FRONT_ROW_LIMIT: usize = 4;

    if !reorders_candidates(scheme) {
        return None;
    }
    let count = rows.len();
    if count < 2 {
        return None;
    }
    let is_sentence_source = |index: usize| {
        let row = rows.get(index);
        SENTENCE_READING_SOURCES.contains(&row.source)
            && row.text.chars().count() >= SENTENCE_SYLLABLES
    };
    let any_reading_answers_key =
        (0..count).any(|index| rows.get(index).answers_key && is_sentence_source(index));
    let first_reading = (0..count).find(|&index| {
        is_sentence_source(index) && (!any_reading_answers_key || rows.get(index).answers_key)
    })?;
    let width = rows.get(first_reading).text.chars().count();
    let is_reading = |index: usize| {
        if any_reading_answers_key {
            rows.get(index).answers_key && is_sentence_source(index)
        } else {
            SENTENCE_READING_SOURCES.contains(&rows.get(index).source)
                && rows.get(index).text.chars().count() == width
        }
    };
    let expand_front_with_unused_keep = width >= LONG_SENTENCE_CHARACTERS;
    let defer_two_char_splits = width == SENTENCE_SYLLABLES
        && first_reading > 0
        && DICTIONARY_SOURCES.contains(&rows.get(0).source)
        && !rows.get(0).corrected;
    let deferred_front_limit =
        defer_two_char_splits.then(|| TWO_CHAR_DEFERRED_PAGE_BUDGET.saturating_sub(first_reading));

    let defer_corrected = !rows.get(first_reading).corrected;
    let is_deferred_reading = |index: usize| defer_corrected && rows.get(index).corrected;
    let first_reading_mean = rows
        .static_evidence(first_reading)
        .and_then(StaticEvidence::mean);
    let leader_mean = (width == LONG_SENTENCE_CHARACTERS)
        .then_some(first_reading_mean)
        .flatten();
    let far_below_leader = |index: usize| {
        if defer_two_char_splits {
            return rows
                .static_evidence(index)
                .and_then(StaticEvidence::mean)
                .is_some_and(|mean| mean < TWO_CHAR_SPLIT_FALLBACK_MAX_MEAN_NATS);
        }
        index != first_reading
            && match (
                leader_mean,
                rows.static_evidence(index).and_then(StaticEvidence::mean),
            ) {
                (Some(leader), Some(candidate)) => leader - candidate > RUNNER_UP_MAX_MEAN_GAP_NATS,
                _ => false,
            }
    };
    let expand_to_five_readings = if width >= LONG_SENTENCE_CHARACTERS {
        let fifth_close = first_reading_mean.is_some_and(|leader| {
            (first_reading..count)
                .filter(|&idx| {
                    is_reading(idx) && !is_deferred_reading(idx) && !far_below_leader(idx)
                })
                .nth(SENTENCE_READINGS)
                .and_then(|idx| rows.static_evidence(idx).and_then(StaticEvidence::mean))
                .is_some_and(|fifth| leader - fifth <= EXPAND_FIFTH_READING_MAX_MEAN_GAP_NATS)
        });
        let second_front_weak = ((first_reading + 1)..count)
            .filter(|&idx| !is_reading(idx))
            .nth(1)
            .is_none_or(|idx| rows.get(idx).text.chars().count() <= 1);
        fifth_close && second_front_weak
    } else {
        false
    };
    let keep = if width >= LONG_SENTENCE_CHARACTERS {
        SENTENCE_READINGS + usize::from(expand_to_five_readings)
    } else {
        1
    };
    let front_limit = if width >= LONG_SENTENCE_CHARACTERS {
        FRONT_NON_READING_LIMIT.saturating_sub(usize::from(expand_to_five_readings))
    } else {
        SHORT_FRONT_ROW_LIMIT
    };
    let is_promoted_exact_non_reading = |index: usize| {
        let row = rows.get(index);
        DICTIONARY_SOURCES.contains(&row.source) && row.answers_key && !row.corrected
    };

    let mut output = 0;
    let mut identity = true;
    visit_runner_up_order(
        count,
        first_reading,
        keep,
        front_limit,
        expand_front_with_unused_keep,
        deferred_front_limit,
        &is_reading,
        &is_deferred_reading,
        &far_below_leader,
        &is_promoted_exact_non_reading,
        |index| {
            identity &= output == index;
            output += 1;
        },
    );
    debug_assert_eq!(output, count);
    if identity {
        return None;
    }

    let mut order = Vec::with_capacity(count);
    visit_runner_up_order(
        count,
        first_reading,
        keep,
        front_limit,
        expand_front_with_unused_keep,
        deferred_front_limit,
        &is_reading,
        &is_deferred_reading,
        &far_below_leader,
        &is_promoted_exact_non_reading,
        |index| {
            order.push(index);
        },
    );
    debug_assert_eq!(order.len(), count);
    if order.iter().enumerate().all(|(seat, index)| seat == *index) {
        return None;
    }
    Some(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, source: u8) -> OrderRow<'_> {
        OrderRow {
            text,
            source,
            answers_key: true,
            corrected: false,
        }
    }

    #[test]
    fn rerank_context_that_fits_is_handed_over_whole() {
        assert_eq!(rerank_context("你好世界", 64, 10), "你好世界");
        assert_eq!(rerank_context("", 64, 10), "");
    }

    #[test]
    fn rerank_context_holds_still_while_a_candidate_grows() {
        let context: String = "今天天气很好我们一起去公园散步".repeat(8);
        let windows: Vec<&str> = (1..=40)
            .map(|longest| rerank_context(&context, 64, longest))
            .collect();
        let mut distinct = windows.clone();
        distinct.dedup();
        assert!(
            distinct.len() <= 64 / RERANK_CONTEXT_STEP + 1,
            "{}",
            distinct.len()
        );
        for (longest, window) in (1..=40).zip(&windows) {
            let count = window.chars().count();
            assert!(count + longest < 64, "longest {longest} kept {count}");
            assert_eq!(count % RERANK_CONTEXT_STEP, 0);
            assert!(context.ends_with(window));
        }
    }

    #[test]
    fn rerank_context_cuts_on_a_character_boundary_and_can_empty() {
        let context = "a中b文".repeat(40);
        let window = rerank_context(&context, 64, 20);
        assert_eq!(window.chars().count(), 32);
        assert!(context.ends_with(window));
        assert_eq!(rerank_context(&context, 64, 70), "");
    }

    #[test]
    fn in_place_order_applies_candidate_permutations() {
        let mut values = vec!["zero", "one", "two", "three", "four"];
        apply_order(&mut values, &[2, 4, 1, 0, 3]);
        assert_eq!(values, vec!["two", "four", "one", "zero", "three"]);
    }

    #[test]
    fn in_place_order_preserves_ownership_and_storage_boundaries() {
        struct Value(usize);
        for count in [0, 1, 127, 128, 129, 256, 8192] {
            let mut order: Vec<usize> = (0..count).collect();
            if count > 1 {
                order[1..].rotate_left(1);
            }
            let mut values: Vec<Value> = (0..count).map(Value).collect();
            let pointer = values.as_ptr();
            let (_, allocations) = crate::ime::personal_rerank::allocations::count(|| {
                apply_order(&mut values, &order);
            });
            assert_eq!(values.as_ptr(), pointer);
            assert!(allocations <= usize::from(count > 128));
            assert_eq!(
                values.iter().map(|value| value.0).collect::<Vec<_>>(),
                order
            );
        }
        let mut values: Vec<usize> = (0..8192).collect();
        let order = values.clone();
        let (_, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            apply_order(&mut values, &order);
        });
        assert_eq!(values, order);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn in_place_order_matches_every_small_permutation() {
        fn visit(order: &mut [usize], start: usize) {
            if start == order.len() {
                let mut values: Vec<usize> = (0..order.len()).collect();
                apply_order(&mut values, order);
                assert_eq!(values, order);
                return;
            }
            for next in start..order.len() {
                order.swap(start, next);
                visit(order, start + 1);
                order.swap(start, next);
            }
        }
        for count in 0..=7 {
            visit(&mut (0..count).collect::<Vec<_>>(), 0);
        }
    }

    #[test]
    fn one_lattice_row_needs_no_temporary_order_allocations() {
        let rows = [row("你好", LATTICE_SOURCE), row("你", 0)];
        let (order, allocations) =
            crate::ime::personal_rerank::allocations::count(|| runner_up_order(0, &rows));
        assert_eq!(order, None);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn engine_order_is_built_only_when_missing() {
        let mut engine_order = Vec::new();
        ensure_engine_order(&mut engine_order, 3);
        assert_eq!(engine_order, vec![0, 1, 2]);
        engine_order.swap(0, 2);
        ensure_engine_order(&mut engine_order, 3);
        assert_eq!(engine_order, vec![2, 1, 0]);
    }

    #[test]
    fn rotate_to_front_keeps_the_rest_in_order() {
        let mut items = vec!["a", "b", "c", "d"];
        rotate_to_front(&mut items, 2);
        assert_eq!(items, vec!["c", "a", "b", "d"]);
    }

    #[test]
    fn pinyin_schemes_reorder_candidates() {
        for ordinal in 0..=u8::MAX {
            if let Some(scheme) = SchemeType::from_u8(ordinal) {
                assert_eq!(reorders_candidates(ordinal), scheme.holds_phrase_progress());
            }
        }
        assert!(reorders_candidates(255));
    }

    #[test]
    fn only_shuangpin_corrections_stay_behind() {
        let shuangpin = SchemeType::Shuangpin as u8;
        assert!(!corrections_may_lead(shuangpin));
        for ordinal in (0..=u8::MAX).filter(|&ordinal| ordinal != shuangpin) {
            assert!(corrections_may_lead(ordinal), "{ordinal}");
        }
    }

    #[test]
    fn long_sentences_keep_four_readings_and_two_prefixes_and_can_expand_to_five_plus_one() {
        let rows = [
            row("今天天气", LATTICE_SOURCE),
            row("今天", 0),
            row("金天天气", LATTICE_SOURCE),
            row("今天添气", LATTICE_SOURCE),
            row("今添天气", LATTICE_SOURCE),
            row("金天", 0),
            row("近天天气", LATTICE_SOURCE),
            row("金", 0),
        ];
        assert_eq!(
            runner_up_order(0, &rows),
            Some(vec![0, 2, 3, 4, 1, 5, 6, 7])
        );
        let reordered: Vec<_> = [0, 2, 3, 4, 1, 5, 6, 7]
            .iter()
            .map(|&idx| rows[idx])
            .collect();
        assert_eq!(runner_up_order(0, &reordered), None);

        let expand_rows = [
            row("今天天气", LATTICE_SOURCE),
            row("今天", 0),
            row("金天天气", LATTICE_SOURCE),
            row("今天添气", LATTICE_SOURCE),
            row("今添天气", LATTICE_SOURCE),
            row("今", 0),
            row("近天天气", LATTICE_SOURCE),
            row("金", 0),
        ];
        let expand_scores = [
            Some(StaticEvidence::new(-16_000, 4)),
            None,
            Some(StaticEvidence::new(-17_000, 4)),
            Some(StaticEvidence::new(-18_000, 4)),
            Some(StaticEvidence::new(-19_000, 4)),
            None,
            Some(StaticEvidence::new(-20_000, 4)),
            None,
        ];
        let expand_scored = ScoredOrderRows::new(&expand_rows, &expand_scores);
        let expand_order = runner_up_order(0, &expand_scored).expect("应动态扩展为 5+1");
        assert_eq!(expand_order, vec![0, 2, 3, 4, 6, 1, 5, 7]);
        let r_rows: Vec<_> = expand_order.iter().map(|&i| expand_rows[i]).collect();
        let r_scores: Vec<_> = expand_order.iter().map(|&i| expand_scores[i]).collect();
        let (again, allocs) = crate::ime::personal_rerank::allocations::count(|| {
            runner_up_order(0, &ScoredOrderRows::new(&r_rows, &r_scores))
        });
        assert_eq!(again, None);
        assert_eq!(allocs, 0);
    }

    #[test]
    fn runner_up_order_recognizes_neural_and_collocation_sources_and_is_idempotent() {
        let rows = [
            row("甲乙丙", LATTICE_SOURCE),
            row("甲乙", 0),
            row("甲", 0),
            row("假乙丙", 11),
            row("加乙丙", 17),
            row("佳乙丙", 19),
            row("嘉乙丙", 20),
            row("乙", 0),
            row("丙", 0),
        ];
        let order = runner_up_order(0, &rows).expect("应重排神经/搭配次选与单字顺序");
        assert_eq!(order, vec![0, 3, 4, 5, 1, 2, 6, 7, 8]);
        let reordered: Vec<_> = order.iter().map(|&index| rows[index]).collect();
        assert_eq!(runner_up_order(0, &reordered), None);
    }

    #[test]
    fn runner_up_order_uses_answers_key_for_different_lengths_and_caps_front_non_readings_with_suffix_fallback(
    ) {
        let mut partial = row("甲乙", LATTICE_SOURCE);
        partial.answers_key = false;
        let rows = [
            row("甲乙丙丁", LATTICE_SOURCE),
            partial,
            row("词一", 0),
            row("词二", 0),
            row("词三", 0),
            row("单", 0),
            row("甲乙丙", 11),
            row("乙丙丁", 17),
            row("甲丙", 11),
        ];
        assert_eq!(
            runner_up_order(0, &rows),
            Some(vec![0, 6, 7, 8, 1, 2, 3, 4, 5])
        );

        let suffix_rows: Vec<_> = [
            row("丙丁戊", LATTICE_SOURCE),
            row("丙定戊", 11),
            row("丙", 0),
            row("丁", 0),
            row("戊", 0),
            row("己", 0),
            row("并丁戊", 17),
            row("柄丁戊", LATTICE_SOURCE),
            row("秉丁戊", 19),
        ]
        .into_iter()
        .map(|mut candidate| {
            candidate.answers_key = false;
            candidate
        })
        .collect();
        assert_eq!(
            runner_up_order(0, &suffix_rows),
            Some(vec![0, 1, 6, 7, 2, 3, 8, 4, 5])
        );
    }

    #[test]
    fn two_character_readings_keep_only_the_first() {
        let rows = [
            row("你好", LATTICE_SOURCE),
            row("倪好", LATTICE_SOURCE),
            row("你", 0),
        ];
        assert_eq!(runner_up_order(0, &rows), Some(vec![0, 2, 1]));
    }

    #[test]
    fn runner_up_order_leaves_lists_it_has_no_say_over() {
        let rows = [
            row("你好", LATTICE_SOURCE),
            row("倪好", LATTICE_SOURCE),
            row("你", 0),
        ];
        assert_eq!(runner_up_order(0, &rows[..1]), None);
        assert_eq!(runner_up_order(0, &[row("你", 0), row("尼", 0)]), None);
        assert_eq!(
            runner_up_order(0, &[row("あ", LATTICE_SOURCE), row("ア", LATTICE_SOURCE)]),
            None
        );
        assert_eq!(
            runner_up_order(0, &[row("你好", LATTICE_SOURCE), row("你", 0)]),
            None
        );
    }

    #[test]
    fn speculative_typo_sentence_defers_first_page_seat() {
        let mut typo = row("乙乙乙", LATTICE_SOURCE);
        typo.corrected = true;
        let mut prefix = row("甲甲", 0);
        prefix.answers_key = false;
        let mut single = row("甲", 0);
        single.answers_key = false;
        let full_rows = [
            row("甲甲甲", LATTICE_SOURCE),
            typo,
            prefix,
            row("甲乙甲", 11),
            row("甲甲乙", LATTICE_SOURCE),
            row("乙甲甲", LATTICE_SOURCE),
            single,
        ];
        let page_order = runner_up_order(0, &full_rows).expect("首页应优先保留四条未纠错整句");
        assert_eq!(page_order, vec![0, 3, 4, 5, 2, 6, 1]);
        let reordered: Vec<_> = page_order.iter().map(|&idx| full_rows[idx]).collect();
        let (again, allocations) =
            crate::ime::personal_rerank::allocations::count(|| runner_up_order(0, &reordered));
        assert_eq!(again, None);
        assert_eq!(allocations, 0);

        let short_list = [row("没干系", LATTICE_SOURCE), typo, row("没", 0)];
        assert_eq!(runner_up_order(0, &short_list), None);
    }

    #[test]
    fn runner_up_order_defers_far_weaker_readings_from_first_page_and_keeps_idempotence() {
        let mut prefix1 = row("许个愿望", 0);
        prefix1.answers_key = false;
        let mut prefix2 = row("许个", 0);
        prefix2.answers_key = false;
        let mut prefix3 = row("许哥", 0);
        prefix3.answers_key = false;
        let mut char1 = row("许", 0);
        char1.answers_key = false;
        let mut char2 = row("续", 0);
        char2.answers_key = false;
        let mut char3 = row("需", 0);
        char3.answers_key = false;
        let rows = [
            row("许个愿", LATTICE_SOURCE),
            prefix1,
            prefix2,
            prefix3,
            char1,
            char2,
            char3,
            row("许各院", LATTICE_SOURCE),
            row("许哥原", LATTICE_SOURCE),
        ];
        let scores = [
            Some(StaticEvidence::new(-15_000, 3)),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(StaticEvidence::new(-30_500, 3)),
            Some(StaticEvidence::new(-30_400, 3)),
        ];
        let scored = ScoredOrderRows::new(&rows, &scores);
        let order = runner_up_order(0, &scored).expect("悬殊落后的拼凑次选不应留在首屏");
        assert_eq!(order, vec![0, 1, 2, 3, 4, 5, 7, 8, 6]);

        let reordered: Vec<_> = order.iter().map(|&idx| rows[idx]).collect();
        let reordered_scores: Vec<_> = order.iter().map(|&idx| scores[idx]).collect();
        let reordered_scored = ScoredOrderRows::new(&reordered, &reordered_scores);
        let (again, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            runner_up_order(0, &reordered_scored)
        });
        assert_eq!(again, None);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn neural_v2_order_and_gate_work_cleanly() {
        assert_eq!(
            neural_v2_fused_order(
                &[-15.0, -8.0, -10.0],
                &[-12.0, -13.0, -11.0],
                &[0.0, 0.0, -2.5],
                1.0,
                0.25
            ),
            Some(vec![1, 2, 0])
        );
        assert_eq!(
            neural_v2_fused_order(&[-5.0, -9.0], &[-6.0, -7.0], &[0.0, 0.0], 1.0, 0.25),
            None
        );

        let dict_led = [row("格式", 0), row("个事", LATTICE_SOURCE)];
        assert!(rerank_needs_model_v2(0, false, &dict_led));
        let english_led = [row("hello", ENGLISH_SOURCE), row("黑楼", LATTICE_SOURCE)];
        assert!(!rerank_needs_model_v2(0, false, &english_led));
        let fallback_led = [row("Cla", FALLBACK_SOURCE), row("C啦", LATTICE_SOURCE)];
        assert!(!rerank_needs_model_v2(0, false, &fallback_led));
    }

    #[test]
    fn runner_up_order_defers_two_char_split_fallbacks_when_dictionary_hits_lead() {
        let mut longer1 = row("处罚的", 0);
        longer1.answers_key = false;
        let mut longer2 = row("出发点", 0);
        longer2.answers_key = false;
        let mut single = row("出", 0);
        single.answers_key = false;
        let rows = [
            row("出发", 0),
            row("处罚", 0),
            row("触发", 0),
            row("楚伐", LATTICE_SOURCE),
            longer1,
            longer2,
            row("除法", 0),
            single,
        ];
        let scores = [
            None,
            None,
            None,
            Some(StaticEvidence::new(-21_000, 2)),
            None,
            None,
            None,
            None,
        ];
        let scored = ScoredOrderRows::new(&rows, &scores);
        let order = runner_up_order(0, &scored).expect("双字极低频 1+1 拼凑次选应让出首屏");
        assert_eq!(order, vec![0, 1, 2, 4, 5, 6, 3, 7]);

        let reordered: Vec<_> = order.iter().map(|&idx| rows[idx]).collect();
        let reordered_scores: Vec<_> = order.iter().map(|&idx| scores[idx]).collect();
        let reordered_scored = ScoredOrderRows::new(&reordered, &reordered_scores);
        let (again, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            runner_up_order(0, &reordered_scored)
        });
        assert_eq!(again, None);
        assert_eq!(allocations, 0);
    }
}
