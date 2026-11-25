//! Merging decoded sentences into the candidate list (quanpin.md §10.4, overlays.md §1.6.2).
//!
//! Ranking when merging into an existing list: exact SQLite full-key hits (Database / UserDatabase) first, then the lattice's whole sentences, then prefixes and everything else.

use std::borrow::Cow;
use std::collections::HashSet;

use super::decode::{
    build_graph, build_graph_borrowed, decode_graph, decode_typo_on_graph,
    decode_typo_on_graph_with_policy, LatticeOptions, SentencePath, TypoSentence,
};
use super::{LatticeLookup, TypoEdgeSource};
use crate::pinyin::syllables::{has_only_complete_pinyin_segments, is_intact};
use crate::types::{CandidateSource, WordItem};

// 短句子合并直接扫描已有行，避免默认小批次为临时哈希表分配堆内存。
const SMALL_SENTENCE_MERGE: usize = 64;
/// 双音节输入下排在整句网格之前的精确双字词典行上限（与首页 6 格对齐），确保带语境的 `[1, 1]` 拆字读法紧随第 6 槽稳进神经融合池。
const MAX_LEADING_EXACT_TWO_SYLLABLE_DICT: usize = 6;

/// 判断单个片段是否为合法拼音声母（含单字母声母与 `zh`/`ch`/`sh`）。
fn is_pinyin_initial(segment: &str) -> bool {
    matches!(
        segment,
        "b" | "p"
            | "m"
            | "f"
            | "d"
            | "t"
            | "n"
            | "l"
            | "g"
            | "k"
            | "h"
            | "j"
            | "q"
            | "x"
            | "z"
            | "c"
            | "s"
            | "r"
            | "y"
            | "w"
            | "zh"
            | "ch"
            | "sh"
    )
}

/// 判断音节序列是否由完整拼音音节或合法声母组成，供简全混拼跨度进入词网格解码。
pub(crate) fn has_only_pinyin_or_initial_segments(segments: &[String]) -> bool {
    !segments.is_empty()
        && segments
            .iter()
            .all(|segment| is_intact(segment) || is_pinyin_initial(segment))
}

/// 解码音节并在 `whole_sentence_insert_position` 插入整句块。
#[allow(dead_code)]
pub fn merge_lattice_candidates(
    candidates: &mut Vec<WordItem>,
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    typed_pinyin: &str,
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
) -> Option<TypoSentence> {
    merge_lattice_candidates_with_typo_policy(
        candidates,
        syllables,
        lookup,
        typed_pinyin,
        options,
        typo_source,
        false,
    )
}

/// 双拼的纠错整句即使分数低于字面整句也要保留为候选；全拼沿用只接受胜过字面整句的规则。
pub(crate) fn merge_lattice_candidates_with_typo_policy(
    candidates: &mut Vec<WordItem>,
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    typed_pinyin: &str,
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
    allow_nonleading_typo: bool,
) -> Option<TypoSentence> {
    let decoded = decode_lattice_with_typo_policy(
        syllables,
        lookup,
        options,
        typo_source,
        allow_nonleading_typo,
    )?;
    merge_decoded(candidates, syllables, typed_pinyin, options, decoded)
}

/// 一次整句解码的结果：未截断的路径和纠错整句。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DecodedSentences {
    pub(crate) paths: Vec<SentencePath>,
    pub(crate) typo: Option<TypoSentence>,
}

/// `merge_lattice_candidates` 的解码一半：少于两个音节或含不完整音节时为 `None`。
pub(crate) fn decode_lattice(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
) -> Option<DecodedSentences> {
    decode_lattice_with_typo_policy(syllables, lookup, options, typo_source, false)
}

fn decode_lattice_with_typo_policy(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
    allow_nonleading_typo: bool,
) -> Option<DecodedSentences> {
    if syllables.len() < 2 || !has_only_complete_pinyin_segments(syllables) {
        return None;
    }
    let graph = build_graph(syllables, lookup, options);
    Some(decode_graph_sentences_with_policy(
        syllables,
        options,
        typo_source,
        &graph,
        allow_nonleading_typo,
    ))
}

/// 允许完整音节与声母混排（简全混拼）的词网格解码入口。
#[allow(dead_code)]
pub(crate) fn decode_lattice_mixed(
    syllables: &[String],
    lookup: &mut LatticeLookup<'_>,
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
) -> Option<DecodedSentences> {
    if syllables.len() < 2 || !has_only_pinyin_or_initial_segments(syllables) {
        return None;
    }
    let graph = build_graph(syllables, lookup, options);
    Some(decode_graph_sentences_with_policy(
        syllables,
        options,
        typo_source,
        &graph,
        false,
    ))
}

/// 借用缓存行建图的 `decode_lattice`。
pub(crate) fn decode_lattice_borrowed<'rows>(
    syllables: &[String],
    lookup: &mut impl FnMut(&[String]) -> &'rows [super::decode::LatticeLexeme],
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
) -> Option<DecodedSentences> {
    if syllables.len() < 2 || !has_only_complete_pinyin_segments(syllables) {
        return None;
    }
    let graph = build_graph_borrowed(syllables, lookup, options);
    Some(decode_graph_sentences_with_policy(
        syllables,
        options,
        typo_source,
        &graph,
        false,
    ))
}

/// 借用缓存行建图的简全混拼词网格解码入口。
#[allow(dead_code)]
pub(crate) fn decode_lattice_mixed_borrowed<'rows>(
    syllables: &[String],
    lookup: &mut impl FnMut(&[String]) -> &'rows [super::decode::LatticeLexeme],
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
) -> Option<DecodedSentences> {
    if syllables.len() < 2 || !has_only_pinyin_or_initial_segments(syllables) {
        return None;
    }
    let graph = build_graph_borrowed(syllables, lookup, options);
    Some(decode_graph_sentences_with_policy(
        syllables,
        options,
        typo_source,
        &graph,
        false,
    ))
}

fn decode_graph_sentences_with_policy(
    syllables: &[String],
    options: &LatticeOptions<'_>,
    typo_source: Option<&mut TypoEdgeSource<'_>>,
    graph: &super::decode::Graph,
    allow_nonleading_typo: bool,
) -> DecodedSentences {
    let paths = decode_graph(graph, options, None);
    if paths.is_empty() {
        return DecodedSentences { paths, typo: None };
    }
    let typo = match typo_source {
        Some(source) if syllables.len() >= 2 => {
            let edges = source(&paths[0]);
            if edges.is_empty() {
                None
            } else if allow_nonleading_typo {
                decode_typo_on_graph_with_policy(graph, options, &paths[0], &edges, true)
            } else {
                decode_typo_on_graph(graph, options, &paths[0], &edges)
            }
        }
        _ => None,
    };
    DecodedSentences { paths, typo }
}

/// `merge_lattice_candidates` 的合并一半：把解码出的整句按 `emit` 和去重插进候选，返回纠错整句交给调用方安放。
pub(crate) fn merge_decoded(
    candidates: &mut Vec<WordItem>,
    syllables: &[String],
    typed_pinyin: &str,
    options: &LatticeOptions<'_>,
    decoded: DecodedSentences,
) -> Option<TypoSentence> {
    let DecodedSentences { paths, typo } = decoded;
    if paths.is_empty() {
        return None;
    }
    merge_decoded_paths(
        candidates,
        syllables,
        typed_pinyin,
        options,
        Cow::Owned(paths),
        typo,
    )
}

/// 缓存命中时先借用路径，只为最终入选的候选复制字段。
pub(crate) fn merge_decoded_borrowed(
    candidates: &mut Vec<WordItem>,
    syllables: &[String],
    typed_pinyin: &str,
    options: &LatticeOptions<'_>,
    decoded: &DecodedSentences,
) -> Option<TypoSentence> {
    merge_decoded_paths(
        candidates,
        syllables,
        typed_pinyin,
        options,
        Cow::Borrowed(&decoded.paths),
        decoded.typo.clone(),
    )
}

#[inline(always)]
fn merge_decoded_paths(
    candidates: &mut Vec<WordItem>,
    syllables: &[String],
    typed_pinyin: &str,
    options: &LatticeOptions<'_>,
    mut paths: Cow<'_, [SentencePath]>,
    typo: Option<TypoSentence>,
) -> Option<TypoSentence> {
    if paths.is_empty() {
        return None;
    }

    if options.emit > 0 {
        paths = match paths {
            Cow::Owned(mut paths) => {
                paths.truncate(options.emit);
                Cow::Owned(paths)
            }
            Cow::Borrowed(paths) => Cow::Borrowed(&paths[..paths.len().min(options.emit)]),
        };
    }
    let already = existing_sentence_set(candidates, paths.len());
    let block = if let Some(already) = already.as_ref() {
        let mut block = Vec::new();
        let capacity = paths.len();
        for path in sentence_path_iter(paths) {
            if already.contains(path.sentence.as_str()) {
                continue;
            }
            if !block.is_empty() && options.include_collocation_best && !options.include_lattice_best
            {
                break;
            }
            if block.is_empty() {
                block = Vec::with_capacity(capacity);
            }
            block.push(owned_sentence_row(
                typed_pinyin,
                path.into_owned(),
                lattice_row_source(&block, options),
            ));
        }
        block
    } else {
        generated_block_linear(candidates, paths, typed_pinyin, options)
    };
    if !block.is_empty() {
        let at = whole_sentence_insert_position(candidates, syllables);
        candidates.splice(at..at, block);
    }
    typo
}

fn existing_sentence_set(candidates: &[WordItem], path_count: usize) -> Option<HashSet<&str>> {
    let dedup_size = candidates.len().saturating_add(path_count);
    (path_count > 1 && dedup_size > SMALL_SENTENCE_MERGE).then(|| {
        let mut already = HashSet::with_capacity(dedup_size);
        already.extend(candidates.iter().map(|item| item.word.as_str()));
        already
    })
}

fn owned_sentence_row(typed_pinyin: &str, path: SentencePath, source: CandidateSource) -> WordItem {
    let mut item = WordItem::new(
        typed_pinyin,
        path.sentence,
        (path.log_prob * 1000.0) as i64,
        source,
        path.key,
    );
    item.sentence_association = true;
    item.sentence_words = path.words;
    item
}

enum SentencePathIter<'a> {
    Owned(std::vec::IntoIter<SentencePath>),
    Borrowed(std::slice::Iter<'a, SentencePath>),
}

impl<'a> Iterator for SentencePathIter<'a> {
    type Item = Cow<'a, SentencePath>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Owned(paths) => paths.next().map(Cow::Owned),
            Self::Borrowed(paths) => paths.next().map(Cow::Borrowed),
        }
    }
}

fn sentence_path_iter(paths: Cow<'_, [SentencePath]>) -> SentencePathIter<'_> {
    match paths {
        Cow::Owned(paths) => SentencePathIter::Owned(paths.into_iter()),
        Cow::Borrowed(paths) => SentencePathIter::Borrowed(paths.iter()),
    }
}

fn sentence_seen_linear(candidates: &[WordItem], block: &[WordItem], sentence: &str) -> bool {
    candidates
        .iter()
        .chain(block)
        .any(|item| item.word == sentence)
}

fn lattice_row_source(block: &[WordItem], options: &LatticeOptions<'_>) -> CandidateSource {
    if block.is_empty() && options.include_collocation_best {
        CandidateSource::Collocation
    } else {
        CandidateSource::Generated
    }
}

fn generated_block_linear<'a>(
    candidates: &[WordItem],
    paths: impl Into<Cow<'a, [SentencePath]>>,
    typed_pinyin: &str,
    options: &LatticeOptions<'_>,
) -> Vec<WordItem> {
    let paths = paths.into();
    let mut block = Vec::new();
    let capacity = paths.len();
    for path in sentence_path_iter(paths) {
        if sentence_seen_linear(candidates, &block, &path.sentence) {
            continue;
        }
        if !block.is_empty() && options.include_collocation_best && !options.include_lattice_best {
            break;
        }
        if block.is_empty() {
            block = Vec::with_capacity(capacity);
        }
        block.push(owned_sentence_row(
            typed_pinyin,
            path.into_owned(),
            lattice_row_source(&block, options),
        ));
    }
    block
}

/// 逐段比较候选规范拼音与输入音节（完整音节要求全等，声母简拼允许前缀匹配），零堆分配。
fn canonical_matches_segments(canonical_pinyin: &str, syllables: &[String]) -> bool {
    let mut canon = canonical_pinyin.split('\'');
    for seg in syllables {
        let Some(piece) = canon.next() else {
            return false;
        };
        let matched = if is_intact(seg) {
            piece == seg
        } else if is_pinyin_initial(seg) {
            piece.starts_with(seg.as_str())
        } else {
            piece == seg
        };
        if !matched {
            return false;
        }
    }
    canon.next().is_none()
}

/// 统计位于整句词网格之前的领跑词典精确命中行数。
pub fn whole_sentence_insert_position(candidates: &[WordItem], syllables: &[String]) -> usize {
    if syllables.is_empty() {
        return 0;
    }
    let n = syllables.len();
    let count = candidates
        .iter()
        .take_while(|item| {
            item.source.is_dictionary()
                && if item.canonical_pinyin.is_empty() || item.fuzzy {
                    covers_all_syllables(item, n)
                } else {
                    canonical_matches_segments(&item.canonical_pinyin, syllables)
                }
        })
        .count();
    if n == 2 {
        count.min(MAX_LEADING_EXACT_TWO_SYLLABLE_DICT)
    } else {
        count
    }
}

fn covers_all_syllables(item: &WordItem, syllables: usize) -> bool {
    syllables > 0
        && (item.word.chars().count() == syllables
            || (!item.canonical_pinyin.is_empty()
                && item.canonical_pinyin.split('\'').count() == syllables))
}

#[cfg(test)]
mod tests {
    use super::super::decode::tests::{lookup, syllables, table, Table};
    use super::super::decode::TypoEdge;
    use super::super::ngram::tests::triple_table;
    use super::super::ngram::SENTENCE_START;
    use super::super::personal::tests::record_run;
    use super::super::personal::PersonalNgram;
    use super::*;
    use std::sync::Arc;

    fn merge(
        candidates: &mut Vec<WordItem>,
        text: &str,
        rows: &Table,
        typed: &str,
        options: &LatticeOptions<'_>,
    ) -> Option<TypoSentence> {
        merge_lattice_candidates(
            candidates,
            &syllables(text),
            &mut lookup(rows),
            typed,
            options,
            None,
        )
    }

    fn dictionary_row(pinyin: &str, word: &str, weight: i64, canonical: &str) -> WordItem {
        WordItem::new(pinyin, word, weight, CandidateSource::Database, canonical)
    }

    #[test]
    fn exact_full_key_row_stays_first() {
        let rows = table(&[
            ("gao", &[("高", 12000)]),
            ("tan", &[("碳", 4000)]),
            ("gang", &[("钢", 5000)]),
            ("tan'gang", &[("碳钢", 16000)]),
        ]);
        let mut candidates = vec![dictionary_row("gktjgh", "高碳钢", 50000, "gao'tan'gang")];
        merge(
            &mut candidates,
            "gao'tan'gang",
            &rows,
            "gktjgh",
            &LatticeOptions::default(),
        );
        assert_eq!(candidates[0].word, "高碳钢");
        assert_eq!(candidates[0].source, CandidateSource::Database);
    }

    #[test]
    fn lattice_goes_ahead_of_a_fallback_row() {
        let rows = table(&[
            ("gao", &[("高", 12000)]),
            ("tan", &[("碳", 4000), ("谈", 9000)]),
            ("gang", &[("钢", 5000), ("刚", 9000)]),
            ("nie", &[("镊", 3000)]),
            ("zi", &[("子", 9000)]),
            ("tan'gang", &[("碳钢", 16000)]),
            ("nie'zi", &[("镊子", 18000)]),
        ]);
        let mut candidates = vec![WordItem::new(
            "gktjghnxzi",
            "高谈刚捏子",
            1,
            CandidateSource::Fallback,
            "",
        )];
        merge(
            &mut candidates,
            "gao'tan'gang'nie'zi",
            &rows,
            "gktjghnxzi",
            &LatticeOptions::default(),
        );
        assert_eq!(candidates[0].word, "高碳钢镊子");
        assert!(candidates.iter().any(|item| item.word == "高谈刚捏子"));
    }

    #[test]
    fn exact_two_syllable_row_stays_ahead() {
        let rows = table(&[
            ("nie", &[("捏", 8000)]),
            ("zi", &[("子", 9000)]),
            ("nie'zi", &[("镊子", 18000)]),
        ]);
        let mut candidates = vec![dictionary_row("nxzi", "捏子", 12000, "nie'zi")];
        merge(
            &mut candidates,
            "nie'zi",
            &rows,
            "nxzi",
            &LatticeOptions::default(),
        );
        assert_eq!(candidates[0].word, "捏子");
        assert_eq!(candidates[0].source, CandidateSource::Database);
        let generated = candidates
            .iter()
            .find(|item| item.word == "镊子")
            .expect("镊子 is offered");
        assert_eq!(generated.source, CandidateSource::Generated);
        assert_eq!(generated.pinyin, "nxzi");
        assert_eq!(generated.canonical_pinyin, "nie'zi");
        assert!(generated.sentence_association);
        assert_eq!(generated.sentence_words, ["镊子"]);
    }

    #[test]
    fn prefix_range_row_does_not_outrank_the_sentence() {
        let rows = table(&[("gun", &[("滚", 10000)]), ("qi", &[("起", 9000)])]);
        let mut candidates = vec![dictionary_row("gunqi", "滚球", 30000, "gun'qiu")];
        merge(
            &mut candidates,
            "gun'qi",
            &rows,
            "gunqi",
            &LatticeOptions::default(),
        );
        assert_eq!(candidates[0].word, "滚起");
        assert_eq!(candidates[0].source, CandidateSource::Generated);
        assert_eq!(candidates[1].word, "滚球");
    }

    #[test]
    fn abbreviated_segments_produce_nothing() {
        let rows = table(&[
            ("g", &[("个", 100)]),
            ("k", &[("可", 100)]),
            ("t", &[("他", 100)]),
            ("g'k't", &[("个可他", 50)]),
        ]);
        let mut candidates = Vec::new();
        merge(
            &mut candidates,
            "g'k't",
            &rows,
            "gkt",
            &LatticeOptions::default(),
        );
        assert!(candidates.is_empty());
    }

    #[test]
    fn one_syllable_produces_nothing() {
        let rows = table(&[("ni", &[("你", 100)])]);
        let mut candidates = Vec::new();
        merge(
            &mut candidates,
            "ni",
            &rows,
            "ni",
            &LatticeOptions::default(),
        );
        assert!(candidates.is_empty());
    }

    #[test]
    fn existing_words_are_not_repeated() {
        let rows = table(&[
            ("ni", &[("你", 9000), ("泥", 100)]),
            ("hao", &[("好", 9000)]),
        ]);
        let mut candidates = vec![WordItem::new(
            "nihao",
            "你好",
            1,
            CandidateSource::CloudSuggestion,
            "",
        )];
        merge(
            &mut candidates,
            "ni'hao",
            &rows,
            "nihao",
            &LatticeOptions::default(),
        );
        assert_eq!(candidates[0].word, "泥好");
        assert_eq!(candidates[1].word, "你好");
        assert_eq!(candidates[1].source, CandidateSource::CloudSuggestion);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn linear_sentence_dedup_scans_existing_rows_without_allocation() {
        let candidates = vec![dictionary_row("nihao", "你好", 1, "ni'hao")];
        let block = vec![dictionary_row("nihao", "拟好", 1, "ni'hao")];
        let (found, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            sentence_seen_linear(&candidates, &block, "拟好")
        });

        assert!(found);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn whole_sentence_insert_position_compares_syllables_without_allocating() {
        let candidates = [dictionary_row("nihao", "你好", 1, "ni'hao")];
        let syllables = syllables("ni'hao");
        let (position, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            whole_sentence_insert_position(&candidates, &syllables)
        });

        assert_eq!(position, 1);
        assert_eq!(allocations, 0, "整句插入位置拼接了临时读音键");
    }

    #[test]
    fn emit_caps_what_reaches_the_list() {
        let directory = tempfile::tempdir().unwrap();
        let rows = table(&[
            ("shu'ru", &[("输入", 20000)]),
            ("fa", &[("法", 800000), ("发", 900000)]),
        ]);
        let trigram = Arc::new(triple_table(
            directory.path(),
            "triple.bin",
            &[(SENTENCE_START, "输入", "法", 4.0)],
        ));
        let mut options = LatticeOptions {
            trigram: Some(trigram),
            nbest: 6,
            emit: 1,
            ..LatticeOptions::default()
        };
        let mut candidates = Vec::new();
        merge(&mut candidates, "shu'ru'fa", &rows, "shurufa", &options);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].word, "输入法");

        options.emit = 0;
        let mut every = Vec::new();
        merge(&mut every, "shu'ru'fa", &rows, "shurufa", &options);
        assert!(every.len() > 1);
        assert_eq!(every[0].word, "输入法");
        assert!(every
            .iter()
            .all(|item| item.source == CandidateSource::Generated && item.sentence_association));
    }

    #[test]
    fn weight_is_truncated_log_prob() {
        let rows = table(&[("ni", &[("你", 500_000)]), ("hao", &[("好", 250_000)])]);
        let mut candidates = Vec::new();
        let options = LatticeOptions::default();
        merge(&mut candidates, "ni'hao", &rows, "nihao", &options);
        let expected = (super::super::decode::edge_log_prob(500_000, 1, &options)
            + super::super::decode::edge_log_prob(250_000, 1, &options))
            * 1000.0;
        assert_eq!(candidates[0].weight, expected as i64);
    }

    #[test]
    fn lattice_rows_carry_their_words() {
        let rows = table(&[
            ("wo", &[("我", 900000)]),
            ("xiang", &[("想", 450000), ("翔", 100000)]),
            ("qu", &[("去", 400000), ("区", 100000)]),
        ]);
        let mut model = PersonalNgram::default();
        record_run(&mut model, &["我", "想", "区"], 2);
        let options = LatticeOptions {
            personal: Some(&model),
            nbest: 16,
            ..LatticeOptions::default()
        };
        let paths = super::super::decode::decode_sentences(
            &syllables("wo'xiang'qu"),
            &mut lookup(&rows),
            &options,
        );
        let mut candidates = Vec::new();
        merge(
            &mut candidates,
            "wo'xiang'qu",
            &rows,
            "wo'xiang'qu",
            &options,
        );
        assert_eq!(candidates[0].word, paths[0].sentence);
        assert_eq!(candidates[0].sentence_words, paths[0].words);
        assert_eq!(candidates.len(), paths.len());
    }

    #[test]
    fn insert_position_counts_exact_leading_rows() {
        let keys = syllables("ni'hao");
        let rows = vec![
            dictionary_row("nihao", "你好", 10, "ni'hao"),
            WordItem::new("nihao", "拟好", 9, CandidateSource::UserDatabase, ""),
            dictionary_row("nihao", "你好吗", 8, "ni'hao'ma"),
            dictionary_row("nihao", "你", 7, "ni"),
        ];
        assert_eq!(whole_sentence_insert_position(&rows, &keys), 2);
        assert_eq!(whole_sentence_insert_position(&rows, &[]), 0);
        assert_eq!(whole_sentence_insert_position(&[], &keys), 0);
        let online = [WordItem::new(
            "nihao",
            "你好",
            1,
            CandidateSource::CloudSuggestion,
            "ni'hao",
        )];
        assert_eq!(whole_sentence_insert_position(&online, &keys), 0);
    }

    #[test]
    fn covers_all_syllables_reads_the_key_too() {
        let item = dictionary_row("xian", "西安", 1, "xi'an");
        assert!(covers_all_syllables(&item, 2));
        let item = dictionary_row("xian", "先", 1, "xi'an");
        assert!(covers_all_syllables(&item, 2));
        assert!(!covers_all_syllables(&item, 0));
    }

    fn typo_rows() -> Table {
        table(&[
            ("ta", &[("他", 900000)]),
            ("shi", &[("是", 900000)]),
            ("jian", &[("见", 500000)]),
            ("shi'jian", &[("时间", 30000)]),
        ])
    }

    fn typo_edge() -> TypoEdge {
        TypoEdge {
            start: 1,
            end: 3,
            key: "shi'jian".into(),
            value: "事件".into(),
            weight: 90000,
            penalty: 0.5,
        }
    }

    #[test]
    fn typo_sentence_is_returned_not_merged() {
        let rows = typo_rows();
        let mut seen = Vec::new();
        let mut source = |best: &SentencePath| {
            seen.push(best.sentence.clone());
            vec![typo_edge()]
        };
        let mut candidates = Vec::new();
        let typo = merge_lattice_candidates(
            &mut candidates,
            &syllables("ta'shi'jian"),
            &mut lookup(&rows),
            "tashijian",
            &LatticeOptions::default(),
            Some(&mut source),
        )
        .expect("a typo sentence");
        assert_eq!(seen, ["他时间"]);
        assert_eq!(typo.sentence, "他事件");
        assert!(candidates.iter().all(|item| item.word != "他事件"));
        assert_eq!(candidates[0].word, "他时间");
    }

    #[test]
    fn typo_decode_supports_two_syllables_and_mixed_segments() {
        let rows = table(&[("shi", &[("是", 900000)]), ("jian", &[("见", 500000)])]);
        let mut called = false;
        let mut source = |_: &SentencePath| {
            called = true;
            vec![TypoEdge {
                start: 0,
                end: 2,
                key: "shi'jian".into(),
                value: "时间".into(),
                weight: 500_000,
                penalty: 2.0,
            }]
        };
        let mut candidates = Vec::new();
        let typo = merge_lattice_candidates(
            &mut candidates,
            &syllables("shi'jian"),
            &mut lookup(&rows),
            "shijian",
            &LatticeOptions::default(),
            Some(&mut source),
        );
        assert!(called, "双音节输入应触发纠错边规划");
        assert_eq!(typo.expect("应产出双音节纠错句").sentence, "时间");
        assert_eq!(candidates.len(), 1);

        let mixed_rows = table(&[
            ("w", &[("我", 5_000_000)]),
            ("xiang'qu", &[("想去", 50_000)]),
        ]);
        let mut mixed_lookup = |span: &[String]| {
            mixed_rows
                .get(&span.join("'"))
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut r| {
                    if r.key == "w" {
                        r.key = "wo".into();
                    }
                    r
                })
                .collect()
        };
        let mixed = decode_lattice_mixed(
            &syllables("w'xiang'qu"),
            &mut mixed_lookup,
            &LatticeOptions::default(),
            None,
        )
        .expect("简全混拼应进入词网格");
        assert_eq!(mixed.paths[0].sentence, "我想去");
        assert_eq!(mixed.paths[0].key, "wo'xiang'qu");
        let mixed_dict = [dictionary_row("wxiangqu", "我想去", 10, "wo'xiang'qu")];
        assert_eq!(
            whole_sentence_insert_position(&mixed_dict, &syllables("w'xiang'qu")),
            1
        );
        let many_two: Vec<WordItem> = (0..20)
            .map(|i| dictionary_row("geshi", &format!("词{i}"), 100 - i as i64, "ge'shi"))
            .collect();
        assert_eq!(
            whole_sentence_insert_position(&many_two, &syllables("ge'shi")),
            MAX_LEADING_EXACT_TWO_SYLLABLE_DICT
        );
    }
}

#[cfg(test)]
#[path = "merge_collocation_tests.rs"]
mod collocation_tests;
