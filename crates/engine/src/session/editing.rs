//! 光标移动、行内编辑文本、音节边界与光标前缀解码。

use super::input::InputSession;
use crate::local::url;
use crate::local::GENERATED_MODE_INPUT_LIMIT;
use crate::shuangpin::query::segment_raw_boundaries;
use crate::types::{Command, KeyResult, LocalInputMode, SchemeType};

impl InputSession {
    pub(super) fn editing_text(&self) -> String {
        if self.dedicated_english {
            return self.dedicated_english_preedit.clone();
        }
        match self.local_mode {
            LocalInputMode::None => self.raw_with_cases().to_owned(),
            _ => self.local_preedit.clone(),
        }
    }

    pub(super) fn editing_text_len(&self) -> usize {
        if self.dedicated_english {
            return self.dedicated_english_preedit.len();
        }
        match self.local_mode {
            LocalInputMode::None => self.raw_with_cases().len(),
            _ => self.local_preedit.len(),
        }
    }

    fn editing_begin(&self) -> usize {
        usize::from(!matches!(
            self.local_mode,
            LocalInputMode::None | LocalInputMode::Url
        ))
    }

    pub(super) fn caret_position(&self) -> usize {
        let length = self.editing_text_len();
        self.caret.unwrap_or(length).min(length)
    }

    pub(super) fn edit_at_caret(&mut self, command: Command) -> KeyResult {
        let text_len = self.editing_text_len();
        let mut caret = self.caret.unwrap_or(text_len).min(text_len);
        let begin = self.editing_begin();
        match command {
            Command::MoveLeft => caret = caret.saturating_sub(1).max(begin),
            Command::MoveRight => caret = (caret + 1).min(text_len),
            Command::MoveHome => caret = begin,
            Command::MoveEnd => caret = text_len,
            Command::Backspace => {
                if caret <= begin {
                    return KeyResult::handled();
                }
                let mut text = self.editing_text();
                caret -= 1;
                let removed = text.remove(caret);
                return self.delete_editing_character(text, caret, removed);
            }
            Command::DeleteForward => {
                if caret == text_len {
                    return KeyResult::handled();
                }
                let mut text = self.editing_text();
                let removed = text.remove(caret);
                return self.delete_editing_character(text, caret, removed);
            }
            _ => return KeyResult::unhandled(),
        }
        self.caret = Some(caret);
        self.update_mixed_candidates();
        KeyResult::handled()
    }

    fn delete_editing_character(&mut self, text: String, caret: usize, removed: char) -> KeyResult {
        if self.local_mode == LocalInputMode::Url {
            if text.is_empty() {
                self.reset_composition();
                return KeyResult::handled();
            }
            if caret == text.len() && self.url_reverts(&text, removed) {
                self.restore_composition_from_url(text);
                return KeyResult::handled();
            }
        }
        self.replace_editing_text(&text, caret)
    }

    pub(super) fn insert_at_caret(&mut self, value: u8) -> KeyResult {
        let mut text = self.editing_text();
        let caret = self.caret_position();
        let lower = value.is_ascii_lowercase();
        let upper = value.is_ascii_uppercase();
        let mut accepted = lower || upper;
        if !self.dedicated_english {
            match self.local_mode {
                LocalInputMode::Unicode => {
                    accepted = value.is_ascii_hexdigit()
                        || (value == b'+' && caret == 1 && !text.contains('+'));
                }
                LocalInputMode::QuickPhrase | LocalInputMode::DateTime => accepted = lower,
                LocalInputMode::None => {
                    let scheme = self.scheme();
                    accepted = lower
                        || (upper && matches!(scheme, SchemeType::Quanpin | SchemeType::Shuangpin));
                    if value == b';'
                        && scheme == SchemeType::Shuangpin
                        && self.profile.uses_semicolon_key()
                    {
                        accepted = text[..caret].rsplit('\'').next().unwrap_or("").len() % 2 == 1;
                    }
                    if value == b'\'' && scheme.accepts_apostrophe() {
                        accepted = caret > 0;
                    }
                }
                LocalInputMode::Emoji | LocalInputMode::Kaomoji => {
                    accepted = accepted || value == b'\'';
                }
                LocalInputMode::Symbol => accepted = value.is_ascii_alphabetic(),
                LocalInputMode::SuperJianpin | LocalInputMode::TemporaryEnglish => {}
                LocalInputMode::Expression => {
                    accepted = text.len() < GENERATED_MODE_INPUT_LIMIT
                        && (self.expression_takes_unit_letter(value, caret)
                            || (value == b'\'' && self.expression_takes_unit_separator(caret))
                            || self
                                .local_mode
                                .spelling_symbols()
                                .as_bytes()
                                .contains(&value));
                }
                LocalInputMode::Command | LocalInputMode::Mention => {
                    accepted = text.len() < GENERATED_MODE_INPUT_LIMIT
                        && (lower
                            || (value == b'\'' && self.command_takes_word_separator(caret))
                            || self.command_takes_argument_symbol(value, caret));
                }
                LocalInputMode::Url => {
                    if !url::accepts(value) {
                        return KeyResult::unhandled();
                    }
                    if text.len() >= url::INPUT_LIMIT {
                        return KeyResult::handled();
                    }
                    accepted = true;
                }
                LocalInputMode::TemporaryJapanese | LocalInputMode::Chaizi => {
                    accepted = false;
                }
            }
        }
        if !accepted {
            if self.dedicated_english {
                return KeyResult::handled();
            }
            if !matches!(
                self.local_mode,
                LocalInputMode::None | LocalInputMode::TemporaryEnglish | LocalInputMode::Url
            ) {
                let mut result = self.commit_raw();
                result.handled = false;
                return result;
            }
            return KeyResult::unhandled();
        }
        let bytes = text.as_bytes();
        if value == b'\''
            && self.local_mode != LocalInputMode::Url
            && ((caret > 0 && bytes[caret - 1] == b'\'') || bytes.get(caret) == Some(&b'\''))
        {
            return KeyResult::handled();
        }
        text.insert(caret, value as char);
        self.replace_editing_text(&text, caret + 1)
    }

    pub(super) fn replace_editing_text(&mut self, text: &str, caret: usize) -> KeyResult {
        let mut diagnostic = None;
        if self.dedicated_english {
            self.dedicated_english_preedit = text.to_owned();
            self.update_dedicated_english_candidates();
        } else if self.local_mode != LocalInputMode::None {
            self.local_preedit = text.to_owned();
            diagnostic = self.update_local_candidates();
        } else {
            self.pending_sequence = Some(text.to_ascii_lowercase());
            self.pending_sequence_with_cases = Some(text.to_owned());
            self.apply_pending_sequence();
        }
        self.caret = Some(caret);
        self.update_mixed_candidates();
        self.invalidate_online_requests();
        self.discard_abandoned_phrase_progress();
        KeyResult::handled().with_diagnostic(diagnostic)
    }

    pub(super) fn segment_raw_boundaries(&self) -> Vec<usize> {
        if self.dedicated_english || self.local_mode != LocalInputMode::None {
            return Vec::new();
        }
        let raw_with_cases = self.raw_with_cases();
        if raw_with_cases.is_empty() {
            return Vec::new();
        }
        if self.has_literal_input() {
            return self.literal_raw_boundaries();
        }
        match self.engine.current_scheme_type() {
            SchemeType::Shuangpin => segment_raw_boundaries(raw_with_cases, self.shuangpin_profile()),
            SchemeType::Quanpin => quanpin_raw_boundaries(
                raw_with_cases,
                self.pinyin_segmentation_with_cases().as_ref(),
            ),
            _ => Vec::new(),
        }
    }

    pub(super) fn set_caret(&mut self, caret: Option<usize>) {
        self.exit_date_time_menu();
        self.leave_conversion();
        if self.engine.current_scheme_type().locks_caret()
            && !self.dedicated_english
            && self.local_mode == LocalInputMode::None
        {
            self.caret = None;
            return;
        }
        let length = self.editing_text_len();
        let begin = self.editing_begin().min(length);
        self.caret = caret.map(|position| position.clamp(begin, length));
        if !self.dedicated_english && self.local_mode == LocalInputMode::None {
            self.update_mixed_candidates();
        }
    }

    pub(super) fn prefix_end(&self) -> usize {
        let raw_length = self.raw_with_cases().len();
        if self.caret.is_none() {
            return raw_length;
        }
        let boundaries = self.segment_raw_boundaries();
        let caret = self.caret_position();
        boundaries
            .iter()
            .rev()
            .find(|boundary| **boundary <= caret)
            .copied()
            .unwrap_or(raw_length)
    }

    pub(super) fn pending_suffix(&self) -> String {
        let raw = self.raw_with_cases();
        let end = self.prefix_end();
        raw.get(end..).unwrap_or_default().to_owned()
    }

    pub(super) fn refresh_prefix_candidates(&mut self) {
        self.prefix_active = false;
        if self.refresh_caret_edit() {
            return;
        }
        if self.caret.is_none() || self.dedicated_english || self.local_mode != LocalInputMode::None
        {
            self.prefix_candidates.clear();
            self.prefix_query_input.clear();
            return;
        }
        let end = self.prefix_end();
        let raw_with_cases = self.raw_with_cases();
        if end == 0 || end >= raw_with_cases.len() {
            self.prefix_candidates.clear();
            self.prefix_query_input.clear();
            return;
        }
        let prefix_with_cases = &raw_with_cases[..end];
        let lowercase = prefix_with_cases
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'\'');
        if lowercase {
            if self.prefix_query_input != prefix_with_cases {
                let prefix = prefix_with_cases.to_owned();
                self.prefix_candidates = self.engine.query_raw_candidates(&prefix, &prefix);
                self.prefix_query_input = prefix;
            }
        } else {
            let prefix = prefix_with_cases.to_ascii_lowercase();
            if self.prefix_query_input != prefix {
                let raw_with_cases = prefix_with_cases.to_owned();
                self.prefix_candidates = self.engine.query_raw_candidates(&prefix, &raw_with_cases);
                self.prefix_query_input = prefix;
            }
        }
        self.prefix_active = true;
    }

    pub(super) fn raw_with_cases(&self) -> &str {
        let request = self.engine.request();
        if request.raw_input_with_cases.is_empty() {
            &request.raw_input
        } else {
            &request.raw_input_with_cases
        }
    }
}

pub(super) fn quanpin_raw_boundaries(raw: &str, display: &str) -> Vec<usize> {
    let raw_bytes = raw.as_bytes();
    let raw_letter_count = raw_bytes.iter().filter(|byte| **byte != b'\'').count();
    if raw_letter_count == 0 {
        return Vec::new();
    }
    let mut boundaries = Vec::with_capacity(raw_letter_count + 1);
    boundaries.push(0);
    let mut raw_offset = 0;
    let mut letters_seen = 0;
    for byte in display.bytes() {
        if byte != b'\'' {
            letters_seen += 1;
            while raw_offset < raw_bytes.len() && raw_bytes[raw_offset] == b'\'' {
                raw_offset += 1;
            }
            raw_offset = raw_offset.saturating_add(1);
            continue;
        }
        let mut next = raw_offset;
        while next < raw_bytes.len() && raw_bytes[next] == b'\'' {
            next += 1;
        }
        if next < raw_bytes.len() {
            boundaries.push(next);
        }
    }
    if letters_seen != raw_letter_count {
        return Vec::new();
    }
    boundaries.push(raw.len());
    boundaries.dedup();
    boundaries
}

#[cfg(test)]
mod tests {
    use super::quanpin_raw_boundaries;

    #[test]
    fn a_lone_initial_is_a_complete_unit_but_a_split_syllable_is_not() {
        assert_eq!(quanpin_raw_boundaries("nihaoma", "ni'hao'ma"), [0, 2, 5, 7]);
        assert_eq!(quanpin_raw_boundaries("nhaoma", "n'hao'ma"), [0, 1, 4, 6]);
    }

    #[test]
    fn quanpin_boundaries_do_not_allocate_temporary_raw_offsets() {
        let (boundaries, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            quanpin_raw_boundaries("nihaoma", "ni'hao'ma")
        });

        assert_eq!(boundaries, [0, 2, 5, 7]);
        assert_eq!(allocations, 1, "boundary allocations: {allocations}");
    }
}
