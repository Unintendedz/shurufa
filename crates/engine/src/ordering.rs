//! 候选排序的唯一实现：句子模型重排、整句次选读法的后移，以及交给重排器的上文窗口。
//!
//! 这里只做决策，不碰候选数组：`rerank_pick` 返回应移到首位的下标，`runner_up_order` 返回一个排列（新座位 -> 旧座位）。`msime-input-runtime` 把结果同步应用到候选元数据和引擎座位映射上，网页引擎（`msime-engine-wasm`）也调用同一组函数，两处的排序因此不会各自漂移。

use crate::SchemeType;

pub use chinese_ime_lm::{CandidateFacts, Reranker, SentenceModel, DICTIONARY_SOURCES};

/// `CandidateSource::Generated`：词网格解码出的整句路径。
pub const LATTICE_SOURCE: u8 = 8;

/// 会产生回答完整按键的整句或长词读法的来源集合：`Generated`、`NeuralKeyboard`、`Collocation`。
const SENTENCE_READING_SOURCES: [u8; 3] = [LATTICE_SOURCE, 11, 17];

/// `CandidateSource::QuickPhrase`, `Emoji` and `Kaomoji`: entries a keyword finds in a catalog, listed in the catalog's order. They are not readings of the key, so the reranker neither compares them nor promotes them: typing `kiss` in kaomoji mode had the model seat the catalog's 646th entry, `French Kiss!(*￣(￣　*)`, above its first.
const CATALOG_SOURCES: [u8; 3] = [5, 6, 7];

/// `CandidateSource::AiSuggestion` 和已经不再产生的占位行 `AiPending`：在线座位规则（`normalize_online_slots`）总把 AI 行放回第一页最后一格和列表末尾，模型提它们也没用；更要紧的是，若让 AI 候选参与比较，它一到就可能抢走本来要提到首位的本地候选，回答到达时本地候选的次序就变了（#6888）。
const AI_SEAT_SOURCES: [u8; 2] = [3, 12];

/// `CandidateSource::EnglishDictionary`。混输英文只在拼音拆不开、英文完全匹配（或用户固定在第一位）时才由引擎放到首位，那是引擎有意的安排；句子模型读的是中文，不能因为英文行不算词典命中就把中文行转到它前面。这道检查不分方案和模式：只有英文行的列表（比如专用英文模式、临时英文）首行也是英文，同样不重排，这是有意的：模型读的是中文，比较英文行没有意义。
const ENGLISH_SOURCE: u8 = 4;

/// `CandidateSource::Fallback`：大写字面输入、特殊符号字面行及无匹配兜底原文。它居首时是引擎保留原样输入的安排，也不参与中文句子模型评分。
const FALLBACK_SOURCE: u8 = 9;

/// `CandidateSource::MixedJapanese`：日文混输行。它领先时是引擎判定拼音给不出整段的词，和领先的英文一样不重排；它在别的位置时也不让模型拿它比较、把它提到首位，句子模型读的是中文。
const MIXED_JAPANESE_SOURCE: u8 = 13;

/// 候选列表通常不超过一页；短列表的文本引用放在栈上，较大列表继续走堆缓冲。
const SMALL_RERANK_TEXTS: usize = 128;

/// 逐键静态证据分支只比较前九条真正回答按键的候选，守住逐键时延预算。
const STATIC_RERANK_LIMIT: usize = 9;

/// 停顿后精排在后台线程运行，放宽至十六条回答当前按键的候选，覆盖完整十二条次选与隔音备选。
const SETTLED_STATIC_RERANK_LIMIT: usize = 16;

/// 短词组与完整子句的分界字数：5 字以内（单条词或双词短语）词内上文短、字级神经模型方差大，
/// 严格执行静态先验护栏与首选防抖门槛；6 字及以上构成完整子句，句内远距语义约束充分且单点同音差值被字数稀释，
/// 放宽免费赤字窗口至极端崩塌线并取消按字倍增的首选门槛。
pub(crate) const FULL_CLAUSE_MIN_CHARACTERS: usize = 6;
pub(crate) const STATIC_PRIOR_FREE_DEFICIT_SHORT_MEAN_NATS: f64 = 0.50;
pub(crate) const STATIC_PRIOR_FREE_DEFICIT_CLAUSE_MEAN_NATS: f64 = 2.00;
pub(crate) const STATIC_PRIOR_EXCESS_PENALTY: f64 = 0.50;
pub(crate) const STATIC_LEADER_PROMOTE_MARGIN_SHORT_MEAN_NATS: f64 = 0.06;

pub(crate) fn static_prior_free_deficit_mean_nats(characters: usize) -> f64 {
    if characters >= FULL_CLAUSE_MIN_CHARACTERS {
        STATIC_PRIOR_FREE_DEFICIT_CLAUSE_MEAN_NATS
    } else {
        STATIC_PRIOR_FREE_DEFICIT_SHORT_MEAN_NATS
    }
}

pub(crate) fn static_leader_promote_margin_mean_nats(total_clause_characters: usize) -> f64 {
    if total_clause_characters >= FULL_CLAUSE_MIN_CHARACTERS {
        0.0
    } else {
        STATIC_LEADER_PROMOTE_MARGIN_SHORT_MEAN_NATS
    }
}

fn fill_guarded_combined_scores(
    texts: &[&str],
    statik: &[f64],
    neural: &[f32],
    combined: &mut [f64],
) {
    let max_statik = statik.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for index in 0..statik.len() {
        let free_deficit = static_prior_free_deficit_mean_nats(texts[index].chars().count());
        let deficit = (max_statik - statik[index]).max(0.0);
        let excess = (deficit - free_deficit).max(0.0);
        combined[index] =
            (statik[index] + f64::from(neural[index])) * 0.5 - STATIC_PRIOR_EXCESS_PENALTY * excess;
    }
}

fn leader_promotion_allowed(
    total_clause_characters: usize,
    leader_statik: f64,
    challenger_statik: f64,
    leader_combined: f64,
    challenger_combined: f64,
) -> bool {
    let margin = static_leader_promote_margin_mean_nats(total_clause_characters);
    challenger_statik >= leader_statik || challenger_combined - leader_combined >= margin
}

/// The traits of the scheme behind `scheme`, which the Engine reports as its `SchemeType` ordinal. Only the placeholder snapshot of a failed refresh carries an ordinal no scheme has; each caller decides what that placeholder means, the way the ordinal comparisons this replaces did.
fn scheme_type(scheme: u8) -> Option<SchemeType> {
    SchemeType::from_u8(scheme)
}

/// Whether the runtime may reorder the scheme's candidate list (the sentence model and the runner-up demotion). A scheme whose selection goes straight to the document instead of being held as phrase progress (the Korean Hanja list) lists its table in frequency order, not readings of one sentence the model can compare, and that order is the one to keep.
pub fn reorders_candidates(scheme: u8) -> bool {
    scheme_type(scheme).is_none_or(SchemeType::holds_phrase_progress)
}

/// 方案的纠错行能不能被重排器提到首位。双拼的纠错整句按产品取舍一律不抢首选（#6034，引擎把它排在整句块之后），所以重排时它既不参与比较，也不让整张表失去词典命中的豁免；全拼的纠错行本来就允许领先（`typo_lead_margin`），照旧。
fn corrections_may_lead(scheme: u8) -> bool {
    scheme_type(scheme) != Some(SchemeType::Shuangpin)
}

/// 原地应用排列，`order` 将每个新座位映射到旧座位。
///
/// 按循环交换候选所有权，不复制元素。记录已访问座位，让每条循环只遍历一次；
/// 短列表用栈缓冲，大列表才申请辅助缓冲，恒等排列直接返回。
pub fn apply_order<T>(items: &mut [T], order: &[usize]) {
    debug_assert_eq!(items.len(), order.len());
    apply_order_with(order, |left, right| items.swap(left, right));
}

/// 按排列的循环调用交换操作，让调用方同步移动多个并行数组。
///
/// `order` 必须是新座位到旧座位的合法排列；交换操作涉及的数组长度必须与它一致。
/// 每条循环只遍历一次，共用访问标记缓冲，恒等排列不调用交换操作。
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

/// Keep the Engine-index mapping empty while the cached candidates are still in Engine order.
/// Reordering paths call this immediately before their first permutation, so ordinary keystrokes
/// avoid rebuilding an identity vector on every snapshot.
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

/// How far the context handed to a reranker moves at a time once it no longer fits the model.
pub const RERANK_CONTEXT_STEP: usize = 16;

/// The tail of the committed text a reranker should see, trimmed so the window holds still while a candidate grows.
///
/// The reranker keeps the model state for its prefix across keystrokes, keyed on the prefix tokens, and that cache is what keeps a keystroke inside a frame. It trims the context itself to leave room for the longest candidate, so left to do that, a context longer than the window slides by one character every time a candidate gains one — which is most keystrokes that complete a syllable. Every slide is a different prefix, so it reran the prefix and dropped every resume point with it, and a keystroke cost 20-45ms instead of about 1ms. Nothing showed it in a short test: the context only outgrows the window after a few sentences in one application, which is when "typing falls behind" was reported.
///
/// Trimming here, in steps, keeps the prefix identical until the longest candidate crosses a step, and the reranker then finds nothing further to trim because everything handed over already fits. A context that fits whole is handed over unchanged, so short contexts rank exactly as before.
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

/// 词网格的自然对数总分，去掉共同的音节奖励；保留整数毫 nat 与字数避免快照丢失精度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticEvidence {
    pub log_prob_milli_nats: i64,
    pub characters: usize,
    /// 引擎逐键 4.5MB 模型已对同一路径与同一上文算出的 `(模型实例标识, 逐字均值毫 nat)`。
    /// `rerank_pick` 在全部参与比较的候选均命中同一模型实例时直接复用该分值，消除单键双跑 4.5MB 推理。
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

/// 排序决策读取的候选行来源。连续的 `OrderRow` 数组和快照里的并行数组都可以实现它。
pub trait OrderRowSource<'a> {
    fn len(&self) -> usize;
    fn get(&self, index: usize) -> OrderRow<'a>;

    /// 没有静态证据的调用方继续使用原模型决策，不从来源或行位置猜一个分数。
    fn static_evidence(&self, _index: usize) -> Option<StaticEvidence> {
        None
    }

    /// 这一行是引擎键盘模型排在它选中那条之后的整句路径（`WordItem::keyboard_runner_up`）。
    /// 逐键重排的调用方用它把这些行排除出比较；停顿后精排不传，照常比较。
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

/// 找出实际交给模型比较的候选中最长的一行。
///
/// 比较集合之后的长候选不能影响上下文窗口，否则会让模型在无关长行出现时重建前缀。
fn longest_rerank_candidate<'a, const LIMIT: usize, R, F>(rows: &R, compared: &F) -> usize
where
    R: OrderRowSource<'a> + ?Sized,
    F: Fn(usize) -> bool,
{
    (0..rows.len())
        .filter(|index| compared(*index))
        .take(LIMIT)
        .map(|index| rows.get(index).text.chars().count())
        .max()
        .unwrap_or(0)
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

    /// 长度和候选不一致时整份作废，按每行都不是键盘模型的次选处理：多比较几行只多花时间，不会排错。
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

/// 首行已是未纠错整句读法时，后排带 `corrected` 的整句读法是词网格附带的音节纠错句（`typo_lead_margin = INFINITY`），
/// 不参与模型重排抢占字面整句席位；切分层整体纠错（首行本身带 `corrected`）或词典行居首的场景不受影响。
fn speculative_typo_sentence(leader: OrderRow<'_>, row: OrderRow<'_>) -> bool {
    !leader.corrected
        && SENTENCE_READING_SOURCES.contains(&leader.source)
        && row.corrected
        && SENTENCE_READING_SOURCES.contains(&row.source)
}

fn any_corrected_key<'a, R: OrderRowSource<'a> + ?Sized>(scheme: u8, rows: &'a R) -> bool {
    corrections_may_lead(scheme) && (0..rows.len()).any(|index| rows.get(index).corrected)
}

/// 判断 `rerank_pick` 是否需要运行模型，提前拒绝时宿主无需把停顿模型移出会话线程。
///
/// 韩文汉字候选按单音节词频排表，不能用中文模型评分。五笔码表回答的列表先排表码行，
/// 再追加拼音纠错行；若让纠错规则撤销表码首选的豁免，就会把 `dyn` 的「态」挤到
/// `dynn` 的「太快」之后。只有完全由拼音兜底回答的五笔列表才按拼音重排。
pub fn rerank_needs_model<'a, R: OrderRowSource<'a> + ?Sized>(
    scheme: u8,
    answered_by_pinyin_fallback: bool,
    rows: &'a R,
) -> bool {
    if !reorders_candidates(scheme) {
        return false;
    }
    if scheme_type(scheme) == Some(SchemeType::Wubi) && !answered_by_pinyin_fallback {
        return false;
    }
    if rows.len() < 2 {
        return false;
    }
    let leader = rows.get(0);
    // 引擎让英文或日文混输行领先的列表原样保留，见 `ENGLISH_SOURCE`、`MIXED_JAPANESE_SOURCE`。
    if matches!(leader.source, ENGLISH_SOURCE | MIXED_JAPANESE_SOURCE) {
        return false;
    }
    // 首行不回答当前按键时，上游同样拒绝比较，无需准备模型输入。
    if !row_answers_key(scheme, leader) {
        return false;
    }
    // 词典命中携带当前按键的词频；全拼出现任何纠错行时，这份依据不再可信，
    // 整表撤销豁免。双拼纠错行不许领先，也不撤销词典首选的豁免。
    let corrected_key = any_corrected_key(scheme, rows);
    let trusted_leader = DICTIONARY_SOURCES.contains(&leader.source) && !corrected_key;
    !trusted_leader
}

/// rerank() 的决策部分：返回应当移到首位的行下标；None 表示不动。
///
/// `scheme` 是 `SchemeType` 序号；不允许重排的方案（韩文汉字表）返回 None，五笔且 `!answered_by_pinyin_fallback` 时返回 None，行数 < 2 时返回 None，首行是英文时返回 None。`Reranker::best_where` 是 `&mut self`，因此这里取 `&mut`。
pub fn rerank_pick<'a, R: OrderRowSource<'a> + ?Sized>(
    reranker: &mut Reranker,
    context: &str,
    scheme: u8,
    answered_by_pinyin_fallback: bool,
    rows: &'a R,
) -> Option<usize> {
    // 键盘模型的次选不会领先；真领先时没有可与之比较的首行，按拒绝比较处理。
    if !rerank_needs_model(scheme, answered_by_pinyin_fallback, rows) || rows.keyboard_runner_up(0)
    {
        return None;
    }
    let leader = rows.get(0);
    // 键盘模型的次选刚被同一个模型在同一上文下排在它选中那条之后，逐键重排不再给它们打分：
    // 它们都回答整个按键串，不排除就会占满比较集合，每键多给几条长句打分，结果却不变。
    let compared = |index: usize| {
        let row = rows.get(index);
        row_answers_key(scheme, row)
            && !speculative_typo_sentence(leader, row)
            && !rows.keyboard_runner_up(index)
    };
    let corrected_key = any_corrected_key(scheme, rows);
    // 只有参与比较的候选需要占用模型上下文窗口；剥离末尾非汉字前缀以免 `<unk>` 污染语境。
    let longest = longest_rerank_candidate::<STATIC_RERANK_LIMIT, _, _>(rows, &compared);
    let context = rerank_context(
        crate::text::trailing_han_clause(context),
        reranker.model().context_length(),
        longest,
    );
    if rows.static_evidence(0).is_some() {
        let mut considered = [0; STATIC_RERANK_LIMIT];
        let mut statik = [0.0; STATIC_RERANK_LIMIT];
        let mut subset = [""; STATIC_RERANK_LIMIT];
        if let Some(count) = fill_static_rerank_inputs::<STATIC_RERANK_LIMIT, _, _>(
            rows,
            compared,
            &mut considered,
            &mut statik,
            &mut subset,
        ) {
            // 若全部参与比较的候选均已由引擎同源 4.5MB 模型在同一上文窗口下算出逐字分，
            // 直接复用缓存分值，省去本轮重复的 4.5MB Transformer 前向计算。
            let model_id = std::ptr::from_ref(reranker.model()) as usize;
            let mut cached_neural = [0.0f32; STATIC_RERANK_LIMIT];
            let mut all_cached = true;
            for index in 0..count {
                match rows
                    .static_evidence(considered[index])
                    .and_then(|ev| ev.keyboard_neural)
                {
                    Some((id, mean_milli_nats)) if id == model_id => {
                        cached_neural[index] = mean_milli_nats as f32 / 1000.0;
                    }
                    _ => {
                        all_cached = false;
                        break;
                    }
                }
            }
            let owned_neural;
            let neural: &[f32] = if all_cached {
                &cached_neural[..count]
            } else {
                owned_neural = reranker.log_probabilities(context, &subset[..count]);
                &owned_neural
            };
            if neural.len() == count {
                // 模型逐字分与词网格逐字分等权混合，并叠加静态先验赤字护栏与低频翻盘阈值。
                let mut combined = [0.0; STATIC_RERANK_LIMIT];
                fill_guarded_combined_scores(
                    &subset[..count],
                    &statik[..count],
                    neural,
                    &mut combined[..count],
                );
                let mut best = 0;
                for index in 1..count {
                    if combined[index] > combined[best] {
                        best = index;
                    }
                }
                let promoted = best != 0
                    && leader_promotion_allowed(
                        context
                            .chars()
                            .count()
                            .saturating_add(subset[0].chars().count()),
                        statik[0],
                        statik[best],
                        combined[0],
                        combined[best],
                    );
                return promoted.then(|| considered[best]);
            }
        }
    }
    with_row_texts(rows, |texts| {
        reranker.best_where(context, texts, |index| CandidateFacts {
            answers_key: compared(index),
            trusted_dictionary_hit: DICTIONARY_SOURCES.contains(&rows.get(index).source)
                && !corrected_key,
        })
    })
}

/// 停顿后精排的整体重排决策：返回新座位到旧座位的排列；无需改动时返回 `None`。
///
/// 停顿阶段不在按键热路径上，因此静态证据分支比较前 16 条回答当前按键的候选，
/// 并按 `0.5 * (static + neural)` 的完整名次原地重排比较集合，而不只旋转第 1 名，
/// 让精排模型排出的第 2、3 名整句也能顺位进入首页。无静态证据时退回 `best_where`。
pub fn settled_rerank_order<'a, R: OrderRowSource<'a> + ?Sized>(
    reranker: &mut Reranker,
    context: &str,
    scheme: u8,
    answered_by_pinyin_fallback: bool,
    rows: &'a R,
) -> Option<Vec<usize>> {
    settled_rerank_order_with_options(
        reranker,
        context,
        scheme,
        answered_by_pinyin_fallback,
        rows,
        false,
    )
}

/// 停顿后精排的可选策略版本：`lock_leader = true` 时锁定首选槽位（`order[0] == 0` 恒成立），
/// 仅对第 2 名及以后的次选整句按停顿大模型排序，彻底消除盲打时空格首选延迟翻脸。
pub fn settled_rerank_order_with_options<'a, R: OrderRowSource<'a> + ?Sized>(
    reranker: &mut Reranker,
    context: &str,
    scheme: u8,
    answered_by_pinyin_fallback: bool,
    rows: &'a R,
    lock_leader: bool,
) -> Option<Vec<usize>> {
    if !rerank_needs_model(scheme, answered_by_pinyin_fallback, rows) {
        return None;
    }
    let leader = rows.get(0);
    let compared = |index: usize| {
        let row = rows.get(index);
        row_answers_key(scheme, row) && !speculative_typo_sentence(leader, row)
    };
    let count = rows.len();
    if rows.static_evidence(0).is_some() {
        let longest =
            longest_rerank_candidate::<SETTLED_STATIC_RERANK_LIMIT, _, _>(rows, &compared);
        let context = rerank_context(
            crate::text::trailing_han_clause(context),
            reranker.model().context_length(),
            longest,
        );
        let mut considered = [0; SETTLED_STATIC_RERANK_LIMIT];
        let mut statik = [0.0; SETTLED_STATIC_RERANK_LIMIT];
        let mut subset = [""; SETTLED_STATIC_RERANK_LIMIT];
        if let Some(compared_count) = fill_static_rerank_inputs::<SETTLED_STATIC_RERANK_LIMIT, _, _>(
            rows,
            compared,
            &mut considered,
            &mut statik,
            &mut subset,
        ) {
            if lock_leader && compared_count < 3 {
                return None;
            }
            let neural = reranker.log_probabilities(context, &subset[..compared_count]);
            if neural.len() == compared_count {
                let mut combined = [0.0f64; SETTLED_STATIC_RERANK_LIMIT];
                let mut rank = [0usize; SETTLED_STATIC_RERANK_LIMIT];
                fill_guarded_combined_scores(
                    &subset[..compared_count],
                    &statik[..compared_count],
                    &neural,
                    &mut combined[..compared_count],
                );
                for (seat, entry) in rank[..compared_count].iter_mut().enumerate() {
                    *entry = seat;
                }
                if lock_leader {
                    // 锁定第 1 槽：仅重排第 2 名及以后的次选，保证停顿后空格首选零翻脸。
                    rank[1..compared_count].sort_by(|&a, &b| combined[b].total_cmp(&combined[a]));
                } else {
                    // 降序排列，平分时保持比较集合内的原有先后次序。
                    rank[..compared_count].sort_by(|&a, &b| combined[b].total_cmp(&combined[a]));
                    if rank[0] != 0
                        && !leader_promotion_allowed(
                            context
                                .chars()
                                .count()
                                .saturating_add(subset[0].chars().count()),
                            statik[0],
                            statik[rank[0]],
                            combined[0],
                            combined[rank[0]],
                        )
                    {
                        if let Some(leader_seat) =
                            rank[..compared_count].iter().position(|&idx| idx == 0)
                        {
                            rank[..=leader_seat].rotate_right(1);
                        }
                    }
                }
                if (0..compared_count).all(|seat| rank[seat] == seat) {
                    return None;
                }
                let mut order: Vec<usize> = (0..count).collect();
                for seat in 0..compared_count {
                    order[considered[seat]] = considered[rank[seat]];
                }
                return Some(order);
            }
        }
    }
    if lock_leader {
        // 无静态证据分支在锁定首选时，仍用大模型对第 2 名及以后的可比候选排序，不触碰第 0 位。
        let mut considered = [0usize; SETTLED_STATIC_RERANK_LIMIT];
        let mut subset = [""; SETTLED_STATIC_RERANK_LIMIT];
        let mut compared_count = 0usize;
        for index in 0..count {
            if !compared(index) {
                continue;
            }
            if compared_count == SETTLED_STATIC_RERANK_LIMIT {
                break;
            }
            considered[compared_count] = index;
            subset[compared_count] = rows.get(index).text;
            compared_count += 1;
        }
        if compared_count < 3 {
            return None;
        }
        let longest = subset[1..compared_count]
            .iter()
            .map(|text| text.chars().count())
            .max()
            .unwrap_or(0);
        let context = rerank_context(
            crate::text::trailing_han_clause(context),
            reranker.model().context_length(),
            longest,
        );
        let neural = reranker.log_probabilities(context, &subset[1..compared_count]);
        if neural.len() != compared_count - 1 {
            return None;
        }
        let mut rank = [0usize; SETTLED_STATIC_RERANK_LIMIT];
        for (seat, entry) in rank[..compared_count].iter_mut().enumerate() {
            *entry = seat;
        }
        rank[1..compared_count].sort_by(|&a, &b| neural[b - 1].total_cmp(&neural[a - 1]));
        if (0..compared_count).all(|seat| rank[seat] == seat) {
            return None;
        }
        let mut order: Vec<usize> = (0..count).collect();
        for seat in 1..compared_count {
            order[considered[seat]] = considered[rank[seat]];
        }
        return Some(order);
    }
    let longest = longest_rerank_candidate::<STATIC_RERANK_LIMIT, _, _>(rows, &compared);
    let context = rerank_context(
        crate::text::trailing_han_clause(context),
        reranker.model().context_length(),
        longest,
    );
    let corrected_key = any_corrected_key(scheme, rows);
    let promote = with_row_texts(rows, |texts| {
        reranker.best_where(context, texts, |index| CandidateFacts {
            answers_key: compared(index),
            trusted_dictionary_hit: DICTIONARY_SOURCES.contains(&rows.get(index).source)
                && !corrected_key,
        })
    })?;
    if promote == 0 || promote >= count {
        return None;
    }
    let mut order: Vec<usize> = (0..count).collect();
    rotate_to_front(&mut order, promote);
    Some(order)
}

fn fill_static_rerank_inputs<'a, const LIMIT: usize, R, F>(
    rows: &'a R,
    compared: F,
    considered: &mut [usize; LIMIT],
    statik: &mut [f64; LIMIT],
    subset: &mut [&'a str; LIMIT],
) -> Option<usize>
where
    R: OrderRowSource<'a> + ?Sized,
    F: Fn(usize) -> bool,
{
    let mut count = 0;
    for index in 0..rows.len() {
        if !compared(index) {
            continue;
        }
        let row = rows.get(index);
        if count == LIMIT {
            break;
        }
        considered[count] = index;
        statik[count] = rows.static_evidence(index)?.mean()?;
        subset[count] = row.text;
        count += 1;
    }
    (count >= 2).then_some(count)
}

fn with_row_texts<'a, R, T>(rows: &'a R, operation: impl FnOnce(&[&'a str]) -> T) -> T
where
    R: OrderRowSource<'a> + ?Sized,
{
    if rows.len() <= SMALL_RERANK_TEXTS {
        let mut texts = [""; SMALL_RERANK_TEXTS];
        for (index, text) in texts.iter_mut().enumerate().take(rows.len()) {
            *text = rows.get(index).text;
        }
        operation(&texts[..rows.len()])
    } else {
        let mut texts = Vec::with_capacity(rows.len());
        texts.extend((0..rows.len()).map(|index| rows.get(index).text));
        operation(&texts)
    }
}

/// demote_runner_up_readings() 的决策部分：返回 order（新座位 -> 旧座位）；恒等排列时返回 None。
///
/// Keep the leading sentence readings together near the top and move the rest of them behind the list.
///
/// The lattice searches several readings of the whole key so that something can choose between them. Leaving all of them at the front fills the candidate page with near-duplicate sentences and pushes the short candidates a user actually wants off it, which is why the search used to be pinned to a single path.
///
/// For a sentence of three or more characters the first page keeps three readings, seated together right after the first one, and only the rest are moved back. One reading was too few once the Google fallback stopped holding a second sentence seat: on sentences-neutral-v1 top5 fell to 0.615 and on sentences-v2 to 0.269 with the correct sentence sitting at reading two or three, and keeping three lifts them to 0.839 and 0.763 while quanpin-words-v1 top5 moves only from 0.940 to 0.938. Shorter readings still keep one, because two-syllable keys are where the runner-ups (倪好, 你号, 你毫 after 你好) push dictionary words off the page, and keeping three there costs words top5 two points.
///
/// They are moved rather than removed. Deleting them threw away the model's later choices, so a reading the model ranked fourth was unreachable even when it was right.
///
/// Only lattice readings are touched. An earlier version of this keyed on "any source that is not a dictionary", which is wrong twice over: a source number says which code produced a candidate, not that two candidates are spellings of one answer, and most of the other sources are plural by design — English words, emoji, kaomoji, quick phrases and AI suggestions all arrive as lists, and that version silently dropped all but one of each.
/// 对三字短词或离合词（`width == LONG_SENTENCE_CHARACTERS`，恰在两字 `keep = 1` 之上），当次选整句的每字
/// 静态证据落后首条读法超过此阈值（nats/字）时，说明首条已是完整词汇级跨度而次选属于 `[1, 2]` / `[2, 1]`
/// 拼凑切分（如「许个愿」对「许各院 / 许哥原」差 `> 3.8` nats/字），不再占用首屏读法席位，改排在第 2 页次选区
/// 开头；四字及以上短语与长句（合法 `[1, 1, 2]` 组合常与 `[2, 2]` 同音词竞争）保留全部 3 个首屏读法席位。
const RUNNER_UP_MAX_MEAN_GAP_NATS: f64 = 3.8;

#[allow(clippy::too_many_arguments)]
fn visit_runner_up_order(
    count: usize,
    first_reading: usize,
    keep: usize,
    front_limit: usize,
    expand_front_with_unused_keep: bool,
    is_reading: &impl Fn(usize) -> bool,
    is_deferred_reading: &impl Fn(usize) -> bool,
    is_far_below_leader: &impl Fn(usize) -> bool,
    mut emit: impl FnMut(usize),
) {
    let primary_reading = |index: usize| is_reading(index) && !is_deferred_reading(index);
    let deferred_reading = |index: usize| is_reading(index) && is_deferred_reading(index);
    // 1. 首条全键读法之前的候选保持原位（如领先的精确词典命中）。
    for index in 0..first_reading {
        emit(index);
    }
    // 2. 首页保留的前 `keep` 条全键读法：排除静态分悬殊落后的拼凑句，优先排未纠错字面读法，名额未满再补纠错整句。
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
    // 3. 留在剩余次选读法之前的首页前排非读法行（三字以上长串把未用的读法名额补给前排非读法行以凑满首屏）。
    let effective_front_limit = if expand_front_with_unused_keep {
        front_limit + keep.saturating_sub(kept_total)
    } else {
        front_limit
    };
    let mut kept_front = 0;
    if effective_front_limit > 0 {
        for index in (first_reading + 1)..count {
            if !is_reading(index) {
                emit(index);
                kept_front += 1;
                if kept_front == effective_front_limit {
                    break;
                }
            }
        }
    }
    // 4. 其余全键次选读法紧随首页前排之后（先近分未纠错次选、再悬殊未纠错次选、再纠错整句），排在所有单字与余下非读法行之前。
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
    // 5. 其余非读法行（超出前排限额的前缀词与单字）按原相对顺序收尾。
    let mut skipped_front = 0;
    for index in (first_reading + 1)..count {
        if is_reading(index) {
            continue;
        }
        if skipped_front < effective_front_limit {
            skipped_front += 1;
            continue;
        }
        emit(index);
    }
}

pub fn runner_up_order<'a, R: OrderRowSource<'a> + ?Sized>(
    scheme: u8,
    rows: &R,
) -> Option<Vec<usize>> {
    // 词网格从两音节开始解码；假名单字等长度不足的行不按整句读法归组。
    const SENTENCE_SYLLABLES: usize = 2;
    // 达到三个汉字时在首页保留 `SENTENCE_READINGS` 条整句读法与至多 `FRONT_NON_READING_LIMIT` 条前排非读法行（满 6 格首屏），其余全键读法紧随第 2 页开头。
    const LONG_SENTENCE_CHARACTERS: usize = 3;
    const SENTENCE_READINGS: usize = 3;
    const FRONT_NON_READING_LIMIT: usize = 3;
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
    // 光标后编辑等模式整表 `answers_key` 为假，此时退回按同字数归组整句读法。
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
    let keep = if width >= LONG_SENTENCE_CHARACTERS {
        SENTENCE_READINGS
    } else {
        1
    };
    let front_limit = if width >= LONG_SENTENCE_CHARACTERS {
        FRONT_NON_READING_LIMIT
    } else {
        SHORT_FRONT_ROW_LIMIT
    };
    let expand_front_with_unused_keep = width >= LONG_SENTENCE_CHARACTERS;

    let defer_corrected = !rows.get(first_reading).corrected;
    let is_deferred_reading = |index: usize| defer_corrected && rows.get(index).corrected;
    let leader_mean = (width == LONG_SENTENCE_CHARACTERS)
        .then(|| {
            rows.static_evidence(first_reading)
                .and_then(StaticEvidence::mean)
        })
        .flatten();
    let far_below_leader = |index: usize| {
        index != first_reading
            && match (
                leader_mean,
                rows.static_evidence(index).and_then(StaticEvidence::mean),
            ) {
                (Some(leader), Some(candidate)) => leader - candidate > RUNNER_UP_MAX_MEAN_GAP_NATS,
                _ => false,
            }
    };

    // 先做零分配线性扫描检查是否已处于目标顺序；仅确实需要重排时才构造返回向量。
    let mut output = 0;
    let mut identity = true;
    visit_runner_up_order(
        count,
        first_reading,
        keep,
        front_limit,
        expand_front_with_unused_keep,
        &is_reading,
        &is_deferred_reading,
        &far_below_leader,
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
        &is_reading,
        &is_deferred_reading,
        &far_below_leader,
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
        // Short contexts are what the sentence eval measured, so they must reach the model untouched.
        assert_eq!(rerank_context("你好世界", 64, 10), "你好世界");
        assert_eq!(rerank_context("", 64, 10), "");
    }

    #[test]
    fn rerank_context_holds_still_while_a_candidate_grows() {
        // A long context used to slide by one character per keystroke, which made the reranker rerun its prefix every time.
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
            // Always the most recent text, never the start of it.
            assert!(context.ends_with(window));
        }
    }

    #[test]
    fn rerank_context_cuts_on_a_character_boundary_and_can_empty() {
        let context = "a中b文".repeat(40);
        let window = rerank_context(&context, 64, 20);
        assert_eq!(window.chars().count(), 32);
        assert!(context.ends_with(window));
        // A candidate that fills the window leaves no room, and the answer is an empty context rather than a panic.
        assert_eq!(rerank_context(&context, 64, 70), "");
    }

    #[test]
    fn rerank_context_length_ignores_candidates_beyond_model_budget() {
        let rows = [
            row("甲", LATTICE_SOURCE),
            row("乙", LATTICE_SOURCE),
            row("丙", LATTICE_SOURCE),
            row("丁", LATTICE_SOURCE),
            row("戊", LATTICE_SOURCE),
            row("己", LATTICE_SOURCE),
            row("庚", LATTICE_SOURCE),
            row("辛", LATTICE_SOURCE),
            row("壬", LATTICE_SOURCE),
            row("这是没有进入模型比较集合的长候选", LATTICE_SOURCE),
        ];
        assert_eq!(
            longest_rerank_candidate::<STATIC_RERANK_LIMIT, _, _>(&rows, &|index| rows[index]
                .answers_key),
            1
        );
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
    #[ignore = "长循环的配对计时，需在空闲主机上显式运行"]
    fn long_candidate_permutations_have_linear_scaling() {
        use std::hint::black_box;
        use std::time::Instant;

        fn elapsed(count: usize) -> u128 {
            let order: Vec<usize> = std::iter::once(0)
                .chain(2..count)
                .chain(std::iter::once(1))
                .collect();
            let mut values: Vec<usize> = (0..count).collect();
            apply_order(&mut values, &order);
            assert_eq!(values, order);
            let begin = Instant::now();
            for _ in 0..32768 / count {
                apply_order(black_box(&mut values), black_box(&order));
            }
            black_box(values);
            begin.elapsed().as_nanos()
        }
        // 两种长度处理相同总行数；交替先后顺序取中位数，预算留出主机噪声。
        let mut small = Vec::new();
        let mut large = Vec::new();
        for round in 0..5 {
            if round % 2 == 0 {
                small.push(elapsed(128));
                large.push(elapsed(8192));
            } else {
                large.push(elapsed(8192));
                small.push(elapsed(128));
            }
        }
        small.sort_unstable();
        large.sort_unstable();
        eprintln!(
            "permutation_equal_rows_small_ns={} large_ns={}",
            small[2], large[2]
        );
        assert!(
            large[2] <= small[2] * 16,
            "同等总行数的长排列成本过高: {large:?} / {small:?}"
        );
    }

    /// Only `CandidateSource::Generated` names alternative readings of one key. Every other source
    /// is plural by design — English words, emoji, kaomoji, quick phrases, AI suggestions — and an
    /// earlier version of this rule kept one of each and dropped the rest.
    #[test]
    fn one_lattice_row_needs_no_temporary_order_allocations() {
        let rows = [row("你好", LATTICE_SOURCE), row("你", 0)];
        let (order, allocations) =
            crate::ime::personal_rerank::allocations::count(|| runner_up_order(0, &rows));
        assert_eq!(order, None);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn short_rerank_texts_need_no_temporary_heap_state() {
        let rows = [row("你好", LATTICE_SOURCE), row("倪好", LATTICE_SOURCE)];
        let (summary, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            with_row_texts(&rows, |texts| (texts.len(), texts[0], texts[1]))
        });
        assert_eq!(summary, (2, "你好", "倪好"));
        assert_eq!(allocations, 0);
    }

    #[test]
    fn static_rerank_inputs_need_no_temporary_heap_state() {
        let rows = [row("甲", LATTICE_SOURCE), row("乙", LATTICE_SOURCE)];
        let scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -1000,
                characters: 1,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -2000,
                characters: 1,
                keyboard_neural: None,
            }),
        ];
        let scored = ScoredOrderRows::new(&rows, &scores);
        let (summary, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            let mut considered = [0; STATIC_RERANK_LIMIT];
            let mut statik = [0.0; STATIC_RERANK_LIMIT];
            let mut subset = [""; STATIC_RERANK_LIMIT];
            let count = fill_static_rerank_inputs::<STATIC_RERANK_LIMIT, _, _>(
                &scored,
                |index| scored.get(index).answers_key,
                &mut considered,
                &mut statik,
                &mut subset,
            );
            (count, considered[0], considered[1], statik[0], subset[1])
        });
        assert_eq!(summary, (Some(2), 0, 1, -1.0, "乙"));
        assert_eq!(allocations, 0);
    }

    #[test]
    fn only_the_lattice_source_is_treated_as_alternative_readings() {
        assert_eq!(LATTICE_SOURCE, 8);
        for plural in [2u8, 3, 4, 5, 6, 7] {
            assert_ne!(LATTICE_SOURCE, plural);
        }
    }

    #[test]
    fn engine_order_is_built_only_when_missing() {
        let mut engine_order = Vec::new();
        ensure_engine_order(&mut engine_order, 3);
        assert_eq!(engine_order, vec![0, 1, 2]);
        // 已经是这一轮的映射时保持原样，不重置成恒等排列。
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
    fn only_korean_keeps_its_table_order() {
        let korean = SchemeType::Korean as u8;
        assert!(!reorders_candidates(korean));
        for ordinal in (0..=u8::MAX).filter(|&ordinal| ordinal != korean) {
            if let Some(scheme) = SchemeType::from_u8(ordinal) {
                assert_eq!(reorders_candidates(ordinal), scheme.holds_phrase_progress());
            }
        }
        // 刷新失败时的占位快照不对应任何方案，照旧允许重排。
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
    fn long_sentences_keep_three_readings_after_the_first() {
        // 第一条整句留在原位，其后两条紧跟着；前排保留不超过三条非读法行，其余整句紧随其后。
        let rows = [
            row("今天天气", LATTICE_SOURCE),
            row("今天", 0),
            row("金天天气", LATTICE_SOURCE),
            row("今天添气", LATTICE_SOURCE),
            row("今添天气", LATTICE_SOURCE),
            row("金", 0),
        ];
        assert_eq!(runner_up_order(0, &rows), Some(vec![0, 2, 3, 1, 5, 4]));
    }

    fn favouring_model(characters: &[char], favoured: &[char]) -> SentenceModel {
        const CONTEXT: usize = 16;
        let vocabulary: Vec<String> = ["<pad>", "<unk>", "<bos>"]
            .into_iter()
            .map(str::to_owned)
            .chain(characters.iter().map(char::to_string))
            .collect();
        let logits: Vec<f32> = [0.0, -8.0, 0.0]
            .into_iter()
            .chain(
                characters
                    .iter()
                    .map(|c| if favoured.contains(c) { 8.0 } else { -8.0 }),
            )
            .collect();
        let config = format!(
            r#"{{"vocab":{},"n_layer":0,"n_head":1,"n_embd":1,"context":{CONTEXT},"dropout":0.0}}"#,
            vocabulary.len()
        );
        let tensors: [(&str, usize, Vec<f32>); 4] = [
            ("tok.weight", vocabulary.len(), logits),
            ("pos.weight", CONTEXT, vec![0.0; CONTEXT]),
            ("ln_f.weight", 1, vec![0.0]),
            ("ln_f.bias", 1, vec![1.0]),
        ];
        let mut header = serde_json::Map::new();
        header.insert(
            "__metadata__".into(),
            serde_json::json!({
                "format": "chinese-ime-lm",
                "version": "1",
                "precision": "f32",
                "config": config,
                "vocab": serde_json::to_string(&vocabulary).unwrap(),
            }),
        );
        let mut data = Vec::new();
        for (name, rows, values) in tensors {
            let start = data.len();
            data.extend(values.iter().flat_map(|value| value.to_le_bytes()));
            header.insert(
                name.into(),
                serde_json::json!({"dtype": "F32", "shape": [rows, 1], "data_offsets": [start, data.len()]}),
            );
        }
        header["ln_f.weight"]["shape"] = serde_json::json!([1]);
        header["ln_f.bias"]["shape"] = serde_json::json!([1]);
        let header = serde_json::to_vec(&serde_json::Value::Object(header)).unwrap();
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&data);
        SentenceModel::load(&bytes).unwrap()
    }

    #[test]
    fn runner_up_order_recognizes_neural_and_collocation_sources_and_is_idempotent() {
        let rows = [
            row("甲乙丙", LATTICE_SOURCE),
            row("甲乙", 0),
            row("甲", 0),
            row("假乙丙", 11),
            row("加乙丙", 17),
            row("佳乙丙", 11),
            row("乙", 0),
            row("丙", 0),
        ];
        let order = runner_up_order(0, &rows).expect("应重排神经/搭配次选与单字顺序");
        assert_eq!(order, vec![0, 3, 4, 1, 2, 6, 5, 7]);
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
        // 首行后保留 2 条回答整码的非同长度整句（下标 6、7），接着至多 3 条前排非读法行（1、2、3），
        // 第 4 条词（4）和单字（5）排到余下的整句（8）之后。
        assert_eq!(
            runner_up_order(0, &rows),
            Some(vec![0, 6, 7, 1, 2, 3, 8, 4, 5])
        );

        // 光标后编辑等场景整表 answers_key 均为假时，退回按同字数归组整句读法。
        let suffix_rows: Vec<_> = [
            row("丙丁戊", LATTICE_SOURCE),
            row("丙定戊", 11),
            row("丙", 0),
            row("丁", 0),
            row("戊", 0),
            row("己", 0),
            row("并丁戊", 17),
            row("柄丁戊", LATTICE_SOURCE),
        ]
        .into_iter()
        .map(|mut candidate| {
            candidate.answers_key = false;
            candidate
        })
        .collect();
        assert_eq!(
            runner_up_order(0, &suffix_rows),
            Some(vec![0, 1, 6, 2, 3, 4, 7, 5])
        );
    }

    #[test]
    fn settled_rerank_order_sorts_up_to_sixteen_static_candidates_and_falls_back_without_evidence()
    {
        let mut reranker = Reranker::new(std::sync::Arc::new(favouring_model(
            &['甲', '乙', '丙', '丁'],
            &['乙', '丙', '丁'],
        )));
        let rows = [
            row("甲甲", LATTICE_SOURCE),
            row("甲乙", LATTICE_SOURCE),
            row("甲丙", LATTICE_SOURCE),
            row("甲丁", LATTICE_SOURCE),
            row("乙甲", LATTICE_SOURCE),
            row("丙甲", LATTICE_SOURCE),
            row("丁甲", LATTICE_SOURCE),
            row("乙乙", LATTICE_SOURCE),
            row("甲戊", LATTICE_SOURCE),
            row("甲己", LATTICE_SOURCE),
            row("甲庚", LATTICE_SOURCE),
            row("乙", 0),
            row("丙乙", LATTICE_SOURCE),
            row("甲辛", LATTICE_SOURCE),
            row("甲壬", LATTICE_SOURCE),
            row("丙丙", LATTICE_SOURCE),
            row("丁丁", LATTICE_SOURCE),
        ];
        // 17 条里只有前 16 条（0..16）进入精排比较集合，16 号“丁丁”虽高分但留在原位；
        // 前 16 条中，静态分较高的“丙丙”(15) 和“乙”(11) 与“丙乙”(12) 按总分顺位排到前列。
        let scores: Vec<Option<StaticEvidence>> = (0..rows.len())
            .map(|index| {
                let milli = match index {
                    15 => -500,
                    12 => -900,
                    11 => -1200,
                    16 => -100,
                    _ => -20000 - (index as i64) * 100,
                };
                Some(StaticEvidence {
                    log_prob_milli_nats: milli,
                    characters: rows[index].text.chars().count(),
                    keyboard_neural: None,
                })
            })
            .collect();
        let scored = ScoredOrderRows::new(&rows, &scores);
        let order = settled_rerank_order(&mut reranker, "", 0, false, &scored)
            .expect("停顿精排应按综合分重排前 16 条候选");
        assert_eq!(&order[..3], &[15, 12, 11]);
        assert_eq!(order[16], 16);

        // 无静态证据时退回仅旋前第 1 名；若本来已有序则返回 None。
        let mut plain_reranker =
            Reranker::new(std::sync::Arc::new(favouring_model(&['甲', '乙'], &['乙'])));
        let fallback_rows = [row("甲", LATTICE_SOURCE), row("乙", LATTICE_SOURCE)];
        assert_eq!(
            settled_rerank_order(&mut plain_reranker, "", 0, false, &fallback_rows),
            Some(vec![1, 0])
        );
        let ordered_rows = [row("乙", LATTICE_SOURCE), row("甲", LATTICE_SOURCE)];
        assert_eq!(
            settled_rerank_order(&mut plain_reranker, "", 0, false, &ordered_rows),
            None
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
        let korean = SchemeType::Korean as u8;
        let rows = [
            row("你好", LATTICE_SOURCE),
            row("倪好", LATTICE_SOURCE),
            row("你", 0),
        ];
        assert_eq!(runner_up_order(korean, &rows), None);
        // 少于两行、没有整句读法、整句只有一个字（假名）、或排列不变时都返回 None。
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
    fn speculative_typo_sentence_never_displaces_uncorrected_readings_and_defers_first_page_seat() {
        let mut reranker =
            Reranker::new(std::sync::Arc::new(favouring_model(&['甲', '乙'], &['乙'])));
        let mut typo = row("乙乙乙", LATTICE_SOURCE);
        typo.corrected = true;
        let mut prefix = row("甲甲", 0);
        prefix.answers_key = false;
        let mut single = row("甲", 0);
        single.answers_key = false;
        let rows = [
            row("甲甲甲", LATTICE_SOURCE),
            typo,
            prefix,
            row("甲乙甲", 11),
            row("甲甲乙", LATTICE_SOURCE),
            single,
        ];
        let scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -15_000,
                characters: 3,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -1_000,
                characters: 3,
                keyboard_neural: None,
            }),
            None,
            Some(StaticEvidence {
                log_prob_milli_nats: -14_000,
                characters: 3,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -16_000,
                characters: 3,
                keyboard_neural: None,
            }),
            None,
        ];
        let scored = ScoredOrderRows::new(&rows, &scores);
        // 首行未纠错时，逐键与停顿重排均只比较未纠错字面读法（0、3、4），不把附带纠错句（1）旋至首位。
        assert_eq!(rerank_pick(&mut reranker, "", 0, false, &scored), Some(3));
        let settled = settled_rerank_order(&mut reranker, "", 0, false, &scored)
            .expect("停顿重排应重排未纠错字面读法");
        assert_eq!(settled, vec![3, 1, 2, 4, 0, 5]);

        // 首页满 3 条未纠错读法时，纠错整句退居前排前缀词之后的次选区，且再次调用保持恒等零分配。
        let page_order = runner_up_order(0, &rows).expect("首页应优先保留三条未纠错整句");
        assert_eq!(page_order, vec![0, 3, 4, 2, 5, 1]);
        let reordered: Vec<_> = page_order.iter().map(|&idx| rows[idx]).collect();
        let (again, allocations) =
            crate::ime::personal_rerank::allocations::count(|| runner_up_order(0, &reordered));
        assert_eq!(again, None);
        assert_eq!(allocations, 0);

        // 未纠错读法不足 3 条时（如 `meiganxi`），纠错整句仍保留在首页字面读法之后。
        let short_list = [row("没干系", LATTICE_SOURCE), typo, row("没", 0)];
        assert_eq!(runner_up_order(0, &short_list), None);
    }

    #[test]
    fn reranker_preserves_literal_fallback_leaders_and_ignores_mixed_english_rows() {
        let model = favouring_model(&['甲', '乙', 'C', '啦'], &['乙', '啦']);
        let mut reranker = Reranker::new(model.into());

        // 大写字面原文（`Fallback`）居首时，逐键与停顿重排均不得把后面的中文拼凑候选旋至首位。
        let literal_rows = [
            row("Cla", FALLBACK_SOURCE),
            row("Class", ENGLISH_SOURCE),
            row("C啦", LATTICE_SOURCE),
        ];
        assert!(!rerank_needs_model(0, false, &literal_rows));
        assert_eq!(
            rerank_pick(&mut reranker, "", 0, false, &literal_rows),
            None
        );
        assert_eq!(
            settled_rerank_order(&mut reranker, "", 0, false, &literal_rows),
            None
        );

        // 中文整句居首且非首选位置混有英文补全行时，静态证据评分不被无静态分数的英文行打断。
        let mixed_rows = [
            row("甲甲", LATTICE_SOURCE),
            row("conf", ENGLISH_SOURCE),
            row("configuration", ENGLISH_SOURCE),
            row("乙乙", LATTICE_SOURCE),
        ];
        let scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -12_000,
                characters: 2,
                keyboard_neural: None,
            }),
            None,
            None,
            Some(StaticEvidence {
                log_prob_milli_nats: -8_000,
                characters: 2,
                keyboard_neural: None,
            }),
        ];
        let scored = ScoredOrderRows::new(&mixed_rows, &scores);
        assert_eq!(rerank_pick(&mut reranker, "", 0, false, &scored), Some(3));
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
        // 首条「许个愿」每字 -5.0 nats；「许各院」「许哥原」每字约 -10.17 nats（落差 > 3.8 nats/字），
        // 应让出第 2、3 格给前排前缀词与单字以凑满首屏 6 格，落选整句紧跟第 2 页开头且仍在剩余单字之前。
        let scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -15_000,
                characters: 3,
                keyboard_neural: None,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(StaticEvidence {
                log_prob_milli_nats: -30_500,
                characters: 3,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -30_400,
                characters: 3,
                keyboard_neural: None,
            }),
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

        // 分差在阈值以内的正常同音次选整句照旧保留在首屏前 3 格。
        let close_rows = [
            row("今天气", LATTICE_SOURCE),
            prefix1,
            prefix2,
            row("金天气", LATTICE_SOURCE),
            row("进天气", LATTICE_SOURCE),
            char1,
        ];
        let close_scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -15_000,
                characters: 3,
                keyboard_neural: None,
            }),
            None,
            None,
            Some(StaticEvidence {
                log_prob_milli_nats: -21_000,
                characters: 3,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -22_000,
                characters: 3,
                keyboard_neural: None,
            }),
            None,
        ];
        let close_scored = ScoredOrderRows::new(&close_rows, &close_scores);
        assert_eq!(
            runner_up_order(0, &close_scored),
            Some(vec![0, 3, 4, 1, 2, 5])
        );

        // 四字及以上短语与长句即使静态均分落差 > 3.8 nats/字，仍保留全部 3 个首屏读法席位，且重排后保持幂等。
        let four_char_rows = [
            row("走板教学", LATTICE_SOURCE),
            prefix1,
            prefix2,
            row("走板胶靴", LATTICE_SOURCE),
            row("走班教学", LATTICE_SOURCE),
            char1,
        ];
        let four_char_scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -16_000,
                characters: 4,
                keyboard_neural: None,
            }),
            None,
            None,
            Some(StaticEvidence {
                log_prob_milli_nats: -18_000,
                characters: 4,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -40_000,
                characters: 4,
                keyboard_neural: None,
            }),
            None,
        ];
        let four_char_scored = ScoredOrderRows::new(&four_char_rows, &four_char_scores);
        let four_char_order =
            runner_up_order(0, &four_char_scored).expect("四字短语应保留全部 3 个首屏读法席位");
        assert_eq!(four_char_order, vec![0, 3, 4, 1, 2, 5]);
        let four_char_reordered: Vec<_> = four_char_order
            .iter()
            .map(|&idx| four_char_rows[idx])
            .collect();
        let four_char_reordered_scores: Vec<_> = four_char_order
            .iter()
            .map(|&idx| four_char_scores[idx])
            .collect();
        let four_char_reordered_scored =
            ScoredOrderRows::new(&four_char_reordered, &four_char_reordered_scores);
        assert_eq!(runner_up_order(0, &four_char_reordered_scored), None);
    }

    fn context_sensitive_layer_model() -> SentenceModel {
        const CONTEXT: usize = 16;
        const EMBD: usize = 2;
        let vocabulary: Vec<String> = ["<pad>", "<unk>", "<bos>", "培", "养", "乙", "丙"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let tok: Vec<f32> = vec![
            0.0, 0.0, // <pad>
            -10.0, 10.0, // <unk>：非汉字若未剥离会压倒正常汉字前文
            -1.0, 1.0, // <bos>：默认倾向「乙」
            3.0, -3.0, // 培
            3.0, -3.0, // 养
            -1.0, 1.0, // 乙
            1.0, -1.0, // 丙
        ];
        let qkv_weight: Vec<f32> = vec![
            0.0, 0.0, 0.0, 0.0, // Q
            0.0, 0.0, 0.0, 0.0, // K
            1.0, 0.0, 0.0, 1.0, // V = I
        ];
        let proj_weight: Vec<f32> = vec![20.0, 0.0, 0.0, 20.0];
        let config = format!(
            r#"{{"vocab":{},"n_layer":1,"n_head":1,"n_embd":{EMBD},"context":{CONTEXT},"dropout":0.0}}"#,
            vocabulary.len()
        );
        let tensors: [(&str, Vec<usize>, Vec<f32>); 16] = [
            ("tok.weight", vec![vocabulary.len(), EMBD], tok),
            ("pos.weight", vec![CONTEXT, EMBD], vec![0.0; CONTEXT * EMBD]),
            ("blocks.0.ln1.weight", vec![EMBD], vec![1.0, 1.0]),
            ("blocks.0.ln1.bias", vec![EMBD], vec![0.0, 0.0]),
            ("blocks.0.qkv.weight", vec![3 * EMBD, EMBD], qkv_weight),
            ("blocks.0.qkv.bias", vec![3 * EMBD], vec![0.0; 3 * EMBD]),
            ("blocks.0.proj.weight", vec![EMBD, EMBD], proj_weight),
            ("blocks.0.proj.bias", vec![EMBD], vec![0.0; EMBD]),
            ("blocks.0.ln2.weight", vec![EMBD], vec![1.0, 1.0]),
            ("blocks.0.ln2.bias", vec![EMBD], vec![0.0, 0.0]),
            (
                "blocks.0.fc.weight",
                vec![4 * EMBD, EMBD],
                vec![0.0; 4 * EMBD * EMBD],
            ),
            ("blocks.0.fc.bias", vec![4 * EMBD], vec![0.0; 4 * EMBD]),
            (
                "blocks.0.out.weight",
                vec![EMBD, 4 * EMBD],
                vec![0.0; EMBD * 4 * EMBD],
            ),
            ("blocks.0.out.bias", vec![EMBD], vec![0.0; EMBD]),
            ("ln_f.weight", vec![EMBD], vec![2.0, 2.0]),
            ("ln_f.bias", vec![EMBD], vec![0.0, 0.0]),
        ];
        let mut header = serde_json::Map::new();
        header.insert(
            "__metadata__".into(),
            serde_json::json!({
                "format": "chinese-ime-lm",
                "version": "1",
                "precision": "f32",
                "config": config,
                "vocab": serde_json::to_string(&vocabulary).unwrap(),
            }),
        );
        let mut data = Vec::new();
        for (name, shape, values) in tensors {
            let start = data.len();
            data.extend(values.iter().flat_map(|value| value.to_le_bytes()));
            header.insert(
                name.into(),
                serde_json::json!({"dtype": "F32", "shape": shape, "data_offsets": [start, data.len()]}),
            );
        }
        let header = serde_json::to_vec(&serde_json::Value::Object(header)).unwrap();
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&data);
        SentenceModel::load(&bytes).unwrap()
    }

    #[test]
    fn rerank_sanitizes_trailing_non_han_context_against_unk_pollution() {
        let model = std::sync::Arc::new(context_sensitive_layer_model());
        let rows = [row("乙", LATTICE_SOURCE), row("丙", LATTICE_SOURCE)];
        let scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 1,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 1,
                keyboard_neural: None,
            }),
        ];
        let scored = ScoredOrderRows::new(&rows, &scores);

        // 纯汉字「培养」前文把「丙丙」推至首位；前面带着表情符号与空格时效果完全一致。
        for ctx in ["培养", "🧐 培养"] {
            let mut reranker = Reranker::new(std::sync::Arc::clone(&model));
            assert_eq!(
                rerank_pick(&mut reranker, ctx, 0, false, &scored),
                Some(1),
                "ctx={ctx}"
            );
            let mut settled = Reranker::new(std::sync::Arc::clone(&model));
            assert_eq!(
                settled_rerank_order(&mut settled, ctx, 0, false, &scored),
                Some(vec![1, 0]),
                "ctx={ctx}"
            );
        }

        // 以标点或非汉字结尾时只剩新子句起点，行为与空上文一致（「乙乙」保持第 0 位）。
        for ctx in ["", "培养，"] {
            let mut reranker = Reranker::new(std::sync::Arc::clone(&model));
            assert_eq!(
                rerank_pick(&mut reranker, ctx, 0, false, &scored),
                None,
                "ctx={ctx}"
            );
            let mut settled = Reranker::new(std::sync::Arc::clone(&model));
            assert_eq!(
                settled_rerank_order(&mut settled, ctx, 0, false, &scored),
                None,
                "ctx={ctx}"
            );
        }
    }

    #[test]
    fn static_prior_guardrail_blocks_low_frequency_hallucination_and_marginal_jitter() {
        // Case 1：短词组（< 6 字）对比「合理同音（赤字 0.35）」与「深静态赤字幻觉（赤字 1.20）」。
        // 在无护栏下「丙」(neural 领先 +1.50, 静态赤字 1.20) 会凭 0.5*(1.50-1.20)=+0.15 反超；
        // 加入赤字护栏（excess=0.70, 额外扣 0.35）后「丙」综合落败，而赤字仅 0.35 的「乙」(+1.30) 正常晋升。
        let short_texts = ["甲甲甲甲", "乙甲乙甲", "丙丙丙丙"];
        let statik = [-3.50, -3.85, -4.70];
        let neural = [-5.00f32, -3.70, -3.50];
        let mut combined = [0.0; 3];
        fill_guarded_combined_scores(&short_texts, &statik, &neural, &mut combined);
        assert!(combined[1] > combined[0]);
        assert!(leader_promotion_allowed(
            4,
            statik[0],
            statik[1],
            combined[0],
            combined[1]
        ));
        assert!(
            combined[2] < combined[0],
            "短串深静态赤字候选（-4.70）在 +1.50 神经偏置下仍应被护栏压在首行之后: {combined:?}"
        );

        // Case 2：短词组挑战者静态分低于首行时，无上文场景综合分仅微弱领先 0.04 (< 0.06) 不得翻盘；
        // 但若（a）挑战者静态分高于首行，或（b）拼入 2 字汉字上文后总长达到 6 字完整子句，均取消额外防抖门槛正常胜出。
        let jitter_texts = ["甲甲", "乙甲", "丙甲"];
        let jitter_statik = [-3.50, -3.60, -3.40];
        let jitter_neural = [-5.00f32, -4.82, -5.08];
        fill_guarded_combined_scores(&jitter_texts, &jitter_statik, &jitter_neural, &mut combined);
        assert!(combined[1] > combined[0] && combined[1] - combined[0] < 0.06);
        assert!(!leader_promotion_allowed(
            2,
            jitter_statik[0],
            jitter_statik[1],
            combined[0],
            combined[1]
        ));
        assert!(
            !leader_promotion_allowed(
                4,
                jitter_statik[0],
                jitter_statik[1],
                combined[0],
                combined[1]
            ),
            "4 字候选在无上文（总长 4 < 6）下仍须越过 0.06 防抖门槛"
        );
        assert!(
            leader_promotion_allowed(
                2 + 4,
                jitter_statik[0],
                jitter_statik[1],
                combined[0],
                combined[1]
            ),
            "2 字汉字上文 + 4 字候选凑满 6 字完整子句时放开防抖门槛"
        );
        assert!(combined[2] > combined[0] && combined[2] - combined[0] < 0.06);
        assert!(leader_promotion_allowed(
            2,
            jitter_statik[0],
            jitter_statik[2],
            combined[0],
            combined[2]
        ));

        // Case 3：6 字及以上完整子句使用放宽后的免费赤字上限（2.00）并取消按字倍增的晋升门槛（0.0），
        // 避免误伤专名或长句单点同音词替换的微弱每字分差（例如 +0.04/字，6 字共 +0.24 nats）。
        let clause_texts = ["甲甲甲甲甲甲", "乙乙乙乙乙乙"];
        let clause_statik = [-3.50, -4.70];
        let clause_neural = [-5.00f32, -3.72];
        let mut clause_combined = [0.0; 2];
        fill_guarded_combined_scores(
            &clause_texts,
            &clause_statik,
            &clause_neural,
            &mut clause_combined,
        );
        assert!(
            clause_combined[1] > clause_combined[0]
                && clause_combined[1] - clause_combined[0] < 0.06
        );
        assert!(leader_promotion_allowed(
            6,
            clause_statik[0],
            clause_statik[1],
            clause_combined[0],
            clause_combined[1]
        ));

        // Case 4：经 settled_rerank_order 端到端验证：明显大优势正常重排。
        let rows = [row("甲甲", LATTICE_SOURCE), row("乙乙", LATTICE_SOURCE)];
        let weak_scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 2,
                keyboard_neural: None,
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -13_000,
                characters: 2,
                keyboard_neural: None,
            }),
        ];
        let weak_scored = ScoredOrderRows::new(&rows, &weak_scores);
        let mut reranker =
            Reranker::new(std::sync::Arc::new(favouring_model(&['甲', '乙'], &['乙'])));
        assert_eq!(
            settled_rerank_order(&mut reranker, "", 0, false, &weak_scored),
            Some(vec![1, 0]),
            "favouring_model 给出远超 0.06 的神经增益时应正常重排"
        );
    }

    #[test]
    fn settled_lock_leader_preserves_slot_zero_and_reorders_runners_up() {
        let rows = [
            row("甲甲", LATTICE_SOURCE),
            row("丙丙", LATTICE_SOURCE),
            row("乙乙", LATTICE_SOURCE),
        ];
        let scores = [
            Some(StaticEvidence::new(-10_000, 2)),
            Some(StaticEvidence::new(-10_200, 2)),
            Some(StaticEvidence::new(-10_200, 2)),
        ];
        let scored = ScoredOrderRows::new(&rows, &scores);
        let mut reranker = Reranker::new(std::sync::Arc::new(favouring_model(
            &['甲', '乙', '丙'],
            &['乙'],
        )));
        // 未锁首选时，乙乙升至第 0 位。
        assert_eq!(
            settled_rerank_order_with_options(&mut reranker, "", 0, false, &scored, false),
            Some(vec![2, 0, 1])
        );
        // 锁定首选时，第 0 位恒为甲甲，仅把更优次选乙乙提到第 1 位。
        assert_eq!(
            settled_rerank_order_with_options(&mut reranker, "", 0, false, &scored, true),
            Some(vec![0, 2, 1])
        );
    }

    #[test]
    fn rerank_pick_reuses_same_model_cached_keyboard_neural_score() {
        let rows = [row("甲甲", LATTICE_SOURCE), row("乙乙", LATTICE_SOURCE)];
        // 模型本身偏好甲，但缓存的同源模型分标明乙乙大幅领先：验证命中缓存时直接采用缓存分而不重跑模型。
        let model = std::sync::Arc::new(favouring_model(&['甲', '乙'], &['甲']));
        let model_id = std::sync::Arc::as_ptr(&model) as usize;
        let cached_scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 2,
                keyboard_neural: Some((model_id, -6_000)),
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 2,
                keyboard_neural: Some((model_id, -1_000)),
            }),
        ];
        let mut reranker = Reranker::new(std::sync::Arc::clone(&model));
        assert_eq!(
            rerank_pick(
                &mut reranker,
                "",
                0,
                false,
                &ScoredOrderRows::new(&rows, &cached_scores),
            ),
            Some(1)
        );
        // 模型实例标识不同时必须丢弃缓存并回退到真实模型推理（此处模型偏好甲，不晋升乙）。
        let mismatched_scores = [
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 2,
                keyboard_neural: Some((model_id.wrapping_add(1), -6_000)),
            }),
            Some(StaticEvidence {
                log_prob_milli_nats: -10_000,
                characters: 2,
                keyboard_neural: Some((model_id.wrapping_add(1), -1_000)),
            }),
        ];
        assert_eq!(
            rerank_pick(
                &mut reranker,
                "",
                0,
                false,
                &ScoredOrderRows::new(&rows, &mismatched_scores),
            ),
            None
        );
    }
}
