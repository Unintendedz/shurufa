//! 词网格合法音节纠错边收集器（quanpin.md §10.5 + Shurufa v2 统一概率纠错信道）。
//!
//! 保留既有 `collect_typo_edges` / `collect_span_typo_edges` 供双拼及兼容分配单测使用，
//! 新增 `collect_probabilistic_typo_edges`：以物理 QWERTY 距离与前后鼻音漏 `g` 真实信道代价（`2.4 ~ 3.6 nats`）
//! 对 2 音节及以上合法输入生成每跨度 Top-3 高置信度 `TypoEdge`。

use std::collections::HashMap;

use crate::cache::FifoCache;
use crate::dictionary::pinyin::PinyinDatabase;
use crate::dictionary::DictRow;
use crate::lattice::{SentencePath, TypoEdge};
use crate::pinyin::syllables::{intact_piece, MAX_SYLLABLE_LENGTH};
use crate::pinyin::typos::{
    autocorrect_bit, base_cost, channel_discounted, channel_typo_enabled, channel_typo_penalty,
    discounted, syllable_typos, SyllableTypo, CHANNEL_TRANSPOSITION_PENALTY,
};
use crate::text::count_utf8_chars;
use crate::types::autocorrect_type;
use crate::user_dictionary::typo_profile::PersonalTypoProfile;

/// Typo edges are looked up for at most this many variant keys per query, most of them cache hits while typing on.
pub const TYPO_KEY_BUDGET: usize = 96;
/// A variant is only looked up as part of a phrase. Replacing a lone character almost never beats the literal reading, and those lookups were the costly half: a syllable's table holds hundreds of rows per key, all read to find the top few. Dropping them halved the lookup time and cost 0.1 point of recall on the typo eval set (QD:37-41).
pub const MIN_TYPO_SPAN_SYLLABLES: usize = 2;
pub const MAX_TYPO_SPAN_SYLLABLES: usize = 3;
pub const TYPO_ROWS_PER_KEY: usize = 4;
/// 统一概率纠错信道在每个 `(start, end)` 词跨度上最多保留的高置信度纠错边数。
pub const PROBABILISTIC_TYPO_EDGES_PER_SPAN: usize = 3;

struct PlannedKey {
    start: usize,
    end: usize,
    key: String,
    penalty: f64,
}

enum WeakPositions {
    Bits(u64),
    Heap(Vec<bool>),
}

impl WeakPositions {
    fn new(length: usize) -> Self {
        if length <= u64::BITS as usize {
            Self::Bits(0)
        } else {
            Self::Heap(vec![false; length])
        }
    }

    fn set(&mut self, position: usize) {
        match self {
            Self::Bits(bits) => *bits |= 1u64 << position,
            Self::Heap(weak) => weak[position] = true,
        }
    }

    fn clear(&mut self) {
        match self {
            Self::Bits(bits) => *bits = 0,
            Self::Heap(weak) => weak.fill(false),
        }
    }

    fn get(&self, position: usize) -> bool {
        match self {
            Self::Bits(bits) => bits & (1u64 << position) != 0,
            Self::Heap(weak) => weak[position],
        }
    }
}

/// 按 `TYPO_KEY_BUDGET` 规划跨度键，通过 `span_cache` 查询并缓存空结果，逐行产生纠错边。
#[cfg(test)]
pub fn collect_typo_edges(
    database: &PinyinDatabase,
    span_cache: &mut FifoCache<String, Vec<DictRow>>,
    profile: &PersonalTypoProfile,
    segments: &[String],
    literal_best: &SentencePath,
    autocorrect_types: u32,
) -> Vec<TypoEdge> {
    let planned = plan_keys(profile, segments, literal_best, autocorrect_types);
    lookup_planned_edges(database, span_cache, &planned)
}

/// 统一概率标度的合法音节纠错边收集器（支持 `segments.len() >= 2`，含 `diannai`/`kuaisi`/`yingai`/`qinkuang`/`penyou`/`yijin`/`meiganxi`）。
/// 信道代价按真实美式 QWERTY 物理距离与前后鼻音漏 `g` 标定在 `2.4 ~ 3.6 nats`，每个词跨度至多保留 Top-3 纠错词边。
pub fn collect_probabilistic_typo_edges(
    database: &PinyinDatabase,
    span_cache: &mut FifoCache<String, Vec<DictRow>>,
    profile: &PersonalTypoProfile,
    segments: &[String],
    literal_best: &SentencePath,
    autocorrect_types: u32,
) -> Vec<TypoEdge> {
    if segments.len() < MIN_TYPO_SPAN_SYLLABLES || autocorrect_types == 0 {
        return Vec::new();
    }
    let planned = plan_probabilistic_keys(profile, segments, literal_best, autocorrect_types);
    if planned.is_empty() {
        return Vec::new();
    }
    let raw_edges = lookup_planned_edges(database, span_cache, &planned);
    if raw_edges.is_empty() {
        return raw_edges;
    }

    // 每个 `(start, end)` 跨度按 `ln(weight) - channel_penalty` 降序去重并截断为 Top-3，兼顾召回与词网格稀疏性。
    let mut by_span: HashMap<(usize, usize), Vec<TypoEdge>> = HashMap::new();
    for edge in raw_edges {
        by_span
            .entry((edge.start, edge.end))
            .or_default()
            .push(edge);
    }
    let mut result = Vec::with_capacity(by_span.len() * PROBABILISTIC_TYPO_EDGES_PER_SPAN);
    let mut spans: Vec<((usize, usize), Vec<TypoEdge>)> = by_span.into_iter().collect();
    spans.sort_by_key(|(span, _)| *span);
    for (_, mut span_edges) in spans {
        span_edges.sort_by(|lhs, rhs| {
            let l_score = (lhs.weight.max(1) as f64).ln() - lhs.penalty;
            let r_score = (rhs.weight.max(1) as f64).ln() - rhs.penalty;
            r_score.total_cmp(&l_score)
        });
        let mut kept = 0usize;
        for idx in 0..span_edges.len() {
            if kept >= PROBABILISTIC_TYPO_EDGES_PER_SPAN {
                break;
            }
            let duplicate = span_edges[..kept]
                .iter()
                .any(|existing| existing.value == span_edges[idx].value);
            if duplicate {
                continue;
            }
            if kept != idx {
                span_edges.swap(kept, idx);
            }
            kept += 1;
        }
        span_edges.truncate(kept);
        result.extend(span_edges);
    }
    result
}

/// 全拼和双拼共用的规划与查词。`typos_at(i)` 给出第 i 个音节可能想打的音节（便宜的类型在前），`accepted(i, typo)` 给出用户接受过这条纠错的次数；全拼按音节拼写生成变体，双拼按当前方案的两键编码生成，之后的弱位置优先、跨度、预算、查词和罚分完全相同。
pub(crate) fn collect_span_typo_edges<'t>(
    database: &PinyinDatabase,
    span_cache: &mut FifoCache<String, Vec<DictRow>>,
    segments: &[String],
    literal_best: &SentencePath,
    autocorrect_types: u32,
    typos_at: impl Fn(usize) -> &'t [SyllableTypo],
    accepted: impl Fn(usize, &SyllableTypo) -> i32,
) -> Vec<TypoEdge> {
    let planned = plan_keys_with(
        segments,
        literal_best,
        autocorrect_types,
        typos_at,
        accepted,
    );
    lookup_planned_edges(database, span_cache, &planned)
}

fn lookup_planned_edges(
    database: &PinyinDatabase,
    span_cache: &mut FifoCache<String, Vec<DictRow>>,
    planned: &[PlannedKey],
) -> Vec<TypoEdge> {
    let mut misses = Vec::new();
    for entry in planned {
        if span_cache.get_ref(&entry.key).is_none() {
            if misses.is_empty() {
                misses = Vec::with_capacity(planned.len());
            }
            misses.push(entry.key.clone());
        }
    }
    if !misses.is_empty() {
        let mut fetched = database.query_exact_keys_per_key(&misses, TYPO_ROWS_PER_KEY);
        for key in misses {
            let key_rows = fetched.remove(&key).unwrap_or_default();
            span_cache.insert(key, key_rows);
        }
    }

    // 纠错键的查词页可能只有一两行；按缓存中的实际行数预留，避免把 96 个键的上限乘进返回缓冲。
    let edge_count = planned
        .iter()
        .filter_map(|entry| span_cache.get_ref(&entry.key))
        .fold(0usize, |count, rows| count.saturating_add(rows.len()));
    let mut edges = Vec::with_capacity(edge_count);
    for entry in planned {
        let Some(found) = span_cache.get_ref(&entry.key) else {
            continue;
        };
        edges.extend(found.iter().map(|row| TypoEdge {
            start: entry.start,
            end: entry.end,
            key: row.key.clone(),
            value: row.value.clone(),
            weight: row.weight,
            penalty: entry.penalty,
        }));
    }
    edges
}

/// 按统一概率信道代价规划弱位置优先的纠错跨度键（支持 2 音节与多音节输入）。
fn plan_probabilistic_keys(
    profile: &PersonalTypoProfile,
    segments: &[String],
    literal_best: &SentencePath,
    autocorrect_types: u32,
) -> Vec<PlannedKey> {
    let n = segments.len();
    let mut weak = WeakPositions::new(n);
    let mut covered = 0;
    for word in &literal_best.words {
        let width = count_utf8_chars(word);
        if width == 0 || covered + width > n {
            covered = n + 1;
            break;
        }
        if width == 1 {
            weak.set(covered);
        }
        covered += width;
    }
    if covered != n {
        weak.clear();
    }
    let positions = (0..n)
        .filter(|&i| weak.get(i))
        .chain((0..n).filter(|&i| !weak.get(i)));

    const PROBABILISTIC_TYPO_KEY_BUDGET: usize = 36;
    // 跨音节 2-to-1 相邻换位信道代价（标准换位 `-2.4` + 跨边界融合微调 `-0.4` = `-2.8 nats`）。
    const MERGE_TRANSPOSITION_PENALTY: f64 = CHANNEL_TRANSPOSITION_PENALTY + 0.40;

    let mut planned: Vec<PlannedKey> = Vec::new();
    // 优先检测相邻合法音节对经单次邻位换位可合并为一个合法音节的情形（如 `jin'ti'na -> jin'tian`、`xi'hu'na -> xi'huan`、`li'na'xi -> lian'xi`），
    // 仅生成至少覆盖 2 个纠错后音节的词组跨度键，避免把孤立合法双音节误改为单字。
    if n >= 3 && autocorrect_types & autocorrect_type::TRANSPOSITION != 0 {
        plan_merge_transposition_keys(segments, MERGE_TRANSPOSITION_PENALTY, &mut planned);
    }

    let mut variants: Vec<(&SyllableTypo, f64)> = Vec::new();
    'positions: for position in positions {
        if planned.len() >= PROBABILISTIC_TYPO_KEY_BUDGET {
            break;
        }
        let typed = &segments[position];
        let typos = syllable_typos(typed);
        variants.clear();
        for typo in typos {
            if !channel_typo_enabled(typed, &typo.syllable, typo.kind, autocorrect_types) {
                continue;
            }
            let accepted = profile.accepted(typed, &typo.syllable);
            let penalty = channel_discounted(
                channel_typo_penalty(typed, &typo.syllable, typo.kind),
                accepted,
            );
            variants.push((typo, penalty));
        }
        // 按折后物理信道代价升序排列，优先尝试前后鼻音漏 `g`、换位与最近邻键。
        variants.sort_by(|lhs, rhs| lhs.1.total_cmp(&rhs.1));
        for &(typo, penalty) in &variants {
            for length in MIN_TYPO_SPAN_SYLLABLES..=MAX_TYPO_SPAN_SYLLABLES.min(n) {
                let first = (position + 1).saturating_sub(length);
                for start in first..=position {
                    if start + length > n {
                        break;
                    }
                    let key = typo_span_key(
                        segments,
                        start,
                        start + length,
                        position - start,
                        &typo.syllable,
                    );
                    if let Some(existing) = planned.iter_mut().find(|entry| entry.key == key) {
                        if penalty < existing.penalty {
                            existing.penalty = penalty;
                        }
                        continue;
                    }
                    if planned.is_empty() {
                        planned = Vec::with_capacity(PROBABILISTIC_TYPO_KEY_BUDGET);
                    }
                    planned.push(PlannedKey {
                        start,
                        end: start + length,
                        key,
                        penalty,
                    });
                    if planned.len() >= PROBABILISTIC_TYPO_KEY_BUDGET {
                        break 'positions;
                    }
                }
            }
        }
    }
    planned
}

/// 扫描相邻音节对 `(segments[i], segments[i+1])`：若拼接后在栈上做 1 次相邻字母对调即成单个合法音节
/// （覆盖全部 `-an/-ian/-uan` 尾位换位，如 `ti'na -> tian`、`ji'na -> jian`、`hu'na -> huan`、`li'na -> lian`），
/// 则向左右扩展成含 2~3 个纠错音节的词组跨度键加入 `planned`。
fn plan_merge_transposition_keys(segments: &[String], penalty: f64, planned: &mut Vec<PlannedKey>) {
    let n = segments.len();
    let mut buf = [0u8; MAX_SYLLABLE_LENGTH];
    for i in 0..n.saturating_sub(1) {
        let left = segments[i].as_bytes();
        let right = segments[i + 1].as_bytes();
        let combined_len = left.len() + right.len();
        if !(3..=MAX_SYLLABLE_LENGTH).contains(&combined_len) {
            continue;
        }
        buf[..left.len()].copy_from_slice(left);
        buf[left.len()..combined_len].copy_from_slice(right);
        let mut merged_syllables: Vec<&'static str> = Vec::new();
        for pos in 0..combined_len - 1 {
            if buf[pos] == buf[pos + 1] {
                continue;
            }
            buf.swap(pos, pos + 1);
            if let Some(syl) = intact_piece(&buf[..combined_len]) {
                if !merged_syllables.contains(&syl) {
                    merged_syllables.push(syl);
                }
            }
            buf.swap(pos, pos + 1);
        }
        for merged in merged_syllables {
            // 向左或向右扩展至少 1 个邻接音节，构造 2~3 个纠错音节的词组键（跨原图 3~4 个音节）。
            for orig_len in 3..=4usize.min(n) {
                let first = (i + 2).saturating_sub(orig_len);
                let last = i.min(n - orig_len);
                for start in first..=last {
                    let end = start + orig_len;
                    let mut key = String::with_capacity(combined_len + orig_len * 4);
                    let mut idx = start;
                    while idx < end {
                        if !key.is_empty() {
                            key.push('\'');
                        }
                        if idx == i {
                            key.push_str(merged);
                            idx += 2;
                        } else {
                            key.push_str(&segments[idx]);
                            idx += 1;
                        }
                    }
                    if let Some(existing) = planned
                        .iter_mut()
                        .find(|entry| entry.start == start && entry.end == end && entry.key == key)
                    {
                        if penalty < existing.penalty {
                            existing.penalty = penalty;
                        }
                        continue;
                    }
                    planned.push(PlannedKey {
                        start,
                        end,
                        key,
                        penalty,
                    });
                }
            }
        }
    }
}

/// 按弱位置优先规划并去重纠错跨度键，最多保留 `TYPO_KEY_BUDGET` 项。
#[cfg(test)]
fn plan_keys(
    profile: &PersonalTypoProfile,
    segments: &[String],
    literal_best: &SentencePath,
    autocorrect_types: u32,
) -> Vec<PlannedKey> {
    plan_keys_with(
        segments,
        literal_best,
        autocorrect_types,
        |position| syllable_typos(&segments[position]),
        |position, typo| profile.accepted(&segments[position], &typo.syllable),
    )
}

fn plan_keys_with<'t>(
    segments: &[String],
    literal_best: &SentencePath,
    autocorrect_types: u32,
    typos_at: impl Fn(usize) -> &'t [SyllableTypo],
    accepted: impl Fn(usize, &SyllableTypo) -> i32,
) -> Vec<PlannedKey> {
    let n = segments.len();
    // A syllable the best literal path spells as a lone character is where a typo most likely broke a phrase, so those positions are tried first. Word widths follow the character counts of the path's words; a path that does not tile the input marks nothing weak.
    let mut weak = WeakPositions::new(n);
    let mut covered = 0;
    for word in &literal_best.words {
        let width = count_utf8_chars(word);
        if width == 0 || covered + width > n {
            covered = n + 1;
            break;
        }
        if width == 1 {
            weak.set(covered);
        }
        covered += width;
    }
    if covered != n {
        weak.clear();
    }
    let positions = (0..n)
        .filter(|&i| weak.get(i))
        .chain((0..n).filter(|&i| !weak.get(i)));

    let mut planned = Vec::new();
    let mut variants = Vec::new();
    'positions: for position in positions {
        if planned.len() >= TYPO_KEY_BUDGET {
            break;
        }
        let typos = typos_at(position);
        variants.clear();
        variants.extend(
            typos
                .iter()
                .filter(|typo| autocorrect_types & autocorrect_bit(typo.kind) != 0)
                .map(|typo| (typo, accepted(position, typo))),
        );
        // Kinds are already cheapest first, so a stable sort on the personal count keeps that as the tie-break.
        variants.sort_by_key(|variant| std::cmp::Reverse(variant.1));
        for &(typo, accepted) in &variants {
            let penalty = discounted(base_cost(typo.kind), accepted);
            for length in MIN_TYPO_SPAN_SYLLABLES..=MAX_TYPO_SPAN_SYLLABLES.min(n) {
                let first = (position + 1).saturating_sub(length);
                for start in first..=position {
                    if start + length > n {
                        break;
                    }
                    let key = typo_span_key(
                        segments,
                        start,
                        start + length,
                        position - start,
                        &typo.syllable,
                    );
                    if planned_key_seen(&planned, &key) {
                        continue;
                    }
                    if planned.is_empty() {
                        planned = Vec::with_capacity(TYPO_KEY_BUDGET);
                    }
                    planned.push(PlannedKey {
                        start,
                        end: start + length,
                        key,
                        penalty,
                    });
                    if planned.len() >= TYPO_KEY_BUDGET {
                        break 'positions;
                    }
                }
            }
        }
    }
    planned
}

fn planned_key_seen(planned: &[PlannedKey], key: &str) -> bool {
    planned.iter().any(|entry| entry.key == key)
}

/// 直接拼接替换后的短音节范围，避免先复制整个范围再改写一个位置。
fn typo_span_key(
    segments: &[String],
    start: usize,
    end: usize,
    replaced: usize,
    replacement: &str,
) -> String {
    debug_assert!(start < end);
    debug_assert!(replaced < end - start);
    let original_bytes = segments[start..end].iter().map(String::len).sum::<usize>();
    let capacity =
        original_bytes - segments[start + replaced].len() + replacement.len() + end - start - 1;
    let mut key = String::with_capacity(capacity);
    for (offset, segment) in segments[start..end].iter().enumerate() {
        if offset > 0 {
            key.push('\'');
        }
        if offset == replaced {
            key.push_str(replacement);
        } else {
            key.push_str(segment);
        }
    }
    key
}

#[cfg(test)]
#[path = "typo_edges/lazy_plan_tests.rs"]
mod lazy_plan_tests;

#[cfg(test)]
#[path = "typo_edges/lazy_buffer_tests.rs"]
mod lazy_buffer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quanpin::fixture::Fixture;
    use crate::types::autocorrect_type;

    #[test]
    fn probabilistic_typo_edges_recover_adjacent_merge_transpositions() {
        let fixture = Fixture::new();
        fixture
            .insert("jin'tian", "今天", 500_000)
            .insert("lian'xi", "联系", 400_000)
            .insert("tian", "天", 2_000_000);
        let db = PinyinDatabase::open(&fixture.database());
        let mut cache = FifoCache::new(64);
        let profile = PersonalTypoProfile::shared(&fixture.journal());
        let literal = SentencePath {
            sentence: "金提拿".to_owned(),
            key: "jin'ti'na".to_owned(),
            log_prob: -25.0,
            words: vec!["金".to_owned(), "提".to_owned(), "拿".to_owned()],
            typo_edges: 0,
        };
        let jintina = ["jin", "ti", "na"].map(str::to_owned);
        let edges = collect_probabilistic_typo_edges(
            &db,
            &mut cache,
            &profile,
            &jintina,
            &literal,
            autocorrect_type::TRANSPOSITION,
        );
        assert!(
            edges
                .iter()
                .any(|e| e.start == 0 && e.end == 3 && e.value == "今天" && e.key == "jin'tian"),
            "{edges:?}"
        );

        let linaxi = ["li", "na", "xi"].map(str::to_owned);
        let edges = collect_probabilistic_typo_edges(
            &db,
            &mut cache,
            &profile,
            &linaxi,
            &literal,
            autocorrect_type::TRANSPOSITION,
        );
        assert!(
            edges
                .iter()
                .any(|e| e.start == 0 && e.end == 3 && e.value == "联系" && e.key == "lian'xi"),
            "{edges:?}"
        );
    }

    #[test]
    fn probabilistic_typo_edges_cover_two_syllable_and_nasal_g_cases_within_budget() {
        let fixture = Fixture::new();
        fixture
            .insert("dian'nao", "电脑", 500_000)
            .insert("kuai'di", "快递", 300_000)
            .insert("ying'gai", "应该", 900_000)
            .insert("qing'kuang", "情况", 400_000)
            .insert("peng'you", "朋友", 500_000)
            .insert("yi'jing", "已经", 800_000)
            .insert("guan'xi", "关系", 400_000);
        let database = PinyinDatabase::open(&fixture.database());
        let profile = PersonalTypoProfile::shared(&fixture.journal());
        let mut cache = FifoCache::new(64);
        let all = autocorrect_type::TRANSPOSITION
            | autocorrect_type::NEIGHBOR
            | autocorrect_type::MISSING_OR_EXTRA;

        for (segs, expected_word) in [
            (&["dian", "nai"][..], "电脑"),
            (&["kuai", "si"][..], "快递"),
            (&["yin", "gai"][..], "应该"),
            (&["qin", "kuang"][..], "情况"),
            (&["pen", "you"][..], "朋友"),
            (&["yi", "jin"][..], "已经"),
            (&["mei", "gan", "xi"][..], "关系"),
        ] {
            let owned: Vec<String> = segs.iter().map(|s| (*s).to_owned()).collect();
            let literal = SentencePath {
                sentence: "甲".repeat(owned.len()),
                key: owned.join("'"),
                log_prob: -20.0,
                words: vec!["甲".to_owned(); owned.len()],
                typo_edges: 0,
            };
            let edges = collect_probabilistic_typo_edges(
                &database, &mut cache, &profile, &owned, &literal, all,
            );
            let hit = edges
                .iter()
                .find(|edge| edge.value == expected_word)
                .unwrap_or_else(|| panic!("{segs:?} 缺少 {expected_word}: {edges:?}"));
            assert!(
                hit.penalty >= 2.0 && hit.penalty <= 3.8,
                "信道代价应在 2.0 ~ 3.8 nats: {}",
                hit.penalty
            );
        }
    }

    #[test]
    fn planning_reuses_the_variant_buffer_across_positions() {
        let directory = tempfile::tempdir().unwrap();
        let profile = PersonalTypoProfile::shared(&directory.path().join("journal.db"));
        let segments = vec!["zhuang".to_owned(); 6];
        let literal_best = SentencePath {
            sentence: "一二三四五六".to_owned(),
            key: "zhuang'zhuang'zhuang'zhuang'zhuang'zhuang".to_owned(),
            log_prob: 0.0,
            words: vec!["一".to_owned(); 6],
            typo_edges: 0,
        };

        let _ = syllable_typos("zhuang");
        let (planned, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            plan_keys(
                &profile,
                &segments,
                &literal_best,
                crate::types::autocorrect_type::TRANSPOSITION
                    | crate::types::autocorrect_type::NEIGHBOR
                    | crate::types::autocorrect_type::MISSING_OR_EXTRA,
            )
        });

        assert_eq!(planned.len(), 20);
        assert!(allocations <= 91, "{allocations}");
    }

    #[test]
    fn weak_position_bits_cover_the_boundary_without_heap_state() {
        let (mut weak, allocations) =
            crate::ime::personal_rerank::allocations::count(|| WeakPositions::new(64));
        weak.set(0);
        weak.set(63);

        assert_eq!(allocations, 0);
        assert!(weak.get(0));
        assert!(weak.get(63));
        assert!(!weak.get(1));
    }

    #[test]
    fn weak_position_storage_falls_back_for_long_internal_inputs() {
        let mut weak = WeakPositions::new(65);
        weak.set(64);
        assert!(weak.get(64));
        weak.clear();
        assert!(!weak.get(64));
    }

    #[test]
    fn planned_key_lookup_scans_owned_keys() {
        let planned = vec![PlannedKey {
            start: 0,
            end: 2,
            key: "ni'hao".to_owned(),
            penalty: 1.0,
        }];
        assert!(planned_key_seen(&planned, "ni'hao"));
        assert!(!planned_key_seen(&planned, "ni'he"));
    }

    #[test]
    fn planned_key_scan_uses_no_temporary_heap_state() {
        let planned = vec![PlannedKey {
            start: 0,
            end: 2,
            key: "ni'hao".to_owned(),
            penalty: 1.0,
        }];
        let (found, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            planned_key_seen(&planned, "ni'hao")
        });

        assert!(found);
        assert_eq!(allocations, 0);
    }

    #[test]
    fn typo_span_key_replaces_one_segment_without_a_temporary_vector() {
        let segments = ["ni".to_owned(), "hao".to_owned(), "ma".to_owned()];

        let (key, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            typo_span_key(&segments, 0, 3, 1, "he")
        });

        assert_eq!(key, "ni'he'ma");
        assert_eq!(key.capacity(), key.len());
        assert_eq!(allocations, 1);
    }

    #[test]
    fn typo_span_keys_match_cloned_ranges_at_every_replacement_position() {
        let segments = [
            "ni".to_owned(),
            "hao".to_owned(),
            "ma".to_owned(),
            "ba".to_owned(),
        ];
        for start in 0..segments.len() {
            for end in start + 1..=segments.len() {
                for replaced in 0..end - start {
                    for replacement in ["", "he", "shang"] {
                        let mut expected = segments[start..end].to_vec();
                        expected[replaced] = replacement.to_owned();
                        let key = typo_span_key(&segments, start, end, replaced, replacement);
                        assert_eq!(key, expected.join("'"));
                        assert_eq!(key.capacity(), key.len());
                    }
                }
            }
        }
    }
}
