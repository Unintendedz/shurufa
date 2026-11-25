//! `ShuangpinEngine`：双拼词典系列查询与模糊音合并。

use std::collections::HashSet;

use super::dictionary::ShuangpinDictionary;
use super::query::{
    fuzzy_initial_spellings, remove_manual_delimiters, segment_input, to_quanpin_segmentation,
};
use super::ShuangpinProfile;
use crate::paths::RuntimePaths;
use crate::quanpin::QuanpinDictionary;
use crate::types::{
    autocorrect_type, CandidateSource, FuzzyPinyinOptions, QueryRequest, SchemeType, WordItem,
};

// 双拼模糊候选的短合并直接扫描已有词，避免临时哈希表和重复索引分配。
const SMALL_FUZZY_DEDUP: usize = 64;

pub struct ShuangpinEngine {
    profile: &'static ShuangpinProfile,
    dictionary: ShuangpinDictionary,
    paths: RuntimePaths,
    /// 首次启用模糊音查询时按需打开。
    fuzzy_dictionary: Option<QuanpinDictionary>,
}

impl ShuangpinEngine {
    pub fn new(profile: &'static ShuangpinProfile, paths: &RuntimePaths) -> Self {
        Self {
            profile,
            dictionary: ShuangpinDictionary::new(profile, paths),
            paths: paths.clone(),
            fuzzy_dictionary: None,
        }
    }

    pub fn query(&mut self, request: &QueryRequest) -> Vec<WordItem> {
        let mut candidates = self.query_standard(request);
        if request.valid {
            let alternatives =
                fuzzy_initial_spellings(&request.raw_input, self.profile, request.fuzzy_pinyin, 32);
            if !alternatives.is_empty() {
                candidates = self.query_fuzzy_alternatives(request, candidates, alternatives);
            }
        }
        candidates
    }

    fn query_fuzzy_alternatives(
        &mut self,
        request: &QueryRequest,
        mut candidates: Vec<WordItem>,
        alternatives: Vec<String>,
    ) -> Vec<WordItem> {
        for spelling in alternatives {
            let mut alternate = request.clone();
            alternate.raw_input_with_cases = spelling
                .bytes()
                .zip(request.raw_input_with_cases.bytes())
                .map(|(new, old)| {
                    char::from(if old.is_ascii_uppercase() {
                        new.to_ascii_uppercase()
                    } else {
                        new
                    })
                })
                .collect();
            alternate.raw_input = spelling;
            let mut rows = self.query_standard(&alternate);
            rows.retain_mut(|row| {
                let mut original = request.raw_input.bytes().filter(|&byte| byte != b'\'');
                let code: Option<String> = row
                    .pinyin
                    .bytes()
                    .map(|byte| {
                        if byte == b'\'' {
                            Some('\'')
                        } else {
                            original.next().map(char::from)
                        }
                    })
                    .collect();
                let Some(code) = code else {
                    return false;
                };
                row.pinyin = code.into();
                row.fuzzy = true;
                true
            });
            append_fuzzy_rows(&mut candidates, rows);
        }
        sort_by_coverage(&mut candidates);
        sort_dictionary_weights(&mut candidates);
        keep_typo_sentence_off_first_seat(&mut candidates);
        candidates
    }

    fn query_standard(&mut self, request: &QueryRequest) -> Vec<WordItem> {
        if !request.valid {
            return Vec::new();
        }
        self.dictionary
            .set_sentence_alternatives(request.sentence_alternatives);
        self.dictionary
            .set_sentence_association(request.sentence_association.clone());
        self.dictionary
            .set_rescoring_context(&request.rescoring_context);
        self.dictionary
            .set_autocorrect_types(request_autocorrect_types(request));

        let raw = &request.raw_input;
        if raw.bytes().all(|byte| byte == b'\'') {
            return Vec::new();
        }
        let segmentation = segment_input(raw, self.profile);
        let exact = self
            .dictionary
            .generate_series(&remove_manual_delimiters(raw), &segmentation, raw);
        let mut candidates = self.append_fuzzy(exact, &segmentation, request.fuzzy_pinyin);
        keep_typo_sentence_off_first_seat(&mut candidates);
        candidates
    }

    fn append_fuzzy(
        &mut self,
        mut exact: Vec<WordItem>,
        raw_segmentation: &str,
        options: FuzzyPinyinOptions,
    ) -> Vec<WordItem> {
        if options.rules == 0 {
            return exact;
        }
        let paths = &self.paths;
        let fuzzy_dictionary = self
            .fuzzy_dictionary
            .get_or_insert_with(|| QuanpinDictionary::new(paths));
        let mut fuzzy = fuzzy_dictionary.fuzzy_candidates(
            &to_quanpin_segmentation(raw_segmentation, self.profile),
            options,
        );
        relabel_fuzzy_rows(&mut fuzzy, raw_segmentation);
        if fuzzy.is_empty() {
            sort_by_coverage(&mut exact);
            return exact;
        }
        append_fuzzy_rows(&mut exact, fuzzy);
        sort_by_coverage(&mut exact);
        sort_dictionary_weights(&mut exact);
        exact
    }

    pub fn expand_initial_candidates(
        &mut self,
        request: &QueryRequest,
        candidates: &mut Vec<WordItem>,
    ) -> bool {
        if request.scheme != SchemeType::Shuangpin {
            return false;
        }
        let segmentation = segment_input(&request.raw_input, self.profile);
        let initial = segmentation.split('\'').next().unwrap_or_default();
        if initial.len() != 1 {
            return false;
        }
        self.dictionary
            .expand_initial_candidates(initial, candidates, &request.raw_input)
    }

    pub fn insert_online_words(
        &mut self,
        request: &QueryRequest,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        if !request.valid {
            return false;
        }
        self.dictionary
            .set_rescoring_context(&request.rescoring_context);
        self.dictionary
            .insert_word_to_series_cache(&request.raw_input, words, source)
    }

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        self.dictionary.clear_online_candidates(source);
    }

    #[cfg(test)]
    pub fn find_candidate(&self, key: &str, value: &str) -> Option<WordItem> {
        self.dictionary.find_candidate(key, value)
    }

    pub fn has_dictionary_reranker(&self) -> bool {
        self.dictionary.has_dictionary_reranker()
    }

    pub fn pick_dictionary_homophone(
        &mut self,
        items: &[&WordItem],
        context: &str,
        effective_weights: Option<&[f64]>,
    ) -> Option<usize> {
        self.dictionary
            .pick_dictionary_homophone(items, context, effective_weights)
    }

    pub fn reset_cache(&mut self) {
        self.dictionary.reset_cache();
        if let Some(fuzzy_dictionary) = &mut self.fuzzy_dictionary {
            fuzzy_dictionary.reset_cache();
        }
    }
}

fn request_autocorrect_types(request: &QueryRequest) -> u32 {
    let transposition = if request.enable_quanpin_autocorrect_transposition {
        autocorrect_type::TRANSPOSITION
    } else {
        0
    };
    let neighbor = if request.enable_quanpin_autocorrect_neighbor {
        autocorrect_type::NEIGHBOR
    } else {
        0
    };
    transposition | neighbor
}

fn coverage(item: &WordItem) -> usize {
    item.pinyin.bytes().filter(|&byte| byte != b'\'').count()
}

fn sort_by_coverage(rows: &mut [WordItem]) {
    rows.sort_by_key(|item| std::cmp::Reverse(coverage(item)));
    let Some(first) = rows.first() else {
        return;
    };
    if !first.sentence_association {
        return;
    }
    let full = coverage(first);
    let bucket = rows
        .iter()
        .take_while(|item| coverage(item) == full)
        .count();
    if rows[..bucket]
        .iter()
        .any(|item| !item.fuzzy && item.source.is_dictionary())
    {
        return;
    }
    rows[..bucket].sort_by_key(|item| !(item.fuzzy && item.source.is_dictionary()));
}

fn sort_dictionary_weights(rows: &mut [WordItem]) {
    let mut start = 0;
    while start < rows.len() {
        let count = coverage(&rows[start]);
        let end = start
            + rows[start..]
                .iter()
                .take_while(|row| coverage(row) == count)
                .count();
        let bucket = &mut rows[start..end];
        let mut previous = i64::MAX;
        let unsorted = bucket
            .iter()
            .filter(|row| row.source.is_dictionary())
            .any(|row| {
                let inverted = row.weight > previous;
                previous = row.weight;
                inverted
            });
        if unsorted {
            let (positions, mut dictionary): (Vec<_>, Vec<_>) = bucket
                .iter_mut()
                .enumerate()
                .filter(|(_, row)| row.source.is_dictionary())
                .map(|(index, row)| (index, std::mem::take(row)))
                .unzip();
            dictionary.sort_by_key(|row| std::cmp::Reverse(row.weight));
            for (index, row) in positions.into_iter().zip(dictionary) {
                bucket[index] = row;
            }
        }
        start = end;
    }
}

fn keep_typo_sentence_off_first_seat(rows: &mut Vec<WordItem>) {
    if rows.first().is_none_or(|row| row.corrected_from.is_empty()) {
        return;
    }
    match rows.iter().position(|row| row.corrected_from.is_empty()) {
        Some(literal) => rows[..=literal].rotate_right(1),
        None => rows.clear(),
    }
}

fn segment_count(segmentation: &str) -> usize {
    if segmentation.is_empty() {
        0
    } else {
        segmentation.bytes().filter(|&byte| byte != b'\'').count();
        segmentation.bytes().filter(|&byte| byte == b'\'').count() + 1
    }
}

fn relabel_fuzzy_rows(rows: &mut [WordItem], raw_segmentation: &str) {
    for item in rows {
        if let Some(prefix) = segment_prefix(raw_segmentation, segment_count(&item.pinyin)) {
            item.pinyin.clear();
            item.pinyin.push_str(prefix);
        }
    }
}

fn segment_prefix(segmentation: &str, count: usize) -> Option<&str> {
    if count == 0 {
        return Some("");
    }
    if segmentation.is_empty() {
        return None;
    }
    let mut segments = segmentation.split('\'');
    let mut end = segments.next()?.len();
    for _ in 1..count {
        end += 1 + segments.next()?.len();
    }
    Some(&segmentation[..end])
}

fn append_fuzzy_rows(exact: &mut Vec<WordItem>, fuzzy: Vec<WordItem>) {
    if exact.len().saturating_add(fuzzy.len()) <= SMALL_FUZZY_DEDUP {
        exact.reserve(fuzzy.len());
        for item in fuzzy {
            if exact.iter().any(|existing| existing.word == item.word) {
                continue;
            }
            exact.push(item);
        }
        return;
    }
    let mut seen: HashSet<&str> = exact.iter().map(|item| item.word.as_str()).collect();
    let duplicates = fuzzy
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (!seen.insert(item.word.as_str())).then_some(index))
        .collect::<Vec<_>>();
    drop(seen);
    let unique_count = fuzzy.len() - duplicates.len();
    exact.reserve(unique_count);
    let mut duplicates = duplicates.into_iter().peekable();
    exact.extend(fuzzy.into_iter().enumerate().filter_map(|(index, item)| {
        if duplicates.peek() == Some(&index) {
            duplicates.next();
            None
        } else {
            Some(item)
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::{
        append_fuzzy_rows, keep_typo_sentence_off_first_seat, relabel_fuzzy_rows, segment_count,
        sort_by_coverage, sort_dictionary_weights,
    };
    use crate::types::{CandidateSource, WordItem};

    fn row(word: &str) -> WordItem {
        WordItem::new("ni", word, 1, CandidateSource::Database, "ni")
    }

    #[test]
    fn fuzzy_dictionary_weights_keep_sentence_scores_and_prefix_slots_separate() {
        let literal = WordItem::new("nihc", "你好", 100, CandidateSource::Database, "ni'hao");
        let mut fuzzy = WordItem::new("ni'hc", "利好", 500, CandidateSource::Database, "li'hao");
        fuzzy.fuzzy = true;
        let sentence = WordItem::new("nihc", "拟好", 10000, CandidateSource::Generated, "ni'hao");
        let prefix = WordItem::new("ni", "你", 100000, CandidateSource::Database, "ni");
        let mut rows = vec![
            literal.clone(),
            sentence.clone(),
            fuzzy.clone(),
            prefix.clone(),
        ];
        sort_by_coverage(&mut rows);
        sort_dictionary_weights(&mut rows);
        assert_eq!(rows, [fuzzy, sentence, literal, prefix]);
    }

    #[test]
    fn fuzzy_relabeling_reuses_capacity_without_allocating() {
        let mut rows = vec![row("首"), row("次"), row("空")];
        rows[0].pinyin = "zhi'hao'jie".into();
        rows[1].pinyin = "zhi'hao".into();
        rows[2].pinyin.clear();
        let pointers = rows
            .iter()
            .map(|row| row.pinyin.as_ptr())
            .collect::<Vec<_>>();
        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            relabel_fuzzy_rows(&mut rows, "zi'hc'jx");
        });
        assert_eq!(allocations, 0);
        assert_eq!(
            rows.iter()
                .map(|row| row.pinyin.as_str())
                .collect::<Vec<_>>(),
            ["zi'hc'jx", "zi'hc", ""]
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.pinyin.as_ptr())
                .collect::<Vec<_>>(),
            pointers
        );
    }

    #[test]
    fn segment_count_does_not_allocate_for_fuzzy_row_relabeling() {
        let (count, allocations) =
            crate::ime::personal_rerank::allocations::count(|| segment_count("ni'hao'jie"));
        assert_eq!(count, 3);
        assert_eq!(allocations, 0);
        assert_eq!(segment_count(""), 0);
        assert_eq!(segment_count("ni"), 1);
    }

    #[test]
    fn short_fuzzy_rows_are_appended_without_temporary_heap_state() {
        let mut exact = Vec::with_capacity(8);
        exact.push(row("你"));
        let fuzzy = vec![row("你"), row("好"), row("好"), row("吗")];
        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            append_fuzzy_rows(&mut exact, fuzzy);
        });
        assert_eq!(allocations, 0);
    }

    #[test]
    fn sorting_by_coverage_keeps_the_typo_sentence_off_the_first_seat() {
        let prefix =
            |word: &str| WordItem::new("mwgf", word, 1, CandidateSource::Database, "mei'gen");
        let mut typo = WordItem::new(
            "mwgfxi",
            "没关系",
            1,
            CandidateSource::Generated,
            "mei'guan'xi",
        );
        typo.sentence_association = true;
        typo.corrected_from = "mwgfxi".into();
        let mut rows = vec![prefix("没跟"), typo, prefix("没")];
        sort_by_coverage(&mut rows);
        keep_typo_sentence_off_first_seat(&mut rows);
        assert_eq!(rows[0].word, "没跟");
    }
}
