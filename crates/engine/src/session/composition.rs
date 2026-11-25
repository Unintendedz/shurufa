//! 选词后组字推进、音节分词读取与预编辑显示。

use std::borrow::Cow;

use super::input::{CreatingWordProgress, InputSession};
use crate::pinyin::autocorrect::{
    autocorrect_cut_detail, looks_like_syllable_with_jianpin_tail, AutocorrectCutSegment,
};
use crate::pinyin::segment::is_complete_pinyin_input;
use crate::pinyin::syllables::{is_intact, normalized_syllable};
use crate::shuangpin::query::{
    is_complete_input, raw_length_for_effective_prefix, remove_manual_delimiters,
};
use crate::shuangpin::ShuangpinProfile;
use crate::text::count_han_chars;
use crate::types::{
    request_autocorrect_mask, LocalInputMode, QueryRequest, SchemeKey, SchemeType, WordItem,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct SelectionTransition {
    pub continues_composition: bool,
    pub full_pure_pinyin: String,
    pub current_segmentation_with_cases: String,
    pub selected_canonical_pinyin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ShuangpinCompositionBase<'a> {
    pub raw_input: Cow<'a, str>,
    pub raw_input_with_cases: Cow<'a, str>,
    pub effective_raw_input: Cow<'a, str>,
    pub effective_raw_input_with_cases: Cow<'a, str>,
}

pub(super) fn resolve_shuangpin_composition_base<'a>(
    request: &'a QueryRequest,
    _profile: &ShuangpinProfile,
) -> ShuangpinCompositionBase<'a> {
    let raw_input = Cow::Borrowed(request.raw_input.as_str());
    let raw_input_with_cases = if request.raw_input_with_cases.is_empty() {
        Cow::Borrowed(request.raw_input.as_str())
    } else {
        Cow::Borrowed(request.raw_input_with_cases.as_str())
    };
    let effective_raw_input = remove_manual_delimiters_cow(request.raw_input.as_str());
    let raw_input_with_cases_source = if request.raw_input_with_cases.is_empty() {
        request.raw_input.as_str()
    } else {
        request.raw_input_with_cases.as_str()
    };
    let effective_raw_input_with_cases = remove_manual_delimiters_cow(raw_input_with_cases_source);
    ShuangpinCompositionBase {
        effective_raw_input,
        effective_raw_input_with_cases,
        raw_input,
        raw_input_with_cases,
    }
}

fn remove_manual_delimiters_cow(raw: &str) -> Cow<'_, str> {
    if raw.contains('\'') {
        Cow::Owned(remove_manual_delimiters(raw))
    } else {
        Cow::Borrowed(raw)
    }
}

fn remove_delimiters(segmented: &str) -> String {
    let mut result = String::with_capacity(segmented.len());
    result.extend(segmented.chars().filter(|c| *c != '\''));
    result
}

fn with_trailing_separator<'a>(preedit: Cow<'a, str>, request: &QueryRequest) -> Cow<'a, str> {
    if request.raw_input_with_cases.ends_with('\'') && !preedit.ends_with('\'') {
        let mut preedit = preedit.into_owned();
        preedit.push('\'');
        Cow::Owned(preedit)
    } else {
        preedit
    }
}

fn remove_consumed_leading_separators(raw: &str) -> &str {
    raw.trim_start_matches('\'')
}

pub(super) fn normalize_canonical_pinyin_for_word(pinyin: &str, word: &str) -> String {
    if canonical_pinyin_is_valid_for_word(pinyin, word) {
        pinyin.to_owned()
    } else {
        String::new()
    }
}

fn canonical_pinyin_is_valid_for_word(pinyin: &str, word: &str) -> bool {
    if pinyin.is_empty() {
        return false;
    }
    let segment_count = pinyin.bytes().filter(|&byte| byte == b'\'').count() + 1;
    if segment_count != count_han_chars(word) {
        return false;
    }
    !pinyin
        .split('\'')
        .any(|segment| segment.is_empty() || !is_complete_pinyin_input(segment))
}

pub(super) fn append_canonical_pinyin(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        return suffix.to_owned();
    }
    if suffix.is_empty() {
        return String::new();
    }
    let mut result = String::with_capacity(prefix.len() + 1 + suffix.len());
    result.push_str(prefix);
    result.push('\'');
    result.push_str(suffix);
    result
}

pub(super) fn fold_autocorrect_letters(text: &str) -> String {
    text.bytes()
        .filter(|byte| *byte != b'\'')
        .map(|byte| match byte.to_ascii_lowercase() {
            b'v' => 'u',
            lower => lower as char,
        })
        .collect()
}

fn folded_autocorrect_byte(byte: u8) -> Option<u8> {
    if byte == b'\'' {
        None
    } else {
        Some(match byte.to_ascii_lowercase() {
            b'v' => b'u',
            lower => lower,
        })
    }
}

fn folded_letters_equal(left: &str, right: &str) -> bool {
    left.bytes()
        .filter_map(folded_autocorrect_byte)
        .eq(right.bytes().filter_map(folded_autocorrect_byte))
}

fn folded_segments_equal(segments: &[AutocorrectCutSegment], text: &str) -> bool {
    segments
        .iter()
        .flat_map(|segment| segment.syllable.bytes().filter_map(folded_autocorrect_byte))
        .eq(text.bytes().filter_map(folded_autocorrect_byte))
}

pub(super) fn build_quanpin_autocorrect_display<'a>(request: &'a QueryRequest) -> Cow<'a, str> {
    let cased = if request.raw_input_with_cases.is_empty() {
        &request.raw_input
    } else {
        &request.raw_input_with_cases
    };
    let base = if request.raw_segmentation.is_empty() {
        cased
    } else {
        &request.raw_segmentation
    };
    if request.raw_input.is_empty() || cased.is_empty() {
        return Cow::Borrowed(base);
    }
    let types = request_autocorrect_mask(
        request.enable_quanpin_autocorrect_transposition,
        request.enable_quanpin_autocorrect_neighbor,
    );
    let letters_rewritten = !folded_letters_equal(base, cased);
    if !letters_rewritten
        && (types == 0 || is_complete_pinyin_input(request.raw_input.trim_end_matches('\'')))
    {
        return Cow::Borrowed(base);
    }
    if looks_like_syllable_with_jianpin_tail(&request.raw_input) {
        return Cow::Borrowed(base);
    }
    let folded_input = fold_autocorrect_letters(cased);
    let cut = autocorrect_cut_detail(&folded_input, types).filter(|cut| !cut.is_empty());
    if let Some(cut) = cut {
        if !letters_rewritten || folded_segments_equal(&cut.segments, base) {
            let boundary_count = cut.segments.len() - 1;
            let mut display = String::with_capacity(cased.len() + cut.segments.len());
            let mut letter_index = 0;
            let mut boundary_index = 0;
            for byte in cased.bytes().filter(|byte| *byte != b'\'') {
                display.push(byte as char);
                letter_index += 1;
                if boundary_index < boundary_count {
                    let segment = &cut.segments[boundary_index];
                    if letter_index == segment.start + segment.raw_text.len() {
                        display.push('\'');
                        boundary_index += 1;
                    }
                }
            }
            return Cow::Owned(display);
        }
    }
    if letters_rewritten {
        Cow::Owned(remove_delimiters(cased))
    } else {
        Cow::Borrowed(base)
    }
}

impl InputSession {
    fn selected_quanpin_length(selected: &WordItem) -> usize {
        if selected.corrected_from.is_empty() {
            remove_delimiters(&selected.pinyin).len()
        } else {
            selected.corrected_from.len()
        }
    }

    pub(super) fn selection_completes_composition(&self, selected: &WordItem) -> bool {
        if let Some(consumed) = self.literal_candidate_consumed(selected) {
            return consumed == self.raw_with_cases().len();
        }
        self.selection_completes_request(self.engine.request(), selected)
    }

    pub(super) fn reading_answers_key(&self, item: &WordItem) -> bool {
        if !item.source.is_dictionary()
            || item.canonical_pinyin.is_empty()
            || item.fuzzy
            || !item.corrected_from.is_empty()
        {
            return true;
        }
        let typed = match item.scheme {
            SchemeType::Quanpin => item.pinyin.as_ref(),
            SchemeType::Shuangpin => self.engine.request().normalized_segmentation.as_str(),
            _ => return true,
        };
        let mut typed = typed.split('\'').filter(|piece| !piece.is_empty());
        let mut reading = item
            .canonical_pinyin
            .split('\'')
            .filter(|piece| !piece.is_empty());
        loop {
            match (typed.next(), reading.next()) {
                (None, None) => return true,
                (Some(typed), Some(reading)) => {
                    if !Self::syllable_answers(typed, reading) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }

    fn syllable_answers(typed: &str, reading: &str) -> bool {
        let matches = |typed: &str, reading: &str| {
            if is_intact(typed) {
                typed.eq_ignore_ascii_case(reading)
            } else {
                reading.len() >= typed.len()
                    && reading.as_bytes()[..typed.len()].eq_ignore_ascii_case(typed.as_bytes())
            }
        };
        if matches(typed, reading) {
            return true;
        }
        typed != reading && matches(&normalized_syllable(typed), &normalized_syllable(reading))
    }

    pub(super) fn selection_completes_request(
        &self,
        request: &QueryRequest,
        selected: &WordItem,
    ) -> bool {
        if self.is_shuangpin() {
            let base = resolve_shuangpin_composition_base(request, self.shuangpin_profile());
            let word_length = count_han_chars(&selected.word) * 2;
            let total = base.effective_raw_input.len();
            let mut consumed = remove_delimiters(&selected.pinyin).len();
            if consumed == 0 || consumed > total {
                consumed = word_length.min(total);
            }
            return consumed >= total;
        }
        let selected_length = Self::selected_quanpin_length(selected);
        let raw = request.raw_input.as_str();
        let raw_with_cases = if request.raw_input_with_cases.is_empty() {
            raw
        } else {
            request.raw_input_with_cases.as_str()
        };
        let consumed_raw = raw_length_for_effective_prefix(raw_with_cases, selected_length);
        selected.pinyin.is_empty()
            || selected_length >= request.normalized_input.len()
            || consumed_raw >= raw.len()
    }

    pub(super) fn advance_composition_after_selection(
        &mut self,
        selected: &WordItem,
    ) -> SelectionTransition {
        let mut transition = SelectionTransition {
            selected_canonical_pinyin: selected.canonical_pinyin.to_string(),
            continues_composition: !self.selection_completes_composition(selected),
            ..SelectionTransition::default()
        };
        if self.is_shuangpin() {
            let (full_pure_pinyin, rest) = {
                let request = self.engine.request();
                let base = resolve_shuangpin_composition_base(request, self.shuangpin_profile());
                let word_length = count_han_chars(&selected.word) * 2;
                let total = base.effective_raw_input.len();
                let full_pure_pinyin = base.effective_raw_input.clone().into_owned();
                let rest = if transition.continues_composition {
                    let mut consumed = remove_delimiters(&selected.pinyin).len();
                    if consumed == 0 || consumed > total {
                        consumed = word_length.min(total);
                    }
                    let start = raw_length_for_effective_prefix(&base.raw_input_with_cases, consumed);
                    let end = base.raw_input.len();
                    Some((
                        remove_consumed_leading_separators(&base.raw_input[start..end]).to_owned(),
                        remove_consumed_leading_separators(&base.raw_input_with_cases[start..end])
                            .to_owned(),
                    ))
                } else {
                    None
                };
                (full_pure_pinyin, rest)
            };
            transition.full_pure_pinyin = full_pure_pinyin;
            if let Some((rest, rest_with_cases)) = rest {
                self.engine
                    .replace_active_raw_input(&rest, &rest_with_cases);
                self.online_requests.invalidate();
                self.update_mixed_candidates();
            }
            transition.current_segmentation_with_cases =
                self.pinyin_segmentation_with_cases().into_owned();
            return transition;
        }

        if transition.continues_composition {
            let (full_pure_pinyin, raw, raw_with_cases) = {
                let request = self.engine.request();
                let raw = request.raw_input.as_str();
                let raw_with_cases = if request.raw_input_with_cases.is_empty() {
                    raw
                } else {
                    request.raw_input_with_cases.as_str()
                };
                (
                    request.normalized_input.clone(),
                    raw.to_owned(),
                    raw_with_cases.to_owned(),
                )
            };
            transition.full_pure_pinyin = full_pure_pinyin;
            let consumed = raw_length_for_effective_prefix(
                &raw_with_cases,
                Self::selected_quanpin_length(selected),
            );
            let rest = remove_consumed_leading_separators(&raw[consumed..]);
            let rest_with_cases = remove_consumed_leading_separators(&raw_with_cases[consumed..]);
            self.engine.replace_active_raw_input(rest, rest_with_cases);
            self.online_requests.invalidate();
            self.update_mixed_candidates();
            transition.current_segmentation_with_cases =
                self.pinyin_segmentation_with_cases().into_owned();
            return transition;
        }
        let request = self.engine.request();
        transition.full_pure_pinyin = request.normalized_input.clone();
        transition.current_segmentation_with_cases =
            self.pinyin_segmentation_with_cases().into_owned();
        transition
    }

    pub(super) fn update_creating_word_progress(
        current_pinyin: &str,
        current_word: &str,
        selected_word: &str,
        transition: &SelectionTransition,
    ) -> CreatingWordProgress {
        let mut word = String::with_capacity(current_word.len() + selected_word.len());
        word.push_str(current_word);
        word.push_str(selected_word);
        let selected_storable = canonical_pinyin_is_valid_for_word(
            &transition.selected_canonical_pinyin,
            selected_word,
        );
        let prior_parts_storable = current_word.is_empty() || !current_pinyin.is_empty();
        let pinyin = if prior_parts_storable && selected_storable {
            append_canonical_pinyin(current_pinyin, &transition.selected_canonical_pinyin)
        } else {
            String::new()
        };
        let completed = !transition.continues_composition;
        let can_store = completed && canonical_pinyin_is_valid_for_word(&pinyin, &word);
        let mut preedit =
            String::with_capacity(word.len() + transition.current_segmentation_with_cases.len());
        preedit.push_str(&word);
        preedit.push_str(&transition.current_segmentation_with_cases);
        CreatingWordProgress {
            preedit,
            pinyin,
            word,
            completed,
            can_store,
        }
    }

    pub(super) fn pinyin_segmentation(&self) -> Cow<'_, str> {
        let request = self.engine.request();
        if request.normalized_segmentation.is_empty() {
            Cow::Borrowed(&request.segmentation)
        } else {
            Cow::Borrowed(&request.normalized_segmentation)
        }
    }

    pub(super) fn pinyin_segmentation_with_cases(&self) -> Cow<'_, str> {
        if self.has_literal_input() {
            return Cow::Borrowed(self.raw_with_cases());
        }
        let request = self.engine.request();
        match self.engine.current_scheme_type() {
            SchemeType::Shuangpin if self.shuangpin_preedit_uses_raw => with_trailing_separator(
                if request.raw_segmentation.is_empty() {
                    Cow::Borrowed(&request.raw_input)
                } else {
                    Cow::Borrowed(&request.raw_segmentation)
                },
                request,
            ),
            SchemeType::Quanpin => {
                with_trailing_separator(build_quanpin_autocorrect_display(request), request)
            }
            SchemeType::Shuangpin => with_trailing_separator(self.pinyin_segmentation(), request),
            _ => Cow::Borrowed(self.raw_with_cases()),
        }
    }

    pub(super) fn is_all_complete_pure_pinyin(&self) -> bool {
        let request = self.engine.request();
        match self.engine.current_scheme_type() {
            SchemeType::Shuangpin => is_complete_input(&request.raw_input, self.shuangpin_profile()),
            SchemeType::Quanpin => {
                let segmentation = self.pinyin_segmentation();
                !segmentation.is_empty() && is_complete_pinyin_input(segmentation.as_ref())
            }
            _ => false,
        }
    }

    pub(super) fn candidate_annotations(&self) -> Vec<String> {
        if self.local_mode.annotates_rows() {
            return self
                .candidates()
                .iter()
                .map(|item| match self.local_mode {
                    LocalInputMode::Command => self
                        .queries
                        .command_title(&item.pinyin)
                        .unwrap_or_default()
                        .to_owned(),
                    LocalInputMode::Mention => self
                        .queries
                        .mention_annotation(&item.word, &item.pinyin)
                        .to_owned(),
                    _ => String::new(),
                })
                .collect();
        }
        self.candidates()
            .iter()
            .map(|item| item.corrected_from.to_string())
            .collect()
    }

    pub(super) fn apply_pending_sequence(&mut self) {
        self.caret = None;
        let raw = self
            .pending_sequence
            .take()
            .unwrap_or_else(|| self.engine.request().raw_input.clone());
        let raw_with_cases = self
            .pending_sequence_with_cases
            .take()
            .unwrap_or_else(|| raw.clone());
        self.engine.replace_active_raw_input(&raw, &raw_with_cases);
        self.online_requests.invalidate();
        self.update_mixed_candidates();
    }

    pub(super) fn recompute_candidates(&mut self) {
        if self.pending_sequence.is_some() || self.pending_sequence_with_cases.is_some() {
            self.apply_pending_sequence();
            return;
        }
        self.engine.handle_key(SchemeKey::Requery);
        self.update_mixed_candidates();
    }

    pub(super) fn candidates_follow_pinyin(&self) -> bool {
        self.engine
            .current_scheme_type()
            .follows_pinyin_candidates()
    }
}

#[cfg(test)]
#[path = "composition/canonical_reading_tests.rs"]
mod canonical_reading_tests;

#[cfg(test)]
#[path = "composition/creating_word_reading_tests.rs"]
mod creating_word_reading_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_readings_need_one_complete_syllable_per_character() {
        assert_eq!(
            normalize_canonical_pinyin_for_word("ni'hao", "你好"),
            "ni'hao"
        );
        assert_eq!(normalize_canonical_pinyin_for_word("ni'hao", "你"), "");
        assert_eq!(normalize_canonical_pinyin_for_word("ni'h", "你好"), "");
        assert_eq!(normalize_canonical_pinyin_for_word("", "你"), "");
    }

    #[test]
    fn canonical_readings_append_and_empty_on_a_gap() {
        assert_eq!(append_canonical_pinyin("", "ni"), "ni");
        assert_eq!(append_canonical_pinyin("ni", "hao"), "ni'hao");
        let result = append_canonical_pinyin("shan", "shui");
        assert_eq!(result.capacity(), result.len());
        assert_eq!(append_canonical_pinyin("ni", ""), "");
        assert_eq!(result, "shan'shui");
    }

    #[test]
    fn an_unknown_earlier_reading_makes_the_phrase_unstorable() {
        let last = SelectionTransition {
            selected_canonical_pinyin: "te'le".into(),
            ..SelectionTransition::default()
        };
        let known = InputSession::update_creating_word_progress("xi", "西", "特乐", &last);
        assert!(known.completed && known.can_store);
        assert_eq!(known.pinyin, "xi'te'le");
        assert_eq!(known.word, "西特乐");

        let unknown = InputSession::update_creating_word_progress("", "西", "特乐", &last);
        assert!(unknown.completed);
        assert!(!unknown.can_store);
        assert!(unknown.pinyin.is_empty());
    }

    #[test]
    fn consumed_separators_are_dropped_from_the_rest() {
        assert_eq!(remove_consumed_leading_separators("''hao"), "hao");
        assert_eq!(remove_consumed_leading_separators("hao'"), "hao'");
        assert_eq!(remove_delimiters("ni'hao'"), "nihao");
    }
}
