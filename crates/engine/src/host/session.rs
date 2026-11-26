//! 宿主门面 `Session` 包装器：ASCII 校验、命令映射、回车学英文词策略与扁平化快照导出。

use std::marker::PhantomData;
use std::path::PathBuf;
use std::rc::Rc;

use super::options::{runtime_paths, session_options, EngineOptions};
use crate::assets;
use crate::diagnostics;
use crate::error::{EngineError, Result};
use crate::local::database::LocalDatabaseLease;
use crate::pinyin::segment::is_complete_pinyin_input;
use crate::types::{
    CandidateEdge, CandidateSource, CommandTableEntry, CommandTranslationQuery, FirstPageRows,
    KeyResult, LocalInputMode, MentionEntry, OnlineQuery, QuickPhraseEntry, SchemeType,
};
use crate::user_dictionary::ngram_store::flush_journal;
use crate::user_dictionary::removal::learn_entered_english_word;

const ENTERED_ENGLISH_WORD_WEIGHT: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    Backspace = 0,
    CommitCandidate = 1,
    CommitRaw = 2,
    Cancel = 3,
    MoveLeft = 4,
    MoveRight = 5,
    MoveHome = 6,
    MoveEnd = 7,
    DeleteForward = 8,
    CycleKanaVariant = 9,
    CommitReading = 10,
    CommitRawWithoutLearning = 11,
    ConvertHanja = 12,
    ConversionLeft = 13,
    ConversionRight = 14,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EngineSnapshot {
    pub prediction: bool,
    pub local_mode: String,
    pub spelling_symbols: String,
    pub dedicated_english: bool,
    pub microsoft_shuangpin: bool,
    pub shuangpin_profile: String,
    pub preedit: String,
    pub reading: String,
    pub display_reading: String,
    pub editing_text: String,
    pub caret_position: usize,
    pub segment_raw_boundaries: Vec<u64>,
    pub candidates: Vec<crate::CandidateEntry>,
    pub scheme: u8,
    pub conversion: String,
    pub conversion_focus_start: usize,
    pub conversion_focus_end: usize,
    pub caret_edit_suffix: bool,
    pub caret_edit_prefix: String,
}

impl EngineSnapshot {
    /// 按展示顺序借用全部候选文本切片，便于轻量断言与旧调用迁移。
    pub fn candidate_texts(&self) -> Vec<&str> {
        self.candidates
            .iter()
            .map(|entry| entry.text.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineResult {
    pub handled: bool,
    pub has_commit: bool,
    pub commit: String,
    pub diagnostic: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OnlineQuerySnapshot {
    pub available: bool,
    pub mode: crate::OnlineInputMode,
    pub scheme: u8,
    pub generation: u64,
    pub identity: String,
    pub query_text: String,
    pub cache_key: String,
    pub pinyin_segments: Vec<String>,
    pub cloud_eligible: bool,
    pub ai_eligible: bool,
    pub session_id: u64,
}

pub struct Session {
    inner: crate::session::Session,
    options: EngineOptions,
    microsoft_shuangpin: bool,
    shuangpin_profile: String,
    _local_databases: LocalDatabaseLease,
    _thread_confined: PhantomData<Rc<()>>,
}

impl Session {
    pub fn set_prediction_allowed(&mut self, allowed: bool) {
        self.inner.set_prediction_allowed(allowed);
    }

    pub fn new(options: &EngineOptions) -> Result<Session> {
        let inner = crate::session::Session::new(session_options(options)?)?;
        let profile = inner.shuangpin_layout();
        Ok(Session {
            inner,
            options: options.clone(),
            microsoft_shuangpin: options.scheme == SchemeType::Shuangpin as u8
                && profile.uses_semicolon_key(),
            shuangpin_profile: profile.kind.name().to_owned(),
            _local_databases: LocalDatabaseLease::new([
                PathBuf::from(&options.dictionaries),
                PathBuf::from(&options.resources),
            ]),
            _thread_confined: PhantomData,
        })
    }

    pub fn snapshot(&self) -> Result<EngineSnapshot> {
        let value = self.inner.snapshot_for_host();
        let count = value.candidates.len();
        let pinned_word = self.inner.pinned_candidate_word();
        let mut raw_entries = Vec::with_capacity(count);
        let mut annotations = value.candidate_annotations.into_iter();
        for (index, candidate) in value.candidates.into_iter().enumerate() {
            let annotation = annotations
                .next()
                .unwrap_or_else(|| candidate.corrected_from.to_string());
            let source = candidate.source as u8;
            let kind = crate::CandidateKind::from_source(source);
            let corrected = !candidate.corrected_from.is_empty();
            let answers_key = value
                .candidate_answers_key
                .get(index)
                .copied()
                .unwrap_or(false);
            let pinned_first =
                candidate.pinned_first || pinned_word.as_deref() == Some(candidate.word.as_str());
            let static_evidence = candidate.static_evidence();
            let keyboard_runner_up = candidate.keyboard_runner_up;
            let code = candidate.pinyin.into_string();
            let text = candidate.word.into_string();
            let mut entry = crate::CandidateEntry::new(text, code, source);
            entry.kind = kind;
            entry.annotation = annotation;
            entry.corrected = corrected;
            entry.answers_key = answers_key;
            entry.pinned_first = pinned_first;
            entry.tier0_gram_log_prob =
                static_evidence.map(|ev| ev.log_prob_milli_nats as f64 / 1000.0);
            entry.static_evidence = static_evidence;
            entry.keyboard_runner_up = keyboard_runner_up;
            // 在召回池去重与后续重排之前绑定底层原始行号，确保选词/置顶/删除永远命中真实引擎条目。
            entry.origin = crate::CandidateOrigin::EngineRow(index);
            raw_entries.push(entry);
        }
        let mut output = EngineSnapshot {
            prediction: value.prediction,
            local_mode: value.local_mode.name().to_owned(),
            spelling_symbols: value.spelling_symbols,
            dedicated_english: value.dedicated_english,
            microsoft_shuangpin: self.microsoft_shuangpin,
            shuangpin_profile: self.shuangpin_profile.clone(),
            preedit: value.preedit,
            display_reading: String::new(),
            reading: String::new(),
            editing_text: value.editing_text,
            caret_position: value.caret_position,
            segment_raw_boundaries: self
                .inner
                .segment_raw_boundaries()
                .into_iter()
                .map(|boundary| boundary as u64)
                .collect(),
            candidates: crate::RecallPool::from_candidates(raw_entries).into_entries(),
            scheme: value.scheme as u8,
            conversion: value.conversion,
            conversion_focus_start: value.conversion_focus.0,
            conversion_focus_end: value.conversion_focus.1,
            caret_edit_suffix: value.caret_edit_suffix,
            caret_edit_prefix: value.caret_edit_prefix,
        };
        // 清理未使用但保留结构的空字段初始化警告占位，确保编译器零告警。
        let _ = &mut output;
        Ok(output)
    }

    pub fn online_query(&self) -> Result<OnlineQuerySnapshot> {
        let Some(query) = self.inner.online_query() else {
            return Ok(OnlineQuerySnapshot::default());
        };
        Ok(OnlineQuerySnapshot {
            available: true,
            mode: query.mode,
            scheme: query.scheme as u8,
            generation: query.generation,
            identity: query.identity,
            query_text: query.query_text,
            cache_key: query.cache_key,
            pinyin_segments: query.pinyin_segments,
            cloud_eligible: query.cloud_eligible,
            ai_eligible: query.ai_eligible,
            session_id: query.session_id,
        })
    }

    pub fn neural_v2_metadata(&self) -> Option<crate::neural_v2::EngineNeuralMetadata> {
        self.inner.neural_v2_metadata()
    }

    pub fn reset_cache(&mut self) {
        self.inner.reset_cache();
    }

    pub fn clear_online_candidates(&mut self, source: u8) -> Result<()> {
        let source = match source {
            0 => CandidateSource::CloudSuggestion,
            1 => CandidateSource::AiSuggestion,
            _ => return Err(EngineError::invalid("invalid online candidate source")),
        };
        self.inner.clear_online_candidates(source);
        Ok(())
    }

    pub fn cloud_candidates_enabled(&self) -> bool {
        self.inner.cloud_candidates_enabled()
    }

    pub fn set_cloud_candidates_enabled(&mut self, enabled: bool) {
        self.inner.set_cloud_candidates_enabled(enabled);
    }

    pub fn english_online_candidates_enabled(&self) -> bool {
        self.inner.english_online_candidates_enabled()
    }

    pub fn set_english_online_candidates_enabled(&mut self, enabled: bool) {
        self.inner.set_english_online_candidates_enabled(enabled);
    }

    pub fn set_caret(&mut self, caret: Option<usize>) {
        self.inner.set_caret(caret);
    }

    pub fn prefix_end(&self) -> usize {
        self.inner.prefix_end()
    }

    pub fn pending_suffix(&self) -> String {
        self.inner.pending_suffix()
    }

    pub fn reset_context(&mut self) {
        self.inner.reset_context();
    }

    pub fn set_command_table(&mut self, table: &[CommandTableEntry]) -> Result<()> {
        match self.inner.set_command_table(table) {
            Some(diagnostic) => Err(EngineError::failed(&diagnostic)),
            None => Ok(()),
        }
    }

    pub fn set_quick_phrase_table(&mut self, table: &[QuickPhraseEntry]) -> Result<()> {
        match self.inner.set_quick_phrase_table(table) {
            Some(diagnostic) => Err(EngineError::failed(&diagnostic)),
            None => Ok(()),
        }
    }

    pub fn set_mention_entries(&mut self, entries: &[MentionEntry]) -> Result<()> {
        match self.inner.set_mention_entries(entries) {
            Some(diagnostic) => Err(EngineError::failed(&diagnostic)),
            None => Ok(()),
        }
    }

    pub fn set_mention_places(&mut self, enabled: bool) -> Result<()> {
        match self.inner.set_mention_places(enabled) {
            Some(diagnostic) => Err(EngineError::failed(&diagnostic)),
            None => Ok(()),
        }
    }

    pub fn set_rescoring_context(&mut self, context: &str) {
        self.inner.set_rescoring_context(context);
    }

    pub fn set_first_page_rows(&mut self, rows: usize) {
        self.inner.set_first_page_rows(FirstPageRows::new(rows));
    }

    pub fn takes_local_separator(&self) -> bool {
        self.inner.takes_local_separator()
    }

    pub fn command_translation_query(&self) -> Option<CommandTranslationQuery> {
        self.inner.command_translation_query()
    }

    pub fn apply_command_translation(
        &mut self,
        query: &CommandTranslationQuery,
        translation: &str,
    ) -> bool {
        self.inner.apply_command_translation(query, translation)
    }

    pub fn apply_online_candidate(
        &mut self,
        query: &OnlineQuerySnapshot,
        candidate: &str,
        source: u8,
    ) -> Result<bool> {
        let Some((request, source)) = online_request(query, source) else {
            return Ok(false);
        };
        Ok(self
            .inner
            .apply_online_candidate(&request, candidate, source))
    }

    pub fn apply_online_candidates(
        &mut self,
        query: &OnlineQuerySnapshot,
        candidates: &[String],
        source: u8,
    ) -> Result<bool> {
        let Some((request, source)) = online_request(query, source) else {
            return Ok(false);
        };
        Ok(self
            .inner
            .apply_online_candidates(&request, candidates, source))
    }

    pub fn character(&mut self, value: u8, shift: bool) -> Result<EngineResult> {
        require_ascii(value, diagnostics::CHARACTER_MUST_BE_ASCII)?;
        Ok(result_for(self.inner.character(value, shift)))
    }

    pub fn expand_initial_candidates(&mut self) -> Result<bool> {
        Ok(self.inner.expand_initial_candidates())
    }

    pub fn command(&mut self, command: Command) -> Result<EngineResult> {
        match command {
            Command::CommitRaw => Ok(self.commit_raw_with_policy()),
            Command::CommitRawWithoutLearning => Ok(self.commit_raw_without_learning()),
            Command::ConvertHanja => Ok(result_for(
                self.inner.command(crate::types::Command::ConvertHanja),
            )),
            Command::ConversionLeft => Ok(result_for(
                self.inner.command(crate::types::Command::ConversionLeft),
            )),
            Command::ConversionRight => Ok(result_for(
                self.inner.command(crate::types::Command::ConversionRight),
            )),
            _ => {
                let engine = crate::types::Command::from_u8(command as u8)
                    .ok_or_else(|| EngineError::invalid(diagnostics::UNSUPPORTED_INPUT_COMMAND))?;
                Ok(result_for(self.inner.command(engine)))
            }
        }
    }

    pub fn conversion_left_from(&mut self, index: usize) -> Result<EngineResult> {
        Ok(result_for(self.inner.conversion_left_from(index)))
    }

    pub fn select(&mut self, index: usize) -> Result<EngineResult> {
        Ok(result_for(self.inner.select(index)))
    }

    pub fn learn_injected_candidate(&mut self, canonical_pinyin: &str, word: &str) -> Result<()> {
        self.inner.learn_injected_candidate(canonical_pinyin, word)
    }

    pub fn pin_candidate(&mut self, index: usize) -> Result<EngineResult> {
        Ok(result_for(self.inner.pin(index)))
    }

    pub fn remove_candidate(&mut self, index: usize) -> Result<EngineResult> {
        Ok(result_for(self.inner.remove(index)))
    }

    pub fn select_edge(&mut self, index: usize, edge: CandidateEdge) -> Result<EngineResult> {
        Ok(result_for(self.inner.select_edge(index, edge)))
    }

    pub fn finish(&mut self, index: usize) -> Result<EngineResult> {
        Ok(result_for(self.inner.finish(index)))
    }

    pub fn punctuation(&mut self, value: u8) -> Result<EngineResult> {
        require_ascii(value, diagnostics::PUNCTUATION_MUST_BE_ASCII)?;
        Ok(result_for(self.inner.punctuation(value)))
    }

    pub fn balance_paired_punctuation_after_auto_close(&mut self, opening: u8) -> Result<()> {
        require_ascii(opening, diagnostics::PAIRED_OPENING_MUST_BE_ASCII)?;
        self.inner
            .balance_paired_punctuation_after_auto_close(opening);
        Ok(())
    }

    pub fn set_chinese_punctuation_enabled(&mut self, enabled: bool) -> Result<()> {
        self.inner.set_chinese_punctuation_enabled(enabled);
        Ok(())
    }

    pub fn set_punctuation_lock(&mut self, lock: u8) -> Result<()> {
        self.inner.set_punctuation_lock(i32::from(lock))
    }

    pub fn set_paired_punctuation_enabled(&mut self, enabled: bool) -> Result<()> {
        self.inner.set_paired_punctuation_enabled(enabled);
        Ok(())
    }

    pub fn set_dedicated_english(&mut self, enabled: bool) -> Result<()> {
        self.inner.set_dedicated_english(enabled);
        Ok(())
    }

    fn commit_raw_without_learning(&mut self) -> EngineResult {
        let before = self.inner.snapshot();
        if !before.dedicated_english {
            return result_for(self.inner.command(crate::types::Command::CommitRaw));
        }
        let raw = before.preedit;
        self.inner.command(crate::types::Command::Cancel);
        result_for(KeyResult::committed(raw))
    }

    fn commit_raw_with_policy(&mut self) -> EngineResult {
        let before = self.inner.snapshot();
        let chinese_scheme = before.scheme.learns_english_words();
        let complete_pure_pinyin = chinese_scheme && {
            let segmentation = if before.normalized_segmentation.is_empty() {
                &before.raw_segmentation
            } else {
                &before.normalized_segmentation
            };
            !segmentation.is_empty() && is_complete_pinyin_input(segmentation)
        };
        let should_learn = self.options.learn_english_on_enter
            && before.conversion.is_empty()
            && !before.local_mode.generates_text()
            && before.local_mode != LocalInputMode::Url
            && (before.dedicated_english
                || before.local_mode != LocalInputMode::None
                || (chinese_scheme && !complete_pure_pinyin));
        let mut result = self.inner.command(crate::types::Command::CommitRaw);
        if let Some(commit) = result.commit.as_deref().filter(|_| should_learn) {
            let text = before.local_mode.without_entry_marker(commit);
            if !text.is_empty() {
                let paths = runtime_paths(&self.options);
                if learn_entered_english_word(
                    &paths.dictionary(assets::ENGLISH_DICTIONARY),
                    &paths.user(assets::USER_JOURNAL),
                    text,
                    ENTERED_ENGLISH_WORD_WEIGHT,
                )
                .is_err()
                {
                    result.diagnostic = Some(diagnostics::ENGLISH_WORD_NOT_LEARNED.to_owned());
                }
            }
        }
        result_for(result)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let journal = runtime_paths(&self.options).user(assets::USER_JOURNAL);
        let _ = flush_journal(&journal);
        crate::user_dictionary::journal::release_thread_journal();
    }
}

pub fn local_mode_counts_as_typing(local_mode: &str) -> bool {
    LocalInputMode::from_name(local_mode).is_none_or(|mode| !mode.generates_text())
}

fn result_for(value: KeyResult) -> EngineResult {
    EngineResult {
        handled: value.handled,
        has_commit: value.commit.is_some(),
        commit: value.commit.unwrap_or_default(),
        diagnostic: value.diagnostic.unwrap_or_default(),
    }
}

fn require_ascii(value: u8, message: &str) -> Result<()> {
    if value > 127 {
        return Err(EngineError::invalid(message));
    }
    Ok(())
}

fn online_request(
    query: &OnlineQuerySnapshot,
    source: u8,
) -> Option<(OnlineQuery, CandidateSource)> {
    if !query.available {
        return None;
    }
    let source = match source {
        0 => CandidateSource::CloudSuggestion,
        1 => CandidateSource::AiSuggestion,
        _ => return None,
    };
    let request = OnlineQuery {
        mode: query.mode,
        scheme: SchemeType::from_u8(query.scheme)?,
        generation: query.generation,
        identity: query.identity.clone(),
        query_text: query.query_text.clone(),
        cache_key: query.cache_key.clone(),
        pinyin_segments: query.pinyin_segments.clone(),
        cloud_eligible: query.cloud_eligible,
        ai_eligible: query.ai_eligible,
        session_id: query.session_id,
    };
    Some((request, source))
}
