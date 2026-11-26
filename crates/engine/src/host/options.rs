//! `EngineOptions` 及其到 `SessionOptions` 的校验与映射。

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::assets;
use crate::diagnostics;
use crate::error::{EngineError, Result};
use crate::paths::RuntimePaths;
use crate::punctuation::{BracketMarks, PunctuationMarks, QuoteMarks};
use crate::session::SessionOptions;
use crate::types::{
    autocorrect_type, fuzzy_rule, CommandTableEntry, EnglishInputOptions, FrequencyAdjustmentMode,
    FrequencyAdjustmentOptions, FuzzyPinyinOptions, LocalModeOptions, MentionEntry,
    MixedExpressiveOptions, QuickPhraseEntry, SchemeSet, SchemeType, SentenceAssociationOptions,
    ShuangpinCustomTable, ShuangpinProfileKind, DEFAULT_FIRST_PAGE_ROWS,
};
use crate::user_dictionary::generation::prepare_runtime_paths_for;

const MAX_TRANSLATION_SIDECAR_BYTES: u64 = 1024 * 1024;

pub(crate) fn write_private_file_no_follow(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    msime_path_trust::reject_symlinked_components(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let file = crate::paths::open_file_no_follow(path)?;
            if !crate::paths::has_single_link(&file)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "translation sidecar is not a single-link regular file",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".custom-translations-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineOptions {
    pub resources: String,
    pub user_data: String,
    pub cache: String,
    pub dictionaries: String,
    /// 0 全拼，1 双拼。
    pub scheme: u8,
    pub enabled_schemes: SchemeSet,
    /// 0 小鹤，1 自然码，2 首道，3 微软，4 自定义，5 智能 ABC。
    pub shuangpin_profile: u8,
    pub shuangpin_custom_profile: Option<ShuangpinCustomTable>,
    pub shuangpin_preedit_uses_raw: bool,
    pub learning: bool,
    pub decoupled_personal_learning: bool,
    pub learn_english_on_enter: bool,
    pub next_word_prediction: bool,
    pub autocorrect_transposition: bool,
    pub autocorrect_neighbor: bool,
    pub fuzzy_pinyin_rules: u32,
    pub chinese_punctuation: bool,
    pub paired_punctuation: bool,
    pub punctuation_lock: u8,
    pub corner_quotes: bool,
    pub hollow_brackets: bool,
    pub enumeration_comma_backslash: bool,
    pub enumeration_comma_slash: bool,
    pub frequency_mode: String,
    pub frequency_trigger_count: u8,
    pub frequency_linear_step: u8,
    pub mixed_candidates: bool,
    pub mixed_english: bool,
    pub english_minimum_prefix: u8,
    pub english_first_page_count: u8,
    pub english_short_prefix_completion: bool,
    pub mixed_emoji: bool,
    pub mixed_kaomoji: bool,
    pub mixed_symbols: bool,
    pub local_unicode: bool,
    pub local_date_time: bool,
    pub local_date_time_inline: bool,
    pub local_date_time_menu: bool,
    pub local_quick_phrase: bool,
    pub local_emoji: bool,
    pub local_kaomoji: bool,
    pub local_super_jianpin: bool,
    pub local_temporary_english: bool,
    pub local_url: bool,
    pub local_expression: bool,
    pub local_expression_lowercase_entry: bool,
    pub local_command: bool,
    pub local_mention: bool,
    pub local_symbol: bool,
    pub local_shift_letter_entry: bool,
    pub command_table: Vec<CommandTableEntry>,
    pub mention_entries: Vec<MentionEntry>,
    pub quick_phrase_table: Vec<QuickPhraseEntry>,
    pub quick_phrase_candidates: bool,
    pub quick_phrase_frequency: bool,
    pub sentence_association: SentenceAssociationOptions,
    pub rescoring_context: String,
    pub sentence_alternatives: bool,
    pub first_page_rows: usize,
    pub single_character_only: bool,
    pub caret_edit_suffix: bool,
    /// Tab 笔画与偏旁筛选（`SessionOptions::tab_stroke_filter`）。
    pub tab_stroke_filter: bool,
    /// `msime-stroke.db` 的绝对路径；为空表示没有可用笔画词库。
    pub stroke_dictionary: String,
}

pub fn prepare_options(
    resources: &str,
    user_data: &str,
    cache: &str,
    content_id: &str,
) -> Result<EngineOptions> {
    prepare_options_for(resources, user_data, cache, content_id, SchemeSet::ALL)
}

pub fn prepare_options_for(
    resources: &str,
    user_data: &str,
    cache: &str,
    content_id: &str,
    enabled_schemes: SchemeSet,
) -> Result<EngineOptions> {
    let paths = prepare_runtime_paths_for(
        Path::new(resources),
        Path::new(user_data),
        Path::new(cache),
        content_id,
        enabled_schemes,
    )?;
    let text = |path: &Path| path.to_string_lossy().into_owned();
    Ok(EngineOptions {
        resources: text(&paths.resources),
        user_data: text(&paths.user_data),
        cache: text(&paths.cache),
        dictionaries: text(&paths.dictionaries),
        scheme: SchemeType::Quanpin as u8,
        enabled_schemes,
        shuangpin_profile: ShuangpinProfileKind::Xiaohe as u8,
        shuangpin_custom_profile: None,
        shuangpin_preedit_uses_raw: true,
        single_character_only: false,
        caret_edit_suffix: false,
        tab_stroke_filter: false,
        stroke_dictionary: String::new(),
        learning: false,
        decoupled_personal_learning: false,
        learn_english_on_enter: true,
        next_word_prediction: false,
        autocorrect_transposition: false,
        autocorrect_neighbor: false,
        fuzzy_pinyin_rules: 0,
        chinese_punctuation: true,
        paired_punctuation: true,
        punctuation_lock: 0,
        corner_quotes: false,
        hollow_brackets: false,
        enumeration_comma_backslash: true,
        enumeration_comma_slash: false,
        frequency_mode: FrequencyAdjustmentMode::Promote.name().to_owned(),
        frequency_trigger_count: 1,
        frequency_linear_step: 1,
        mixed_candidates: true,
        mixed_english: true,
        english_minimum_prefix: 5,
        english_first_page_count: 1,
        english_short_prefix_completion: false,
        mixed_emoji: false,
        mixed_kaomoji: false,
        mixed_symbols: false,
        local_unicode: true,
        local_date_time: true,
        local_date_time_inline: true,
        local_date_time_menu: true,
        local_quick_phrase: true,
        local_emoji: true,
        local_kaomoji: true,
        local_super_jianpin: true,
        local_temporary_english: true,
        local_url: true,
        local_expression: false,
        local_expression_lowercase_entry: false,
        local_command: false,
        local_mention: false,
        local_symbol: true,
        local_shift_letter_entry: true,
        command_table: Vec::new(),
        mention_entries: Vec::new(),
        quick_phrase_table: Vec::new(),
        quick_phrase_candidates: false,
        quick_phrase_frequency: true,
        sentence_association: SentenceAssociationOptions::default(),
        rescoring_context: String::new(),
        sentence_alternatives: false,
        first_page_rows: DEFAULT_FIRST_PAGE_ROWS,
    })
}

pub fn session_options(options: &EngineOptions) -> Result<SessionOptions> {
    prepare_translation_sidecar(options)?;
    let scheme = SchemeType::from_u8(options.scheme)
        .filter(|scheme| scheme.is_pinyin())
        .ok_or_else(|| EngineError::invalid(diagnostics::UNSUPPORTED_INPUT_SCHEME))?;
    let shuangpin_profile = shuangpin_profile(options)?;
    let mode = FrequencyAdjustmentMode::from_name(&options.frequency_mode)
        .ok_or_else(|| EngineError::invalid(diagnostics::UNSUPPORTED_FREQUENCY_MODE))?;
    let autocorrect_types = if options.autocorrect_transposition {
        autocorrect_type::TRANSPOSITION
    } else {
        0
    } | if options.autocorrect_neighbor {
        autocorrect_type::NEIGHBOR
    } else {
        0
    };
    let mut session = SessionOptions::new(runtime_paths(options));
    session.scheme = scheme;
    session.enabled_schemes = options.enabled_schemes;
    session.shuangpin_profile = shuangpin_profile;
    session.shuangpin_custom_profile = options.shuangpin_custom_profile.clone();
    session.shuangpin_preedit_uses_raw = options.shuangpin_preedit_uses_raw;
    session.single_character_only = options.single_character_only;
    session.caret_edit_suffix = options.caret_edit_suffix;
    session.tab_stroke_filter = options.tab_stroke_filter;
    session.stroke_dictionary = PathBuf::from(&options.stroke_dictionary);
    session.learning = options.learning;
    session.decoupled_personal_learning = options.decoupled_personal_learning;
    session.learn_english_on_enter = options.learn_english_on_enter;
    session.next_word_prediction = options.next_word_prediction;
    session.autocorrect_types = autocorrect_types;
    session.fuzzy_pinyin = FuzzyPinyinOptions {
        rules: options.fuzzy_pinyin_rules & fuzzy_rule::ALL,
    };
    session.chinese_punctuation = options.chinese_punctuation;
    session.paired_punctuation = options.paired_punctuation;
    session.punctuation_lock = i32::from(options.punctuation_lock);
    session.punctuation_marks = PunctuationMarks {
        quotes: if options.corner_quotes {
            QuoteMarks::Corner
        } else {
            QuoteMarks::Curly
        },
        brackets: if options.hollow_brackets {
            BracketMarks::Hollow
        } else {
            BracketMarks::Solid
        },
        enumeration_comma_backslash: options.enumeration_comma_backslash,
        enumeration_comma_slash: options.enumeration_comma_slash,
    };
    session.frequency = FrequencyAdjustmentOptions {
        mode,
        trigger_count: i32::from(options.frequency_trigger_count),
        linear_step: i32::from(options.frequency_linear_step),
    };
    session.mixed_candidates = options.mixed_candidates;
    session.english = EnglishInputOptions {
        mixed_candidates: options.mixed_english,
        minimum_prefix: usize::from(options.english_minimum_prefix),
        first_page_count: usize::from(options.english_first_page_count),
        short_prefix_completion: options.english_short_prefix_completion,
    };
    session.expressive = MixedExpressiveOptions {
        emoji_candidates: options.mixed_emoji,
        kaomoji_candidates: options.mixed_kaomoji,
        symbol_candidates: options.mixed_symbols,
    };
    session.local_modes = LocalModeOptions {
        unicode: options.local_unicode,
        date_time: options.local_date_time,
        date_time_inline: options.local_date_time_inline,
        date_time_menu: options.local_date_time_menu,
        quick_phrase: options.local_quick_phrase,
        emoji: options.local_emoji,
        kaomoji: options.local_kaomoji,
        super_jianpin: options.local_super_jianpin,
        temporary_english: options.local_temporary_english,
        url: options.local_url,
        temporary_japanese: false,
        expression: options.local_expression,
        expression_lowercase_entry: options.local_expression_lowercase_entry,
        command: options.local_command,
        mention: options.local_mention,
        chaizi: false,
        symbol: options.local_symbol,
        shift_letter_entry: options.local_shift_letter_entry,
    };
    session.command_table = options.command_table.clone();
    session.mention_entries = options.mention_entries.clone();
    session.quick_phrase_table = options.quick_phrase_table.clone();
    session.quick_phrase_candidates = options.quick_phrase_candidates;
    session.quick_phrase_frequency = options.quick_phrase_frequency;
    session.sentence_alternatives = options.sentence_alternatives;
    session.sentence_association = options.sentence_association.clone();
    session.first_page_rows = options.first_page_rows;
    session.rescoring_context = options.rescoring_context.clone();
    Ok(session)
}

pub fn prepare_translation_sidecar(options: &EngineOptions) -> Result<()> {
    let paths = runtime_paths(options);
    paths.validate()?;
    let target = paths.dictionary(assets::TRANSLATIONS);
    let parent = target
        .parent()
        .ok_or_else(|| EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED))?;
    reject_storage_ancestors(parent)
        .map_err(|_| EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED))?;
    let user = paths.user(assets::TRANSLATIONS);
    let source = if is_real_file(&user) {
        user
    } else {
        paths.resource(assets::TRANSLATIONS)
    };
    if !is_real_file(&source) {
        let _ = std::fs::remove_file(&target);
        return Ok(());
    }
    if std::fs::symlink_metadata(&target)
        .map(|metadata| !metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return Err(EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED));
    }
    let source_file = crate::paths::open_file_no_follow(&source)
        .map_err(|_| EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED))?;
    let source_size = source_file
        .metadata()
        .map_err(|_| EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED))?
        .len();
    if source_size > MAX_TRANSLATION_SIDECAR_BYTES {
        return Err(EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED));
    }
    let mut contents = Vec::with_capacity(source_size as usize);
    if source_file
        .take(MAX_TRANSLATION_SIDECAR_BYTES + 1)
        .read_to_end(&mut contents)
        .is_err()
        || contents.len() as u64 > MAX_TRANSLATION_SIDECAR_BYTES
    {
        return Err(EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED));
    }
    let copied = std::fs::create_dir_all(parent).and_then(|()| {
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "translation sidecar parent is not a real directory",
            ));
        }
        write_private_file_no_follow(&target, &contents).map(|()| contents.len() as u64)
    });
    copied
        .map(|_| ())
        .map_err(|_| EngineError::failed(diagnostics::TRANSLATION_SIDECAR_FAILED))
}

fn reject_storage_ancestors(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if !crate::paths::is_trusted_system_alias(&current) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "translation sidecar path has a symbolic-link ancestor",
                    ));
                }
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "translation sidecar parent is not a directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

pub(super) fn runtime_paths(options: &EngineOptions) -> RuntimePaths {
    RuntimePaths {
        resources: PathBuf::from(&options.resources),
        user_data: PathBuf::from(&options.user_data),
        cache: PathBuf::from(&options.cache),
        dictionaries: PathBuf::from(&options.dictionaries),
    }
}

pub(super) fn shuangpin_profile(options: &EngineOptions) -> Result<ShuangpinProfileKind> {
    let profile = ShuangpinProfileKind::from_u8(options.shuangpin_profile);
    if !options.enabled_schemes.contains(SchemeType::Shuangpin) {
        return Ok(profile.unwrap_or_default());
    }
    profile.ok_or_else(|| EngineError::invalid(diagnostics::UNSUPPORTED_SHUANGPIN_PROFILE))
}
