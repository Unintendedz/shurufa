//! 候选混排与手动干预（置顶到首位、删除词条）。

use std::borrow::Cow;

use super::input::InputSession;
use crate::assets;
use crate::diagnostics;
use crate::ime::personal_rerank::personal_context_rerank_order;
use crate::ime::queries::{keep_expressive_rows_off_the_lead, MODE_ENGLISH_LIMIT};
use crate::local::date_time::{inline_date_time_keyword, insert_inline_date_time, LocalDateTime};
use crate::local::jianpin::jianpin_ranking_context;
use crate::pinyin::segment::{cut_pinyin_by_mode, join_segments, CutMode};
use crate::text::{count_utf8_chars, is_han_phrase};
use crate::types::{
    CandidateSource, FrequencyAdjustmentMode, FrequencyAdjustmentOptions, KeyResult,
    LocalInputMode, PersonalDictionaryKind, SchemeType, WordItem,
};
use crate::user_dictionary::journal::has_matching_user_weight;
use crate::user_dictionary::overlay::{
    apply_personal_overlay_scoped, effective_bayesian_weights_scoped, personal_cache_log_bonus,
    unix_now_seconds,
};
use crate::user_dictionary::positions::{
    apply_fixed_positions_with_state, clear_fixed_position_with_pin_context, is_pinned_candidate,
    pinned_candidate_for, record_pinned_candidate_with_position_context,
    set_fixed_position_with_pin_context,
};
use crate::user_dictionary::ranking::candidate_dictionary_key;
use crate::user_dictionary::removal::delete_dictionary_candidate;

fn english_position_context(input: &str) -> String {
    let mut context = String::with_capacity("english:".len() + input.len());
    context.push_str("english:");
    context.extend(
        input
            .chars()
            .map(|character| character.to_ascii_lowercase()),
    );
    context
}

fn plain_position_context(context: &str) -> String {
    let mut plain = String::with_capacity(context.len());
    plain.extend(context.chars().filter(|character| *character != '\''));
    plain
}

pub(super) fn clone_candidate_rows(source: &[WordItem], destination: &mut Vec<WordItem>) {
    let common = source.len().min(destination.len());
    for (target, item) in destination.iter_mut().take(common).zip(source.iter()) {
        target.pinyin.clone_from(&item.pinyin);
        target.canonical_pinyin.clone_from(&item.canonical_pinyin);
        target.word.clone_from(&item.word);
        target.weight = item.weight;
        target.source = item.source;
        target.scheme = item.scheme;
        target.pinned_first = item.pinned_first;
        target.fixed_position = item.fixed_position;
        target.fuzzy = item.fuzzy;
        target.corrected_from.clone_from(&item.corrected_from);
        target.sentence_association = item.sentence_association;
        target.sentence_words.clone_from(&item.sentence_words);
        target.keyboard_runner_up = item.keyboard_runner_up;
        target.extra_syllables = item.extra_syllables;
        target.keyboard_neural = item.keyboard_neural;
    }
    if destination.len() > source.len() {
        destination.truncate(source.len());
    } else {
        destination.extend(source[common..].iter().cloned());
    }
}

impl InputSession {
    pub(super) fn apply_english_word_case(&self, source: CandidateSource, word: &mut String) {
        if source != CandidateSource::EnglishDictionary
            || self.local_mode != LocalInputMode::None
            || self.dedicated_english
        {
            return;
        }
        let raw = &self.engine.request().raw_input_with_cases;
        if !raw.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return;
        }
        if raw.len() > 1 && raw.bytes().all(|byte| byte.is_ascii_uppercase()) {
            word.make_ascii_uppercase();
        } else if word
            .get(..raw.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(raw))
        {
            word.replace_range(..raw.len(), raw);
        } else if raw.as_bytes().first().is_some_and(u8::is_ascii_uppercase) {
            if let Some(first) = word.get_mut(..1) {
                first.make_ascii_uppercase();
            }
        }
    }

    pub(super) fn update_mixed_candidates(&mut self) {
        self.merge_mixed_candidates();
        self.apply_stroke_filter();
    }

    fn merge_mixed_candidates(&mut self) {
        if self.update_literal_candidates() {
            return;
        }
        self.refresh_prefix_candidates();
        let mut decoded = self
            .ranking_candidates
            .take()
            .unwrap_or_else(|| std::mem::take(&mut self.mixed_candidates));
        let mut reranked = std::mem::take(&mut self.mixed_candidates);
        if self.prefix_active {
            clone_candidate_rows(&self.prefix_candidates, &mut decoded);
        } else {
            clone_candidate_rows(self.engine.candidates(), &mut decoded);
            if self.quick_phrase_candidates
                && !self.caret_edit.active
                && self.mixed_candidates_enabled
                && self.local_mode_options.quick_phrase
            {
                let code = self.engine.request().raw_input.clone();
                let slot = (self.quick_phrase_frequency
                    && self.frequency.mode != FrequencyAdjustmentMode::Disabled)
                    .then(|| {
                        crate::user_dictionary::ranking::quick_phrase_slot(
                            self.journal_path(),
                            &code,
                        )
                    })
                    .flatten();
                self.queries.insert_quick_phrase_candidates(
                    &code,
                    &mut decoded,
                    slot,
                    self.first_page_rows,
                );
            }
        }
        self.personal_reranked = false;
        let mut overlay_leader_protected = false;
        if self.decoupled_personal_learning
            && self.local_mode == LocalInputMode::None
            && !self.dedicated_english
            && !self.prefix_active
            && self.scheme().is_pinyin()
        {
            let entry_key = self.pinyin_ranking_context();
            let pinned = pinned_candidate_for(self.journal_path(), &entry_key);
            let outcome = apply_personal_overlay_scoped(
                self.journal_path(),
                &entry_key,
                pinned.as_deref(),
                self.app_scope,
                &mut decoded,
                unix_now_seconds(),
            );
            overlay_leader_protected = outcome.leader_protected;
        }
        let reordered = match self.chain.previous.as_deref() {
            Some(previous) if self.personal_context_applies() => {
                let journal = self.journal_path();
                let context = self.pinyin_ranking_context();
                let model = self.personal_context.model();
                personal_context_rerank_order(
                    &decoded,
                    &model,
                    self.chain.earlier.as_deref(),
                    previous,
                    &|row: &WordItem| {
                        self.prefix_active || self.selection_completes_composition(row)
                    },
                    &mut |leader: &str| {
                        overlay_leader_protected
                            || Self::leader_is_pinned_or_user_weighted(
                                journal,
                                &context,
                                self.frequency.mode,
                                &decoded,
                                leader,
                            )
                    },
                )
            }
            _ => None,
        };
        match reordered {
            Some(order) => {
                clone_candidate_rows(&decoded, &mut reranked);
                order.reorder(&mut reranked);
                if let Some((start, win)) = self.find_single_char_prefix_promote(&reranked) {
                    reranked[start..=win].rotate_right(1);
                }
                self.mixed_candidates = self.mixed_from(reranked);
                self.ranking_candidates = Some(self.mixed_from(decoded));
                self.personal_reranked = true;
            }
            None => {
                if let Some(promote) = self.contextual_dictionary_promote(&decoded) {
                    clone_candidate_rows(&decoded, &mut reranked);
                    reranked[..=promote].rotate_right(1);
                    if let Some((start, win)) = self.find_single_char_prefix_promote(&reranked) {
                        reranked[start..=win].rotate_right(1);
                    }
                    self.mixed_candidates = self.mixed_from(reranked);
                    self.ranking_candidates = Some(self.mixed_from(decoded));
                    self.personal_reranked = true;
                } else if let Some((start, win)) = self.find_single_char_prefix_promote(&decoded) {
                    clone_candidate_rows(&decoded, &mut reranked);
                    reranked[start..=win].rotate_right(1);
                    self.mixed_candidates = self.mixed_from(reranked);
                    self.ranking_candidates = Some(self.mixed_from(decoded));
                    self.personal_reranked = true;
                } else {
                    self.mixed_candidates = self.mixed_from(decoded);
                }
            }
        }
    }

    fn leader_is_pinned_or_user_weighted(
        journal: &std::path::Path,
        context: &str,
        frequency_mode: FrequencyAdjustmentMode,
        decoded: &[WordItem],
        leader: &str,
    ) -> bool {
        is_pinned_candidate(journal, context, leader)
            || (frequency_mode != FrequencyAdjustmentMode::Disabled
                && decoded.first().is_some_and(|row| {
                    if !row.source.is_dictionary() || !row.scheme.is_pinyin() {
                        return false;
                    }
                    let key = if row.canonical_pinyin.is_empty() {
                        Cow::Owned(candidate_dictionary_key(row, context))
                    } else {
                        Cow::Borrowed(row.canonical_pinyin.as_ref())
                    };
                    has_matching_user_weight(
                        journal,
                        PersonalDictionaryKind::Pinyin,
                        &key,
                        &row.word,
                        row.weight,
                    )
                }))
    }

    #[inline]
    fn item_reading(item: &WordItem) -> &str {
        if item.canonical_pinyin.is_empty() {
            item.pinyin.as_str()
        } else {
            item.canonical_pinyin.as_str()
        }
    }

    fn contextual_dictionary_promote(&mut self, decoded: &[WordItem]) -> Option<usize> {
        const HOMOPHONE_SCAN_WINDOW: usize = 48;
        if self.local_mode != LocalInputMode::None
            || self.dedicated_english
            || self.prefix_active
            || self.caret_edit.active
            || self.has_literal_input()
            || !self.engine.has_han_rescoring_context()
        {
            return None;
        }
        let first = decoded.first()?;
        let reading = Self::item_reading(first);
        if first.source != CandidateSource::Database
            || !first.scheme.is_pinyin()
            || first.fuzzy
            || !first.corrected_from.is_empty()
            || reading.is_empty()
            || !self.selection_completes_composition(first)
            || !self.reading_answers_key(first)
            || !self.engine.has_dictionary_reranker(first.scheme)
        {
            return None;
        }
        let typed_segmentation = match first.scheme {
            SchemeType::Quanpin => first.pinyin.as_str(),
            SchemeType::Shuangpin => self.engine.request().normalized_segmentation.as_str(),
            _ => return None,
        };
        let characters = first.word.chars().count();
        if characters == 0 {
            return None;
        }
        let pieces: Vec<&str> = typed_segmentation
            .split('\'')
            .filter(|piece| !piece.is_empty())
            .collect();
        let all_intact = pieces
            .iter()
            .all(|piece| crate::pinyin::syllables::is_intact(piece));
        let aligned_jianpin = characters >= 2
            && pieces.len() == characters
            && pieces.iter().all(|piece| {
                crate::pinyin::syllables::is_intact(piece)
                    || (piece.len() == 1 && piece.as_bytes()[0].is_ascii_lowercase())
            });
        if !all_intact && !aligned_jianpin {
            return None;
        }
        let scheme = first.scheme;
        let mut refs = [first; crate::lattice::octagram::MAX_RERANK_PATHS];
        let mut indices = [0usize; crate::lattice::octagram::MAX_RERANK_PATHS];
        let mut count = 1usize;
        for (index, item) in decoded
            .iter()
            .enumerate()
            .take(HOMOPHONE_SCAN_WINDOW)
            .skip(1)
        {
            if item.source == CandidateSource::Database
                && item.scheme == scheme
                && !item.fuzzy
                && item.corrected_from.is_empty()
                && Self::item_reading(item) == reading
                && item.word.chars().count() == characters
                && self.selection_completes_composition(item)
                && self.reading_answers_key(item)
            {
                refs[count] = item;
                indices[count] = index;
                count += 1;
                if count == crate::lattice::octagram::MAX_RERANK_PATHS {
                    break;
                }
            }
        }
        if count < 2 {
            return None;
        }
        let journal = self.journal_path();
        let context = self.pinyin_ranking_context();
        if is_pinned_candidate(journal, &context, &first.word) {
            return None;
        }
        let effective_weights = self.decoupled_personal_learning.then(|| {
            effective_bayesian_weights_scoped(
                journal,
                &context,
                &refs[..count],
                self.app_scope,
                unix_now_seconds(),
            )
        });
        let effective_weights = effective_weights.flatten();
        let best = self.engine.pick_dictionary_homophone(
            scheme,
            &refs[..count],
            effective_weights.as_deref(),
        )?;
        (best > 0).then_some(indices[best])
    }

    /// 多音节输入时，若 `.gram` 存在强上下文约束（如 `"还真是" + xian...`），
    /// 将首音节主单字切片中最契合上文的单字提至单字前缀首位。
    fn find_single_char_prefix_promote(&mut self, items: &[WordItem]) -> Option<(usize, usize)> {
        const PREFIX_SCAN_WINDOW: usize = 48;
        if self.local_mode != LocalInputMode::None
            || self.dedicated_english
            || self.prefix_active
            || self.caret_edit.active
            || self.has_literal_input()
            || !self.engine.has_han_rescoring_context()
        {
            return None;
        }
        let scheme = self.scheme();
        if !scheme.is_pinyin() || !self.engine.has_dictionary_reranker(scheme) {
            return None;
        }
        let first_syl = {
            let seg = self.pinyin_segmentation();
            let mut parts = seg.split('\'').filter(|s| !s.is_empty());
            let first = parts.next()?;
            parts.next()?;
            if !crate::pinyin::syllables::is_intact(first) {
                return None;
            }
            first.to_owned()
        };
        let first_idx = items.iter().position(|item| {
            item.source == CandidateSource::Database
                && item.scheme == scheme
                && !item.fuzzy
                && item.corrected_from.is_empty()
                && item.word.chars().count() == 1
                && !self.selection_completes_composition(item)
                && Self::item_reading(item) == first_syl
        })?;
        let first = &items[first_idx];
        let journal = self.journal_path();
        let context = self.pinyin_ranking_context();
        if first.pinned_first
            || is_pinned_candidate(journal, &context, &first.word)
            || is_pinned_candidate(journal, &first_syl, &first.word)
        {
            return None;
        }
        let mut refs = [first; crate::lattice::octagram::MAX_RERANK_PATHS];
        let mut indices = [first_idx; crate::lattice::octagram::MAX_RERANK_PATHS];
        let mut count = 1usize;
        for (offset, item) in items[first_idx + 1..]
            .iter()
            .take(PREFIX_SCAN_WINDOW - 1)
            .enumerate()
        {
            if item.source == CandidateSource::Database
                && item.scheme == scheme
                && !item.fuzzy
                && item.corrected_from.is_empty()
                && item.word.chars().count() == 1
                && !self.selection_completes_composition(item)
                && Self::item_reading(item) == first_syl
            {
                refs[count] = item;
                indices[count] = first_idx + 1 + offset;
                count += 1;
                if count == crate::lattice::octagram::MAX_RERANK_PATHS {
                    break;
                }
            }
        }
        if count < 2 {
            return None;
        }
        let best = self
            .engine
            .pick_dictionary_homophone(scheme, &refs[..count], None)?;
        (best > 0).then_some((first_idx, indices[best]))
    }

    pub(super) fn ranking_list(&self) -> &[WordItem] {
        self.ranking_candidates
            .as_deref()
            .unwrap_or_else(|| self.candidates())
    }

    pub(super) fn update_local_candidates(&mut self) -> Option<String> {
        let now = if matches!(
            self.local_mode,
            LocalInputMode::DateTime | LocalInputMode::Command
        ) {
            self.local_now()
        } else {
            LocalDateTime::default()
        };
        let scheme = self.scheme();
        let result = self.queries.local(
            self.local_mode,
            &self.local_preedit,
            scheme,
            &now,
            self.engine.candidates(),
        );
        let mut candidates = result.candidates;
        self.apply_candidate_positions(&mut candidates);
        self.local_candidates = candidates;
        self.add_local_fallback_candidate();
        if self.local_mode == LocalInputMode::Url {
            self.add_url_completions();
        }
        if self.local_mode == LocalInputMode::TemporaryEnglish {
            let mut candidates = std::mem::take(&mut self.local_candidates);
            self.append_english_online_rows(&mut candidates);
            self.local_candidates = candidates;
        }
        result.diagnostic
    }

    pub(super) fn add_local_fallback_candidate(&mut self) {
        if self.local_candidates.is_empty() && !self.local_preedit.is_empty() {
            self.local_candidates.push(WordItem::new(
                self.local_preedit.clone(),
                self.local_preedit.clone(),
                1,
                CandidateSource::Fallback,
                "",
            ));
        }
    }

    pub(super) fn update_dedicated_english_candidates(&mut self) {
        if self.dedicated_english_preedit.is_empty() {
            self.dedicated_english_candidates.clear();
            return;
        }
        let prefix = crate::ime::queries::lowercase_prefix(&self.dedicated_english_preedit);
        self.queries.english_dictionary().query_prefix_into(
            &prefix,
            MODE_ENGLISH_LIMIT,
            &mut self.dedicated_english_candidates,
        );
        let mut candidates = std::mem::take(&mut self.dedicated_english_candidates);
        self.apply_candidate_positions(&mut candidates);
        if candidates.is_empty() {
            candidates.push(WordItem::new(
                "",
                self.dedicated_english_preedit.clone(),
                0,
                CandidateSource::Generated,
                "",
            ));
        }
        self.append_english_online_rows(&mut candidates);
        self.dedicated_english_candidates = candidates;
    }

    pub(super) fn apply_candidate_positions(&mut self, items: &mut Vec<WordItem>) {
        let regular = self.local_mode == LocalInputMode::None && !self.dedicated_english;
        let has_english =
            self.dedicated_english || self.local_mode == LocalInputMode::TemporaryEnglish;
        let journal = self.journal_path();
        let journal_exists = journal.try_exists().unwrap_or(false);
        if !journal_exists && !items.iter().any(|item| item.source.takes_online_seat()) {
            return;
        }
        let include_missing = regular && self.engine.request().raw_input.len() == 1;
        if regular {
            let context = self.position_context(false);
            let scheme = self.scheme();
            let engine = &self.engine;
            let mut finder = |key: &str, word: &str| engine.find_candidate(scheme, key, word);
            apply_fixed_positions_with_state(
                journal,
                context.as_ref(),
                items,
                include_missing,
                Some(&mut finder),
                false,
                journal_exists,
            );
        } else if self.local_mode == LocalInputMode::SuperJianpin {
            let context = self.position_context(false);
            apply_fixed_positions_with_state(
                journal,
                context.as_ref(),
                items,
                false,
                None,
                false,
                journal_exists,
            );
        }
        if has_english
            || items
                .iter()
                .any(|item| item.source == CandidateSource::EnglishDictionary)
        {
            let context = self.position_context(true);
            apply_fixed_positions_with_state(
                journal,
                context.as_ref(),
                items,
                false,
                None,
                true,
                journal_exists,
            );
        }
    }

    pub(super) fn position_context(&self, english: bool) -> Cow<'_, str> {
        let request = self.engine.request();
        if english {
            let input = if self.dedicated_english {
                &self.dedicated_english_preedit
            } else if self.local_mode == LocalInputMode::TemporaryEnglish {
                &self.local_preedit[1..]
            } else {
                &request.raw_input_with_cases
            };
            return Cow::Owned(english_position_context(input));
        }
        if self.local_mode == LocalInputMode::SuperJianpin {
            return Cow::Owned(jianpin_ranking_context(
                &self.local_preedit[1..],
                self.scheme(),
                self.shuangpin_profile(),
            ));
        }
        let context: Cow<'_, str> = if request.normalized_input.is_empty() {
            self.pinyin_segmentation()
        } else {
            Cow::Borrowed(&request.normalized_input)
        };
        if request.raw_input.len() == 1 {
            return context;
        }
        let plain = if context.contains('\'') {
            Cow::Owned(plain_position_context(context.as_ref()))
        } else {
            Cow::Borrowed(context.as_ref())
        };
        match cut_pinyin_by_mode(&plain, CutMode::Correction).first() {
            Some(cut) => Cow::Owned(join_segments(cut)),
            None => context,
        }
    }

    pub(super) fn ranking_index(&self, displayed: usize) -> Option<usize> {
        let shown = self.candidates().get(displayed)?;
        let Some(ordered) = self.ranking_candidates.as_deref() else {
            return Some(displayed);
        };
        let found = ordered.iter().position(|item| {
            item.word == shown.word
                && item.source == shown.source
                && item.pinyin == shown.pinyin
                && item.canonical_pinyin == shown.canonical_pinyin
        });
        Some(found.unwrap_or(displayed))
    }

    /// 设定或取消候选的第 `1..=5` 位固定排位（`position == 0` 取消固定，同步清除手动置顶）。
    pub(super) fn set_candidate_position(&mut self, index: usize, position: u8) -> KeyResult {
        if self.conversion.is_some() || self.date_time_menu_page.is_some() || position > 5 {
            return KeyResult::unhandled();
        }
        let Some(selected) = self.candidates().get(index) else {
            return KeyResult::unhandled();
        };
        if !self.is_editable_source(selected) {
            return KeyResult::unhandled();
        }
        let english = selected.source == CandidateSource::EnglishDictionary;
        let context = self.position_context(english);
        let key = if english || selected.canonical_pinyin.is_empty() {
            &selected.pinyin
        } else {
            &selected.canonical_pinyin
        };
        if context.is_empty() || key.is_empty() {
            return KeyResult::unhandled();
        }
        let pin_context = (!english
            && self.local_mode == LocalInputMode::None
            && !self.dedicated_english
            && self.scheme().is_pinyin())
        .then(|| self.pinyin_ranking_context());
        let journal = self.journal_path();
        let written = if position == 0 {
            clear_fixed_position_with_pin_context(
                journal,
                context.as_ref(),
                pin_context.as_deref(),
                key,
                &selected.word,
            )
        } else {
            set_fixed_position_with_pin_context(
                journal,
                context.as_ref(),
                pin_context.as_deref(),
                key,
                &selected.word,
                position,
            )
        };
        if written.is_err() {
            return KeyResult::handled()
                .with_diagnostic(Some(diagnostics::POSITION_NOT_PERSISTED.to_owned()));
        }
        KeyResult::handled().with_diagnostic(self.refresh_after_candidate_change())
    }

    pub(super) fn pin_candidate(&mut self, index: usize) -> KeyResult {
        if self.conversion.is_some() || self.date_time_menu_page.is_some() {
            return KeyResult::unhandled();
        }
        let Some(index) = self.ranking_index(index) else {
            return KeyResult::unhandled();
        };
        let Some(selected) = self.ranking_list().get(index) else {
            return KeyResult::unhandled();
        };
        if !self.is_editable_source(selected) {
            return KeyResult::unhandled();
        }
        let selected_source = selected.source;
        let selected_word = selected.word.clone();
        let pin = FrequencyAdjustmentOptions {
            mode: FrequencyAdjustmentMode::Pin,
            trigger_count: 1,
            linear_step: 1,
        };
        if let Some(diagnostic) = self.adjust_candidate_frequency(index, pin, true) {
            return KeyResult::handled().with_diagnostic(Some(diagnostic));
        }
        if selected_source != CandidateSource::EnglishDictionary
            && self.local_mode == LocalInputMode::None
            && !self.dedicated_english
            && self.scheme().is_pinyin()
            && record_pinned_candidate_with_position_context(
                self.journal_path(),
                &self.pinyin_ranking_context(),
                Some(self.position_context(false).as_ref()),
                &selected_word,
            )
            .is_err()
        {
            return KeyResult::handled()
                .with_diagnostic(Some(diagnostics::PIN_NOT_PERSISTED.to_owned()));
        }
        KeyResult::handled().with_diagnostic(self.refresh_after_candidate_change())
    }

    pub(super) fn remove_candidate(&mut self, index: usize) -> KeyResult {
        if self.conversion.is_some() || self.date_time_menu_page.is_some() {
            return KeyResult::unhandled();
        }
        let Some(selected) = self.candidates().get(index) else {
            return KeyResult::unhandled();
        };
        let english = selected.source == CandidateSource::EnglishDictionary;
        if !self.is_editable_source(selected) || (!english && count_utf8_chars(&selected.word) <= 1)
        {
            return KeyResult::unhandled();
        }
        let kind = if english {
            PersonalDictionaryKind::English
        } else {
            PersonalDictionaryKind::Pinyin
        };
        let key = if english {
            &selected.pinyin
        } else {
            &selected.canonical_pinyin
        };
        if key.is_empty() {
            return KeyResult::unhandled();
        }
        let dictionary = self.paths.dictionary(if english {
            assets::ENGLISH_DICTIONARY
        } else {
            assets::MAIN_DICTIONARY
        });
        if delete_dictionary_candidate(&dictionary, self.journal_path(), kind, key, &selected.word)
            .is_err()
        {
            return KeyResult::handled()
                .with_diagnostic(Some(diagnostics::REMOVAL_NOT_PERSISTED.to_owned()));
        }
        KeyResult::handled().with_diagnostic(self.refresh_after_candidate_change())
    }

    fn refresh_after_candidate_change(&mut self) -> Option<String> {
        self.reset_cache();
        if self.dedicated_english {
            self.update_dedicated_english_candidates();
            return None;
        }
        if self.local_mode != LocalInputMode::None {
            return self.update_local_candidates();
        }
        self.recompute_candidates();
        None
    }

    fn mixed_from(&mut self, decoded: Vec<WordItem>) -> Vec<WordItem> {
        let english_may_lead = !self.prefix_active;
        let mixed_enabled = self.mixed_candidates_enabled && !self.caret_edit.active;
        let symbols_allowed = !self.prefix_active;
        let association_input = if self.prefix_active {
            self.prefix_query_input.as_str()
        } else {
            self.engine.request().raw_input.as_str()
        };
        let scheme = self.scheme();
        let mut mixed = self.queries.mixed(
            decoded,
            association_input,
            scheme,
            self.english_options,
            self.expressive_options,
            self.dedicated_english,
            self.local_mode,
            english_may_lead,
            symbols_allowed,
            None,
            self.engine.request().first_page_rows.get(),
            mixed_enabled,
        );
        self.insert_inline_date_time(&mut mixed);
        if self.cloud_candidates_enabled
            && self
                .online_query()
                .is_some_and(|query| query.cloud_eligible)
            && !mixed.iter().any(|item| {
                matches!(
                    item.source,
                    CandidateSource::CloudSuggestion | CandidateSource::CloudPending
                )
            })
        {
            let mut position = 1.min(mixed.len());
            while position < mixed.len()
                && (mixed[position].fixed_position > 0
                    || mixed[position].pinned_first
                    || mixed[position].source == CandidateSource::Contact)
            {
                position += 1;
            }
            mixed.insert(
                position,
                WordItem::new(
                    self.engine.request().raw_input.as_str(),
                    "☁",
                    0,
                    CandidateSource::CloudPending,
                    "",
                ),
            );
        }
        // 在线占位（CloudPending）与云/AI 行统一交由固定排位规则就位，避免后插把第 2–5 位固定词右移挤离目标席位。
        self.apply_candidate_positions(&mut mixed);
        self.prune_multi_syllable_tail(&mut mixed);
        if self.single_character_only && scheme.filters_to_single_characters() {
            mixed.retain(|item| !is_han_phrase(&item.word));
        }
        if self.local_mode == LocalInputMode::None {
            keep_expressive_rows_off_the_lead(&mut mixed);
        }
        if !self.caret_edit.active {
            self.insert_url_scheme_completions(&mut mixed);
        }
        if !self.prefix_active
            && !self.caret_edit.active
            && !self.dedicated_english
            && self.local_mode == LocalInputMode::None
            && !self.single_character_only
        {
            let input = match scheme {
                SchemeType::Quanpin => {
                    Some(Cow::Borrowed(self.engine.request().raw_input.as_str()))
                }
                SchemeType::Shuangpin => Some(self.pinyin_ranking_context()),
                _ => None,
            };
            if let Some(input) = input {
                self.queries.append_contact_candidates(&input, &mut mixed);
            }
        }
        mixed
    }

    /// 多音节常规拼音输入时收敛尾部长尾噪声：
    /// 1) 剔除混音切分漏出的非首主音节单字（如 `xianzhezhishi` 中 `xi` 下的 `洗/西`，保留 `xi'an` 等多字词）；
    /// 2) 首音节主单字超过 24 枚时清理 `weight <= 0` 的零权重生僻字并封顶 64 枚，避免候选翻百余页；
    /// 3) 将非词典合成整句封顶 12 条。置顶/固定/用户词及笔画筛选与单字模式均完整保留。
    fn prune_multi_syllable_tail(&self, items: &mut Vec<WordItem>) {
        if self.local_mode != LocalInputMode::None
            || self.dedicated_english
            || self.prefix_active
            || self.caret_edit.active
            || self.single_character_only
            || self.stroke_filter.is_some()
        {
            return;
        }
        let scheme = self.scheme();
        if !scheme.is_pinyin() {
            return;
        }
        let seg = self.pinyin_segmentation();
        let first_syl = {
            let mut parts = seg.split('\'').filter(|s| !s.is_empty());
            let Some(first) = parts.next() else {
                return;
            };
            if parts.next().is_none() || !crate::pinyin::syllables::is_intact(first) {
                return;
            }
            first
        };
        const HEAD_SINGLE_KEEP: usize = 24;
        const MAX_PRIMARY_SINGLE_CHARS: usize = 64;
        const MAX_GENERATED_SENTENCES: usize = 16;

        let primary_single_total = items
            .iter()
            .filter(|item| {
                !item.pinned_first
                    && item.fixed_position == 0
                    && item.source == CandidateSource::Database
                    && item.scheme == scheme
                    && !item.fuzzy
                    && item.corrected_from.is_empty()
                    && item.word.chars().count() == 1
                    && !self.selection_completes_composition(item)
                    && Self::item_reading(item) == first_syl
            })
            .count();
        let prune_rare_singles = primary_single_total > HEAD_SINGLE_KEEP;

        let mut primary_single_kept = 0usize;
        let mut generated_sentences_kept = 0usize;
        items.retain(|item| {
            if item.pinned_first
                || item.fixed_position > 0
                || item.source == CandidateSource::UserDatabase
                || !item.scheme.is_pinyin()
            {
                return true;
            }
            if item.sentence_association && !item.source.is_dictionary() {
                if generated_sentences_kept >= MAX_GENERATED_SENTENCES {
                    return false;
                }
                generated_sentences_kept += 1;
                return true;
            }
            if item.source == CandidateSource::Database
                && !item.fuzzy
                && item.corrected_from.is_empty()
                && item.word.chars().count() == 1
                && !self.selection_completes_composition(item)
            {
                let reading = Self::item_reading(item);
                if reading != first_syl {
                    return false;
                }
                if prune_rare_singles
                    && ((primary_single_kept >= HEAD_SINGLE_KEEP && item.weight <= 0)
                        || primary_single_kept >= MAX_PRIMARY_SINGLE_CHARS)
                {
                    return false;
                }
                primary_single_kept += 1;
            }
            true
        });
    }

    fn insert_inline_date_time(&self, items: &mut Vec<WordItem>) {
        if !self.local_mode_options.date_time_inline
            || !self.mixed_candidates_enabled
            || self.prefix_active
            || self.caret_edit.active
            || self.single_character_only
            || self.dedicated_english
            || self.local_mode != LocalInputMode::None
            || self.scheme() != SchemeType::Quanpin
        {
            return;
        }
        let raw = self.engine.request().raw_input.as_str();
        let Some((keyword, anchor, kind)) = inline_date_time_keyword(|keyword| {
            raw.bytes()
                .filter(|&byte| byte != b'\'')
                .eq(keyword.bytes())
        }) else {
            return;
        };
        let menu = self.local_mode_options.date_time_menu.then_some(keyword);
        insert_inline_date_time(items, anchor, kind, raw, || self.local_now(), menu);
    }

    /// 为 Tier 1 两阶段异步神经解码提取当前组字态的权威音节路径、统一静态分、信道代价与等跨度标记。
    pub(super) fn neural_v2_metadata(&self) -> Option<crate::neural_v2::EngineNeuralMetadata> {
        if self.local_mode != LocalInputMode::None
            || self.dedicated_english
            || self.prefix_active
            || self.caret_edit.active
            || self.stroke_filter.is_some()
            || self.conversion.is_some()
            || !self.has_composition()
            || !self.scheme().is_pinyin()
        {
            return None;
        }
        let request = self.engine.request();
        let raw_input = request.raw_input.clone();
        let segmentation = if !request.normalized_segmentation.is_empty() {
            request.normalized_segmentation.clone()
        } else {
            request.raw_segmentation.clone()
        };
        let syllable_paths = crate::neural_v2::build_syllable_paths(
            &raw_input,
            &segmentation,
            crate::neural_v2::DEFAULT_MAX_SYLLABLE_PATHS,
        );
        let primary_syllables = segmentation
            .split('\'')
            .filter(|piece| !piece.is_empty())
            .count();
        let journal = self.journal_path();
        let rank_context = self.pinyin_ranking_context();
        let pinned_word = pinned_candidate_for(journal, &rank_context);
        let now_unix = unix_now_seconds();

        let folded_raw: String = raw_input
            .chars()
            .filter(|&c| c != '\'')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let scheme = self.scheme();
        let han_clause = self.engine.rescoring_han_clause();
        let mut tail_buf = String::with_capacity(32);
        const MAX_COLLOCATION_EVAL_ITEMS: usize = 24;
        let mut collocation_eval_count = 0usize;
        let items = self.candidates();
        let has_exact_literal_dict = !folded_raw.is_empty()
            && items.iter().take(8).any(|item| {
                item.source.is_dictionary()
                    && item.corrected_from.is_empty()
                    && !item.fuzzy
                    && !item.canonical_pinyin.is_empty()
                    && item
                        .canonical_pinyin
                        .chars()
                        .filter(|&c| c != '\'')
                        .map(|c| c.to_ascii_lowercase())
                        .eq(folded_raw.chars())
            });
        let mut candidates = Vec::with_capacity(items.len());
        for item in items {
            let corrected_full_span =
                !item.corrected_from.is_empty() && !folded_raw.is_empty() && {
                    let folded_from: String = item
                        .corrected_from
                        .chars()
                        .filter(|&c| c != '\'')
                        .map(|c| c.to_ascii_lowercase())
                        .collect();
                    folded_from == folded_raw
                };
            let folded_pinyin_full = !folded_raw.is_empty() && {
                let folded_py: String = item
                    .pinyin
                    .chars()
                    .filter(|&c| c != '\'')
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                folded_py == folded_raw
            };
            let answers_key = self.selection_completes_composition(item)
                && (self.reading_answers_key(item) || corrected_full_span || folded_pinyin_full);
            let word_chars = item.word.chars().count();
            let pinyin_syllables = item
                .pinyin
                .split('\'')
                .filter(|piece| !piece.is_empty())
                .count();
            let max_allowed_syllables =
                primary_syllables.saturating_add(usize::from(item.extra_syllables));
            let canonical_syllables = if item.canonical_pinyin.is_empty() {
                word_chars
            } else {
                item.canonical_pinyin
                    .split('\'')
                    .filter(|piece| !piece.is_empty())
                    .count()
            };
            let span_ceiling = if corrected_full_span {
                max_allowed_syllables.saturating_add(1)
            } else {
                max_allowed_syllables
            };
            let exact_alt_seg = folded_pinyin_full
                && pinyin_syllables == word_chars
                && canonical_syllables == word_chars;
            let exact_span = answers_key
                && word_chars > 0
                && (primary_syllables == 0
                    || exact_alt_seg
                    || (word_chars <= span_ceiling && canonical_syllables <= span_ceiling));

            let mut user_log_prob = if item.source == CandidateSource::UserDatabase {
                crate::neural_v2::DEFAULT_USER_DICTIONARY_LOG_PRIOR
            } else {
                0.0
            };
            if self.decoupled_personal_learning
                && !rank_context.is_empty()
                && item.source.is_dictionary()
            {
                user_log_prob += personal_cache_log_bonus(
                    journal,
                    &rank_context,
                    &item.word,
                    self.app_scope,
                    now_unix,
                );
            }

            let mut stat_log_prob = if let Some(ev) = item.static_evidence() {
                Some(ev.log_prob_milli_nats as f64 / 1000.0)
            } else if item.source.is_dictionary() {
                if (word_chars >= 3 && item.weight == 100)
                    || (self.decoupled_personal_learning
                        && word_chars >= 2
                        && item.weight < 500
                        && user_log_prob > 0.0)
                {
                    None
                } else {
                    Some(crate::neural_v2::dictionary_unified_stat_log_prob(
                        item.weight,
                        word_chars.max(1),
                    ))
                }
            } else {
                None
            };

            let mut collocation_log_prob = 0.0;
            if collocation_eval_count < MAX_COLLOCATION_EVAL_ITEMS
                && exact_span
                && item.corrected_from.is_empty()
                && !item.fuzzy
            {
                collocation_eval_count += 1;
                if item.source.is_dictionary() {
                    if !han_clause.is_empty() {
                        if let Some(bonus) = self
                            .engine
                            .query_collocation_bonus(scheme, han_clause, &item.word, true)
                        {
                            let capped = bonus.min(12.0);
                            if capped > 0.0 {
                                stat_log_prob = stat_log_prob.map(|s| s + capped);
                                collocation_log_prob = 0.28 * capped;
                            }
                        }
                    }
                } else if matches!(
                    item.source,
                    CandidateSource::Generated | CandidateSource::Collocation
                ) && word_chars >= 2
                {
                    if let Some(last_word) = item.sentence_words.last() {
                        let prefix_in_cand =
                            item.word.strip_suffix(last_word.as_str()).unwrap_or("");
                        let prefix_chars = prefix_in_cand.chars().count();
                        let tail_str = if prefix_chars >= 5 || han_clause.is_empty() {
                            crate::text::last_characters(prefix_in_cand, 5)
                        } else {
                            let clause_tail =
                                crate::text::last_characters(han_clause, 5 - prefix_chars);
                            tail_buf.clear();
                            tail_buf.push_str(clause_tail);
                            tail_buf.push_str(prefix_in_cand);
                            tail_buf.as_str()
                        };
                        if !tail_str.is_empty() {
                            if let Some(bonus) = self
                                .engine
                                .query_collocation_bonus(scheme, tail_str, last_word, true)
                            {
                                let capped = bonus.min(12.0);
                                if capped > 0.0 {
                                    collocation_log_prob = 0.28 * capped;
                                }
                            }
                        }
                    }
                }
            }

            let mut channel_log_prob = 0.0;
            if !item.corrected_from.is_empty() {
                let target_py = if item.canonical_pinyin.is_empty() {
                    item.pinyin.as_ref()
                } else {
                    item.canonical_pinyin.as_ref()
                };
                channel_log_prob +=
                    typo_channel_log_penalty(&folded_raw, target_py, has_exact_literal_dict);
            }
            if item.fuzzy {
                channel_log_prob += crate::neural_v2::DEFAULT_FUZZY_CHANNEL_PENALTY;
            }
            let extra_hiatus =
                usize::from(item.extra_syllables).max(if exact_alt_seg && primary_syllables > 0 {
                    word_chars.saturating_sub(primary_syllables)
                } else {
                    0
                });
            if extra_hiatus > 0 {
                channel_log_prob += -0.6 * (extra_hiatus as f64);
            }
            let fixed_position = if item.fixed_position > 0 {
                item.fixed_position
            } else if item.pinned_first || pinned_word.as_deref() == Some(item.word.as_str()) {
                1
            } else {
                0
            };
            let pinned = fixed_position > 0;

            let exact_literal_dict = !folded_raw.is_empty()
                && item.source.is_dictionary()
                && item.corrected_from.is_empty()
                && !item.fuzzy
                && !item.canonical_pinyin.is_empty()
                && item
                    .canonical_pinyin
                    .chars()
                    .filter(|&c| c != '\'')
                    .map(|c| c.to_ascii_lowercase())
                    .eq(folded_raw.chars());

            let has_lexical_head = extra_hiatus == 0
                && (item.source.is_dictionary()
                    || item
                        .sentence_words
                        .first()
                        .is_some_and(|w| w.chars().count() >= 2));

            candidates.push(crate::neural_v2::EngineNeuralCandidateMeta {
                stat_log_prob,
                collocation_log_prob,
                channel_log_prob,
                user_log_prob,
                exact_span,
                exact_literal_dict,
                pinned,
                fixed_position,
                has_lexical_head,
            });
        }

        Some(crate::neural_v2::EngineNeuralMetadata {
            raw_input,
            segmentation,
            syllable_paths,
            candidates,
        })
    }
}

fn typo_channel_log_penalty(
    folded_raw: &str,
    canonical_pinyin: &str,
    has_exact_literal_dict: bool,
) -> f64 {
    let raw_bytes = folded_raw.as_bytes();
    let corr_bytes: Vec<u8> = canonical_pinyin
        .bytes()
        .filter(|&b| b != b'\'')
        .map(|b| b.to_ascii_lowercase())
        .collect();
    if raw_bytes.is_empty() || corr_bytes.is_empty() || raw_bytes == corr_bytes.as_slice() {
        return crate::neural_v2::DEFAULT_TYPO_CHANNEL_PENALTY;
    }
    let is_single_nasal_g = |shorter: &[u8], longer: &[u8]| -> bool {
        if longer.len() != shorter.len() + 1 {
            return false;
        }
        let mut i = 0usize;
        let mut j = 0usize;
        let mut used = false;
        while i < shorter.len() && j < longer.len() {
            if shorter[i] == longer[j] {
                i += 1;
                j += 1;
            } else if !used && longer[j] == b'g' && j > 0 && longer[j - 1] == b'n' {
                used = true;
                j += 1;
            } else {
                return false;
            }
        }
        used || (j + 1 == longer.len() && longer[j] == b'g' && j > 0 && longer[j - 1] == b'n')
    };
    if is_single_nasal_g(raw_bytes, &corr_bytes) || is_single_nasal_g(&corr_bytes, raw_bytes) {
        return if has_exact_literal_dict { -3.4 } else { -2.4 };
    }
    if raw_bytes.len() == corr_bytes.len() {
        let diffs: Vec<usize> = raw_bytes
            .iter()
            .zip(&corr_bytes)
            .enumerate()
            .filter_map(|(idx, (a, b))| (a != b).then_some(idx))
            .collect();
        if diffs.len() == 2
            && diffs[1] == diffs[0] + 1
            && raw_bytes[diffs[0]] == corr_bytes[diffs[1]]
            && raw_bytes[diffs[1]] == corr_bytes[diffs[0]]
        {
            return if has_exact_literal_dict { -3.6 } else { -2.6 };
        }
        return if has_exact_literal_dict { -4.6 } else { -2.8 };
    }
    if !has_exact_literal_dict {
        return -2.8;
    }
    if raw_bytes.len() > corr_bytes.len() {
        let is_repeated_slip = raw_bytes.len() == corr_bytes.len() + 1
            && (0..=corr_bytes.len()).any(|pos| {
                raw_bytes[..pos] == corr_bytes[..pos]
                    && raw_bytes[pos + 1..] == corr_bytes[pos..]
                    && ((pos > 0 && raw_bytes[pos] == corr_bytes[pos - 1])
                        || (pos < corr_bytes.len() && raw_bytes[pos] == corr_bytes[pos]))
            });
        return if is_repeated_slip { -3.8 } else { -7.0 };
    }
    -4.4
}

#[cfg(test)]
mod tests {
    use super::{clone_candidate_rows, english_position_context, plain_position_context};
    use crate::types::{CandidateSource, WordItem};

    #[test]
    fn english_position_context_preserves_non_ascii_while_lowercasing_ascii() {
        assert_eq!(english_position_context("HeLLo 世界"), "english:hello 世界");
    }

    #[test]
    fn plain_position_context_reserves_source_capacity() {
        let source: String = (0..100)
            .map(|index| if index % 5 == 0 { '\'' } else { 'a' })
            .collect();
        let plain = plain_position_context(&source);
        assert_eq!(plain, source.replace('\'', ""));
        assert_eq!(plain.capacity(), source.len());
    }

    #[test]
    fn cloning_candidate_rows_reuses_existing_storage() {
        let source = vec![
            WordItem::new("ni", "你", 1, CandidateSource::Database, "ni"),
            WordItem::new("hao", "好", 1, CandidateSource::Database, "hao"),
        ];
        let mut destination = source.clone();

        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            clone_candidate_rows(&source, &mut destination);
        });

        assert_eq!(destination, source);
        assert_eq!(allocations, 0);
    }
}
