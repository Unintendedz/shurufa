//! 候选选词与上屏逻辑。

use super::chain::Pick;
use super::input::InputSession;
use crate::diagnostics;
use crate::error::Result;
use crate::pinyin::segment::{join_segments, split_segments};
use crate::pinyin::syllables::normalize_umlaut_aliases;
use crate::quanpin::QuanpinEngine;
use crate::text::count_utf8_chars;
use crate::types::{
    CandidateEdge, CandidateSource, KeyResult, LocalInputMode, PersonalDictionaryKind, WordItem,
};
use crate::user_dictionary::journal::is_user_inserted;

/// 提交只会读取这些候选字段；纠错来源用于计算原始按键的消费长度。
pub(super) fn clone_selected_candidate(item: &WordItem, copy_sentence_words: bool) -> WordItem {
    WordItem {
        pinyin: item.pinyin.clone(),
        canonical_pinyin: item.canonical_pinyin.clone(),
        word: item.word.clone(),
        source: item.source,
        scheme: item.scheme,
        corrected_from: item.corrected_from.clone(),
        sentence_words: if copy_sentence_words && item.source.is_generated_or_fallback() {
            item.sentence_words.clone()
        } else {
            Vec::new()
        },
        ..WordItem::default()
    }
}

impl InputSession {
    pub(super) fn select_candidate(&mut self, index: usize) -> KeyResult {
        if self.conversion.is_some() {
            return self.select_conversion_row(index);
        }
        if !self.has_composition() || index >= self.candidates().len() {
            return KeyResult::unhandled();
        }
        if matches!(
            self.candidates()[index].source,
            CandidateSource::AiPending | CandidateSource::CloudPending
        ) {
            return KeyResult::handled();
        }
        if let Some(text) = self.url_continuation(index) {
            self.enter_url_mode(text);
            return KeyResult::handled();
        }
        self.commit(index)
    }

    pub(super) fn select_candidate_edge(&mut self, index: usize, edge: CandidateEdge) -> KeyResult {
        if self.conversion.is_some() || !self.has_composition() {
            return KeyResult::unhandled();
        }
        if let Some(keyword) = self.date_time_menu_keyword_at(index) {
            self.enter_date_time_menu(&keyword);
            return KeyResult::handled();
        }
        if self.date_time_menu_page.is_some() {
            return KeyResult::unhandled();
        }
        let Some(candidate) = self.candidates().get(index) else {
            return KeyResult::unhandled();
        };
        let mut character = edge.character(&candidate.word).to_owned();
        if character.is_empty() {
            return KeyResult::unhandled();
        }
        if self.caret_edit.active {
            character = self.caret_edit_edge_text(&character);
        }
        self.reset_composition();
        self.chain.reset();
        KeyResult::committed(character)
    }

    /// 逐段选词直至当前组字全部上屏。
    pub(super) fn finish_composition(&mut self, first_index: usize) -> KeyResult {
        self.end_uppercase_run();
        let mut result = KeyResult::unhandled();
        let mut index = if self.candidates().get(first_index).is_some_and(|item| {
            matches!(
                item.source,
                CandidateSource::AiPending | CandidateSource::CloudPending
            )
        }) {
            0
        } else {
            first_index
        };
        if self.local_mode == LocalInputMode::None
            && self
                .candidates()
                .get(index)
                .is_some_and(|item| self.is_url_completion(item))
        {
            index = self
                .candidates()
                .iter()
                .position(|item| !self.is_url_completion(item))
                .unwrap_or(usize::MAX);
        }
        if self.conversion.is_some() {
            result = self.commit_conversion();
            index = 0;
        }
        if let Some(keyword) = self.date_time_menu_keyword_at(index) {
            self.enter_date_time_menu(&keyword);
            result.handled = true;
            return result;
        }
        while self.has_composition() {
            let part = self.commit(index);
            index = 0;
            result.handled = true;
            if let Some(text) = part.commit {
                result
                    .commit
                    .get_or_insert_with(String::new)
                    .push_str(&text);
            }
            if part.diagnostic.is_some() {
                result.diagnostic = part.diagnostic;
            }
        }
        self.chain.last_pick = None;
        result
    }

    /// 选中第 `index` 条并上屏或推进组字。
    pub(super) fn commit(&mut self, index: usize) -> KeyResult {
        if self.caret_edit.active && self.conversion.is_none() && self.date_time_menu_page.is_none()
        {
            if let Some(result) = self.commit_caret_edit(index) {
                return result;
            }
        }
        self.commit_plain(index)
    }

    fn commit_plain(&mut self, index: usize) -> KeyResult {
        if let Some(keyword) = self.date_time_menu_keyword_at(index) {
            self.enter_date_time_menu(&keyword);
            return KeyResult::handled();
        }
        if let Some(result) = self.commit_literal_candidate(index) {
            return result;
        }
        self.caret = None;
        let committed = |text: Option<String>, diagnostic: Option<String>| KeyResult {
            handled: true,
            commit: text,
            diagnostic,
        };
        if self.local_mode != LocalInputMode::None {
            let text = self
                .candidates()
                .get(index)
                .map(|item| item.word.to_string());
            let diagnostic = self
                .ranking_index(index)
                .and_then(|learn_index| self.learn_candidate(learn_index));
            self.chain.reset();
            self.reset_composition();
            return committed(text, diagnostic);
        }
        let copy_sentence_words = self.personal_context_applies();
        let selected = self
            .candidates()
            .get(index)
            .map(|item| clone_selected_candidate(item, copy_sentence_words));
        let text = match &selected {
            Some(item) => {
                let mut text = item.word.to_string();
                self.apply_english_word_case(item.source, &mut text);
                Some(text)
            }
            None if matches!(
                self.local_mode,
                LocalInputMode::TemporaryEnglish | LocalInputMode::TemporaryJapanese
            ) && self.local_preedit.len() == 1 =>
            {
                None
            }
            None => Some(self.raw_commit_text()),
        };
        let diagnostic = self
            .ranking_index(index)
            .and_then(|learn_index| self.learn_candidate(learn_index));
        self.commit_selection(selected, text, diagnostic)
    }

    /// 拼音组字里上屏选中行之后：记个人上下文、推进组字或结束组字并造词。
    pub(super) fn commit_selection(
        &mut self,
        selected: Option<WordItem>,
        text: Option<String>,
        mut diagnostic: Option<String>,
    ) -> KeyResult {
        let committed = |text: Option<String>, diagnostic: Option<String>| KeyResult {
            handled: true,
            commit: text,
            diagnostic,
        };
        let Some(selected) = selected else {
            self.record_context_into(&mut diagnostic, None, false, false);
            self.chain.last_pick = None;
            self.reset_composition();
            return committed(text, diagnostic);
        };
        let has_dictionary_reading = selected.source.is_dictionary();
        let has_generated_reading =
            selected.source.is_generated_or_fallback() && !selected.canonical_pinyin.is_empty();
        if !((has_dictionary_reading || has_generated_reading)
            && self.local_mode == LocalInputMode::None
            && !self.dedicated_english
            && self.candidates_follow_pinyin())
        {
            self.record_context_into(&mut diagnostic, Some(&selected), false, false);
            self.chain.last_pick = None;
            self.reset_composition();
            return committed(text, diagnostic);
        }

        let learned_sentence_word = has_dictionary_reading
            && self.personal_context_applies()
            && !self.chain.same_composition
            && !selected.canonical_pinyin.is_empty()
            && count_utf8_chars(&selected.word) >= 3
            && self.selection_completes_composition(&selected)
            && {
                let mut segments = split_segments(&selected.canonical_pinyin);
                normalize_umlaut_aliases(&mut segments);
                is_user_inserted(
                    self.journal_path(),
                    PersonalDictionaryKind::Pinyin,
                    &join_segments(&segments),
                    &selected.word,
                )
            };
        let opens_composition = self.phrase_progress.word.is_empty();
        let transition = self.advance_composition_after_selection(&selected);
        self.record_context_into(
            &mut diagnostic,
            Some(&selected),
            learned_sentence_word,
            transition.continues_composition,
        );
        let pick = (has_dictionary_reading
            && !learned_sentence_word
            && self.learning_enabled
            && self.personal_context_enabled)
            .then(|| Pick {
                canonical_pinyin: transition.selected_canonical_pinyin.clone(),
                word: selected.word.to_string(),
                committed_at: self.steady_now(),
            });
        if opens_composition {
            if let (Some(current), Some(previous)) = (&pick, self.chain.last_pick.clone()) {
                if let Some(pair_diagnostic) = self.learn_pick_pair(&previous, current) {
                    diagnostic.get_or_insert(pair_diagnostic);
                }
            }
        }
        self.chain.last_pick = pick;
        if transition.continues_composition && self.chain.previous.is_some() {
            self.update_mixed_candidates();
        }
        let progress = Self::update_creating_word_progress(
            &self.phrase_progress.pinyin,
            &self.phrase_progress.word,
            &selected.word,
            &transition,
        );
        if transition.continues_composition {
            self.phrase_progress = progress;
            self.discard_abandoned_phrase_progress();
            return committed(text, diagnostic);
        }
        if !self.phrase_progress.word.is_empty()
            && progress.can_store
            && self.learning_enabled
            && self
                .store_user_phrase_from_canonical_pinyin(&progress.pinyin, &progress.word)
                .is_err()
        {
            diagnostic = Some(diagnostics::PHRASE_NOT_PERSISTED.to_owned());
        }
        self.reset_composition();
        committed(text, diagnostic)
    }

    pub(super) fn store_user_phrase_from_canonical_pinyin(
        &mut self,
        pinyin: &str,
        word: &str,
    ) -> Result<()> {
        self.canonical_phrase_engine()
            .create_word_from_canonical_pinyin(pinyin, word)
    }

    pub(super) fn canonical_phrase_engine(&mut self) -> &mut QuanpinEngine {
        let paths = &self.paths;
        self.canonical_phrase_engine
            .get_or_insert_with(|| QuanpinEngine::new(paths))
    }

    pub(super) fn record_context_into(
        &mut self,
        diagnostic: &mut Option<String>,
        selected: Option<&WordItem>,
        learned_sentence_word: bool,
        composition_left: bool,
    ) {
        if let Some(context_diagnostic) =
            self.record_personal_context(selected, learned_sentence_word, composition_left)
        {
            diagnostic.get_or_insert(context_diagnostic);
        }
    }
}
