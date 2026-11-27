//! `QuanpinDictionary` (`R/quanpin/quanpin_dictionary.cpp`, quanpin.md §4, §6, §7.5-§7.7, §9.9, §11, §13, §14): the query pipeline and its caches, plus the canonical-pinyin phrase writer learning uses (pins, removals and frequency learning go through `user_dictionary` directly, see `ime::registry`).

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::assets::{MAIN_DICTIONARY, USER_JOURNAL};
use crate::cache::FifoCache;
use crate::dictionary::pinyin::PinyinDatabase;
use crate::dictionary::DictRow;
use crate::error::{EngineError, Result};
use crate::ime::online_batch::{
    replace_cached_online_candidate_batch, replace_online_candidate_batch,
};
use crate::lattice::decode::{
    decode_pinned, decode_sentences, edge_log_prob, BigramMemo, PinnedSpan, SentenceNgramTables,
};
use crate::lattice::merge::{
    decode_lattice, decode_lattice_borrowed, merge_decoded, merge_decoded_borrowed,
    whole_sentence_insert_position, DecodedSentences,
};
use crate::lattice::octagram::{pick_collocation_homophone, CONTEXT_CHARACTERS};
#[cfg(test)]
use crate::lattice::personal::PersonalTransition;
use crate::lattice::{SentencePath, TypoEdgeSource};
use crate::paths::RuntimePaths;
use crate::pinyin::autocorrect::{
    autocorrect_cut_kbest, looks_like_syllable_with_jianpin_tail, neighbor_two_letter_cuts,
    MAX_AUTOCORRECT_INPUT_LENGTH,
};
use crate::pinyin::fuzzy::{fuzzy_segmentations, FUZZY_SEGMENTATION_LIMIT};
use crate::pinyin::graph::{
    build_syllable_dag, build_syllable_graph, dag_candidate_paths, dag_complete_candidate_paths,
    enumerate_complete_segmentations, enumerate_kbest_complete_segmentations,
    DAG_HIATUS_SPLIT_COST, DAG_INITIAL_EDGE_COST, SYLLABLE_GRAPH_PATH_LIMIT,
};
use crate::pinyin::jianpin::{
    has_mixed_complete_and_initial, is_mixed_jianpin_lattice_eligible, matches_mixed_segments,
    split_initial_token_paths, QuerySource,
};
use crate::pinyin::segment::{
    cut_pinyin_by_mode, is_complete_pinyin_input, join_segments, split_segments, CutMode,
};
use crate::pinyin::syllables::{
    has_only_complete_pinyin_segments, normalize_umlaut_aliases, sparse_pinyin_fallback_segments,
};
use crate::text::{count_han_chars, last_characters};
use crate::types::{
    autocorrect_type, CandidateSource, FirstPageRows, FuzzyPinyinOptions, PersonalDictionaryKind,
    SentenceAssociationOptions, WordItem,
};
use crate::user_dictionary::journal::record_user_insert;
use crate::user_dictionary::ngram_store::PersonalNgramStore;
use crate::user_dictionary::typo_profile::PersonalTypoProfile;

use super::series::{
    append_unique_words, fold_reading, folded_reading_equal, mark_autocorrect_candidates,
    merge_alternative_rows, merge_alternative_segmentations, merge_series_by_coverage,
    resolve_series_query, series_cache_key, SeriesResolution,
    ALTERNATIVE_SEGMENTATION_CANDIDATE_LIMIT, AUTOCORRECT_CUT_KBEST, LONGER_PHRASE_EXTRA_SYLLABLES,
    LONGER_PHRASE_LIMIT, MAX_SYLLABLES_FOR_MULTIPLE_SEGMENTATIONS,
};
use super::typo_edges::collect_probabilistic_typo_edges;

#[cfg(test)]
mod marking_buffer_tests;

mod page_copy;
mod path_page;

use path_page::PathPage;

pub const CACHE_CAPACITY: usize = 128;
pub const TYPO_SPAN_CACHE_CAPACITY: usize = 512;
/// 词网格的跨度查询结果按跨度缓存的条数。九键每按一键要对几十条切分各解一次词网格，这些切分大量共用同样的跨度，下一键又会重复上一键的大部分跨度，所以容量按一次长输入里出现的不同跨度数来定。
pub const LATTICE_SPAN_CACHE_CAPACITY: usize = 4096;
/// 按切分缓存的词典原始行（`query_segments_keyed_flat`、单字母的 `query_initial`、`query_longer_phrases` 和模糊前缀的批量查询）每种的条数。`cache` 只有 128 条，九键一键的几十条切分会把它整个挤掉；这一层只存与查询方式无关的词典行，命中时与直接查库完全一样。
pub const ROW_CACHE_CAPACITY: usize = 512;
/// 按音节缓存的整句解码结果条数。九键每按一个数字要对二十条上下的切分各解一次整句，其中约三分之一是同一次刷新或上一个数字已经解过的音节串（同一条备选切分挂在好几条九键路径下）；一个长词打下来不同的音节串有两三百条。每条只存解码出的几条句子，内存很小。
pub const SENTENCE_DECODE_CACHE_CAPACITY: usize = 512;
/// A single-letter query returns at most this many rows until `expand_initial_candidates`.
pub const INITIAL_CANDIDATE_LIMIT: usize = 24;
pub const SPARSE_FALLBACK_THRESHOLD: usize = 8;
/// Twice the series cache's capacity: more keys than that cannot all still be cached.
pub const PERSONAL_SCORED_KEY_LIMIT: usize = 256;
/// The reference's `INT_MAX` row limit, kept as a finite value so every `LIMIT` bind stays in range.
const UNLIMITED_ROWS: usize = i32::MAX as usize;
/// The budget of fuzzy paths one `fuzzy_candidates` call may query, shared by every prefix length (QD:1691).
const FUZZY_PATH_BUDGET: usize = 128;
const FUZZY_ROW_LIMIT: usize = 128;
// k-best 纠错切分去掉主切分后最多 8 条，再加上完整音节图最多 32 条。
const ALTERNATIVE_SEGMENTATION_SEEN_CAPACITY: usize =
    AUTOCORRECT_CUT_KBEST - 1 + SYLLABLE_GRAPH_PATH_LIMIT;
/// The masks whose lists depend on the typo profile and carry typo sentences.
const TYPO_EDGE_TYPES: u32 = autocorrect_type::TRANSPOSITION
    | autocorrect_type::NEIGHBOR
    | autocorrect_type::MISSING_OR_EXTRA;

#[derive(Clone)]
struct CachedResolution {
    key: String,
    resolution: SeriesResolution,
}

#[derive(Clone)]
struct CachedFuzzyCandidates {
    rules: u32,
    segmentation: String,
    candidates: Vec<WordItem>,
}

#[derive(Clone)]
struct CachedFuzzyRows {
    rules: u32,
    limit: usize,
    typed: String,
    rows: Vec<DictRow>,
}

#[derive(Clone)]
struct CachedDecode {
    /// 解码时个人模型的版本；模型一变，条目作废。
    personal_version: u64,
    decoded: DecodedSentences,
}

#[derive(Clone)]
struct CachedLatticeSpan {
    span_limit: usize,
    span: String,
    rows: Vec<DictRow>,
}

enum ColdRows {
    Transfer,
    #[cfg(test)]
    Borrow,
}

enum ProjectedRows {
    Selected(Vec<WordItem>),
    /// 只供借用冷页使用，谓词已全部接受，尚未复制字段。
    #[cfg(test)]
    All,
}

impl From<Vec<WordItem>> for ProjectedRows {
    fn from(rows: Vec<WordItem>) -> Self {
        Self::Selected(rows)
    }
}

impl ProjectedRows {
    fn selected(self) -> Vec<WordItem> {
        match self {
            Self::Selected(rows) => rows,
            #[cfg(test)]
            Self::All => unreachable!("只有借用冷页可以延后复制"),
        }
    }
}

/// The reference answered a failed write with `ERROR_CODE` and no message; callers map any failure to their own diagnostic.
const DICTIONARY_UNAVAILABLE: &str = "Pinyin dictionary is unavailable";
const ENTRY_REJECTED: &str = "Pinyin does not spell the word one syllable per character";

pub struct QuanpinDictionary {
    database: PinyinDatabase,
    paths: RuntimePaths,
    sentence_tables: SentenceNgramTables,
    typo_profile: Arc<PersonalTypoProfile>,
    /// The profile generation the cached lists were built under.
    typo_generation: u64,
    personal: Arc<PersonalNgramStore>,
    /// The personal model version the personal-scored lists were built under.
    personal_version: u64,
    /// Series keys whose lists were scored with the personal model, dropped when its version moves.
    personal_scored_keys: HashSet<String>,
    /// 每条切分的词典页，完整展开页保留原来的拥有型表示。
    cache: FifoCache<String, PathPage>,
    /// Whole answers per series key, including online rows.
    series_cache: FifoCache<String, Vec<WordItem>>,
    segmentation_cache: FifoCache<String, Vec<String>>,
    /// Survives `reset_cache`: a resolution depends only on the input and the static correction tables, never on dictionary rows.
    resolution_cache: FifoCache<u64, CachedResolution>,
    /// Fuzzy rows are dictionary-dependent, but their composite key is indexed by hash so a cache hit does not allocate a formatted string.
    fuzzy_cache: FifoCache<u64, CachedFuzzyCandidates>,
    /// Rows of typo-variant span keys, empty answers included. Dictionary rows only, so it is cleared with the other caches.
    typo_span_cache: FifoCache<String, Vec<DictRow>>,
    /// 词网格每个跨度的 `query_lattice_span` 结果，空结果也存。哈希只做索引，命中后再核对上限和原样跨度，避免每次命中都拼接键字符串。
    lattice_span_cache: FifoCache<u64, CachedLatticeSpan>,
    /// 每条切分的 `query_segments_keyed_flat` 结果，键是切分本身；单字母的 `query_initial` 结果也存在这里，键前加 `\u{1}`。
    segment_row_cache: FifoCache<String, Vec<DictRow>>,
    /// 每条切分的 `query_longer_phrases` 结果，键是切分本身。
    longer_row_cache: FifoCache<String, Vec<DictRow>>,
    /// `fuzzy_candidates` 里每个前缀那一次批量查询的结果，键是模糊规则、路径上限和前缀切分；三者定下来，查的路径和返回的行就定了。
    fuzzy_row_cache: FifoCache<u64, CachedFuzzyRows>,
    /// 上面四个行缓存填入时的 `PRAGMA data_version`。它们只存词典行，随其他缓存一起清空；此外每次真正计算一个查询前都对一次版本，别的连接写过词典就先清掉，所以命中的行永远和此刻直接查库一样，不会像 `cache` 那样等到下次 `reset_cache_if_database_changed` 才更新。
    row_cache_version: Option<i64>,
    /// 词网格解码查二元分的备忘，跨查询保留；表不变它就一直有效，见 `BigramMemo`。
    bigram_memo: RefCell<BigramMemo>,
    /// 整句解码结果，键见 `sentence_decode_key`。
    sentence_decode_cache: FifoCache<String, CachedDecode>,
    /// octagram `.gram` 语法模型；路径由 `set_sentence_association` 给定、经
    /// `GramDb::shared` 进程内共享，缺失或非法时为 `None` 退回无搭配打分。
    collocation_db: Option<Arc<crate::lattice::octagram::GramDb>>,
    sentence_alternatives: bool,
    sentence_association: SentenceAssociationOptions,
    /// 首页放得下几行，决定全长组的补全词是按权重并进来还是接在整组末尾；来自宿主的每页候选数（`QueryRequest::first_page_rows`）。存成 `FirstPageRows` 是为了让 0 在这里没有入口，全拼和九键的「0 当 1」只在 `FirstPageRows::new` 做一次。
    first_page_rows: FirstPageRows,
    rescoring_context: String,
    /// The reading and mask of the query being answered; marking and the typo edges read them within that query.
    pinyin_segmentation: String,
    /// Every non-primary correction cut, joined; marking compares against them on every query, cached or not.
    alternative_segmentations: Vec<String>,
    current_autocorrect_types: u32,
}

impl QuanpinDictionary {
    /// Opens the generation's `msime-pinyin.db`, loads the n-gram tables, the personal context store and the typo profile off the keystroke path, and warms the statement cache (QD:231-268).
    pub fn new(paths: &RuntimePaths) -> Self {
        let database = PinyinDatabase::open(&paths.dictionary(MAIN_DICTIONARY));
        database.warm_up();
        // 构造时预热并保留表；共享缓存淘汰不能让按键查询重新映射和检查整张表。
        let sentence_tables = SentenceNgramTables::load(paths);
        let journal = paths.user(USER_JOURNAL);
        let personal = PersonalNgramStore::for_journal(&journal);
        drop(personal.model());
        let personal_version = personal.version();
        let typo_profile = PersonalTypoProfile::shared(&journal);
        let typo_generation = typo_profile.generation();
        let mut dictionary = Self {
            database,
            paths: paths.clone(),
            sentence_tables,
            typo_profile,
            typo_generation,
            personal,
            personal_version,
            personal_scored_keys: HashSet::new(),
            cache: FifoCache::new(CACHE_CAPACITY),
            series_cache: FifoCache::new(CACHE_CAPACITY),
            segmentation_cache: FifoCache::new(CACHE_CAPACITY),
            resolution_cache: FifoCache::new(CACHE_CAPACITY),
            fuzzy_cache: FifoCache::new(CACHE_CAPACITY),
            typo_span_cache: FifoCache::new(TYPO_SPAN_CACHE_CAPACITY),
            lattice_span_cache: FifoCache::new(LATTICE_SPAN_CACHE_CAPACITY),
            segment_row_cache: FifoCache::new(ROW_CACHE_CAPACITY),
            longer_row_cache: FifoCache::new(ROW_CACHE_CAPACITY),
            fuzzy_row_cache: FifoCache::new(ROW_CACHE_CAPACITY),
            row_cache_version: None,
            bigram_memo: RefCell::default(),
            sentence_decode_cache: FifoCache::new(SENTENCE_DECODE_CACHE_CAPACITY),
            collocation_db: None,
            sentence_alternatives: false,
            sentence_association: SentenceAssociationOptions::default(),
            first_page_rows: FirstPageRows::default(),
            rescoring_context: String::new(),
            pinyin_segmentation: String::new(),
            alternative_segmentations: Vec::new(),
            current_autocorrect_types: 0,
        };
        dictionary.reset_cache_if_database_changed();
        dictionary
    }

    /// `query_exact`, the fuzzy merge and letter-count sort, then autocorrect marking (QD:1711-1735). `segmentation` may be empty.
    pub fn query(
        &mut self,
        raw: &str,
        segmentation: &str,
        autocorrect_types: u32,
        fuzzy: FuzzyPinyinOptions,
    ) -> Vec<WordItem> {
        let mut result = self.query_exact(raw, segmentation, autocorrect_types);
        if fuzzy.rules != 0 && !raw.is_empty() {
            // Fuzzy rows are merged here, not in `query_exact`, so the ordinary cache slots stay free of preference-specific rows. The split is of the request's own segmentation, not the normalised one, so a typed `lue` expands under `lue` (quanpin.md §6).
            let normalized_segments = if segmentation.is_empty() {
                self.resolve_segments(raw, segmentation)
            } else {
                Vec::new()
            };
            let typed = fuzzy_segmentation(segmentation, &normalized_segments);
            let rows = self.fuzzy_candidates(typed.as_ref(), fuzzy);
            // 同词的字面前缀不能遮住覆盖更多输入的模糊整词。
            merge_series_by_coverage(&mut result, rows);
            yield_stand_in_sentence_to_fuzzy_rows(&mut result);
        }
        // Marking runs after every change, including the fuzzy merge, so every row the caller sees carries its mark.
        mark_autocorrect_candidates(
            &mut result,
            raw,
            &self.pinyin_segmentation,
            &self.alternative_segmentations,
        );
        result
    }

    /// 借用完整行筛选；只有核对失效后的系列热页允许提前排名。
    #[cfg(test)]
    pub(crate) fn query_filtered_rows(
        &mut self,
        raw: &str,
        segmentation: &str,
        fuzzy: FuzzyPinyinOptions,
        mut keep: impl FnMut(&WordItem, bool) -> bool,
    ) -> Vec<WordItem> {
        if fuzzy.rules != 0 {
            // 模糊合并的去重和站位必须先完成，避免被筛掉的词让另一条同文行复活。
            let mut rows = self.query(raw, segmentation, 0, fuzzy);
            rows.retain(|row| keep(row, false));
            return rows;
        }
        let mut result =
            self.query_exact_projected(raw, segmentation, 0, ColdRows::Borrow, |rows, cached| {
                match rows {
                    Cow::Owned(mut rows) => {
                        rows.retain(|row| keep(row, cached));
                        ProjectedRows::Selected(rows)
                    }
                    Cow::Borrowed(rows) => {
                        if !cached {
                            // 谓词全部成功前不淘汰缓存；全留时留给外层复用旧页复制。
                            let Some(rejected) = rows.iter().position(|row| !keep(row, false))
                            else {
                                return ProjectedRows::All;
                            };
                            let mut result = Vec::with_capacity(rows.len());
                            result.extend_from_slice(&rows[..rejected]);
                            result.extend(
                                rows[rejected + 1..]
                                    .iter()
                                    .filter(|row| keep(row, false))
                                    .cloned(),
                            );
                            return ProjectedRows::Selected(result);
                        }
                        // 容器预留完整行数，只跳过拒绝行的字段复制。
                        let mut result = Vec::with_capacity(rows.len());
                        result.extend(rows.iter().filter(|row| keep(row, cached)).cloned());
                        ProjectedRows::Selected(result)
                    }
                }
            });
        mark_autocorrect_candidates(
            &mut result,
            raw,
            &self.pinyin_segmentation,
            &self.alternative_segmentations,
        );
        result
    }

    /// 旧筛选接口只供冻结刷新与既有分配对照使用。
    #[cfg(test)]
    pub(crate) fn query_filtered(
        &mut self,
        raw: &str,
        segmentation: &str,
        fuzzy: FuzzyPinyinOptions,
        mut keep: impl FnMut(&str, &str, &str, bool) -> bool,
    ) -> Vec<WordItem> {
        self.query_filtered_rows(raw, segmentation, fuzzy, |row, _| {
            keep(&row.word, &row.pinyin, &row.canonical_pinyin, row.fuzzy)
        })
    }

    /// 把选中的词记进个人上下文模型。
    #[cfg(test)]
    pub fn record_personal_use(&self, transitions: &[PersonalTransition]) -> Result<()> {
        self.personal.record(transitions)
    }

    /// 简拼 `codes`（同样长度，一个字母一个音节）对应的词条，按权重从高到低最多 `limit` 行，见 `PinyinDatabase::query_jianpin_codes`。
    #[cfg(test)]
    pub fn query_jianpin_codes(&self, codes: &[String], limit: usize) -> Vec<WordItem> {
        initial_items(self.database.query_jianpin_codes(codes, limit))
    }

    /// 首字母的 24 行候选展开为全部行（QD:506-558），同时更新行缓存与产生该列表的查询槽。
    pub fn expand_initial_candidates(
        &mut self,
        raw: &str,
        segmentation: &str,
        autocorrect_types: u32,
        code: &str,
        candidates: &mut Vec<WordItem>,
    ) -> bool {
        if code.len() != 1 {
            return false;
        }
        let is_limited_initial =
            |item: &WordItem| item.source == CandidateSource::Database && item.pinyin == code;
        let limited_count = candidates
            .iter()
            .filter(|item| is_limited_initial(item))
            .count();
        // Exactly the cap: fewer rows means the initial has nothing more to give.
        if limited_count != INITIAL_CANDIDATE_LIMIT {
            return false;
        }
        let expanded = matched_items(self.database.query_initial(code, UNLIMITED_ROWS), code);
        if expanded.len() <= limited_count {
            return false;
        }
        let first_limited = candidates
            .iter()
            .position(is_limited_initial)
            .expect("limited_count checked above");
        candidates.reserve_exact(expanded.len() - limited_count);
        let mut removed = 0;
        candidates.retain(|item| {
            if is_limited_initial(item) {
                removed += 1;
                false
            } else {
                true
            }
        });
        debug_assert_eq!(removed, limited_count);
        candidates.splice(first_limited..first_limited, expanded.iter().cloned());
        self.cache.insert(code.to_string(), expanded.into());
        // 按参考规则写入不带 `C:` 的槽（QD:554）：单字母不纠错。根据调用方查询重算键，避免第二次光标前缀查询把整段输入的展开列表写入自己的槽。
        let cache_key =
            self.project_resolution(raw, segmentation, autocorrect_types, |resolution| {
                series_cache_key(raw, &resolution.segmentation, autocorrect_types)
            });
        let key = self.series_slot(&cache_key);
        self.series_cache.insert(key, candidates.clone());
        true
    }

    /// 切分的模糊候选按规则与读法缓存（QD:1680-1709），双拼也复用该路径。
    pub fn fuzzy_candidates(
        &mut self,
        segmentation: &str,
        options: FuzzyPinyinOptions,
    ) -> Vec<WordItem> {
        if options.rules == 0 || !self.database.is_open() {
            return Vec::new();
        }
        self.reset_cache_if_database_changed();
        let hash = fuzzy_cache_hash(options.rules, segmentation);
        if let Some(cached) = self.fuzzy_cache.get_ref(&hash) {
            if fuzzy_cache_key_matches(cached, options.rules, segmentation) {
                return cached.candidates.clone();
            }
        }
        self.validate_row_caches();
        let segments: Vec<&str> = if segmentation.is_empty() {
            Vec::new()
        } else {
            let count = segmentation.bytes().filter(|&byte| byte == b'\'').count() + 1;
            let mut segments = Vec::with_capacity(count);
            segments.extend(segmentation.split('\''));
            segments
        };
        let mut result = Vec::new();
        let mut budget = FUZZY_PATH_BUDGET;
        let mut typed = segmentation;
        for count in (1..=segments.len()).rev() {
            if budget <= 1 {
                break;
            }
            if count < segments.len() {
                typed = typed.rsplit_once('\'').map_or("", |(prefix, _)| prefix);
            }
            let prefix = &segments[..count];
            let limit = FUZZY_SEGMENTATION_LIMIT.min(budget);
            // 路径照样算出来（纯计算，不查库），预算照旧按路径数扣；只有查库这一步走缓存。
            let paths = fuzzy_segmentations(prefix, options, limit);
            if paths.is_empty() {
                continue;
            }
            budget -= paths.len();
            append_cached_fuzzy_rows(
                &self.database,
                &mut self.fuzzy_row_cache,
                &paths,
                options.rules,
                limit,
                typed,
                &mut result,
            );
        }
        self.fuzzy_cache.insert(
            hash,
            CachedFuzzyCandidates {
                rules: options.rules,
                segmentation: segmentation.to_owned(),
                candidates: result.clone(),
            },
        );
        result
    }

    /// `replace_online_candidate_batch` on the series cache slot of `(raw, segmentation, autocorrect_types)` (QD:1737-1773).
    pub fn insert_online_words(
        &mut self,
        raw: &str,
        segmentation: &str,
        autocorrect_types: u32,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        if raw.is_empty() || words.is_empty() {
            return false;
        }
        // 按查询原规则重算键，使候选写入下次刷新读取的槽。
        let cache_key =
            self.project_resolution(raw, segmentation, autocorrect_types, |resolution| {
                match resolution {
                    Cow::Borrowed(resolution) => resolution.cache_key.clone(),
                    Cow::Owned(resolution) => resolution.cache_key,
                }
            });
        let key = self.series_slot(&cache_key);
        if let Some(list) = self.series_cache.get_mut(&key) {
            return replace_cached_online_candidate_batch(list, raw, words, source);
        }
        let mut list = Vec::new();
        if !replace_online_candidate_batch(&mut list, raw, words, source) {
            return false;
        }
        self.series_cache.insert(key, list);
        true
    }

    /// Remove one provider's rows from every cached answer while retaining the other provider and dictionary rows.
    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        self.series_cache.retain_mut(|_, rows| {
            rows.retain(|item| item.source != source);
            !rows.is_empty()
        });
    }

    pub fn find_candidate(&self, key: &str, value: &str) -> Option<WordItem> {
        self.database
            .find_weight(key, value)
            .map(|weight| WordItem::new(key, value, weight, CandidateSource::Database, key))
    }

    pub fn knows_han_char(&self, han: &str) -> bool {
        self.database.han_char_exists(han)
    }

    /// 整句改字的候选：读音恰为 `span`（完整音节）的词条，按权重从高到低，至多 `limit` 条。
    pub fn conversion_rows(&mut self, span: &[String], limit: usize) -> Vec<DictRow> {
        self.validate_row_caches();
        cached_lattice_span(&self.database, &mut self.lattice_span_cache, span, limit)
    }

    /// 整句改字的重新转换：与整句候选同一套词网格、n 元表和个人模型，钉住的段原样保留（`decode_pinned`）。
    pub fn convert_pinned(&mut self, syllables: &[String], pins: &[PinnedSpan]) -> Vec<PinnedSpan> {
        self.validate_row_caches();
        let personal = Arc::clone(&self.personal);
        let model = personal.model();
        let mut options = self.sentence_tables.options(false);
        options.personal = Some(&*model);
        options.bigram_memo = Some(&self.bigram_memo);
        let span_limit = options.span_limit;
        let database = &self.database;
        let lattice_spans = &mut self.lattice_span_cache;
        let mut lookup =
            |span: &[String]| cached_lattice_span(database, lattice_spans, span, span_limit);
        decode_pinned(syllables, &mut lookup, &options, pins)
    }

    /// Umlaut-normalised canonical key, one complete syllable per Han character, no re-cut; an existing row is OK without a journal write (QD:1215-1268).
    pub fn create_word_from_canonical_pinyin(&mut self, pinyin: &str, word: &str) -> Result<()> {
        // Online rows arrive with the typed spelling; they are stored under the nve/lve/ju keys queries resolve to.
        let mut segments = split_segments(pinyin);
        normalize_umlaut_aliases(&mut segments);
        // The caller gave an explicit segmentation, and these checks already prove one complete syllable per character. Validating through a re-cut would erase the boundaries and cut qi'e'huan as qie'huan, rejecting a valid phrase.
        if segments.is_empty()
            || segments.len() != count_han_chars(word)
            || segments
                .iter()
                .any(|segment| segment.is_empty() || !is_complete_pinyin_input(segment))
        {
            return Err(EngineError::invalid(ENTRY_REJECTED));
        }
        self.insert_new_word(&join_segments(&segments), word)
    }

    /// A change clears the row and series caches.
    pub fn set_sentence_alternatives(&mut self, enabled: bool) {
        if self.sentence_alternatives == enabled {
            return;
        }
        self.sentence_alternatives = enabled;
        // The cached lists were assembled under the previous answer.
        self.cache.clear();
        self.series_cache.clear();
        self.sentence_decode_cache.clear();
    }

    /// 首页行数只决定补全词是按权重并进来还是接在整组末尾；改了就清掉按旧门槛组装的整条答案（照 `set_sentence_alternatives`）。`cache` 存的是按切分查出来的词库行，补全词是查完它才并进来的，所以这里动不了它，清掉只会白白丢掉 128 行、再查一次 SQLite。
    pub fn set_first_page_rows(&mut self, rows: FirstPageRows) {
        if self.first_page_rows == rows {
            return;
        }
        self.first_page_rows = rows;
        self.series_cache.clear();
    }

    /// 设置整句联想与 `.gram` 语法模型参数；变更时清空缓存。
    pub fn set_sentence_association(&mut self, options: SentenceAssociationOptions) {
        if self.sentence_association == options {
            return;
        }
        // 模型路径为空或不可载时回退 `None`：与 `msime-bigram.bin` 缺失的既有降级一致。
        self.collocation_db = if options.collocation_model.as_os_str().is_empty() {
            None
        } else {
            crate::lattice::octagram::GramDb::shared(&options.collocation_model)
        };
        self.sentence_association = options;
        self.reset_cache();
    }

    /// Keeps the part of the committed text the collocation scorer reads; while active it is part of every series slot (`series_slot`).
    pub fn set_rescoring_context(&mut self, context: &str) {
        let trimmed = last_characters(context, CONTEXT_CHARACTERS);
        if self.rescoring_context != trimmed {
            self.rescoring_context = trimmed.to_string();
        }
    }

    /// 是否已挂载 `.gram` 搭配重排器，供首组同音字典候选按前文重排。
    pub fn has_dictionary_reranker(&self) -> bool {
        self.collocation_db.is_some() && self.sentence_association.collocation_weight != 0.0
    }

    /// 用可选 `.gram` 语法搭配模型按 `context` 在多字同音词中选出应提至首位的下标。
    pub fn pick_dictionary_homophone(
        &mut self,
        items: &[&WordItem],
        context: &str,
        effective_weights: Option<&[f64]>,
    ) -> Option<usize> {
        pick_collocation_homophone(
            self.collocation_db.as_deref(),
            context,
            items,
            self.sentence_association.collocation_weight,
            &self.sentence_association.collocation_grammar,
            effective_weights,
        )
    }

    /// 只读查询 `.gram` 语法模型在给定前文尾窗上的非负搭配净增益；未启用或权重非正时返回 `None`。
    pub fn query_collocation_bonus(
        &self,
        context_tail: &str,
        word: &str,
        is_rear: bool,
    ) -> Option<f64> {
        let db = self.collocation_db.as_deref()?;
        let weight = self.sentence_association.collocation_weight;
        if weight <= 0.0 || word.is_empty() {
            return None;
        }
        let config = &self.sentence_association.collocation_grammar;
        let raw = db.query(context_tail, word, is_rear, config);
        Some((weight * (raw - config.non_collocation_penalty)).max(0.0))
    }

    /// 是否挂载了会读取上文语义的整句组件（非零权重的 `.gram` 搭配模型）。
    fn sentence_reads_context(&self) -> bool {
        self.has_dictionary_reranker()
    }

    /// The series cache slot of a resolution's key (overlays.md §1.6.2). The association switches decide which sentence rows a list carries, so they are always part of the slot. The reference bypassed the cache while a neural model ran, because its scores came from an asynchronous worker; scoring here is synchronous, so a reranked list is cached under the trimmed context it was scored with and a commit moves the next query to a fresh slot instead of clearing every list.
    fn series_slot(&self, cache_key: &str) -> String {
        series_slot_key(
            cache_key,
            &self.sentence_association,
            self.sentence_reads_context(),
            &self.rescoring_context,
        )
    }

    /// Clears everything except the resolution memo (QD:1400-1407).
    pub fn reset_cache(&mut self) {
        self.cache.clear();
        self.series_cache.clear();
        self.personal_scored_keys.clear();
        self.segmentation_cache.clear();
        self.fuzzy_cache.clear();
        self.typo_span_cache.clear();
        self.clear_row_caches();
    }

    fn clear_row_caches(&mut self) {
        // 整句是从这些行解出来的。
        self.sentence_decode_cache.clear();
        self.lattice_span_cache.clear();
        self.segment_row_cache.clear();
        self.longer_row_cache.clear();
        self.fuzzy_row_cache.clear();
    }

    /// 只读当前版本号，不改 `database_changed` 记下的那份，所以其他缓存何时重置和以前完全一样。读不到版本（词典没打开或读取失败）时每次都清空，等于不缓存。
    fn validate_row_caches(&mut self) {
        let version = self.database.data_version();
        if version.is_none() || version != self.row_cache_version {
            self.clear_row_caches();
            self.row_cache_version = version;
        }
    }

    /// QD:320-482.
    fn query_exact(&mut self, raw: &str, segmentation: &str, types: u32) -> Vec<WordItem> {
        self.query_exact_projected(raw, segmentation, types, ColdRows::Transfer, |rows, _| {
            rows.into_owned()
        })
    }

    /// 同步投影只能筛选、复制或移交原行，不改字段和顺序；冷页全保留时可交换副本与原页。
    /// 回调的布尔值只标识核对失效后的系列热命中，借用的冷页仍为 `false`。
    fn query_exact_projected<P: Into<ProjectedRows>>(
        &mut self,
        raw: &str,
        segmentation: &str,
        types: u32,
        cold: ColdRows,
        project: impl FnOnce(Cow<'_, [WordItem]>, bool) -> P,
    ) -> Vec<WordItem> {
        if raw.is_empty() {
            return project(Cow::Owned(Vec::new()), false).into().selected();
        }
        self.current_autocorrect_types = types;
        if types & TYPO_EDGE_TYPES != 0 {
            // Only a stat per query unless the journal changed. Lists built without a mask never depend on the profile.
            self.typo_profile.refresh_if_changed();
            let generation = self.typo_profile.generation();
            if generation != self.typo_generation {
                self.typo_generation = generation;
                self.series_cache.clear();
                // 纠错整句按纠错资料规划。
                self.sentence_decode_cache.clear();
                self.personal_scored_keys.clear();
            }
        }
        let mut segments = None;
        let sentence_reads_context = self.sentence_reads_context();
        let segmentation_cache = &mut self.segmentation_cache;
        let association = self.sentence_association.clone();
        let context = &self.rescoring_context;
        let pinyin_segmentation = &mut self.pinyin_segmentation;
        let (resolution_hash, (resolution, marking_alternatives, slot)) =
            project_cached_resolution_with(
                &mut self.resolution_cache,
                raw,
                segmentation,
                types,
                || {
                    let segments = segments.get_or_insert_with(|| {
                        resolve_cached_segments(segmentation_cache, raw, segmentation)
                    });
                    resolve_series_query(raw, segments, types)
                },
                |resolution| {
                    // 热命中也发布本次标记；高成本读法仍算纠错。
                    pinyin_segmentation.clone_from(&resolution.segmentation);
                    let alternatives = resolution
                        .alternative_corrected_cuts
                        .iter()
                        .chain(&resolution.costlier_corrected_cuts)
                        .map(|cut| join_segments(cut))
                        .collect();
                    let slot = series_slot_key(
                        &resolution.cache_key,
                        &association,
                        sentence_reads_context,
                        context,
                    );
                    let owned = match resolution {
                        Cow::Borrowed(_) => None,
                        Cow::Owned(resolution) => Some(resolution),
                    };
                    (owned, alternatives, slot)
                },
            );
        self.alternative_segmentations = marking_alternatives;
        self.drop_personal_scored_results();
        if self.series_cache.contains(&slot) {
            if self.database.database_changed() {
                // 清空切分备忘前取得重建需要的音节，保留原先的缓存复用与分配成本。
                segments.get_or_insert_with(|| self.resolve_segments(raw, segmentation));
                self.reset_cache();
            }
            if let Some(cached) = self.series_cache.get_ref(&slot) {
                return project(Cow::Borrowed(cached), true).into().selected();
            }
        }

        let segments = segments.unwrap_or_else(|| self.resolve_segments(raw, segmentation));

        // 候选槽失效不会改解析备忘；复用已核对的索引，避免重复哈希和完整键校验。
        let resolution = resolution.unwrap_or_else(|| {
            self.resolution_cache
                .get_ref(&resolution_hash)
                .expect("候选失效检查必须保留解析备忘")
                .resolution
                .clone()
        });

        // 下面每一次读词典都可能走行缓存，先确认它们还和库里一致。
        self.validate_row_caches();
        let alternatives = alternative_segmentations(raw, &segments, &resolution, types);
        let primary_segmentation = resolution.segmentation.as_str();
        let mut result;
        // 只有主切分留下键盘模型的次选整句：备选切分的次选大多是生造的读法（`da'o'shi'kan` 的「打哦试看」），而它们自己的两条整句已经在列表里。
        let mut runner_ups = Vec::new();
        // 比主切分多拆音节的隔音备选切分整句，扣减虚增的词组长度分并接在主切分次选之后。
        let mut extra_syllable_runner_ups = Vec::new();
        if resolution.corrected_input {
            // 纠错读法先生成主序列与同代价备选，随后再与字面读法按统一信道对数概率融合，
            // 防止 `shj/shh/zhw/shz/xigz/zenmy` 等高频简拼词被数百条单字或短词纠错行埋到后排。
            result = self.query_primary_series(
                raw,
                primary_segmentation,
                &resolution.corrected,
                &mut runner_ups,
            );
            let literal = if segmentation.is_empty() {
                let literal_segmentation = join_segments(&segments);
                self.query_series(raw, &literal_segmentation, &segments)
            } else {
                self.query_series(raw, segmentation, &segments)
            };
            // 同代价备选纠错读法先与主纠错切分按词频合并。
            if !alternatives.is_empty() {
                result = self.merge_alternatives_with_runner_ups(
                    raw,
                    primary_segmentation,
                    &resolution.corrected,
                    &alternatives,
                    result,
                    Some(&mut extra_syllable_runner_ups),
                );
            }
            // 更高代价纠错读法接在主纠错层之后。
            for costlier in &resolution.costlier_corrected_cuts {
                let rows = self.query_series(raw, &join_segments(costlier), costlier);
                append_unique_words(&mut result, rows);
            }
            // 先于 `merge_literal_jianpin_with_corrected` 融合备选完整音节/吞音候选，避免尾部简拼长句（`mo'hu'yi'g'n -> 模糊一个年`）混入整句排序池。
            if types != 0 && !raw.contains('\'') {
                self.merge_syllable_dag_competing_cuts(
                    &mut result,
                    raw,
                    &segments,
                    &resolution,
                    &alternatives,
                    types,
                );
            }
            self.merge_literal_jianpin_with_corrected(
                &mut result,
                literal,
                raw,
                &segments,
                &resolution.corrected,
            );
        } else {
            result =
                self.query_primary_series(raw, primary_segmentation, &segments, &mut runner_ups);
            if !alternatives.is_empty() {
                result = self.merge_alternatives_with_runner_ups(
                    raw,
                    primary_segmentation,
                    &segments,
                    &alternatives,
                    result,
                    Some(&mut extra_syllable_runner_ups),
                );
            }
            if types != 0 && !raw.contains('\'') && !has_only_complete_pinyin_segments(&segments) {
                self.merge_competing_incomplete_typo_cuts(&mut result, raw, &segments, types);
            }
            if types != 0 && !raw.contains('\'') {
                self.merge_syllable_dag_competing_cuts(
                    &mut result,
                    raw,
                    &segments,
                    &resolution,
                    &alternatives,
                    types,
                );
            }
        }
        // 接在所有切分、补全词和单字之后：放在主切分自己的那段末尾时，另一切分的正确读法（`xieshanga` 的「协商啊」）会排到它们后面，被 `runner_up_order` 挪去列表末尾。
        append_unique_words(&mut result, runner_ups);
        append_unique_words(&mut result, extra_syllable_runner_ups);
        let (cached, projected) = match cold {
            ColdRows::Transfer => {
                let cached = result.clone();
                (
                    Some(cached),
                    project(Cow::Owned(result), false).into().selected(),
                )
            }
            #[cfg(test)]
            ColdRows::Borrow => {
                match project(Cow::Borrowed(&result), false).into() {
                    ProjectedRows::All => (None, result),
                    ProjectedRows::Selected(projected) => {
                        if projected.len() == result.len() {
                            // 既有投影回调已复制的全页仍直接入缓存。
                            (Some(projected), result)
                        } else {
                            // 外层富余容量不随整页转移而长期留在 FIFO 中。
                            result.shrink_to_fit();
                            (Some(result), projected)
                        }
                    }
                }
            }
        };
        // 单音节主路径也可能沿完整备选切分组句，需要跟随个人模型失效。
        let personal_scored = segments.len() >= 2
            || resolution.corrected.len() >= 2
            || alternatives
                .iter()
                .any(|path| path.len() >= 2 && is_literal_segmentation(raw, path));
        let personal_slot = personal_scored.then(|| slot.clone());
        if let Some(cached) = cached {
            self.series_cache.insert(slot, cached);
        } else {
            let (cached, previous) = self.series_cache.insert_with_previous(slot, Vec::new());
            *cached = page_copy::clone_rows_reusing(&projected, previous.unwrap_or_default());
        }
        if let Some(slot) = personal_slot {
            self.remember_personal_scored_key(slot);
        }
        projected
    }

    /// 完整词继续按词频合并，字面备选另外补齐前缀和整句，不能叠加一层纠错。
    #[cfg(test)]
    fn merge_alternatives(
        &mut self,
        raw: &str,
        primary_segmentation: &str,
        primary_segments: &[String],
        alternatives: &[Vec<String>],
        result: Vec<WordItem>,
    ) -> Vec<WordItem> {
        self.merge_alternatives_with_runner_ups(
            raw,
            primary_segmentation,
            primary_segments,
            alternatives,
            result,
            None,
        )
    }

    /// 完整词继续按词频合并，字面备选另外补齐前缀和整句，不能叠加一层纠错。
    ///
    /// 比主切分多拆出音节的字面切分（隔音备选），其整句行会少受一次单字归一惩罚而偏高，
    /// 因此把多出的音节数记入 `extra_syllables`，让 `static_evidence` 扣除对应词长奖励而不改原始 `weight`；
    /// 启用神经重排时同步标为 `keyboard_runner_up` 并移至主切分次选之后，避免在逐键重排和首屏预算里压过主切分合法候选。
    fn merge_alternatives_with_runner_ups(
        &mut self,
        raw: &str,
        primary_segmentation: &str,
        primary_segments: &[String],
        alternatives: &[Vec<String>],
        result: Vec<WordItem>,
        mut extra_syllable_runner_ups: Option<&mut Vec<WordItem>>,
    ) -> Vec<WordItem> {
        let mut result = self.merge_alternative_full(
            raw,
            primary_segmentation,
            primary_segments,
            alternatives,
            result,
        );
        let mut supplemental = Vec::new();
        // 相同音节前缀在多条完整路径中会重复出现；它们的行和消费范围相同，只查一次。
        let mut queried_prefixes = HashMap::new();
        if is_literal_segmentation(raw, primary_segments) {
            for count in 1..primary_segments.len() {
                let key = join_segments(&primary_segments[..count]);
                if let Some(rows) = self.cache.get_ref_by(&key) {
                    queried_prefixes.insert(key, rows.len());
                }
            }
        }
        let _ = &mut extra_syllable_runner_ups;
        let mut alt_sentences = Vec::new();
        let legacy_phrase_bonus_milli =
            (crate::lattice::decode::PHRASE_LENGTH_BONUS * 1000.0).round() as i64;
        let has_active_collocation =
            self.collocation_db.is_some() && self.sentence_association.collocation_weight != 0.0;
        // 挂载 `.gram` 时，多拆音节路径每多出一段单字切分都会白拿一份 `collocation_edge_offset`（`+3.0 nats`），
        // 须在基础 `-0.60 nats` 信道代价之上扣回 `-2.20 nats` 偏移红利，防止 `yu'de'da'o -> 余地大哦` / `gu'a -> 姑阿` 挤占首页。
        let split_cost_milli =
            ((DAG_HIATUS_SPLIT_COST - if has_active_collocation { 2.20 } else { 0.0 }) * 1000.0)
                .round() as i64;
        if primary_segments.len() > MAX_SYLLABLES_FOR_MULTIPLE_SEGMENTATIONS {
            // 长句快速通道：仅保留变化边界在主词典中命中真实多字词的 Top-4 备选切分，
            // 直接复用 `lattice_span_cache` 做整句解码，跳过重复前缀 SQLite 查询，将长句耗时砍低 10-15x。
            const LONG_SENTENCE_ALT_DECODE_LIMIT: usize = 4;
            let probe_limit = self.sentence_tables.options(false).span_limit;
            let mut ranked_paths: Vec<(&[String], i64)> = Vec::new();
            for path in alternatives {
                if !is_literal_segmentation(raw, path) {
                    continue;
                }
                if let Some(lexical_weight) = long_sentence_alternative_lexical_score(
                    &self.database,
                    &mut self.lattice_span_cache,
                    probe_limit,
                    primary_segments,
                    path,
                ) {
                    ranked_paths.push((path.as_slice(), lexical_weight));
                }
            }
            ranked_paths.sort_by_key(|lhs| std::cmp::Reverse(lhs.1));
            ranked_paths.truncate(LONG_SENTENCE_ALT_DECODE_LIMIT);
            for (path, _) in ranked_paths {
                let joined = join_segments(path);
                let mut scratch = Vec::new();
                self.merge_sentences_with_typos(&mut scratch, raw, &joined, path, false, None);
                let extra_splits = path.len().saturating_sub(primary_segments.len());
                let channel_adjust = (extra_splits as i64)
                    .saturating_mul(split_cost_milli.saturating_sub(legacy_phrase_bonus_milli));
                for mut item in scratch {
                    if item.sentence_association {
                        if extra_splits > 0 {
                            item.weight = item.weight.saturating_add(channel_adjust);
                        }
                        item.extra_syllables = 0;
                        item.keyboard_runner_up = false;
                        alt_sentences.push(item);
                    }
                }
            }
        } else {
            for path in alternatives {
                if is_literal_segmentation(raw, path) {
                    let rows = self.query_series_with_typos(
                        raw,
                        &join_segments(path),
                        path,
                        false,
                        Some(&mut queried_prefixes),
                        None,
                    );
                    let extra_splits = path.len().saturating_sub(primary_segments.len());
                    // 额外零声母边界只施加小额真实信道代价（`-0.60 nats`/额外边界），绝不设 `-30 nats` 死刑或放逐至 `keyboard_runner_up`。
                    let channel_adjust = (extra_splits as i64)
                        .saturating_mul(split_cost_milli.saturating_sub(legacy_phrase_bonus_milli));
                    for mut item in rows {
                        if item.sentence_association {
                            if extra_splits > 0 {
                                item.weight = item.weight.saturating_add(channel_adjust);
                            }
                            item.extra_syllables = 0;
                            item.keyboard_runner_up = false;
                            alt_sentences.push(item);
                        } else {
                            supplemental.push(item);
                        }
                    }
                }
            }
        }
        if !supplemental.is_empty() {
            merge_series_by_coverage(&mut result, supplemental);
        }
        if !alt_sentences.is_empty() {
            merge_alternative_sentences_by_score(&mut result, raw, primary_segments, alt_sentences);
            // 若高置信纠错整句（如 `yingai -> 应该`）紧随弱词典同音词（如 `ying'ai -> 盈爱`）之后且概率净胜 `+2.5 nats` 以上，
            // 直接提至首位，避免弱生僻词压制明显错拼（高频真实词如 `一斤`/`喜欢` 不会被翻转）。
            if let Some(typo_idx) = result
                .iter()
                .take(12)
                .position(|item| item.sentence_association && !item.corrected_from.is_empty())
            {
                if typo_idx > 0
                    && primary_segments.len() == 2
                    && result[0].source.is_dictionary()
                    && result[0].corrected_from.is_empty()
                    && result[0].weight < 5_000
                {
                    let opts = self.sentence_tables.options(false);
                    let dict_syls = result[0]
                        .canonical_pinyin
                        .split('\'')
                        .filter(|p| !p.is_empty())
                        .count()
                        .max(1);
                    if dict_syls == 2 {
                        let dict_score_milli =
                            (edge_log_prob(result[0].weight, dict_syls, &opts) * 1000.0) as i64;
                        if result[typo_idx].weight > dict_score_milli + 2500 {
                            let promoted = result.remove(typo_idx);
                            result.insert(0, promoted);
                        }
                    }
                }
            }
        }
        result
    }

    /// `merge_alternative_segmentations` 的完整词查询部分，升位与合并规则在 `series`。
    fn merge_alternative_full(
        &mut self,
        raw: &str,
        primary_segmentation: &str,
        primary_segments: &[String],
        alternatives: &[Vec<String>],
        result: Vec<WordItem>,
    ) -> Vec<WordItem> {
        let rows = self.database.query_exact_segmentations_keyed_flat(
            alternatives,
            ALTERNATIVE_SEGMENTATION_CANDIDATE_LIMIT,
        );
        if rows.is_empty() {
            return result;
        }
        let cache_key = path_cache_key(raw, primary_segmentation);
        enum PrimaryPage<'a> {
            Cached(&'a PathPage),
            Owned(Vec<WordItem>),
        }
        let primary_full = if let Some(primary_full) = self.cache.get_ref_by(cache_key) {
            PrimaryPage::Cached(primary_full)
        } else {
            PrimaryPage::Owned(self.query_single_path_miss(
                cache_key,
                primary_segmentation,
                primary_segments,
            ))
        };
        let alternative_full = rows
            .into_iter()
            .map(|row| {
                WordItem::new(
                    row.key.clone(),
                    row.value,
                    row.weight,
                    CandidateSource::Database,
                    row.key,
                )
            })
            .collect();
        match primary_full {
            PrimaryPage::Cached(page) => {
                merge_alternative_rows(result, page.cloned_rows(cache_key), alternative_full)
            }
            PrimaryPage::Owned(rows) => {
                merge_alternative_segmentations(result, rows, alternative_full)
            }
        }
    }

    fn append_query_rows(result: &mut Vec<WordItem>, rows: Vec<WordItem>) {
        if result.is_empty() && result.capacity() == 0 {
            *result = rows;
            return;
        }
        result.reserve(rows.len());
        result.extend(rows);
    }

    /// 对应 QD:560-730：从长到短查前缀组，再合并词网格、纠错整句与稀疏回退。
    fn query_series(
        &mut self,
        raw: &str,
        segmentation: &str,
        segments: &[String],
    ) -> Vec<WordItem> {
        self.query_series_with_typos(raw, segmentation, segments, true, None, None)
    }

    /// 主切分的 `query_series`：键盘模型的次选整句交给 `runner_ups`，由调用方接在整张列表末尾。
    fn query_primary_series(
        &mut self,
        raw: &str,
        segmentation: &str,
        segments: &[String],
        runner_ups: &mut Vec<WordItem>,
    ) -> Vec<WordItem> {
        self.query_series_with_typos(raw, segmentation, segments, true, None, Some(runner_ups))
    }

    fn query_series_with_typos(
        &mut self,
        raw: &str,
        segmentation: &str,
        segments: &[String],
        include_typos: bool,
        mut queried_prefixes: Option<&mut HashMap<String, usize>>,
        runner_ups: Option<&mut Vec<WordItem>>,
    ) -> Vec<WordItem> {
        if segments.is_empty() {
            return self.query_single_path(raw, segmentation, segments);
        }
        let mut result = Vec::new();
        let mut skipped_prefix_rows = 0usize;
        let prefix_key = series_prefix_key(segmentation, segments);
        let mut partial_segmentation = prefix_key.as_ref();
        for count in (1..=segments.len()).rev() {
            if count < segments.len() {
                partial_segmentation = partial_segmentation
                    .rsplit_once('\'')
                    .map_or("", |(prefix, _)| prefix);
            }
            let partial = &segments[..count];
            if count < segments.len() {
                if let Some(prefixes) = queried_prefixes.as_deref_mut() {
                    if let Some(&count) = prefixes.get(partial_segmentation) {
                        skipped_prefix_rows = skipped_prefix_rows.saturating_add(count);
                        continue;
                    }
                }
                let before = result.len();
                self.append_single_path(&mut result, raw, partial_segmentation, partial);
                self.append_initial_split_rows(&mut result, raw, partial);
                if let Some(prefixes) = queried_prefixes.as_deref_mut() {
                    prefixes.insert(partial_segmentation.to_owned(), result.len() - before);
                }
                continue;
            }
            let mut rows = self.query_single_path(raw, partial_segmentation, partial);
            if self.append_initial_split_rows(&mut rows, raw, partial) {
                rows.sort_by_key(|item| std::cmp::Reverse(item.weight));
            }
            if count == segments.len() {
                // 同长度表里常常只剩冷僻同音词：ping'guo 只有 苹果 1143881、评过 1180、平果 169、平锅 1，而 苹果电脑 21495、苹果公司 19725 在 tbl_4_p，同长度查询看不到，第二位就落到冷僻同音词上。所以更长的补全词按权重并入这一组，权重出自同一语料，跨表可比。`pinyin` 仍是键入的切分（上屏只消耗已键入的音节），`canonical_pinyin` 保留完整读音供持久化。
                // 但首位只留给读音和键入完全一致的同长度行里最重的那一行：补全词上屏会多出用户没打的字，权重再高也不能抢首选（#7345：dacuozi 的首选曾是 打错字了，doumeiren 的首选曾是 都没人管理）。补全词只和其余同长度行竞争第二位起的位置，苹果电脑 仍排在 平锅 前面。同长度查询没有精确键时会退到前缀区间（zhi'liang'jian 只查到 zhi'liang'jiang 的 质量奖），那一行的读音并不是键入的读音，不钉首位，和补全词一起按权重排；同长度查询一行都没有时同样按权重排，整句块随后由 `whole_sentence_insert_position` 插到首位。
                // 精确同音词自己够填首页时，补全词不再按权重挤进来，改成接在整组末尾：首页的座位仍归同音词（qi'shi 的首屏曾是 其实 骑士 其实是 企事业单位），但往后翻还翻得到它们（zhongguo、shide 的精确表本来就够一页，直接丢掉等于让「中国人民」「是的话」从候选里消失，词库以后给 ping'guo 多加几个冷僻同音词，「苹果电脑」也会整个不见）。整组本身至少就有 `first_page_rows` 行，所以接在末尾的补全词一定落在首页之后。一页几行取宿主的每页候选数（`first_page_rows`）；首页上还有整句块、AI 位、云候选位和后插入的用户词，这个门槛看不见它们，但补全词只是被推后，不再消失。
                let exact_rows = rows
                    .iter()
                    .filter(|item| item.canonical_pinyin == partial_segmentation)
                    .count();
                let longer = self.longer_phrase_candidates(partial_segmentation, partial);
                if !longer.is_empty() {
                    let corrected_reading = !folded_reading_equal(partial_segmentation, raw);
                    if !corrected_reading && exact_rows < self.first_page_rows.get() {
                        // `query_database` 和 `segment_row_cache` 都保持词典按权重降序的顺序；这里的页只来自这条路径，不能再被其它来源打乱，所以无需为长词合并重新排序。
                        let lead = usize::from(
                            rows.first()
                                .is_some_and(|item| item.canonical_pinyin == partial_segmentation),
                        );
                        rows.extend(longer);
                        rows[lead..].sort_by_key(|item| std::cmp::Reverse(item.weight));
                    } else {
                        // 纠错读法（如 `xainzai`/`womne`/`shneme`/`gonsi`）的补全续写词严格排在所有等跨度同音词之后，
                        // 彻底杜绝「现在已经/我们可以/什么时候/公司董事会」挤占首页。
                        rows.extend(longer);
                    }
                }
            }
            Self::append_query_rows(&mut result, rows);
        }

        if segments.len() >= 2 && has_only_complete_pinyin_segments(segments) {
            self.merge_sentences_with_typos(
                &mut result,
                raw,
                segmentation,
                segments,
                include_typos,
                runner_ups,
            );
        } else if is_mixed_jianpin_lattice_eligible(segments)
            && !segments
                .windows(2)
                .any(|pair| !crate::pinyin::syllables::is_intact(&pair[0]) && pair[1] == "o")
            && ((has_mixed_complete_and_initial(segments)
                && segments[..segments.len() - 1]
                    .iter()
                    .any(|seg| !crate::pinyin::syllables::is_intact(seg)))
                || (crate::pinyin::jianpin::is_pure_jianpin(segments) && segments.len() >= 4))
        {
            self.merge_mixed_jianpin_sentences(
                &mut result,
                raw,
                segmentation,
                segments,
                runner_ups,
            );
        }

        // 跳过的重复行仍计入原路径的稀疏门槛，不能因此额外触发读音回退。
        if result.len().saturating_add(skipped_prefix_rows) < SPARSE_FALLBACK_THRESHOLD {
            for fallback in sparse_pinyin_fallback_segments(segments) {
                if fallback.is_empty() {
                    continue;
                }
                let fallback_segmentation = join_segments(&fallback);
                let rows = self.query_single_path(raw, &fallback_segmentation, &fallback);
                append_unique_words(&mut result, rows);
            }
        }
        result
    }

    /// 自动切分里的双字母声母另试单字母读法；手动边界不改，完整音节不拆。
    fn append_initial_split_rows(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segments: &[String],
    ) -> bool {
        // 单独的声母前缀仍只查单字，避免把错误拼写开头的 sh 扩成「上好」并遮住纠错关闭的边界。
        if segments.len() < 2 || raw.contains('\'') {
            return false;
        }
        let before = result.len();
        for path in split_initial_token_paths(segments, SYLLABLE_GRAPH_PATH_LIMIT) {
            let code = join_segments(&path);
            let rows = self.query_single_path(raw, &code, &path);
            append_unique_words(result, rows);
        }
        result.len() != before
    }

    /// The lattice sentence block and the typo sentence (QD:644-721, overlays.md §1.6.2). Both rank below a dictionary row that already answers the whole key, which is what `whole_sentence_insert_position` computes; inserting at index 0 once turned 百依百顺 into 白一百顺 and 颁布实施 into 版不是是.
    #[cfg(test)]
    fn merge_sentences(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segmentation: &str,
        segments: &[String],
    ) {
        self.merge_sentences_with_typos(result, raw, segmentation, segments, true, None);
    }

    fn merge_sentences_with_typos(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segmentation: &str,
        segments: &[String],
        include_typos: bool,
        runner_ups: Option<&mut Vec<WordItem>>,
    ) {
        let association = self.sentence_association.clone();
        let has_collocation =
            self.collocation_db.is_some() && association.collocation_weight != 0.0;
        // 〔搭配〕行即解码最优路径：模型在且显示开关开时才出。
        let include_collocation_best = association.collocation_rerank && has_collocation;
        if !association.word_lattice && !include_collocation_best {
            return;
        }
        // Held for the whole merge: the options borrow the model.
        let personal = Arc::clone(&self.personal);
        let model = personal.model();
        let mut options = self.sentence_tables.options(self.sentence_alternatives);
        options.include_lattice_best = association.word_lattice;
        options.include_collocation_best = include_collocation_best;
        options.show_next_on_duplicate = association.show_next_on_duplicate;
        options.personal = Some(&*model);
        options.bigram_memo = Some(&self.bigram_memo);
        if has_collocation {
            options.preceding_context = crate::text::trailing_han_clause(&self.rescoring_context);
        }
        // 搭配项逐边叠加字级搭配分，与二元/三元在同一 Viterbi 里竞争；
        // `GramDb::query()` 与词网格主分同为自然对数域，直接相加不再乘 `LOG10_E`。
        let scorer;
        if has_collocation {
            let db = self.collocation_db.as_ref().expect("collocation db loaded");
            let config = association.collocation_grammar;
            scorer =
                move |tail: &str, word: &str, is_rear: bool| db.query(tail, word, is_rear, &config);
            options.collocation_scorer = Some(&scorer);
            options.collocation_weight = association.collocation_weight;
        }

        let typed = if segmentation.is_empty() {
            raw
        } else {
            segmentation
        };
        let types = self.current_autocorrect_types;
        let typo_eligible = include_typos && types & TYPO_EDGE_TYPES != 0 && !raw.contains('\'');
        let span_limit = options.span_limit;
        let database = &self.database;
        let span_cache = &mut self.typo_span_cache;
        let profile = &*self.typo_profile;
        let mut typo_edges = |literal_best: &SentencePath| {
            collect_probabilistic_typo_edges(
                database,
                span_cache,
                profile,
                segments,
                literal_best,
                types,
            )
        };
        let typo_source: Option<&mut TypoEdgeSource<'_>> = if typo_eligible {
            Some(&mut typo_edges)
        } else {
            None
        };
        // 模型已按需重载，此刻的版本就是这次解码用的版本。
        let personal_version = personal.version();
        let decode_key =
            sentence_decode_key(segments, types, typo_eligible, options.preceding_context);
        let _ = runner_ups;
        let cached = self
            .sentence_decode_cache
            .get_ref(&decode_key)
            .filter(|cached| cached.personal_version == personal_version);
        let emit_two_syl_splits = self.sentence_alternatives || include_collocation_best;
        let mut two_syl_split_pool: Vec<WordItem> = Vec::new();
        let mut two_syl_literal_best: Option<SentencePath> = None;
        let typo = if let Some(cached) = cached {
            if segments.len() == 2 {
                if typo_eligible {
                    two_syl_literal_best = cached.decoded.paths.first().cloned();
                }
                if emit_two_syl_splits {
                    collect_two_syllable_split_pool(
                        &cached.decoded.paths,
                        typed,
                        &mut two_syl_split_pool,
                    );
                }
            }
            merge_decoded_borrowed(result, segments, typed, &options, &cached.decoded)
        } else {
            let span_count = lattice_span_count(segments.len(), options.max_phrase_syllables);
            let decoded = if span_count <= LATTICE_SPAN_CACHE_CAPACITY {
                warm_lattice_span_cache(
                    database,
                    &mut self.lattice_span_cache,
                    segments,
                    span_limit,
                    options.max_phrase_syllables,
                );
                let lattice_spans = &self.lattice_span_cache;
                let mut lookup = |span: &[String]| {
                    cached_lattice_span_ref(lattice_spans, span, span_limit)
                        .expect("warmed lattice span cache entry")
                };
                decode_lattice_borrowed(segments, &mut lookup, &options, typo_source)
            } else {
                let lattice_spans = &mut self.lattice_span_cache;
                let mut lookup = |span: &[String]| {
                    cached_lattice_span(database, lattice_spans, span, span_limit)
                };
                decode_lattice(segments, &mut lookup, &options, typo_source)
            };
            if let Some(decoded) = &decoded {
                if segments.len() == 2 {
                    if typo_eligible {
                        two_syl_literal_best = decoded.paths.first().cloned();
                    }
                    if emit_two_syl_splits {
                        collect_two_syllable_split_pool(
                            &decoded.paths,
                            typed,
                            &mut two_syl_split_pool,
                        );
                    }
                }
                self.sentence_decode_cache.insert(
                    decode_key,
                    CachedDecode {
                        personal_version,
                        decoded: decoded.clone(),
                    },
                );
            }
            decoded.and_then(|decoded| merge_decoded(result, segments, typed, &options, decoded))
        };

        // 双音节输入：确保高频 `1+1` 拆字组合（如 `个事`/`快寄`/`点气`/`才花`）紧随前 8 条词典同音词进入统一融合池，
        // 绝不再被几十条生僻词典行或单字列表挤出 Top-24/32。
        if segments.len() == 2 && !two_syl_split_pool.is_empty() {
            insert_two_syllable_split_pool(result, segments, two_syl_split_pool);
        }

        // 双音节合法音节纠错（`diannai->电脑`、`kuaisi->快递`、`yingai->应该`、`qinkuang->情况`、`penyou->朋友`、`yijin->已经`）：
        // 补齐 2 音节候选与次选纠错竞争池，交由统一概率与 Tier 1 完整上文神经模型共同裁决。
        let mut extra_two_syl_typos: Vec<(WordItem, bool)> = Vec::new();
        if segments.len() == 2 && typo_eligible {
            if let Some(literal_best) = two_syl_literal_best.as_ref() {
                let edges = collect_probabilistic_typo_edges(
                    &self.database,
                    &mut self.typo_span_cache,
                    &self.typo_profile,
                    segments,
                    literal_best,
                    types,
                );
                let exact_dict_leader = has_exact_literal_dict_leader(result, segments);
                let lead_margin = if options.typo_lead_margin.is_finite() {
                    options.typo_lead_margin
                } else {
                    1.0
                };
                let folded = fold_reading(raw);
                for edge in edges {
                    if edge.start != 0 || edge.end != 2 || edge.value == literal_best.sentence {
                        continue;
                    }
                    let mut score = edge_log_prob(edge.weight, 2, &options) - edge.penalty;
                    if let Some(scorer) = options.collocation_scorer {
                        if options.collocation_weight > 0.0 {
                            score += options.collocation_weight
                                * (scorer(options.preceding_context, &edge.value, true)
                                    + options.collocation_edge_offset);
                        }
                    }
                    // 首行已命中严格精确词典整词（如 `假设`、`兄台`、`奶子`）时，Tier 0 同步首帧绝不让纠错词提前篡夺第 0 槽，
                    // 仅将其放入候选池前排（`competitive`）交由 Tier 1 神经模型结合完整语境裁决；无精确字面词时（如 `diannai`）照常救回首位。
                    let leads = !exact_dict_leader && score > literal_best.log_prob + lead_margin;
                    let competitive = score >= literal_best.log_prob - 3.5;
                    if !leads && !competitive {
                        continue;
                    }
                    let mut item = WordItem::new(
                        typed,
                        edge.value.clone(),
                        (score * 1000.0) as i64,
                        CandidateSource::Generated,
                        edge.key,
                    );
                    item.sentence_association = true;
                    item.sentence_words = vec![edge.value];
                    item.corrected_from = folded.as_str().into();
                    extra_two_syl_typos.push((item, leads));
                }
                extra_two_syl_typos.sort_by_key(|lhs| std::cmp::Reverse(lhs.0.weight));
                extra_two_syl_typos.truncate(4);
            }
        }

        if let Some(typo) = typo {
            let existing_idx = result.iter().position(|item| item.word == typo.sentence);
            let can_insert = match existing_idx {
                None => true,
                Some(idx) if is_strict_prefix_uncorrected(&result[idx], raw) => {
                    result.remove(idx);
                    true
                }
                Some(_) => false,
            };
            if can_insert {
                let exact_dict_exists = has_exact_literal_dict_leader(result, segments)
                    || has_any_exact_literal_dict_candidate(result, segments);
                // 3+ 音节合法全拼：若字面路径已含真实多字词（如 `阡陌+之上`、`界定+不准`、`隔日+周一`），
                // Tier 0 要求悬殊余量（`>= 9.5 nats`）才允许篡夺首槽，否则排在字面整句之后进入 Top-16 交由 Tier 1 `Qwen3-0.6B` 裁决；
                // 纯单字零散拼句（如 `mei'gan'xi -> 没+干+系`）要求 `>= 5.5 nats` 余量（真漏键 `没关系` 净胜 `>15 nats` 仍稳居首槽）。
                let effective_margin = if segments.len() >= 3 {
                    if typo.literal_has_multi_char_word {
                        f64::INFINITY
                    } else {
                        options.typo_lead_margin.max(5.5)
                    }
                } else {
                    options.typo_lead_margin
                };
                let leads = !exact_dict_exists && typo.leads(effective_margin);
                let mut at = if leads {
                    0
                } else {
                    whole_sentence_insert_position(result, segments).min(8)
                };
                if !leads {
                    let max_typo_seat = (at + 4).min(12);
                    while at < result.len() && at < max_typo_seat && result[at].sentence_association
                    {
                        at += 1;
                    }
                }
                let mut sentence = WordItem::new(
                    typed,
                    typo.sentence,
                    (typo.score * 1000.0) as i64,
                    CandidateSource::Generated,
                    typo.key,
                );
                sentence.sentence_association = true;
                sentence.sentence_words = typo.words;
                sentence.corrected_from = fold_reading(raw).into();
                result.insert(at, sentence);
            }
        }

        for (item, leads) in extra_two_syl_typos {
            if let Some(existing_idx) = result
                .iter()
                .position(|existing| existing.word == item.word)
            {
                if !is_strict_prefix_uncorrected(&result[existing_idx], raw) {
                    continue;
                }
                result.remove(existing_idx);
            }
            let at = if leads
                && result
                    .first()
                    .is_none_or(|first| first.corrected_from.is_empty())
            {
                0
            } else {
                whole_sentence_insert_position(result, segments).min(8)
            };
            result.insert(at, item);
        }
    }

    /// 简全混拼与长纯简拼的统一词网格整句解码：彻底根治 `wxiangquchifan`/`nzmhaimeilai`/`dengwoyx`/`womeigx`/`mingtianzs`/`wobzd` 截断。
    fn merge_mixed_jianpin_sentences(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segmentation: &str,
        segments: &[String],
        runner_ups: Option<&mut Vec<WordItem>>,
    ) {
        let association = self.sentence_association.clone();
        let has_collocation =
            self.collocation_db.is_some() && association.collocation_weight != 0.0;
        let include_collocation_best = association.collocation_rerank && has_collocation;
        if !association.word_lattice && !include_collocation_best {
            return;
        }
        let personal = Arc::clone(&self.personal);
        let model = personal.model();
        let mut options = self.sentence_tables.options(true);
        options.nbest = options.nbest.max(24);
        options.include_lattice_best = association.word_lattice;
        options.include_collocation_best = include_collocation_best;
        options.show_next_on_duplicate = association.show_next_on_duplicate;
        options.personal = Some(&*model);
        options.bigram_memo = Some(&self.bigram_memo);
        if has_collocation {
            options.preceding_context = crate::text::trailing_han_clause(&self.rescoring_context);
        }
        let scorer;
        if has_collocation {
            let db = self.collocation_db.as_ref().expect("collocation db loaded");
            let config = association.collocation_grammar;
            scorer =
                move |tail: &str, word: &str, is_rear: bool| db.query(tail, word, is_rear, &config);
            options.collocation_scorer = Some(&scorer);
            options.collocation_weight = association.collocation_weight;
        }

        // 主混拼切分与最多 2 条来自 `SyllableDag` / 声母拆分的替代混拼切分共同解码取并集。
        let mut candidate_cuts: Vec<(Vec<String>, f64)> = vec![(segments.to_vec(), 0.0)];
        if !raw.contains('\'') {
            for split in split_initial_token_paths(segments, 2) {
                if !candidate_cuts.iter().any(|(c, _)| c == &split) {
                    candidate_cuts.push((split, -0.30));
                }
            }
            let dag = build_syllable_dag(raw, 0);
            for path in dag_candidate_paths(&dag, 6) {
                if path.syllables.len() >= 2
                    && is_mixed_jianpin_lattice_eligible(&path.syllables)
                    && !candidate_cuts.iter().any(|(c, _)| c == &path.syllables)
                {
                    let extra_initials = path.initial_edges.saturating_sub(
                        segments
                            .iter()
                            .filter(|s| !crate::pinyin::syllables::is_intact(s))
                            .count(),
                    );
                    candidate_cuts.push((
                        path.syllables,
                        (extra_initials as f64) * DAG_INITIAL_EDGE_COST,
                    ));
                    if candidate_cuts.len() >= 3 {
                        break;
                    }
                }
            }
        }

        let span_limit = options.span_limit;
        let database = &self.database;
        let lattice_spans = &mut self.lattice_span_cache;
        let mut all_paths: Vec<(SentencePath, String)> = Vec::new();
        for (cut, channel_bias) in &candidate_cuts {
            let cut_typed = if cut == segments && !segmentation.is_empty() {
                segmentation.to_owned()
            } else {
                join_segments(cut)
            };
            let mut lookup = |span: &[String]| -> Vec<DictRow> {
                if has_only_complete_pinyin_segments(span) {
                    return cached_lattice_span(database, lattice_spans, span, span_limit);
                }
                if span.len() == 1 {
                    if span[0].len() == 1 {
                        return database.query_initial(&span[0], span_limit);
                    }
                    return database.query_lattice_span_with_trailing_initial(span, span_limit);
                }
                let mut rows =
                    database.query_segments_keyed_flat(span, span_limit, QuerySource::Quanpin);
                rows.retain(|row| {
                    row.value.chars().count() == span.len()
                        && matches_mixed_segments(&row.key, span, QuerySource::Quanpin)
                });
                rows
            };
            for mut path in decode_sentences(cut, &mut lookup, &options) {
                // 简全混拼整句至少须包含一个多字词组，杜绝英文拼写（如 `hello -> he'l'lo`）被单字硬凑成「和咯咯」类噪音整句。
                if !path.words.iter().any(|word| word.chars().count() >= 2) {
                    continue;
                }
                path.log_prob += *channel_bias;
                all_paths.push((path, cut_typed.clone()));
            }
        }
        if all_paths.is_empty() {
            return;
        }
        all_paths.sort_by(|lhs, rhs| rhs.0.log_prob.total_cmp(&lhs.0.log_prob));

        // 按词面去重并保留最高分路径；插在完整覆盖当前混拼串的词典整词之后、不完整前缀词之前。
        let insert_at = result
            .iter()
            .take_while(|item| {
                item.source.is_dictionary()
                    && !item.canonical_pinyin.is_empty()
                    && matches_mixed_segments(
                        &item.canonical_pinyin,
                        segments,
                        QuerySource::Quanpin,
                    )
            })
            .count();
        let mut front_block = Vec::new();
        let mut extra_runner_ups = Vec::new();
        let mut seen: HashSet<String> = result.iter().map(|item| item.word.to_string()).collect();
        for (path, cut_typed) in all_paths {
            if !seen.insert(path.sentence.clone()) {
                continue;
            }
            let source = if front_block.is_empty() && include_collocation_best {
                CandidateSource::Collocation
            } else {
                CandidateSource::Generated
            };
            let mut item = WordItem::new(
                cut_typed,
                path.sentence,
                (path.log_prob * 1000.0) as i64,
                source,
                path.key,
            );
            item.sentence_association = true;
            item.sentence_words = path.words;
            if front_block.len() < 8 {
                front_block.push(item);
            } else {
                extra_runner_ups.push(item);
            }
        }
        if !front_block.is_empty() {
            result.splice(insert_at..insert_at, front_block);
        }
        if let Some(runner_ups) = runner_ups {
            runner_ups.extend(extra_runner_ups);
        }
    }

    /// 当纠错主路径与合法多音节字面简拼/混拼并存（如 `shj -> shi` vs `sh'j 手机`、`zenmy -> zen'y` vs `zen'm'y 怎么样`）时，
    /// 按统一自然对数物理信道概率比较双方领跑候选：
    /// - 无字面全跨度匹配（如 `bqng`/`xainzai`/`womne`/`gonsi`）时保持原样零干扰；
    /// - 字面简拼存在真实多字全跨度词时，始终让双方前排共同进入 Top-16 池供 Tier 1 结合上文精准裁决，
    ///   且当字面简拼信道后验不低于纠错读法时将其置前，避免高频简拼词被数百条纠错单字埋没。
    fn merge_literal_jianpin_with_corrected(
        &self,
        result: &mut Vec<WordItem>,
        literal: Vec<WordItem>,
        raw: &str,
        segments: &[String],
        corrected_segments: &[String],
    ) {
        if literal.is_empty() || segments.len() < 2 || !is_mixed_jianpin_lattice_eligible(segments)
        {
            append_unique_words(result, literal);
            return;
        }
        let mut literal_full = Vec::new();
        let mut literal_rest = Vec::new();
        for item in literal {
            // 仅接纳词典真实存在的多音节全跨度简拼/混拼词（如 `shj -> 手机`、`zenmy -> 怎么样`），
            // 不让推测性的纯简拼词网格造句反压真实的单键拼写纠错（如 `sahng -> 上`）。
            let full_match = item.source.is_dictionary()
                && !item.canonical_pinyin.is_empty()
                && matches_mixed_segments(&item.canonical_pinyin, segments, QuerySource::Quanpin);
            if full_match {
                literal_full.push(item);
            } else {
                literal_rest.push(item);
            }
        }
        if literal_full.is_empty() {
            append_unique_words(result, literal_rest);
            return;
        }
        let Some(corr_top) = result.first() else {
            *result = literal_full;
            append_unique_words(result, literal_rest);
            return;
        };
        let opts = self.sentence_tables.options(false);
        let lit_top = &literal_full[0];
        let lit_initials = segments
            .iter()
            .filter(|seg| !crate::pinyin::syllables::is_intact(seg))
            .count();
        let lit_base = edge_log_prob(lit_top.weight, segments.len(), &opts);
        // 每个简拼声母承担真实的 DAG 首字母代价（`-1.15 nats`），高频词自然压过邻键/漏键纠错，低权生僻词则让位纠错。
        let lit_score = lit_base + (lit_initials as f64) * DAG_INITIAL_EDGE_COST;

        let corr_base = if corr_top.sentence_association {
            corr_top.weight as f64 / 1000.0
        } else {
            edge_log_prob(corr_top.weight, corrected_segments.len().max(1), &opts)
        };
        let (corr_penalty, is_transposition) =
            estimated_correction_channel_penalty(raw, corrected_segments);
        let corr_score = corr_base - corr_penalty;

        let mut merged = Vec::with_capacity(result.len() + literal_full.len() + literal_rest.len());
        let old_result = std::mem::take(result);
        // 仅在非相邻换位纠错且字面简拼确为真实常用词（权 >= 5,000）并胜过信道惩罚后的纠错分时，才让字面简拼居首；
        // 其他情况保留纠错读法居首，同时仍把前 8 条字面简拼提至前排（绝不埋到第 500 位之后）供 Tier 1 结合上文裁决。
        let literal_leads = !is_transposition && lit_top.weight >= 5_000 && lit_score >= corr_score;
        if literal_leads {
            // 字面简拼胜出：前 12 条字面全跨度简拼居首，紧随前 6 条纠错主候选同入 Top-16 神经融合池。
            let lit_head_len = literal_full.len().min(12);
            let lit_tail = literal_full.split_off(lit_head_len);
            let corr_head_len = old_result.len().min(6);
            let mut corr_iter = old_result.into_iter();
            merged.extend(literal_full);
            append_unique_words(
                &mut merged,
                corr_iter.by_ref().take(corr_head_len).collect(),
            );
            append_unique_words(&mut merged, lit_tail);
            append_unique_words(&mut merged, corr_iter.collect());
        } else {
            // 纠错读法胜出（如 `sahng -> 上`、`zhng -> 中`）：保留前 6 条纠错居首，紧接前 8 条字面全跨度简拼避免被埋没。
            let corr_head_len = old_result.len().min(6);
            let mut corr_iter = old_result.into_iter();
            let lit_head_len = literal_full.len().min(8);
            let lit_tail = literal_full.split_off(lit_head_len);
            merged.extend(corr_iter.by_ref().take(corr_head_len));
            append_unique_words(&mut merged, literal_full);
            append_unique_words(&mut merged, corr_iter.collect());
            append_unique_words(&mut merged, lit_tail);
        }
        append_unique_words(&mut merged, literal_rest);
        *result = merged;
    }

    /// 为表面看似带简拼尾或两字母非法尾的串（如 `xiexiw -> 谢谢`、`meiwentj -> 没问题`）评估高置信度纠错整词，
    /// 仅在真实概率后验胜过字面混拼时提至前排，不干扰 `zheg -> 这个`、`mingtianzs -> 明天再说` 等真实简全混拼。
    fn merge_competing_incomplete_typo_cuts(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segments: &[String],
        types: u32,
    ) {
        let mut candidate_cuts: Vec<(Vec<String>, f64)> = Vec::new();
        if looks_like_syllable_with_jianpin_tail(raw) {
            for cut in autocorrect_cut_kbest(raw, types, 3) {
                let penalty = if cut.weight <= crate::pinyin::autocorrect::TRANSPOSITION_WEIGHT {
                    crate::pinyin::typos::CHANNEL_TRANSPOSITION_PENALTY
                } else {
                    crate::pinyin::typos::CHANNEL_NEIGHBOR_BASE_PENALTY
                };
                let syls = cut.into_syllables();
                if !candidate_cuts.iter().any(|(existing, _)| existing == &syls) {
                    candidate_cuts.push((syls, penalty));
                }
            }
        }
        if raw.len() >= 4 && types & autocorrect_type::NEIGHBOR != 0 {
            for (cut, penalty) in neighbor_two_letter_cuts(raw, types, 3) {
                let syls = cut.into_syllables();
                if !candidate_cuts.iter().any(|(existing, _)| existing == &syls) {
                    candidate_cuts.push((syls, penalty));
                }
            }
        }
        if candidate_cuts.is_empty() {
            return;
        }

        let association = self.sentence_association.clone();
        let has_collocation =
            self.collocation_db.is_some() && association.collocation_weight != 0.0;
        let preceding = crate::text::trailing_han_clause(&self.rescoring_context);
        let mut options = self.sentence_tables.options(false);
        options.nbest = 6;
        options.bigram_memo = Some(&self.bigram_memo);
        if has_collocation {
            options.preceding_context = preceding;
        }
        let scorer;
        if has_collocation {
            let db = self.collocation_db.as_ref().expect("collocation db loaded");
            let config = association.collocation_grammar;
            scorer =
                move |tail: &str, word: &str, is_rear: bool| db.query(tail, word, is_rear, &config);
            options.collocation_scorer = Some(&scorer);
            options.collocation_weight = association.collocation_weight;
        }

        // 计算当前字面最佳全跨度候选的基线概率分。
        let has_literal_full_dict = result.first().is_some_and(|item| {
            item.source.is_dictionary()
                && !item.canonical_pinyin.is_empty()
                && matches_mixed_segments(&item.canonical_pinyin, segments, QuerySource::Quanpin)
        });
        let literal_score = if has_literal_full_dict {
            let dict_row = &result[0];
            // 完整匹配的精确混拼词典整词（如 `zheg -> 这个`）给予正常 0.0 信道基准，并与搭配分同域比较。
            let mut base = edge_log_prob(dict_row.weight, segments.len(), &options);
            if let Some(col_scorer) = options.collocation_scorer {
                base += options.collocation_weight
                    * (col_scorer(preceding, &dict_row.word, true)
                        + options.collocation_edge_offset);
            }
            base
        } else if let Some(sent_row) = result.first().filter(|item| item.sentence_association) {
            let initials = segments
                .iter()
                .filter(|s| !crate::pinyin::syllables::is_intact(s))
                .count();
            (sent_row.weight as f64 / 1000.0) + (initials as f64) * DAG_INITIAL_EDGE_COST
        } else {
            -30.0
        };

        let admit_floor = if has_literal_full_dict {
            literal_score - 2.5
        } else {
            // 混拼整句往往比两字母邻键纠错切分多 1 个音节并叠加二元/搭配偏移；
            // 放宽竞争准入窗至 `-7.0 nats`，确保全跨度邻键纠错词（如 `meiwentj -> 没问题`）进入前排交 Tier 1 裁决。
            literal_score - 7.0
        };
        let folded = fold_reading(raw);
        let span_limit = options.span_limit;
        let database = &self.database;
        let lattice_spans = &mut self.lattice_span_cache;
        let mut best_rows: Vec<(WordItem, f64)> = Vec::new();
        for (cut, penalty) in candidate_cuts {
            let joined = join_segments(&cut);
            let exact_rows =
                database.query_exact_segmentations_keyed_flat(std::slice::from_ref(&cut), 4);
            for row in exact_rows {
                let mut score = edge_log_prob(row.weight, cut.len(), &options) - penalty;
                if let Some(col_scorer) = options.collocation_scorer {
                    score += options.collocation_weight
                        * (col_scorer(preceding, &row.value, true)
                            + options.collocation_edge_offset);
                }
                let mut item = WordItem::new(
                    joined.clone(),
                    row.value,
                    row.weight,
                    CandidateSource::Database,
                    row.key,
                );
                item.corrected_from = folded.as_str().into();
                if let Some((_, existing_score)) = best_rows
                    .iter_mut()
                    .find(|(existing, _)| existing.word == item.word)
                {
                    if score > *existing_score {
                        *existing_score = score;
                    }
                } else {
                    best_rows.push((item, score));
                }
            }
            // 对 2+ 音节纠错切分同步走词网格解码（如 `mei'wen'ti -> 没+问题`），避免三字词表占位权重 `100` 误杀常见组合词；
            // 若 `exact_rows` 已登记同词面 `Database` 行，沿用其 `Database` 来源并刷新为真实词网格分，避免被 `runner_up_order` 延后。
            if cut.len() >= 2 {
                let mut lookup = |span: &[String]| {
                    cached_lattice_span(database, lattice_spans, span, span_limit)
                };
                for path in decode_sentences(&cut, &mut lookup, &options)
                    .into_iter()
                    .take(3)
                {
                    let score = path.log_prob - penalty;
                    if let Some((_, existing_score)) = best_rows
                        .iter_mut()
                        .find(|(existing, _)| existing.word == path.sentence)
                    {
                        if score > *existing_score {
                            *existing_score = score;
                        }
                        continue;
                    }
                    let mut item = WordItem::new(
                        joined.clone(),
                        path.sentence,
                        (score * 1000.0) as i64,
                        CandidateSource::Generated,
                        path.key,
                    );
                    item.sentence_association = true;
                    item.sentence_words = path.words;
                    item.corrected_from = folded.as_str().into();
                    best_rows.push((item, score));
                }
            }
        }
        best_rows.retain(|(_, score)| *score > admit_floor);
        if best_rows.is_empty() {
            return;
        }
        best_rows.sort_by(|lhs, rhs| rhs.1.total_cmp(&lhs.1));
        best_rows.truncate(4);
        let lead_threshold = if has_literal_full_dict {
            literal_score + 2.5
        } else {
            literal_score + 0.5
        };
        let mut insert_lead = 0usize;
        let mut insert_runner = result.len().min(4);
        for (item, score) in best_rows {
            if score > lead_threshold {
                if let Some(existing_idx) = result
                    .iter()
                    .position(|existing| existing.word == item.word)
                {
                    if existing_idx < insert_lead {
                        continue;
                    }
                    result.remove(existing_idx);
                }
                result.insert(insert_lead, item);
                insert_lead += 1;
                insert_runner = (insert_runner + 1).min(result.len());
            } else {
                if let Some(existing_idx) = result
                    .iter()
                    .position(|existing| existing.word == item.word)
                {
                    if !is_strict_prefix_uncorrected(&result[existing_idx], raw) {
                        continue;
                    }
                    result.remove(existing_idx);
                    let pos = existing_idx.min(insert_runner).min(result.len());
                    result.insert(pos, item);
                    insert_runner = (pos + 1).max(insert_runner).min(result.len());
                } else {
                    let pos = insert_runner.min(result.len());
                    result.insert(pos, item);
                    insert_runner += 1;
                }
            }
        }
    }

    /// 基于 `SyllableDag` 完整音节信道融合跨音节双辅音吞音恢复（`-n+n-`/`-ng+g-`）
    /// 与纠错输入上的备选音节竞争（如 `mohuyign -> mo'hu'yin 模糊音`、`beneng -> ben'neng 本能`）。
    fn merge_syllable_dag_competing_cuts(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segments: &[String],
        resolution: &SeriesResolution,
        alternatives: &[Vec<String>],
        types: u32,
    ) {
        if raw.len() < 4
            || raw.len() > MAX_AUTOCORRECT_INPUT_LENGTH
            || types & autocorrect_type::MISSING_OR_EXTRA == 0
        {
            return;
        }
        let primary_segments: &[String] = if resolution.corrected_input {
            &resolution.corrected
        } else {
            segments
        };
        if primary_segments.len() < 2 {
            return;
        }
        let geminate_candidate =
            types & autocorrect_type::DELETION != 0 && may_have_geminate_boundary(raw);
        let nasal_gn_candidate = resolution.corrected_input
            && types & autocorrect_type::INSERTION != 0
            && may_have_transposed_extra_nasal_g(raw);
        if !geminate_candidate && !nasal_gn_candidate {
            return;
        }
        let dag = build_syllable_dag(raw, types);
        let dag_paths = dag_complete_candidate_paths(&dag, 12);
        if dag_paths.is_empty() {
            return;
        }
        let primary_channel_log_prob = if !resolution.corrected_input {
            0.0
        } else {
            dag_paths
                .iter()
                .find(|path| path.syllables == primary_segments)
                .map_or_else(
                    || -estimated_correction_channel_penalty(raw, primary_segments).0,
                    |path| path.channel_log_prob,
                )
        };
        let best_channel_log_prob = dag_paths
            .first()
            .map_or(primary_channel_log_prob, |path| path.channel_log_prob);

        let span_limit = self.sentence_tables.options(false).span_limit;
        let mut candidate_cuts: Vec<(Vec<String>, f64)> = Vec::new();
        for path in dag_paths {
            if path.typo_edges == 0
                || path.syllables.len() < 2
                || path.syllables == primary_segments
                || alternatives.contains(&path.syllables)
                || resolution.costlier_corrected_cuts.contains(&path.syllables)
            {
                continue;
            }
            if resolution.corrected_input {
                let channel_floor = best_channel_log_prob.max(primary_channel_log_prob) - 1.50;
                if path.geminate_edges == 0 && path.channel_log_prob < channel_floor {
                    continue;
                }
            } else if path.geminate_edges == 0 || path.typo_edges != path.geminate_edges {
                continue;
            }
            if primary_segments.len() > MAX_SYLLABLES_FOR_MULTIPLE_SEGMENTATIONS
                && !has_changed_boundary_lexical_support(
                    &self.database,
                    &mut self.lattice_span_cache,
                    span_limit,
                    primary_segments,
                    &path.syllables,
                )
            {
                continue;
            }
            let relative_channel_delta = path.channel_log_prob - primary_channel_log_prob;
            candidate_cuts.push((path.syllables, relative_channel_delta));
            if candidate_cuts.len() >= 3 {
                break;
            }
        }
        if candidate_cuts.is_empty() {
            return;
        }

        let leading_dict = result
            .iter()
            .take_while(|item| {
                item.source.is_dictionary()
                    && !item.canonical_pinyin.is_empty()
                    && (folded_reading_equal(&item.canonical_pinyin, raw)
                        || item
                            .canonical_pinyin
                            .split('\'')
                            .eq(primary_segments.iter().map(String::as_str))
                        || (resolution.corrected_input
                            && item.word.chars().count() == primary_segments.len()
                            && item.canonical_pinyin.split('\'').count() == primary_segments.len()))
            })
            .count();
        let insert_at = if primary_segments.len() == 2 {
            leading_dict.min(8)
        } else {
            leading_dict
        };

        struct DagPooledCandidate {
            item: WordItem,
            score_milli: i64,
            dict_weight: Option<i64>,
        }

        let mut pooled: Vec<DagPooledCandidate> = Vec::new();
        {
            let association = self.sentence_association.clone();
            let has_collocation =
                self.collocation_db.is_some() && association.collocation_weight != 0.0;
            let preceding = crate::text::trailing_han_clause(&self.rescoring_context);
            let personal = Arc::clone(&self.personal);
            let model = personal.model();
            let mut options = self.sentence_tables.options(self.sentence_alternatives);
            options.nbest = 6;
            options.personal = Some(&*model);
            options.bigram_memo = Some(&self.bigram_memo);
            if has_collocation {
                options.preceding_context = preceding;
            }
            let scorer;
            if has_collocation {
                let db = self.collocation_db.as_ref().expect("collocation db loaded");
                let config = association.collocation_grammar;
                scorer = move |tail: &str, word: &str, is_rear: bool| {
                    db.query(tail, word, is_rear, &config)
                };
                options.collocation_scorer = Some(&scorer);
                options.collocation_weight = association.collocation_weight;
            }

            let folded = fold_reading(raw);
            let database = &self.database;
            let lattice_spans = &mut self.lattice_span_cache;
            for (cut, relative_channel_delta) in &candidate_cuts {
                let joined = join_segments(cut);
                let mut push_candidate =
                    |item: WordItem, score_milli: i64, dict_weight: Option<i64>| {
                        if let Some(existing) = pooled
                            .iter_mut()
                            .find(|existing| existing.item.word == item.word)
                        {
                            let merged_dict_weight = existing.dict_weight.or(dict_weight);
                            if score_milli > existing.score_milli {
                                existing.item = item;
                                existing.score_milli = score_milli;
                            }
                            existing.dict_weight = merged_dict_weight;
                        } else {
                            pooled.push(DagPooledCandidate {
                                item,
                                score_milli,
                                dict_weight,
                            });
                        }
                    };

                let exact_rows =
                    database.query_exact_segmentations_keyed_flat(std::slice::from_ref(cut), 4);
                for row in exact_rows {
                    let mut score =
                        edge_log_prob(row.weight, cut.len(), &options) + *relative_channel_delta;
                    if let Some(col_scorer) = options.collocation_scorer {
                        score += options.collocation_weight
                            * (col_scorer(preceding, &row.value, true)
                                + options.collocation_edge_offset);
                    }
                    let score_milli = (score * 1000.0) as i64;
                    let word = row.value;
                    let mut item = WordItem::new(
                        joined.clone(),
                        word.clone(),
                        score_milli,
                        CandidateSource::Generated,
                        row.key,
                    );
                    item.sentence_association = true;
                    item.sentence_words = vec![word];
                    item.corrected_from = folded.as_str().into();
                    push_candidate(item, score_milli, Some(row.weight));
                }

                let mut lookup = |span: &[String]| {
                    cached_lattice_span(database, lattice_spans, span, span_limit)
                };
                for path in decode_sentences(cut, &mut lookup, &options)
                    .into_iter()
                    .take(4)
                {
                    let score = path.log_prob + *relative_channel_delta;
                    let score_milli = (score * 1000.0) as i64;
                    let mut item = WordItem::new(
                        joined.clone(),
                        path.sentence,
                        score_milli,
                        CandidateSource::Generated,
                        path.key,
                    );
                    item.sentence_association = true;
                    item.sentence_words = path.words;
                    item.corrected_from = folded.as_str().into();
                    push_candidate(item, score_milli, None);
                }
            }
        }

        if pooled.is_empty() {
            return;
        }
        pooled.sort_by_key(|entry| std::cmp::Reverse(entry.score_milli));
        pooled.truncate(8);

        let best_sentence_weight = result[insert_at..]
            .iter()
            .filter(|item| item.sentence_association)
            .map(|item| item.weight)
            .max()
            .unwrap_or(i64::MIN);
        let mut alt_sentences: Vec<WordItem> = Vec::new();
        let mut deferred_two_char_dict: Vec<WordItem> = Vec::new();

        for mut entry in pooled {
            if result[..insert_at]
                .iter()
                .any(|existing| existing.word == entry.item.word)
            {
                continue;
            }
            if let Some(existing_idx) = result[insert_at..]
                .iter()
                .position(|existing| existing.word == entry.item.word)
            {
                let abs_idx = insert_at + existing_idx;
                if result[abs_idx].sentence_association
                    && result[abs_idx].weight >= entry.score_milli
                {
                    continue;
                }
                result.remove(abs_idx);
            }
            // 两音节清洗整词存在前排直拼基准（如 `fanan -> 发难`、`xianan -> 先按`）时，
            // 若吞音实词（如 `犯难`/`险难`）分值未压过首条 `[1,1]` 拆字句，恢复原始词典正权重 `raw_weight`
            // 并以 `Database` 行紧随首读法后入前页（满足下游 Tier 1/叠加层的正词频不变量）；
            // 若如 `kunan -> 困难` 自身压过 `[1,1]` 拆字，则保留为 `sentence_association` 顺位占据 `#3`。
            if primary_segments.len() == 2 && entry.score_milli < best_sentence_weight {
                if let Some(raw_weight) = entry.dict_weight {
                    let mean_nats = (entry.score_milli as f64 / 1000.0) / 2.0;
                    if mean_nats >= -10.0 && deferred_two_char_dict.len() < 2 {
                        entry.item.source = CandidateSource::Database;
                        entry.item.weight = raw_weight;
                        entry.item.sentence_association = false;
                        entry.item.sentence_words.clear();
                        deferred_two_char_dict.push(entry.item);
                    }
                    continue;
                }
            }
            alt_sentences.push(entry.item);
        }

        if !alt_sentences.is_empty() {
            merge_alternative_sentences_at(result, insert_at, alt_sentences);
        }
        if !deferred_two_char_dict.is_empty() {
            let mut pos = insert_at.min(result.len());
            while pos < result.len() && result[pos].sentence_association {
                pos += 1;
            }
            result.splice(pos..pos, deferred_two_char_dict);
        }
    }

    /// 完整音节的长词续接（QD:732-749）：输入切分作为 `pinyin`，完整长键作为规范键。
    fn longer_phrase_candidates(
        &mut self,
        segmentation: &str,
        segments: &[String],
    ) -> Vec<WordItem> {
        if let Some(rows) = self.longer_row_cache.get_ref_by(segmentation) {
            return matched_items(rows.iter().cloned(), segmentation);
        }
        let rows = self.database.query_longer_phrases(
            segments,
            LONGER_PHRASE_EXTRA_SYLLABLES,
            LONGER_PHRASE_LIMIT,
        );
        self.longer_row_cache
            .insert(segmentation.to_owned(), rows.clone());
        matched_items(rows, segmentation)
    }

    /// 单条切分的词典页按原键缓存（QD:751-765），空查询仍为空。
    fn query_single_path(
        &mut self,
        raw: &str,
        segmentation: &str,
        segments: &[String],
    ) -> Vec<WordItem> {
        let cache_key = path_cache_key(raw, segmentation);
        if let Some(cached) = self.cache.get_ref_by(cache_key) {
            return cached.copy_rows(cache_key);
        }
        self.query_single_path_miss(cache_key, segmentation, segments)
    }

    /// 较短热前缀直接克隆到最终结果，避免建立随后立即搬走的中间候选页。
    fn append_single_path(
        &mut self,
        result: &mut Vec<WordItem>,
        raw: &str,
        segmentation: &str,
        segments: &[String],
    ) {
        if result.is_empty() && result.capacity() == 0 {
            *result = self.query_single_path(raw, segmentation, segments);
            return;
        }
        let cache_key = path_cache_key(raw, segmentation);
        if let Some(rows) = self.cache.get_ref_by(cache_key) {
            rows.append_to(cache_key, result);
            return;
        }
        let rows = self.query_single_path_miss(cache_key, segmentation, segments);
        Self::append_query_rows(result, rows);
    }

    /// 冷页仍保存紧凑副本并交出原查询结果；两个入口共用同一次未命中处理。
    fn query_single_path_miss(
        &mut self,
        cache_key: &str,
        segmentation: &str,
        segments: &[String],
    ) -> Vec<WordItem> {
        let result = self.query_database(segments, segmentation);
        self.cache.insert(
            cache_key.to_owned(),
            PathPage::from_dictionary(cache_key, &result),
        );
        result
    }

    /// QD:830-869：单字母查首字母表，最多 24 行；其它输入走原级联查询。
    fn query_database(&mut self, segments: &[String], segmentation: &str) -> Vec<WordItem> {
        if segments.len() == 1 && segments[0].len() == 1 {
            let matched_code = if segmentation.is_empty() {
                segments[0].as_str()
            } else {
                segmentation
            };
            // 单字节 UTF-8 音节必为 ASCII；局部键只供查找，前缀与普通切分键分开。
            let key_bytes = [1, segments[0].as_bytes()[0]];
            let key = std::str::from_utf8(&key_bytes).expect("单字节音节与缓存前缀必须是 ASCII");
            if let Some(rows) = self.segment_row_cache.get_ref_by(key) {
                return matched_items(rows.iter().cloned(), matched_code);
            }
            let rows = self
                .database
                .query_initial(&segments[0], INITIAL_CANDIDATE_LIMIT);
            self.segment_row_cache
                .insert(format!("\u{1}{}", segments[0]), rows.clone());
            return matched_items(rows, matched_code);
        }
        // 音节已在 `resolve_segments` 规范化一次，直接使用标准键查找。
        let code = segmentation.is_empty().then(|| join_segments(segments));
        let key = code.as_deref().unwrap_or(segmentation);
        if let Some(rows) = self.segment_row_cache.get_ref_by(key) {
            return matched_items(rows.iter().cloned(), key);
        }
        let rows =
            self.database
                .query_segments_keyed_flat(segments, UNLIMITED_ROWS, QuerySource::Quanpin);
        self.segment_row_cache.insert(key.to_owned(), rows.clone());
        matched_items(rows, key)
    }

    /// 显式和自动切分共用一次别名规范化，进入查询管线的音节保持标准拼写（QD:767-775）。
    fn resolve_segments(&mut self, raw: &str, segmentation: &str) -> Vec<String> {
        resolve_cached_segments(&mut self.segmentation_cache, raw, segmentation)
    }

    /// 拥有型解析入口只保留作旧查询的测试对照；产品按已核对的索引延迟取得副本。
    #[cfg(test)]
    fn resolution(
        &mut self,
        raw: &str,
        segmentation: &str,
        segments: &[String],
        types: u32,
    ) -> SeriesResolution {
        project_cached_resolution(
            &mut self.resolution_cache,
            raw,
            segmentation,
            segments,
            types,
            |resolution| resolution.into_owned(),
        )
        .1
    }

    /// 热投影不构造音节；冷未命中保留原音节备忘和解析副本，并交出计算结果。
    fn project_resolution<T>(
        &mut self,
        raw: &str,
        segmentation: &str,
        types: u32,
        project: impl FnOnce(Cow<'_, SeriesResolution>) -> T,
    ) -> T {
        let segmentation_cache = &mut self.segmentation_cache;
        project_cached_resolution_with(
            &mut self.resolution_cache,
            raw,
            segmentation,
            types,
            || {
                let segments = resolve_cached_segments(segmentation_cache, raw, segmentation);
                resolve_series_query(raw, &segments, types)
            },
            project,
        )
        .1
    }

    /// 插入尚不存在的词条并记入用户日志（QD:1200-1212, QD:1234-1266）；词条已存在时直接视作成功且不重复写日志。
    fn insert_new_word(&mut self, key: &str, word: &str) -> Result<()> {
        if !self.database.is_open() {
            return Err(EngineError::failed(DICTIONARY_UNAVAILABLE));
        }
        let Some(weight) = self.database.weight_for_new_word(key, word) else {
            return Ok(());
        };
        self.database.insert_word_with_weight(key, word, weight)?;
        // 参考实现忽略日志写入失败。这里词条已落入工作词库，先清缓存；仍向调用方返回日志错误，避免丢日志导致下一代词库丢失该词。
        let journaled = record_user_insert(
            &self.paths.user(USER_JOURNAL),
            PersonalDictionaryKind::Pinyin,
            key,
            word,
            weight,
            "",
        );
        self.reset_cache();
        journaled
    }

    /// QD:1409-1432.
    fn reset_cache_if_database_changed(&mut self) {
        if self.database.database_changed() {
            self.reset_cache();
        }
    }

    /// QD:283-293.
    fn drop_personal_scored_results(&mut self) {
        // Reading the model reloads it when another store or process changed the tables.
        drop(self.personal.model());
        let version = self.personal.version();
        if version == self.personal_version {
            return;
        }
        self.personal_version = version;
        for key in self.personal_scored_keys.drain() {
            self.series_cache.remove(&key);
        }
    }

    /// QD:271-281.
    fn remember_personal_scored_key(&mut self, key: String) {
        self.personal_scored_keys.insert(key);
        // The series cache evicts on its own, so keys it no longer holds are dropped here; otherwise the set grows for as long as the model's version stays put.
        if self.personal_scored_keys.len() > PERSONAL_SCORED_KEY_LIMIT {
            let series_cache = &self.series_cache;
            self.personal_scored_keys
                .retain(|key| series_cache.contains(key));
        }
    }
}

/// 按缓存字段拆分借用，使调用方能同时读取系列槽配置；完整键核对和冷副本规则集中保留。
#[cfg(test)]
fn project_cached_resolution<T>(
    cache: &mut FifoCache<u64, CachedResolution>,
    raw: &str,
    segmentation: &str,
    segments: &[String],
    types: u32,
    project: impl FnOnce(Cow<'_, SeriesResolution>) -> T,
) -> (u64, T) {
    project_cached_resolution_with(
        cache,
        raw,
        segmentation,
        types,
        || resolve_series_query(raw, segments, types),
        project,
    )
}

/// 解析热命中不调用构造闭包；常规查询因此可把音节拥有值延迟到真正未命中时。
fn project_cached_resolution_with<T>(
    cache: &mut FifoCache<u64, CachedResolution>,
    raw: &str,
    segmentation: &str,
    types: u32,
    resolve: impl FnOnce() -> SeriesResolution,
    project: impl FnOnce(Cow<'_, SeriesResolution>) -> T,
) -> (u64, T) {
    let hash = resolution_cache_hash(types, raw, segmentation);
    if let Some(cached) = cache.get_ref(&hash) {
        if resolution_cache_key_matches(&cached.key, types, raw, segmentation) {
            return (hash, project(Cow::Borrowed(&cached.resolution)));
        }
    }
    let resolution = resolve();
    let key = format!("{types}\u{1f}{raw}\u{1f}{segmentation}");
    cache.insert(
        hash,
        CachedResolution {
            key,
            resolution: resolution.clone(),
        },
    );
    (hash, project(Cow::Owned(resolution)))
}

/// 直接构造拥有型候选；缓存行可用惰性克隆迭代，避免完整的中间行页。
fn matched_items(rows: impl IntoIterator<Item = DictRow>, matched_code: &str) -> Vec<WordItem> {
    rows.into_iter()
        .map(|row| {
            WordItem::new(
                matched_code,
                row.value,
                row.weight,
                CandidateSource::Database,
                row.key,
            )
        })
        .collect()
}

/// `tbl_1_<c>` 的行转成 `WordItem(key, value, weight, Database, key)`。
#[cfg(test)]
fn initial_items(rows: Vec<DictRow>) -> Vec<WordItem> {
    rows.into_iter()
        .map(|row| {
            WordItem::new(
                row.key.clone(),
                row.value,
                row.weight,
                CandidateSource::Database,
                row.key,
            )
        })
        .collect()
}

/// `query_lattice_span` 经过跨度缓存；哈希碰撞时重新查库，不混用不同跨度或上限的结果。
/// 跨度来自已切分的音节，每项不含撇号，因此分隔符编码能核对原样音节边界。
fn cached_lattice_span(
    database: &PinyinDatabase,
    cache: &mut FifoCache<u64, CachedLatticeSpan>,
    span: &[String],
    span_limit: usize,
) -> Vec<DictRow> {
    let hash = lattice_span_cache_hash(span_limit, span);
    if let Some(cached) = cache.get_ref(&hash) {
        if lattice_span_cache_key_matches(cached, span_limit, span) {
            return cached.rows.clone();
        }
    }
    let rows = database.query_lattice_span(span, span_limit);
    let mut key = String::with_capacity(span.iter().map(|syllable| syllable.len() + 1).sum());
    for syllable in span {
        key.push('\'');
        key.push_str(syllable);
    }
    cache.insert(
        hash,
        CachedLatticeSpan {
            span_limit,
            span: key,
            rows: rows.clone(),
        },
    );
    rows
}

fn ensure_cached_lattice_span(
    database: &PinyinDatabase,
    cache: &mut FifoCache<u64, CachedLatticeSpan>,
    span: &[String],
    span_limit: usize,
) {
    let hash = lattice_span_cache_hash(span_limit, span);
    if let Some(cached) = cache.get_ref(&hash) {
        if lattice_span_cache_key_matches(cached, span_limit, span) {
            // 预热批次可能命中上一轮缓存；把命中项移到队尾，避免后续冷跨度插入时把本轮早先命中的项驱逐，导致借用查询找不到刚预热的行。
            let cached = cache
                .remove(&hash)
                .expect("lattice span cache hit disappeared");
            cache.insert(hash, cached);
            return;
        }
    }
    let rows = database.query_lattice_span(span, span_limit);
    let mut key = String::with_capacity(span.iter().map(|syllable| syllable.len() + 1).sum());
    for syllable in span {
        key.push('\'');
        key.push_str(syllable);
    }
    cache.insert(
        hash,
        CachedLatticeSpan {
            span_limit,
            span: key,
            rows,
        },
    );
}

fn cached_lattice_span_ref<'a>(
    cache: &'a FifoCache<u64, CachedLatticeSpan>,
    span: &[String],
    span_limit: usize,
) -> Option<&'a [DictRow]> {
    let hash = lattice_span_cache_hash(span_limit, span);
    let cached = cache.get_ref(&hash)?;
    lattice_span_cache_key_matches(cached, span_limit, span).then_some(cached.rows.as_slice())
}

fn warm_lattice_span_cache(
    database: &PinyinDatabase,
    cache: &mut FifoCache<u64, CachedLatticeSpan>,
    syllables: &[String],
    span_limit: usize,
    max_phrase_syllables: usize,
) {
    let max_len = max_phrase_syllables.max(1);
    for start in 0..syllables.len() {
        for end in start + 1..=syllables.len().min(start + max_len) {
            ensure_cached_lattice_span(database, cache, &syllables[start..end], span_limit);
        }
    }
}

/// `sentence_decode_cache` 的键：音节串、这次查询的纠错类型、是否规划纠错整句，以及非空连续汉字前文。纠错整句与跨边界上文仅在各自启用时进入键值。
fn sentence_decode_key(
    segments: &[String],
    types: u32,
    typo_eligible: bool,
    preceding_context: &str,
) -> String {
    let extra_context = if preceding_context.is_empty() {
        0
    } else {
        preceding_context.len() + 1
    };
    let mut key = String::with_capacity(
        segments
            .iter()
            .map(|segment| segment.len() + 1)
            .sum::<usize>()
            + 12
            + extra_context,
    );
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            key.push('\'');
        }
        key.push_str(segment);
    }
    key.push('\u{1f}');
    key.push_str(&types.to_string());
    key.push(if typo_eligible { 't' } else { '-' });
    if !preceding_context.is_empty() {
        key.push('\u{1f}');
        key.push_str(preceding_context);
    }
    key
}

fn lattice_span_count(syllable_count: usize, max_phrase_syllables: usize) -> usize {
    let max_len = max_phrase_syllables.max(1);
    (0..syllable_count)
        .map(|start| syllable_count.min(start.saturating_add(max_len)) - start)
        .sum()
}

fn lattice_span_cache_hash(span_limit: usize, span: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    span_limit.hash(&mut hasher);
    span.hash(&mut hasher);
    hasher.finish()
}

fn lattice_span_cache_key_matches(
    cached: &CachedLatticeSpan,
    span_limit: usize,
    span: &[String],
) -> bool {
    if cached.span_limit != span_limit {
        return false;
    }
    let mut remaining = cached.span.as_str();
    for syllable in span {
        let Some(rest) = remaining.strip_prefix('\'') else {
            return false;
        };
        let Some(rest) = rest.strip_prefix(syllable) else {
            return false;
        };
        remaining = rest;
    }
    remaining.is_empty()
}

// 只有参数字节恰好等于音节拼接结果才借用，保留空段、别名及嵌入撇号的旧键。
fn series_prefix_key<'a>(segmentation: &'a str, segments: &[String]) -> Cow<'a, str> {
    let mut remaining = segmentation;
    for (index, segment) in segments.iter().enumerate() {
        if index != 0 {
            let Some(rest) = remaining.strip_prefix('\'') else {
                return Cow::Owned(join_segments(segments));
            };
            remaining = rest;
        }
        let Some(rest) = remaining.strip_prefix(segment.as_str()) else {
            return Cow::Owned(join_segments(segments));
        };
        remaining = rest;
    }
    if remaining.is_empty() {
        Cow::Borrowed(segmentation)
    } else {
        Cow::Owned(join_segments(segments))
    }
}

#[cfg(test)]
fn truncate_last_segment(segmentation: &mut String) {
    if let Some(boundary) = segmentation.rfind('\'') {
        segmentation.truncate(boundary);
    } else {
        segmentation.clear();
    }
}

fn series_slot_key(
    cache_key: &str,
    options: &SentenceAssociationOptions,
    has_rerankers: bool,
    rescoring_context: &str,
) -> String {
    let context_suffix = if has_rerankers {
        rescoring_context.len() + 1
    } else {
        0
    };
    let mut slot = String::with_capacity(cache_key.len() + 5 + context_suffix);
    slot.push_str(cache_key);
    slot.push('\u{1f}');
    slot.push('S');
    slot.push(if options.word_lattice { '1' } else { '0' });
    slot.push(if options.neural_keyboard { '1' } else { '0' });
    slot.push(if options.show_next_on_duplicate {
        '1'
    } else {
        '0'
    });
    if has_rerankers {
        slot.push('\u{1f}');
        slot.push_str(rescoring_context);
    }
    slot
}

fn fuzzy_segmentation<'a>(segmentation: &'a str, normalized_segments: &[String]) -> Cow<'a, str> {
    if segmentation.is_empty() {
        Cow::Owned(join_segments(normalized_segments))
    } else {
        Cow::Borrowed(segmentation)
    }
}

fn fuzzy_cache_hash(rules: u32, segmentation: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    rules.hash(&mut hasher);
    segmentation.hash(&mut hasher);
    hasher.finish()
}

fn fuzzy_cache_key_matches(cached: &CachedFuzzyCandidates, rules: u32, segmentation: &str) -> bool {
    cached.rules == rules && cached.segmentation == segmentation
}

/// 热页直接投影，冷页保留原缓存副本与 SQLite 页容量策略。
fn append_cached_fuzzy_rows<'a>(
    database: &PinyinDatabase,
    cache: &mut FifoCache<u64, CachedFuzzyRows>,
    paths: &[Vec<String>],
    rules: u32,
    limit: usize,
    typed: impl Into<Cow<'a, str>>,
    result: &mut Vec<WordItem>,
) {
    let typed = typed.into();
    let hash = fuzzy_row_cache_hash(rules, limit, &typed);
    if let Some(cached) = cache.get_ref(&hash) {
        if fuzzy_row_cache_key_matches(cached, rules, limit, &typed) {
            append_fuzzy_items(cached.rows.iter().cloned(), typed, result);
            return;
        }
    }
    let rows = database.query_exact_segmentations_keyed_flat(paths, FUZZY_ROW_LIMIT);
    cache.insert(
        hash,
        CachedFuzzyRows {
            rules,
            limit,
            typed: typed.as_ref().to_owned(),
            rows: rows.clone(),
        },
    );
    append_fuzzy_items(rows.into_iter(), typed, result);
}

/// 取得首行才拥有匹配码，已有拥有值直接接管；其余行复制同一读法。
fn append_fuzzy_items(
    mut rows: impl ExactSizeIterator<Item = DictRow>,
    typed: Cow<'_, str>,
    result: &mut Vec<WordItem>,
) {
    result.reserve(rows.len());
    if let Some(row) = rows.next() {
        let mut item = WordItem::new(
            typed.into_owned(),
            row.value,
            row.weight,
            CandidateSource::Database,
            row.key,
        );
        item.fuzzy = true;
        result.push(item);
    }
    for row in rows {
        let mut item = WordItem::new(
            result.last().expect("首个模糊候选已插入").pinyin.clone(),
            row.value,
            row.weight,
            CandidateSource::Database,
            row.key,
        );
        item.fuzzy = true;
        result.push(item);
    }
}

fn fuzzy_row_cache_hash(rules: u32, limit: usize, typed: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    rules.hash(&mut hasher);
    limit.hash(&mut hasher);
    typed.hash(&mut hasher);
    hasher.finish()
}

fn fuzzy_row_cache_key_matches(
    cached: &CachedFuzzyRows,
    rules: u32,
    limit: usize,
    typed: &str,
) -> bool {
    cached.rules == rules && cached.limit == limit && cached.typed == typed
}

fn path_cache_key<'a>(raw: &'a str, segmentation: &'a str) -> &'a str {
    if segmentation.is_empty() {
        raw
    } else {
        segmentation
    }
}

/// 自动切分的冷副本先写入备忘，别名规范化只修改返回值，保留原有缓存容量与内容。
fn resolve_cached_segments(
    cache: &mut FifoCache<String, Vec<String>>,
    raw: &str,
    segmentation: &str,
) -> Vec<String> {
    let mut segments = if segmentation.is_empty() {
        if let Some(cached) = lookup_cached_segments(cache, raw) {
            cached
        } else {
            let segments = first_correction_cut(raw).unwrap_or_default();
            cache.insert(raw.to_owned(), segments.clone());
            segments
        }
    } else {
        split_segments(segmentation)
    };
    normalize_umlaut_aliases(&mut segments);
    segments
}

fn lookup_cached_segments(
    cache: &FifoCache<String, Vec<String>>,
    key: &str,
) -> Option<Vec<String>> {
    cache.get_ref_by(key).cloned()
}

fn resolution_cache_hash(types: u32, raw: &str, segmentation: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    types.hash(&mut hasher);
    raw.hash(&mut hasher);
    segmentation.hash(&mut hasher);
    hasher.finish()
}

fn resolution_cache_key_matches(key: &str, types: u32, raw: &str, segmentation: &str) -> bool {
    let mut parts = key.split('\u{1f}');
    parts.next().and_then(|part| part.parse().ok()) == Some(types)
        && parts.next() == Some(raw)
        && parts.next() == Some(segmentation)
        && parts.next().is_none()
}

/// The readings that compete with the primary one (QD:394-443): with a mask, the same-cost corrections and then every correction-mode cut; for a short all-complete input, every complete segmentation of the letters. Deduplicated against the primary and the costlier cuts, at most 32.
fn alternative_segmentations(
    raw: &str,
    segments: &[String],
    resolution: &SeriesResolution,
    types: u32,
) -> Vec<Vec<String>> {
    let mut alternatives = Vec::new();
    // Seeding the costlier cuts keeps them out of the frequency-competing tier; they are appended after it.
    let mut seen: [Option<String>; ALTERNATIVE_SEGMENTATION_SEEN_CAPACITY] =
        std::array::from_fn(|_| None);
    let mut seen_length = 0;
    let primary_segmentation = resolution.segmentation.as_str();
    for cut in &resolution.costlier_corrected_cuts {
        remember_segmentation_key(
            &mut seen,
            &mut seen_length,
            primary_segmentation,
            join_segments(cut),
        );
    }
    let mut append = |candidate: Vec<String>| {
        let key = join_segments(&candidate);
        if !key.is_empty()
            && alternatives.len() < SYLLABLE_GRAPH_PATH_LIMIT
            && remember_segmentation_key(&mut seen, &mut seen_length, primary_segmentation, key)
        {
            if alternatives.is_empty() {
                alternatives.reserve(SYLLABLE_GRAPH_PATH_LIMIT);
            }
            alternatives.push(candidate);
        }
    };
    // Correction-mode alternatives follow the autocorrect switch; offering them unconditionally made a misspelled "sahng" produce the corrected word with autocorrection turned off. Correction alternatives go first: they explain the letters actually typed, which the alias readings after them do not.
    if types != 0 {
        for cut in &resolution.alternative_corrected_cuts {
            append(cut.to_vec());
        }
        for cut in cut_pinyin_by_mode(raw, CutMode::Correction) {
            append(cut);
        }
    }
    if !raw.contains('\'') && has_only_complete_pinyin_segments(segments) {
        let graph = build_syllable_graph(raw);
        if segments.len() <= MAX_SYLLABLES_FOR_MULTIPLE_SEGMENTATIONS {
            for path in enumerate_complete_segmentations(&graph, SYLLABLE_GRAPH_PATH_LIMIT) {
                append(path);
            }
        } else {
            // 长句（>4 音节）保留同最小段数备选（extra=0）与单处零声母拆分割音（extra=1），
            // 随后在 `merge_alternatives_with_runner_ups` 中按变化边界词典实证剪枝为 Top-4，杜绝无意义伪隔音导致 17x 重复解码。
            for path in enumerate_kbest_complete_segmentations(&graph, 1, 16) {
                append(path.syllables);
            }
        }
    }
    alternatives
}

fn remember_segmentation_key(
    seen: &mut [Option<String>; ALTERNATIVE_SEGMENTATION_SEEN_CAPACITY],
    seen_length: &mut usize,
    primary: &str,
    key: String,
) -> bool {
    if key == primary
        || *seen_length == ALTERNATIVE_SEGMENTATION_SEEN_CAPACITY
        || seen[..*seen_length]
            .iter()
            .flatten()
            .any(|existing| existing == &key)
    {
        return false;
    }
    seen[*seen_length] = Some(key);
    *seen_length += 1;
    true
}

/// 仅接受没有手动边界、没有替换字母的完整切分。
fn is_literal_segmentation(raw: &str, segments: &[String]) -> bool {
    !raw.contains('\'')
        && has_only_complete_pinyin_segments(segments)
        && segments
            .iter()
            .flat_map(|segment| segment.bytes())
            .eq(raw.bytes())
}

/// Letters of a row's matched code, what the fuzzy merge sorts by.
fn matched_letters(pinyin: &str) -> usize {
    pinyin.bytes().filter(|&byte| byte != b'\'').count()
}

/// 整句只按原拼音解码，`whole_sentence_insert_position` 把它放在原拼音覆盖整个输入的词库行之后；它排到首位，通常是因为原拼音没有这样的词，这时它只是替补。开了模糊音，按模糊读法覆盖整个输入的词库行就是这个输入的整词，替补要让给它：`zongguo` 开 z=zh 时「中国」排到整句「总国」前面。同一档里只要还有原拼音覆盖整个输入的精确词库行（整词、另一种切分的整词，哪怕排在更长的补全词后面），就一行也不动，精确仍先于模糊；补全出的更长词条不算覆盖。挪动只在覆盖整个输入的那一档里做，模糊行之间、其余行之间的先后都不变。不开模糊音时不经过这里。
fn yield_stand_in_sentence_to_fuzzy_rows(result: &mut [WordItem]) {
    let Some(first) = result.first() else {
        return;
    };
    if !first.sentence_association {
        return;
    }
    let letters = matched_letters(&first.pinyin);
    let bucket = result
        .iter()
        .take_while(|item| matched_letters(&item.pinyin) == letters)
        .count();
    let typed_reading_answers = result[..bucket].iter().any(|item| {
        !item.fuzzy
            && item.source.is_dictionary()
            && matched_letters(&item.canonical_pinyin) == letters
    });
    if typed_reading_answers {
        return;
    }
    // 稳定排序：模糊行整体挪到前面，各自内部的先后不变。
    result[..bucket].sort_by_key(|item| !item.fuzzy);
}

/// 零堆分配前哨检查：跨音节 `-n + n-` / `-ng + g-` 吞音恢复仅可能发生在串内出现 `n + 元音` 或 `ng + 元音` 处。
#[inline]
fn may_have_geminate_boundary(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 4
        && (1..bytes.len().saturating_sub(1)).any(|idx| {
            bytes[idx] == b'n'
                && (matches!(bytes[idx + 1], b'a' | b'e' | b'i' | b'o' | b'u' | b'v')
                    || (bytes[idx + 1] == b'g'
                        && idx + 2 < bytes.len()
                        && matches!(bytes[idx + 2], b'a' | b'e' | b'o' | b'u')))
        })
}

/// 零堆分配前哨检查：鼻音尾换位多键 `*gn -> *n`（如 `mohuyign`）仅可能出现在含 `gn` 子串的输入中。
#[inline]
fn may_have_transposed_extra_nasal_g(raw: &str) -> bool {
    raw.as_bytes().windows(2).any(|pair| pair == b"gn")
}

/// 为长句（>4 音节）中发生吞音或音节纠错改写的路径验证变化区间是否命中真实词库多字词组，避免无意义全句解码。
fn has_changed_boundary_lexical_support(
    database: &PinyinDatabase,
    lattice_spans: &mut FifoCache<u64, CachedLatticeSpan>,
    probe_limit: usize,
    primary: &[String],
    alt: &[String],
) -> bool {
    let mut prefix = 0usize;
    let min_len = primary.len().min(alt.len());
    while prefix < min_len && primary[prefix] == alt[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < min_len
        && primary[primary.len() - 1 - suffix] == alt[alt.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let diff_start = prefix;
    let diff_end = alt.len().saturating_sub(suffix);
    if diff_start >= diff_end {
        return false;
    }
    for span_len in 2..=4usize.min(alt.len()) {
        let min_s = diff_start.saturating_sub(span_len - 1);
        let max_s = diff_end.saturating_sub(1).min(alt.len() - span_len);
        for s in min_s..=max_s {
            let e = s + span_len;
            if e <= diff_start || s >= diff_end {
                continue;
            }
            let span = &alt[s..e];
            ensure_cached_lattice_span(database, lattice_spans, span, probe_limit);
            if cached_lattice_span_ref(lattice_spans, span, probe_limit)
                .is_some_and(|rows| !rows.is_empty())
            {
                return true;
            }
        }
    }
    false
}

/// 为长句（>4 音节）的字面备选切分计算变化边界处的词典多字词实证权重。
/// 仅当新切出的核心跨度（如 `xi'an->西安`、`ti'an->提案`、`ji'e->饥饿`、`fang'an->方案`）或跨边界邻接词组
/// （如 `he'li'+'an'pai`、`xie'shang`）在词库中真实存在时返回 `Some(weight)`，否则直接剪枝避免伪隔音（`xi'a`/`ya'o`/`gu'o`）反复解码。
fn long_sentence_alternative_lexical_score(
    database: &PinyinDatabase,
    lattice_spans: &mut FifoCache<u64, CachedLatticeSpan>,
    probe_limit: usize,
    primary: &[String],
    alt: &[String],
) -> Option<i64> {
    let mut prefix = 0usize;
    let min_len = primary.len().min(alt.len());
    while prefix < min_len && primary[prefix] == alt[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < min_len
        && primary[primary.len() - 1 - suffix] == alt[alt.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let diff_start = prefix;
    let diff_end = alt.len().saturating_sub(suffix);
    if diff_start >= diff_end {
        return None;
    }
    let mut span_weight = |start: usize, end: usize| -> i64 {
        if end <= start + 1 || end > alt.len() || end - start > 4 {
            return 0;
        }
        let span = &alt[start..end];
        ensure_cached_lattice_span(database, lattice_spans, span, probe_limit);
        cached_lattice_span_ref(lattice_spans, span, probe_limit)
            .and_then(|rows| rows.first())
            .map_or(0, |row| row.weight)
    };
    let core_len = diff_end - diff_start;
    let core_weight = if (2..=4).contains(&core_len) {
        span_weight(diff_start, diff_end)
    } else {
        0
    };
    let left_weight = if diff_start > 0 && diff_start < alt.len() {
        span_weight(diff_start - 1, diff_start + 1)
    } else {
        0
    };
    let right_weight = if diff_end < alt.len() && diff_end >= 1 {
        span_weight(diff_end - 1, diff_end + 1)
    } else {
        0
    };
    let extra_splits = alt.len().saturating_sub(primary.len());
    if core_weight > 0 {
        return Some(core_weight.max(left_weight.min(right_weight)));
    }
    if extra_splits == 0 && (left_weight > 0 || right_weight > 0) {
        return Some(left_weight.max(right_weight));
    }
    if extra_splits > 0 && left_weight > 0 && right_weight > 0 {
        return Some(left_weight.min(right_weight));
    }
    None
}

/// 按物理键盘编辑形态估算整串纠错切分至少承担的信道代价（nats）及是否为相邻换位，用于与字面多音节简拼的统一后验竞争。
fn estimated_correction_channel_penalty(raw: &str, corrected_segments: &[String]) -> (f64, bool) {
    let raw_bytes: Vec<u8> = raw
        .bytes()
        .filter(|&b| b != b'\'')
        .map(|b| b.to_ascii_lowercase())
        .collect();
    let corr_bytes: Vec<u8> = corrected_segments
        .iter()
        .flat_map(|seg| seg.bytes())
        .map(|b| b.to_ascii_lowercase())
        .collect();
    if raw_bytes.len() != corr_bytes.len() {
        return (
            crate::pinyin::typos::CHANNEL_MISSING_OR_EXTRA_PENALTY,
            false,
        );
    }
    let diffs: Vec<usize> = raw_bytes
        .iter()
        .zip(&corr_bytes)
        .enumerate()
        .filter_map(|(idx, (a, b))| (a != b).then_some(idx))
        .collect();
    if diffs.len() == 2
        && diffs[1] == diffs[0] + 1
        && raw_bytes[diffs[0]] == corr_bytes[diffs[1]]
        && raw_bytes[diffs[1]] == corr_bytes[diffs[0]]
    {
        return (crate::pinyin::typos::CHANNEL_TRANSPOSITION_PENALTY, true);
    }
    (crate::pinyin::typos::CHANNEL_NEIGHBOR_BASE_PENALTY, false)
}

/// 判断已有候选行是否为未纠错的严格前缀匹配（消费字母数小于完整原始输入 `raw`）。
/// 全跨度纠错行（如 `meiwentj -> mei'wen'ti`）可原地升级此类短前缀行（如 `mei'wen't`），避免阻断全跨度竞争。
fn is_strict_prefix_uncorrected(existing: &WordItem, raw: &str) -> bool {
    existing.corrected_from.is_empty() && matched_letters(&existing.pinyin) < matched_letters(raw)
}

/// 判断候选列表首行是否为覆盖当前全部字面音节的精确词典整词。
fn has_exact_literal_dict_leader(result: &[WordItem], segments: &[String]) -> bool {
    result
        .iter()
        .find(|item| !(item.sentence_association && !item.source.is_dictionary()))
        .is_some_and(|item| {
            item.source.is_dictionary()
                && item.corrected_from.is_empty()
                && !item.canonical_pinyin.is_empty()
                && item
                    .canonical_pinyin
                    .split('\'')
                    .eq(segments.iter().map(String::as_str))
        })
}

/// 判断候选池前 64 项内是否存在覆盖当前全部字面音节的未纠错精确词典整词（含被前缀或占位规则后置的整词，如 `遇得到`/`按理来说`）。
fn has_any_exact_literal_dict_candidate(result: &[WordItem], segments: &[String]) -> bool {
    result.iter().take(64).any(|item| {
        item.source.is_dictionary()
            && item.corrected_from.is_empty()
            && !item.fuzzy
            && !item.canonical_pinyin.is_empty()
            && item
                .canonical_pinyin
                .split('\'')
                .eq(segments.iter().map(String::as_str))
    })
}

/// 从 2 音节词网格解码路径中收集前 8 条 `[1,1]` 拆字组合，供平等送入 Top-24/32 融合池。
fn collect_two_syllable_split_pool(paths: &[SentencePath], typed: &str, out: &mut Vec<WordItem>) {
    const TWO_SYLLABLE_SPLIT_POOL_LIMIT: usize = 8;
    for path in paths {
        if path.words.len() != 2 {
            continue;
        }
        if out.iter().any(|existing| existing.word == path.sentence) {
            continue;
        }
        let mut item = WordItem::new(
            typed,
            path.sentence.clone(),
            (path.log_prob * 1000.0) as i64,
            CandidateSource::Generated,
            path.key.clone(),
        );
        item.sentence_association = true;
        item.sentence_words = path.words.clone();
        out.push(item);
        if out.len() >= TWO_SYLLABLE_SPLIT_POOL_LIMIT {
            break;
        }
    }
}

/// 把双音节 `[1,1]` 拆字组合插在前排精确词典同音词（至多前 8 席）之后，确保进入 Top-24/32 统一融合池。
fn insert_two_syllable_split_pool(
    result: &mut Vec<WordItem>,
    segments: &[String],
    splits: Vec<WordItem>,
) {
    const MAX_LEADING_EXACT_BEFORE_SPLITS: usize = 8;
    let mut at =
        whole_sentence_insert_position(result, segments).min(MAX_LEADING_EXACT_BEFORE_SPLITS);
    while at < result.len() && result[at].sentence_association {
        at += 1;
    }
    let mut to_insert = Vec::new();
    for item in splits {
        if !result.iter().any(|existing| existing.word == item.word) {
            to_insert.push(item);
        }
    }
    if !to_insert.is_empty() {
        result.splice(at..at, to_insert);
    }
}

/// 在统一对数概率尺度下融合主切分与全部字面备选切分的整句行，按 `weight`（毫 nat）降序排列并按词面去重。
fn merge_alternative_sentences_by_score(
    result: &mut Vec<WordItem>,
    raw: &str,
    primary_segments: &[String],
    alt_sentences: Vec<WordItem>,
) {
    // 跳过覆盖当前全部字面字母的前排精确词典整词（含主切分与零声母备选切分如 `he'li'an'pai -> 合理安排`、`fang'an -> 方案`）。
    let leading_dict = result
        .iter()
        .take_while(|item| {
            item.source.is_dictionary()
                && !item.canonical_pinyin.is_empty()
                && folded_reading_equal(&item.canonical_pinyin, raw)
        })
        .count();
    let insert_at = if primary_segments.len() == 2 {
        leading_dict.min(8)
    } else {
        leading_dict
    };
    let literal_sentence_led = result
        .get(insert_at..)
        .and_then(|slice| slice.iter().find(|item| item.sentence_association))
        .is_some_and(|first| first.corrected_from.is_empty());
    merge_alternative_sentences_at(result, insert_at, alt_sentences);
    // 若主切分已明确判定未纠错字面整句领跑，合并纯字面隔音备选整句后不得让附带纠错句借重排反超篡夺首槽。
    if literal_sentence_led
        && result
            .get(insert_at)
            .is_some_and(item_is_corrected_sentence)
    {
        if let Some(rel_idx) = result[insert_at..]
            .iter()
            .position(|item| item.sentence_association && item.corrected_from.is_empty())
        {
            let leader = result.remove(insert_at + rel_idx);
            result.insert(insert_at, leader);
        }
    }
}

fn item_is_corrected_sentence(item: &WordItem) -> bool {
    item.sentence_association && !item.corrected_from.is_empty()
}

/// 从给定 `insert_at` 位置起按统一对数分（毫 nat）重排并融合整句候选。
fn merge_alternative_sentences_at(
    result: &mut Vec<WordItem>,
    insert_at: usize,
    alt_sentences: Vec<WordItem>,
) {
    let mut pool: Vec<WordItem> = Vec::new();
    let mut non_sentences: Vec<WordItem> = Vec::with_capacity(result.len());
    for (idx, item) in std::mem::take(result).into_iter().enumerate() {
        if idx >= insert_at && item.sentence_association {
            pool.push(item);
        } else {
            non_sentences.push(item);
        }
    }
    pool.extend(alt_sentences);
    // 按统一自然对数分（毫 nat）稳定降序，纠错整句已含物理信道代价，直接与字面/隔音整句平等竞争。
    pool.sort_by_key(|lhs| std::cmp::Reverse(lhs.weight));
    const MAX_MERGED_ALT_SENTENCES: usize = 24;
    let mut deduped: Vec<WordItem> = Vec::with_capacity(pool.len().min(MAX_MERGED_ALT_SENTENCES));
    for item in pool {
        let exists_in_non_sentences = non_sentences
            .iter()
            .take(insert_at)
            .any(|existing| existing.word == item.word);
        if exists_in_non_sentences || deduped.iter().any(|existing| existing.word == item.word) {
            continue;
        }
        deduped.push(item);
        if deduped.len() >= MAX_MERGED_ALT_SENTENCES {
            break;
        }
    }
    // 若前缀/非整句区存在与融合整句同名的词，剔除后排重复项以维持全局词面唯一。
    let tail_start = insert_at.min(non_sentences.len());
    let mut tail = non_sentences.split_off(tail_start);
    tail.retain(|item| !deduped.iter().any(|sent| sent.word == item.word));
    non_sentences.extend(deduped);
    non_sentences.extend(tail);
    *result = non_sentences;
}

fn first_correction_cut(pinyin: &str) -> Option<Vec<String>> {
    cut_pinyin_by_mode(pinyin, CutMode::Correction)
        .into_iter()
        .next()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "dictionary/alternative_series_tests.rs"]
mod alternative_series_tests;

#[cfg(test)]
#[path = "dictionary/lattice_span_cache_tests.rs"]
mod lattice_span_cache_tests;

#[cfg(test)]
#[path = "dictionary/sentence_decode_cache_tests.rs"]
mod sentence_decode_cache_tests;

#[cfg(test)]
#[path = "dictionary/row_projection_tests.rs"]
mod row_projection_tests;

#[cfg(test)]
#[path = "dictionary/initial_expansion_projection_tests.rs"]
mod initial_expansion_projection_tests;

#[cfg(test)]
#[path = "dictionary/fuzzy_row_projection_tests.rs"]
mod fuzzy_row_projection_tests;

#[cfg(test)]
#[path = "dictionary/resolution_projection_tests.rs"]
mod resolution_projection_tests;

#[cfg(test)]
#[path = "dictionary/primary_segmentation_tests.rs"]
mod primary_segmentation_tests;

#[cfg(test)]
#[path = "dictionary/series_hit_resolution_tests.rs"]
mod series_hit_resolution_tests;

#[cfg(test)]
#[path = "dictionary/lazy_segments_tests.rs"]
mod lazy_segments_tests;

#[cfg(test)]
#[path = "dictionary/slot_key_ownership_tests.rs"]
mod slot_key_ownership_tests;

#[cfg(test)]
#[path = "dictionary/series_prefix_borrow_tests.rs"]
mod series_prefix_borrow_tests;

#[cfg(test)]
#[path = "dictionary/initial_row_key_tests.rs"]
mod initial_row_key_tests;

#[cfg(test)]
#[path = "dictionary/fuzzy_typed_borrow_tests.rs"]
mod fuzzy_typed_borrow_tests;

#[cfg(test)]
#[path = "dictionary/fuzzy_input_borrow_tests.rs"]
mod fuzzy_input_borrow_tests;

#[cfg(test)]
#[path = "dictionary/prefix_row_stream_tests.rs"]
mod prefix_row_stream_tests;

#[cfg(test)]
mod alternative_row_stream_tests;

#[cfg(test)]
mod filtered_query_tests;

#[cfg(test)]
mod empty_projection_tests;

#[cfg(test)]
mod evicted_page_tests;

#[cfg(test)]
mod online_cache_update_tests;

#[cfg(test)]
mod chime_regression_tests {
    use super::*;
    use crate::quanpin::fixture::Fixture;
    use crate::types::FuzzyPinyinOptions;

    const ALL_TYPES: u32 = autocorrect_type::TRANSPOSITION
        | autocorrect_type::NEIGHBOR
        | autocorrect_type::MISSING_OR_EXTRA;
    const NO_FUZZY: FuzzyPinyinOptions = FuzzyPinyinOptions { rules: 0 };

    #[test]
    fn donguan_keeps_leading_corrected_dict_word_ahead_of_compositional_split() {
        let fixture = Fixture::new();
        fixture.insert("dong'guan", "东莞", 374_460);
        fixture.insert("dong'guan", "东关", 15_265);
        fixture.insert("dong", "东", 2_000_000);
        fixture.insert("guan", "官", 1_500_000);
        fixture.insert("dong'huan", "动换", 3_480);

        let mut dict = QuanpinDictionary::new(&fixture.paths);
        dict.sentence_alternatives = true;
        dict.sentence_association = SentenceAssociationOptions {
            word_lattice: true,
            ..SentenceAssociationOptions::default()
        };
        let rows = dict.query("donguan", "", ALL_TYPES, NO_FUZZY);
        assert_eq!(rows.first().map(|r| r.word.as_str()), Some("东莞"));
    }

    #[test]
    fn geminate_elision_and_transposed_nasal_g_calibration_end_to_end() {
        let fixture = Fixture::new();
        // 1. 4 音节吞音词与直拼组合竞争
        fixture.insert("chun'nuan'hua'kai", "春暖花开", 20_870);
        fixture.insert("chu", "出", 1_000_000);
        fixture.insert("nuan", "暖", 300_000);
        fixture.insert("hua", "花", 800_000);
        fixture.insert("kai", "开", 900_000);
        // 2. 2 音节清洗基线保持直拼首屈，同时吞音词进入前排（按 `Z1 = 5e8` 量级模拟真实单字与双字频差）
        fixture.insert("fa'nan", "发难", 13_505);
        fixture.insert("fan'an", "翻案", 5_690);
        fixture.insert("fan'nan", "犯难", 6_680);
        fixture.insert("fa", "发", 50_000_000);
        fixture.insert("fan", "范", 30_000_000);
        fixture.insert("nan", "难", 20_000_000);
        fixture.insert("nan", "南", 18_000_000);
        fixture.insert("han", "汗", 8_000_000);
        fixture.insert("xian'an", "先按", 15_130);
        fixture.insert("xia'nan", "峡南", 5);
        fixture.insert("xian'nan", "险难", 1_175);
        fixture.insert("xian", "先", 40_000_000);
        fixture.insert("xi", "西", 40_000_000);
        fixture.insert("xia", "下", 30_000_000);
        fixture.insert("an", "安", 25_000_000);
        fixture.insert("ku'nan", "苦难", 85_145);
        fixture.insert("kun'nan", "困难", 500_397);
        fixture.insert("ku", "苦", 1_000_000);
        fixture.insert("kun", "困", 500_000);
        fixture.insert("ben'neng", "本能", 123_955);
        fixture.insert("ben", "本", 2_000_000);
        fixture.insert("ceng", "层", 500_000);
        // 3. 句尾 `-gn` 错拼与换位保护
        fixture.insert("mo'hu'yin", "模糊音", 1_200);
        fixture.insert("mo'hu", "模糊", 50_000);
        fixture.insert("ying", "应", 1_500_000);
        fixture.insert("shuang'pin", "双拼", 10_570);

        let mut dict = QuanpinDictionary::new(&fixture.paths);
        dict.sentence_alternatives = true;
        dict.sentence_association = SentenceAssociationOptions {
            word_lattice: true,
            ..SentenceAssociationOptions::default()
        };
        let query_words = |dict: &mut QuanpinDictionary, raw: &str, types: u32| -> Vec<String> {
            dict.query(raw, "", types, NO_FUZZY)
                .into_iter()
                .map(|r| r.word.to_string())
                .collect()
        };

        let chun = query_words(&mut dict, "chunuanhuakai", ALL_TYPES);
        assert_eq!(
            chun.first().map(String::as_str),
            Some("春暖花开"),
            "{chun:?}"
        );
        // 未开启纠错时保持直拼手感，不触发吞音恢复。
        let chun_clean = query_words(&mut dict, "chunuanhuakai", 0);
        assert_ne!(
            chun_clean.first().map(String::as_str),
            Some("春暖花开"),
            "{chun_clean:?}"
        );

        let fan_items = dict.query("fanan", "", ALL_TYPES, NO_FUZZY);
        assert_eq!(fan_items.first().map(|r| r.word.as_str()), Some("发难"));
        let fan_deferred = fan_items
            .iter()
            .take(7)
            .find(|r| r.word == "犯难")
            .unwrap_or_else(|| panic!("fanan 前排应包含 犯难: {fan_items:?}"));
        assert_eq!(fan_deferred.source, CandidateSource::Database);
        assert_eq!(fan_deferred.weight, 6_680);

        let xian_items = dict.query("xianan", "", ALL_TYPES, NO_FUZZY);
        assert_eq!(xian_items.first().map(|r| r.word.as_str()), Some("先按"));
        let xian_deferred = xian_items
            .iter()
            .take(9)
            .find(|r| r.word == "险难")
            .unwrap_or_else(|| panic!("xianan 前排应包含 险难: {xian_items:?}"));
        assert_eq!(xian_deferred.source, CandidateSource::Database);
        assert_eq!(xian_deferred.weight, 1_175);

        let ku_items = dict.query("kunan", "", ALL_TYPES, NO_FUZZY);
        assert_eq!(ku_items.first().map(|r| r.word.as_str()), Some("苦难"));
        assert!(
            ku_items.iter().take(4).any(|r| r.word == "困难"),
            "kunan 前排应包含 困难: {ku_items:?}"
        );

        let ben = query_words(&mut dict, "beneng", ALL_TYPES);
        assert_eq!(ben.first().map(String::as_str), Some("本能"), "{ben:?}");

        let mohu = query_words(&mut dict, "mohuyign", ALL_TYPES);
        assert!(
            mohu.iter().take(3).any(|w| w == "模糊音"),
            "mohuyign 前 3 应包含 模糊音: {mohu:?}"
        );

        let shuang = query_words(&mut dict, "shaungpin", ALL_TYPES);
        assert_eq!(
            shuang.first().map(String::as_str),
            Some("双拼"),
            "{shuang:?}"
        );
    }
}
