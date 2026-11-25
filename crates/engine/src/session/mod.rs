//! 公开会话 `Session`：承载全拼、双拼、英文补全、本地表达式与个人学习管线。

mod candidates;
mod caret_edit;
mod chain;
mod clock;
mod commit;
mod composition;
mod conversion;
mod editing;
mod input;
mod learning;
mod literal;
mod online;
pub mod options;
mod prediction;
#[cfg(test)]
mod tests;

use crate::diagnostics;
use crate::error::{EngineError, Result};
use crate::types::{
    CandidateEdge, CandidateSource, Command, CommandTableEntry, CommandTranslationQuery,
    FirstPageRows, KeyResult, LocalInputMode, MentionEntry, OnlineQuery, QuickPhraseEntry,
    SchemeType, WordItem,
};

pub use clock::Clock;
use input::InputSession;
pub use options::{SessionOptions, SessionSnapshot};

pub struct Session {
    input: InputSession,
    shuangpin_preedit_uses_raw: bool,
    prediction: prediction::Prediction,
}

impl Session {
    pub fn new(options: SessionOptions) -> Result<Session> {
        options.paths.validate()?;
        let input = InputSession::new(&options)?;
        Ok(Session {
            input,
            shuangpin_preedit_uses_raw: options.shuangpin_preedit_uses_raw,
            prediction: prediction::Prediction::new(
                options.next_word_prediction,
                options.paths.dictionary(crate::assets::MAIN_DICTIONARY),
            ),
        })
    }

    pub(crate) fn shuangpin_layout(&self) -> &'static crate::shuangpin::ShuangpinProfile {
        self.input.profile
    }

    pub fn set_clock(&mut self, clock: Clock) {
        self.input.clock = clock;
    }

    pub fn character(&mut self, value: u8, shift_only: bool) -> KeyResult {
        self.prediction.clear();
        self.input.handle_character(value, shift_only)
    }

    pub fn command(&mut self, command: Command) -> KeyResult {
        if !self.prediction.rows.is_empty() {
            self.prediction.clear();
            return if command == Command::Cancel {
                KeyResult::handled()
            } else {
                self.input.handle_command(command)
            };
        }
        let selected = (self.prediction.enabled && command == Command::CommitCandidate)
            .then(|| self.snapshot().candidates.first().cloned())
            .flatten();
        let result = self.input.handle_command(command);
        self.predict_after_commit(selected, &result);
        result
    }

    pub fn conversion_left_from(&mut self, index: usize) -> KeyResult {
        self.prediction.clear();
        self.input.conversion_left_from(index)
    }

    pub fn candidate_key(&mut self, value: u8) -> KeyResult {
        self.prediction.clear();
        self.input.handle_candidate_key(value)
    }

    pub fn punctuation(&mut self, value: u8) -> KeyResult {
        self.prediction.clear();
        self.input.handle_punctuation(value)
    }

    pub fn set_chinese_punctuation_enabled(&mut self, enabled: bool) {
        self.input.chinese_punctuation_enabled = enabled;
    }

    pub fn set_punctuation_lock(&mut self, lock: i32) -> Result<()> {
        if !(0..=2).contains(&lock) {
            return Err(EngineError::invalid(diagnostics::INVALID_PUNCTUATION_LOCK));
        }
        self.input.punctuation_lock = lock;
        Ok(())
    }

    pub fn set_paired_punctuation_enabled(&mut self, enabled: bool) {
        self.input.punctuation.set_paired_enabled(enabled);
    }

    pub fn set_punctuation_marks(&mut self, marks: crate::punctuation::PunctuationMarks) {
        self.input.punctuation.set_marks(marks);
    }

    pub fn balance_paired_punctuation_after_auto_close(&mut self, opening: u8) {
        self.input.punctuation.balance_after_auto_close(opening);
    }

    pub fn select(&mut self, index: usize) -> KeyResult {
        if !self.prediction.rows.is_empty() {
            let Some((row, word, pinyin)) = self.prediction.selected(index) else {
                return KeyResult::unhandled();
            };
            let diagnostic = self.input.record_personal_context(Some(&row), false, false);
            self.show_prediction(&word, &pinyin, &row.word);
            return KeyResult::committed(row.word.to_string()).with_diagnostic(diagnostic);
        }
        let selected = self
            .prediction
            .enabled
            .then(|| self.snapshot().candidates.get(index).cloned())
            .flatten();
        let result = self.input.select_candidate(index);
        self.predict_after_commit(selected, &result);
        result
    }

    pub fn select_edge(&mut self, index: usize, edge: CandidateEdge) -> KeyResult {
        if !self.prediction.rows.is_empty() {
            return KeyResult::unhandled();
        }
        self.input.select_candidate_edge(index, edge)
    }

    /// 将词典候选置顶至第 1 位，不上屏。
    pub fn pin(&mut self, index: usize) -> KeyResult {
        if !self.prediction.rows.is_empty() {
            return KeyResult::unhandled();
        }
        self.input.pin_candidate(index)
    }

    /// 删除/屏蔽词典多字词候选，单字受保护。
    pub fn remove(&mut self, index: usize) -> KeyResult {
        if !self.prediction.rows.is_empty() {
            return KeyResult::unhandled();
        }
        self.input.remove_candidate(index)
    }

    pub fn finish(&mut self, first_index: usize) -> KeyResult {
        self.prediction.clear();
        self.input.finish_composition(first_index)
    }

    pub fn switch_scheme(&mut self, scheme: SchemeType) -> Result<()> {
        self.input.switch_scheme(scheme)?;
        self.prediction.clear();
        Ok(())
    }

    pub fn set_dedicated_english(&mut self, enabled: bool) {
        if self.input.dedicated_english == enabled {
            return;
        }
        self.prediction.clear();
        self.input.set_dedicated_english_mode(enabled);
    }

    pub fn reset_cache(&mut self) {
        self.prediction.clear();
        self.input.reset_cache();
    }

    pub fn set_command_table(&mut self, table: &[CommandTableEntry]) -> Option<String> {
        self.input.set_command_table(table)
    }

    pub fn set_quick_phrase_table(&mut self, table: &[QuickPhraseEntry]) -> Option<String> {
        self.input.set_quick_phrase_table(table)
    }

    pub fn set_mention_entries(&mut self, entries: &[MentionEntry]) -> Option<String> {
        self.input.set_mention_entries(entries)
    }

    pub fn set_mention_places(&mut self, enabled: bool) -> Option<String> {
        self.input.set_mention_places(enabled)
    }

    pub fn reset_context(&mut self) {
        self.prediction.clear();
        self.input.last_overlay_learning = None;
        self.input.reset_commit_context();
    }

    pub fn undo_last_personal_learning(&mut self) -> bool {
        self.input.undo_last_personal_learning()
    }

    pub fn set_prediction_allowed(&mut self, allowed: bool) {
        self.prediction.set_allowed(allowed);
    }

    pub fn set_personal_context_enabled(&mut self, enabled: bool) {
        self.prediction.clear();
        self.input.set_personal_context_enabled(enabled);
    }

    pub fn cloud_candidates_enabled(&self) -> bool {
        self.input.cloud_candidates_enabled
    }

    pub fn set_cloud_candidates_enabled(&mut self, enabled: bool) {
        self.input.set_cloud_candidates_enabled(enabled);
    }

    pub fn english_online_candidates_enabled(&self) -> bool {
        self.input.english_online_candidates_enabled
    }

    pub fn set_english_online_candidates_enabled(&mut self, enabled: bool) {
        self.input.set_english_online_candidates_enabled(enabled);
    }

    pub fn set_rescoring_context(&mut self, context: &str) {
        self.input.set_rescoring_context(context);
    }

    pub fn set_first_page_rows(&mut self, rows: FirstPageRows) {
        self.input.set_first_page_rows(rows);
    }

    pub fn expand_initial_candidates(&mut self) -> bool {
        self.input.expand_initial_candidates()
    }

    pub fn set_caret(&mut self, caret: Option<usize>) {
        self.prediction.clear();
        self.input.set_caret(caret);
    }

    pub fn prefix_end(&self) -> usize {
        self.input.prefix_end()
    }

    pub fn pending_suffix(&self) -> String {
        self.input.pending_suffix()
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot_with_display_options(false)
    }

    pub(crate) fn snapshot_for_host(&self) -> SessionSnapshot {
        self.snapshot_with_display_options(true)
    }

    fn snapshot_with_display_options(&self, segmented_quanpin: bool) -> SessionSnapshot {
        if !self.prediction.rows.is_empty() {
            return SessionSnapshot {
                prediction: true,
                scheme: self.input.scheme(),
                local_mode: self.input.local_mode,
                spelling_symbols: self.input.spelling_symbols(),
                shuangpin_profile: self.input.profile.kind.name().to_owned(),
                candidates: self.prediction.rows.clone(),
                candidate_sources: self.prediction.rows.iter().map(|row| row.source).collect(),
                candidate_annotations: vec![String::new(); self.prediction.rows.len()],
                candidate_answers_key: vec![false; self.prediction.rows.len()],
                ..SessionSnapshot::default()
            };
        }
        let input = &self.input;
        let mut candidates = input.candidates().to_vec();
        for item in &mut candidates {
            input.apply_english_word_case(item.source, &mut item.word);
        }
        let caret_edit_suffix = input.caret_edit.active && input.conversion.is_none();
        let caret_edit_prefix = input
            .caret_edit_prefix()
            .filter(|_| caret_edit_suffix && !candidates.is_empty())
            .unwrap_or_default();
        let segmented_preedit = (segmented_quanpin && input.scheme() == SchemeType::Quanpin)
            || (!self.shuangpin_preedit_uses_raw && input.scheme() == SchemeType::Shuangpin);
        let preedit = if segmented_preedit
            && input.local_mode == LocalInputMode::None
            && !input.dedicated_english
        {
            input.pinyin_segmentation_with_cases().into_owned()
        } else {
            input.preedit()
        };
        SessionSnapshot {
            prediction: false,
            scheme: input.scheme(),
            local_mode: input.local_mode,
            spelling_symbols: input.spelling_symbols(),
            preedit,
            raw_segmentation: input.raw_segmentation(),
            normalized_segmentation: input.normalized_segmentation(),
            dedicated_english: input.dedicated_english,
            editing_text: input.editing_text(),
            caret_position: input.caret_position(),
            shuangpin_profile: input.profile.kind.name().to_owned(),
            candidate_sources: candidates.iter().map(|item| item.source).collect(),
            candidate_annotations: input.candidate_annotations(),
            candidate_answers_key: candidates
                .iter()
                .map(|item| {
                    !matches!(
                        item.source,
                        CandidateSource::AiPending
                            | CandidateSource::CloudPending
                            | CandidateSource::Contact
                    ) && !input.is_url_completion(item)
                        && input.selection_completes_composition(item)
                        && input.reading_answers_key(item)
                })
                .collect(),
            conversion: input
                .conversion
                .as_ref()
                .map(|edit| edit.text())
                .unwrap_or_default(),
            conversion_focus: input
                .conversion
                .as_ref()
                .map_or((0, 0), |edit| edit.focus_span()),
            caret_edit_suffix,
            caret_edit_prefix,
            candidates,
        }
    }

    pub fn segment_raw_boundaries(&self) -> Vec<usize> {
        self.input.segment_raw_boundaries()
    }

    pub fn online_query(&self) -> Option<OnlineQuery> {
        self.input.online_query()
    }

    pub fn neural_v2_metadata(&self) -> Option<crate::neural_v2::EngineNeuralMetadata> {
        if !self.prediction.rows.is_empty() {
            return None;
        }
        self.input.neural_v2_metadata()
    }

    /// 返回当前组字上下文在本地学习日志 `pinned_candidates` 中显式置顶的候选词文本。
    pub(crate) fn pinned_candidate_word(&self) -> Option<String> {
        if !self.prediction.rows.is_empty()
            || self.input.local_mode != LocalInputMode::None
            || self.input.dedicated_english
            || !self.input.has_composition()
            || !self.input.scheme().is_pinyin()
        {
            return None;
        }
        let journal = self.input.journal_path();
        if journal.as_os_str().is_empty() {
            return None;
        }
        let context = self.input.pinyin_ranking_context();
        crate::user_dictionary::positions::pinned_candidate_for(journal, &context)
    }

    pub fn takes_local_separator(&self) -> bool {
        self.input.takes_local_separator()
    }

    pub fn command_translation_query(&self) -> Option<CommandTranslationQuery> {
        self.input.command_translation_query()
    }

    pub fn apply_command_translation(
        &mut self,
        query: &CommandTranslationQuery,
        translation: &str,
    ) -> bool {
        self.input.apply_command_translation(query, translation)
    }

    pub fn apply_online_candidates(
        &mut self,
        query: &OnlineQuery,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        self.input.apply_online_candidates(query, words, source)
    }

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        self.input.clear_online_candidates(source);
    }

    pub fn apply_online_candidate(
        &mut self,
        query: &OnlineQuery,
        word: &str,
        source: CandidateSource,
    ) -> bool {
        self.input.apply_online_candidate(query, word, source)
    }

    fn predict_after_commit(&mut self, selected: Option<WordItem>, result: &KeyResult) {
        if !self.prediction.enabled
            || self.input.has_composition()
            || self.input.dedicated_english
            || self.input.local_mode != LocalInputMode::None
            || !matches!(
                self.input.scheme(),
                SchemeType::Quanpin | SchemeType::Shuangpin
            )
        {
            return;
        }
        let Some(selected) = selected else { return };
        if result.commit.as_deref() != Some(selected.word.as_str()) {
            return;
        }
        let previous = selected
            .sentence_words
            .last()
            .map_or(selected.word.as_str(), String::as_str);
        self.show_prediction(&selected.word, &selected.canonical_pinyin, previous);
    }

    fn show_prediction(&mut self, word: &str, pinyin: &str, previous: &str) {
        let personal = self
            .input
            .personal_context_applies()
            .then_some(self.input.personal_context.as_ref());
        self.prediction.show(word, pinyin, previous, personal);
    }
}
