//! 码表无关句内中英同轨混输（Modeless Inline Code-Switching）解码器：
//! 当纯小写全拼整段无法完整切分时，允许在句子中切分出 1～2 个高置信英文词槽位，
//! 剩余中文段严格使用全拼音节图与只读 [`PinyinDatabase`] + [`SentenceNgramTables`] 解码，
//! 输出无额外空格的紧凑混输候选，上屏时由宿主按 `chinese_english_spacing` 偏好自动排版。

use std::collections::{HashMap, HashSet};

use crate::assets;
use crate::dictionary::english::EnglishDictionary;
use crate::dictionary::pinyin::PinyinDatabase;
use crate::lattice::decode::{decode_sentences, edge_log_prob, SentenceNgramTables};
use crate::lattice::SentencePath;
use crate::paths::RuntimePaths;
use crate::pinyin::graph::{build_syllable_graph, enumerate_kbest_complete_segmentations};
use crate::pinyin::segment::is_complete_pinyin_input;
use crate::pinyin::syllables::{canonical_lattice_syllable, is_intact};
use crate::types::{CandidateSource, WordItem};

/// 触发句内混输的输入字节数下限。
pub(crate) const MIN_CODE_SWITCH_INPUT_LEN: usize = 5;
/// 触发句内混输的输入字节数上限，防止超长键入导致组合爆炸。
pub(crate) const MAX_CODE_SWITCH_INPUT_LEN: usize = 40;
/// 单个英文词槽位的最大字符长度。
const MAX_ENGLISH_SLOT_LEN: usize = 24;
/// 最大全拼音节字节长度（如 `shuang`/`chuang`）。
const MAX_SYLLABLE_BYTES: usize = 6;
/// 每个中文子段从音节图保留的最佳切分路径上限。
const CHINESE_SPAN_SEG_LIMIT: usize = 3;
/// 最多向混输层返回的不重复最优候选数。
const MAX_CODE_SWITCH_CANDIDATES: usize = 2;
/// 次优混输方案相对最优方案允许的最大分差（nat），避免「吗cos」「哦社」等明显劣质切分污染首页槽位。
const ALT_CANDIDATE_MAX_GAP_NATS: f64 = 4.5;
/// 来自随包英文词库的非规范词最低权重门槛，过滤生僻冷僻缩略词噪声。
const MIN_NON_CANONICAL_DB_WEIGHT: i64 = 500_000;
/// 每次增加一个英文切换槽位的先验惩罚（nat），天然偏好最小必要切换数并抑制真中文短词被误切。
const CODE_SWITCH_SLOT_PENALTY: f64 = 16.0;
/// 规范开发者词表项的置信加成（nat）。
const CANONICAL_TERM_BONUS: f64 = 6.0;
/// 两字母高置信技术缩写白名单；明确排除 `an`/`in`/`he`/`me`/`to`/`do`/`so`/`no` 等英文常见介词与代词。
const SHORT_TECH_ENGLISH_ALLOWLIST: &[&str] = &[
    "ai", "cd", "ci", "db", "go", "id", "io", "js", "ml", "mr", "os", "pr", "py", "ts", "ui", "ux",
];
/// 英文高频介词/代词停用表，在 2 字母混输边界上绝不抢占中文。
const SHORT_STOP_PREPOSITIONS: &[&str] = &["an", "do", "he", "in", "me", "no", "so", "to"];

struct CodeSwitchResources {
    pinyin_db: PinyinDatabase,
    tables: SentenceNgramTables,
}

/// 句内中英同轨混输解码器，按需懒加载只读拼音词库与 N-gram 表。
#[derive(Default)]
pub(crate) struct CodeSwitchDecoder {
    resources: Option<CodeSwitchResources>,
}

#[derive(Clone)]
struct EnglishSpanMatch {
    start: usize,
    end: usize,
    display: String,
    score: f64,
    canonical: bool,
}

#[derive(Clone)]
struct ChineseSpanDecode {
    text: String,
    log_prob: f64,
}

impl CodeSwitchDecoder {
    /// 当 `prefix` 满足无模式句内混输守卫条件时生成最多 `MAX_CODE_SWITCH_CANDIDATES` 条候选。
    /// 若整段本身能完整拼读、已经是合法整词英文、或主拼音词库不可用，则静默返回空列表。
    pub(crate) fn generate(
        &mut self,
        prefix: &str,
        paths: &RuntimePaths,
        english: &EnglishDictionary,
    ) -> Vec<WordItem> {
        let len = prefix.len();
        if !(MIN_CODE_SWITCH_INPUT_LEN..=MAX_CODE_SWITCH_INPUT_LEN).contains(&len)
            || !prefix.bytes().all(|byte| byte.is_ascii_lowercase())
            || is_complete_pinyin_input(prefix)
        {
            return Vec::new();
        }
        if !english.ready() || english.query_exact(prefix).is_some() {
            return Vec::new();
        }
        let resources = self.resources.get_or_insert_with(|| {
            let pinyin_db =
                PinyinDatabase::open_readonly(&paths.dictionary(assets::MAIN_DICTIONARY));
            let tables = if pinyin_db.is_open() {
                SentenceNgramTables::load(paths)
            } else {
                SentenceNgramTables::default()
            };
            CodeSwitchResources { pinyin_db, tables }
        });
        if !resources.pinyin_db.is_open() {
            return Vec::new();
        }

        // 预先在栈上计算所有子区间 `[i, j)` 是否能由完整全拼音节覆盖，`O(n^2)` 且零堆分配。
        let stride = MAX_CODE_SWITCH_INPUT_LEN + 1;
        let mut can_zh = [false; (MAX_CODE_SWITCH_INPUT_LEN + 1) * (MAX_CODE_SWITCH_INPUT_LEN + 1)];
        for i in 0..=len {
            can_zh[i * stride + i] = true;
        }
        for i in 0..len {
            for k in i..len {
                if !can_zh[i * stride + k] {
                    continue;
                }
                let max_end = (k + MAX_SYLLABLE_BYTES).min(len);
                for end in (k + 1)..=max_end {
                    if is_intact(&prefix[k..end]) {
                        can_zh[i * stride + end] = true;
                    }
                }
            }
        }
        let has_zh = |l: usize, r: usize| can_zh[l * stride + r];

        let mut zh_cache: HashMap<(usize, usize), Option<ChineseSpanDecode>> = HashMap::new();
        let mut en_candidates: Vec<EnglishSpanMatch> = Vec::new();
        let mut middle_left_starts = vec![0usize];
        for s in 1..len {
            if has_zh(0, s) {
                middle_left_starts.push(s);
            }
        }
        let mut suffix_ends = vec![len];
        for e in (1..len).rev() {
            if has_zh(e, len) {
                suffix_ends.push(e);
            }
        }

        // 仅检查左端能接续全拼、或右端能接续全拼、或本身处于双槽候选窗口内的英文子串，剪除绝大多数无效区间。
        for s in 0..len {
            let left_ok = has_zh(0, s);
            let max_end = (s + MAX_ENGLISH_SLOT_LEN).min(len);
            for e in (s + 2)..=max_end {
                if s == 0 && e == len {
                    continue;
                }
                let right_ok = has_zh(e, len);
                let possible_slot2 = right_ok
                    && middle_left_starts
                        .iter()
                        .any(|&s1| s1 < s && (s1 + 2..=s).any(|e1| has_zh(e1, s)));
                let possible_slot1 = left_ok
                    && suffix_ends
                        .iter()
                        .any(|&e2| e2 > e && (e..e2.saturating_sub(1)).any(|s2| has_zh(e, s2)));
                if !(left_ok && right_ok) && !possible_slot1 && !possible_slot2 {
                    continue;
                }
                let slice = &prefix[s..e];
                if let Some(matched) =
                    match_english_span(slice, s, e, english, resources, &mut zh_cache, prefix)
                {
                    en_candidates.push(matched);
                }
            }
        }

        if en_candidates.is_empty() {
            return Vec::new();
        }

        let mut scored_plans: Vec<(f64, String)> = Vec::new();

        // 1) 单英文槽位：`[zh: 0..s] + [en: s..e] + [zh: e..len]`（至少一侧中文非空）。
        for en in &en_candidates {
            if (en.start == 0 && en.end == len) || !has_zh(0, en.start) || !has_zh(en.end, len) {
                continue;
            }
            let Some(left) = decode_optional_zh(0, en.start, prefix, resources, &mut zh_cache)
            else {
                continue;
            };
            let Some(right) = decode_optional_zh(en.end, len, prefix, resources, &mut zh_cache)
            else {
                continue;
            };
            let score = left.log_prob + right.log_prob + en.score - CODE_SWITCH_SLOT_PENALTY;
            let mut text =
                String::with_capacity(left.text.len() + en.display.len() + right.text.len());
            text.push_str(&left.text);
            text.push_str(&en.display);
            text.push_str(&right.text);
            scored_plans.push((score, text));
        }

        // 2) 双英文槽位：`[zh: 0..s1] + [en1] + [zh: e1..s2] + [en2] + [zh: e2..len]`。
        for (i, en1) in en_candidates.iter().enumerate() {
            if !has_zh(0, en1.start) {
                continue;
            }
            let Some(left) = decode_optional_zh(0, en1.start, prefix, resources, &mut zh_cache)
            else {
                continue;
            };
            for (j, en2) in en_candidates.iter().enumerate() {
                if i == j || en2.start < en1.end || !has_zh(en2.end, len) {
                    continue;
                }
                // 至少保留 1 个非空中文拼音段；且不把单个已存在的连续英文词拆成两半。
                let has_any_chinese = en1.start > 0 || en1.end < en2.start || en2.end < len;
                if !has_any_chinese {
                    continue;
                }
                if en1.end == en2.start {
                    if !(en1.canonical && en2.canonical)
                        || english.query_exact(&prefix[en1.start..en2.end]).is_some()
                    {
                        continue;
                    }
                } else if !has_zh(en1.end, en2.start) {
                    continue;
                }
                let Some(mid) =
                    decode_optional_zh(en1.end, en2.start, prefix, resources, &mut zh_cache)
                else {
                    continue;
                };
                let Some(right) =
                    decode_optional_zh(en2.end, len, prefix, resources, &mut zh_cache)
                else {
                    continue;
                };
                let score = left.log_prob + mid.log_prob + right.log_prob + en1.score + en2.score
                    - 2.0 * CODE_SWITCH_SLOT_PENALTY;
                let mut text = String::with_capacity(
                    left.text.len()
                        + en1.display.len()
                        + mid.text.len()
                        + en2.display.len()
                        + right.text.len(),
                );
                text.push_str(&left.text);
                text.push_str(&en1.display);
                text.push_str(&mid.text);
                text.push_str(&en2.display);
                text.push_str(&right.text);
                scored_plans.push((score, text));
            }
        }

        if scored_plans.is_empty() {
            return Vec::new();
        }
        scored_plans.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let mut seen = HashSet::new();
        let mut results = Vec::new();
        let best_score = scored_plans[0].0;
        for (idx, (score, text)) in scored_plans.into_iter().enumerate() {
            if idx > 0 && best_score - score > ALT_CANDIDATE_MAX_GAP_NATS {
                break;
            }
            if seen.insert(text.clone()) {
                let synthetic_weight = 1_000_000_i64.saturating_sub((idx as i64) * 100);
                results.push(WordItem::new(
                    prefix,
                    text,
                    synthetic_weight,
                    CandidateSource::Generated,
                    "",
                ));
                if results.len() >= MAX_CODE_SWITCH_CANDIDATES {
                    break;
                }
            }
        }
        results
    }
}

fn resolve_english_hit(slice: &str, english: &EnglishDictionary) -> Option<(String, i64, bool)> {
    let canonical = EnglishDictionary::canonical_term(slice);
    let db_hit = english.query_exact(slice);
    match (canonical, db_hit) {
        (Some((disp, w)), Some(row)) => {
            if row.word == disp {
                Some((row.word.to_string(), row.weight.max(w), true))
            } else if w >= row.weight {
                Some((disp.to_string(), w, true))
            } else {
                Some((row.word.to_string(), row.weight, true))
            }
        }
        (Some((disp, w)), None) => Some((disp.to_string(), w, true)),
        (None, Some(row)) if row.weight >= MIN_NON_CANONICAL_DB_WEIGHT => {
            Some((row.word.to_string(), row.weight, false))
        }
        _ => None,
    }
}

fn match_english_span(
    slice: &str,
    start: usize,
    end: usize,
    english: &EnglishDictionary,
    resources: &CodeSwitchResources,
    zh_cache: &mut HashMap<(usize, usize), Option<ChineseSpanDecode>>,
    full_prefix: &str,
) -> Option<EnglishSpanMatch> {
    if slice.len() == 2
        && (SHORT_STOP_PREPOSITIONS.contains(&slice)
            || !SHORT_TECH_ENGLISH_ALLOWLIST.contains(&slice))
    {
        return None;
    }

    let (display, weight, is_canonical) = resolve_english_hit(slice, english)?;

    if slice.len() == 2 {
        let pinyin_penalty = if is_intact(slice) { 12.0 } else { 0.0 };
        let bonus = if is_canonical {
            CANONICAL_TERM_BONUS
        } else {
            0.0
        };
        let score = (weight.max(1) as f64).ln() - 25.0 + bonus + 1.5 * (slice.len() as f64)
            - pinyin_penalty;
        return Some(EnglishSpanMatch {
            start,
            end,
            display,
            score,
            canonical: is_canonical,
        });
    }

    // 非规范英文短词（如 `you`/`man`/`can`/`she`/`long`/`run`/`mai`/`ren`）若自身就是单音节合法全拼，极易抢走中文边界，直接过滤。
    if !is_canonical && slice.len() <= 4 && is_intact(slice) {
        return None;
    }

    // 非规范英文长词若自身还能拆为完整汉语多字词（如 `women` -> `我们`），优先留给中文。
    let mut pinyin_penalty = 0.0;
    if is_complete_pinyin_input(slice) {
        if !is_canonical {
            if let Some(decoded) = decode_chinese_span(start, end, full_prefix, resources, zh_cache)
            {
                let has_phrase = decoded.words_are_multi_char_or_short_pair(slice);
                if has_phrase {
                    return None;
                }
            }
            pinyin_penalty = 18.0;
        } else {
            pinyin_penalty = 6.0;
        }
    }

    let bonus = if is_canonical {
        CANONICAL_TERM_BONUS
    } else {
        0.0
    };
    let score =
        (weight.max(1) as f64).ln() - 25.0 + bonus + 1.5 * (slice.len() as f64) - pinyin_penalty;
    Some(EnglishSpanMatch {
        start,
        end,
        display,
        score,
        canonical: is_canonical,
    })
}

impl ChineseSpanDecode {
    fn empty() -> Self {
        Self {
            text: String::new(),
            log_prob: 0.0,
        }
    }
}

trait MultiCharCheck {
    fn words_are_multi_char_or_short_pair(&self, slice: &str) -> bool;
}

impl MultiCharCheck for ChineseSpanDecode {
    fn words_are_multi_char_or_short_pair(&self, slice: &str) -> bool {
        let char_count = self.text.chars().count();
        char_count <= 2 || is_intact(slice)
    }
}

fn decode_optional_zh(
    start: usize,
    end: usize,
    prefix: &str,
    resources: &CodeSwitchResources,
    cache: &mut HashMap<(usize, usize), Option<ChineseSpanDecode>>,
) -> Option<ChineseSpanDecode> {
    if start == end {
        return Some(ChineseSpanDecode::empty());
    }
    decode_chinese_span(start, end, prefix, resources, cache)
}

fn decode_chinese_span(
    start: usize,
    end: usize,
    prefix: &str,
    resources: &CodeSwitchResources,
    cache: &mut HashMap<(usize, usize), Option<ChineseSpanDecode>>,
) -> Option<ChineseSpanDecode> {
    if let Some(cached) = cache.get(&(start, end)) {
        return cached.clone();
    }
    let decoded = decode_chinese_span_uncached(start, &prefix[start..end], resources);
    cache.insert((start, end), decoded.clone());
    decoded
}

fn decode_chinese_span_uncached(
    start: usize,
    span: &str,
    resources: &CodeSwitchResources,
) -> Option<ChineseSpanDecode> {
    if span.is_empty() || !is_complete_pinyin_input(span) {
        return None;
    }
    let graph = build_syllable_graph(span);
    let paths = enumerate_kbest_complete_segmentations(&graph, 2, CHINESE_SPAN_SEG_LIMIT);
    if paths.is_empty() {
        return None;
    }
    let options = resources.tables.options(false);
    let span_limit = options.span_limit;
    let mut best_path: Option<SentencePath> = None;

    for dag_path in paths {
        if !dag_path.is_complete || dag_path.syllables.is_empty() {
            continue;
        }
        let syllables: Vec<String> = dag_path
            .syllables
            .iter()
            .map(|syl| canonical_lattice_syllable(syl).to_string())
            .collect();
        // 非句首（紧跟在英文槽位之后）的孤立单音节（如 `he`、`de`、`zai`）不套用句首 `SENTENCE_START` 二元偏好，
        // 避免把夹在英文之间的 `he` 误译成句首姓氏「何」而非高频连词「和」。
        if start > 0 && syllables.len() == 1 {
            if let Some(top_row) = resources
                .pinyin_db
                .query_lattice_span(&syllables, 1)
                .into_iter()
                .next()
            {
                let candidate = SentencePath {
                    sentence: top_row.value.clone(),
                    key: syllables[0].clone(),
                    log_prob: edge_log_prob(top_row.weight, 1, &options),
                    words: vec![top_row.value],
                    typo_edges: 0,
                };
                let replace = best_path
                    .as_ref()
                    .is_none_or(|current| candidate.log_prob > current.log_prob);
                if replace {
                    best_path = Some(candidate);
                }
                continue;
            }
        }
        let mut lookup =
            |subspan: &[String]| resources.pinyin_db.query_lattice_span(subspan, span_limit);
        let decoded = decode_sentences(&syllables, &mut lookup, &options);
        if let Some(top) = decoded.into_iter().next() {
            let replace = best_path
                .as_ref()
                .is_none_or(|current| top.log_prob > current.log_prob);
            if replace {
                best_path = Some(top);
            }
        }
    }

    best_path.map(|best| ChineseSpanDecode {
        text: best.sentence,
        log_prob: best.log_prob,
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::CodeSwitchDecoder;
    use crate::assets;
    use crate::dictionary::english::EnglishDictionary;
    use crate::paths::RuntimePaths;

    fn setup_fixture_paths() -> (TempDir, RuntimePaths, EnglishDictionary) {
        let dir = tempfile::tempdir().unwrap();
        let eng_path = dir.path().join(assets::ENGLISH_DICTIONARY);
        crate::ensure_english_schema(&eng_path).unwrap();
        let eng_conn = Connection::open(&eng_path).unwrap();
        eng_conn
            .execute_batch(
                "INSERT INTO english_words VALUES
                   ('review', 'review', 339067778),
                   ('view', 'view', 602279334),
                   ('cos', 'cos', 6519725),
                   ('he', 'he', 842847219),
                   ('you', 'you', 1600000000),
                   ('women', 'women', 200000000),
                   ('apple', 'apple', 50551171);",
            )
            .unwrap();
        drop(eng_conn);

        let py_path = dir.path().join(assets::MAIN_DICTIONARY);
        let py_conn = Connection::open(&py_path).unwrap();
        py_conn
            .execute_batch(
                "CREATE TABLE tbl_1_w(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_w VALUES('wo','w','我',90000000);
                 CREATE TABLE tbl_1_s(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_s VALUES('shi','s','是',50000000),('shi','s','使',10000000);
                 CREATE TABLE tbl_1_y(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_y VALUES('yong','y','用',40000000),('you','y','有',35000000);
                 CREATE TABLE tbl_1_h(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_h VALUES('he','h','和',60000000);
                 CREATE TABLE tbl_1_m(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_m VALUES('ma','m','吗',25000000),('men','m','们',30000000);
                 CREATE TABLE tbl_1_b(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_b VALUES('bang','b','帮',25000000);
                 CREATE TABLE tbl_1_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_z VALUES('zhe','z','这',60000000);
                 CREATE TABLE tbl_1_g(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_g VALUES('ge','g','个',70000000);
                 CREATE TABLE tbl_1_r(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_1_r VALUES('re','r','热',5000000);
                 CREATE TABLE tbl_2_s(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_2_s VALUES('shi''yong','sy','使用',40000000);
                 CREATE TABLE tbl_2_b(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_2_b VALUES('bang''wo','bw','帮我',35000000);
                 CREATE TABLE tbl_2_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_2_z VALUES('zhe''ge','zg','这个',55000000);
                 CREATE TABLE tbl_2_w(key TEXT, jp TEXT, value TEXT, weight INTEGER);
                 INSERT INTO tbl_2_w VALUES('wo''men','wm','我们',80000000),('wo''you','wy','我有',20000000);",
            )
            .unwrap();
        drop(py_conn);

        let paths = RuntimePaths {
            dictionaries: dir.path().to_owned(),
            ..RuntimePaths::default()
        };
        let english = EnglishDictionary::open(&eng_path, None, None);
        (dir, paths, english)
    }

    #[test]
    fn generates_single_and_dual_slot_inline_codeswitch_candidates() {
        let (_dir, paths, english) = setup_fixture_paths();
        let mut decoder = CodeSwitchDecoder::default();

        let mac = decoder.generate("woshiyongmacos", &paths, &english);
        assert_eq!(mac.first().map(|w| w.word.as_str()), Some("我使用macOS"));
        assert!(mac[0].canonical_pinyin.is_empty());

        let dual = decoder.generate("woshiyongmacoshegithub", &paths, &english);
        assert_eq!(
            dual.first().map(|w| w.word.as_str()),
            Some("我使用macOS和GitHub")
        );

        let pr = decoder.generate("bangworeviewzhegepr", &paths, &english);
        assert_eq!(
            pr.first().map(|w| w.word.as_str()),
            Some("帮我review这个PR")
        );

        // 不能用 `you`/`women` 抢走 `woyou`/`women`，也不得把纯中文 `woshiyong`/`women` 拆成英文。
        let you_guard = decoder.generate("womenyongmacos", &paths, &english);
        assert_eq!(
            you_guard.first().map(|w| w.word.as_str()),
            Some("我们用macOS")
        );
        let you_guard2 = decoder.generate("woyoumacos", &paths, &english);
        assert_eq!(
            you_guard2.first().map(|w| w.word.as_str()),
            Some("我有macOS")
        );

        // 整词纯英文不误切（`macos` 绝不出 `吗cos`）。
        assert!(decoder.generate("macos", &paths, &english).is_empty());
        assert!(decoder.generate("review", &paths, &english).is_empty());
        // 纯合法全拼零触发。
        assert!(decoder.generate("woshiyong", &paths, &english).is_empty());
        assert!(decoder.generate("women", &paths, &english).is_empty());
    }
}
