//! 双拼按键处理与查询请求切分构造。

use super::query::{
    apply_segmentation_cases_into, effective_input_length, segment_input,
    to_quanpin_segmentation_into,
};
use super::ShuangpinProfile;
use crate::types::{QueryRequest, SchemeKey, SchemeType};

pub struct ShuangpinScheme {
    profile: &'static ShuangpinProfile,
    raw: String,
}

impl ShuangpinScheme {
    pub fn new(profile: &'static ShuangpinProfile) -> Self {
        Self {
            profile,
            raw: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.raw.clear();
    }

    pub fn handle_key(&mut self, key: SchemeKey) -> bool {
        match key {
            SchemeKey::Backspace => self.raw.pop().is_some(),
            SchemeKey::Apostrophe => {
                if !self.raw.ends_with('\'') {
                    self.raw.push('\'');
                    true
                } else {
                    false
                }
            }
            SchemeKey::Semicolon => {
                if self.accepts_ing_key() {
                    self.raw.push(';');
                    true
                } else {
                    false
                }
            }
            SchemeKey::Letter(letter) if letter.is_ascii_alphabetic() => {
                self.raw.push(char::from(letter));
                true
            }
            SchemeKey::Letter(_) | SchemeKey::Symbol(_) | SchemeKey::Requery => {
                false
            }
        }
    }

    /// `;` 只能是一个音节的第二个键（接在自上一 `'` 起奇数长度的片段之后）。
    fn accepts_ing_key(&self) -> bool {
        self.profile.uses_semicolon_key() && self.raw.rsplit('\'').next().unwrap_or("").len() % 2 == 1
    }

    #[cfg(test)]
    pub fn build_request(&self) -> QueryRequest {
        let mut request = QueryRequest::default();
        self.build_request_into(&mut request);
        request
    }

    /// 将双拼请求写入已有存储，避免逐键刷新重复构建按键和切分字符串。
    pub fn build_request_into(&self, request: &mut QueryRequest) {
        Self::build_request_from_raw_into(self.profile, &self.raw, &self.raw, request);
    }

    /// 从原文直接写入查询请求。
    pub fn build_request_from_raw_into(
        profile: &'static ShuangpinProfile,
        raw: &str,
        raw_with_cases: &str,
        request: &mut QueryRequest,
    ) {
        request.scheme = SchemeType::Shuangpin;
        let raw_with_cases = if raw_with_cases.is_empty() {
            raw
        } else {
            raw_with_cases
        };
        request.raw_input.clear();
        request.raw_input.extend(
            raw_with_cases
                .bytes()
                .map(|byte| char::from(byte.to_ascii_lowercase())),
        );
        request.raw_input_with_cases.clear();
        request.raw_input_with_cases.push_str(raw_with_cases);
        request.raw_input_with_helpcode_marks.clear();
        request.syllable_helpcodes.clear();
        request.end_helpcodes.clear();
        request.helpcode_source_index.clear();
        request.helpcode_decorations.clear();

        request.valid = effective_input_length(&request.raw_input) > 0;
        if request.valid {
            let segmentation = segment_input(&request.raw_input, profile);
            apply_segmentation_cases_into(
                &segmentation,
                &request.raw_input_with_cases,
                &mut request.raw_segmentation,
            );
            to_quanpin_segmentation_into(
                &segmentation,
                profile,
                &mut request.normalized_segmentation,
            );
            request
                .segmentation
                .clone_from(&request.normalized_segmentation);
            request.normalized_input.clear();
            request
                .normalized_input
                .reserve(effective_input_length(&request.normalized_segmentation));
            for part in request.normalized_segmentation.split('\'') {
                request.normalized_input.push_str(part);
            }
        } else {
            request.raw_segmentation.clear();
            request.normalized_segmentation.clear();
            request.segmentation.clear();
            request.normalized_input.clear();
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shuangpin::profile::profile;
    use crate::types::ShuangpinProfileKind;

    fn scheme(kind: ShuangpinProfileKind) -> ShuangpinScheme {
        ShuangpinScheme::new(profile(kind).unwrap())
    }

    fn type_text(scheme: &mut ShuangpinScheme, text: &str) {
        for byte in text.bytes() {
            scheme.handle_key(match byte {
                b'\'' => SchemeKey::Apostrophe,
                b';' => SchemeKey::Semicolon,
                letter => SchemeKey::Letter(letter),
            });
        }
    }

    #[test]
    fn refreshing_a_request_reuses_the_case_output_buffer() {
        let mut typed = scheme(ShuangpinProfileKind::Xiaohe);
        type_text(&mut typed, "NiHc");
        let mut request = typed.build_request();
        let expected = request.clone();
        let pointer = request.raw_segmentation.as_ptr();
        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            typed.build_request_into(&mut request);
        });
        assert_eq!(request, expected);
        assert_eq!(request.raw_segmentation, "Ni'Hc");
        assert_eq!(request.raw_segmentation.as_ptr(), pointer);
        assert_eq!(allocations, 2);
    }

    #[test]
    fn only_microsoft_takes_semicolon_after_an_odd_chunk() {
        for kind in [
            ShuangpinProfileKind::Xiaohe,
            ShuangpinProfileKind::Ziranma,
            ShuangpinProfileKind::Shoudao,
            ShuangpinProfileKind::Microsoft,
            ShuangpinProfileKind::SmartAbc,
        ] {
            let mut typed = scheme(kind);
            type_text(&mut typed, "b;");
            let expected = if kind == ShuangpinProfileKind::Microsoft {
                "b;"
            } else {
                "b"
            };
            assert_eq!(typed.preedit(), expected, "{kind:?}");
        }
        let mut microsoft = scheme(ShuangpinProfileKind::Microsoft);
        type_text(&mut microsoft, ";ni;");
        assert_eq!(microsoft.preedit(), "ni");
        type_text(&mut microsoft, "'b;");
        assert_eq!(microsoft.preedit(), "ni'b;");
    }

    #[test]
    fn requests_carry_both_segmentations() {
        let mut typed = scheme(ShuangpinProfileKind::Xiaohe);
        type_text(&mut typed, "nihcC");
        let request = typed.build_request();
        assert!(request.valid);
        assert_eq!(request.scheme, SchemeType::Shuangpin);
        assert_eq!(request.raw_input, "nihcc");
        assert_eq!(request.raw_input_with_cases, "nihcC");
        assert_eq!(request.raw_segmentation, "ni'hc'C");
        assert_eq!(request.normalized_segmentation, "ni'hao'c");
        assert_eq!(request.segmentation, "ni'hao'c");
        assert_eq!(request.normalized_input, "nihaoc");
    }
}
