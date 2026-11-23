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
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// 单条跨语种语音术语，支持规范词、音近/错写别名、无声调拼音、场景分类、权重与来源标记。
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// 从语音上屏后用户手动修改中提取出的单条候选纠错词。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedCorrection {
    pub canonical: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias: String,
}

/// 单条已落盘的自动学习记录，附带修改前快照以支持气泡一键撤销。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedTermRecord {
    pub canonical: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_term: Option<VocabularyTerm>,
}

/// 单次词条写入（新增或合并更新）结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertTermOutcome {
    pub term: VocabularyTerm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_term: Option<VocabularyTerm>,
    pub changed: bool,
}

/// 事后编辑自动学词报告。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnCorrectionsReport {
    pub learned: Vec<LearnedTermRecord>,
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

static VOCAB_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn normalize_term_source(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "manual" => "manual".to_owned(),
        "auto_learned" => "auto_learned".to_owned(),
        _ => String::new(),
    }
}

/// 原子写入自定义语音词表文件：写同目录独立临时文件并 `sync_all` 后 `rename` 覆盖，不加文件锁。
pub fn save_vocabulary_file(path: &Path, vocab: &VoiceVocabulary) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid voice vocabulary path".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create voice vocabulary directory: {error}"))?;

    let normalized = VoiceVocabulary {
        version: if vocab.version == 0 {
            default_vocabulary_version()
        } else {
            vocab.version
        },
        terms: sanitize_terms(vocab.terms.clone(), MAX_VOCABULARY_LIMIT),
    };
    let bytes = serde_json::to_vec_pretty(&normalized)
        .map_err(|error| format!("failed to serialize voice vocabulary: {error}"))?;
    if bytes.len() as u64 > MAX_VOCABULARY_FILE_BYTES {
        return Err("voice vocabulary file exceeds size limit".to_owned());
    }

    let pid = std::process::id();
    let seq = VOCAB_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(".voice-vocabulary.{pid}.{stamp}.{seq}.tmp"));

    let write_result = (|| -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("failed to write voice vocabulary: {error}"));
    }
    if let Err(error) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("failed to replace voice vocabulary: {error}"));
    }
    Ok(())
}

/// 原子写入 `<preferences_directory>/voice-vocabulary.json`。
pub fn save_to_preferences_dir(
    preferences_dir: &Path,
    vocab: &VoiceVocabulary,
) -> Result<(), String> {
    save_vocabulary_file(&preferences_dir.join(VOCABULARY_FILE_NAME), vocab)
}

/// 向 `<preferences_directory>/voice-vocabulary.json` 新增或合并更新单条术语。
pub fn upsert_term_in_preferences_dir(
    preferences_dir: &Path,
    raw_term: VocabularyTerm,
) -> Result<UpsertTermOutcome, String> {
    let mut sanitized = sanitize_terms(vec![raw_term], 1);
    let incoming = sanitized
        .pop()
        .ok_or_else(|| "invalid voice vocabulary term".to_owned())?;

    let mut vocab = load_from_preferences_dir(preferences_dir)?;
    let key = incoming.canonical.to_ascii_lowercase();
    if let Some(pos) = vocab
        .terms
        .iter()
        .position(|existing| existing.canonical.to_ascii_lowercase() == key)
    {
        let previous = vocab.terms[pos].clone();
        let mut updated = previous.clone();
        updated.canonical = incoming.canonical.clone();
        merge_aliases(&mut updated.aliases, &incoming.aliases, &updated.canonical);
        if !incoming.pinyin.is_empty() {
            updated.pinyin = incoming.pinyin.clone();
        } else if updated.pinyin.is_empty() {
            updated.pinyin = derive_han_pinyin(&updated.canonical).unwrap_or_default();
        }
        if !incoming.category.is_empty() {
            updated.category = incoming.category.clone();
        }
        updated.weight = updated.weight.max(incoming.weight);
        // 手动词条升级优先于自动学习标记；仅在原本无来源时补写新来源。
        if incoming.source == "manual" || updated.source.is_empty() {
            updated.source = incoming.source.clone();
        }
        let changed = updated != previous;
        if changed {
            vocab.terms[pos] = updated.clone();
            save_to_preferences_dir(preferences_dir, &vocab)?;
        }
        return Ok(UpsertTermOutcome {
            term: updated,
            previous_term: Some(previous),
            changed,
        });
    }

    vocab.terms.insert(0, incoming.clone());
    vocab.terms.truncate(MAX_VOCABULARY_LIMIT);
    save_to_preferences_dir(preferences_dir, &vocab)?;
    Ok(UpsertTermOutcome {
        term: incoming,
        previous_term: None,
        changed: true,
    })
}

/// 从 `<preferences_directory>/voice-vocabulary.json` 删除指定规范词（忽略 ASCII 大小写）。
pub fn remove_term_in_preferences_dir(
    preferences_dir: &Path,
    canonical: &str,
) -> Result<bool, String> {
    let target = canonical.trim();
    if target.is_empty() {
        return Ok(false);
    }
    let mut vocab = load_from_preferences_dir(preferences_dir)?;
    let before = vocab.terms.len();
    let target_lower = target.to_ascii_lowercase();
    vocab
        .terms
        .retain(|term| term.canonical.to_ascii_lowercase() != target_lower);
    let removed = vocab.terms.len() != before;
    if removed {
        save_to_preferences_dir(preferences_dir, &vocab)?;
    }
    Ok(removed)
}

/// 撤销一次词条写入：若 `previous_term` 为空则删除该词条，否则精确恢复为修改前的原词条。
pub fn restore_term_in_preferences_dir(
    preferences_dir: &Path,
    canonical: &str,
    previous_term: Option<VocabularyTerm>,
) -> Result<(), String> {
    let target = canonical.trim();
    if target.is_empty() {
        return Ok(());
    }
    let mut vocab = load_from_preferences_dir(preferences_dir)?;
    let target_lower = target.to_ascii_lowercase();
    let existing_pos = vocab
        .terms
        .iter()
        .position(|term| term.canonical.to_ascii_lowercase() == target_lower);

    match (existing_pos, previous_term) {
        (Some(pos), Some(prev)) => {
            if let Some(restored) = sanitize_terms(vec![prev], 1).pop() {
                vocab.terms[pos] = restored;
                save_to_preferences_dir(preferences_dir, &vocab)?;
            }
        }
        (None, Some(prev)) => {
            if let Some(restored) = sanitize_terms(vec![prev], 1).pop() {
                vocab.terms.insert(0, restored);
                vocab.terms.truncate(MAX_VOCABULARY_LIMIT);
                save_to_preferences_dir(preferences_dir, &vocab)?;
            }
        }
        (Some(pos), None) => {
            vocab.terms.remove(pos);
            save_to_preferences_dir(preferences_dir, &vocab)?;
        }
        (None, None) => {}
    }
    Ok(())
}

fn normalize_edit_box_text(raw: &str) -> String {
    raw.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&ch| {
            !matches!(
                ch,
                '\u{FFFC}' | '\u{FEFF}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}'
            )
        })
        .collect()
}

fn is_ascii_term_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}

fn is_edge_trim_punct(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            ',' | '.'
                | '!'
                | '?'
                | ';'
                | ':'
                | '"'
                | '\''
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '，'
                | '。'
                | '！'
                | '？'
                | '；'
                | '：'
                | '、'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '…'
        )
}

fn has_meaningful_letter_or_han(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_ascii_alphabetic() || is_han_character(ch))
}

fn trim_edge_punct(text: &str) -> &str {
    text.trim_matches(is_edge_trim_punct)
}

/// 从语音上屏后用户的局部手动修改中提取高置信度纠错词条（纯函数，无网络、无 I/O）。
///
/// 内置严格误收护栏：忽略清空发送、大段重写、纯句尾续写、纯删除、纯标点/纯数字改动与单字汉字替换。
pub fn extract_edit_corrections(
    inserted_text: &str,
    original_box: &str,
    edited_box: &str,
) -> Vec<LearnedCorrection> {
    let inserted = normalize_edit_box_text(inserted_text);
    let inserted_trimmed = inserted.trim();
    if inserted_trimmed.is_empty() {
        return Vec::new();
    }

    let edited = normalize_edit_box_text(edited_box);
    if edited.trim().is_empty() {
        return Vec::new();
    }

    let original = if original_box.trim().is_empty() {
        inserted.clone()
    } else {
        normalize_edit_box_text(original_box)
    };
    if original == edited {
        return Vec::new();
    }

    // 确认 `inserted` 确实位于原始输入框内，防止把用户在别处段落的编辑误当成语音纠错。
    let Some(ins_byte_start) = original.find(inserted_trimmed) else {
        return Vec::new();
    };
    let ins_char_start = original[..ins_byte_start].chars().count();
    let ins_char_len = inserted_trimmed.chars().count();
    let ins_char_end = ins_char_start + ins_char_len;

    let orig_chars: Vec<char> = original.chars().collect();
    let edit_chars: Vec<char> = edited.chars().collect();

    let mut prefix = 0usize;
    let min_len = orig_chars.len().min(edit_chars.len());
    while prefix < min_len && orig_chars[prefix] == edit_chars[prefix] {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < (min_len - prefix)
        && orig_chars[orig_chars.len() - 1 - suffix] == edit_chars[edit_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut old_start = prefix;
    let mut old_end = orig_chars.len() - suffix;
    let mut new_start = prefix;
    let mut new_end = edit_chars.len() - suffix;

    // 改动必须与刚语音上屏的文本区间相交。
    if old_end < ins_char_start || old_start > ins_char_end {
        return Vec::new();
    }

    // 若改动落在 ASCII 词内部（如 `claude`→`Claude`、`Shurfa`→`Shurufa`、`Claudee`→`Claude`），
    // 对称向外扩展到完整 ASCII 词边界。
    let touches_ascii = orig_chars[old_start..old_end]
        .iter()
        .chain(edit_chars[new_start..new_end].iter())
        .any(|&ch| ch.is_ascii_alphanumeric())
        || (old_start > 0
            && orig_chars[old_start - 1].is_ascii_alphanumeric()
            && old_end < orig_chars.len()
            && orig_chars[old_end].is_ascii_alphanumeric());
    if touches_ascii {
        while old_start > 0
            && new_start > 0
            && is_ascii_term_char(orig_chars[old_start - 1])
            && orig_chars[old_start - 1] == edit_chars[new_start - 1]
        {
            old_start -= 1;
            new_start -= 1;
        }
        while old_end < orig_chars.len()
            && new_end < edit_chars.len()
            && is_ascii_term_char(orig_chars[old_end])
            && orig_chars[old_end] == edit_chars[new_end]
        {
            old_end += 1;
            new_end += 1;
        }
    }

    // 拦截纯续写、纯删除及大段重写。
    let old_span_len = old_end.saturating_sub(old_start);
    let new_span_len = new_end.saturating_sub(new_start);
    if old_span_len == 0 || new_span_len == 0 || old_span_len > 24 || new_span_len > 36 {
        return Vec::new();
    }
    if ins_char_len > 12 && old_span_len * 2 > ins_char_len {
        return Vec::new();
    }

    let raw_old: String = orig_chars[old_start..old_end].iter().collect();
    let raw_new: String = edit_chars[new_start..new_end].iter().collect();
    if raw_old.contains('\n') || raw_new.contains('\n') {
        return Vec::new();
    }

    let old_token = trim_edge_punct(&raw_old);
    let new_token = trim_edge_punct(&raw_new);
    if old_token.is_empty()
        || new_token.is_empty()
        || old_token == new_token
        || new_token.contains("://")
        || !has_meaningful_letter_or_han(old_token)
        || !has_meaningful_letter_or_han(new_token)
    {
        return Vec::new();
    }

    let new_has_ascii = new_token.chars().any(|ch| ch.is_ascii_alphabetic());
    let old_chars_count = old_token.chars().count();
    let new_chars_count = new_token.chars().count();

    if new_has_ascii {
        if !(2..=40).contains(&new_chars_count)
            || !(2..=40).contains(&old_chars_count)
            || new_token.split_whitespace().count() > 3
            || old_token.split_whitespace().count() > 4
        {
            return Vec::new();
        }
        let alias = if old_token.eq_ignore_ascii_case(new_token) {
            String::new()
        } else {
            old_token.to_owned()
        };
        return vec![LearnedCorrection {
            canonical: new_token.to_owned(),
            alias,
        }];
    }

    // 纯中文/非 ASCII 纠错采用更严格门控：必须 2..=8 字完整词、不含空白和内部标点、不互为简单删增子串。
    let is_clean_han_term = |s: &str| {
        let count = s.chars().count();
        (2..=8).contains(&count)
            && s.chars()
                .all(|ch| is_han_character(ch) || ch.is_ascii_digit())
            && s.chars().any(is_han_character)
    };
    if !is_clean_han_term(old_token) || !is_clean_han_term(new_token) {
        return Vec::new();
    }
    if old_token.contains(new_token) || new_token.contains(old_token) {
        return Vec::new();
    }

    vec![LearnedCorrection {
        canonical: new_token.to_owned(),
        alias: old_token.to_owned(),
    }]
}

/// 从一次事后编辑提取纠错词、去重并原子写入 `<preferences_directory>/voice-vocabulary.json`。
pub fn learn_corrections_in_preferences_dir(
    preferences_dir: &Path,
    inserted_text: &str,
    original_box: &str,
    edited_box: &str,
) -> Result<LearnCorrectionsReport, String> {
    let candidates = extract_edit_corrections(inserted_text, original_box, edited_box);
    if candidates.is_empty() {
        return Ok(LearnCorrectionsReport::default());
    }

    let mut report = LearnCorrectionsReport::default();
    for candidate in candidates.into_iter().take(2) {
        let mut aliases = Vec::new();
        if !candidate.alias.is_empty() {
            aliases.push(candidate.alias.clone());
        }
        let outcome = upsert_term_in_preferences_dir(
            preferences_dir,
            VocabularyTerm {
                canonical: candidate.canonical.clone(),
                aliases,
                pinyin: String::new(),
                category: String::new(),
                weight: 800,
                source: "auto_learned".to_owned(),
            },
        )?;
        if outcome.changed {
            report.learned.push(LearnedTermRecord {
                canonical: outcome.term.canonical,
                alias: candidate.alias,
                previous_term: outcome.previous_term,
            });
        }
    }
    Ok(report)
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
                        source: String::new(),
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
                        source: String::new(),
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
                        source: String::new(),
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
        let source = normalize_term_source(&term.source);
        result.push(VocabularyTerm {
            canonical,
            aliases,
            pinyin,
            category,
            weight: term.weight,
            source,
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
            source: String::new(),
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

    #[test]
    fn extract_edit_corrections_captures_high_precision_replacements_and_filters_noise() {
        // 1. 中文音译错词改英文专有名词
        assert_eq!(
            extract_edit_corrections(
                "我觉得用可劳德写代码很快",
                "早安，我觉得用可劳德写代码很快",
                "早安，我觉得用Claude写代码很快"
            ),
            vec![LearnedCorrection {
                canonical: "Claude".to_owned(),
                alias: "可劳德".to_owned(),
            }]
        );

        // 2. 英文漏字/拼写纠错（自动按 ASCII 词边界展开）
        assert_eq!(
            extract_edit_corrections("测试 Shurfa 语音", "测试 Shurfa 语音", "测试 Shurufa 语音"),
            vec![LearnedCorrection {
                canonical: "Shurufa".to_owned(),
                alias: "Shurfa".to_owned(),
            }]
        );

        // 3. 英文大小写规范纠错（alias 置空，不写冗余同拼写别名）
        assert_eq!(
            extract_edit_corrections("打开 claude 看看", "打开 claude 看看", "打开 Claude 看看"),
            vec![LearnedCorrection {
                canonical: "Claude".to_owned(),
                alias: String::new(),
            }]
        );

        // 4. 多词英文产品名纠错
        assert_eq!(
            extract_edit_corrections(
                "看看 t3cod 的结果",
                "看看 t3cod 的结果",
                "看看 T3 Code 的结果"
            ),
            vec![LearnedCorrection {
                canonical: "T3 Code".to_owned(),
                alias: "t3cod".to_owned(),
            }]
        );

        // 5. 2~8 字纯中文完整词替换（无单字截断歧义）
        assert_eq!(
            extract_edit_corrections(
                "帮我装一下舒鲁法看看",
                "帮我装一下舒鲁法看看",
                "帮我装一下水杉看看"
            ),
            vec![LearnedCorrection {
                canonical: "水杉".to_owned(),
                alias: "舒鲁法".to_owned(),
            }]
        );

        // 6. 护栏：清空发送、纯续写、纯删除、整句重写、单字替换、纯标点改动均必须忽略
        assert!(extract_edit_corrections("测试句子", "测试句子", "").is_empty());
        assert!(
            extract_edit_corrections("帮我跑单测", "帮我跑单测", "帮我跑单测顺便看看日志")
                .is_empty()
        );
        assert!(extract_edit_corrections(
            "我觉得这个方案很不错",
            "我觉得这个方案很不错",
            "我觉得这个方案不错"
        )
        .is_empty());
        assert!(extract_edit_corrections(
            "今天下午两点钟我们开个会同步一下下周排期",
            "今天下午两点钟我们开个会同步一下下周排期",
            "明天上午直接上线吧"
        )
        .is_empty());
        assert!(extract_edit_corrections("你做的很好", "你做的很好", "你做得很好").is_empty());
        assert!(extract_edit_corrections("好的", "好的", "好的！").is_empty());
    }

    #[test]
    fn vocabulary_persistence_learn_dedup_and_undo_restore_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        // 首次自动学词写入 voice-vocabulary.json
        let first = learn_corrections_in_preferences_dir(
            dir.path(),
            "用可劳德跑一次回归",
            "用可劳德跑一次回归",
            "用Claude跑一次回归",
        )
        .unwrap();
        assert_eq!(first.learned.len(), 1);
        assert_eq!(first.learned[0].canonical, "Claude");
        assert_eq!(first.learned[0].alias, "可劳德");
        assert!(first.learned[0].previous_term.is_none());

        let loaded = load_from_preferences_dir(dir.path()).unwrap();
        assert_eq!(loaded.terms.len(), 1);
        assert_eq!(loaded.terms[0].source, "auto_learned");
        assert_eq!(loaded.terms[0].aliases, ["可劳德"]);

        // 同一词条+别名重复学词不再重复触发提示
        let dup = learn_corrections_in_preferences_dir(
            dir.path(),
            "用可劳德跑一次回归",
            "用可劳德跑一次回归",
            "用Claude跑一次回归",
        )
        .unwrap();
        assert!(dup.learned.is_empty());

        // 再学同一个词的新别名：记录 previous_term，撤销可精确还原旧状态而非误删整条
        let second = learn_corrections_in_preferences_dir(
            dir.path(),
            "用克劳德跑一次回归",
            "用克劳德跑一次回归",
            "用Claude跑一次回归",
        )
        .unwrap();
        assert_eq!(second.learned.len(), 1);
        assert!(second.learned[0].previous_term.is_some());
        let after_second = load_from_preferences_dir(dir.path()).unwrap();
        assert_eq!(after_second.terms[0].aliases, ["可劳德", "克劳德"]);

        restore_term_in_preferences_dir(
            dir.path(),
            &second.learned[0].canonical,
            second.learned[0].previous_term.clone(),
        )
        .unwrap();
        let after_undo_second = load_from_preferences_dir(dir.path()).unwrap();
        assert_eq!(after_undo_second.terms[0].aliases, ["可劳德"]);

        // 撤销首次新增：彻底移除词条
        restore_term_in_preferences_dir(
            dir.path(),
            &first.learned[0].canonical,
            first.learned[0].previous_term.clone(),
        )
        .unwrap();
        assert!(load_from_preferences_dir(dir.path()).unwrap().is_empty());
    }
}
