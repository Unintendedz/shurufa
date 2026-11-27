//! Graph construction, beam search and the typo sentence decode (quanpin.md §10.1-§10.5, Google parameters removed).
//!
//! A phrase graph and a Viterbi beam search over dictionary spans, after libpinyin's PinyinLookup2 (the unigram path score is a product of P(word), with a beam per syllable step) and sunpinyin's lattice columns. The unigram normaliser supplies the usual "fewer tokens win" bias.
//!
//! With a bigram table, each transition also earns `bigram_weight * max(0, ln(P(next|previous) / P(next)))`. Without it every path spelling the same syllables is judged on its words' frequencies alone, which is why 配置于权限 used to beat 配置与权限: 于 is the commoner character and nothing else had an opinion. The term is a bonus rather than a replacement: an absent pair leaves the path where the unigram score put it, and so does a pair the table scores below zero, since the table keeps only pairs seen often enough and an absent one is no evidence of being likelier than an observed, unlikely one. On sentences-neutral-v1 dropping the negative side raised top-1 0.608 -> 0.617 and top-5 0.833 -> 0.847 with the reranker, 0.480 -> 0.490 and 0.789 -> 0.803 without it (#7583).
//!
//! A trigram cannot be searched the same way without carrying two words of history in every beam entry, so it is applied after the search: the n best paths are rescored with what the third word adds over the second, then reordered. That is the reason to decode more paths than are shown.

use std::cell::RefCell;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell;

use super::cxx_sort;
use super::ngram::{NgramTable, SENTENCE_START};
use super::personal::PersonalNgram;
use super::LatticeLookup;
use crate::assets;
use crate::dictionary::DictRow;
use crate::paths::RuntimePaths;

/// A lattice span row: exact key, value, dictionary weight.
pub type LatticeLexeme = DictRow;

/// 字级搭配打分函数的签名：`scorer(上下文尾窗, 候选词, 是否句尾词) -> 主分同域的搭配分`。
pub type CollocationScorer<'a> = dyn Fn(&str, &str, bool) -> f64 + 'a;

/// The knobs of one decode.
#[derive(Clone)]
pub struct LatticeOptions<'a> {
    pub beam: usize,
    /// 从末列回溯的路径数。默认5条；产品的 `SentenceNgramTables::options` 搜索6条，神经重排启用时搜索12条。
    pub nbest: usize,
    /// Cap on rows per span. The DB lookup already applies the same cap.
    pub span_limit: usize,
    pub max_phrase_syllables: usize,
    /// Heuristic unigram normaliser against a phrase-length bonus: single-character msime-pinyin.db weights are corpus counts, phrase weights are on a smaller scale.
    pub unigram_z: f64,
    /// 多字词每音节奖励，并扣一份作为每词插入代价。默认值 20 沿用原校准：旧评分从 3 提到 20 时，句子首选率上升至平台期，词级结果不变；继续增大会损失手写句子集。
    pub phrase_length_bonus: f64,
    /// How much each context term may move a path (tests/src/eval_sentences.cpp and the sweep above): at bonus 20 the bigram term is worth doubling, sentences-v2 top-1 0.173 -> 0.189 and MRR 0.393 -> 0.401 with sentences-v1 unchanged, while 3.0 starts trading page coverage for it.
    pub bigram_weight: f64,
    /// The trigram weight moved nothing at any value tried, which is what a term that only fires on a third word of history looks like on sets this short; it stays at 1.0 rather than being tuned to a number the measurement cannot support.
    pub trigram_weight: f64,
    /// A transition v -> w earns `personal_weight * min(personal_max_bonus, ln((1 - mu) + mu * P_personal / P_static))`, where mu is the model's confidence in v (0 for a context never seen, capped at 0.5) and P_static stands for the static conditional probability, `personal_reference_probability * exp(bigram bonus)`. That is the log of blending the two probabilities, expressed as an increment over the static score the path already carries, so a context the user never typed after contributes nothing and one typed often shifts paths by a bounded amount. Within a composition the first word's context is the sentence start; words committed before it do not enter the lattice.
    ///
    /// Calibrated with test_personal_context_input_session.cpp against a 1.5-nat static gap between two homophones: one accepted sentence (every word counted once) must not flip it, one explicit pick (counted twice) must, and a trigram seen after one earlier word must pick a different continuation than after another. With the reference probability at 0.05 the first two cases move a path by 1.20 and 1.76 nats, and the most any transition can gain, at mu = 0.5, is about 3 nats. The cap only binds where the static tables call a pair unlikely, which is exactly where a single mistaken pick would otherwise outweigh them.
    pub personal_weight: f64,
    pub personal_reference_probability: f64,
    pub personal_max_bonus: f64,
    /// Paths to emit; 0 emits all (`sentence_alternatives`).
    pub emit: usize,
    /// How far, per typo edge, the corrected sentence has to beat the literal one before it takes the leading seat. Infinite keeps it behind the whole-sentence rows, where it only adds a candidate. Swept with eval_sentences --autocorrect-types 7: margins of 8 and 4 moved clean top-1 at every base cost from 8 to 28 (sentences-v2 46.5% -> 45.2%, sentences-neutral-v1 19.5% -> 18.4..18.9%).
    pub typo_lead_margin: f64,
    /// Beam of the typo pass, which only needs its best path. At 8 and at 4 the typo and clean eval numbers were the same as at the full beam, and the pass cost dropped from about 0.45 ms to 0.1-0.2 ms per query.
    pub typo_beam: usize,
    /// Shared by every session and outliving them.
    pub bigram: Option<Arc<NgramTable>>,
    pub trigram: Option<Arc<NgramTable>>,
    /// Borrowed while the caller holds the owning store's read lock. `None`, or a model with nothing recorded, scores every path exactly as without it.
    pub personal: Option<&'a PersonalNgram>,
    /// 二元分的备忘，只影响速度不影响结果；`None` 时每次直接查表。
    pub bigram_memo: Option<&'a RefCell<BigramMemo>>,
    /// With neural rerankers: whether the unreranked best still gets a Generated row.
    pub include_lattice_best: bool,
    /// With neural rerankers: take a source's next distinct path when its first pick is already listed.
    pub show_next_on_duplicate: bool,
    /// 字级搭配打分（octagram `.gram`）：入参是路径已累计文本的尾窗、候选词、
    /// 是否句尾词，返回自然对数域的搭配分（与词网格主分同域）；`None`（默认）= 关闭。
    pub collocation_scorer: Option<&'a CollocationScorer<'a>>,
    /// 搭配项的标定系数：scorer 存在且它大于 0 时，逐边叠加 `weight * (scorer(...) + collocation_edge_offset)` 到路径分。
    pub collocation_weight: f64,
    /// 搭配分相对未命中基线（`-6.0`）的逐边回退补偿（单位：自然对数）。
    /// 默认 `0.0` 沿用原始分值（兼容测试与自定义打分器）；产品配置设为 `3.0`，将未命中回退由 `-6.0/边` 校准为 `-3.0/边`，消除对多边路径的双重罚分。
    pub collocation_edge_offset: f64,
    /// 语法模型激活时把解码最优路径以 `CandidateSource::Collocation` 行展示；
    /// 与 `include_lattice_best` 同开且指向同一句时归属 Collocation。
    pub include_collocation_best: bool,
    /// 末列额外的束宽下限；为 0 时沿用 `beam`。产品的 `SentenceNgramTables::options` 设为 `224`。
    pub terminal_beam: usize,
    /// 末列在 `terminal_diversity_margin` 窗口内对同一非空前缀先取的唯一句子上限；为 0 时不限流。
    pub terminal_max_per_prefix: usize,
    /// 末列对共享相同末尾 `>= 2` 字后缀的路径配额上限（默认 0 关闭；产品设为 2，从结构上根除 Top-3 后缀塌缩）。
    pub terminal_max_per_suffix: usize,
    /// 末列对共享相同词边界位掩码骨架的路径配额上限（默认 0 自动推算/关闭；产品设为 6）。
    pub terminal_max_per_segmentation: usize,
    /// 启用末列多样性配额时，第一遍主窗口相对最优路径的自然对数分差上限。
    pub terminal_diversity_margin: f64,
    /// 已剥离非汉字尾缀的光标前连续汉字上文（由调用方通过 `trailing_han_clause` 传入）。
    /// 非空时，首列静态二元表不再套用句首 `<bos>` 奖励，而用末尾前词续接；`.gram` 尾窗不足 8 字时向前补齐。
    pub preceding_context: &'a str,
}

/// 统一概率底座不再施加词长断崖加分（已由 `20.0` 归零）；保留常量定义供跨模块调用方编译兼容。
pub const PHRASE_LENGTH_BONUS: f64 = 0.0;

/// 统一概率尺度下的单字语料归一常数（对应 `msime-pinyin.db` `tbl_1` 的语料总字频量级）。
pub const UNIFIED_UNIGRAM_Z: f64 = 5e8;

/// `msime-pinyin.db` 多字词桶轻度解压系数 `1.05`：温和拉开高频词与低权怪词差距，不误伤 2k–20k 中频真词。
const PHRASE_WEIGHT_DECOMPRESSION: f64 = 1.05;
/// `tbl_2` 头部核心词（p99 = 500k）对应的双字联合负对数概率锚点（约 `-3.43 nats/字`）。
const PHRASE_TWO_SYLLABLE_ANCHOR_NATS: f64 = 6.85;
/// 三字及以上词每多 1 个音节对应的词内隐式转移熵基线（nats/音节）。
const PHRASE_EXTRA_SYLLABLE_STEP_NATS: f64 = 0.60;

/// 各长度词桶在 `msime-pinyin.db` 中的实测 p99 权重参考锚（纯数据分桶统计，零词条特判）。
const fn phrase_bucket_p99_ref(syllables: usize) -> f64 {
    match syllables {
        2 => 500_000.0,
        3 => 85_000.0,
        4 => 45_000.0,
        _ => 15_000.0,
    }
}

/// `msime-pinyin.db` 三字及以上词表未测频词条的默认占位权重。
const UNMEASURED_PHRASE_SENTINEL_WEIGHT: i64 = 100;

/// 未测频占位词条折算后的平滑先验等效频次，避免大量占位长词压过高频短词组合。
const UNMEASURED_PHRASE_EFFECTIVE_WEIGHT: f64 = 15.0;

/// 产品模式下按句长与跨度自适应出边上限：保留足够中频多字词与单字（单字 `40/32/24`、多字 `28/24/20`），
/// 兼顾 `In-Pool >= 95%` 召回与 `last_key P95 <= 4 ms` 时延。
fn adaptive_span_limit(
    sentence_len: usize,
    span_len: usize,
    options: &LatticeOptions<'_>,
) -> usize {
    if options.terminal_max_per_suffix == 0 || options.span_limit == usize::MAX {
        return options.span_limit;
    }
    let cap = if sentence_len <= 2 {
        options.span_limit
    } else if sentence_len <= 6 {
        if span_len == 1 {
            40
        } else {
            28
        }
    } else if sentence_len <= 12 {
        if span_len == 1 {
            32
        } else {
            24
        }
    } else if span_len == 1 {
        24
    } else {
        20
    };
    options.span_limit.min(cap)
}

/// 产品模式下按句长自适应束宽：`n <= 6: 56`、`n <= 12: 40`、`n > 12: 32`。
fn adaptive_beam(sentence_len: usize, options: &LatticeOptions<'_>) -> usize {
    if options.terminal_max_per_suffix == 0 {
        return options.beam;
    }
    let cap = if sentence_len <= 6 {
        56
    } else if sentence_len <= 12 {
        40
    } else {
        32
    };
    options.beam.min(cap)
}

/// 双音节解码开启骨架多样性时，第一遍中 `[2]` 整词骨架最多占用的席位数（剩余席位立即留给 `[1, 1]` 拆字路径进入 Top-14 神经池）。
const TWO_SYLLABLE_PHRASE_PASS1_CAP: usize = 6;
/// 双音节解码开启骨架多样性时为 `[1, 1]` 拆字路径预留的最低席位数。
#[cfg_attr(not(test), allow(dead_code))]
const TWO_SYLLABLE_SPLIT_RESERVE: usize = 8;
/// 混淆网络 1-Span 局部扰动：保留前 6 条束搜索主路径不动，将最多 10 条位置轮询扰动句插进 Top-16 神经融合池。
const CN_KEEP_PRIMARY_PATHS: usize = 6;
const CN_TOP16_RESERVE: usize = 10;
/// `nbest > 16`（如 32）时，在保留 6 条次选束搜索路径后再追加最多 8 条长句其余位置/次优扰动。
const CN_EXTRA_POOL_RESERVE: usize = 8;
const CN_REST_BEAM_FRONT_KEEP: usize = 6;
/// 每个位置桶默认保留的局部替补上限（若前 3 条均未改写分词结构，再额外保底 1 条最优拆字/合并/重切候选）。
const CN_MAX_RIVALS_PER_POS: usize = 3;
/// 允许进入位置轮询与末列产品的最大对数概率跌幅（nats）：与 `terminal_diversity_margin = 24.0` 对齐并略留余量，
/// 放行首词无上文 `.gram` 共现但靠后续强搭配翻盘的路径（如 `还真是 + 贤者之石`），
/// 同时挡掉相对最优句低 `26 nats` 以上的生僻拼凑噪声。
const CN_MAX_LOCAL_DROP: f64 = 26.0;
/// 产品模式下每条父假设按目标列偏移各自保留的最高分出边上限（仅作用于中间列单字边与末列边），
/// 防止高熵父假设向同一目标列倾倒过量同音单字出边把次优前驱挤出束宽与末列 `terminal_limit`。
const PER_PARENT_DEST_CAP: usize = 8;
const MAX_CAP_SPAN: usize = 8;

/// 句末语气词字符集（保留导出兼容；v2 不再施加固定魔数加分，由 `.gram` 句尾搭配与神经模型决策）。
#[allow(dead_code)]
pub const SENTENCE_FINAL_PARTICLES: &[char] = &[
    '啊', '呀', '吧', '呢', '吗', '啦', '嘛', '哦', '哇', '呗', '咯', '喽',
];

/// 已清退的句末语气词固定加分魔数（归零，不再干预解码排序）。
#[allow(dead_code)]
pub const SENTENCE_FINAL_PARTICLE_BONUS: f64 = 0.0;

/// 叠字叹词（啊啊、呃呃）不是词组，不拿多音节的词组奖励，按单字口径计分。否则九宫格上 `22` 的「啊啊」凭奖励压过「吧」，「相关吧」变成「相关啊啊」。
const REDUPLICATED_INTERJECTIONS: &[char] = &[
    '啊', '呃', '哦', '嗯', '呀', '哇', '噢', '唉', '哎', '喔', '诶', '额', '谔',
];

/// 计词组奖励时这个词算几个音节：叠字叹词只算一个。
fn bonus_syllables(word: &str, syllables: usize) -> usize {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return syllables;
    };
    let reduplicated =
        syllables > 1 && REDUPLICATED_INTERJECTIONS.contains(&first) && chars.all(|c| c == first);
    if reduplicated {
        1
    } else {
        syllables
    }
}

impl Default for LatticeOptions<'_> {
    fn default() -> Self {
        Self {
            beam: 32,
            nbest: 5,
            span_limit: 32,
            max_phrase_syllables: 7,
            unigram_z: UNIFIED_UNIGRAM_Z,
            phrase_length_bonus: PHRASE_LENGTH_BONUS,
            bigram_weight: 2.0,
            trigram_weight: 1.0,
            personal_weight: 1.0,
            personal_reference_probability: 0.05,
            personal_max_bonus: 2.5,
            emit: 0,
            typo_lead_margin: f64::INFINITY,
            typo_beam: 8,
            bigram: None,
            trigram: None,
            personal: None,
            bigram_memo: None,
            include_lattice_best: true,
            show_next_on_duplicate: false,
            collocation_scorer: None,
            collocation_weight: 0.0,
            collocation_edge_offset: 0.0,
            include_collocation_best: false,
            terminal_beam: 0,
            terminal_max_per_prefix: 0,
            terminal_max_per_suffix: 0,
            terminal_max_per_segmentation: 0,
            terminal_diversity_margin: 0.0,
            preceding_context: "",
        }
    }
}

/// 活动词典持有构造时预热的 n 元表，按键查询不再构造路径或访问进程共享缓存。
#[derive(Default)]
pub(crate) struct SentenceNgramTables {
    bigram: Option<Arc<NgramTable>>,
    trigram: Option<Arc<NgramTable>>,
}

impl SentenceNgramTables {
    pub(crate) fn load(paths: &RuntimePaths) -> Self {
        Self {
            bigram: NgramTable::shared(&paths.dictionary(assets::BIGRAM_TABLE)),
            trigram: NgramTable::shared(&paths.dictionary(assets::TRIGRAM_TABLE)),
        }
    }

    pub(crate) fn options<'a>(&self, sentence_alternatives: bool) -> LatticeOptions<'a> {
        LatticeOptions {
            beam: 56,
            nbest: 32,
            span_limit: 40,
            emit: if sentence_alternatives { 0 } else { 1 },
            typo_lead_margin: 1.0,
            bigram: self.bigram.clone(),
            trigram: self.trigram.clone(),
            collocation_edge_offset: 3.0,
            terminal_beam: 224,
            terminal_max_per_prefix: 2,
            terminal_max_per_suffix: 2,
            terminal_max_per_segmentation: 6,
            terminal_diversity_margin: 24.0,
            ..LatticeOptions::default()
        }
    }
}

/// 路径型整句配置入口，与 `SentenceNgramTables::options` 保持同参供测试对照。
#[cfg(test)]
pub fn make_sentence_lattice_options<'a>(
    paths: &RuntimePaths,
    sentence_alternatives: bool,
) -> LatticeOptions<'a> {
    LatticeOptions {
        beam: 56,
        nbest: 32,
        span_limit: 40,
        emit: if sentence_alternatives { 0 } else { 1 },
        typo_lead_margin: 1.0,
        bigram: NgramTable::shared(&paths.dictionary(assets::BIGRAM_TABLE)),
        trigram: NgramTable::shared(&paths.dictionary(assets::TRIGRAM_TABLE)),
        collocation_edge_offset: 3.0,
        terminal_beam: 224,
        terminal_max_per_prefix: 2,
        terminal_max_per_suffix: 2,
        terminal_max_per_segmentation: 6,
        terminal_diversity_margin: 24.0,
        ..LatticeOptions::default()
    }
}

/// 备忘最多存的二元分条数，满了整个清空重来，约 0.5 MB。
const BIGRAM_MEMO_CAPACITY: usize = 1 << 15;

#[cfg(test)]
thread_local! {
    static BIGRAM_MEMO_HASH_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// 二元分的备忘。九键一次刷新要对几十条切分各解一次词网格，它们的前后词对大量重复，而每次查表要在 12 MB 的映射里跳几次。值就是表里查到的分数（查不到为 0），只要表不变就永远正确；表换了（按 `Arc` 的地址判断）就清空。
#[derive(Default)]
pub struct BigramMemo {
    table: usize,
    scores: HashMap<u64, f32, BuildHasherDefault<KeyHasher>>,
}

impl BigramMemo {
    fn score(&mut self, table: &Arc<NgramTable>, previous: &str, next: &str) -> f32 {
        let Some(key) = table.bigram_key(previous, next) else {
            return 0.0;
        };
        let address = Arc::as_ptr(table) as usize;
        if self.table != address {
            self.scores.clear();
            self.table = address;
        }
        let full = self.scores.len() >= BIGRAM_MEMO_CAPACITY;
        match self.scores.entry(key) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) if full => {
                let key = entry.into_key();
                self.scores.clear();
                let score = table.score(key);
                self.scores.insert(key, score);
                score
            }
            Entry::Vacant(entry) => {
                let score = table.score(key);
                entry.insert(score);
                score
            }
        }
    }
}

/// 备忘的键本身就是 FNV-1a 散列，直接拿来当散列值，不再过一遍 SipHash。
#[derive(Default)]
pub struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        #[cfg(test)]
        BIGRAM_MEMO_HASH_CALLS.with(|calls| calls.set(calls.get() + 1));
        self.0 = bytes.iter().fold(self.0, |state, &byte| {
            (state ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3)
        });
    }

    fn write_u64(&mut self, value: u64) {
        #[cfg(test)]
        BIGRAM_MEMO_HASH_CALLS.with(|calls| calls.set(calls.get() + 1));
        self.0 = value;
    }
}

/// One decoded reading.
#[derive(Debug, Clone, PartialEq)]
pub struct SentencePath {
    pub sentence: String,
    /// The syllable keys of the words, joined with `'`.
    pub key: String,
    pub log_prob: f64,
    pub words: Vec<String>,
    /// Words on this path that came from a typo edge rather than from the typed syllables.
    pub typo_edges: usize,
}

/// A typo-substituted span the typo decode may use in place of the literal syllables.
#[derive(Debug, Clone, PartialEq)]
pub struct TypoEdge {
    pub start: usize,
    pub end: usize,
    /// The word's own reading, so a path through it carries the corrected pronunciation.
    pub key: String,
    pub value: String,
    pub weight: i64,
    /// Subtracted from the edge's usual log probability.
    pub penalty: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypoSentence {
    pub sentence: String,
    /// The corrected reading.
    pub key: String,
    pub score: f64,
    pub words: Vec<String>,
    pub edges: usize,
    pub literal_score: f64,
    /// 字面最优路径是否已包含至少一个多字词（`char_count >= 2`）。
    pub literal_has_multi_char_word: bool,
}

impl TypoSentence {
    /// `score > literal + margin * edges`; with the default infinite margin never, so the typo sentence lands right after the sentence block.
    pub fn leads(&self, margin: f64) -> bool {
        self.edges > 0 && self.score > self.literal_score + margin * self.edges as f64
    }
}

/// 统一对数概率尺度的边先验：
/// - 单字边保持真实未压缩语料对数概率 `ln(w) - ln(Z_1)`；
/// - 多字词边按 `msime-pinyin.db` 各长度桶实测 p99 分位与解压系数 `1.05` 还原动态范围，
///   并叠加每多余音节的词内隐式转移基线，使高频核心词（`>= 20k`）稳胜拆字，
///   而低权冷僻双字怪词（`<= 5k`）不再凭借跨词边界差压死常见 `1+1` 口语单字组合。
pub fn edge_log_prob(weight: i64, syllables: usize, options: &LatticeOptions<'_>) -> f64 {
    let raw_weight = if weight > 0 { weight as f64 } else { 1.0 };
    let z1 = if options.unigram_z > 1.0 {
        options.unigram_z
    } else {
        UNIFIED_UNIGRAM_Z
    };
    if syllables <= 1 {
        raw_weight.ln() - z1.ln()
    } else {
        let effective_weight = if syllables >= 3 && weight == UNMEASURED_PHRASE_SENTINEL_WEIGHT {
            UNMEASURED_PHRASE_EFFECTIVE_WEIGHT
        } else {
            raw_weight
        };
        let p99_ref = phrase_bucket_p99_ref(syllables);
        let z_shift = (z1 / UNIFIED_UNIGRAM_Z).ln();
        let anchor = PHRASE_TWO_SYLLABLE_ANCHOR_NATS
            + (syllables.saturating_sub(2) as f64) * PHRASE_EXTRA_SYLLABLE_STEP_NATS
            + z_shift;
        PHRASE_WEIGHT_DECOMPRESSION * (effective_weight.ln() - p99_ref.ln()) - anchor
            + options.phrase_length_bonus * (syllables - 1) as f64
    }
}

/// 建词格、束搜索并做三元重排。调用方负责音节准入，查询行必须携带完整读音。
/// 普通整句合并仍走 `merge_lattice_candidates`，为纠错复用同一张图。
pub fn decode_sentences(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
) -> Vec<SentencePath> {
    if syllables.is_empty() {
        return Vec::new();
    }
    decode_graph(&build_graph(syllables, lookup, options), options, None)
}

/// 按音节位置约束词边后再解码，保证低频的辅助码匹配字不会在束搜索前被淘汰。
#[cfg(test)]
pub fn decode_constrained_sentences(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
    accepts: impl Fn(usize, &str) -> bool,
) -> Vec<SentencePath> {
    if syllables.is_empty() {
        return Vec::new();
    }
    let mut unfiltered = options.clone();
    unfiltered.span_limit = usize::MAX;
    let mut graph = build_graph(syllables, lookup, &unfiltered);
    // 终点计数只在本次过滤中使用，跨起始列清零复用。
    let mut counts = vec![0; syllables.len() + 1];
    for (start, edges) in graph.iter_mut().enumerate() {
        counts.fill(0);
        edges.retain(|edge| {
            if !accepts(start, &edge.word) || counts[edge.end] >= options.span_limit {
                return false;
            }
            counts[edge.end] += 1;
            true
        });
    }
    decode_graph(&graph, options, None)
}

/// Decode again with the typo edges added at `typo_beam`, and accept the best path only if it uses a typo edge, differs from the literal best and outscores it (WL:490-525). The product reaches `decode_typo_on_graph` through `merge_lattice_candidates`.
#[cfg(test)]
pub fn decode_typo_sentence(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
    literal_best: &SentencePath,
    edges: &[TypoEdge],
) -> Option<TypoSentence> {
    if syllables.is_empty() || edges.is_empty() {
        return None;
    }
    decode_typo_on_graph(
        &build_graph(syllables, lookup, options),
        options,
        literal_best,
        edges,
    )
}

pub(super) struct Edge {
    pub(super) end: usize,
    pub(super) word: String,
    pub(super) key: String,
    pub(super) log_prob: f64,
    pub(super) typo: bool,
}

/// Edges by start column.
pub(super) type Graph = Vec<Vec<Edge>>;

/// 在指定字符序号替换为 `'她'` / `'它'` 的比较与构造辅助。要求原位就是 3 字节 UTF-8 的 `'他'`。
fn char_offset_if_exact_len(word: &str, target_pos: usize, expected_len: usize) -> Option<usize> {
    let mut offset = None;
    let mut count = 0;
    for (byte_idx, _) in word.char_indices() {
        if count == target_pos {
            offset = Some(byte_idx);
        }
        count += 1;
        if count > expected_len {
            return None;
        }
    }
    (count == expected_len).then_some(offset).flatten()
}

/// 比较将 `word[offset..offset+3]` 替换为 `replacement` 后是否与 `other` 相等（栈上完成，零堆分配）。
fn equals_replaced_pronoun(word: &str, offset: usize, replacement: &str, other: &str) -> bool {
    if word.len() != other.len() || replacement.len() != 3 {
        return false;
    }
    let wb = word.as_bytes();
    let ob = other.as_bytes();
    wb[..offset] == ob[..offset]
        && replacement.as_bytes() == &ob[offset..offset + 3]
        && wb[offset + 3..] == ob[offset + 3..]
}

fn build_replaced_pronoun(word: &str, offset: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(word.len());
    out.push_str(&word[..offset]);
    out.push_str(replacement);
    out.push_str(&word[offset + 3..]);
    out
}

/// 与语料中「他→她」的自然对数级差对齐，无上文时保持「他 > 她 > 它」，指物语境下可被重排模型顺畅推至首位。
const INANIMATE_PRONOUN_BACKOFF: f64 = 0.35;

/// 多音节跨度在 `"ta"` 音节位同时收录「他」与「她」词边、却漏收同形「它」词边时，
/// 证明该位在此搭配中是第三人称单数代词而非专名或语素（排除「吉他」「他乡」），
/// 按两者较低对数分减 `INANIMATE_PRONOUN_BACKOFF` 补齐「它」边，保留词组长度奖励下限。
fn append_inanimate_pronoun_edges(
    span: &[String],
    end: usize,
    begin: usize,
    edges: &mut Vec<Edge>,
    options: &LatticeOptions<'_>,
) {
    let span_len = span.len();
    if span_len < 2 || edges.len() <= begin || !span.iter().any(|syllable| syllable == "ta") {
        return;
    }
    let initial_end = edges.len();
    let floor = edge_log_prob(1, span_len, options);
    for (pos, syllable) in span.iter().enumerate() {
        if syllable != "ta" {
            continue;
        }
        for idx in begin..initial_end {
            let Some(offset) = char_offset_if_exact_len(&edges[idx].word, pos, span_len) else {
                continue;
            };
            if edges[idx].word.as_bytes().get(offset..offset + 3) != Some("他".as_bytes()) {
                continue;
            }
            let Some(she_log_prob) = edges[begin..initial_end]
                .iter()
                .find(|candidate| {
                    equals_replaced_pronoun(&edges[idx].word, offset, "她", &candidate.word)
                })
                .map(|candidate| candidate.log_prob)
            else {
                continue;
            };
            if edges[begin..].iter().any(|candidate| {
                equals_replaced_pronoun(&edges[idx].word, offset, "它", &candidate.word)
            }) {
                continue;
            }
            let it_word = build_replaced_pronoun(&edges[idx].word, offset, "它");
            let key = edges[idx].key.clone();
            let log_prob =
                (edges[idx].log_prob.min(she_log_prob) - INANIMATE_PRONOUN_BACKOFF).max(floor);
            edges.push(Edge {
                end,
                word: it_word,
                key,
                log_prob,
                typo: false,
            });
        }
    }
}

/// WL:110-148.
pub(super) fn build_graph(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
) -> Graph {
    let n = syllables.len();
    let max_len = options.max_phrase_syllables.max(1);
    let mut graph: Graph = (0..n).map(|_| Vec::new()).collect();
    // 重复跨度只查一次字典，缓存首个图边区间；首次查询行直接移交字段。
    struct CachedSpan {
        column: usize,
        edges: std::ops::Range<usize>,
        reserve: usize,
    }
    let span_capacity = (0..n)
        .map(|start| n.min(start.saturating_add(max_len)) - start)
        .sum();
    let mut span_cache: HashMap<&[String], CachedSpan> = HashMap::with_capacity(span_capacity);
    for start in 0..n {
        // 同一列的跨度长度各异，重复键必在更早的列；追加图边不改变区间索引。
        let (earlier, remaining) = graph.split_at_mut(start);
        let edges = &mut remaining[0];
        for end in start + 1..=n.min(start + max_len) {
            let span = &syllables[start..end];
            match span_cache.entry(span) {
                std::collections::hash_map::Entry::Occupied(entry) => {
                    let cached = entry.get();
                    edges.reserve(cached.reserve);
                    for edge in &earlier[cached.column][cached.edges.clone()] {
                        edges.push(Edge {
                            end,
                            word: edge.word.clone(),
                            key: edge.key.clone(),
                            log_prob: edge.log_prob,
                            typo: false,
                        });
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let rows = lookup(span);
                    let limit = adaptive_span_limit(n, end - start, options);
                    edges.reserve(rows.len().min(limit));
                    let begin = edges.len();
                    let mut span_key = None;
                    for row in rows.into_iter().take(limit) {
                        if row.value.is_empty() {
                            continue;
                        }
                        let log_prob = edge_log_prob(
                            row.weight,
                            bonus_syllables(&row.value, end - start),
                            options,
                        );
                        edges.push(Edge {
                            end,
                            word: row.value,
                            key: if row.key.is_empty() {
                                span_key.get_or_insert_with(|| span.join("'")).clone()
                            } else {
                                row.key
                            },
                            log_prob,
                            typo: false,
                        });
                    }
                    append_inanimate_pronoun_edges(span, end, begin, edges, options);
                    let span_edges = begin..edges.len();
                    let reserve = span_edges.len();
                    entry.insert(CachedSpan {
                        column: start,
                        edges: span_edges,
                        reserve,
                    });
                }
            }
        }
    }
    super::separable_vo::append_separable_vo_edges(syllables, &mut graph, options);
    graph
}

/// 与 `build_graph` 相同，但查询行只读借用调用方缓存；图边仍取得自己的字段所有权。
pub(super) fn build_graph_borrowed<'rows>(
    syllables: &[String],
    lookup: &mut impl FnMut(&[String]) -> &'rows [LatticeLexeme],
    options: &LatticeOptions<'_>,
) -> Graph {
    let n = syllables.len();
    let max_len = options.max_phrase_syllables.max(1);
    let mut graph: Graph = (0..n).map(|_| Vec::new()).collect();
    struct CachedSpan {
        column: usize,
        edges: std::ops::Range<usize>,
        reserve: usize,
    }
    let span_capacity = (0..n)
        .map(|start| n.min(start.saturating_add(max_len)) - start)
        .sum();
    let mut span_cache: HashMap<&[String], CachedSpan> = HashMap::with_capacity(span_capacity);
    for start in 0..n {
        let (earlier, remaining) = graph.split_at_mut(start);
        let edges = &mut remaining[0];
        for end in start + 1..=n.min(start + max_len) {
            let span = &syllables[start..end];
            match span_cache.entry(span) {
                std::collections::hash_map::Entry::Occupied(entry) => {
                    let cached = entry.get();
                    edges.reserve(cached.reserve);
                    for edge in &earlier[cached.column][cached.edges.clone()] {
                        edges.push(Edge {
                            end,
                            word: edge.word.clone(),
                            key: edge.key.clone(),
                            log_prob: edge.log_prob,
                            typo: false,
                        });
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let rows = lookup(span);
                    let limit = adaptive_span_limit(n, end - start, options);
                    edges.reserve(rows.len().min(limit));
                    let begin = edges.len();
                    let mut span_key = None;
                    for row in rows.iter().take(limit) {
                        if row.value.is_empty() {
                            continue;
                        }
                        edges.push(Edge {
                            end,
                            word: row.value.clone(),
                            key: if row.key.is_empty() {
                                span_key.get_or_insert_with(|| span.join("'")).clone()
                            } else {
                                row.key.clone()
                            },
                            log_prob: edge_log_prob(
                                row.weight,
                                bonus_syllables(&row.value, end - start),
                                options,
                            ),
                            typo: false,
                        });
                    }
                    append_inanimate_pronoun_edges(span, end, begin, edges, options);
                    let span_edges = begin..edges.len();
                    let reserve = span_edges.len();
                    entry.insert(CachedSpan {
                        column: start,
                        edges: span_edges,
                        reserve,
                    });
                }
            }
        }
    }
    super::separable_vo::append_separable_vo_edges(syllables, &mut graph, options);
    graph
}

struct BeamColumn<'g> {
    hyps: Vec<Hyp<'g>>,
    streamed: bool,
}

#[derive(Clone, Copy)]
struct Hyp<'g> {
    score: f64,
    /// 前驱假设的列号与行号；起点填 `(0, 0)`，只有 `edge` 非空时才可回溯。
    prev: (usize, usize),
    /// 到达本假设的图边；仅起点为 `None`，同时表示没有前驱。
    edge: Option<&'g Edge>,
    typo_edges: usize,
}

/// 搭配查询的上下文窗口：`.gram` 的查询键最多编入 8 个码点，尾窗留 8 个足够。
const COLLOCATION_TAIL_CODEPOINTS: usize = 8;

/// 定长内联的 UTF-8 尾窗，随 `Hyp` 按值复制，不进堆。
#[derive(Clone, Copy, Default)]
struct CollocationTail {
    bytes: [u8; COLLOCATION_TAIL_CODEPOINTS * 4],
    len: u8,
}

impl CollocationTail {
    fn as_str(&self) -> &str {
        #[cfg(all(test, debug_assertions))]
        collocation_tail_tests::record_tail_string();
        // 内容只由词边文本拼成，恒为合法 UTF-8；兜底空串不 panic。
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    /// 尾窗推进一个词：留下 (tail + word) 的最后 `COLLOCATION_TAIL_CODEPOINTS` 个码点。
    fn push(&mut self, word: &str) {
        let word_cps = word.chars().count();
        if word_cps >= COLLOCATION_TAIL_CODEPOINTS {
            let start = word
                .char_indices()
                .nth(word_cps - COLLOCATION_TAIL_CODEPOINTS)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let tail = &word[start..];
            self.bytes[..tail.len()].copy_from_slice(tail.as_bytes());
            self.len = tail.len() as u8;
            return;
        }
        let keep = COLLOCATION_TAIL_CODEPOINTS - word_cps;
        // 先算好位置结束对 self 的只读借用，再原地搬移字节。
        let (start, tail_len) = {
            let tail = self.as_str();
            let skip = tail.chars().count().saturating_sub(keep);
            (
                tail.char_indices()
                    .nth(skip)
                    .map(|(i, _)| i)
                    .unwrap_or(tail.len()),
                tail.len(),
            )
        };
        let keep_bytes = tail_len - start;
        self.bytes.copy_within(start..tail_len, 0);
        self.bytes[keep_bytes..keep_bytes + word.len()].copy_from_slice(word.as_bytes());
        self.len = (keep_bytes + word.len()) as u8;
    }
}

/// 沿 `prev` 链回溯，把路径已累计文本的最后 `COLLOCATION_TAIL_CODEPOINTS`
/// 个码点装进尾窗。只为 `collocation_scorer` 激活的假设回溯一次，
/// 未激活路径不占 `Hyp` 的内存。
#[cfg_attr(not(test), allow(dead_code))]
fn collocation_tail(columns: &[BeamColumn<'_>], pos: usize, index: usize) -> CollocationTail {
    collocation_tail_with_context(columns, pos, index, "")
}

/// 同 [`collocation_tail`]，但当当前拼路径不足 8 个码点且 `preceding_context` 非空时，
/// 向前补入紧邻汉字前文。
fn collocation_tail_with_context(
    columns: &[BeamColumn<'_>],
    pos: usize,
    index: usize,
    preceding_context: &str,
) -> CollocationTail {
    #[cfg(all(test, debug_assertions))]
    collocation_tail_tests::record_tail();
    let mut words = [None; COLLOCATION_TAIL_CODEPOINTS];
    let mut count = 0;
    let mut cps = 0usize;
    let (mut p, mut i) = (pos, index);
    while let Some(edge) = columns[p].hyps[i].edge {
        words[count] = Some(edge.word.as_str());
        count += 1;
        cps += edge.word.chars().count();
        if cps >= COLLOCATION_TAIL_CODEPOINTS || count >= words.len() {
            break;
        }
        let prev = columns[p].hyps[i].prev;
        p = prev.0;
        i = prev.1;
    }
    let mut tail = CollocationTail::default();
    if cps < COLLOCATION_TAIL_CODEPOINTS && !preceding_context.is_empty() {
        let prefix =
            crate::text::last_characters(preceding_context, COLLOCATION_TAIL_CODEPOINTS - cps);
        if !prefix.is_empty() {
            tail.push(prefix);
        }
    }
    for word in words[..count].iter().rev().flatten() {
        tail.push(word);
    }
    tail
}

/// 按分数将单个假设写入目标列（支持流式堆选或普通追加）。
#[inline]
fn push_column_hyp<'g>(target: &mut BeamColumn<'g>, limit: usize, next: Hyp<'g>) {
    if target.streamed {
        cxx_sort::push_partial(&mut target.hyps, limit, next, &mut scores_higher);
    } else {
        target.hyps.push(next);
    }
}

/// 同一列里每个父假设按目标列偏移各自保留最多 `PER_PARENT_DEST_CAP` 条最高分出边（栈缓冲零堆分配），
/// 仅限制中间列单字转移与末列转移，防止前几个领跑父假设各自倾倒 40+ 条同音单字把次优前驱挤出 `eff_beam` 与末列 `terminal_limit`。
fn insert_capped_dest_hyp<'g>(
    buf: &mut [Option<Hyp<'g>>; PER_PARENT_DEST_CAP],
    len: &mut usize,
    cand: Hyp<'g>,
) {
    if *len < buf.len() {
        buf[*len] = Some(cand);
        *len += 1;
        return;
    }
    let mut min_idx = 0usize;
    let mut min_score = buf[0].expect("filled slot").score;
    for (idx, slot) in buf.iter().enumerate().skip(1) {
        let score = slot.expect("filled slot").score;
        if score < min_score {
            min_score = score;
            min_idx = idx;
        }
    }
    if cand.score > min_score {
        buf[min_idx] = Some(cand);
    }
}

/// The C++ comparator of both beam sorts (WL:106, WL:447).
fn scores_higher(a: &Hyp<'_>, b: &Hyp<'_>) -> bool {
    a.score > b.score
}

/// Keeps the best `beam` hypotheses with libc++'s `partial_sort`, whose heap selection decides which of several tied hypotheses survive and in what order (WL:101-108).
fn keep_beam(column: &mut Vec<Hyp<'_>>, beam: usize) {
    if column.len() <= beam {
        return;
    }
    cxx_sort::partial_sort(column, beam, &mut scores_higher);
    column.truncate(beam);
}

/// 按前向图累计各列在剪枝前可能收到的假设上界；源列剪枝后最多保留 `beam` 条。
fn column_capacities(graph: &Graph, extra: Option<&Graph>, beam: usize) -> Vec<usize> {
    let mut capacities = vec![0usize; graph.len() + 1];
    capacities[0] = 1;
    for (start, edges) in graph.iter().enumerate() {
        let surviving = capacities[start].min(beam);
        for edge in edges
            .iter()
            .chain(extra.into_iter().flat_map(|extra| &extra[start]))
        {
            if edge.end <= graph.len() {
                capacities[edge.end] = capacities[edge.end].saturating_add(surviving);
            }
        }
    }
    capacities
}

/// Null when the options carry no personal data worth consulting, so the decode keeps its exact prior arithmetic (WL:71-76).
fn active_personal<'a>(options: &LatticeOptions<'a>) -> Option<&'a PersonalNgram> {
    options
        .personal
        .filter(|personal| !personal.is_empty() && options.personal_weight != 0.0)
}

/// WL:80-89. `static_bonus` is the unweighted static bigram increment of the same transition, 0 without a table.
fn personal_transition(
    personal: &PersonalNgram,
    context: &super::personal::PersonalContext,
    word: &str,
    static_bonus: f64,
    options: &LatticeOptions<'_>,
) -> f64 {
    let mu = context.confidence;
    let reference = options.personal_reference_probability * static_bonus.exp();
    if mu <= 0.0 || reference <= 0.0 {
        return 0.0;
    }
    let lift = ((1.0 - mu) + mu * personal.probability(context, word) / reference).ln();
    options.personal_weight * options.personal_max_bonus.min(lift)
}

// 小列只在线性借用槽中定位，超过上限后沿用原哈希索引。
const SMALL_BIGRAM_ROWS: usize = 8;

/// WL:385-485. `extra` holds more edges per column searched as if they were part of `graph`, so the typo decode adds edges without copying the literal graph.
pub(super) fn decode_graph(
    graph: &Graph,
    options: &LatticeOptions<'_>,
    extra: Option<&Graph>,
) -> Vec<SentencePath> {
    // 常见预算与非末列一致，单独实例化以保留展开循环的固定束宽。
    if options.nbest > options.beam || options.terminal_beam > options.beam {
        decode_graph_with_terminal::<true>(graph, options, extra)
    } else {
        decode_graph_with_terminal::<false>(graph, options, extra)
    }
}

fn decode_graph_with_terminal<'g, const WIDE_TERMINAL: bool>(
    graph: &'g Graph,
    options: &LatticeOptions<'_>,
    extra: Option<&'g Graph>,
) -> Vec<SentencePath> {
    let n = graph.len();
    let personal = active_personal(options);
    let bigram_table = options.bigram.as_ref();
    let bigram = options.bigram.as_deref();
    // 搭配项是逐边加分：scorer 存在且标定系数为正才生效，退回现状与缺表完全一致。
    let collocation = options
        .collocation_scorer
        .filter(|_| options.collocation_weight > 0.0);
    let mut memo = options.bigram_memo.map(RefCell::borrow_mut);
    let eff_beam = adaptive_beam(n, options);
    let capacities = column_capacities(graph, extra, eff_beam);
    let terminal_limit = if WIDE_TERMINAL {
        if options.terminal_max_per_suffix > 0 {
            eff_beam
                .saturating_mul(4)
                .max(options.nbest)
                .min(options.terminal_beam.max(options.nbest))
        } else {
            eff_beam.max(options.nbest).max(options.terminal_beam)
        }
    } else {
        eff_beam
    };
    // 各列先按入列上界决定是否流式选择；末列使用最终结果预算。
    let mut columns: Vec<BeamColumn<'_>> = capacities
        .into_iter()
        .enumerate()
        .map(|(position, capacity)| {
            let limit = if WIDE_TERMINAL && position == n {
                terminal_limit
            } else {
                eff_beam
            };
            let stream_capacity = limit.saturating_add(1);
            let streamed = limit != 0 && capacity > stream_capacity;
            BeamColumn {
                hyps: Vec::with_capacity(if streamed { stream_capacity } else { capacity }),
                streamed,
            }
        })
        .collect();
    columns[0].hyps.push(Hyp {
        score: 0.0,
        prev: (0, 0),
        edge: None,
        typo_edges: 0,
    });

    // 同一列里很多假设经由同一个词到达，只是更早的历史不同；它们对每条出边查到的二元分完全一样。按前一个词把这一列所有出边的二元分记下来，同一个词只查一遍表。
    let preceding_word = crate::text::last_characters(options.preceding_context, 2);
    let row_capacity = eff_beam.max(options.nbest);
    let mut bigram_rows: Vec<Vec<f32>> = Vec::new();
    let mut bigram_row_indices: HashMap<&str, usize> = HashMap::new();
    let mut small_bigram_row_words = [""; SMALL_BIGRAM_ROWS];
    // 搭配打分按列去重缓存：同一列中共享相同最后 5 码点尾窗的假设对全部出边复用同一行 `.gram` 分，
    // 仅在产品补偿模式（`collocation_edge_offset != 0.0`）开启，保持全部冻结对照测试逐事件一致。
    let cache_collocation_rows = collocation.is_some() && options.collocation_edge_offset != 0.0;
    let mut collocation_row_keys: Vec<CollocationTail> = Vec::new();
    let mut collocation_rows: Vec<Vec<f64>> = Vec::new();
    for pos in 0..n {
        if columns[pos].streamed {
            cxx_sort::finish_partial(&mut columns[pos].hyps, eff_beam, &mut scores_higher);
        } else {
            keep_beam(&mut columns[pos].hyps, eff_beam);
        }
        let (done, ahead) = columns.split_at_mut(pos + 1);
        let current = &done[pos].hyps;
        // 行存储跨列复用；前词索引和有效行数只属于当前列，评分必须重新填满。
        let mut used_bigram_rows = 0;
        let mut used_collocation_rows = 0usize;
        bigram_row_indices.clear();
        collocation_row_keys.clear();
        // 只读原始切片，评分与展开都按主图、附加图的相同顺序重新遍历。
        let literal = &graph[pos];
        let extra_edges = extra.map_or(&[][..], |extra| extra[pos].as_slice());
        let outgoing_len = literal.len() + extra_edges.len();
        let outgoing = literal.iter().chain(extra_edges.iter());
        for (index, hyp) in current.iter().enumerate() {
            // 第 0 列在有连续汉字上文时改用末尾前词续接，不再套用句首 `<bos>` 奖励；仅当无紧邻汉字上文时才使用 `SENTENCE_START`。
            let previous = match hyp.edge {
                Some(edge) => edge.word.as_str(),
                None if preceding_word.is_empty() => SENTENCE_START,
                None => preceding_word,
            };
            let personal_previous = hyp.edge.map_or(SENTENCE_START, |edge| edge.word.as_str());
            let bonuses = bigram_table.map(|table| {
                let cached_row = if used_bigram_rows <= SMALL_BIGRAM_ROWS {
                    small_bigram_row_words[..used_bigram_rows]
                        .iter()
                        .position(|word| *word == previous)
                } else {
                    bigram_row_indices.get(previous).copied()
                };
                let row = if let Some(row) = cached_row {
                    row
                } else {
                    let row = used_bigram_rows;
                    if row == bigram_rows.len() {
                        if bigram_rows.is_empty() {
                            bigram_rows.reserve_exact(row_capacity);
                        }
                        bigram_rows.push(Vec::new());
                    }
                    let scores = &mut bigram_rows[row];
                    scores.clear();
                    scores.reserve_exact(outgoing_len);
                    match memo.as_deref_mut() {
                        Some(memo) => scores.extend(
                            outgoing
                                .clone()
                                .map(|edge| memo.score(table, previous, &edge.word)),
                        ),
                        None => scores.extend(
                            outgoing
                                .clone()
                                .map(|edge| table.bigram(previous, &edge.word)),
                        ),
                    }
                    used_bigram_rows += 1;
                    if row < SMALL_BIGRAM_ROWS {
                        small_bigram_row_words[row] = previous;
                    } else {
                        if row == SMALL_BIGRAM_ROWS {
                            // 第九个不同前词才建立哈希索引，行号仍按首次出现的顺序。
                            bigram_row_indices.reserve(row_capacity);
                            bigram_row_indices.extend(
                                small_bigram_row_words
                                    .iter()
                                    .enumerate()
                                    .map(|(index, word)| (*word, index)),
                            );
                        }
                        bigram_row_indices.insert(previous, row);
                    }
                    row
                };
                &bigram_rows[row]
            });
            // 前驱列已经剪枝且不会再改变；前驱列为零时没有更早的词，起点也填零。
            let context = personal.map(|personal| {
                let earlier = match hyp.prev {
                    (column, at) if column > 0 => {
                        done[column].hyps[at].edge.map(|edge| edge.word.as_str())
                    }
                    _ => None,
                };
                personal.context(earlier, Some(personal_previous))
            });
            // 同一假设的出边共享上下文；空列或搭配停用时不回溯。
            // 产品模式下按 5 码点搭配尾窗在列内缓存整行搭配分，避免长句同尾窗假设重复探测 400 MB `.gram`。
            let prepared_tail = collocation.filter(|_| outgoing_len != 0).map(|scorer| {
                (
                    scorer,
                    collocation_tail_with_context(done, pos, index, options.preceding_context),
                )
            });
            let cached_collocation_row: Option<&[f64]> = if cache_collocation_rows {
                prepared_tail.as_ref().map(|(scorer, tail)| {
                    // `GramConfig::default()` 只看上下文末尾至多 5 码点，截齐尾窗键以提高列内命中率。
                    let tail_str = tail.as_str();
                    let short_tail = crate::text::last_characters(tail_str, 5);
                    let mut key = CollocationTail::default();
                    if !short_tail.is_empty() {
                        key.push(short_tail);
                    }
                    let key_bytes = &key.bytes[..key.len as usize];
                    let found = collocation_row_keys[..used_collocation_rows]
                        .iter()
                        .position(|existing| &existing.bytes[..existing.len as usize] == key_bytes);
                    let row = if let Some(row) = found {
                        row
                    } else {
                        let row = used_collocation_rows;
                        if row == collocation_rows.len() {
                            collocation_rows.push(Vec::new());
                        }
                        let scores = &mut collocation_rows[row];
                        scores.clear();
                        scores.reserve(outgoing_len);
                        let offset = options.collocation_edge_offset;
                        let weight = options.collocation_weight;
                        for edge in outgoing.clone() {
                            let raw = scorer(short_tail, &edge.word, edge.end == n);
                            scores.push(weight * (raw + offset));
                        }
                        collocation_row_keys.push(key);
                        used_collocation_rows += 1;
                        row
                    };
                    collocation_rows[row].as_slice()
                })
            } else {
                None
            };
            let direct_collocation = if cache_collocation_rows {
                None
            } else {
                prepared_tail
                    .as_ref()
                    .map(|(scorer, tail)| (*scorer, tail.as_str()))
            };
            let cap_dest_fanout =
                options.terminal_max_per_suffix > 0 && extra.is_none() && n >= 3 && pos >= 1;
            let mut dest_bufs: [[Option<Hyp<'g>>; PER_PARENT_DEST_CAP]; MAX_CAP_SPAN] =
                [[None; PER_PARENT_DEST_CAP]; MAX_CAP_SPAN];
            let mut dest_lens = [0usize; MAX_CAP_SPAN];
            for (at, edge) in outgoing.clone().enumerate() {
                let mut score = hyp.score + edge.log_prob;
                if let Some(row) = cached_collocation_row {
                    score += row[at];
                } else if let Some((scorer, tail)) = direct_collocation {
                    // 句尾词把 is_rear 置真：`.gram` 的「词+$」rear 项据此参与打分。
                    score += options.collocation_weight * scorer(tail, &edge.word, edge.end == n);
                }
                let static_bonus = bonuses.map_or(0.0, |bonuses| f64::from(bonuses[at]));
                if bigram.is_some() {
                    // Only a pair the table favours moves the path. A pair it calls less likely than chance counts the same as one it never kept: the table drops rare pairs, so an absent pair is usually rarer still, and penalising the observed one made a common word lose to a rarer one with no pair at all. 句首→一大 is -1.0 and 句首→益达 is absent, which put 益达拖 ahead of 一大拖 for yidatuo (#7583).
                    score += options.bigram_weight * static_bonus.max(0.0);
                }
                if let (Some(personal), Some(context)) = (personal, context.as_ref()) {
                    if context.confidence > 0.0 {
                        score += personal_transition(
                            personal,
                            context,
                            &edge.word,
                            static_bonus,
                            options,
                        );
                    }
                }
                let next = Hyp {
                    score,
                    prev: (pos, index),
                    edge: Some(edge),
                    typo_edges: hyp.typo_edges + usize::from(edge.typo),
                };
                let dist = edge.end - pos;
                if cap_dest_fanout
                    && (1..=MAX_CAP_SPAN).contains(&dist)
                    && (dist == 1 || edge.end == n)
                {
                    insert_capped_dest_hyp(
                        &mut dest_bufs[dist - 1],
                        &mut dest_lens[dist - 1],
                        next,
                    );
                } else {
                    let limit = if WIDE_TERMINAL && edge.end == n {
                        terminal_limit
                    } else {
                        eff_beam
                    };
                    push_column_hyp(&mut ahead[dist - 1], limit, next);
                }
            }
            for dist in 1..=MAX_CAP_SPAN {
                let count = dest_lens[dist - 1];
                if count == 0 {
                    continue;
                }
                let slice = &mut dest_bufs[dist - 1][..count];
                slice.sort_by(|a, b| {
                    b.expect("filled slot")
                        .score
                        .total_cmp(&a.expect("filled slot").score)
                });
                let limit = if WIDE_TERMINAL && pos + dist == n {
                    terminal_limit
                } else {
                    eff_beam
                };
                let target = &mut ahead[dist - 1];
                for next in slice.iter().flatten().copied() {
                    push_column_hyp(target, limit, next);
                }
            }
        }
    }

    // 末列不再被后续路径回溯；先完成同一堆选择，再沿用原最终排序。
    let terminal = &mut columns[n];
    let mut last = std::mem::take(&mut terminal.hyps);
    if last.is_empty() {
        return Vec::new();
    }
    if terminal.streamed {
        cxx_sort::finish_partial(&mut last, terminal_limit, &mut scores_higher);
    } else {
        keep_beam(&mut last, terminal_limit);
    }
    cxx_sort::sort(&mut last, &mut scores_higher);
    let take = options.nbest.min(last.len());

    let max_per_prefix = options.terminal_max_per_prefix;
    let max_per_suffix = options.terminal_max_per_suffix;
    let max_per_seg_opt = options.terminal_max_per_segmentation;
    let diversity_active =
        (max_per_prefix > 0 || max_per_suffix > 0 || max_per_seg_opt > 0) && n >= 2 && take > 1;
    let mut trace = Vec::new();
    let mut paths: Vec<SentencePath>;
    let mut post_trigram_suffix_cap = 0usize;
    let mut post_trigram_prefix_cap = 0usize;
    if !diversity_active {
        // 未启用多样性配额时保持原快速路径与内存分配语义，兼容全部冻结对照测试。
        let inline_unique = last.len() <= SMALL_SENTENCE_PATHS;
        paths = Vec::with_capacity(last.len());
        for hyp in &last {
            if inline_unique && paths.len() == take {
                break;
            }
            materialize_terminal_hyp(hyp, &columns, n, &mut trace, &mut paths, inline_unique);
        }
        drop(trace);
        if !inline_unique {
            retain_unique_sentences(&mut paths, take);
        }
    } else {
        paths = Vec::with_capacity(take);
        let best_score = last[0].score;
        let margin = options.terminal_diversity_margin;
        // `n >= 3` 时同时启用前驱列、首词（避免 Top-6 被同一前词包场挤走次优锚点）、末尾 >=2 字后缀与分词骨架多维配额；
        // `n == 2` 时在 `nbest > 8` 下限制单一整词骨架不超过 `take - 6`，确保至少 8 席留给 `[1, 1]` 拆字路径。
        let prefix_cap = if n >= 3 { max_per_prefix } else { 0 };
        let head_cap = if n >= 2 && max_per_prefix > 0 {
            max_per_prefix.saturating_add(1)
        } else {
            0
        };
        let suffix_cap = if n == 2 && max_per_suffix > 0 {
            max_per_suffix.max(3)
        } else if n >= 3 {
            max_per_suffix
        } else {
            0
        };
        // 双音节输入下，精确 `tbl_2` 整词已由词典直出，第一遍将 `[2]` 整词骨架限为 6 席、
        // `[1, 1]` 拆字骨架按 `take - 6` 放行，使带语境的 `[1, 1]` 拆字紧随前 6 条精确词直接落入 Top-14 神经池。
        let two_syl_phrase_mask = 1u64;
        let seg_cap = if n == 2 {
            if take > TWO_SYLLABLE_PHRASE_PASS1_CAP + 2 {
                take - TWO_SYLLABLE_PHRASE_PASS1_CAP
            } else {
                0
            }
        } else if max_per_seg_opt > 0 {
            max_per_seg_opt
        } else if max_per_prefix > 0 {
            (take / 3).max(max_per_prefix.saturating_mul(2))
        } else {
            0
        };
        post_trigram_suffix_cap = suffix_cap;
        post_trigram_prefix_cap = prefix_cap;

        const MULTI_CHAR_HEAD_RESCUE_LIMIT: usize = 3;
        const MAX_LEADING_SINGLE_PAIR_PASS1: usize = 4;
        let pass1_limit = if n >= 3 && head_cap > 0 && take >= 16 {
            take - MULTI_CHAR_HEAD_RESCUE_LIMIT
        } else {
            take
        };
        let mut prefix_counts: Vec<((usize, usize), usize)> = Vec::new();
        let mut head_counts: Vec<(&'g str, usize)> = Vec::new();
        let mut suffix_counts: Vec<(String, usize)> = Vec::new();
        let mut seg_counts: Vec<(u64, usize)> = Vec::new();
        let mut leading_single_pair_count = 0usize;
        let mut deferred: Vec<usize> = Vec::new();
        let mut pass1_end = last.len();
        let mut sentence_buf = String::new();

        // 第一遍：扫描分差窗口内满足全部维度配额的多样性假设；超配额假设进入 `deferred`。
        for (hyp_idx, hyp) in last.iter().enumerate() {
            if paths.len() == pass1_limit {
                pass1_end = hyp_idx;
                break;
            }
            if (best_score - hyp.score) > margin {
                pass1_end = hyp_idx;
                break;
            }
            let prefix = hyp.prev;
            let prefix_throttled =
                prefix_cap > 0 && prefix.0 >= 1 && count_pair(&prefix_counts, prefix) >= prefix_cap;
            let seg_sig = segmentation_signature(hyp, &columns);
            let this_seg_cap = if n == 2 && seg_cap > 0 && seg_sig == two_syl_phrase_mask {
                TWO_SYLLABLE_PHRASE_PASS1_CAP
            } else {
                seg_cap
            };
            let seg_throttled = this_seg_cap > 0 && count_u64(&seg_counts, seg_sig) >= this_seg_cap;
            if prefix_throttled || seg_throttled {
                deferred.push(hyp_idx);
                continue;
            }
            trace_terminal_sentence(hyp, &columns, n, &mut trace, &mut sentence_buf);
            if sentence_buf.is_empty() || paths.iter().any(|p| p.sentence == sentence_buf) {
                continue;
            }
            let leading_single_pair = n >= 4
                && max_per_prefix > 0
                && trace.len() >= 2
                && trace[0].end == 1
                && trace[1].end == 2;
            if leading_single_pair && leading_single_pair_count >= MAX_LEADING_SINGLE_PAIR_PASS1 {
                deferred.push(hyp_idx);
                continue;
            }
            let head_word = (head_cap > 0 && trace.len() >= 2).then(|| trace[0].word.as_str());
            if let Some(hw) = head_word {
                if count_head_str(&head_counts, hw) >= head_cap {
                    deferred.push(hyp_idx);
                    continue;
                }
            }
            let suffix = if suffix_cap > 0 && sentence_buf.chars().count() >= 3 {
                Some(crate::text::last_characters(&sentence_buf, 2))
            } else if suffix_cap > 0 && n == 2 && trace.len() == 2 {
                Some(crate::text::last_characters(&sentence_buf, 1))
            } else {
                None
            };
            if let Some(suf) = suffix {
                if count_str(&suffix_counts, suf) >= suffix_cap {
                    deferred.push(hyp_idx);
                    continue;
                }
            }
            let suffix_owned = suffix.map(str::to_owned);
            push_traced_path(hyp, &trace, &sentence_buf, n, &mut paths);
            if leading_single_pair {
                leading_single_pair_count += 1;
            }
            if prefix_cap > 0 && prefix.0 >= 1 {
                increment_pair(&mut prefix_counts, prefix);
            }
            if let Some(hw) = head_word {
                increment_head_str(&mut head_counts, hw);
            }
            if let Some(suf) = suffix_owned {
                increment_str(&mut suffix_counts, suf);
            }
            if seg_cap > 0 {
                increment_u64(&mut seg_counts, seg_sig);
            }
        }

        // 第二遍（先于 `deferred`）：采纳窗口外尚未在第一遍出现过的全新分词骨架路径，
        // 若已开启多字首词保底预留则同样受 `pass1_limit` 约束，保留最后 3 席给未见多字首词代表。
        let mut novel_admitted: Vec<usize> = Vec::new();
        if paths.len() < pass1_limit && pass1_end < last.len() {
            let pass1_segs: Vec<u64> = seg_counts.iter().map(|&(sig, _)| sig).collect();
            for (offset, hyp) in last[pass1_end..].iter().enumerate() {
                if paths.len() == pass1_limit {
                    break;
                }
                let seg_sig = segmentation_signature(hyp, &columns);
                if pass1_segs.contains(&seg_sig) {
                    continue;
                }
                let prefix = hyp.prev;
                if prefix_cap > 0
                    && prefix.0 >= 1
                    && count_pair(&prefix_counts, prefix) >= prefix_cap
                {
                    continue;
                }
                let this_seg_cap = if n == 2 && seg_cap > 0 && seg_sig == two_syl_phrase_mask {
                    TWO_SYLLABLE_PHRASE_PASS1_CAP
                } else {
                    seg_cap
                };
                if this_seg_cap > 0 && count_u64(&seg_counts, seg_sig) >= this_seg_cap {
                    continue;
                }
                trace_terminal_sentence(hyp, &columns, n, &mut trace, &mut sentence_buf);
                if sentence_buf.is_empty() || paths.iter().any(|p| p.sentence == sentence_buf) {
                    continue;
                }
                let head_word = (head_cap > 0 && trace.len() >= 2).then(|| trace[0].word.as_str());
                if let Some(hw) = head_word {
                    if count_head_str(&head_counts, hw) >= head_cap {
                        continue;
                    }
                }
                let suffix = if suffix_cap > 0 && sentence_buf.chars().count() >= 3 {
                    Some(crate::text::last_characters(&sentence_buf, 2))
                } else if suffix_cap > 0 && n == 2 && trace.len() == 2 {
                    Some(crate::text::last_characters(&sentence_buf, 1))
                } else {
                    None
                };
                if let Some(suf) = suffix {
                    if count_str(&suffix_counts, suf) >= suffix_cap {
                        continue;
                    }
                }
                let suffix_owned = suffix.map(str::to_owned);
                push_traced_path(hyp, &trace, &sentence_buf, n, &mut paths);
                novel_admitted.push(pass1_end + offset);
                if prefix_cap > 0 && prefix.0 >= 1 {
                    increment_pair(&mut prefix_counts, prefix);
                }
                if let Some(hw) = head_word {
                    increment_head_str(&mut head_counts, hw);
                }
                if let Some(suf) = suffix_owned {
                    increment_str(&mut suffix_counts, suf);
                }
                if seg_cap > 0 {
                    increment_u64(&mut seg_counts, seg_sig);
                }
            }
        }

        // 多音节首词保底通道：先于通用 `deferred` 回填，依次扫描 `deferred` 与 `last[pass1_end..]`
        // 抢救至多 3 条首词为双音节及以上（`trace[0].end >= 2`）且主束中尚无任何同首词路径的代表整句，
        // 避免 `[1, 1, 2]` 占满第一遍与后缀配额后将 `[2, 2]` 次优首词锚点彻底挤出主池。
        let mut head_rescued_indices = [usize::MAX; MULTI_CHAR_HEAD_RESCUE_LIMIT];
        let mut head_rescued_count = 0usize;
        if head_cap > 0 && paths.len() < take {
            let rescue_margin = margin.max(26.0);
            for hyp_idx in deferred
                .iter()
                .copied()
                .chain((pass1_end..last.len()).filter(|hyp_idx| !novel_admitted.contains(hyp_idx)))
            {
                if paths.len() == take || head_rescued_count == MULTI_CHAR_HEAD_RESCUE_LIMIT {
                    break;
                }
                let hyp = &last[hyp_idx];
                if (best_score - hyp.score) > rescue_margin {
                    if hyp_idx >= pass1_end {
                        break;
                    }
                    continue;
                }
                trace_terminal_sentence(hyp, &columns, n, &mut trace, &mut sentence_buf);
                if sentence_buf.is_empty()
                    || trace.len() < 2
                    || trace[0].end < 2
                    || paths.iter().any(|p| p.sentence == sentence_buf)
                {
                    continue;
                }
                let hw = trace[0].word.as_str();
                if paths
                    .iter()
                    .any(|p| p.words.first().map(String::as_str) == Some(hw))
                {
                    continue;
                }
                let suffix_owned = (suffix_cap > 0 && sentence_buf.chars().count() >= 3)
                    .then(|| crate::text::last_characters(&sentence_buf, 2).to_owned());
                push_traced_path(hyp, &trace, &sentence_buf, n, &mut paths);
                head_rescued_indices[head_rescued_count] = hyp_idx;
                head_rescued_count += 1;
                if prefix_cap > 0 && hyp.prev.0 >= 1 {
                    increment_pair(&mut prefix_counts, hyp.prev);
                }
                increment_head_str(&mut head_counts, hw);
                if let Some(suf) = suffix_owned {
                    increment_str(&mut suffix_counts, suf);
                }
                if seg_cap > 0 {
                    increment_u64(&mut seg_counts, segmentation_signature(hyp, &columns));
                }
            }
        }

        // 第三遍：仍有空位时，按分数顺序回填窗口内被暂缓的假设。
        for &hyp_idx in &deferred {
            if paths.len() == take {
                break;
            }
            if head_rescued_indices[..head_rescued_count].contains(&hyp_idx) {
                continue;
            }
            materialize_terminal_hyp(&last[hyp_idx], &columns, n, &mut trace, &mut paths, true);
        }

        // 第四遍：仍未集齐时，补齐窗口外其余假设。
        for (offset, hyp) in last[pass1_end..].iter().enumerate() {
            if paths.len() == take {
                break;
            }
            let hyp_idx = pass1_end + offset;
            if novel_admitted.contains(&hyp_idx)
                || head_rescued_indices[..head_rescued_count].contains(&hyp_idx)
            {
                continue;
            }
            materialize_terminal_hyp(hyp, &columns, n, &mut trace, &mut paths, true);
        }
        drop(trace);
    }
    let trigram_applied = rescore_with_trigram(&mut paths, options);
    let post_trigram_head_cap = if n >= 3 && max_per_prefix > 0 {
        max_per_prefix.saturating_add(1)
    } else {
        0
    };
    if trigram_applied
        && diversity_active
        && (post_trigram_suffix_cap > 0 || post_trigram_prefix_cap > 0 || post_trigram_head_cap > 0)
    {
        enforce_post_trigram_diversity(
            &mut paths,
            post_trigram_prefix_cap,
            post_trigram_suffix_cap,
            post_trigram_head_cap,
            options.terminal_diversity_margin,
        );
    }
    if diversity_active
        && options.terminal_max_per_suffix > 0
        && extra.is_none()
        && n >= 3
        && options.nbest >= 8
        && !paths.is_empty()
    {
        augment_with_confusion_network_perturbations(graph, options, &mut paths, options.nbest);
    }
    paths
}

fn count_pair(counts: &[((usize, usize), usize)], key: (usize, usize)) -> usize {
    counts
        .iter()
        .find(|(slot, _)| *slot == key)
        .map_or(0, |(_, count)| *count)
}

fn increment_pair(counts: &mut Vec<((usize, usize), usize)>, key: (usize, usize)) {
    if let Some((_, count)) = counts.iter_mut().find(|(slot, _)| *slot == key) {
        *count += 1;
    } else {
        counts.push((key, 1));
    }
}

fn count_u64(counts: &[(u64, usize)], key: u64) -> usize {
    counts
        .iter()
        .find(|(slot, _)| *slot == key)
        .map_or(0, |(_, count)| *count)
}

fn increment_u64(counts: &mut Vec<(u64, usize)>, key: u64) {
    if let Some((_, count)) = counts.iter_mut().find(|(slot, _)| *slot == key) {
        *count += 1;
    } else {
        counts.push((key, 1));
    }
}

fn count_head_str(counts: &[(&str, usize)], key: &str) -> usize {
    counts
        .iter()
        .find(|(slot, _)| *slot == key)
        .map_or(0, |(_, count)| *count)
}

fn increment_head_str<'a>(counts: &mut Vec<(&'a str, usize)>, key: &'a str) {
    if let Some((_, count)) = counts.iter_mut().find(|(slot, _)| *slot == key) {
        *count += 1;
    } else {
        counts.push((key, 1));
    }
}

fn count_str(counts: &[(String, usize)], key: &str) -> usize {
    counts
        .iter()
        .find(|(slot, _)| slot == key)
        .map_or(0, |(_, count)| *count)
}

fn increment_str(counts: &mut Vec<(String, usize)>, key: String) {
    if let Some((_, count)) = counts.iter_mut().find(|(slot, _)| *slot == key) {
        *count += 1;
    } else {
        counts.push((key, 1));
    }
}

/// 回溯计算整句路径的词边界签名（各词起始列的位掩码），用于限制单一分词骨架的同音笛卡尔积占满 `nbest`。
fn segmentation_signature<'g>(hyp: &Hyp<'g>, columns: &[BeamColumn<'g>]) -> u64 {
    let mut mask = 0u64;
    let mut cursor = Some(hyp);
    let mut shift = 0u32;
    while let Some(step) = cursor {
        if step.edge.is_none() {
            break;
        }
        let (column, at) = step.prev;
        if column < u64::BITS as usize {
            mask |= 1u64 << column;
        } else {
            mask ^= (column as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .rotate_left(shift & 63);
        }
        shift = shift.wrapping_add(7);
        cursor = Some(&columns[column].hyps[at]);
    }
    mask
}

/// 将单条末列假设回溯到 `trace` 并把拼接文本写入可复用缓冲 `sentence_buf`，用于多样性去重前检查。
fn trace_terminal_sentence<'g>(
    hyp: &Hyp<'g>,
    columns: &[BeamColumn<'g>],
    n: usize,
    trace: &mut Vec<&'g Edge>,
    sentence_buf: &mut String,
) {
    trace.clear();
    trace.reserve_exact(n);
    let mut cursor = Some(hyp);
    while let Some(step) = cursor {
        let Some(edge) = step.edge else { break };
        trace.push(edge);
        let (column, at) = step.prev;
        cursor = Some(&columns[column].hyps[at]);
    }
    trace.reverse();
    sentence_buf.clear();
    for edge in trace.iter() {
        sentence_buf.push_str(&edge.word);
    }
}

/// 将已完成回溯的假设物化为 `SentencePath` 追加进 `paths`。
fn push_traced_path<'g>(
    hyp: &Hyp<'g>,
    trace: &[&'g Edge],
    sentence: &str,
    n: usize,
    paths: &mut Vec<SentencePath>,
) {
    let mut words = Vec::with_capacity(n);
    let mut key = String::with_capacity(
        trace.iter().map(|edge| edge.key.len()).sum::<usize>() + trace.len().saturating_sub(1),
    );
    for (index, edge) in trace.iter().enumerate() {
        words.push(edge.word.clone());
        if index != 0 {
            key.push('\'');
        }
        key.push_str(&edge.key);
    }
    paths.push(SentencePath {
        sentence: sentence.to_owned(),
        key,
        log_prob: hyp.score,
        words,
        typo_edges: hyp.typo_edges,
    });
}

/// 三元重排重新按分数排序后，维护窗口内的末尾后缀、首词与同前缀配额，防止暂缓路径重新挤回 Top-3 引发后缀或首词塌缩。
fn enforce_post_trigram_diversity(
    paths: &mut Vec<SentencePath>,
    prefix_cap: usize,
    suffix_cap: usize,
    head_cap: usize,
    margin: f64,
) {
    if paths.len() <= 2 {
        return;
    }
    let best_score = paths[0].log_prob;
    let mut prefix_counts: Vec<(String, usize)> = Vec::new();
    let mut head_counts: Vec<(String, usize)> = Vec::new();
    let mut suffix_counts: Vec<(String, usize)> = Vec::new();
    let mut primary = Vec::with_capacity(paths.len());
    let mut deferred = Vec::new();
    let mut tail = Vec::new();

    for path in paths.drain(..) {
        if (best_score - path.log_prob) > margin {
            tail.push(path);
            continue;
        }
        let prefix_key = (prefix_cap > 0 && path.words.len() >= 2).then(|| {
            let mut buf = String::new();
            for word in &path.words[..path.words.len() - 1] {
                buf.push_str(word);
                buf.push('\'');
            }
            buf
        });
        let head_key = (head_cap > 0 && path.words.len() >= 2).then(|| path.words[0].clone());
        let suffix_key = (suffix_cap > 0 && path.sentence.chars().count() >= 3)
            .then(|| crate::text::last_characters(&path.sentence, 2).to_owned());
        let prefix_full = prefix_key
            .as_deref()
            .is_some_and(|key| count_str(&prefix_counts, key) >= prefix_cap);
        let head_full = head_key
            .as_deref()
            .is_some_and(|key| count_str(&head_counts, key) >= head_cap);
        let suffix_full = suffix_key
            .as_deref()
            .is_some_and(|key| count_str(&suffix_counts, key) >= suffix_cap);
        if prefix_full || head_full || suffix_full {
            deferred.push(path);
            continue;
        }
        if let Some(key) = prefix_key {
            increment_str(&mut prefix_counts, key);
        }
        if let Some(key) = head_key {
            increment_str(&mut head_counts, key);
        }
        if let Some(key) = suffix_key {
            increment_str(&mut suffix_counts, key);
        }
        primary.push(path);
    }
    // 对 `deferred` 中多字首词尚未出现在 `primary` 前排的代表路径，保底至多 2 条留在前 8 内，
    // 其余未见多字首词路径紧随其后排在 `deferred` 最前，防止被 `[1, 1, 2]` 占满后缀配额挤出 CN 锚点池。
    const POST_TRIGRAM_FRONT_ANCHORS: usize = 8;
    const POST_TRIGRAM_MULTI_HEAD_RESCUE: usize = 2;
    let front_end = primary.len().min(6);
    let mut front_heads: Vec<String> = Vec::with_capacity(POST_TRIGRAM_FRONT_ANCHORS);
    for p in &primary[..front_end] {
        if let Some(hw) = p.words.first() {
            if !front_heads.iter().any(|existing| existing == hw) {
                front_heads.push(hw.clone());
            }
        }
    }
    let mut rescue_front = Vec::with_capacity(POST_TRIGRAM_MULTI_HEAD_RESCUE);
    let mut deferred_unseen = Vec::new();
    let mut deferred_rest = Vec::new();
    let mut deferred_unseen_heads: Vec<String> = Vec::new();
    for path in deferred {
        let multi_head = (path.words.len() >= 2 && path.words[0].chars().count() >= 2)
            .then(|| path.words[0].as_str());
        let unseen_in_primary = multi_head.is_some_and(|hw| count_str(&head_counts, hw) == 0);
        let unseen_in_front =
            multi_head.is_some_and(|hw| !front_heads.iter().any(|existing| existing == hw));
        if let Some(hw) = multi_head {
            if (unseen_in_primary || unseen_in_front)
                && !deferred_unseen_heads.iter().any(|existing| existing == hw)
            {
                let owned_hw = hw.to_owned();
                deferred_unseen_heads.push(owned_hw.clone());
                if rescue_front.len() < POST_TRIGRAM_MULTI_HEAD_RESCUE
                    && primary.len() + rescue_front.len()
                        <= POST_TRIGRAM_FRONT_ANCHORS.max(primary.len())
                {
                    front_heads.push(owned_hw);
                    rescue_front.push(path);
                    continue;
                }
                deferred_unseen.push(path);
                continue;
            }
        }
        if unseen_in_primary {
            deferred_unseen.push(path);
        } else {
            deferred_rest.push(path);
        }
    }
    if !rescue_front.is_empty() {
        let insert_at = primary
            .len()
            .min(POST_TRIGRAM_FRONT_ANCHORS.saturating_sub(rescue_front.len()));
        primary.splice(insert_at..insert_at, rescue_front);
    }
    primary.extend(deferred_unseen);
    primary.extend(deferred_rest);
    primary.extend(tail);
    *paths = primary;
}

#[derive(Clone, Copy)]
struct AlignedSpan<'g> {
    start: usize,
    end: usize,
    edge: &'g Edge,
}

/// 将解码所得最优路径的词序列精确对齐回已建词网格 `graph` 的 `(start, end, &Edge)` 跨度。
fn align_path_to_graph<'g>(graph: &'g Graph, words: &[String]) -> Option<Vec<AlignedSpan<'g>>> {
    fn dfs<'g>(
        graph: &'g Graph,
        words: &[String],
        word_idx: usize,
        col: usize,
        out: &mut Vec<AlignedSpan<'g>>,
    ) -> bool {
        if word_idx == words.len() {
            return col == graph.len();
        }
        if col >= graph.len() {
            return false;
        }
        let target = &words[word_idx];
        let mut first: Option<&'g Edge> = None;
        let mut extras: Vec<&'g Edge> = Vec::new();
        for edge in &graph[col] {
            if edge.word != *target {
                continue;
            }
            match first {
                None => first = Some(edge),
                Some(prev) if prev.end == edge.end => {
                    if edge.log_prob > prev.log_prob {
                        first = Some(edge);
                    }
                }
                Some(_) => {
                    if let Some(slot) = extras.iter_mut().find(|e| e.end == edge.end) {
                        if edge.log_prob > slot.log_prob {
                            *slot = edge;
                        }
                    } else {
                        extras.push(edge);
                    }
                }
            }
        }
        if let Some(edge) = first {
            out.push(AlignedSpan {
                start: col,
                end: edge.end,
                edge,
            });
            if dfs(graph, words, word_idx + 1, edge.end, out) {
                return true;
            }
            out.pop();
        }
        for edge in extras {
            out.push(AlignedSpan {
                start: col,
                end: edge.end,
                edge,
            });
            if dfs(graph, words, word_idx + 1, edge.end, out) {
                return true;
            }
            out.pop();
        }
        false
    }

    let mut out = Vec::with_capacity(words.len());
    dfs(graph, words, 0, 0, &mut out).then_some(out)
}

/// 局部扰动专用的栈内 UTF-8 尾窗缓冲（最多 8 码点，零堆分配，不干扰 `CollocationTail` 测试计数）。
#[derive(Clone, Copy, Default)]
struct CnTail {
    bytes: [u8; COLLOCATION_TAIL_CODEPOINTS * 4],
    len: u8,
}

impl CnTail {
    #[inline]
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    fn push(&mut self, word: &str, max_chars: usize) {
        let max_chars = max_chars.min(COLLOCATION_TAIL_CODEPOINTS);
        if word.is_empty() || max_chars == 0 {
            return;
        }
        let word_cps = word.chars().count();
        if word_cps >= max_chars {
            let tail = crate::text::last_characters(word, max_chars);
            self.bytes[..tail.len()].copy_from_slice(tail.as_bytes());
            self.len = tail.len() as u8;
            return;
        }
        let keep = max_chars - word_cps;
        let (start, cur_len) = {
            let cur = self.as_str();
            let cur_cps = cur.chars().count();
            let skip = cur_cps.saturating_sub(keep);
            let start = cur
                .char_indices()
                .nth(skip)
                .map_or(cur.len(), |(idx, _)| idx);
            (start, cur.len())
        };
        let keep_bytes = cur_len - start;
        self.bytes.copy_within(start..cur_len, 0);
        self.bytes[keep_bytes..keep_bytes + word.len()].copy_from_slice(word.as_bytes());
        self.len = (keep_bytes + word.len()) as u8;
    }
}

#[derive(Clone, Copy)]
struct CnPerturbation<'g> {
    span_lo: usize,
    span_hi: usize,
    rep: [&'g Edge; 2],
    rep_len: usize,
    local_delta: f64,
}

impl<'g> CnPerturbation<'g> {
    #[inline]
    fn single(span_lo: usize, span_hi: usize, edge: &'g Edge, local_delta: f64) -> Self {
        Self {
            span_lo,
            span_hi,
            rep: [edge, edge],
            rep_len: 1,
            local_delta,
        }
    }

    #[inline]
    fn pair(
        span_lo: usize,
        span_hi: usize,
        first: &'g Edge,
        second: &'g Edge,
        local_delta: f64,
    ) -> Self {
        Self {
            span_lo,
            span_hi,
            rep: [first, second],
            rep_len: 2,
            local_delta,
        }
    }

    #[inline]
    fn is_structural(&self) -> bool {
        self.rep_len != (self.span_hi - self.span_lo) || self.span_hi > self.span_lo + 1
    }
}

struct CnContext<'a, 'opt, 'g> {
    graph: &'g Graph,
    aligned: &'a [AlignedSpan<'g>],
    word_bytes: &'a [usize],
    best: &'a SentencePath,
    n: usize,
    options: &'a LatticeOptions<'opt>,
    collocation: Option<&'opt CollocationScorer<'opt>>,
    colloc_max_chars: usize,
    bigram: Option<&'a NgramTable>,
    trigram: Option<&'a NgramTable>,
    personal: Option<&'opt PersonalNgram>,
    preceding_word: &'opt str,
}

impl<'a, 'opt, 'g> CnContext<'a, 'opt, 'g> {
    fn tail_before(&self, span_lo: usize) -> CnTail {
        let mut tail = CnTail::default();
        let prefix = &self.best.sentence[..self.word_bytes[span_lo]];
        let prefix_tail = crate::text::last_characters(prefix, self.colloc_max_chars);
        let prefix_cps = prefix_tail.chars().count();
        if prefix_cps < self.colloc_max_chars && !self.options.preceding_context.is_empty() {
            let ctx = crate::text::last_characters(
                self.options.preceding_context,
                self.colloc_max_chars - prefix_cps,
            );
            tail.push(ctx, self.colloc_max_chars);
        }
        tail.push(prefix_tail, self.colloc_max_chars);
        tail
    }

    #[inline]
    fn prev_word_before(&self, span_lo: usize) -> &str {
        if span_lo > 0 {
            self.aligned[span_lo - 1].edge.word.as_str()
        } else if self.preceding_word.is_empty() {
            SENTENCE_START
        } else {
            self.preceding_word
        }
    }

    #[inline]
    fn colloc_term(&self, tail: &str, word: &str, is_rear: bool) -> f64 {
        match self.collocation {
            Some(scorer) => {
                self.options.collocation_weight
                    * (scorer(tail, word, is_rear) + self.options.collocation_edge_offset)
            }
            None => 0.0,
        }
    }

    #[inline]
    fn bigram_term(&self, prev: &str, word: &str) -> f64 {
        match self.bigram {
            Some(table) => {
                self.options.bigram_weight * f64::from(table.bigram(prev, word)).max(0.0)
            }
            None => 0.0,
        }
    }

    /// 精确计算替换窗口 `aligned[span_lo..span_hi]`（含受其影响的后续 `.gram`/bigram/trigram/personal 上下文窗口）
    /// 的局部总分，使 `rep_score - orig_score` 与整句重解码的分数差严格一致。
    fn score_window(&self, span_lo: usize, span_hi: usize, edges: &[&'g Edge]) -> f64 {
        let mut tail = self.tail_before(span_lo);
        let mut prev_word: &str = self.prev_word_before(span_lo);
        let (mut personal_before, mut personal_prev): (Option<&str>, &str) = match span_lo {
            0 => (None, SENTENCE_START),
            1 => (None, self.aligned[0].edge.word.as_str()),
            _ => (
                Some(self.aligned[span_lo - 2].edge.word.as_str()),
                self.aligned[span_lo - 1].edge.word.as_str(),
            ),
        };
        let (mut trigram_before, mut trigram_prev): (&str, &str) = match span_lo {
            0 => (
                SENTENCE_START,
                if !self.preceding_word.is_empty() {
                    self.preceding_word
                } else {
                    SENTENCE_START
                },
            ),
            1 => (
                if !self.preceding_word.is_empty() {
                    self.preceding_word
                } else {
                    SENTENCE_START
                },
                self.aligned[0].edge.word.as_str(),
            ),
            _ => (
                self.aligned[span_lo - 2].edge.word.as_str(),
                self.aligned[span_lo - 1].edge.word.as_str(),
            ),
        };
        let mut word_index = span_lo;
        let mut score = 0.0;

        // 计算受替换词影响的后续固定词数量（覆盖 trigram/personal 及完整 `.gram` 尾窗字符数）。
        let mut suffix_count = 0usize;
        let mut suffix_chars = 0usize;
        for span in &self.aligned[span_hi..] {
            let need_trigram = suffix_count < 2
                && (self.trigram.is_some()
                    || self.personal.is_some()
                    || (suffix_count == 0 && self.bigram.is_some()));
            let need_colloc = self.collocation.is_some() && suffix_chars < self.colloc_max_chars;
            if !need_trigram && !need_colloc {
                break;
            }
            suffix_chars += span.edge.word.chars().count();
            suffix_count += 1;
        }

        let steps = edges.iter().copied().map(|edge| (true, edge)).chain(
            self.aligned[span_hi..span_hi + suffix_count]
                .iter()
                .map(|span| (false, span.edge)),
        );
        for (add_unigram, edge) in steps {
            if add_unigram {
                score += edge.log_prob;
            }
            if let Some(scorer) = self.collocation {
                let raw = scorer(tail.as_str(), &edge.word, edge.end == self.n);
                score +=
                    self.options.collocation_weight * (raw + self.options.collocation_edge_offset);
            }
            let static_bonus = self
                .bigram
                .map_or(0.0, |table| f64::from(table.bigram(prev_word, &edge.word)));
            if self.bigram.is_some() {
                score += self.options.bigram_weight * static_bonus.max(0.0);
            }
            if let Some(personal) = self.personal {
                let ctx = personal.context(personal_before, Some(personal_prev));
                if ctx.confidence > 0.0 {
                    score +=
                        personal_transition(personal, &ctx, &edge.word, static_bonus, self.options);
                }
            }
            if let Some(trigram) = self.trigram {
                if self.options.trigram_weight != 0.0
                    && (self.preceding_word.is_empty() || word_index > 0)
                {
                    score += self.options.trigram_weight
                        * f64::from(trigram.trigram(trigram_before, trigram_prev, &edge.word));
                }
            }
            tail.push(&edge.word, self.colloc_max_chars);
            prev_word = edge.word.as_str();
            if word_index == 0 {
                personal_before = None;
                personal_prev = edge.word.as_str();
            } else {
                personal_before = Some(personal_prev);
                personal_prev = edge.word.as_str();
            }
            trigram_before = trigram_prev;
            trigram_prev = edge.word.as_str();
            word_index += 1;
        }
        score
    }

    /// 双向 2-stage 局部 DP：对跨度 `(s..e)` 在切点 `mid` 搜索最优两词组合 `(e1, e2)`，
    /// 同步结合左上下文 (`left_tail -> e1`)、内部转移 (`e1 -> e2`) 与右邻词前瞻 (`e2 -> next`)。
    #[allow(clippy::too_many_arguments)]
    fn collect_two_edge_splits(
        &self,
        span_lo: usize,
        span_hi: usize,
        s: usize,
        mid: usize,
        e: usize,
        orig_score: f64,
        max_left: usize,
        full_right_cap: usize,
        secondary_right_cap: usize,
        top_pairs_cap: usize,
        bucket: &mut Vec<CnPerturbation<'g>>,
        primary: &[SentencePath],
    ) {
        let tail_before = self.tail_before(span_lo);
        let tail_before_str = tail_before.as_str();
        let prev_word = self.prev_word_before(span_lo);

        let mut left_cands: Vec<(&'g Edge, f64)> = self.graph[s]
            .iter()
            .filter(|edge| edge.end == mid)
            .map(|edge| {
                let s1 = edge.log_prob
                    + self.colloc_term(tail_before_str, &edge.word, false)
                    + self.bigram_term(prev_word, &edge.word);
                (edge, s1)
            })
            .collect();
        if left_cands.is_empty() {
            return;
        }
        left_cands.sort_by(|a, b| b.1.total_cmp(&a.1));
        left_cands.truncate(max_left);

        let next_span = self.aligned.get(span_hi);
        let mut right_cands: Vec<(&'g Edge, f64)> = self.graph[mid]
            .iter()
            .filter(|edge| edge.end == e)
            .map(|edge| {
                // 预计算右邻词前瞻分（只做一遍 O(K)），使被右文强预测的中频单字（如「补 -> 政策」「洗 -> 掉」）也能稳进前排。
                let lookahead = next_span.map_or(0.0, |next| {
                    let is_next_rear = next.end == self.n;
                    self.bigram_term(&edge.word, &next.edge.word)
                        + self.colloc_term(&edge.word, &next.edge.word, is_next_rear)
                });
                (edge, edge.log_prob + lookahead)
            })
            .collect();
        if right_cands.is_empty() {
            return;
        }
        right_cands.sort_by(|a, b| b.1.total_cmp(&a.1));

        let mut top_pairs: Vec<(f64, &'g Edge, &'g Edge)> = Vec::with_capacity(top_pairs_cap + 1);
        for (li, &(e1, s1)) in left_cands.iter().enumerate() {
            let right_limit = if li < 6 {
                right_cands.len().min(full_right_cap)
            } else {
                right_cands.len().min(secondary_right_cap)
            };
            if right_limit == 0 {
                continue;
            }
            let mut tail_mid = tail_before;
            tail_mid.push(&e1.word, self.colloc_max_chars);
            let tail_mid_str = tail_mid.as_str();
            for &(e2, r_prior) in &right_cands[..right_limit] {
                let quick_score = s1
                    + r_prior
                    + self.colloc_term(tail_mid_str, &e2.word, e == self.n)
                    + self.bigram_term(&e1.word, &e2.word);
                if top_pairs.len() < top_pairs_cap {
                    top_pairs.push((quick_score, e1, e2));
                } else if let Some((min_idx, &(min_score, _, _))) = top_pairs
                    .iter()
                    .enumerate()
                    .min_by(|a, b| (a.1).0.total_cmp(&(b.1).0))
                {
                    if quick_score > min_score {
                        top_pairs[min_idx] = (quick_score, e1, e2);
                    }
                }
            }
        }

        top_pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, e1, e2) in top_pairs {
            let delta = self.score_window(span_lo, span_hi, &[e1, e2]) - orig_score;
            offer_cn_perturbation(
                bucket,
                CnPerturbation::pair(span_lo, span_hi, e1, e2, delta),
                self.aligned,
                self.best,
                self.word_bytes,
                primary,
            );
        }
    }
}

fn matches_sentence_parts(sentence: &str, prefix: &str, rep: &[&Edge], suffix: &str) -> bool {
    let rep_bytes: usize = rep.iter().map(|edge| edge.word.len()).sum();
    if sentence.len() != prefix.len() + rep_bytes + suffix.len() {
        return false;
    }
    if !sentence.starts_with(prefix) || !sentence.ends_with(suffix) {
        return false;
    }
    let mut mid = &sentence[prefix.len()..sentence.len() - suffix.len()];
    for edge in rep {
        let Some(rest) = mid.strip_prefix(edge.word.as_str()) else {
            return false;
        };
        mid = rest;
    }
    mid.is_empty()
}

fn cn_segment_bytes<'a, 'g>(
    aligned: &'a [AlignedSpan<'g>],
    cand: &'a CnPerturbation<'g>,
    hi: usize,
) -> impl Iterator<Item = u8> + 'a {
    cand.rep[..cand.rep_len]
        .iter()
        .flat_map(|edge| edge.word.bytes())
        .chain(
            aligned[cand.span_hi..hi]
                .iter()
                .flat_map(|span| span.edge.word.bytes()),
        )
}

fn offer_cn_perturbation<'g>(
    bucket: &mut Vec<CnPerturbation<'g>>,
    cand: CnPerturbation<'g>,
    aligned: &[AlignedSpan<'g>],
    best: &SentencePath,
    word_bytes: &[usize],
    primary: &[SentencePath],
) {
    if !cand.local_delta.is_finite() || cand.local_delta < -CN_MAX_LOCAL_DROP {
        return;
    }
    let rep = &cand.rep[..cand.rep_len];
    let orig_bytes = aligned[cand.span_lo..cand.span_hi]
        .iter()
        .flat_map(|span| span.edge.word.bytes());
    let rep_bytes = rep.iter().flat_map(|edge| edge.word.bytes());
    if orig_bytes.eq(rep_bytes) {
        return;
    }
    let prefix = &best.sentence[..word_bytes[cand.span_lo]];
    let suffix = &best.sentence[word_bytes[cand.span_hi]..];
    if primary
        .iter()
        .any(|path| matches_sentence_parts(&path.sentence, prefix, rep, suffix))
    {
        return;
    }
    if let Some(existing) = bucket.iter_mut().find(|other| {
        let hi = other.span_hi.max(cand.span_hi);
        cn_segment_bytes(aligned, other, hi).eq(cn_segment_bytes(aligned, &cand, hi))
    }) {
        if cand.local_delta > existing.local_delta {
            *existing = cand;
        }
        return;
    }
    bucket.push(cand);
}

fn finalize_cn_bucket(bucket: &mut Vec<CnPerturbation<'_>>) {
    bucket.sort_by(|a, b| b.local_delta.total_cmp(&a.local_delta));
    if bucket.len() <= CN_MAX_RIVALS_PER_POS {
        return;
    }
    let has_structural_in_top = bucket[..CN_MAX_RIVALS_PER_POS]
        .iter()
        .any(|cand| cand.is_structural());
    if !has_structural_in_top {
        if let Some(rel) = bucket[CN_MAX_RIVALS_PER_POS..]
            .iter()
            .position(|cand| cand.is_structural())
        {
            let best_structural = bucket.remove(CN_MAX_RIVALS_PER_POS + rel);
            bucket.truncate(CN_MAX_RIVALS_PER_POS);
            bucket.push(best_structural);
            return;
        }
    }
    bucket.truncate(CN_MAX_RIVALS_PER_POS);
}

fn materialize_cn_perturbation(
    aligned: &[AlignedSpan<'_>],
    best: &SentencePath,
    cand: &CnPerturbation<'_>,
) -> SentencePath {
    let rep = &cand.rep[..cand.rep_len];
    let edge_count = cand.span_lo + rep.len() + (aligned.len() - cand.span_hi);
    let mut sentence = String::with_capacity(best.sentence.len() + 8);
    let mut key = String::with_capacity(best.key.len() + 8);
    let mut words = Vec::with_capacity(edge_count);
    let mut first = true;
    for edge in aligned[..cand.span_lo]
        .iter()
        .map(|span| span.edge)
        .chain(rep.iter().copied())
        .chain(aligned[cand.span_hi..].iter().map(|span| span.edge))
    {
        sentence.push_str(&edge.word);
        words.push(edge.word.clone());
        if !first {
            key.push('\'');
        }
        first = false;
        key.push_str(&edge.key);
    }
    SentencePath {
        sentence,
        key,
        log_prob: best.log_prob + cand.local_delta,
        words,
        typo_edges: 0,
    }
}

struct CnAnchorData<'a, 'g> {
    anchor_path: std::borrow::Cow<'a, SentencePath>,
    aligned: Vec<AlignedSpan<'g>>,
}

/// 基于单个锚点路径收集其全部 1-Span 局部扰动并写入按 `(round, anchor_idx, -total_log_prob)` 排序用的候选列表。
#[allow(clippy::too_many_arguments)]
fn collect_cn_candidates_for_anchor<'g>(
    graph: &'g Graph,
    options: &LatticeOptions<'_>,
    anchor: &SentencePath,
    primary_slice: &[SentencePath],
    min_total_log_prob: f64,
    anchor_idx: usize,
    collocation: Option<&'g CollocationScorer<'g>>,
    colloc_max_chars: usize,
    bigram: Option<&NgramTable>,
    trigram: Option<&NgramTable>,
    personal: Option<&PersonalNgram>,
    preceding_word: &str,
    ranked: &mut Vec<(usize, usize, f64, CnPerturbation<'g>)>,
) -> Option<Vec<AlignedSpan<'g>>> {
    let aligned = align_path_to_graph(graph, &anchor.words)?;
    let m = aligned.len();
    if m == 0 {
        return None;
    }

    let mut word_bytes = Vec::with_capacity(m + 1);
    let mut byte_cursor = 0usize;
    word_bytes.push(0);
    for span in &aligned {
        byte_cursor += span.edge.word.len();
        word_bytes.push(byte_cursor);
    }
    if byte_cursor != anchor.sentence.len() {
        return None;
    }

    let ctx = CnContext {
        graph,
        aligned: &aligned,
        word_bytes: &word_bytes,
        best: anchor,
        n: graph.len(),
        options,
        collocation,
        colloc_max_chars,
        bigram,
        trigram,
        personal,
        preceding_word,
    };
    let mut buckets: Vec<Vec<CnPerturbation<'_>>> = (0..m).map(|_| Vec::new()).collect();

    for i in 0..m {
        let span = aligned[i];
        let s = span.start;
        let e = span.end;
        let len = e - s;
        let orig_score = ctx.score_window(i, i + 1, &[span.edge]);

        // 算子 A：同跨度竞争词边替换（扫描全部 `graph[s]` 中落点为 `e` 的同音词/字）。
        for edge in &graph[s] {
            if edge.end != e || edge.word == span.edge.word {
                continue;
            }
            let delta = ctx.score_window(i, i + 1, &[edge]) - orig_score;
            offer_cn_perturbation(
                &mut buckets[i],
                CnPerturbation::single(i, i + 1, edge, delta),
                &aligned,
                anchor,
                &word_bytes,
                primary_slice,
            );
        }

        // 算子 B：双音节整词拆分为 `[1, 1]`（双向语境 2-stage DP），扩大左/右配额以覆盖「之 + 石」等中频搭配组合。
        if len == 2 {
            ctx.collect_two_edge_splits(
                i,
                i + 1,
                s,
                s + 1,
                e,
                orig_score,
                10,
                28,
                16,
                12,
                &mut buckets[i],
                primary_slice,
            );
        }

        // 算子 D1：三音节整词拆分为 `[1, 2]` 与 `[2, 1]`。
        if len == 3 {
            ctx.collect_two_edge_splits(
                i,
                i + 1,
                s,
                s + 1,
                e,
                orig_score,
                4,
                8,
                0,
                6,
                &mut buckets[i],
                primary_slice,
            );
            ctx.collect_two_edge_splits(
                i,
                i + 1,
                s,
                s + 2,
                e,
                orig_score,
                4,
                8,
                0,
                6,
                &mut buckets[i],
                primary_slice,
            );
        }

        // 相邻双词跨度算子（`[1, 1] -> [2]` 合并与 `[2, 1] / [1, 2]` 三音节局部重切/合并）。
        if i + 1 < m {
            let next = aligned[i + 1];
            let next_len = next.end - next.start;
            let pair_len = len + next_len;
            if pair_len == 2 {
                // 算子 C：相邻两单字合并为双字词 `[1, 1] -> [2]`。
                let orig_pair_score = ctx.score_window(i, i + 2, &[span.edge, next.edge]);
                for edge in &graph[s] {
                    if edge.end != next.end {
                        continue;
                    }
                    let delta = ctx.score_window(i, i + 2, &[edge]) - orig_pair_score;
                    offer_cn_perturbation(
                        &mut buckets[i],
                        CnPerturbation::single(i, i + 2, edge, delta),
                        &aligned,
                        anchor,
                        &word_bytes,
                        primary_slice,
                    );
                }
                // 算子 C2：相邻两单字联合重解码 `[1, 1] -> [1, 1]`（双向 2-stage DP），
                // 解锁首词锚点替换后需两单字协同搭配才能命中的非词典组合（如「贤者 + 知 + 事」->「贤者 + 之 + 石」）。
                ctx.collect_two_edge_splits(
                    i,
                    i + 2,
                    s,
                    s + 1,
                    next.end,
                    orig_pair_score,
                    10,
                    28,
                    16,
                    12,
                    &mut buckets[i],
                    primary_slice,
                );
            } else if pair_len == 3 {
                // 算子 D2/D3：三音节 `[2, 1]` 或 `[1, 2]` 合并为 `[3]`，或翻转切点重切为 `[1, 2]`/`[2, 1]`。
                let orig_pair_score = ctx.score_window(i, i + 2, &[span.edge, next.edge]);
                for edge in &graph[s] {
                    if edge.end != next.end {
                        continue;
                    }
                    let delta = ctx.score_window(i, i + 2, &[edge]) - orig_pair_score;
                    offer_cn_perturbation(
                        &mut buckets[i],
                        CnPerturbation::single(i, i + 2, edge, delta),
                        &aligned,
                        anchor,
                        &word_bytes,
                        primary_slice,
                    );
                }
                let alt_mid = if len == 2 { s + 1 } else { s + 2 };
                ctx.collect_two_edge_splits(
                    i,
                    i + 2,
                    s,
                    alt_mid,
                    next.end,
                    orig_pair_score,
                    4,
                    8,
                    0,
                    6,
                    &mut buckets[i],
                    primary_slice,
                );
            }
        }

        finalize_cn_bucket(&mut buckets[i]);
    }

    // 按位置轮询排序：优先服务 Top-6 中尚未出现任何变体的前/中段位置（`base_round = 0`），
    // 已在 Top-6 中产生变体的尾部位置自动后移轮次；把 `[1, 1, ...]` 非成词单字起手轮次后移 1 轮，
    // 防止高熵单字同音替换在 `round = 0` 抢走多字首词锚点（如 `贤者之石`）的 Top-16 席位。
    for (i, bucket) in buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        let prefix = &anchor.sentence[..word_bytes[i]];
        let suffix = &anchor.sentence[word_bytes[i + 1]..];
        let existing_variants = primary_slice
            .iter()
            .filter(|path| {
                path.sentence != anchor.sentence
                    && path.sentence.starts_with(prefix)
                    && path.sentence.ends_with(suffix)
                    && path.sentence.len() > prefix.len() + suffix.len()
            })
            .count();
        let base_round = existing_variants.min(2);
        for (rank, &cand) in bucket.iter().enumerate() {
            let total_log_prob = anchor.log_prob + cand.local_delta;
            if total_log_prob < min_total_log_prob {
                continue;
            }
            let leading_single_pair = cand.span_lo == 0
                && cand.rep[0].end == 1
                && ((cand.rep_len >= 2 && cand.rep[1].end == 2)
                    || (cand.rep_len == 1
                        && aligned
                            .get(cand.span_hi)
                            .is_some_and(|next| next.end - next.start == 1)));
            let round = base_round + rank + usize::from(leading_single_pair);
            ranked.push((round, anchor_idx, total_log_prob, cand));
        }
    }

    Some(aligned)
}

/// 混淆网络 1-Span 局部扰动：复用已建词网格 `graph`，沿 1-best 主路径、不同多字首词次选锚点及
/// `graph[0]` 高分多音节首词种子，生成同跨度同音、`[2] -> [1, 1]` 拆字、`[1, 1] -> [2]` 合并与三音节局部重切替补，
/// 按轮询公平注入 Top-16 神经融合池（同时保留前 6 条束搜索主路径不动），
/// 消除前段同音领跑路径对中后段搭配词的锁死。
#[allow(clippy::too_many_arguments)]
fn build_multi_head_seed_anchor<'g>(
    graph: &'g Graph,
    options: &LatticeOptions<'_>,
    lead_edge: &'g Edge,
    base_candidates: &[SentencePath],
    collocation: Option<&'g CollocationScorer<'g>>,
    colloc_max_chars: usize,
    bigram: Option<&NgramTable>,
    trigram: Option<&NgramTable>,
    personal: Option<&PersonalNgram>,
    preceding_word: &str,
) -> Option<SentencePath> {
    for base_path in base_candidates {
        let Some(base_aligned) = align_path_to_graph(graph, &base_path.words) else {
            continue;
        };
        let Some(split_span_hi) = base_aligned
            .iter()
            .enumerate()
            .position(|(idx, span)| idx > 0 && span.start == lead_edge.end)
        else {
            continue;
        };
        let mut word_bytes = Vec::with_capacity(base_aligned.len() + 1);
        let mut byte_cursor = 0usize;
        word_bytes.push(0);
        for span in &base_aligned {
            byte_cursor += span.edge.word.len();
            word_bytes.push(byte_cursor);
        }
        if byte_cursor != base_path.sentence.len() {
            continue;
        }
        let ctx = CnContext {
            graph,
            aligned: &base_aligned,
            word_bytes: &word_bytes,
            best: base_path,
            n: graph.len(),
            options,
            collocation,
            colloc_max_chars,
            bigram,
            trigram,
            personal,
            preceding_word,
        };
        let orig_edges: Vec<&Edge> = base_aligned[..split_span_hi]
            .iter()
            .map(|span| span.edge)
            .collect();
        let orig_score = ctx.score_window(0, split_span_hi, &orig_edges);
        let new_score = ctx.score_window(0, split_span_hi, &[lead_edge]);
        let delta = new_score - orig_score;
        if !delta.is_finite() {
            continue;
        }
        let cand = CnPerturbation::single(0, split_span_hi, lead_edge, delta);
        let seed = materialize_cn_perturbation(&base_aligned, base_path, &cand);
        if seed.log_prob.is_finite() {
            return Some(seed);
        }
    }
    None
}

fn augment_with_confusion_network_perturbations(
    graph: &Graph,
    options: &LatticeOptions<'_>,
    paths: &mut Vec<SentencePath>,
    take: usize,
) {
    let keep_primary = CN_KEEP_PRIMARY_PATHS.min(paths.len()).min(take);
    let top16_target = (keep_primary + CN_TOP16_RESERVE).min(take);
    let top16_cn_budget = top16_target.saturating_sub(keep_primary);
    if top16_cn_budget == 0 {
        return;
    }
    let extra_cn_budget = if take > top16_target {
        CN_EXTRA_POOL_RESERVE.min(take - top16_target)
    } else {
        0
    };
    let total_cn_budget = top16_cn_budget + extra_cn_budget;

    let collocation = options
        .collocation_scorer
        .filter(|_| options.collocation_weight > 0.0);
    let colloc_max_chars = if options.collocation_edge_offset != 0.0 {
        5
    } else {
        COLLOCATION_TAIL_CODEPOINTS
    };
    let bigram = options.bigram.as_deref();
    let trigram = options.trigram.as_deref();
    let personal = active_personal(options);
    let preceding_word = crate::text::last_characters(options.preceding_context, 2);

    let best_log_prob = paths[0].log_prob;
    let min_total_log_prob = best_log_prob - CN_MAX_LOCAL_DROP;
    const MAX_EXTRA_ANCHORS: usize = 4;
    let scan_end = paths.len().min(10);
    let mut anchor_candidates: Vec<(std::borrow::Cow<'_, SentencePath>, bool)> =
        Vec::with_capacity(1 + MAX_EXTRA_ANCHORS);
    anchor_candidates.push((std::borrow::Cow::Borrowed(&paths[0]), false));
    let has_head = |chosen: &[(std::borrow::Cow<'_, SentencePath>, bool)], hw: &str| -> bool {
        chosen
            .iter()
            .any(|(p, _)| p.words.first().map(String::as_str) == Some(hw))
    };

    // 1. 优先吸收 `paths[1..scan_end]` 中不同多字首词的代表路径。
    for path in &paths[1..scan_end] {
        if anchor_candidates.len() > MAX_EXTRA_ANCHORS {
            break;
        }
        let Some(head) = path.words.first() else {
            continue;
        };
        if path.log_prob < min_total_log_prob
            || head.chars().count() < 2
            || has_head(&anchor_candidates, head)
        {
            continue;
        }
        anchor_candidates.push((std::borrow::Cow::Borrowed(path), false));
    }

    // 2. 若仍未集齐多字首词锚点，直接从 `graph[0]` 提取前 3 条高先验多音节首词边，
    //    对齐 `paths[..scan_end]` 的后段边界合成种子路径（如「贤者」+「只是」->「贤者只是」）。
    if anchor_candidates.len() <= MAX_EXTRA_ANCHORS {
        let mut lead_edges: Vec<&Edge> = graph[0]
            .iter()
            .filter(|edge| !edge.typo && edge.end >= 2 && edge.end < graph.len())
            .collect();
        lead_edges.sort_by(|a, b| b.log_prob.total_cmp(&a.log_prob));
        let mut top_lead_edges: Vec<&Edge> = Vec::with_capacity(3);
        for edge in lead_edges {
            if top_lead_edges
                .iter()
                .any(|existing| existing.word == edge.word && existing.end == edge.end)
            {
                continue;
            }
            top_lead_edges.push(edge);
            if top_lead_edges.len() == 3 {
                break;
            }
        }
        for lead_edge in top_lead_edges {
            if anchor_candidates.len() > MAX_EXTRA_ANCHORS {
                break;
            }
            if has_head(&anchor_candidates, &lead_edge.word) {
                continue;
            }
            if let Some(seed) = build_multi_head_seed_anchor(
                graph,
                options,
                lead_edge,
                &paths[..scan_end],
                collocation,
                colloc_max_chars,
                bigram,
                trigram,
                personal,
                preceding_word,
            ) {
                if seed.log_prob >= min_total_log_prob - 2.0 {
                    anchor_candidates.push((std::borrow::Cow::Owned(seed), true));
                }
            }
        }
    }

    // 3. 若仍有空余锚点席位，再从 `paths[1..scan_end]` 补入不同单字首词路径。
    for path in &paths[1..scan_end] {
        if anchor_candidates.len() > MAX_EXTRA_ANCHORS {
            break;
        }
        let Some(head) = path.words.first() else {
            continue;
        };
        if path.log_prob < min_total_log_prob
            || head.chars().count() >= 2
            || has_head(&anchor_candidates, head)
        {
            continue;
        }
        anchor_candidates.push((std::borrow::Cow::Borrowed(path), false));
    }

    let primary_slice = &paths[..keep_primary];
    let mut ranked: Vec<(usize, usize, f64, CnPerturbation<'_>)> = Vec::new();
    let mut anchors: Vec<CnAnchorData<'_, '_>> = Vec::with_capacity(anchor_candidates.len());

    for (anchor_path, is_seed) in anchor_candidates {
        let anchor_idx = anchors.len();
        if let Some(aligned) = collect_cn_candidates_for_anchor(
            graph,
            options,
            anchor_path.as_ref(),
            primary_slice,
            min_total_log_prob,
            anchor_idx,
            collocation,
            colloc_max_chars,
            bigram,
            trigram,
            personal,
            preceding_word,
            &mut ranked,
        ) {
            if is_seed
                && anchor_path.log_prob >= min_total_log_prob
                && !primary_slice
                    .iter()
                    .any(|p| p.sentence == anchor_path.sentence)
            {
                ranked.push((
                    1,
                    anchor_idx,
                    anchor_path.log_prob,
                    CnPerturbation::single(0, 1, aligned[0].edge, 0.0),
                ));
            }
            anchors.push(CnAnchorData {
                anchor_path,
                aligned,
            });
        }
    }
    if anchors.is_empty() || ranked.is_empty() {
        return;
    }

    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| b.2.total_cmp(&a.2))
    });

    let mut cn_selected: Vec<SentencePath> = Vec::with_capacity(total_cn_budget);
    for (_, anchor_idx, _, cand) in ranked {
        if cn_selected.len() == total_cn_budget {
            break;
        }
        let anchor = &anchors[anchor_idx];
        let path = materialize_cn_perturbation(&anchor.aligned, anchor.anchor_path.as_ref(), &cand);
        if primary_slice.iter().any(|p| p.sentence == path.sentence)
            || cn_selected.iter().any(|p| p.sentence == path.sentence)
        {
            continue;
        }
        cn_selected.push(path);
    }
    drop(anchors);
    if cn_selected.is_empty() {
        return;
    }

    // 组装最终路径：
    // 1. `[0..keep_primary]`：Top-6 束搜索主路径保持原序不动；
    // 2. 紧接插入至多 10 条轮询 1-Span 扰动进入 Top-16（直达 Tier 1 神经池）；
    // 3. 保留至多 6 条其余束搜索路径，随后补入余下轮询扰动，最后以剩余束搜索路径填满 `take`。
    let mut drained = std::mem::take(paths).into_iter();
    let mut result: Vec<SentencePath> = Vec::with_capacity(take);
    for _ in 0..keep_primary {
        if let Some(path) = drained.next() {
            result.push(path);
        }
    }
    let mut rest_beam: Vec<Option<SentencePath>> = drained.map(Some).collect();

    let mut cn_iter = cn_selected.into_iter();
    for path in cn_iter.by_ref().take(top16_cn_budget) {
        if result.len() == take {
            break;
        }
        result.push(path);
    }
    let top16_len = result.len();
    if keep_primary > 0 && top16_len > keep_primary {
        let bubble_suffix_cap = options.terminal_max_per_suffix;
        let bubble_head_cap = if graph.len() >= 3 && options.terminal_max_per_prefix > 0 {
            options.terminal_max_per_prefix.saturating_add(1)
        } else {
            0
        };
        for idx in keep_primary..top16_len {
            let floor_lp = result[keep_primary - 1].log_prob;
            if result[idx].log_prob <= floor_lp {
                continue;
            }
            let cand_suffix = (bubble_suffix_cap > 0 && result[idx].sentence.chars().count() >= 3)
                .then(|| crate::text::last_characters(&result[idx].sentence, 2));
            let suffix_ok = cand_suffix.is_none_or(|suf| {
                result[..keep_primary - 1]
                    .iter()
                    .filter(|p| {
                        p.sentence.chars().count() >= 3
                            && crate::text::last_characters(&p.sentence, 2) == suf
                    })
                    .count()
                    < bubble_suffix_cap
            });
            let cand_head = (bubble_head_cap > 0 && result[idx].words.len() >= 2)
                .then(|| result[idx].words[0].as_str());
            let head_ok = cand_head.is_none_or(|hw| {
                result[..keep_primary - 1]
                    .iter()
                    .filter(|p| {
                        p.words.len() >= 2 && p.words.first().map(String::as_str) == Some(hw)
                    })
                    .count()
                    < bubble_head_cap
            });
            if suffix_ok && head_ok {
                result[keep_primary..=idx].rotate_right(1);
                let mut pos = keep_primary;
                while pos > 0 && result[pos].log_prob > result[pos - 1].log_prob {
                    result.swap(pos, pos - 1);
                    pos -= 1;
                }
            }
        }
    }

    let mut front_beam_added = 0usize;
    for slot in &mut rest_beam {
        if result.len() == take || front_beam_added >= CN_REST_BEAM_FRONT_KEEP {
            break;
        }
        let Some(path) = slot.as_ref() else {
            continue;
        };
        if result
            .iter()
            .any(|existing| existing.sentence == path.sentence)
        {
            *slot = None;
            continue;
        }
        if let Some(owned) = slot.take() {
            result.push(owned);
            front_beam_added += 1;
        }
    }

    for path in cn_iter {
        if result.len() == take {
            break;
        }
        if result
            .iter()
            .any(|existing| existing.sentence == path.sentence)
        {
            continue;
        }
        result.push(path);
    }

    for path in rest_beam.into_iter().flatten() {
        if result.len() == take {
            break;
        }
        if result
            .iter()
            .any(|existing| existing.sentence == path.sentence)
        {
            continue;
        }
        result.push(path);
    }

    *paths = result;
}

fn materialize_terminal_hyp<'g>(
    hyp: &Hyp<'g>,
    columns: &[BeamColumn<'g>],
    n: usize,
    trace: &mut Vec<&'g Edge>,
    paths: &mut Vec<SentencePath>,
    inline_unique: bool,
) -> bool {
    // 图在回溯期间不变；首条实际访问的路径才申请引用页，后续路径复用它。
    trace.clear();
    trace.reserve_exact(n);
    let mut cursor = Some(hyp);
    while let Some(step) = cursor {
        let Some(edge) = step.edge else { break };
        trace.push(edge);
        let (column, at) = step.prev;
        cursor = Some(&columns[column].hyps[at]);
    }
    trace.reverse();
    let mut sentence = String::with_capacity(trace.iter().map(|edge| edge.word.len()).sum());
    for edge in trace.iter() {
        sentence.push_str(&edge.word);
    }
    if sentence.is_empty() || (inline_unique && paths.iter().any(|path| path.sentence == sentence))
    {
        return false;
    }
    // 保留的路径才复制拥有词；按边数连接读音，空读音也保留原分隔符。
    let mut words = Vec::with_capacity(n);
    let mut key = String::with_capacity(
        trace.iter().map(|edge| edge.key.len()).sum::<usize>() + trace.len().saturating_sub(1),
    );
    for (index, edge) in trace.iter().enumerate() {
        words.push(edge.word.clone());
        if index != 0 {
            key.push('\'');
        }
        key.push_str(&edge.key);
    }
    paths.push(SentencePath {
        sentence,
        key,
        log_prob: hyp.score,
        words,
        typo_edges: hyp.typo_edges,
    });
    true
}

const SMALL_SENTENCE_PATHS: usize = 64;

fn retain_unique_sentences(paths: &mut Vec<SentencePath>, take: usize) {
    if paths.len() <= SMALL_SENTENCE_PATHS {
        let mut write = 0;
        for read in 0..paths.len() {
            if paths[..write]
                .iter()
                .any(|path| path.sentence == paths[read].sentence)
            {
                continue;
            }
            if write != read {
                paths.swap(write, read);
            }
            write += 1;
        }
        paths.truncate(write.min(take));
        return;
    }
    let mut sentences = HashSet::with_capacity(paths.len());
    let duplicates = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| (!sentences.insert(path.sentence.as_str())).then_some(index))
        .collect::<Vec<_>>();
    drop(sentences);
    let mut duplicates = duplicates.into_iter().peekable();
    let mut write = 0;
    for read in 0..paths.len() {
        if duplicates.peek() == Some(&read) {
            duplicates.next();
            continue;
        }
        if write != read {
            paths.swap(write, read);
        }
        write += 1;
    }
    paths.truncate(write.min(take));
}

#[cfg(test)]
fn contains_sentence(paths: &[SentencePath], sentence: &str) -> bool {
    paths.iter().any(|path| path.sentence == sentence)
}

/// WL:168-186. The beam carries one word of history, so a third word of context cannot be searched without widening every hypothesis into (position, last two words); the survivors are rescored instead. Each entry holds what the third word adds over the second, so summing it onto a path that already carries its bigram score is the whole model, not a second opinion.
fn rescore_with_trigram(paths: &mut [SentencePath], options: &LatticeOptions<'_>) -> bool {
    let Some(trigram) = options.trigram.as_deref() else {
        return false;
    };
    if options.trigram_weight == 0.0 || paths.len() < 2 {
        return false;
    }
    let preceding_word = crate::text::last_characters(options.preceding_context, 2);
    for path in paths.iter_mut() {
        let mut bonus = 0.0;
        for (index, word) in path.words.iter().enumerate() {
            if !preceding_word.is_empty() && index == 0 {
                // 句中紧接汉字续写时不查伪句首 `(<bos>, <bos>, word)`，以免漏给句中前缀虚增句首分。
                continue;
            }
            let before = if index >= 2 {
                path.words[index - 2].as_str()
            } else if !preceding_word.is_empty() {
                preceding_word
            } else {
                SENTENCE_START
            };
            let previous = if index >= 1 {
                path.words[index - 1].as_str()
            } else {
                SENTENCE_START
            };
            bonus += f64::from(trigram.trigram(before, previous, word));
        }
        path.log_prob += options.trigram_weight * bonus;
    }
    paths.sort_by(|a, b| b.log_prob.total_cmp(&a.log_prob));
    true
}

/// WL:490-525. Typo edges keep the literal edge scale minus their penalty, so the comparison with the literal sentence is on the same terms as everything else in the decode.
pub(super) fn decode_typo_on_graph(
    graph: &Graph,
    options: &LatticeOptions<'_>,
    literal_best: &SentencePath,
    edges: &[TypoEdge],
) -> Option<TypoSentence> {
    decode_typo_on_graph_with_policy(graph, options, literal_best, edges, false)
}

/// 解码纠错整句；双拼即使纠错句分数低于当前字面整句，也要把它作为候选提供给用户。
pub(super) fn decode_typo_on_graph_with_policy(
    graph: &Graph,
    options: &LatticeOptions<'_>,
    literal_best: &SentencePath,
    edges: &[TypoEdge],
    allow_nonleading: bool,
) -> Option<TypoSentence> {
    let n = graph.len();
    let mut capacities = vec![0usize; n];
    for typo in edges {
        if typo.start < typo.end && typo.end <= n && !typo.value.is_empty() {
            capacities[typo.start] = capacities[typo.start].saturating_add(1);
        }
    }
    let mut extra: Graph = capacities.into_iter().map(Vec::with_capacity).collect();
    let mut any = false;
    for typo in edges {
        if typo.start >= typo.end || typo.end > n || typo.value.is_empty() {
            continue;
        }
        extra[typo.start].push(Edge {
            end: typo.end,
            word: typo.value.clone(),
            key: typo.key.clone(),
            log_prob: edge_log_prob(typo.weight, typo.end - typo.start, options) - typo.penalty,
            typo: true,
        });
        any = true;
    }
    if !any {
        return None;
    }
    let typo_options = LatticeOptions {
        beam: options.typo_beam,
        nbest: if allow_nonleading {
            options.nbest.max(64)
        } else {
            options.nbest
        },
        terminal_beam: 0,
        terminal_max_per_prefix: 0,
        terminal_diversity_margin: 0.0,
        ..options.clone()
    };
    let paths = decode_graph(graph, &typo_options, Some(&extra));
    let max_typo_edges = if allow_nonleading {
        usize::MAX
    } else {
        // 2..=5 音节合法全拼至多允许单处纠错边，彻底杜绝 4 字干净句被两处独立误纠拼成幻觉整句（如 `gongxunyongji -> 通讯统计*`）。
        (n / 3).max(1)
    };
    let best = paths
        .into_iter()
        .find(|path| path.typo_edges > 0 && path.typo_edges <= max_typo_edges)?;
    // The narrower beam can lose the literal best path, so beating it is checked rather than assumed.
    if best.typo_edges == 0 || best.sentence == literal_best.sentence {
        return None;
    }
    if !allow_nonleading && best.log_prob <= literal_best.log_prob {
        return None;
    }
    let literal_has_multi_char_word = literal_best
        .words
        .iter()
        .any(|word| word.chars().count() >= 2);
    Some(TypoSentence {
        sentence: best.sentence,
        key: best.key,
        score: best.log_prob,
        words: best.words,
        edges: best.typo_edges,
        literal_score: literal_best.log_prob,
        literal_has_multi_char_word,
    })
}

/// 整句改字里的一段：音节 `start..end` 转换成 `word`，`key` 是它的读音（音节以 `'` 连接）。改字只处理一个汉字对应一个音节的句子，所以 `word` 的字数等于 `end - start`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedSpan {
    pub start: usize,
    pub end: usize,
    pub word: String,
    pub key: String,
}

/// 在 `pins` 的约束下重新转换 `syllables`：钉住的段原样出现，其余音节由词网格解出，不能跨过任何钉住的段。只用一个汉字对应一个音节的词，结果按音节切成首尾相接、覆盖全部音节的段。`pins` 须互不重叠且落在音节范围内；某个钉住的词不在词库里时按最低权重补一条边。解不出路径时为空。
pub fn decode_pinned(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
    pins: &[PinnedSpan],
) -> Vec<PinnedSpan> {
    let n = syllables.len();
    if n == 0 || pins.iter().any(|pin| pin.start >= pin.end || pin.end > n) {
        return Vec::new();
    }
    let mut graph = build_graph(syllables, lookup, options);
    for (start, edges) in graph.iter_mut().enumerate() {
        edges.retain(|edge| {
            if edge.word.chars().count() != edge.end - start {
                return false;
            }
            pins.iter().all(|pin| {
                let overlaps = start < pin.end && pin.start < edge.end;
                !overlaps || (start == pin.start && edge.end == pin.end && edge.word == pin.word)
            })
        });
    }
    for pin in pins {
        let column = &mut graph[pin.start];
        if !column.iter().any(|edge| edge.end == pin.end) {
            column.push(Edge {
                end: pin.end,
                word: pin.word.clone(),
                key: pin.key.clone(),
                log_prob: edge_log_prob(1, pin.end - pin.start, options),
                typo: false,
            });
        }
    }
    let Some(best) = decode_graph(&graph, options, None).into_iter().next() else {
        return Vec::new();
    };
    let keys: Vec<&str> = best.key.split('\'').collect();
    if keys.len() != n {
        return Vec::new();
    }
    let mut spans = Vec::with_capacity(best.words.len());
    let mut start = 0;
    for word in best.words {
        let end = start + word.chars().count();
        spans.push(PinnedSpan {
            start,
            end,
            key: keys[start..end].join("'"),
            word,
        });
        start = end;
    }
    spans
}

#[cfg(test)]
#[path = "decode_bigrams_tests.rs"]
mod bigram_buffer_tests;

#[cfg(test)]
#[path = "decode_bigram_memo_entry_tests.rs"]
mod bigram_memo_entry_tests;

#[cfg(test)]
pub(super) mod tests {
    use std::collections::HashSet;

    use super::super::ngram::tests::{pair_table, triple_table};
    use super::super::personal::tests::record_run;
    use super::*;

    pub(crate) type Table = HashMap<String, Vec<LatticeLexeme>>;

    pub(crate) fn row(key: &str, value: &str, weight: i64) -> LatticeLexeme {
        LatticeLexeme {
            key: key.to_owned(),
            value: value.to_owned(),
            weight,
        }
    }

    /// Rows keyed by the `'`-joined span, the C++ `make_table_lattice_lookup`.
    pub(crate) fn table(entries: &[(&str, &[(&str, i64)])]) -> Table {
        entries
            .iter()
            .map(|(key, rows)| {
                (
                    (*key).to_owned(),
                    rows.iter()
                        .map(|(value, weight)| row(key, value, *weight))
                        .collect(),
                )
            })
            .collect()
    }

    pub(crate) fn lookup(table: &Table) -> impl FnMut(&[String]) -> Vec<LatticeLexeme> + '_ {
        move |span: &[String]| table.get(&span.join("'")).cloned().unwrap_or_default()
    }

    pub(crate) fn syllables(text: &str) -> Vec<String> {
        text.split('\'').map(str::to_owned).collect()
    }

    fn decode(text: &str, rows: &Table, options: &LatticeOptions<'_>) -> Vec<SentencePath> {
        decode_sentences(&syllables(text), &mut lookup(rows), options)
    }

    fn best(text: &str, rows: &Table, options: &LatticeOptions<'_>) -> String {
        let paths = decode(text, rows, options);
        assert!(!paths.is_empty(), "the lattice decoded nothing for {text}");
        paths[0].sentence.clone()
    }

    #[test]
    fn sentence_lookup_scans_existing_paths() {
        let paths = vec![SentencePath {
            sentence: "你好".to_owned(),
            key: "ni'hao".to_owned(),
            log_prob: 0.0,
            words: vec!["你".to_owned(), "好".to_owned()],
            typo_edges: 0,
        }];
        assert!(contains_sentence(&paths, "你好"));
        assert!(!contains_sentence(&paths, "泥好"));
    }

    #[test]
    fn a_complete_phrase_does_not_lose_to_extra_frequency_terms() {
        let rows = table(&[
            ("xing'he", &[("星禾", 50_000)]),
            ("mian'ban", &[("面板", 20_000)]),
            ("xing'he'mian'ban", &[("星河面板", 100)]),
        ]);
        assert_eq!(
            best("xing'he'mian'ban", &rows, &LatticeOptions::default()),
            "星河面板"
        );
    }

    #[test]
    fn edge_log_prob_uses_unified_length_neutral_log_prior() {
        let options = LatticeOptions::default();
        assert!((edge_log_prob(500_000_000, 1, &options) - 0.0).abs() < 1e-12);
        assert_eq!(edge_log_prob(0, 1, &options), -UNIFIED_UNIGRAM_Z.ln());
        assert_eq!(
            edge_log_prob(-5, 1, &options),
            edge_log_prob(1, 1, &options)
        );
        // R3 温和分桶校准刻度下：高频核心双字词与 5k-20k 中频真词均受保护，
        // 同时「血铅(3985)」「过不(3333)」等低权双字怪词在静态先验上即低于高频单字组合「些+钱」「国+补」。
        let xie_qian =
            edge_log_prob(3_750_000, 1, &options) + edge_log_prob(1_030_000, 1, &options);
        let xue_qian = edge_log_prob(3_985, 2, &options);
        assert!(
            xie_qian > xue_qian + 0.5,
            "些+钱 ({xie_qian:.3}) 应高于血铅 ({xue_qian:.3})"
        );
        let guo_bu_split =
            edge_log_prob(5_000_000, 1, &options) + edge_log_prob(400_000, 1, &options);
        let guo_bu_noise = edge_log_prob(3_333, 2, &options);
        assert!(
            guo_bu_split > guo_bu_noise,
            "国+补 ({guo_bu_split:.3}) 应高于过不 ({guo_bu_noise:.3})"
        );
        let medium_real_phrase = edge_log_prob(15_000, 2, &options);
        let rare_split = edge_log_prob(300_000, 1, &options) + edge_log_prob(300_000, 1, &options);
        assert!(
            medium_real_phrase > rare_split + 2.0,
            "中频真词 15k ({medium_real_phrase:.3}) 应稳胜普通拆字 ({rare_split:.3})"
        );
        let xian_zai_phrase = edge_log_prob(503_000, 2, &options);
        let xian_zai_split =
            edge_log_prob(1_660_000, 1, &options) + edge_log_prob(19_200_000, 1, &options);
        assert!(
            xian_zai_phrase > xian_zai_split,
            "高频双字词现在 ({xian_zai_phrase:.3}) 应胜过先+在 ({xian_zai_split:.3})"
        );
        // 三字及以上未测频占位档 `w == 100` 按平滑先验 `15.0` 与桶 p99 解压计算。
        let expected_placeholder = PHRASE_WEIGHT_DECOMPRESSION
            * (UNMEASURED_PHRASE_EFFECTIVE_WEIGHT.ln() - phrase_bucket_p99_ref(3).ln())
            - (PHRASE_TWO_SYLLABLE_ANCHOR_NATS + PHRASE_EXTRA_SYLLABLE_STEP_NATS);
        assert!((edge_log_prob(100, 3, &options) - expected_placeholder).abs() < 1e-12);
        let odd = LatticeOptions {
            unigram_z: 0.5,
            ..LatticeOptions::default()
        };
        assert_eq!(edge_log_prob(10, 1, &odd), edge_log_prob(10, 1, &options));
    }

    #[test]
    fn columns_reserve_their_incoming_beam_fanout() {
        let edge = |end| Edge {
            end,
            word: "词".to_owned(),
            key: "ci".to_owned(),
            log_prob: 0.0,
            typo: false,
        };
        let graph = vec![vec![edge(1), edge(2)], vec![edge(2)], vec![]];
        let extra = vec![vec![], vec![edge(2)], vec![]];

        assert_eq!(column_capacities(&graph, Some(&extra), 4), [1, 1, 3, 0]);
    }

    #[test]
    fn decode_without_bigram_does_not_allocate_bigram_rows() {
        let graph = vec![Vec::new()];
        let options = LatticeOptions::default();
        let (paths, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            decode_graph(&graph, &options, None)
        });

        assert!(paths.is_empty());
        assert_eq!(allocations, 3);
    }

    #[test]
    fn bigram_row_allocations_do_not_grow_with_column_count() {
        use crate::ime::personal_rerank::allocations::measure;

        let directory = tempfile::tempdir().unwrap();
        let bigram = Arc::new(pair_table(
            directory.path(),
            "zero.bin",
            &[("甲", "甲", 0.0)],
        ));
        for length in [1, 2, 8, 32] {
            let graph = (0..length)
                .map(|position| {
                    vec![Edge {
                        end: position + 1,
                        word: "甲".to_owned(),
                        key: "jia".to_owned(),
                        log_prob: -1.0,
                        typo: false,
                    }]
                })
                .collect();
            let options = LatticeOptions {
                beam: 1,
                nbest: 1,
                ..LatticeOptions::default()
            };
            let (plain, without) = measure(|| decode_graph(&graph, &options, None));
            let options = LatticeOptions {
                bigram: Some(Arc::clone(&bigram)),
                ..options
            };
            let (scored, with) = measure(|| decode_graph(&graph, &options, None));
            assert_eq!(scored, plain);
            assert_eq!(with.remaining_bytes, without.remaining_bytes);
            assert_eq!(with.minimum_bytes, 0);
            assert!(with.peak_bytes <= without.peak_bytes + 256);
            assert!(
                with.allocations <= without.allocations + 3,
                "{length} 列：无表 {without:?}，有表 {with:?}"
            );
        }
    }

    /// test_pinyin.cpp:540-548, like the other fake-lookup lattice cases of `test_word_lattice` (:534-677) ported here and in merge.rs; the SQLite lookup case (:641-666) is in dictionary/pinyin.rs.
    #[test]
    fn nie_zi_prefers_the_phrase() {
        let rows = table(&[
            ("nie", &[("捏", 8000), ("聂", 4000), ("镊", 3000)]),
            ("zi", &[("子", 9000)]),
            ("nie'zi", &[("镊子", 18000), ("孽子", 2000)]),
        ]);
        assert_eq!(best("nie'zi", &rows, &LatticeOptions::default()), "镊子");
    }

    /// test_pinyin.cpp:550-564.
    #[test]
    fn five_syllables_join_the_best_phrases() {
        let rows = table(&[
            ("gao", &[("高", 12000), ("搞", 11000)]),
            ("tan", &[("谈", 9000), ("碳", 4000), ("摊", 3500)]),
            ("gang", &[("刚", 9000), ("钢", 5000), ("岗", 4000)]),
            ("nie", &[("捏", 8000), ("镊", 3000)]),
            ("zi", &[("子", 9000)]),
            ("gao'tan", &[("高谈", 20000)]),
            ("tan'gang", &[("碳钢", 16000)]),
            ("nie'zi", &[("镊子", 18000)]),
        ]);
        let paths = decode("gao'tan'gang'nie'zi", &rows, &LatticeOptions::default());
        assert_eq!(paths[0].sentence, "高碳钢镊子");
        assert_eq!(paths[0].key, "gao'tan'gang'nie'zi");
        assert_eq!(paths[0].words, ["高", "碳钢", "镊子"]);
        assert_eq!(paths[0].typo_edges, 0);
    }

    #[test]
    fn sentence_final_particles_carry_no_hardcoded_magic_bonus() {
        let rows = table(&[
            ("zhai'quan", &[("债券", 5000)]),
            ("ma", &[("马", 9000), ("吗", 3000)]),
        ]);
        // 清退旧 +3.0 句末语气词魔数后，无搭配/神经信号时按真实词频排序，再由 `.gram` 或神经模型决定语境偏好。
        assert_eq!(SENTENCE_FINAL_PARTICLE_BONUS, 0.0);
        assert_eq!(
            best("zhai'quan'ma", &rows, &LatticeOptions::default()),
            "债券马"
        );
    }

    #[test]
    fn the_particle_bonus_is_only_for_the_last_word() {
        let rows = table(&[
            ("ma", &[("马", 9000), ("吗", 3000)]),
            ("shang", &[("上", 9000)]),
        ]);
        assert_eq!(best("ma'shang", &rows, &LatticeOptions::default()), "马上");
    }

    #[test]
    fn reduplicated_interjections_take_no_phrase_bonus() {
        assert_eq!(bonus_syllables("啊啊", 2), 1);
        assert_eq!(bonus_syllables("呃呃呃", 3), 1);
        assert_eq!(bonus_syllables("爸爸", 2), 2);
        assert_eq!(bonus_syllables("哈哈", 2), 2);
        assert_eq!(bonus_syllables("啊", 1), 1);
    }

    /// test_pinyin.cpp:598-612.
    #[test]
    fn hen_la_ji_beats_the_rarer_homophone() {
        let rows = table(&[
            ("xing", &[("性", 8000)]),
            ("neng", &[("能", 8000)]),
            ("hen", &[("很", 20000), ("狠", 3000)]),
            ("la", &[("拉", 5000)]),
            ("ji", &[("圾", 4000)]),
            ("xing'neng", &[("性能", 15000)]),
            ("la'ji", &[("垃圾", 14000)]),
        ]);
        let sentence = best("xing'neng'hen'la'ji", &rows, &LatticeOptions::default());
        assert!(
            sentence.contains("很垃圾") && !sentence.contains("狠垃圾"),
            "{sentence}"
        );
    }

    #[test]
    fn paths_are_distinct_sorted_and_capped() {
        let rows = table(&[
            ("shu", &[("书", 9000), ("输", 8000), ("数", 7000)]),
            ("ru", &[("入", 9000), ("如", 8000)]),
            ("shu'ru", &[("输入", 20000)]),
        ]);
        let options = LatticeOptions {
            nbest: 3,
            ..LatticeOptions::default()
        };
        let paths = decode("shu'ru", &rows, &options);
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0].sentence, "输入");
        assert!(paths
            .windows(2)
            .all(|pair| pair[0].log_prob >= pair[1].log_prob));
        let sentences: HashSet<_> = paths.iter().map(|path| path.sentence.as_str()).collect();
        assert_eq!(sentences.len(), paths.len());
        assert!(decode_sentences(&[], &mut lookup(&rows), &options).is_empty());
        assert!(
            decode("shu'xx", &rows, &options).is_empty(),
            "an uncovered syllable leaves no path"
        );
    }

    #[test]
    fn same_sentence_from_two_segmentations_is_listed_once() {
        let rows = table(&[("ma", &[("马", 9000)]), ("ma'ma", &[("马马", 100)])]);
        let paths = decode("ma'ma", &rows, &LatticeOptions::default());
        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0].words,
            ["马马"],
            "the higher-scoring spelling survives"
        );
    }

    #[test]
    fn sentence_path_dedup_uses_no_temporary_heap_state_for_small_beams() {
        let mut paths = (0..32)
            .map(|index| SentencePath {
                sentence: format!("句{}", index % 16),
                key: "a".to_owned(),
                log_prob: index as f64,
                words: vec!["句".to_owned()],
                typo_edges: 0,
            })
            .collect::<Vec<_>>();

        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            retain_unique_sentences(&mut paths, 12);
        });

        assert_eq!(allocations, 0);
        assert_eq!(paths.len(), 12);
    }

    #[test]
    fn repeated_span_cache_keys_do_not_allocate_joined_strings() {
        let syllables = vec!["a".to_owned(); 8];
        let options = LatticeOptions {
            max_phrase_syllables: 3,
            ..LatticeOptions::default()
        };
        let (graph, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            let mut lookup = |_: &[String]| Vec::new();
            build_graph(&syllables, &mut lookup, &options)
        });

        assert!(graph.iter().all(Vec::is_empty));
        assert!(
            allocations <= 8,
            "span cache should avoid joined key allocations: {allocations}"
        );
    }

    #[test]
    fn span_limit_and_empty_values_are_respected() {
        let rows = table(&[("a", &[("", 900_000), ("甲", 10), ("乙", 1_000_000)])]);
        let options = LatticeOptions {
            span_limit: 2,
            ..LatticeOptions::default()
        };
        let paths = decode("a", &rows, &options);
        assert_eq!(paths.len(), 1, "the third row is past the span limit");
        assert_eq!(paths[0].sentence, "甲");
        let mut calls = 0;
        let mut counting = |span: &[String]| {
            calls += 1;
            rows.get(&span.join("'")).cloned().unwrap_or_default()
        };
        decode_sentences(&syllables("a'a'a"), &mut counting, &options);
        assert_eq!(calls, 3, "a, a'a and a'a'a are each looked up once");
    }

    #[test]
    fn row_key_falls_back_to_the_span_key() {
        let mut rows = table(&[("ni", &[("你", 9000)]), ("hao", &[("好", 9000)])]);
        rows.insert("ni'hao".into(), vec![row("", "你好", 30000)]);
        let paths = decode("ni'hao", &rows, &LatticeOptions::default());
        assert_eq!(paths[0].sentence, "你好");
        assert_eq!(paths[0].key, "ni'hao");
    }

    #[test]
    fn bigram_flips_the_transition() {
        let directory = tempfile::tempdir().unwrap();
        let rows = table(&[
            ("pei'zhi", &[("配置", 20000)]),
            ("yu", &[("于", 900000), ("与", 300000)]),
            ("quan'xian", &[("权限", 18000)]),
        ]);
        let text = "pei'zhi'yu'quan'xian";
        assert_eq!(best(text, &rows, &LatticeOptions::default()), "配置于权限");

        let bigram = Arc::new(pair_table(
            directory.path(),
            "transition.bin",
            &[("配置", "与", 3.0)],
        ));
        let mut options = LatticeOptions {
            bigram: Some(bigram),
            ..LatticeOptions::default()
        };
        let with = decode(text, &rows, &options);
        assert_eq!(
            with[0].sentence, "配置与权限",
            "one known pair flips the path"
        );
        // A pair the corpus never saw is not penalised into last place, which is what makes the table safe to ship against a partial corpus.
        assert!(with.len() > 1, "the alternative path is still generated");

        // The weight has to be able to turn the term off again.
        options.bigram_weight = 0.0;
        assert_eq!(best(text, &rows, &options), "配置于权限");
    }

    #[test]
    fn an_unlikely_pair_does_not_lose_to_an_unseen_one() {
        let directory = tempfile::tempdir().unwrap();
        let rows = table(&[
            ("yi'da", &[("一大", 301240), ("益达", 200000)]),
            ("tuo", &[("拖", 463823)]),
        ]);
        let text = "yi'da'tuo";
        assert_eq!(best(text, &rows, &LatticeOptions::default()), "一大拖");

        // The shipped table has 句首 -> 一大 at -1.0 and no 句首 -> 益达 at all (#7583).
        let bigram = Arc::new(pair_table(
            directory.path(),
            "start.bin",
            &[(SENTENCE_START, "一大", -1.0)],
        ));
        let options = LatticeOptions {
            bigram: Some(bigram),
            ..LatticeOptions::default()
        };
        assert_eq!(
            best(text, &rows, &options),
            "一大拖",
            "a pair scored below chance weighs the same as a pair the table never kept"
        );
    }

    #[test]
    fn trigram_reorders_the_finished_paths() {
        let directory = tempfile::tempdir().unwrap();
        let rows = table(&[
            ("shu'ru", &[("输入", 20000)]),
            ("fa", &[("法", 800000), ("发", 900000)]),
        ]);
        let text = "shu'ru'fa";
        let plain = decode(text, &rows, &LatticeOptions::default());
        assert!(plain.len() >= 2);
        assert_eq!(plain[0].sentence, "输入发");

        let trigram = Arc::new(triple_table(
            directory.path(),
            "triple.bin",
            &[(SENTENCE_START, "输入", "法", 4.0)],
        ));
        let mut options = LatticeOptions {
            trigram: Some(trigram),
            ..LatticeOptions::default()
        };
        assert_eq!(best(text, &rows, &options), "输入法");
        options.trigram_weight = 0.0;
        assert_eq!(
            best(text, &rows, &options),
            "输入发",
            "weight zero leaves the search order alone"
        );
    }

    #[test]
    fn sentence_options_use_v2_diverse_budget() {
        let paths = RuntimePaths::default();
        let shown = make_sentence_lattice_options(&paths, false);
        assert_eq!(
            (shown.nbest, shown.emit),
            (32, 1),
            "v2 默认搜索 32 条多样性路径，单行模式输出 1 条"
        );
        let all = make_sentence_lattice_options(&paths, true);
        assert_eq!(
            (all.nbest, all.emit),
            (32, 0),
            "多候选模式搜索并输出全部 32 条路径"
        );
        assert!(all.bigram.is_none() && all.trigram.is_none());
        assert_eq!(all.beam, 56);
        assert_eq!(all.terminal_beam, 224);
        assert_eq!(all.span_limit, 40);
        assert_eq!(all.terminal_max_per_prefix, 2);
        assert_eq!(all.terminal_max_per_suffix, 2);
        assert_eq!(all.terminal_max_per_segmentation, 6);
        assert!(all.include_lattice_best && !all.show_next_on_duplicate);
    }

    #[test]
    fn sentence_options_load_the_generation_tables() {
        let directory = tempfile::tempdir().unwrap();
        pair_table(
            directory.path(),
            assets::BIGRAM_TABLE,
            &[("配置", "与", 3.0)],
        );
        let paths = RuntimePaths {
            dictionaries: directory.path().to_path_buf(),
            ..RuntimePaths::default()
        };
        let options = make_sentence_lattice_options(&paths, true);
        assert_eq!(options.bigram.expect("bigram loads").len(), 1);
        assert!(options.trigram.is_none());
    }

    fn personal_lexicon() -> Table {
        // 想 leads 翔 by ln(4.5) = 1.5 nats of static evidence.
        table(&[
            ("wo", &[("我", 900000)]),
            ("ni", &[("你", 900000)]),
            ("xiang", &[("想", 450000), ("翔", 100000)]),
            ("qu", &[("去", 400000), ("区", 100000)]),
        ])
    }

    fn with_personal(personal: &PersonalNgram) -> LatticeOptions<'_> {
        LatticeOptions {
            personal: Some(personal),
            ..LatticeOptions::default()
        }
    }

    #[test]
    fn empty_personal_model_changes_nothing() {
        let rows = personal_lexicon();
        let empty = PersonalNgram::default();
        let plain = decode("wo'xiang'qu", &rows, &LatticeOptions::default());
        let with_empty = decode("wo'xiang'qu", &rows, &with_personal(&empty));
        assert_eq!(plain, with_empty);
        assert_eq!(plain[0].sentence, "我想去");
    }

    #[test]
    fn one_pick_flips_a_path_one_acceptance_does_not() {
        let rows = personal_lexicon();
        let mut accepted = PersonalNgram::default();
        record_run(&mut accepted, &["我", "翔"], 1);
        assert_eq!(best("wo'xiang", &rows, &with_personal(&accepted)), "我想");
        let mut picked = PersonalNgram::default();
        record_run(&mut picked, &["我", "翔"], 2);
        assert_eq!(best("wo'xiang", &rows, &with_personal(&picked)), "我翔");

        let silenced = LatticeOptions {
            personal_weight: 0.0,
            ..with_personal(&picked)
        };
        assert_eq!(best("wo'xiang", &rows, &silenced), "我想");
    }

    #[test]
    fn personal_trigram_tells_contexts_apart() {
        // After 我想 the user picked 区, after 你想 去. The bigram 想 -> 区/去 is a tie; only the trigram can tell them apart, and it has to beat 去's static lead to do it.
        let rows = personal_lexicon();
        let mut model = PersonalNgram::default();
        for _ in 0..4 {
            record_run(&mut model, &["我", "想", "区"], 1);
            record_run(&mut model, &["你", "想", "去"], 1);
        }
        assert_eq!(best("wo'xiang'qu", &rows, &with_personal(&model)), "我想区");
        assert_eq!(best("ni'xiang'qu", &rows, &with_personal(&model)), "你想去");
    }

    #[test]
    fn typo_sentence_needs_a_typo_edge_and_a_better_score() {
        let rows = table(&[
            ("shi", &[("是", 900000)]),
            ("jina", &[]),
            ("jian", &[("见", 500000)]),
            ("shi'jian", &[("时间", 30000)]),
            ("ta", &[("他", 900000)]),
        ]);
        let text = "ta'shi'jian";
        let mut rows_lookup = lookup(&rows);
        let options = LatticeOptions::default();
        let literal = decode_sentences(&syllables(text), &mut rows_lookup, &options);
        assert_eq!(literal[0].sentence, "他时间");

        let edge = |value: &str, weight: i64, penalty: f64| TypoEdge {
            start: 1,
            end: 3,
            key: "shi'jian".into(),
            value: value.into(),
            weight,
            penalty,
        };
        // A typo phrase worth more than the literal one wins and carries its own key.
        let typo = decode_typo_sentence(
            &syllables(text),
            &mut rows_lookup,
            &options,
            &literal[0],
            &[edge("事件", 90000, 0.5)],
        )
        .expect("the typo sentence outscores the literal one");
        assert_eq!(typo.sentence, "他事件");
        assert_eq!(typo.key, "ta'shi'jian");
        assert_eq!(typo.words, ["他", "事件"]);
        assert_eq!(typo.edges, 1);
        assert_eq!(typo.literal_score, literal[0].log_prob);
        assert!(typo.score > typo.literal_score);
        assert!(!typo.leads(f64::INFINITY));
        assert!(typo.leads(0.0));
        assert!(!typo.leads(typo.score - typo.literal_score));

        // Too expensive: the literal path stays best, so there is no typo sentence.
        assert!(decode_typo_sentence(
            &syllables(text),
            &mut rows_lookup,
            &options,
            &literal[0],
            &[edge("事件", 90000, 40.0)],
        )
        .is_none());
        // The same sentence as the literal best is not a correction.
        assert!(decode_typo_sentence(
            &syllables(text),
            &mut rows_lookup,
            &options,
            &literal[0],
            &[edge("时间", 900000, 0.0)],
        )
        .is_none());
        // Edges outside the input or without a value are ignored.
        let invalid = [
            TypoEdge {
                start: 2,
                end: 4,
                ..edge("事件", 90000, 0.0)
            },
            TypoEdge {
                start: 2,
                end: 2,
                ..edge("事件", 90000, 0.0)
            },
            edge("", 90000, 0.0),
        ];
        assert!(decode_typo_sentence(
            &syllables(text),
            &mut rows_lookup,
            &options,
            &literal[0],
            &invalid
        )
        .is_none());
        assert!(decode_typo_sentence(
            &syllables(text),
            &mut rows_lookup,
            &options,
            &literal[0],
            &[]
        )
        .is_none());
    }

    #[test]
    fn typo_leads_ignores_zero_edges() {
        let typo = TypoSentence {
            sentence: "甲".into(),
            key: "jia".into(),
            score: 10.0,
            words: vec!["甲".into()],
            edges: 0,
            literal_score: 0.0,
            literal_has_multi_char_word: false,
        };
        assert!(!typo.leads(0.0));
    }

    fn pinned(start: usize, end: usize, word: &str, key: &str) -> PinnedSpan {
        PinnedSpan {
            start,
            end,
            word: word.to_owned(),
            key: key.to_owned(),
        }
    }

    fn pinned_words(spans: &[PinnedSpan]) -> Vec<&str> {
        spans.iter().map(|span| span.word.as_str()).collect()
    }

    #[test]
    fn pinned_decode_keeps_the_pin_and_reconverts_around_it() {
        let rows = table(&[
            ("bei", &[("被", 9000), ("北", 5000), ("背", 4000)]),
            ("jing", &[("经", 9000), ("京", 3000), ("景", 2000)]),
            ("bei'jing", &[("背景", 30000), ("北京", 20000)]),
            ("qu", &[("去", 9000)]),
        ]);
        let options = LatticeOptions::default();
        let text = syllables("qu'bei'jing");
        let free = decode_pinned(&text, &mut lookup(&rows), &options, &[]);
        assert_eq!(pinned_words(&free), ["去", "背景"]);
        assert_eq!(free[1], pinned(1, 3, "背景", "bei'jing"));

        // 钉住一个两字词：它原样出现，左边照常解码。
        let word = decode_pinned(
            &text,
            &mut lookup(&rows),
            &options,
            &[pinned(1, 3, "北京", "bei'jing")],
        );
        assert_eq!(pinned_words(&word), ["去", "北京"]);

        // 钉住词中间的一个字：跨过它的词不能再用，左边那个音节单独解码。
        let character = decode_pinned(
            &text,
            &mut lookup(&rows),
            &options,
            &[pinned(2, 3, "京", "jing")],
        );
        assert_eq!(pinned_words(&character), ["去", "被", "京"]);
        assert_eq!(character[2], pinned(2, 3, "京", "jing"));
    }

    #[test]
    fn a_pin_missing_from_the_dictionary_still_decodes() {
        let rows = table(&[("qu", &[("去", 9000)]), ("bei", &[("被", 9000)])]);
        let spans = decode_pinned(
            &syllables("qu'bei"),
            &mut lookup(&rows),
            &LatticeOptions::default(),
            &[pinned(1, 2, "贝", "bei")],
        );
        assert_eq!(pinned_words(&spans), ["去", "贝"]);
        assert!(decode_pinned(
            &syllables("qu'bei"),
            &mut lookup(&rows),
            &LatticeOptions::default(),
            &[pinned(1, 3, "贝", "bei")],
        )
        .is_empty());
    }

    #[test]
    fn inanimate_pronoun_edges_are_synthesized_only_when_both_genders_exist() {
        let rows = table(&[
            ("zhi'dao'ta", &[("知道他", 50000), ("知道她", 20000)]),
            // 已有“它”时不重复插入；仅单一性别的专名/普通词（吉他）也不派生“它”。
            (
                "wen'ta",
                &[("问他", 50000), ("问她", 40000), ("问它", 35000)],
            ),
            ("ji'ta", &[("吉他", 80000)]),
            ("she", &[("设", 1000)]),
        ]);
        let options = LatticeOptions::default();
        // 含重复跨度 `zhi'dao'ta'zhi'dao'ta` 以同时覆盖 Span 缓存命中与未命中分支。
        let text = syllables("zhi'dao'ta'zhi'dao'ta'wen'ta'ji'ta");
        let graph = build_graph(&text, &mut lookup(&rows), &options);

        let check_span = |col: usize| {
            let it_edges: Vec<&Edge> = graph[col]
                .iter()
                .filter(|edge| edge.word == "知道它")
                .collect();
            assert_eq!(it_edges.len(), 1);
            let she_lp = graph[col]
                .iter()
                .find(|edge| edge.word == "知道她")
                .unwrap()
                .log_prob;
            let expected = (she_lp - INANIMATE_PRONOUN_BACKOFF).max(edge_log_prob(1, 3, &options));
            assert!((it_edges[0].log_prob - expected).abs() < 1e-12);
        };
        check_span(0);
        check_span(3);

        let wen_it_count = graph[6].iter().filter(|edge| edge.word == "问它").count();
        assert_eq!(wen_it_count, 1);
        assert!(!graph[8].iter().any(|edge| edge.word == "吉它"));

        // `build_graph_borrowed` 应产出一致的图边（包括缓存复用的第二个 `zhi'dao'ta`）。
        let mut borrowed_lookup = |span: &[String]| -> &[LatticeLexeme] {
            rows.get(&span.join("'")).map(Vec::as_slice).unwrap_or(&[])
        };
        let borrowed = build_graph_borrowed(&text, &mut borrowed_lookup, &options);
        assert_eq!(borrowed[3].iter().filter(|e| e.word == "知道它").count(), 1);
    }

    #[test]
    fn terminal_prefix_diversity_preserves_runner_up_prefix_and_backfills_single_prefix() {
        let base = LatticeOptions {
            beam: 16,
            nbest: 3,
            terminal_beam: 64,
            terminal_max_per_prefix: 2,
            terminal_diversity_margin: 8.0,
            ..LatticeOptions::default()
        };
        let rich = table(&[
            ("wo'men'qu", &[("我们去", 100000), ("我门去", 20000)]),
            (
                "de",
                &[
                    ("的", 900000),
                    ("地", 800000),
                    ("得", 700000),
                    ("德", 600000),
                    ("㝵", 1),
                ],
            ),
        ]);
        let top3: Vec<String> = decode("wo'men'qu'de", &rich, &base)
            .into_iter()
            .map(|path| path.sentence)
            .collect();
        // 4 音节以上且前驱列 >= 3 时，第一遍给「我们去」最多留 2 条，让「我门去的」留在前 3 位。
        assert_eq!(top3, ["我们去的", "我们去地", "我门去的"]);

        let backfilled: Vec<String> = decode(
            "wo'men'qu'de",
            &rich,
            &LatticeOptions {
                nbest: 5,
                ..base.clone()
            },
        )
        .into_iter()
        .map(|path| path.sentence)
        .collect();
        // 窗口外低分字「㝵」不得插队，先取窗口内次优前驱再回填暂缓队列中的「我们去得」。
        assert_eq!(
            backfilled,
            ["我们去的", "我们去地", "我门去的", "我门去地", "我们去得"]
        );

        let single_prefix = table(&[
            ("wo'men'qu", &[("我们去", 100000)]),
            (
                "de",
                &[
                    ("的", 900000),
                    ("地", 800000),
                    ("得", 700000),
                    ("德", 600000),
                ],
            ),
        ]);
        let single_sentences: Vec<String> = decode(
            "wo'men'qu'de",
            &single_prefix,
            &LatticeOptions {
                nbest: 4,
                ..base.clone()
            },
        )
        .into_iter()
        .map(|path| path.sentence)
        .collect();
        assert_eq!(
            single_sentences,
            ["我们去的", "我们去地", "我们去得", "我们去德"]
        );

        // 三音节词组查询不触发前驱限流，同一双字前驱下的第 5 位同音结尾仍应完整保留。
        let lexical = table(&[
            ("ming'liang", &[("明亮", 9000)]),
            ("ming", &[("明", 500000)]),
            ("liang'zhuan", &[("亮赚", 800)]),
            (
                "zhuan",
                &[
                    ("转", 1200000),
                    ("专", 360000),
                    ("赚", 88000),
                    ("传", 62000),
                    ("砖", 40000),
                ],
            ),
        ]);
        let lexical_sentences: Vec<String> = decode(
            "ming'liang'zhuan",
            &lexical,
            &LatticeOptions { nbest: 6, ..base },
        )
        .into_iter()
        .map(|path| path.sentence)
        .collect();
        assert!(
            lexical_sentences.iter().any(|s| s == "明亮砖"),
            "三音节词组不应限流同前驱末字: {lexical_sentences:?}"
        );
    }

    #[test]
    fn terminal_segmentation_diversity_prevents_cartesian_homophones_from_evicting_parallel_cuts() {
        let options = LatticeOptions {
            beam: 32,
            nbest: 8,
            terminal_beam: 64,
            terminal_max_per_prefix: 2,
            terminal_diversity_margin: 8.0,
            ..LatticeOptions::default()
        };
        let rows = table(&[
            ("jin'tian'qu", &[("今天去", 126_000)]),
            (
                "you'shi",
                &[
                    ("优势", 500_700),
                    ("有时", 500_400),
                    ("又是", 500_300),
                    ("有事", 138_600),
                ],
            ),
            ("hou'ye", &[("侯爷", 9_300), ("后夜", 3_400)]),
            ("bu'qing'chu'gai", &[("不清楚该", 100)]),
            ("you'shi'hou", &[("有时候", 364_000)]),
            ("ye'bu'qing'chu", &[("也不清楚", 149_500)]),
            ("gai", &[("该", 1_351_000)]),
            ("xuan'na", &[("选哪", 20_500)]),
            ("ge", &[("个", 7_259_000), ("各", 513_000)]),
        ]);
        let decoded: Vec<String> = decode(
            "jin'tian'qu'you'shi'hou'ye'bu'qing'chu'gai'xuan'na'ge",
            &rows,
            &options,
        )
        .into_iter()
        .map(|path| path.sentence)
        .collect();
        assert!(
            decoded.iter().any(|s| s == "今天去有时候也不清楚该选哪个"),
            "中段同音笛卡尔积不应把并行正统切分挤出 nbest: {decoded:?}"
        );
    }

    #[test]
    fn typo_decode_uses_unified_probability_scale_without_normalization_gap_hack() {
        let options = LatticeOptions::default();
        let rows = table(&[
            ("xin'ji", &[("心急", 50_000)]),
            ("qiu", &[("求", 1_200_000)]),
            ("kuai", &[("快", 2_500_000)]),
        ]);
        let text = "xin'ji'qiu'kuai";
        let mut rows_lookup = lookup(&rows);
        let literal = decode(text, &rows, &options);
        assert_eq!(literal[0].sentence, "心急求快");
        // 在统一对数概率尺度下，正常信道代价（3.0 nats）即可自然拦住中频误纠「区块(50k)」，无需旧 `ln(1e6)` 补丁。
        let false_positive = TypoEdge {
            start: 2,
            end: 4,
            key: "qu'kuai".into(),
            value: "区块".into(),
            weight: 50_000,
            penalty: 3.0,
        };
        assert!(decode_typo_sentence(
            &syllables(text),
            &mut rows_lookup,
            &options,
            &literal[0],
            &[false_positive],
        )
        .is_none());

        // 真实高频错拼词（如低频「干+系」对高频「关系(100k)」）在真实信道代价 `3.0` 下自然胜出。
        let genuine_rows = table(&[
            ("mei", &[("没", 1_000)]),
            ("gan", &[("干", 1_000)]),
            ("xi", &[("系", 1_000)]),
        ]);
        let mut genuine_lookup = lookup(&genuine_rows);
        let genuine_literal = decode("mei'gan'xi", &genuine_rows, &options);
        let genuine_edge = TypoEdge {
            start: 1,
            end: 3,
            key: "guan'xi".into(),
            value: "关系".into(),
            weight: 100_000,
            penalty: 3.0,
        };
        let accepted = decode_typo_sentence(
            &syllables("mei'gan'xi"),
            &mut genuine_lookup,
            &options,
            &genuine_literal[0],
            &[genuine_edge],
        )
        .expect("统一尺度下真实错拼应正常产出纠错整句");
        assert_eq!(accepted.sentence, "没关系");
    }

    #[test]
    fn unified_scoring_and_2d_diversity_prevent_suffix_collapse_and_reserve_two_char_splits() {
        // 1. 构造 `zheng'le'xie'qian` 场景：即使用 3 个同尾 `[2, 2]`（共享后缀「血铅」）包围，
        //    统一尺度 + 后缀 2 席配额 + 新骨架优先回填也能彻底消灭 Top-3 后缀塌缩并召回「挣了些钱」。
        let suffix_rows = table(&[
            (
                "zheng'le",
                &[("挣了", 12_630), ("争了", 8_715), ("整了", 7_165)],
            ),
            ("xie'qian", &[("血铅", 3_985), ("斜前", 200)]),
            ("xie", &[("些", 3_750_000), ("写", 2_000_000)]),
            ("qian", &[("钱", 1_030_000), ("前", 3_970_000)]),
        ]);
        let v2_opts = LatticeOptions {
            beam: 64,
            nbest: 16,
            terminal_beam: 128,
            terminal_max_per_prefix: 2,
            terminal_max_per_suffix: 2,
            terminal_max_per_segmentation: 6,
            terminal_diversity_margin: 24.0,
            ..LatticeOptions::default()
        };
        let decoded: Vec<String> = decode("zheng'le'xie'qian", &suffix_rows, &v2_opts)
            .into_iter()
            .map(|p| p.sentence)
            .collect();
        let pos_xieqian = decoded
            .iter()
            .position(|s| s == "挣了些钱")
            .expect("统一概率下应召回「挣了些钱」");
        let pos_xueqian = decoded
            .iter()
            .position(|s| s == "挣了血铅")
            .expect("应保留「挣了血铅」备选");
        assert!(
            pos_xieqian < pos_xueqian && pos_xieqian < 6,
            "即使无 .gram，「挣了些钱」(#{pos_xieqian}) 也应领先「挣了血铅」(#{pos_xueqian}): {decoded:?}"
        );
        let xueqian_in_top3 = decoded
            .iter()
            .take(3)
            .filter(|s| s.ends_with("血铅"))
            .count();
        assert!(
            xueqian_in_top3 <= 2,
            "Top-3 共享「血铅」后缀不得超过 2 席: {decoded:?}"
        );
        // 叠加 `.gram` 搭配信号后（含边回退补偿 `collocation_edge_offset = 3.0`），「挣了些钱」稳居第 1。
        let gram_scorer = |tail: &str, word: &str, _is_rear: bool| match (tail, word) {
            (t, "挣了") if t.ends_with("是") => -0.62,
            (t, "些") if t.ends_with("挣了") => -0.55,
            (t, "钱") if t.ends_with("挣了些") => 0.54,
            _ => -6.0,
        };
        let gram_opts = LatticeOptions {
            collocation_scorer: Some(&gram_scorer),
            collocation_weight: 1.0,
            collocation_edge_offset: 3.0,
            preceding_context: "应该多少还是",
            ..v2_opts.clone()
        };
        let gram_decoded = decode("zheng'le'xie'qian", &suffix_rows, &gram_opts);
        assert_eq!(
            gram_decoded[0].sentence, "挣了些钱",
            "叠加 .gram 搭配后「挣了些钱」应稳居 #1: {gram_decoded:?}"
        );

        // 2. 即使人为把 `[2, 2]` 权重抬高到远超窗口，后缀配额 + 新骨架优先于 deferred 回填仍保证
        //    Top-3 最多 2 条「血铅」且 `[2, 1, 1]` 在 deferred 之前入池。
        let extreme_rows = table(&[
            (
                "zheng'le",
                &[("挣了", 500_000), ("争了", 480_000), ("整了", 460_000)],
            ),
            ("xie'qian", &[("血铅", 5_000_000)]),
            ("xie", &[("些", 300_000)]),
            ("qian", &[("钱", 300_000)]),
        ]);
        let tight_margin = LatticeOptions {
            terminal_diversity_margin: 1.0,
            ..v2_opts.clone()
        };
        let extreme: Vec<String> = decode("zheng'le'xie'qian", &extreme_rows, &tight_margin)
            .into_iter()
            .map(|p| p.sentence)
            .collect();
        assert_eq!(
            extreme
                .iter()
                .take(3)
                .filter(|s| s.ends_with("血铅"))
                .count(),
            2,
            "即使 [2,2] 大幅领先，Top-3 同后缀也绝不超过 2 席: {extreme:?}"
        );
        let pos_novel = extreme
            .iter()
            .position(|s| s == "挣了些钱")
            .expect("应召回新骨架");
        let pos_deferred = extreme
            .iter()
            .position(|s| s == "整了血铅")
            .expect("应回填暂缓路径");
        assert!(
            pos_novel < pos_deferred,
            "未采纳的新骨架路径必须优先于 deferred 同骨架路径入池: {extreme:?}"
        );

        // 3. `n == 2` 时，30 条精确双字词不得把 `[1, 1]` 拆字路径挤出 `nbest = 32`（至少预留 8 席）。
        let dict_two: Vec<(&str, i64)> = [
            "格式", "歌诗", "隔世", "各市", "革室", "葛氏", "割尸", "阁室", "格氏", "戈什", "格事",
            "歌室", "隔室", "各室", "革事", "葛事", "割事", "阁事", "格世", "歌世", "隔事", "各世",
            "革世", "葛世", "割世", "阁世", "格诗", "隔诗", "各诗", "革诗",
        ]
        .iter()
        .enumerate()
        .map(|(i, &w)| (w, 500_000 - (i as i64) * 1_000))
        .collect();
        let mut two_rows = Table::new();
        two_rows.insert(
            "ge'shi".into(),
            dict_two
                .iter()
                .map(|&(w, wt)| row("ge'shi", w, wt))
                .collect(),
        );
        two_rows.insert(
            "ge".into(),
            [
                ("个", 14_000_000),
                ("各", 800_000),
                ("哥", 500_000),
                ("歌", 400_000),
            ]
            .iter()
            .map(|&(w, wt)| row("ge", w, wt))
            .collect(),
        );
        two_rows.insert(
            "shi".into(),
            [("事", 2_000_000), ("是", 20_000_000), ("时", 3_000_000)]
                .iter()
                .map(|&(w, wt)| row("shi", w, wt))
                .collect(),
        );
        let two_opts = LatticeOptions {
            nbest: 32,
            ..v2_opts
        };
        let two_decoded = decode("ge'shi", &two_rows, &two_opts);
        let split_count = two_decoded.iter().filter(|p| p.words.len() == 2).count();
        assert!(
            split_count >= TWO_SYLLABLE_SPLIT_RESERVE,
            "双音节 nbest=32 应至少保留 {TWO_SYLLABLE_SPLIT_RESERVE} 席 [1,1] 拆字路径，实际 {split_count}"
        );
        assert!(
            two_decoded.iter().take(14).any(|p| p.sentence == "个事"),
            "双音节 `[1,1]` 拆字路径「个事」应直接落入 Top-14 神经融合池: {:?}",
            two_decoded
                .iter()
                .take(14)
                .map(|p| &p.sentence)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn preceding_context_replaces_sentence_start_bigram_and_feeds_collocation_tail() {
        let directory = tempfile::tempdir().unwrap();
        let bigram = Arc::new(pair_table(
            directory.path(),
            "context.bin",
            &[(SENTENCE_START, "中式", 2.0), ("培养", "忠实", 2.0)],
        ));
        let rows = table(&[
            ("zhong'shi", &[("中式", 12_000), ("忠实", 10_000)]),
            ("gu'ke", &[("顾客", 50_000)]),
        ]);

        // 无上文时套用句首二元项，首选「中式顾客」。
        let no_ctx = LatticeOptions {
            bigram: Some(Arc::clone(&bigram)),
            ..LatticeOptions::default()
        };
        assert_eq!(best("zhong'shi'gu'ke", &rows, &no_ctx), "中式顾客");

        // 带连续汉字上文时，首列不再触发句首 `<bos>` 加成，改查上文尾词二元转移，同时向搭配打分器传递跨边界尾窗。
        let tails = RefCell::new(Vec::<(String, String, bool)>::new());
        let scorer = |tail: &str, word: &str, is_rear: bool| {
            tails
                .borrow_mut()
                .push((tail.to_string(), word.to_string(), is_rear));
            0.0
        };
        let with_ctx = LatticeOptions {
            bigram: Some(bigram),
            collocation_scorer: Some(&scorer),
            collocation_weight: 1.0,
            preceding_context: "非常认真地培养",
            ..LatticeOptions::default()
        };
        assert_eq!(best("zhong'shi'gu'ke", &rows, &with_ctx), "忠实顾客");
        let observed = tails.borrow();
        assert!(observed
            .iter()
            .any(|(tail, word, is_rear)| tail == "非常认真地培养" && word == "忠实" && !is_rear));
        assert!(observed
            .iter()
            .any(|(tail, word, is_rear)| tail == "常认真地培养忠实" && word == "顾客" && *is_rear));
    }

    #[test]
    fn confusion_network_1span_perturbation_recovers_early_and_medial_rivals_into_top16() {
        // 构造 14 音节长句：末位与近末位充满高熵同音（`改/盖/该/概/钙` × `的/地/得`），
        // 窄束宽前向搜索会把末列耗在尾部同音上；混淆网络 1-Span 轮询扰动必须把句首同音、
        // 句中 `[2]->[1,1]` 拆字与 `[1,1]->[2]` 合并全部召回进 Top-16，且分值与整句重算完全一致。
        let rows = table(&[
            ("wo", &[("我", 29_000_000)]),
            (
                "xiang",
                &[("向", 2_200_000), ("想", 1_600_000), ("像", 900_000)],
            ),
            ("qu'kan", &[("去看", 60_000)]),
            ("yi'xia", &[("一下", 450_000)]),
            ("na'ge", &[("那个", 400_000)]),
            ("wan'xi", &[("惋惜", 200_000)]),
            (
                "wan",
                &[
                    ("万", 4_000_000),
                    ("完", 3_500_000),
                    ("晚", 2_500_000),
                    ("玩", 1_800_000),
                    ("弯", 900_000),
                    ("碗", 650_000),
                ],
            ),
            (
                "xi",
                &[
                    ("西", 4_200_000),
                    ("息", 3_100_000),
                    ("希", 2_400_000),
                    ("习", 1_900_000),
                    ("戏", 1_200_000),
                    ("洗", 700_000),
                ],
            ),
            ("he", &[("和", 18_000_000), ("合", 2_000_000)]),
            ("li", &[("里", 3_200_000), ("理", 2_400_000)]),
            ("he'li", &[("湖里", 3_500)]),
            (
                "de",
                &[("的", 28_000_000), ("地", 9_000_000), ("得", 8_500_000)],
            ),
            (
                "gai",
                &[
                    ("改", 2_500_000),
                    ("盖", 2_400_000),
                    ("该", 2_300_000),
                    ("概", 2_200_000),
                    ("钙", 2_100_000),
                ],
            ),
        ]);
        let gram_scorer = |tail: &str, word: &str, _is_rear: bool| match (tail, word) {
            (t, "碗") if t.ends_with("那个") => -2.2,
            (t, "洗") if t.ends_with("碗") => -2.5,
            _ => -6.0,
        };
        let v2_opts = LatticeOptions {
            beam: 12,
            nbest: 32,
            terminal_beam: 24,
            terminal_max_per_prefix: 2,
            terminal_max_per_suffix: 2,
            terminal_max_per_segmentation: 6,
            terminal_diversity_margin: 24.0,
            collocation_scorer: Some(&gram_scorer),
            collocation_weight: 1.0,
            collocation_edge_offset: 3.0,
            ..LatticeOptions::default()
        };
        let input = "wo'xiang'qu'kan'yi'xia'na'ge'wan'xi'he'li'de'gai";
        let paths = decode(input, &rows, &v2_opts);
        assert!(paths.len() >= 16, "应产出足够候选: {}", paths.len());
        assert_eq!(paths[0].sentence, "我向去看一下那个惋惜和里的改");

        // 校验全部候选去重且读音跨度严格覆盖 14 音节。
        let unique: HashSet<&str> = paths.iter().map(|p| p.sentence.as_str()).collect();
        assert_eq!(
            unique.len(),
            paths.len(),
            "混淆网络扰动结果不得出现重复整句"
        );
        for path in &paths {
            assert_eq!(
                path.key.split('\'').count(),
                14,
                "扰动句读音必须严格覆盖 14 音节: {} ({})",
                path.sentence,
                path.key
            );
        }

        let top16: Vec<&str> = paths.iter().take(16).map(|p| p.sentence.as_str()).collect();
        // 1. 句首同跨度同音替补 `向 -> 想` 落入 Top-16 且局部增量分值严格精确。
        let xiang_path = paths
            .iter()
            .take(16)
            .find(|p| p.sentence == "我想去看一下那个惋惜和里的改")
            .unwrap_or_else(|| panic!("句首同音替补应落入 Top-16: {top16:?}"));
        let expected_xiang_lp = paths[0].log_prob
            + (edge_log_prob(1_600_000, 1, &v2_opts) - edge_log_prob(2_200_000, 1, &v2_opts));
        assert!(
            (xiang_path.log_prob - expected_xiang_lp).abs() < 1e-9,
            "句首同音替补对数概率应与精确整句重算一致: got {}, expect {}",
            xiang_path.log_prob,
            expected_xiang_lp
        );

        // 2. 句中 `[2] -> [1, 1]` 拆字替补 `惋惜 -> 碗+洗`（两单字均为 #6 非首位）凭借双向语境 DP 落入 Top-16。
        let wanxi_path = paths
            .iter()
            .take(16)
            .find(|p| p.sentence == "我向去看一下那个碗洗和里的改")
            .unwrap_or_else(|| panic!("句中 [2]->[1,1] 拆字替补应落入 Top-16: {top16:?}"));
        let expected_wanxi_lp = paths[0].log_prob
            + (edge_log_prob(650_000, 1, &v2_opts)
                + (-2.2 + 3.0)
                + edge_log_prob(700_000, 1, &v2_opts)
                + (-2.5 + 3.0)
                - (edge_log_prob(200_000, 2, &v2_opts) + (-6.0 + 3.0)));
        assert!(
            (wanxi_path.log_prob - expected_wanxi_lp).abs() < 1e-9,
            "句中拆字替补对数概率应与精确整句重算一致: got {}, expect {}",
            wanxi_path.log_prob,
            expected_wanxi_lp
        );

        // 3. 句中 `[1, 1] -> [2]` 合并替补 `和+里 -> 湖里` 落入 Top-16。
        assert!(
            top16.contains(&"我向去看一下那个惋惜湖里的改"),
            "句中 [1,1]->[2] 合并替补应落入 Top-16: {top16:?}"
        );

        // 4. 验证 `terminal_max_per_suffix == 0` 时扰动通道严格关闭（冻结测试零扰动）。
        let legacy_opts = LatticeOptions {
            terminal_max_per_prefix: 0,
            terminal_max_per_suffix: 0,
            terminal_max_per_segmentation: 0,
            beam: 4,
            nbest: 8,
            terminal_beam: 8,
            ..v2_opts
        };
        let legacy = decode(input, &rows, &legacy_opts);
        assert!(
            legacy.len() <= 8,
            "关闭后缀多样性时不得触发混淆网络扩展: {}",
            legacy.len()
        );
    }

    #[test]
    fn intermediate_single_char_fanout_and_multi_anchor_cn_recover_collocation_phrase() {
        let rows = table(&[
            (
                "xian'zhe",
                &[("闲着", 120_000), ("贤者", 15_000), ("先哲", 7_800)],
            ),
            (
                "zhi'shi",
                &[("只是", 900_000), ("知识", 310_000), ("之时", 308_000)],
            ),
            (
                "xian",
                &[
                    ("先", 4_000_000),
                    ("现", 3_500_000),
                    ("闲", 1_500_000),
                    ("贤", 900_000),
                    ("线", 850_000),
                    ("显", 800_000),
                    ("鲜", 700_000),
                    ("仙", 600_000),
                ],
            ),
            (
                "zhe",
                &[
                    ("这", 12_000_000),
                    ("着", 5_000_000),
                    ("者", 2_000_000),
                    ("折", 500_000),
                    ("哲", 300_000),
                    ("遮", 200_000),
                    ("辙", 100_000),
                    ("蔗", 50_000),
                ],
            ),
            (
                "zhi",
                &[
                    ("之", 5_000_000),
                    ("只", 4_500_000),
                    ("知", 4_000_000),
                    ("直", 3_500_000),
                    ("指", 3_000_000),
                    ("制", 2_500_000),
                    ("治", 2_000_000),
                    ("志", 1_800_000),
                    ("至", 1_600_000),
                    ("值", 1_400_000),
                    ("置", 1_200_000),
                    ("质", 1_000_000),
                    ("止", 900_000),
                    ("支", 800_000),
                    ("织", 700_000),
                    ("枝", 600_000),
                ],
            ),
            (
                "shi",
                &[
                    ("是", 15_000_000),
                    ("时", 8_000_000),
                    ("事", 7_000_000),
                    ("实", 6_000_000),
                    ("市", 5_000_000),
                    ("识", 4_000_000),
                    ("士", 3_000_000),
                    ("世", 2_500_000),
                    ("视", 2_000_000),
                    ("式", 1_800_000),
                    ("示", 1_500_000),
                    ("石", 450_000),
                    ("祏", 1),
                ],
            ),
        ]);
        let gram_scorer = |tail: &str, word: &str, is_rear: bool| match (tail, word, is_rear) {
            (t, "之", false) if t.ends_with("贤者") => -0.5,
            (t, "石", true) if t.ends_with("贤者之") => 0.8,
            _ => -6.0,
        };
        let opts = LatticeOptions {
            beam: 12,
            nbest: 32,
            terminal_beam: 24,
            terminal_max_per_prefix: 2,
            terminal_max_per_suffix: 2,
            terminal_max_per_segmentation: 6,
            terminal_diversity_margin: 24.0,
            collocation_scorer: Some(&gram_scorer),
            collocation_weight: 1.0,
            collocation_edge_offset: 3.0,
            ..LatticeOptions::default()
        };
        let paths = decode("xian'zhe'zhi'shi", &rows, &opts);
        let sentences: Vec<&str> = paths.iter().map(|p| p.sentence.as_str()).collect();
        let top16: Vec<&str> = sentences.iter().take(16).copied().collect();
        assert!(
            top16.contains(&"贤者之石"),
            "多锚点 CN 与中间列单字限流应把「贤者之石」救回 Top-16: {top16:?}"
        );
        let top6_same_head = sentences
            .iter()
            .take(6)
            .filter(|s| s.starts_with("闲着"))
            .count();
        assert!(
            top6_same_head <= 3,
            "首词配额应阻止前 6 条被「闲着*」包场并保住次优锚点: {sentences:?}"
        );
        assert!(
            !top16.iter().any(|s| s.ends_with('祏')),
            "低频生僻单字拼凑噪声不应混入 Top-16 神经候选池: {top16:?}"
        );
    }

    #[test]
    fn cn_joint_single_pair_recovers_collocation_from_two_single_single_anchor() {
        // 当束宽有限使次优首词在中间列被剪枝、主锚点尾段为 `[1, 1]` 拆字（如「闲着+知+事」）且无匹配 `[2]` 词典项时，
        // 算子 C2 `[1, 1] -> [1, 1]` 联合重解码与首词种子锚点（「贤者+知+事」）须协同恢复「贤者之石」并上浮至前 6 席。
        let rows = table(&[
            ("xian'zhe", &[("闲着", 120_000), ("贤者", 15_000)]),
            (
                "zhi",
                &[
                    ("只", 5_000_000),
                    ("知", 4_400_000),
                    ("直", 1_900_000),
                    ("至", 1_200_000),
                    ("之", 1_170_000),
                ],
            ),
            (
                "shi",
                &[
                    ("是", 15_000_000),
                    ("时", 8_200_000),
                    ("事", 4_700_000),
                    ("石", 885_000),
                ],
            ),
        ]);
        let gram_scorer = |tail: &str, word: &str, is_rear: bool| {
            if tail == "贤者" && word == "之" {
                5.5
            } else if tail == "贤者之" && word == "石" && is_rear {
                12.0
            } else {
                -4.0
            }
        };
        let opts = LatticeOptions {
            beam: 4,
            nbest: 32,
            terminal_beam: 16,
            terminal_max_per_prefix: 2,
            terminal_max_per_suffix: 2,
            terminal_max_per_segmentation: 6,
            terminal_diversity_margin: 24.0,
            collocation_scorer: Some(&gram_scorer),
            collocation_weight: 1.0,
            collocation_edge_offset: 3.0,
            ..LatticeOptions::default()
        };
        let paths = decode("xian'zhe'zhi'shi", &rows, &opts);
        let top6: Vec<&str> = paths.iter().take(6).map(|p| p.sentence.as_str()).collect();
        assert!(
            top6.contains(&"贤者之石"),
            "算子 C2 [1,1]->[1,1] 与上浮逻辑应使「贤者之石」进入前 6 席: {top6:?}"
        );
    }
}

#[cfg(test)]
#[path = "graph_ownership_tests.rs"]
mod graph_ownership_tests;

#[cfg(test)]
#[path = "decode_hyp_layout_tests.rs"]
mod hyp_layout_tests;

#[cfg(test)]
#[path = "decode_capacity_tests.rs"]
mod capacity_tests;

#[cfg(test)]
#[path = "decode_traceback_tests.rs"]
mod traceback_tests;

#[cfg(test)]
#[path = "decode_return_tests.rs"]
mod return_tests;

#[cfg(test)]
#[path = "decode_prefix_tests.rs"]
mod prefix_tests;

#[cfg(test)]
#[path = "decode_outgoing_tests.rs"]
mod outgoing_tests;

#[cfg(test)]
#[path = "decode_entry_tests.rs"]
mod entry_tests;

#[cfg(test)]
#[path = "decode_stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "decode_materialization_tests.rs"]
mod materialization_tests;

#[cfg(test)]
#[path = "decode_terminal_tests.rs"]
mod terminal_tests;

#[cfg(test)]
#[path = "decode_constrained_tests.rs"]
mod constrained_tests;

#[cfg(test)]
#[path = "decode_row_index_tests.rs"]
mod row_index_tests;

#[cfg(test)]
#[path = "decode_collocation_tests.rs"]
mod collocation_tests;

#[cfg(test)]
#[path = "decode_collocation_tail_tests.rs"]
mod collocation_tail_tests;
