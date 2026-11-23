//! 跨语种个人语音词表与别名纠错。
//!
//! 与只处理纯汉字拼音的 [`super::hotwords`] 互补：本模块同时承载英文专有名词、中英混排术语、
//! 代码标识符、文件名与音近别名映射，并提供两条消费路径：
//! 1. [`apply_deterministic`]：用于关闭润色或润色降级时的确定性别名替换与拼音热词纠偏；
//! 2. [`select_prompt_terms`]：按字面/拼音近似度、应用场景与权重筛选 Top-K 术语注入润色 Prompt，避免 Prompt 膨胀。

use super::hotwords::{self, Hotword};
use super::postprocess::AppCategory;
use msime_client_base::bounded_io::read_bounded_file_with;
use msime_client_base::text::{has_disallowed_control_with_options, is_han_character};
use pinyin::ToPinyinMulti;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::path::Path;

/// 个人语音词表默认条数上限。
pub const DEFAULT_VOCABULARY_LIMIT: usize = 300;
/// 个人语音词表硬性条数上限。
pub const MAX_VOCABULARY_LIMIT: usize = 1_000;
/// 注入润色 Prompt 的术语默认上限与硬上限（防 9B 本地模型首字延迟抬升和术语硬凑）。
pub const DEFAULT_PROMPT_TERM_LIMIT: usize = 60;
pub const MAX_PROMPT_TERM_LIMIT: usize = 60;
/// 偏好目录下自定义语音词表文件名。
pub const VOCABULARY_FILE_NAME: &str = "voice-vocabulary.json";
/// 自定义语音词表文件只读上限（1 MiB）。
pub const MAX_VOCABULARY_FILE_BYTES: u64 = 1 << 20;
/// 单条术语规范词或别名的最大字节长度。
const MAX_TERM_BYTES: usize = 128;
/// 单条快捷短语并入语音词表时的最大字符数（过滤整段模板）。
const MAX_QUICK_PHRASE_CHARS: usize = 32;

fn default_vocabulary_version() -> u32 {
    1
}

/// 单条跨语种语音术语，支持规范词、音近/错写别名、无声调拼音、场景分类与权重。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabularyTerm {
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub pinyin: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub weight: i64,
}

/// 跨语种个人语音词表文档，与 `<preferences_directory>/voice-vocabulary.json` 结构一致。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceVocabulary {
    #[serde(default = "default_vocabulary_version")]
    pub version: u32,
    #[serde(default)]
    pub terms: Vec<VocabularyTerm>,
}

impl Default for VoiceVocabulary {
    fn default() -> Self {
        Self {
            version: default_vocabulary_version(),
            terms: Vec::new(),
        }
    }
}

impl VoiceVocabulary {
    /// 由词条列表构造词表文档。
    pub fn new(terms: Vec<VocabularyTerm>) -> Self {
        Self {
            version: default_vocabulary_version(),
            terms,
        }
    }

    /// 词表是否为空。
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// 从拼音/英文/快捷短语词库分页读出的原始词条视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryEntryInput {
    pub key: String,
    pub value: String,
    pub weight: i64,
}

fn io_read_error(error: io::Error) -> String {
    format!("failed to read voice vocabulary: {error}")
}

/// 解析并规范化 `voice-vocabulary.json` 文本内容。
pub fn parse_vocabulary_json(content: &str) -> Result<VoiceVocabulary, String> {
    let parsed: VoiceVocabulary =
        serde_json::from_str(content).map_err(|_| "invalid voice vocabulary json".to_owned())?;
    Ok(VoiceVocabulary {
        version: if parsed.version == 0 {
            default_vocabulary_version()
        } else {
            parsed.version
        },
        terms: sanitize_terms(parsed.terms, MAX_VOCABULARY_LIMIT),
    })
}

/// 只读加载指定路径的自定义语音词表文件；文件不存在时视为空词表。
///
/// 采用普通有界原子读取，不加文件锁，满足跨平台文件锁门禁约束。
pub fn load_vocabulary_file(path: &Path) -> Result<VoiceVocabulary, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(VoiceVocabulary::default());
        }
        Err(error) => return Err(io_read_error(error)),
    };
    let bytes = read_bounded_file_with(
        file,
        MAX_VOCABULARY_FILE_BYTES,
        || "voice vocabulary file exceeds size limit".to_owned(),
        io_read_error,
    )?;
    let content =
        String::from_utf8(bytes).map_err(|_| "voice vocabulary file is not valid utf-8")?;
    parse_vocabulary_json(&content)
}

/// 只读加载 `<preferences_directory>/voice-vocabulary.json`；文件不存在时视为空词表。
pub fn load_from_preferences_dir(preferences_dir: &Path) -> Result<VoiceVocabulary, String> {
    load_vocabulary_file(&preferences_dir.join(VOCABULARY_FILE_NAME))
}

/// 融合自定义语音词表、用户拼音词库、英文词库与快捷短语，去重并按优先级排序截断。
///
/// 自定义 `voice-vocabulary.json` 享有最高合并优先级，同词形（忽略 ASCII 大小写）合并时保留自定义规范写法与别名。
pub fn build_vocabulary(
    custom: Option<&VoiceVocabulary>,
    pinyin_entries: &[DictionaryEntryInput],
    english_entries: &[DictionaryEntryInput],
    quick_phrase_entries: &[DictionaryEntryInput],
    limit: usize,
) -> VoiceVocabulary {
    let cap = limit.min(MAX_VOCABULARY_LIMIT);
    if cap == 0 {
        return VoiceVocabulary::default();
    }

    // 键为去首尾空白并转小写的规范词，值为 `(是否来自自定义文件, 词条)`。
    let mut merged: HashMap<String, (bool, VocabularyTerm)> = HashMap::new();

    if let Some(custom_vocab) = custom {
        for raw_term in sanitize_terms(custom_vocab.terms.clone(), MAX_VOCABULARY_LIMIT) {
            let key = raw_term.canonical.to_ascii_lowercase();
            merged
                .entry(key)
                .and_modify(|(is_custom, existing)| {
                    *is_custom = true;
                    existing.weight = existing.weight.max(raw_term.weight);
                    if existing.pinyin.is_empty() && !raw_term.pinyin.is_empty() {
                        existing.pinyin = raw_term.pinyin.clone();
                    }
                    if existing.category.is_empty() && !raw_term.category.is_empty() {
                        existing.category = raw_term.category.clone();
                    }
                    merge_aliases(
                        &mut existing.aliases,
                        &raw_term.aliases,
                        &existing.canonical,
                    );
                })
                .or_insert((true, raw_term));
        }
    }

    // 用户拼音词库：复用 `hotwords` 的汉字筛选与拼音规整逻辑，保留原始权重。
    let mut pinyin_weights: HashMap<String, i64> = HashMap::new();
    for entry in pinyin_entries {
        let value = entry.value.trim();
        if !value.is_empty() {
            pinyin_weights
                .entry(value.to_owned())
                .and_modify(|weight| *weight = (*weight).max(entry.weight))
                .or_insert(entry.weight);
        }
    }
    let normalized_hotwords = hotwords::hotwords_from_entries(
        pinyin_entries
            .iter()
            .map(|entry| (entry.value.as_str(), entry.key.as_str())),
        MAX_VOCABULARY_LIMIT,
    );
    for hotword in normalized_hotwords {
        let weight = pinyin_weights.get(&hotword.text).copied().unwrap_or(0);
        let key = hotword.text.to_ascii_lowercase();
        merged
            .entry(key)
            .and_modify(|(_, existing)| {
                if existing.pinyin.is_empty() {
                    existing.pinyin = hotword.pinyin.clone();
                }
                existing.weight = existing.weight.max(weight);
            })
            .or_insert_with(|| {
                (
                    false,
                    VocabularyTerm {
                        canonical: hotword.text,
                        aliases: Vec::new(),
                        pinyin: hotword.pinyin,
                        category: "pinyin".to_owned(),
                        weight,
                    },
                )
            });
    }

    // 用户英文词库：保留至少 2 字符的有效英文/专有词形。
    for entry in english_entries {
        let Some(canonical) = clean_single_line_token(&entry.value) else {
            continue;
        };
        if canonical.chars().count() < 2 {
            continue;
        }
        let key = canonical.to_ascii_lowercase();
        merged
            .entry(key)
            .and_modify(|(_, existing)| {
                existing.weight = existing.weight.max(entry.weight);
            })
            .or_insert_with(|| {
                (
                    false,
                    VocabularyTerm {
                        canonical,
                        aliases: Vec::new(),
                        pinyin: String::new(),
                        category: "english".to_owned(),
                        weight: entry.weight,
                    },
                )
            });
    }

    // 快捷短语：仅吸纳简短单行术语（2..=32 字符），跳过长模板和整段文本。
    for entry in quick_phrase_entries {
        let Some(canonical) = clean_single_line_token(&entry.value) else {
            continue;
        };
        let char_count = canonical.chars().count();
        if !(2..=MAX_QUICK_PHRASE_CHARS).contains(&char_count) {
            continue;
        }
        let derived_pinyin = derive_han_pinyin(&canonical).unwrap_or_default();
        let key = canonical.to_ascii_lowercase();
        merged
            .entry(key)
            .and_modify(|(_, existing)| {
                existing.weight = existing.weight.max(entry.weight);
                if existing.pinyin.is_empty() && !derived_pinyin.is_empty() {
                    existing.pinyin = derived_pinyin.clone();
                }
            })
            .or_insert_with(|| {
                (
                    false,
                    VocabularyTerm {
                        canonical,
                        aliases: Vec::new(),
                        pinyin: derived_pinyin,
                        category: "quick_phrase".to_owned(),
                        weight: entry.weight,
                    },
                )
            });
    }

    let mut items: Vec<(bool, VocabularyTerm)> = merged.into_values().collect();
    items.sort_by(|(custom_a, term_a), (custom_b, term_b)| {
        custom_b
            .cmp(custom_a)
            .then_with(|| term_b.weight.cmp(&term_a.weight))
            .then_with(|| term_a.canonical.cmp(&term_b.canonical))
    });
    items.truncate(cap);
    VoiceVocabulary::new(items.into_iter().map(|(_, term)| term).collect())
}

/// 确定性应用跨语种词表：先按别名与大小写规范做最长非重叠匹配，再调用拼音热词纠偏高权重中文词条。
pub fn apply_deterministic(text: &str, vocab: &VoiceVocabulary) -> String {
    apply_deterministic_terms(text, &vocab.terms)
}

/// 直接基于术语切片执行确定性别名替换与拼音热词纠偏。
pub fn apply_deterministic_terms(text: &str, terms: &[VocabularyTerm]) -> String {
    if text.is_empty() || terms.is_empty() {
        return text.to_owned();
    }
    let after_aliases = apply_alias_and_casing_rules(text, terms);
    apply_pinyin_hotwords(&after_aliases, terms)
}

/// 依据 `raw_text` 字面/编辑距离/拼音近似度、应用场景与权重筛选 Top-K 术语，供润色 Prompt 注入。
///
/// `max_terms` 自动封顶至 [`MAX_PROMPT_TERM_LIMIT`]（60），防止 Prompt 膨胀引发延迟上升与硬凑术语。
pub fn select_prompt_terms(
    raw_text: &str,
    app_category: AppCategory,
    vocab: &VoiceVocabulary,
    max_terms: usize,
) -> Vec<String> {
    let cap = max_terms.min(MAX_PROMPT_TERM_LIMIT);
    if cap == 0 || vocab.terms.is_empty() {
        return Vec::new();
    }

    let raw_lower = raw_text.to_ascii_lowercase();
    let raw_ascii_tokens = extract_ascii_tokens(raw_text);
    let raw_chars: Vec<char> = raw_text.chars().collect();
    let raw_han_set: HashSet<char> = raw_chars
        .iter()
        .copied()
        .filter(|&ch| is_han_character(ch))
        .collect();
    let raw_fuzzy_sounds: Vec<Option<Vec<String>>> = raw_chars
        .iter()
        .map(|&ch| {
            is_han_character(ch).then(|| {
                char_readings(ch)
                    .into_iter()
                    .map(|reading| fuzzy_syllable(&reading))
                    .collect()
            })
        })
        .collect();
    let raw_initials: Vec<Option<Vec<char>>> = raw_fuzzy_sounds
        .iter()
        .map(|sounds| {
            sounds.as_ref().map(|list| {
                let mut initials = Vec::with_capacity(list.len());
                for syllable in list {
                    if let Some(first) = syllable.chars().next() {
                        if !initials.contains(&first) {
                            initials.push(first);
                        }
                    }
                }
                initials
            })
        })
        .collect();

    let mut seen = HashSet::new();
    let mut scored: Vec<(i64, i64, String)> = Vec::with_capacity(vocab.terms.len());

    for term in &vocab.terms {
        let canonical = term.canonical.trim();
        if canonical.is_empty() || !seen.insert(canonical.to_ascii_lowercase()) {
            continue;
        }
        let mut score: i64 = 0;

        // 1. 字面或别名直接命中（最高相关度）。
        let canonical_lower = canonical.to_ascii_lowercase();
        let direct_hit = (!raw_lower.is_empty() && raw_lower.contains(&canonical_lower))
            || term.aliases.iter().any(|alias| {
                let alias_trimmed = alias.trim();
                !alias_trimmed.is_empty() && raw_lower.contains(&alias_trimmed.to_ascii_lowercase())
            });
        if direct_hit {
            score += 1_000;
        }

        // 2. 中文拼音模糊读音、首字母序列与汉字重叠度。
        let canonical_han: Vec<char> = canonical
            .chars()
            .filter(|&ch| is_han_character(ch))
            .collect();
        if canonical_han.len() >= 2 {
            let term_syllables = term_fuzzy_syllables(term);
            if !term_syllables.is_empty() {
                if matches_fuzzy_window(&raw_fuzzy_sounds, &term_syllables) {
                    score += 800;
                } else {
                    let term_initials: Vec<char> = term_syllables
                        .iter()
                        .filter_map(|syllable| syllable.chars().next())
                        .collect();
                    if term_initials.len() >= 2
                        && matches_initial_window(&raw_initials, &term_initials)
                    {
                        score += 350;
                    }
                }
            }
            let shared_han = canonical_han
                .iter()
                .filter(|ch| raw_han_set.contains(ch))
                .count()
                .min(4) as i64;
            score += shared_han * 120;
        }

        // 3. 英文/代码词与原始转写中 ASCII 词元的编辑距离与前缀近似度。
        if !raw_ascii_tokens.is_empty() {
            let mut candidate_tokens = extract_ascii_tokens(canonical);
            for alias in &term.aliases {
                candidate_tokens.extend(extract_ascii_tokens(alias));
            }
            let mut best_latin_score: i64 = 0;
            for cand in &candidate_tokens {
                if cand.len() < 2 {
                    continue;
                }
                for token in &raw_ascii_tokens {
                    if token == cand {
                        best_latin_score = best_latin_score.max(900);
                        break;
                    }
                    let max_len = token.len().max(cand.len());
                    let min_len = token.len().min(cand.len());
                    if max_len <= 48 && min_len >= 3 {
                        let dist = levenshtein(token, cand);
                        let close_enough = (dist == 1 && max_len >= 4)
                            || (dist <= 2 && max_len >= 6 && dist * 3 <= max_len);
                        if close_enough {
                            best_latin_score = best_latin_score.max(550 - (dist as i64) * 80);
                        } else if shared_prefix_len(token, cand) >= 3 {
                            best_latin_score = best_latin_score.max(220);
                        }
                    }
                }
            }
            score += best_latin_score;
        }

        // 4. 应用场景分类加分。
        score += category_affinity_bonus(&term.category, app_category);

        // 5. 词条权重基础分（封顶避免单条权重压垮上下文命中）。
        score += term.weight.clamp(0, 10_000) / 100;

        scored.push((score, term.weight, canonical.to_owned()));
    }

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored.truncate(cap);
    scored.into_iter().map(|(_, _, text)| text).collect()
}

/// 清洗单行术语文本：去首尾空白、拒控制字符与超长词条。
fn clean_single_line_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_TERM_BYTES
        || has_disallowed_control_with_options(trimmed, false)
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn sanitize_terms(terms: Vec<VocabularyTerm>, limit: usize) -> Vec<VocabularyTerm> {
    let mut result = Vec::with_capacity(terms.len().min(limit));
    let mut seen = HashSet::new();
    for term in terms {
        if result.len() >= limit {
            break;
        }
        let Some(canonical) = clean_single_line_token(&term.canonical) else {
            continue;
        };
        let key = canonical.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        let mut aliases = Vec::new();
        merge_aliases(&mut aliases, &term.aliases, &canonical);
        let pinyin = clean_single_line_token(&term.pinyin)
            .or_else(|| derive_han_pinyin(&canonical))
            .unwrap_or_default();
        let category = clean_single_line_token(&term.category)
            .map(|cat| cat.to_ascii_lowercase())
            .unwrap_or_default();
        result.push(VocabularyTerm {
            canonical,
            aliases,
            pinyin,
            category,
            weight: term.weight,
        });
    }
    result
}

fn merge_aliases(target: &mut Vec<String>, incoming: &[String], canonical: &str) {
    let canonical_lower = canonical.to_ascii_lowercase();
    for alias in incoming {
        let Some(cleaned) = clean_single_line_token(alias) else {
            continue;
        };
        let alias_lower = cleaned.to_ascii_lowercase();
        if alias_lower == canonical_lower {
            continue;
        }
        if !target
            .iter()
            .any(|existing| existing.to_ascii_lowercase() == alias_lower)
        {
            target.push(cleaned);
        }
    }
}

struct AliasRule {
    pattern_lower: Vec<char>,
    canonical: String,
    require_left_ascii_boundary: bool,
    require_right_ascii_boundary: bool,
    is_explicit_alias: bool,
    weight: i64,
}

fn is_ascii_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// 判断规范词本身是否具有值得自动恢复的特殊大小写或标识符结构（如 `Shurufa`、`Next.js`、`AGENTS.md`）。
fn canonical_needs_case_restore(canonical: &str) -> bool {
    let chars: Vec<char> = canonical.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    let has_ascii_alpha = chars.iter().any(char::is_ascii_alphabetic);
    if !has_ascii_alpha {
        return false;
    }
    let has_upper = chars.iter().any(char::is_ascii_uppercase);
    let has_ident_punct = chars
        .iter()
        .any(|ch| matches!(ch, '.' | '_' | '-' | '/' | '+'));
    has_upper || has_ident_punct
}

fn apply_alias_and_casing_rules(text: &str, terms: &[VocabularyTerm]) -> String {
    let mut rules: Vec<AliasRule> = Vec::new();
    for term in terms {
        if term.weight < 0 {
            continue;
        }
        let Some(canonical) = clean_single_line_token(&term.canonical) else {
            continue;
        };
        // 规范词自身若有大写/标识结构，加入大小写保真规则（同时防短别名切碎已正确的长规范词）。
        if canonical_needs_case_restore(&canonical) {
            let pat: Vec<char> = canonical
                .chars()
                .map(|ch| ch.to_ascii_lowercase())
                .collect();
            if pat.len() >= 2 {
                let left_bound = pat.first().copied().is_some_and(is_ascii_word_char);
                let right_bound = pat.last().copied().is_some_and(is_ascii_word_char);
                rules.push(AliasRule {
                    pattern_lower: pat,
                    canonical: canonical.clone(),
                    require_left_ascii_boundary: left_bound,
                    require_right_ascii_boundary: right_bound,
                    is_explicit_alias: false,
                    weight: term.weight,
                });
            }
        }
        for alias in &term.aliases {
            let Some(cleaned) = clean_single_line_token(alias) else {
                continue;
            };
            let pat: Vec<char> = cleaned.chars().map(|ch| ch.to_ascii_lowercase()).collect();
            // 中文别名与英文别名均要求长度 >= 2，避免单字误替换。
            if pat.len() < 2 {
                continue;
            }
            let left_bound = pat.first().copied().is_some_and(is_ascii_word_char);
            let right_bound = pat.last().copied().is_some_and(is_ascii_word_char);
            rules.push(AliasRule {
                pattern_lower: pat,
                canonical: canonical.clone(),
                require_left_ascii_boundary: left_bound,
                require_right_ascii_boundary: right_bound,
                is_explicit_alias: true,
                weight: term.weight,
            });
        }
    }
    if rules.is_empty() {
        return text.to_owned();
    }

    // 最长模式优先；同长度时显式别名与高权重优先。
    rules.sort_by(|a, b| {
        b.pattern_lower
            .len()
            .cmp(&a.pattern_lower.len())
            .then_with(|| b.is_explicit_alias.cmp(&a.is_explicit_alias))
            .then_with(|| b.weight.cmp(&a.weight))
    });

    let chars: Vec<char> = text.chars().collect();
    let lower_chars: Vec<char> = chars.iter().map(|ch| ch.to_ascii_lowercase()).collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;

    'outer: while index < chars.len() {
        for rule in &rules {
            let len = rule.pattern_lower.len();
            let end = index + len;
            if end > chars.len() {
                continue;
            }
            if lower_chars[index..end] != rule.pattern_lower[..] {
                continue;
            }
            if rule.require_left_ascii_boundary && index > 0 && is_ascii_word_char(chars[index - 1])
            {
                continue;
            }
            if rule.require_right_ascii_boundary
                && end < chars.len()
                && is_ascii_word_char(chars[end])
            {
                continue;
            }
            out.push_str(&rule.canonical);
            index = end;
            continue 'outer;
        }
        out.push(chars[index]);
        index += 1;
    }

    out
}

/// 将词表中的纯汉字词条按权重降序送入 `hotwords::correct` 执行拼音模糊纠偏。
fn apply_pinyin_hotwords(text: &str, terms: &[VocabularyTerm]) -> String {
    let mut han_terms: Vec<(&VocabularyTerm, i64)> = terms
        .iter()
        .filter(|term| {
            let trimmed = term.canonical.trim();
            term.weight >= 0
                && trimmed.chars().count() >= 2
                && trimmed.chars().all(is_han_character)
        })
        .map(|term| (term, term.weight))
        .collect();
    if han_terms.is_empty() {
        return text.to_owned();
    }
    han_terms.sort_by_key(|a| std::cmp::Reverse(a.1));
    let hotwords: Vec<Hotword> = hotwords::hotwords_from_entries(
        han_terms
            .iter()
            .map(|(term, _)| (term.canonical.as_str(), term.pinyin.as_str())),
        MAX_VOCABULARY_LIMIT,
    );
    if hotwords.is_empty() {
        return text.to_owned();
    }
    hotwords::correct(text, &hotwords)
}

fn derive_han_pinyin(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 || !chars.iter().all(|&ch| is_han_character(ch)) {
        return None;
    }
    let hotwords = hotwords::hotwords_from_entries([(text, "")], 1);
    hotwords.into_iter().next().map(|hw| hw.pinyin)
}

fn char_readings(ch: char) -> Vec<String> {
    let Some(multi) = ch.to_pinyin_multi() else {
        return Vec::new();
    };
    let mut readings = Vec::new();
    for reading in multi {
        let plain = normalize_pinyin_syllable(reading.plain());
        if !plain.is_empty() && !readings.contains(&plain) {
            readings.push(plain);
        }
    }
    readings
}

fn normalize_pinyin_syllable(syllable: &str) -> String {
    let mut out = String::with_capacity(syllable.len());
    for ch in syllable.chars().flat_map(char::to_lowercase) {
        match ch {
            'a'..='z' => out.push(ch),
            'ü' => out.push('v'),
            'ā' | 'á' | 'ǎ' | 'à' => out.push('a'),
            'ē' | 'é' | 'ě' | 'è' => out.push('e'),
            'ī' | 'í' | 'ǐ' | 'ì' => out.push('i'),
            'ō' | 'ó' | 'ǒ' | 'ò' => out.push('o'),
            'ū' | 'ú' | 'ǔ' | 'ù' => out.push('u'),
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => out.push('v'),
            _ => {}
        }
    }
    out
}

/// 模糊拼音折叠（与 `hotwords.rs` 一致：zh/z、ch/c、sh/s、n/l、an/ang、en/eng、in/ing）。
fn fuzzy_syllable(syllable: &str) -> String {
    let (initial, rest) = if let Some(rest) = syllable.strip_prefix("zh") {
        ("z", rest)
    } else if let Some(rest) = syllable.strip_prefix("ch") {
        ("c", rest)
    } else if let Some(rest) = syllable.strip_prefix("sh") {
        ("s", rest)
    } else if let Some(rest) = syllable
        .strip_prefix('n')
        .filter(|rest| !rest.is_empty() && *rest != "g")
    {
        ("l", rest)
    } else {
        ("", syllable)
    };
    let rest = if let Some(stem) = rest.strip_suffix("ang") {
        format!("{stem}an")
    } else if let Some(stem) = rest.strip_suffix("eng") {
        format!("{stem}en")
    } else if let Some(stem) = rest.strip_suffix("ing") {
        format!("{stem}in")
    } else {
        rest.to_owned()
    };
    format!("{initial}{rest}")
}

fn term_fuzzy_syllables(term: &VocabularyTerm) -> Vec<String> {
    if !term.pinyin.trim().is_empty() {
        let parsed: Vec<String> = term
            .pinyin
            .split(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '-')
            .map(normalize_pinyin_syllable)
            .filter(|syllable| !syllable.is_empty())
            .map(|syllable| fuzzy_syllable(&syllable))
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    term.canonical
        .chars()
        .filter(|&ch| is_han_character(ch))
        .filter_map(|ch| char_readings(ch).into_iter().next())
        .map(|syllable| fuzzy_syllable(&syllable))
        .collect()
}

fn matches_fuzzy_window(haystack: &[Option<Vec<String>>], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    for start in 0..=haystack.len() - needle.len() {
        let matched = needle.iter().enumerate().all(|(offset, key)| {
            haystack[start + offset]
                .as_ref()
                .is_some_and(|options| options.contains(key))
        });
        if matched {
            return true;
        }
    }
    false
}

fn matches_initial_window(haystack: &[Option<Vec<char>>], needle: &[char]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    for start in 0..=haystack.len() - needle.len() {
        let matched = needle.iter().enumerate().all(|(offset, key)| {
            haystack[start + offset]
                .as_ref()
                .is_some_and(|options| options.contains(key))
        });
        if matched {
            return true;
        }
    }
    false
}

/// 提取小写 ASCII 词元（同时拆分 CamelCase 与 `_`/`-`/`.` 子词便于近似匹配）。
fn extract_ascii_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            push_split_ascii_tokens(&current, &mut tokens);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_split_ascii_tokens(&current, &mut tokens);
    }
    tokens
}

fn push_split_ascii_tokens(raw: &str, out: &mut Vec<String>) {
    let whole = raw.to_ascii_lowercase();
    if whole.len() >= 2 && !out.contains(&whole) {
        out.push(whole);
    }
    // 拆分 CamelCase 子词（如 `OpenRouter` -> `open`, `router`）。
    let bytes = raw.as_bytes();
    let mut start = 0;
    for index in 1..bytes.len() {
        let prev_lower = bytes[index - 1].is_ascii_lowercase();
        let curr_upper = bytes[index].is_ascii_uppercase();
        if prev_lower && curr_upper {
            let piece = raw[start..index].to_ascii_lowercase();
            if piece.len() >= 2 && !out.contains(&piece) {
                out.push(piece);
            }
            start = index;
        }
    }
    if start > 0 {
        let piece = raw[start..].to_ascii_lowercase();
        if piece.len() >= 2 && !out.contains(&piece) {
            out.push(piece);
        }
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let mut prev: Vec<usize> = (0..=b_bytes.len()).collect();
    let mut curr = vec![0; b_bytes.len() + 1];
    for (i, &ac) in a_bytes.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &bc) in b_bytes.iter().enumerate() {
            let cost = usize::from(ac != bc);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_bytes.len()]
}

fn shared_prefix_len(a: &str, b: &str) -> usize {
    a.bytes()
        .zip(b.bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

fn category_affinity_bonus(category: &str, app_category: AppCategory) -> i64 {
    let cat = category.trim().to_ascii_lowercase();
    if cat.is_empty() {
        return 0;
    }
    let matched = match app_category {
        AppCategory::TerminalIde => matches!(
            cat.as_str(),
            "tech" | "code" | "dev" | "cli" | "tool" | "programming" | "product" | "english"
        ),
        AppCategory::AiChat => matches!(
            cat.as_str(),
            "ai" | "model" | "tech" | "product" | "tool" | "dev"
        ),
        AppCategory::Im => matches!(
            cat.as_str(),
            "person" | "contact" | "name" | "chat" | "product" | "brand"
        ),
        AppCategory::Mail => matches!(
            cat.as_str(),
            "person" | "company" | "org" | "business" | "product" | "brand"
        ),
        AppCategory::BrowserDoc => matches!(
            cat.as_str(),
            "product" | "doc" | "tech" | "brand" | "company"
        ),
        AppCategory::General => matches!(cat.as_str(), "product" | "brand" | "person" | "tech"),
    };
    if matched {
        150
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(canonical: &str, aliases: &[&str], category: &str, weight: i64) -> VocabularyTerm {
        VocabularyTerm {
            canonical: canonical.to_owned(),
            aliases: aliases.iter().map(|s| (*s).to_owned()).collect(),
            pinyin: String::new(),
            category: category.to_owned(),
            weight,
        }
    }

    #[test]
    fn loads_missing_file_as_empty_and_parses_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let empty = load_from_preferences_dir(dir.path()).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.version, 1);

        let payload = r#"{
            "version": 1,
            "terms": [
                {
                    "canonical": "Shurufa",
                    "aliases": ["舒鲁法", "shuru fa", "Shurufa"],
                    "category": "Product",
                    "weight": 120
                },
                {
                    "canonical": "水杉输入法",
                    "aliases": ["水山输入法"],
                    "weight": 90
                }
            ]
        }"#;
        std::fs::write(dir.path().join(VOCABULARY_FILE_NAME), payload).unwrap();
        let loaded = load_from_preferences_dir(dir.path()).unwrap();
        assert_eq!(loaded.terms.len(), 2);
        assert_eq!(loaded.terms[0].canonical, "Shurufa");
        assert_eq!(loaded.terms[0].aliases, ["舒鲁法", "shuru fa"]);
        assert_eq!(loaded.terms[0].category, "product");
        assert_eq!(loaded.terms[1].pinyin, "shui shan shu ru fa");
    }

    #[test]
    fn build_vocabulary_merges_three_sources_with_custom_priority() {
        let custom = VoiceVocabulary::new(vec![term("Shurufa", &["舒鲁法"], "product", 100)]);
        let pinyin = [DictionaryEntryInput {
            key: "ming'tian'ke'ji".to_owned(),
            value: "明天科技".to_owned(),
            weight: 80,
        }];
        let english = [
            DictionaryEntryInput {
                key: "shurufa".to_owned(),
                value: "shurufa".to_owned(),
                weight: 500,
            },
            DictionaryEntryInput {
                key: "playwright".to_owned(),
                value: "Playwright".to_owned(),
                weight: 60,
            },
        ];
        let quick = [
            DictionaryEntryInput {
                key: "ns".to_owned(),
                value: "NovaSync".to_owned(),
                weight: 70,
            },
            DictionaryEntryInput {
                key: "long".to_owned(),
                value: "这是一条非常长的快捷短语模板用于测试长度上限过滤不会被收进语音词表里面去"
                    .to_owned(),
                weight: 999,
            },
        ];

        let vocab = build_vocabulary(Some(&custom), &pinyin, &english, &quick, 10);
        // 自定义 `Shurufa` 保留大小写与别名，不被英文词库小写覆盖。
        assert_eq!(vocab.terms[0].canonical, "Shurufa");
        assert_eq!(vocab.terms[0].aliases, ["舒鲁法"]);
        assert_eq!(vocab.terms[0].weight, 500);
        assert!(vocab.terms.iter().any(|item| item.canonical == "明天科技"));
        assert!(vocab
            .terms
            .iter()
            .any(|item| item.canonical == "Playwright"));
        assert!(vocab.terms.iter().any(|item| item.canonical == "NovaSync"));
        assert!(!vocab
            .terms
            .iter()
            .any(|item| item.canonical.starts_with("这是一条非常长")));
    }

    #[test]
    fn apply_deterministic_enforces_ascii_word_boundaries_and_longest_match() {
        let vocab = VoiceVocabulary::new(vec![
            term("Claude", &["cloud"], "ai", 100),
            term("Django", &["江狗"], "tech", 100),
            term("payload", &["配乐的"], "tech", 90),
            term("API Key", &["apik", "a p i k"], "tech", 100),
            term("AcmeCheck", &["acme"], "product", 100),
            term("水杉输入法", &[], "product", 100),
        ]);

        // `Cloud` 命中，但 `Cloudflare` / `iCloud` 受词边界保护不被误改。
        let text = "我用Cloud和江狗检查配乐的，顺便看Cloudflare和iCloud，还有a p i k和水山输入法";
        let out = apply_deterministic(text, &vocab);
        assert_eq!(
            out,
            "我用Claude和Django检查payload，顺便看Cloudflare和iCloud，还有API Key和水杉输入法"
        );
    }

    #[test]
    fn apply_deterministic_ignores_single_char_aliases_and_preserves_canonical_case() {
        let vocab = VoiceVocabulary::new(vec![
            term("DeepSeek", &["深", "d"], "ai", 100),
            term("AGENTS.md", &[], "code", 90),
        ]);
        // 单字中文与单字母别名必须被忽略，不破坏正文；规范词大小写自动恢复。
        let out = apply_deterministic("深入检查 deepseek 和 agents.md 文件", &vocab);
        assert_eq!(out, "深入检查 DeepSeek 和 AGENTS.md 文件");
    }

    #[test]
    fn select_prompt_terms_ranks_by_relevance_category_and_caps_at_60() {
        let mut terms = vec![
            term("Django", &["江狗"], "tech", 50),
            term("Cloudflare", &[], "tech", 40),
            term("水杉输入法", &[], "product", 30),
            term("NovaSync", &[], "tech", 20),
            term("晨曦餐厅", &[], "chat", 90),
        ];
        for i in 0..80 {
            terms.push(term(&format!("ExtraTerm{i:02}"), &[], "misc", 10));
        }
        let vocab = VoiceVocabulary::new(terms);

        let selected = select_prompt_terms(
            "用江狗接入 Cloudfare，再测一下水山输入法",
            AppCategory::TerminalIde,
            &vocab,
            100,
        );
        assert_eq!(selected.len(), MAX_PROMPT_TERM_LIMIT);
        // 别名命中、编辑距离近似命中、模糊拼音命中均排在前列。
        let top4 = &selected[..4];
        assert!(top4.contains(&"Django".to_owned()), "top4={top4:?}");
        assert!(top4.contains(&"Cloudflare".to_owned()), "top4={top4:?}");
        assert!(top4.contains(&"水杉输入法".to_owned()), "top4={top4:?}");
    }
}
