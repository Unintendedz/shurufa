//! `ShuangpinDictionary`：双拼切分全拼后的系列查词、词网格整句合并与在线行缓存。

use std::collections::HashSet;
use std::sync::Arc;

use super::query::{raw_length_for_effective_prefix, remove_manual_delimiters};
use super::typo_edges::{collect_shuangpin_typo_edges, SHUANGPIN_TYPO_TYPES};
use super::utils::convert_seg_shuangpin_to_seg_complete_pinyin;
use super::ShuangpinProfile;
use crate::assets;
use crate::cache::FifoCache;
use crate::dictionary::pinyin::PinyinDatabase;
use crate::dictionary::DictRow;
use crate::ime::online_batch::{
    replace_cached_online_candidate_batch, replace_online_candidate_batch,
};
use crate::lattice::decode::{decode_sentences, SentenceNgramTables};
use crate::lattice::merge::{
    merge_decoded, merge_lattice_candidates_with_typo_policy, whole_sentence_insert_position,
    DecodedSentences,
};
use crate::lattice::octagram::{pick_collocation_homophone, CONTEXT_CHARACTERS};
use crate::lattice::{SentencePath, TypoEdgeSource};
use crate::paths::RuntimePaths;
use crate::pinyin::jianpin::QuerySource;
use crate::pinyin::segment::split_segments;
use crate::pinyin::syllables::{has_only_complete_pinyin_segments, is_intact};
use crate::quanpin::dictionary::{
    CACHE_CAPACITY, INITIAL_CANDIDATE_LIMIT, TYPO_SPAN_CACHE_CAPACITY,
};
use crate::quanpin::series::fold_reading;
use crate::text::last_characters;
use crate::types::{
    CandidateSource, SentenceAssociationOptions, ShuangpinProfileKind, WordItem,
};
use crate::user_dictionary::ngram_store::PersonalNgramStore;

// 双拼前缀候选的短合并直接扫描已有词，避免临时哈希表和重复索引分配。
const SMALL_PREFIX_DEDUP: usize = 64;
/// 常见双拼纠错输入的两键分段放在栈上。
const INLINE_TYPO_CODES: usize = 32;
const UNLIMITED_ROWS: usize = i32::MAX as usize;
const PERSONAL_SCORED_KEY_LIMIT: usize = 2 * CACHE_CAPACITY;

pub struct ShuangpinDictionary {
    profile: &'static ShuangpinProfile,
    database: PinyinDatabase,
    sentence_tables: SentenceNgramTables,
    personal: Arc<PersonalNgramStore>,
    collocation_db: Option<Arc<crate::lattice::octagram::GramDb>>,
    sentence_alternatives: bool,
    sentence_association: SentenceAssociationOptions,
    rescoring_context: String,
    rows_cache: FifoCache<String, Vec<WordItem>>,
    series_cache: FifoCache<String, Vec<WordItem>>,
    personal_scored_keys: HashSet<String>,
    personal_version: u64,
    typo_span_cache: FifoCache<String, Vec<DictRow>>,
    autocorrect_types: u32,
}

impl ShuangpinDictionary {
    pub fn new(profile: &'static ShuangpinProfile, paths: &RuntimePaths) -> Self {
        let mut database = PinyinDatabase::open(&paths.dictionary(assets::MAIN_DICTIONARY));
        let mut sentence_tables = SentenceNgramTables::default();
        if database.is_open() {
            database.warm_up();
            sentence_tables = SentenceNgramTables::load(paths);
            database.database_changed();
        }
        let personal = PersonalNgramStore::for_journal(&paths.user(assets::USER_JOURNAL));
        drop(personal.model());
        let personal_version = personal.version();
        Self {
            profile,
            database,
            sentence_tables,
            personal,
            collocation_db: None,
            sentence_alternatives: false,
            sentence_association: SentenceAssociationOptions::default(),
            rescoring_context: String::new(),
            rows_cache: FifoCache::new(CACHE_CAPACITY),
            series_cache: FifoCache::new(CACHE_CAPACITY),
            personal_scored_keys: HashSet::new(),
            personal_version,
            typo_span_cache: FifoCache::new(TYPO_SPAN_CACHE_CAPACITY),
            autocorrect_types: 0,
        }
    }

    fn sentence_reads_context(&self) -> bool {
        self.has_dictionary_reranker()
    }

    fn sentence_cache_key(&self, key: &str) -> String {
        if !self.sentence_reads_context() {
            key.to_string()
        } else {
            format!("{key}\u{1}{}", self.rescoring_context)
        }
    }

    fn reset_cache_if_database_changed(&mut self) {
        if self.database.database_changed() {
            self.reset_cache();
        }
    }

    fn drop_personal_scored_results(&mut self) {
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

    fn query_initial(&self, code: &str, limit: usize) -> Vec<WordItem> {
        if !self.database.is_open() || code.len() != 1 || !code.as_bytes()[0].is_ascii_lowercase() {
            return Vec::new();
        }
        let initial = convert_seg_shuangpin_to_seg_complete_pinyin(code, self.profile);
        self.database
            .query_initial(&initial, limit)
            .into_iter()
            .map(|row| {
                WordItem::new(
                    code,
                    row.value,
                    row.weight,
                    CandidateSource::Database,
                    row.key,
                )
            })
            .collect()
    }

    fn query_rows(&self, pure: &str, segmentation: &str) -> Vec<WordItem> {
        if !self.database.is_open() || segmentation.is_empty() {
            return Vec::new();
        }
        let segments = split_segments(&convert_seg_shuangpin_to_seg_complete_pinyin(
            segmentation,
            self.profile,
        ));
        if segments.is_empty() {
            return Vec::new();
        }
        let mut rows = self.database.query_segments_keyed_flat(
            &segments,
            UNLIMITED_ROWS,
            QuerySource::Shuangpin,
        );
        if self.profile.kind == ShuangpinProfileKind::Shoudao && segmentation == "dk" {
            rows.extend(self.database.query_segments_keyed_flat(
                &["den".to_string()],
                UNLIMITED_ROWS,
                QuerySource::Shuangpin,
            ));
            rows.sort_by_key(|row| std::cmp::Reverse(row.weight));
            let mut seen = HashSet::new();
            rows.retain(|row| seen.insert(row.value.clone()));
        }
        rows.into_iter()
            .map(|row| {
                WordItem::new(
                    pure,
                    row.value,
                    row.weight,
                    CandidateSource::Database,
                    row.key,
                )
            })
            .collect()
    }

    fn generate(&mut self, pure: &str, segmentation: &str, cache_key: &str) -> Vec<WordItem> {
        match pure.len() {
            0 => return Vec::new(),
            1 => return self.query_initial(pure, INITIAL_CANDIDATE_LIMIT),
            _ => {}
        }
        let key = if cache_key.is_empty() {
            pure
        } else {
            cache_key
        };
        if self.rows_cache.get_ref_by(key).is_some() {
            self.reset_cache_if_database_changed();
            if let Some(cached) = self.rows_cache.get_ref_by(key) {
                return cached.clone();
            }
        }
        let rows = self.query_rows(pure, segmentation);
        self.rows_cache.insert(key.to_owned(), rows.clone());
        rows
    }

    pub fn generate_series(&mut self, pure: &str, segmentation: &str, raw: &str) -> Vec<WordItem> {
        match pure.len() {
            0 => return Vec::new(),
            1 => return self.query_initial(pure, INITIAL_CANDIDATE_LIMIT),
            _ => {}
        }
        let rows_key = if raw.is_empty() { pure } else { raw };
        let mut key = self
            .sentence_reads_context()
            .then(|| self.sentence_cache_key(rows_key));
        self.drop_personal_scored_results();
        let cache_hit = match key.as_ref() {
            Some(key) => self.series_cache.get_ref(key).is_some(),
            None => self.series_cache.get_ref_by(rows_key).is_some(),
        };
        if cache_hit {
            self.reset_cache_if_database_changed();
            let cached = match key.as_ref() {
                Some(key) => self.series_cache.get_ref(key),
                None => self.series_cache.get_ref_by(rows_key),
            };
            if let Some(cached) = cached {
                return cached.clone();
            }
        }

        let segments = split_segments(&convert_seg_shuangpin_to_seg_complete_pinyin(
            segmentation,
            self.profile,
        ));
        let incomplete_tail = segments.split_last().is_some_and(|(last, leading)| {
            !is_intact(last) && has_only_complete_pinyin_segments(leading)
        });
        let mut candidates = self.generate(pure, segmentation, rows_key);
        let mut prefix_rows = Vec::with_capacity(prefix_group_count(segmentation));
        let mut prefix = segmentation;
        while let Some(cut) = prefix.rfind('\'') {
            prefix = &prefix[..cut];
            let prefix_pure = remove_manual_delimiters(prefix);
            let rows = if incomplete_tail && prefix_rows.is_empty() {
                let end = raw_length_for_effective_prefix(rows_key, prefix_pure.len());
                self.generate_series(&prefix_pure, prefix, &rows_key[..end])
            } else {
                self.generate(&prefix_pure, prefix, "")
            };
            prefix_rows.push(rows);
        }
        append_prefix_rows(&mut candidates, prefix_rows);

        self.merge_sentences(&mut candidates, &segments, segmentation, pure, rows_key);

        let key = key.take().unwrap_or_else(|| rows_key.to_owned());
        self.series_cache.insert(key.clone(), candidates.clone());
        if segments.len() >= 2 {
            self.personal_scored_keys.insert(key);
            if self.personal_scored_keys.len() > PERSONAL_SCORED_KEY_LIMIT {
                let series_cache = &self.series_cache;
                self.personal_scored_keys
                    .retain(|scored| series_cache.contains(scored));
            }
        }
        candidates
    }

    fn merge_sentences(
        &mut self,
        candidates: &mut Vec<WordItem>,
        segments: &[String],
        segmentation: &str,
        typed: &str,
        rows_key: &str,
    ) {
        let association = self.sentence_association.clone();
        let word_lattice = association.word_lattice;
        let has_collocation =
            self.collocation_db.is_some() && association.collocation_weight != 0.0;
        let include_collocation_best = association.collocation_rerank && has_collocation;
        if !word_lattice && !include_collocation_best {
            return;
        }
        let mut options = self.sentence_tables.options(self.sentence_alternatives);
        options.include_lattice_best = word_lattice;
        options.include_collocation_best = include_collocation_best;
        options.show_next_on_duplicate = association.show_next_on_duplicate;
        let scorer;
        if has_collocation {
            let db = self.collocation_db.as_ref().expect("collocation db loaded");
            let config = association.collocation_grammar;
            scorer =
                move |tail: &str, word: &str, is_rear: bool| db.query(tail, word, is_rear, &config);
            options.collocation_scorer = Some(&scorer);
            options.collocation_weight = association.collocation_weight;
        }
        let model = self.personal.model();
        options.personal = (!model.is_empty()).then_some(&*model);
        if has_collocation {
            options.preceding_context = crate::text::trailing_han_clause(&self.rescoring_context);
        }
        let database = &self.database;
        let span_limit = options.span_limit;
        if segments.split_last().is_some_and(|(last, leading)| {
            !is_intact(last) && has_only_complete_pinyin_segments(leading)
        }) {
            let mut lookup = |span: &[String]| {
                database.query_lattice_span_with_trailing_initial(span, span_limit)
            };
            let paths = decode_sentences(segments, &mut lookup, &options);
            merge_decoded(
                candidates,
                segments,
                typed,
                &options,
                DecodedSentences { paths, typo: None },
            );
            return;
        }
        let mut lookup = |span: &[String]| database.query_lattice_span(span, span_limit);
        let codes = typo_codes(self.autocorrect_types, segments, segmentation, rows_key);
        let types = self.autocorrect_types;
        let profile = self.profile;
        let span_cache = &mut self.typo_span_cache;
        let mut typo_edges = |literal_best: &SentencePath| {
            collect_shuangpin_typo_edges(
                database,
                span_cache,
                profile,
                codes.as_slice(),
                segments,
                literal_best,
                types,
            )
        };
        let typo_source: Option<&mut TypoEdgeSource<'_>> = if codes.is_empty() {
            None
        } else {
            Some(&mut typo_edges)
        };
        let typo = merge_lattice_candidates_with_typo_policy(
            candidates,
            segments,
            &mut lookup,
            typed,
            &options,
            typo_source,
            true,
        );
        let Some(typo) = typo else {
            return;
        };
        if candidates.iter().any(|item| item.word == typo.sentence) {
            return;
        }
        let Some(at) = typo_sentence_seat(candidates, segments) else {
            return;
        };
        let mut sentence = WordItem::new(
            typed,
            typo.sentence,
            (typo.score * 1000.0) as i64,
            CandidateSource::Generated,
            typo.key,
        );
        sentence.sentence_association = true;
        sentence.sentence_words = typo.words;
        sentence.corrected_from = fold_reading(typed).into();
        candidates.insert(at, sentence);
    }

    pub fn expand_initial_candidates(
        &mut self,
        code: &str,
        candidates: &mut Vec<WordItem>,
        series_key: &str,
    ) -> bool {
        if code.len() != 1 {
            return false;
        }
        let is_limited =
            |item: &WordItem| item.source == CandidateSource::Database && item.pinyin == code;
        let limited = candidates.iter().filter(|item| is_limited(item)).count();
        if limited != INITIAL_CANDIDATE_LIMIT {
            return false;
        }
        let expanded = self.query_initial(code, UNLIMITED_ROWS);
        if expanded.len() <= limited {
            return false;
        }
        let first_limited = candidates
            .iter()
            .position(is_limited)
            .expect("limited count checked above");
        candidates.reserve_exact(expanded.len() - limited);
        let mut removed = 0;
        candidates.retain(|item| {
            if is_limited(item) {
                removed += 1;
                false
            } else {
                true
            }
        });
        debug_assert_eq!(removed, limited);
        candidates.splice(first_limited..first_limited, expanded.iter().cloned());
        if !series_key.is_empty() {
            let key = self.sentence_cache_key(series_key);
            self.series_cache.insert(key, candidates.clone());
        }
        true
    }

    pub fn insert_word_to_series_cache(
        &mut self,
        raw: &str,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        if raw.is_empty() {
            return false;
        }
        let key = self.sentence_cache_key(raw);
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

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        self.series_cache.retain_mut(|_, rows| {
            rows.retain(|item| item.source != source);
            !rows.is_empty()
        });
    }

    #[cfg(test)]
    pub fn find_candidate(&self, key: &str, value: &str) -> Option<WordItem> {
        self.database
            .find_weight(key, value)
            .map(|weight| WordItem::new(key, value, weight, CandidateSource::Database, key))
    }

    pub fn set_autocorrect_types(&mut self, types: u32) {
        let types = types & SHUANGPIN_TYPO_TYPES;
        if self.autocorrect_types == types {
            return;
        }
        self.autocorrect_types = types;
        self.series_cache.clear();
        self.personal_scored_keys.clear();
    }

    pub fn set_sentence_alternatives(&mut self, enabled: bool) {
        if self.sentence_alternatives == enabled {
            return;
        }
        self.sentence_alternatives = enabled;
        self.series_cache.clear();
    }

    pub fn set_sentence_association(&mut self, options: SentenceAssociationOptions) {
        if self.sentence_association == options {
            return;
        }
        self.collocation_db = if options.collocation_model.as_os_str().is_empty() {
            None
        } else {
            crate::lattice::octagram::GramDb::shared(&options.collocation_model)
        };
        self.sentence_association = options;
        self.reset_cache();
    }

    pub fn set_rescoring_context(&mut self, context: &str) {
        let trimmed = last_characters(context, CONTEXT_CHARACTERS);
        if self.rescoring_context != trimmed {
            self.rescoring_context = trimmed.to_string();
        }
    }

    pub fn has_dictionary_reranker(&self) -> bool {
        self.collocation_db.is_some() && self.sentence_association.collocation_weight != 0.0
    }

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

    pub fn reset_cache(&mut self) {
        self.rows_cache.clear();
        self.series_cache.clear();
        self.personal_scored_keys.clear();
        self.typo_span_cache.clear();
    }
}

fn typo_sentence_seat(candidates: &[WordItem], segments: &[String]) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    let mut at = whole_sentence_insert_position(candidates, segments);
    while at < candidates.len() && candidates[at].sentence_association {
        at += 1;
    }
    Some(at.max(1))
}

struct TypoCodes<'a> {
    values: [&'a str; INLINE_TYPO_CODES],
    len: usize,
    heap: Option<Vec<&'a str>>,
}

impl<'a> TypoCodes<'a> {
    fn empty() -> Self {
        Self {
            values: [""; INLINE_TYPO_CODES],
            len: 0,
            heap: None,
        }
    }

    fn as_slice(&self) -> &[&'a str] {
        self.heap.as_deref().unwrap_or(&self.values[..self.len])
    }

    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

fn typo_codes<'a>(
    types: u32,
    segments: &[String],
    segmentation: &'a str,
    rows_key: &str,
) -> TypoCodes<'a> {
    if types & SHUANGPIN_TYPO_TYPES == 0 || rows_key.contains('\'') {
        return TypoCodes::empty();
    }
    let mut inline = [""; INLINE_TYPO_CODES];
    let mut len = 0;
    let mut heap: Option<Vec<&'a str>> = None;
    for code in segmentation.split('\'') {
        if code.len() != 2 {
            return TypoCodes::empty();
        }
        if let Some(values) = &mut heap {
            values.push(code);
        } else if len < INLINE_TYPO_CODES {
            inline[len] = code;
            len += 1;
        } else {
            let mut values = Vec::with_capacity(segments.len());
            values.extend_from_slice(&inline[..len]);
            values.push(code);
            heap = Some(values);
        }
    }
    match heap {
        Some(values) if values.len() == segments.len() => TypoCodes {
            values: inline,
            len,
            heap: Some(values),
        },
        Some(_) => TypoCodes::empty(),
        None if len == segments.len() => TypoCodes {
            values: inline,
            len,
            heap: None,
        },
        None => TypoCodes::empty(),
    }
}

fn prefix_group_count(segmentation: &str) -> usize {
    segmentation.matches('\'').count()
}

fn append_prefix_rows(candidates: &mut Vec<WordItem>, prefix_rows: Vec<Vec<WordItem>>) {
    let total = candidates.len().saturating_add(
        prefix_rows
            .iter()
            .fold(0usize, |total, rows| total.saturating_add(rows.len())),
    );
    if total <= SMALL_PREFIX_DEDUP {
        candidates.reserve(total.saturating_sub(candidates.len()));
        for rows in prefix_rows {
            for item in rows {
                if candidates.iter().any(|existing| existing.word == item.word) {
                    continue;
                }
                candidates.push(item);
            }
        }
        return;
    }
    let added = total.saturating_sub(candidates.len());
    candidates.reserve(added);
    let original_len = candidates.len();
    for rows in prefix_rows {
        candidates.extend(rows);
    }
    let mut listed = HashSet::with_capacity(candidates.len());
    listed.extend(
        candidates[..original_len]
            .iter()
            .map(|item| item.word.as_str()),
    );
    let mut unique = Vec::with_capacity(added);
    for (index, item) in candidates.iter().enumerate().skip(original_len) {
        if listed.insert(item.word.as_str()) {
            unique.push(index);
        }
    }
    drop(listed);
    let mut write = original_len;
    for read in unique {
        if write != read {
            candidates.swap(write, read);
        }
        write += 1;
    }
    candidates.truncate(write);
}

#[cfg(test)]
mod tests {
    use super::{
        append_prefix_rows, prefix_group_count, typo_codes, typo_sentence_seat, ShuangpinDictionary,
        INLINE_TYPO_CODES, SHUANGPIN_TYPO_TYPES,
    };
    use crate::shuangpin::profile::profile;
    use crate::types::{CandidateSource, WordItem};

    fn row(word: &str) -> WordItem {
        WordItem::new("ni", word, 1, CandidateSource::Database, "ni")
    }

    #[test]
    fn active_dictionary_reuses_ngram_tables_after_shared_eviction() {
        for bigram in [false, true] {
            for trigram in [false, true] {
                let fixture = crate::lattice::active_tables_tests::fixture(bigram, trigram);
                let selected = profile(crate::types::ShuangpinProfileKind::Xiaohe).unwrap();
                let mut dictionary = ShuangpinDictionary::new(selected, &fixture.paths);
                let segments = ["ni", "hao", "ma"].map(str::to_owned);
                crate::lattice::active_tables_tests::assert_query_stable(&fixture.paths, || {
                    let mut result = Vec::new();
                    dictionary.merge_sentences(
                        &mut result,
                        &segments,
                        "ni'hc'ma",
                        "nihcma",
                        "ni'hc'ma",
                    );
                    result
                });
            }
        }
    }

    #[test]
    fn the_typo_sentence_never_takes_the_first_seat() {
        let segments: Vec<String> = ["mei", "gen", "xi"].map(str::to_owned).into();
        let prefix =
            |word: &str| WordItem::new("mwgf", word, 1, CandidateSource::Database, "mei'gen");
        let mut sentence = WordItem::new(
            "mwgfxi",
            "没跟系",
            1,
            CandidateSource::Generated,
            "mei'gen'xi",
        );
        sentence.sentence_association = true;
        let exact = WordItem::new(
            "mwgfxi",
            "没跟戏",
            1,
            CandidateSource::Database,
            "mei'gen'xi",
        );

        assert_eq!(typo_sentence_seat(&[], &segments), None);
        assert_eq!(
            typo_sentence_seat(&[prefix("没跟"), prefix("没")], &segments),
            Some(1)
        );
        assert_eq!(
            typo_sentence_seat(&[sentence.clone(), prefix("没跟")], &segments),
            Some(1)
        );
        assert_eq!(
            typo_sentence_seat(&[exact, sentence, prefix("没跟")], &segments),
            Some(2)
        );
    }

    #[test]
    fn prefix_group_count_matches_manual_boundaries() {
        assert_eq!(prefix_group_count("ni'hao'ba"), 2);
        assert_eq!(prefix_group_count("nihao"), 0);
    }

    #[test]
    fn short_prefix_rows_are_appended_without_temporary_heap_state() {
        let mut candidates = Vec::with_capacity(8);
        candidates.push(row("你"));
        let prefix_rows = vec![vec![row("你"), row("好")], vec![row("好"), row("吗")]];
        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            append_prefix_rows(&mut candidates, prefix_rows);
        });
        assert_eq!(allocations, 0);
    }

    #[test]
    fn typo_code_projection_keeps_invalid_and_long_input_semantics() {
        let segments: Vec<String> = ["mei", "gen"].map(str::to_owned).into();
        assert!(typo_codes(SHUANGPIN_TYPO_TYPES, &segments, "mw'gf", "mw'gf").is_empty());
        assert!(typo_codes(SHUANGPIN_TYPO_TYPES, &segments, "mwgf", "mwgf").is_empty());
        assert!(typo_codes(0, &segments, "mw'gf", "mwgf").is_empty());

        let long_segments = vec!["aa".to_owned(); INLINE_TYPO_CODES + 1];
        let long_segmentation = long_segments.join("'");
        let codes = typo_codes(
            SHUANGPIN_TYPO_TYPES,
            &long_segments,
            &long_segmentation,
            &long_segmentation.replace('\'', ""),
        );
        assert_eq!(codes.as_slice().len(), INLINE_TYPO_CODES + 1);
    }
}

#[cfg(test)]
mod online_cache_update_tests;
