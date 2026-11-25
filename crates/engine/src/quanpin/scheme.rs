//! 全拼按键处理与查询请求切分构造。

use crate::pinyin::segment::{cut_pinyin_by_mode, join_segments, CutMode};
use crate::shuangpin::query::{apply_segmentation_cases_into, remove_manual_delimiters};
use crate::types::{QueryRequest, SchemeKey, SchemeType};

fn replace_string(target: &mut String, source: String) {
    if target.capacity() == 0 {
        *target = source;
    } else {
        target.clone_from(&source);
    }
}

#[derive(Debug, Clone, Default)]
pub struct QuanpinScheme {
    raw: String,
}

impl QuanpinScheme {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.raw.clear();
    }

    /// 接受 ASCII 字母与单撇号分隔符；退格弹出末字符。
    pub fn handle_key(&mut self, key: SchemeKey) -> bool {
        match key {
            SchemeKey::Letter(letter) if letter.is_ascii_alphabetic() => {
                self.raw.push(char::from(letter));
                true
            }
            SchemeKey::Apostrophe => {
                if !self.raw.ends_with('\'') {
                    self.raw.push('\'');
                    true
                } else {
                    false
                }
            }
            SchemeKey::Backspace => self.raw.pop().is_some(),
            SchemeKey::Letter(_) | SchemeKey::Semicolon | SchemeKey::Symbol(_) | SchemeKey::Requery => false,
        }
    }

    #[cfg(test)]
    pub fn build_request(&self) -> QueryRequest {
        let mut request = QueryRequest::default();
        self.build_request_into(&mut request);
        request
    }

    /// 将全拼请求写入已有存储，避免逐键刷新重复分配按键和切分字符串。
    pub fn build_request_into(&self, request: &mut QueryRequest) {
        Self::build_request_from_raw_into(&self.raw, &self.raw, request);
    }

    /// 从原文直接写入查询请求，光标前缀查询不必创建临时方案。
    pub fn build_request_from_raw_into(
        raw: &str,
        raw_with_cases: &str,
        request: &mut QueryRequest,
    ) {
        request.scheme = SchemeType::Quanpin;
        request.raw_input.clear();
        request.raw_input.extend(
            raw.bytes()
                .map(|byte| char::from(byte.to_ascii_lowercase())),
        );
        let raw_with_cases = if raw_with_cases.is_empty() {
            raw
        } else {
            raw_with_cases
        };
        request.raw_input_with_cases.clear();
        request.raw_input_with_cases.push_str(raw_with_cases);
        Self::apply_segmentation(request, CutMode::Correction);
    }

    pub fn preedit(&self) -> String {
        self.raw.clone()
    }

    pub fn set_raw_input(&mut self, raw: &str, raw_with_cases: &str) {
        let source = if raw_with_cases.is_empty() {
            raw
        } else {
            raw_with_cases
        };
        self.raw.clear();
        self.raw.reserve(source.len());
        self.raw.push_str(source);
    }

    /// 按给定切分模式填充规范化切分与大小写保留切分。
    pub fn apply_segmentation(request: &mut QueryRequest, mode: CutMode) {
        let normalized_source = request.raw_input.clone();
        let normalized_input = remove_manual_delimiters(&normalized_source);
        let normalized_input_is_empty = normalized_input.is_empty();
        replace_string(&mut request.normalized_input, normalized_input);
        let normalized_segmentation = if normalized_input_is_empty {
            String::new()
        } else {
            cut_pinyin_by_mode(&normalized_source, mode)
                .first()
                .map(|segments| join_segments(segments))
                .unwrap_or_default()
        };
        if normalized_segmentation.is_empty() {
            replace_string(&mut request.normalized_segmentation, normalized_source);
        } else {
            replace_string(
                &mut request.normalized_segmentation,
                normalized_segmentation,
            );
        }

        let cased_source = request.raw_input_with_cases.as_str();
        apply_segmentation_cases_into(
            &request.normalized_segmentation,
            cased_source,
            &mut request.raw_segmentation,
        );
        if cased_source.ends_with('\'') && !request.raw_segmentation.ends_with('\'') {
            request.raw_segmentation.push('\'');
        }
        request
            .segmentation
            .clone_from(&request.normalized_segmentation);
        request.valid = !normalized_input_is_empty;
    }

    /// 按字面贪心切分重切（关闭纠错时使用）。
    pub fn apply_literal_segmentation(request: &mut QueryRequest) {
        Self::apply_segmentation(request, CutMode::Greedy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> QuanpinScheme {
        let mut scheme = QuanpinScheme::new();
        for byte in text.bytes() {
            scheme.handle_key(if byte == b'\'' {
                SchemeKey::Apostrophe
            } else {
                SchemeKey::Letter(byte)
            });
        }
        scheme
    }

    #[test]
    fn refreshing_a_request_reuses_the_case_output_buffer() {
        let scheme = typed("NiHao");
        let mut request = QueryRequest::default();
        scheme.build_request_into(&mut request);
        let expected = request.clone();
        let pointer = request.raw_segmentation.as_ptr();
        scheme.build_request_into(&mut request);
        assert_eq!(request, expected);
        assert_eq!(request.raw_segmentation, "Ni'Hao");
        assert_eq!(request.raw_segmentation.as_ptr(), pointer);
    }

    #[test]
    fn keys_edit_the_raw_letters() {
        let mut scheme = typed("Ni''");
        assert_eq!(scheme.preedit(), "Ni'");
        scheme.handle_key(SchemeKey::Semicolon);
        scheme.handle_key(SchemeKey::Requery);
        scheme.handle_key(SchemeKey::Letter(b'1'));
        scheme.handle_key(SchemeKey::Symbol(b'-'));
        scheme.handle_key(SchemeKey::Symbol(b' '));
        assert_eq!(scheme.preedit(), "Ni'");
        scheme.handle_key(SchemeKey::Backspace);
        assert_eq!(scheme.preedit(), "Ni");
        scheme.reset();
        assert_eq!(scheme.preedit(), "");
    }

    #[test]
    fn host_raw_input_prefers_the_cased_form() {
        let mut scheme = QuanpinScheme::new();
        scheme.set_raw_input("nihao", "NiHao");
        assert_eq!(scheme.preedit(), "NiHao");
        scheme.set_raw_input("nihao", "");
        assert_eq!(scheme.preedit(), "nihao");
    }

    #[test]
    fn request_is_cut_in_correction_mode() {
        let request = typed("nihao").build_request();
        assert_eq!(request.scheme, SchemeType::Quanpin);
        assert_eq!(request.raw_input, "nihao");
        assert_eq!(request.normalized_input, "nihao");
        assert_eq!(request.normalized_segmentation, "ni'hao");
        assert_eq!(request.segmentation, "ni'hao");
        assert_eq!(request.raw_segmentation, "ni'hao");
        assert!(request.valid);

        let aliased = typed("sahng").build_request();
        assert_eq!(aliased.segmentation, "shang");
        let mut literal = aliased.clone();
        QuanpinScheme::apply_literal_segmentation(&mut literal);
        assert_ne!(literal.segmentation, "shang");
    }

    #[test]
    fn trailing_apostrophe_and_case_survive_in_the_raw_segmentation() {
        let request = typed("NiHao'").build_request();
        assert_eq!(request.raw_input, "nihao'");
        assert_eq!(request.segmentation, "ni'hao");
        assert_eq!(request.raw_segmentation, "Ni'Hao'");
    }

    #[test]
    fn uppercase_letters_stay_in_pinyin_segmentation() {
        let request = typed("maO").build_request();
        assert_eq!(request.raw_input, "mao");
        assert_eq!(request.normalized_input, "mao");
        assert_eq!(request.raw_segmentation, "maO");
    }

    #[test]
    fn empty_input_is_invalid() {
        let request = typed("'").build_request();
        assert!(!request.valid);
        assert!(!QuanpinScheme::new().build_request().valid);
    }
}
