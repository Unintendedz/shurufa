//! `InputSession`：全拼与双拼的组字、编辑、本地表达式与上屏调度核心。

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::caret_edit::CaretEdit;
use super::chain::CommitChain;
use super::clock::Clock;
use super::conversion::ConversionEdit;
use super::online::OnlineRequestGuard;
use super::options::SessionOptions;
use super::stroke_filter::{StrokeDictionary, StrokeFilter};
use crate::assets;
use crate::diagnostics;
use crate::error::{EngineError, Result};
use crate::ime::queries::CandidateQueries;
use crate::ime::ImeSession;
use crate::local::date_time::LocalDateTime;
use crate::local::url;
use crate::local::GENERATED_MODE_INPUT_LIMIT;
use crate::paths::RuntimePaths;
use crate::punctuation::PunctuationPolicy;
use crate::shuangpin::custom::session_profile;
use crate::shuangpin::ShuangpinProfile;
use crate::time::Instant;
use crate::types::{
    CandidateSource, Command, CommandTableEntry, EnglishInputOptions, FirstPageRows,
    FrequencyAdjustmentOptions, KeyResult, LocalInputMode, LocalModeOptions, MentionEntry,
    MixedExpressiveOptions, QuickPhraseEntry, SchemeKey, SchemeType, WordItem,
};
use crate::user_dictionary::ngram_store::PersonalNgramStore;
use crate::user_dictionary::removal::learn_entered_english_word;
use crate::user_dictionary::typo_profile::PersonalTypoProfile;

const ENTERED_ENGLISH_WORD_WEIGHT: i64 = 10;

/// 最近一次写入独立个人叠加层事件表的记录，供上屏后 5 秒内退格删光整段词时确定性回滚并写入 24 小时负反馈。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LastOverlayLearning {
    pub event_id: i64,
    pub entry_key: String,
    pub word: String,
    pub committed_at_unix: i64,
    pub remaining_chars: usize,
}

/// 分段选词暂存的造词进度。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct CreatingWordProgress {
    pub pinyin: String,
    pub word: String,
    pub preedit: String,
    pub completed: bool,
    pub can_store: bool,
}

pub(super) struct InputSession {
    pub paths: RuntimePaths,
    pub journal_path: PathBuf,
    pub engine: ImeSession,
    pub queries: CandidateQueries,
    pub quick_phrase_candidates: bool,
    pub quick_phrase_frequency: bool,
    pub first_page_rows: usize,
    pub clock: Clock,
    pub profile: &'static ShuangpinProfile,
    pub caret: Option<usize>,
    pub phrase_progress: CreatingWordProgress,
    pub pending_sequence: Option<String>,
    pub pending_sequence_with_cases: Option<String>,
    pub autocorrect_types: u32,
    pub chinese_punctuation_enabled: bool,
    pub punctuation_lock: i32,
    pub learning_enabled: bool,
    pub decoupled_personal_learning: bool,
    pub last_overlay_learning: Option<LastOverlayLearning>,
    pub learn_english_on_enter: bool,
    pub personal_context_enabled: bool,
    pub punctuation: PunctuationPolicy,
    pub frequency: FrequencyAdjustmentOptions,
    pub local_mode_options: LocalModeOptions,
    pub english_options: EnglishInputOptions,
    pub expressive_options: MixedExpressiveOptions,
    pub dedicated_english: bool,
    pub dedicated_english_preedit: String,
    pub dedicated_english_candidates: Vec<WordItem>,
    pub english_online_candidates_enabled: bool,
    pub english_online_rows: Vec<WordItem>,
    pub mixed_candidates: Vec<WordItem>,
    pub mixed_candidates_enabled: bool,
    pub local_mode: LocalInputMode,
    pub local_preedit: String,
    pub local_candidates: Vec<WordItem>,
    pub uppercase_passthrough: bool,
    pub date_time_menu_page: Option<Vec<WordItem>>,
    pub online_requests: OnlineRequestGuard,
    pub cloud_candidates_enabled: bool,
    pub chain: CommitChain,
    pub personal_context: Arc<PersonalNgramStore>,
    pub typo_profile: Arc<PersonalTypoProfile>,
    pub personal_reranked: bool,
    pub ranking_candidates: Option<Vec<WordItem>>,
    pub prefix_active: bool,
    pub prefix_query_input: String,
    pub prefix_candidates: Vec<WordItem>,
    pub caret_edit_suffix: bool,
    pub caret_edit: CaretEdit,
    pub shuangpin_preedit_uses_raw: bool,
    pub single_character_only: bool,
    pub conversion: Option<ConversionEdit>,
    /// Tab 笔画与偏旁筛选开关（`SessionOptions::tab_stroke_filter`）。
    pub tab_stroke_filter: bool,
    /// `msime-stroke.db` 文件路径；首次按 Tab 时只读打开。
    pub stroke_dictionary_path: PathBuf,
    pub stroke_dictionary: Option<StrokeDictionary>,
    pub stroke_filter: Option<StrokeFilter>,
}

impl InputSession {
    pub fn new(options: &SessionOptions) -> Result<Self> {
        if !frequency_options_valid(options.frequency) || !english_options_valid(options.english) {
            return Err(EngineError::invalid(diagnostics::INVALID_SESSION_OPTIONS));
        }
        let paths = options.paths.clone();
        let journal = paths.user(assets::USER_JOURNAL);
        let profile = session_profile(
            options.shuangpin_profile,
            options.shuangpin_custom_profile.as_ref(),
            options.enabled_schemes.contains(SchemeType::Shuangpin),
        )?;
        let mut engine = ImeSession::new(options.scheme, options.enabled_schemes, profile, &paths)?;
        engine.set_autocorrect_types(0);
        let mut session = Self {
            queries: CandidateQueries::new(&paths, profile),
            quick_phrase_candidates: options.quick_phrase_candidates,
            quick_phrase_frequency: options.quick_phrase_frequency,
            first_page_rows: FirstPageRows::new(options.first_page_rows).get(),
            engine,
            clock: Clock::default(),
            profile,
            caret: None,
            phrase_progress: CreatingWordProgress::default(),
            pending_sequence: None,
            pending_sequence_with_cases: None,
            autocorrect_types: 0,
            chinese_punctuation_enabled: true,
            punctuation_lock: 0,
            learning_enabled: true,
            decoupled_personal_learning: options.decoupled_personal_learning,
            last_overlay_learning: None,
            learn_english_on_enter: options.learn_english_on_enter,
            personal_context_enabled: true,
            punctuation: PunctuationPolicy::default(),
            frequency: FrequencyAdjustmentOptions::default(),
            local_mode_options: LocalModeOptions::default(),
            english_options: EnglishInputOptions::default(),
            expressive_options: MixedExpressiveOptions::default(),
            dedicated_english: false,
            dedicated_english_preedit: String::new(),
            dedicated_english_candidates: Vec::new(),
            english_online_candidates_enabled: false,
            english_online_rows: Vec::new(),
            mixed_candidates: Vec::new(),
            mixed_candidates_enabled: true,
            local_mode: LocalInputMode::None,
            local_preedit: String::new(),
            local_candidates: Vec::new(),
            uppercase_passthrough: false,
            date_time_menu_page: None,
            online_requests: OnlineRequestGuard::new(),
            cloud_candidates_enabled: false,
            chain: CommitChain::default(),
            personal_context: PersonalNgramStore::for_journal(&journal),
            typo_profile: PersonalTypoProfile::shared(&journal),
            personal_reranked: false,
            ranking_candidates: None,
            prefix_active: false,
            prefix_query_input: String::new(),
            prefix_candidates: Vec::new(),
            caret_edit_suffix: options.caret_edit_suffix,
            caret_edit: CaretEdit::default(),
            shuangpin_preedit_uses_raw: options.shuangpin_preedit_uses_raw,
            single_character_only: options.single_character_only,
            conversion: None,
            tab_stroke_filter: options.tab_stroke_filter,
            stroke_dictionary_path: options.stroke_dictionary.clone(),
            stroke_dictionary: None,
            stroke_filter: None,
            journal_path: journal,
            paths,
        };

        session.set_autocorrect_types(options.autocorrect_types);
        session
            .engine
            .set_fuzzy_pinyin_options(options.fuzzy_pinyin);
        session.frequency = options.frequency;
        session.mixed_candidates_enabled = options.mixed_candidates;
        session.english_options = options.english;
        session.set_local_mode_options(options.local_modes);
        session.queries.set_command_table(&options.command_table);
        session.queries.set_mentions(&options.mention_entries);
        session
            .queries
            .set_quick_phrase_table(&options.quick_phrase_table);
        session.expressive_options = options.expressive;
        session.set_personal_context_enabled(options.personal_context);

        session.chinese_punctuation_enabled = options.chinese_punctuation;
        session
            .punctuation
            .set_paired_enabled(options.paired_punctuation);
        session.punctuation.set_marks(options.punctuation_marks);
        session.punctuation_lock = options.punctuation_lock;
        session.learning_enabled = options.learning;
        session
            .engine
            .set_sentence_alternatives(options.sentence_alternatives);
        session
            .engine
            .set_sentence_association(options.sentence_association.clone());
        session
            .engine
            .set_first_page_rows(FirstPageRows::new(options.first_page_rows));
        session
            .engine
            .set_rescoring_context(&options.rescoring_context);
        session.update_mixed_candidates();
        Ok(session)
    }

    pub fn handle_character(&mut self, value: u8, shift_only: bool) -> KeyResult {
        self.punctuation.clear_quote_undo();
        self.last_overlay_learning = None;
        self.exit_date_time_menu();
        let result = self
            .stroke_filter_character(value)
            .unwrap_or_else(|| self.dispatch_character(value, shift_only));
        self.uppercase_passthrough = value.is_ascii_uppercase() && !result.handled;
        result
    }

    pub(super) fn end_uppercase_run(&mut self) {
        self.uppercase_passthrough = false;
    }

    pub(super) fn dispatch_character(&mut self, value: u8, shift_only: bool) -> KeyResult {
        if self.conversion.is_some() {
            if !(value.is_ascii_alphabetic() || value == b'\'' || value == b';') {
                return KeyResult::unhandled();
            }
            self.leave_conversion();
        }
        if self.takes_literal_symbol(value) {
            return self.insert_literal_symbol(value);
        }
        if self
            .caret
            .is_some_and(|caret| caret < self.editing_text_len())
        {
            return self.insert_at_caret(value);
        }
        self.caret = None;
        if self.dedicated_english {
            if value.is_ascii_alphabetic() {
                self.invalidate_online_requests();
                self.dedicated_english_preedit.push(value as char);
                self.update_dedicated_english_candidates();
            }
            return KeyResult::handled();
        }
        if self.local_mode != LocalInputMode::None {
            return self.handle_local_character(value);
        }
        if !self.has_composition() {
            let entry = if shift_only && !self.uppercase_passthrough {
                self.local_mode_for_entry(value)
            } else {
                None
            }
            .or_else(|| self.local_mode_for_symbol(value))
            .or_else(|| self.local_mode_for_lowercase_v(value));
            if let Some(mode) = entry {
                return KeyResult::handled().with_diagnostic(self.enter_local_mode(mode, value));
            }
        }
        if let Some(text) = self.url_entry(value) {
            self.enter_url_mode(text);
            return KeyResult::handled();
        }

        let scheme = self.scheme();
        let lowercase_letter = value.is_ascii_lowercase();
        let microsoft_final =
            value == b';' && scheme == SchemeType::Shuangpin && self.profile.uses_semicolon_key();
        let starts_pinyin_with_shift = shift_only
            && (self.local_mode_options.shift_letter_entry
                || (self.mixed_candidates_enabled
                    && self.english_options.mixed_candidates
                    && self.english_options.short_prefix_completion))
            && !self.uppercase_passthrough;
        let composing_pinyin_uppercase = value.is_ascii_uppercase()
            && (self.has_composition() || starts_pinyin_with_shift)
            && matches!(scheme, SchemeType::Quanpin | SchemeType::Shuangpin);
        if !lowercase_letter && !composing_pinyin_uppercase && value != b'\'' && !microsoft_final {
            if !self.has_composition() {
                self.reset_commit_context();
            }
            return KeyResult::unhandled();
        }
        if value == b'\'' && !self.has_composition() {
            self.reset_commit_context();
            return KeyResult::unhandled();
        }
        if !self.has_composition()
            && self.chain.previous.is_some()
            && self.chain.paused(self.steady_now())
        {
            self.chain.reset();
        }

        let key = if value == b'\'' {
            SchemeKey::Apostrophe
        } else if microsoft_final {
            SchemeKey::Semicolon
        } else if value.is_ascii_alphabetic() {
            SchemeKey::Letter(value)
        } else {
            SchemeKey::Symbol(value)
        };
        let changed = self.engine.handle_key(key);
        self.update_mixed_candidates();
        if !changed {
            return KeyResult::unhandled();
        }
        self.invalidate_online_requests();
        KeyResult::handled()
    }

    pub fn handle_local_character(&mut self, value: u8) -> KeyResult {
        let letter = value.is_ascii_alphabetic();
        let accepted = match self.local_mode {
            LocalInputMode::TemporaryEnglish => {
                if !letter {
                    return KeyResult::unhandled();
                }
                true
            }
            LocalInputMode::SuperJianpin => {
                letter
                    && !(value.is_ascii_uppercase()
                        && self.local_preedit.len() == 1
                        && matches!(self.scheme(), SchemeType::Quanpin | SchemeType::Shuangpin))
            }
            LocalInputMode::Emoji | LocalInputMode::Kaomoji => letter || value == b'\'',
            LocalInputMode::Symbol => letter,
            LocalInputMode::QuickPhrase | LocalInputMode::DateTime => value.is_ascii_lowercase(),
            LocalInputMode::Unicode => {
                value.is_ascii_hexdigit() || (value == b'+' && self.local_preedit == "U")
            }
            LocalInputMode::Expression => {
                let unit_letter =
                    self.expression_takes_unit_letter(value, self.local_preedit.len());
                let unit_separator = value == b'\''
                    && self.expression_takes_unit_separator(self.local_preedit.len());
                self.local_preedit.len() < GENERATED_MODE_INPUT_LIMIT
                    && (unit_letter
                        || unit_separator
                        || self
                            .local_mode
                            .spelling_symbols()
                            .as_bytes()
                            .contains(&value))
            }
            LocalInputMode::Command | LocalInputMode::Mention => {
                if self.local_preedit.len() == 1 && value.is_ascii_punctuation() {
                    return self.commit_bare_prefix(value);
                }
                self.local_preedit.len() < GENERATED_MODE_INPUT_LIMIT
                    && (value.is_ascii_lowercase()
                        || (value == b'\''
                            && self.command_takes_word_separator(self.local_preedit.len()))
                        || self.command_takes_argument_symbol(value, self.local_preedit.len()))
            }
            LocalInputMode::Url => {
                if !url::accepts(value) {
                    return KeyResult::unhandled();
                }
                self.local_preedit.len() < url::INPUT_LIMIT
            }
            LocalInputMode::None | LocalInputMode::TemporaryJapanese | LocalInputMode::Chaizi => {
                return KeyResult::unhandled()
            }
        };
        if !accepted {
            let entry = self.local_preedit.as_bytes();
            if entry.len() == 1
                && ((value.is_ascii_uppercase() && entry[0].is_ascii_uppercase())
                    || (letter && entry[0] == b'v'))
                && matches!(self.scheme(), SchemeType::Quanpin | SchemeType::Shuangpin)
            {
                let mut raw = self.local_preedit.clone();
                raw.push(value as char);
                self.reset_composition();
                self.engine.replace_active_raw_input(&raw, &raw);
                self.update_mixed_candidates();
                return KeyResult::handled();
            }
            let mut result = self.commit_raw();
            result.handled = false;
            return result;
        }
        self.invalidate_online_requests();
        self.local_preedit.push(value as char);
        KeyResult::handled().with_diagnostic(self.update_local_candidates())
    }

    pub fn handle_command(&mut self, command: Command) -> KeyResult {
        if command != Command::CommitCandidate {
            self.exit_date_time_menu();
        }
        self.end_uppercase_run();
        if let Some(result) = self.stroke_filter_command(command) {
            return result;
        }
        if !self.has_composition() {
            match command {
                Command::Backspace => {
                    self.punctuation.undo_last_quote();
                    self.step_overlay_learning_backspace();
                }
                _ => {
                    self.punctuation.clear_quote_undo();
                    self.last_overlay_learning = None;
                }
            }
            self.chain.reset();
            return KeyResult::unhandled();
        }
        if self.conversion.is_some() {
            if let Some(result) = self.handle_conversion_command(command) {
                return result;
            }
        }
        let command = match command {
            Command::ConversionLeft | Command::ConversionRight => {
                if let Some(result) = self.move_conversion(command) {
                    return result;
                }
                if command == Command::ConversionLeft {
                    Command::MoveLeft
                } else {
                    Command::MoveRight
                }
            }
            other => other,
        };
        match command {
            Command::MoveLeft
            | Command::MoveRight
            | Command::MoveHome
            | Command::MoveEnd
            | Command::DeleteForward => self.edit_at_caret(command),
            Command::Backspace => self.backspace(),
            Command::CommitCandidate => self.commit(0),
            Command::CommitRaw => self.commit_raw(),
            Command::Cancel => {
                self.reset_composition();
                self.chain.same_composition = false;
                KeyResult::handled()
            }
            Command::CycleKanaVariant
            | Command::CommitReading
            | Command::ConvertHanja
            | Command::ConversionLeft
            | Command::ConversionRight => KeyResult::unhandled(),
        }
    }

    pub fn conversion_left_from(&mut self, index: usize) -> KeyResult {
        self.exit_date_time_menu();
        if self.conversion.is_none() && self.has_composition() {
            if let Some(result) = self.move_conversion_from(Command::ConversionLeft, index) {
                return result;
            }
            return self.handle_command(Command::MoveLeft);
        }
        self.handle_command(Command::ConversionLeft)
    }

    pub fn handle_candidate_key(&mut self, value: u8) -> KeyResult {
        self.end_uppercase_run();
        if !self.has_composition() || !(b'1'..=b'9').contains(&value) {
            if !self.has_composition() {
                self.reset_commit_context();
            }
            return KeyResult::unhandled();
        }
        self.select_candidate(usize::from(value - b'1'))
    }

    pub fn handle_punctuation(&mut self, value: u8) -> KeyResult {
        self.last_overlay_learning = None;
        self.exit_date_time_menu();
        self.end_uppercase_run();
        if self.takes_literal_symbol(value) {
            return self.handle_character(value, false);
        }
        if matches!(
            self.local_mode,
            LocalInputMode::Command | LocalInputMode::Mention
        ) && self.local_preedit.len() == 1
        {
            return self.commit_bare_prefix(value);
        }
        if value == b'\'' && self.takes_local_separator() {
            return self.handle_character(value, false);
        }
        if self
            .local_mode
            .spelling_symbols()
            .as_bytes()
            .contains(&value)
        {
            return self.handle_character(value, false);
        }
        if self.url_entry(value).is_some() {
            return self.handle_character(value, false);
        }
        let ascii_marks = if self.dedicated_english {
            self.punctuation_lock != 1
        } else {
            !self.engine.current_scheme_type().uses_chinese_punctuation()
        };
        if ascii_marks && self.local_mode == LocalInputMode::None {
            if !self.has_composition() {
                self.reset_commit_context();
                return KeyResult::unhandled();
            }
            let mut result = self.finish_composition(0);
            self.chain.reset();
            result.handled = true;
            result
                .commit
                .get_or_insert_with(String::new)
                .push(char::from(value));
            return result;
        }
        let mark = if self.chinese_punctuation_enabled && self.punctuation_lock != 2 {
            self.punctuation.translate(value)
        } else {
            None
        };
        let Some(mark) = mark else {
            self.reset_commit_context();
            return KeyResult::unhandled();
        };
        let mut result = self.finish_composition(0);
        self.chain.reset();
        result.handled = true;
        let mut text = result.commit.take().unwrap_or_default();
        text.push_str(mark);
        result.commit = Some(text);
        result
    }

    pub(super) fn expression_takes_unit_letter(&self, value: u8, caret: usize) -> bool {
        let text = self.local_preedit.as_bytes();
        value.is_ascii_lowercase() && text[..caret.min(text.len())].iter().any(u8::is_ascii_digit)
    }

    pub(super) fn expression_takes_unit_separator(&self, caret: usize) -> bool {
        let text = self.local_preedit.as_bytes();
        let caret = caret.min(text.len());
        text[..caret].last().is_some_and(u8::is_ascii_lowercase) && !text.contains(&b'\'')
    }

    pub fn takes_local_separator(&self) -> bool {
        match self.local_mode {
            LocalInputMode::Expression => {
                self.expression_takes_unit_separator(self.caret_position())
            }
            LocalInputMode::Command => self.command_takes_word_separator(self.caret_position()),
            _ => false,
        }
    }

    pub(super) fn command_takes_word_separator(&self, caret: usize) -> bool {
        self.local_mode == LocalInputMode::Command
            && self
                .local_preedit
                .get(1..caret)
                .is_some_and(crate::local::command::takes_word_separator)
    }

    pub(super) fn command_takes_argument_symbol(&self, value: u8, caret: usize) -> bool {
        self.local_mode == LocalInputMode::Command
            && crate::local::command::extended_argument_symbol(value)
            && self.command_argument_symbols_at(caret)
    }

    fn command_argument_symbols_at(&self, caret: usize) -> bool {
        self.local_mode == LocalInputMode::Command
            && self
                .local_preedit
                .get(1..caret)
                .is_some_and(|code| self.queries.extended_plugin_trigger_typed(code))
    }

    fn commit_bare_prefix(&mut self, value: u8) -> KeyResult {
        let prefix = self.local_preedit.as_bytes()[0];
        self.reset_composition();
        self.chain.reset();
        let translate = self.chinese_punctuation_enabled && self.punctuation_lock != 2;
        let keys = [prefix, value];
        let translated = keys.map(|key| {
            if translate {
                self.punctuation.translate(key)
            } else {
                None
            }
        });
        let capacity = keys
            .into_iter()
            .zip(&translated)
            .map(|(key, mark)| mark.map_or_else(|| char::from(key).len_utf8(), str::len))
            .sum();
        let mut text = String::with_capacity(capacity);
        for (key, mark) in keys.into_iter().zip(translated) {
            match mark {
                Some(mark) => text.push_str(mark),
                None => text.push(char::from(key)),
            }
        }
        KeyResult::committed(text)
    }

    pub fn spelling_symbols(&self) -> String {
        let mut symbols = self.base_spelling_symbols();
        symbols.extend(self.literal_spelling_symbols());
        if self.stroke_filter_takes_tab() {
            symbols.push('\t');
        }
        symbols
    }

    fn base_spelling_symbols(&self) -> String {
        if self.dedicated_english {
            return String::new();
        }
        if self.command_argument_symbols_at(self.caret_position()) {
            let mut symbols = String::from("0123456789");
            symbols.push_str(crate::local::command::EXTENDED_ARGUMENT_SYMBOLS);
            return symbols;
        }
        if self.local_mode != LocalInputMode::None {
            return self.local_mode.spelling_symbols().to_owned();
        }
        if let Some(keys) = self.url_entry_keys() {
            return keys;
        }
        if self.has_composition() || !self.scheme().opens_table_modes() {
            return String::new();
        }
        (*b"/@")
            .into_iter()
            .filter(|key| self.local_mode_for_symbol(*key).is_some())
            .map(char::from)
            .collect()
    }

    pub fn set_command_table(&mut self, table: &[CommandTableEntry]) -> Option<String> {
        self.queries.set_command_table(table);
        (self.local_mode == LocalInputMode::Command)
            .then(|| self.update_local_candidates())
            .flatten()
    }

    pub fn set_quick_phrase_table(&mut self, table: &[QuickPhraseEntry]) -> Option<String> {
        self.queries.set_quick_phrase_table(table);
        (self.local_mode == LocalInputMode::QuickPhrase)
            .then(|| self.update_local_candidates())
            .flatten()
    }

    pub fn set_mention_entries(&mut self, entries: &[MentionEntry]) -> Option<String> {
        self.queries.set_mentions(entries);
        if self.local_mode == LocalInputMode::Mention {
            self.update_local_candidates()
        } else {
            if self.local_mode == LocalInputMode::None && self.has_composition() {
                self.update_mixed_candidates();
            }
            None
        }
    }

    pub fn set_mention_places(&mut self, enabled: bool) -> Option<String> {
        self.queries.set_mention_places(enabled);
        (self.local_mode == LocalInputMode::Mention)
            .then(|| self.update_local_candidates())
            .flatten()
    }

    pub fn has_composition(&self) -> bool {
        if self.dedicated_english {
            return !self.dedicated_english_preedit.is_empty();
        }
        if self.local_mode != LocalInputMode::None {
            return !self.local_preedit.is_empty();
        }
        !self.engine.preedit().is_empty()
    }

    pub fn scheme(&self) -> SchemeType {
        self.engine.current_scheme_type()
    }

    pub fn preedit(&self) -> String {
        if self.dedicated_english {
            return self.dedicated_english_preedit.clone();
        }
        if self.local_mode != LocalInputMode::None {
            return self.local_preedit.clone();
        }
        self.engine.preedit().to_owned()
    }

    pub fn raw_segmentation(&self) -> String {
        if self.dedicated_english {
            return self.dedicated_english_preedit.clone();
        }
        if self.local_mode != LocalInputMode::None {
            return self.local_preedit.clone();
        }
        self.engine.request().raw_segmentation.clone()
    }

    pub fn normalized_segmentation(&self) -> String {
        if self.dedicated_english {
            return self.dedicated_english_preedit.clone();
        }
        if self.local_mode != LocalInputMode::None {
            return self.local_preedit.clone();
        }
        self.engine.request().normalized_segmentation.clone()
    }

    pub fn candidates(&self) -> &[WordItem] {
        if let Some(edit) = &self.conversion {
            return edit.rows();
        }
        if self.dedicated_english {
            return &self.dedicated_english_candidates;
        }
        if self.local_mode != LocalInputMode::None {
            return &self.local_candidates;
        }
        if let Some(page) = &self.date_time_menu_page {
            return page;
        }
        &self.mixed_candidates
    }

    pub fn switch_scheme(&mut self, scheme: SchemeType) -> Result<()> {
        self.engine.activate(scheme)?;
        self.reset_composition();
        self.chain.reset();
        self.engine.switch_scheme(scheme)?;
        self.update_mixed_candidates();
        Ok(())
    }

    pub fn set_dedicated_english_mode(&mut self, enabled: bool) {
        if self.dedicated_english == enabled {
            return;
        }
        self.reset_composition();
        self.chain.reset();
        self.dedicated_english = enabled;
    }

    pub fn set_personal_context_enabled(&mut self, enabled: bool) {
        if self.personal_context_enabled == enabled {
            return;
        }
        self.personal_context_enabled = enabled;
        if !enabled {
            self.reset_commit_context();
        }
    }

    pub fn set_cloud_candidates_enabled(&mut self, enabled: bool) {
        if self.cloud_candidates_enabled == enabled {
            return;
        }
        self.cloud_candidates_enabled = enabled;
        self.update_mixed_candidates();
    }

    pub fn set_english_online_candidates_enabled(&mut self, enabled: bool) {
        if self.english_online_candidates_enabled == enabled {
            return;
        }
        self.english_online_candidates_enabled = enabled;
        self.invalidate_online_requests();
    }

    pub(super) fn invalidate_online_requests(&mut self) {
        self.online_requests.invalidate();
        self.english_online_rows.clear();
        self.dedicated_english_candidates
            .retain(|row| !row.source.is_online());
        self.local_candidates.retain(|row| !row.source.is_online());
    }

    pub fn set_rescoring_context(&mut self, context: &str) {
        self.engine.set_rescoring_context(context);
    }

    pub fn set_first_page_rows(&mut self, rows: FirstPageRows) {
        self.first_page_rows = rows.get();
        self.engine.set_first_page_rows(rows);
    }

    pub(super) fn enter_date_time_menu(&mut self, keyword: &str) {
        let items = crate::local::date_time::query_date_time(keyword, &self.local_now());
        if !items.is_empty() {
            self.date_time_menu_page = Some(items);
        }
    }

    pub(super) fn exit_date_time_menu(&mut self) {
        self.date_time_menu_page = None;
    }

    pub(super) fn date_time_menu_keyword_at(&self, index: usize) -> Option<String> {
        let item = self.candidates().get(index)?;
        crate::local::date_time::is_date_time_menu_item(item)
            .then(|| crate::local::date_time::date_time_menu_keyword(item).to_owned())
    }

    pub fn reset_composition(&mut self) {
        self.date_time_menu_page = None;
        self.caret = None;
        self.conversion = None;
        self.prefix_candidates.clear();
        self.prefix_query_input.clear();
        self.prefix_active = false;
        self.caret_edit = CaretEdit::default();
        self.phrase_progress = CreatingWordProgress::default();
        self.pending_sequence = None;
        self.pending_sequence_with_cases = None;
        self.invalidate_online_requests();
        self.local_mode = LocalInputMode::None;
        self.local_preedit.clear();
        self.local_candidates.clear();
        self.dedicated_english_preedit.clear();
        self.dedicated_english_candidates.clear();
        self.mixed_candidates.clear();
        self.personal_reranked = false;
        self.ranking_candidates = None;
        self.stroke_filter = None;
        self.engine.reset();
    }

    pub fn reset_commit_context(&mut self) {
        self.end_uppercase_run();
        self.chain.reset();
        if self.personal_reranked {
            self.update_mixed_candidates();
        }
    }

    pub fn reset_cache(&mut self) {
        self.engine.reset_cache();
        self.prefix_query_input.clear();
        self.caret_edit.prefix_input.clear();
    }

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        self.engine.clear_online_candidates(source);
        self.online_requests.invalidate();
        self.english_online_rows.retain(|row| row.source != source);
        self.dedicated_english_candidates
            .retain(|row| row.source != source);
        self.local_candidates.retain(|row| row.source != source);
        if !self.dedicated_english && self.local_mode == LocalInputMode::None {
            self.update_mixed_candidates();
        }
    }

    pub(super) fn expand_initial_candidates(&mut self) -> bool {
        self.exit_date_time_menu();
        if self.conversion.is_some() {
            return false;
        }
        self.refresh_prefix_candidates();
        let grew = if self.prefix_active && self.caret_edit.active {
            let start = self.caret_edit_suffix_start();
            let end = self.raw_with_cases().len();
            self.engine.expand_current_raw_range_initial_candidates(
                &self.prefix_query_input,
                start..end,
                &mut self.prefix_candidates,
            )
        } else if self.prefix_active {
            self.engine.expand_current_raw_prefix_initial_candidates(
                &self.prefix_query_input,
                self.prefix_end(),
                &mut self.prefix_candidates,
            )
        } else {
            self.engine.expand_initial_candidates()
        };
        if !grew {
            return false;
        }
        self.update_mixed_candidates();
        true
    }

    pub(super) fn steady_now(&self) -> Instant {
        (self.clock.steady)()
    }

    pub(super) fn local_now(&self) -> LocalDateTime {
        (self.clock.local)()
    }

    pub(super) fn shuangpin_profile(&self) -> &'static ShuangpinProfile {
        self.profile
    }

    pub(super) fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    pub(super) fn is_shuangpin(&self) -> bool {
        self.engine.current_scheme_type() == SchemeType::Shuangpin
    }

    pub(super) fn pinyin_ranking_context(&self) -> Cow<'_, str> {
        let request = self.engine.request();
        if request.normalized_segmentation.is_empty() {
            Cow::Owned(request.segmentation.clone())
        } else {
            Cow::Borrowed(&request.normalized_segmentation)
        }
    }

    pub(super) fn discard_abandoned_phrase_progress(&mut self) {
        if !self.has_composition() {
            self.phrase_progress = CreatingWordProgress::default();
            self.caret_edit.held.clear();
            self.chain.same_composition = false;
        }
    }

    fn set_autocorrect_types(&mut self, types: u32) {
        if self.autocorrect_types == types {
            return;
        }
        self.autocorrect_types = types;
        self.engine.set_autocorrect_types(types);
        self.update_mixed_candidates();
    }

    fn set_local_mode_options(&mut self, options: LocalModeOptions) {
        self.local_mode_options = options;
        if options.expression {
            crate::local::units::warm_up_in_background();
        }
        if self.local_mode != LocalInputMode::None && !local_mode_enabled(options, self.local_mode)
        {
            self.reset_composition();
        }
    }

    fn local_mode_for_entry(&self, value: u8) -> Option<LocalInputMode> {
        let options = self.local_mode_options;
        if !options.shift_letter_entry {
            return None;
        }
        let (mode, enabled) = match value {
            b'U' => (LocalInputMode::Unicode, options.unicode),
            b'T' => (LocalInputMode::DateTime, options.date_time),
            b'K' => (LocalInputMode::QuickPhrase, options.quick_phrase),
            b'E' => (LocalInputMode::Emoji, options.emoji),
            b'M' => (LocalInputMode::Kaomoji, options.kaomoji),
            b'J' => (LocalInputMode::SuperJianpin, options.super_jianpin),
            b'Y' => (LocalInputMode::TemporaryEnglish, options.temporary_english),
            b'V' => (LocalInputMode::Expression, options.expression),
            b'S' => (LocalInputMode::Symbol, options.symbol),
            _ => return None,
        };
        let scheme_opens = if matches!(
            mode,
            LocalInputMode::QuickPhrase
                | LocalInputMode::Unicode
                | LocalInputMode::DateTime
                | LocalInputMode::TemporaryEnglish
                | LocalInputMode::Symbol
        ) {
            self.scheme().opens_table_modes()
        } else {
            self.scheme().opens_local_modes()
        };
        (enabled && scheme_opens).then_some(mode)
    }

    fn local_mode_for_lowercase_v(&self, value: u8) -> Option<LocalInputMode> {
        let options = self.local_mode_options;
        (value == b'v'
            && options.expression
            && options.expression_lowercase_entry
            && self.scheme() == SchemeType::Quanpin)
            .then_some(LocalInputMode::Expression)
    }

    fn local_mode_for_symbol(&self, value: u8) -> Option<LocalInputMode> {
        if !self.scheme().opens_table_modes()
            || !self.chinese_punctuation_enabled
            || self.punctuation_lock == 2
        {
            return None;
        }
        let options = self.local_mode_options;
        let (mode, enabled) = match value {
            b'/' => (LocalInputMode::Command, options.command),
            b'@' => (LocalInputMode::Mention, options.mention),
            _ => return None,
        };
        enabled.then_some(mode)
    }

    fn url_entry_ready(&self) -> bool {
        self.local_mode_options.url
            && !self.dedicated_english
            && self.local_mode == LocalInputMode::None
            && self.scheme().detects_urls()
            && self.has_composition()
            && self
                .caret
                .is_none_or(|caret| caret >= self.editing_text_len())
    }

    fn url_entry_keys(&self) -> Option<String> {
        if !self.url_entry_ready() {
            return None;
        }
        Some(url::entry_keys(self.raw_with_cases())).filter(|keys| !keys.is_empty())
    }

    fn url_entry(&self, value: u8) -> Option<String> {
        if !self.url_entry_ready() {
            return None;
        }
        let raw = self.raw_with_cases();
        url::opens(raw, value).then(|| format!("{raw}{}", char::from(value)))
    }

    pub(super) fn enter_url_mode(&mut self, text: String) {
        self.reset_composition();
        self.local_mode = LocalInputMode::Url;
        self.local_preedit = text;
        self.chain.reset();
        self.add_local_fallback_candidate();
        self.add_url_completions();
    }

    pub(super) fn add_url_completions(&mut self) {
        let completions = url::email_completions(&self.local_preedit)
            .into_iter()
            .chain(url::tld_completions(&self.local_preedit));
        self.local_candidates.extend(
            completions.map(|text| WordItem::new("", text, 0, CandidateSource::Fallback, "")),
        );
    }

    pub(super) fn insert_url_scheme_completions(&self, items: &mut Vec<WordItem>) {
        if !self.url_entry_ready() {
            return;
        }
        let at = usize::from(items.first().is_some_and(|item| {
            item.source.is_dictionary() || item.source == CandidateSource::EnglishDictionary
        }));
        let completions = url::scheme_completions(self.raw_with_cases())
            .map(|text| WordItem::new("", text, 0, CandidateSource::Fallback, ""));
        items.splice(at..at, completions);
    }

    pub(super) fn is_url_completion(&self, item: &WordItem) -> bool {
        if item.source != CandidateSource::Fallback {
            return false;
        }
        match self.local_mode {
            LocalInputMode::None => {
                self.url_entry_ready()
                    && url::scheme_completions(self.raw_with_cases()).any(|text| text == item.word)
            }
            LocalInputMode::Url => item.word != self.local_preedit,
            _ => false,
        }
    }

    pub(super) fn url_continuation(&self, index: usize) -> Option<String> {
        let item = self.candidates().get(index)?;
        let continues = match self.local_mode {
            LocalInputMode::None => self.is_url_completion(item),
            LocalInputMode::Url => {
                item.source == CandidateSource::Fallback
                    && url::tld_completions(&self.local_preedit).contains(&item.word)
            }
            _ => false,
        };
        continues.then(|| item.word.to_string())
    }

    fn enter_local_mode(&mut self, mode: LocalInputMode, letter: u8) -> Option<String> {
        self.local_mode = mode;
        self.local_preedit = (letter as char).to_string();
        self.local_candidates.clear();
        self.chain.reset();
        if matches!(mode, LocalInputMode::Command | LocalInputMode::Mention) {
            return self.update_local_candidates();
        }
        self.add_local_fallback_candidate();
        None
    }

    fn backspace(&mut self) -> KeyResult {
        if self
            .caret
            .is_some_and(|caret| caret < self.editing_text_len())
        {
            return self.edit_at_caret(Command::Backspace);
        }
        self.caret = None;
        if self.dedicated_english {
            self.invalidate_online_requests();
            self.dedicated_english_preedit.pop();
            self.update_dedicated_english_candidates();
            return KeyResult::handled();
        }
        if self.local_mode == LocalInputMode::Url {
            return self.edit_at_caret(Command::Backspace);
        }
        if self.local_mode != LocalInputMode::None {
            if self.local_preedit.len() <= 1 {
                self.reset_composition();
                return KeyResult::handled();
            }
            self.invalidate_online_requests();
            self.local_preedit.pop();
            return KeyResult::handled().with_diagnostic(self.update_local_candidates());
        }
        self.engine.handle_key(SchemeKey::Backspace);
        self.update_mixed_candidates();
        self.discard_abandoned_phrase_progress();
        self.invalidate_online_requests();
        KeyResult::handled()
    }

    pub(super) fn url_reverts(&self, remaining: &str, removed: char) -> bool {
        u8::try_from(removed).is_ok_and(|key| url::opens(remaining, key))
    }

    pub(super) fn restore_composition_from_url(&mut self, letters: String) {
        self.reset_composition();
        self.engine.replace_active_raw_input(&letters, &letters);
        self.invalidate_online_requests();
        self.update_mixed_candidates();
        if self.raw_with_cases() != letters {
            self.enter_url_mode(letters);
        }
    }

    pub(super) fn raw_commit_text(&self) -> String {
        self.preedit()
    }

    pub(super) fn commit_raw(&mut self) -> KeyResult {
        let raw = self.raw_commit_text();
        let mut diagnostic = None;
        if self.learn_english_on_enter
            && self.dedicated_english
            && learn_entered_english_word(
                &self.paths.dictionary(assets::ENGLISH_DICTIONARY),
                self.journal_path(),
                self.local_mode.without_entry_marker(&raw),
                ENTERED_ENGLISH_WORD_WEIGHT,
            )
            .is_err()
        {
            diagnostic = Some(diagnostics::ENGLISH_WORD_NOT_LEARNED.to_owned());
        }
        if diagnostic.is_none() {
            diagnostic = self.learn_rejected_correction();
        }
        self.reset_composition();
        self.chain.reset();
        KeyResult::committed(raw).with_diagnostic(diagnostic)
    }

    pub(super) fn is_editable_source(&self, item: &WordItem) -> bool {
        item.source == CandidateSource::EnglishDictionary
            || (item.source.is_dictionary() && self.scheme().is_pinyin())
    }
}

fn frequency_options_valid(options: FrequencyAdjustmentOptions) -> bool {
    (1..=10).contains(&options.trigger_count) && (1..=10).contains(&options.linear_step)
}

fn english_options_valid(options: EnglishInputOptions) -> bool {
    (1..=8).contains(&options.minimum_prefix) && (1..=3).contains(&options.first_page_count)
}

fn local_mode_enabled(options: LocalModeOptions, mode: LocalInputMode) -> bool {
    match mode {
        LocalInputMode::None => true,
        LocalInputMode::Unicode => options.unicode,
        LocalInputMode::DateTime => options.date_time,
        LocalInputMode::QuickPhrase => options.quick_phrase,
        LocalInputMode::Emoji => options.emoji,
        LocalInputMode::Kaomoji => options.kaomoji,
        LocalInputMode::SuperJianpin => options.super_jianpin,
        LocalInputMode::TemporaryEnglish => options.temporary_english,
        LocalInputMode::Expression => options.expression,
        LocalInputMode::Command => options.command,
        LocalInputMode::Mention => options.mention,
        LocalInputMode::Symbol => options.symbol,
        LocalInputMode::Url => options.url,
        LocalInputMode::TemporaryJapanese | LocalInputMode::Chaizi => false,
    }
}
