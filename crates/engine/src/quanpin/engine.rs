//! `QuanpinEngine`：全拼词典查询与单字母候选展开门控。

use crate::dictionary::DictRow;
use crate::error::Result;
use crate::lattice::decode::PinnedSpan;
use crate::paths::RuntimePaths;
use crate::pinyin::segment::split_segments;
use crate::types::{request_autocorrect_mask, CandidateSource, QueryRequest, SchemeType, WordItem};

use super::QuanpinDictionary;

pub struct QuanpinEngine {
    dictionary: QuanpinDictionary,
}

impl QuanpinEngine {
    pub fn new(paths: &RuntimePaths) -> Self {
        Self {
            dictionary: QuanpinDictionary::new(paths),
        }
    }

    pub fn query(&mut self, request: &QueryRequest) -> Vec<WordItem> {
        if !request.valid {
            return Vec::new();
        }
        self.dictionary
            .set_sentence_alternatives(request.sentence_alternatives);
        self.dictionary
            .set_sentence_association(request.sentence_association.clone());
        self.dictionary.set_first_page_rows(request.first_page_rows);
        self.dictionary
            .set_rescoring_context(&request.rescoring_context);

        let types = request_autocorrect_mask(
            request.enable_quanpin_autocorrect_transposition,
            request.enable_quanpin_autocorrect_neighbor,
        );
        self.dictionary.query(
            &request.raw_input,
            &request.segmentation,
            types,
            request.fuzzy_pinyin,
        )
    }

    /// 仅当全拼首段为单个字母时展开全部单字候选。
    pub fn expand_initial_candidates(
        &mut self,
        request: &QueryRequest,
        candidates: &mut Vec<WordItem>,
    ) -> bool {
        if request.scheme != SchemeType::Quanpin {
            return false;
        }
        let segments = split_segments(&request.segmentation);
        match segments.first() {
            Some(first) if first.len() == 1 => self.dictionary.expand_initial_candidates(
                &request.raw_input,
                &request.segmentation,
                request_autocorrect_mask(
                    request.enable_quanpin_autocorrect_transposition,
                    request.enable_quanpin_autocorrect_neighbor,
                ),
                first,
                candidates,
            ),
            _ => false,
        }
    }

    pub fn insert_online_words(
        &mut self,
        request: &QueryRequest,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        self.dictionary.insert_online_words(
            &request.raw_input,
            &request.segmentation,
            request_autocorrect_mask(
                request.enable_quanpin_autocorrect_transposition,
                request.enable_quanpin_autocorrect_neighbor,
            ),
            words,
            source,
        )
    }

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        self.dictionary.clear_online_candidates(source);
    }

    pub fn find_candidate(&self, key: &str, value: &str) -> Option<WordItem> {
        self.dictionary.find_candidate(key, value)
    }

    pub fn has_dictionary_reranker(&self) -> bool {
        self.dictionary.has_dictionary_reranker()
    }

    pub fn pick_dictionary_homophone(
        &mut self,
        items: &[&WordItem],
        context: &str,
        effective_weights: Option<&[f64]>,
    ) -> Option<usize> {
        self.dictionary
            .pick_dictionary_homophone(items, context, effective_weights)
    }

    pub fn knows_han_char(&self, han: &str) -> bool {
        self.dictionary.knows_han_char(han)
    }

    pub fn create_word_from_canonical_pinyin(&mut self, pinyin: &str, word: &str) -> Result<()> {
        self.dictionary
            .create_word_from_canonical_pinyin(pinyin, word)
    }

    pub fn conversion_rows(&mut self, span: &[String], limit: usize) -> Vec<DictRow> {
        self.dictionary.conversion_rows(span, limit)
    }

    pub fn convert_pinned(&mut self, syllables: &[String], pins: &[PinnedSpan]) -> Vec<PinnedSpan> {
        self.dictionary.convert_pinned(syllables, pins)
    }

    pub fn reset_cache(&mut self) {
        self.dictionary.reset_cache()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quanpin::fixture::Fixture;
    use crate::quanpin::QuanpinScheme;
    use crate::types::SchemeKey;

    fn request(text: &str) -> QueryRequest {
        let mut scheme = QuanpinScheme::new();
        for byte in text.bytes() {
            scheme.handle_key(if byte == b'\'' {
                SchemeKey::Apostrophe
            } else {
                SchemeKey::Letter(byte)
            });
        }
        let mut request = QueryRequest::default();
        scheme.build_request_into(&mut request);
        request.sentence_alternatives = true;
        request
    }

    fn words(items: &[WordItem]) -> Vec<&str> {
        items.iter().map(|item| item.word.as_str()).collect()
    }

    #[test]
    fn quanpin_retroflex_abbreviations_keep_both_initial_readings() {
        let fixture = Fixture::new();
        fixture
            .insert("zhong'hua'ren'min'gong'he'guo", "中华人民共和国", 10_000)
            .insert("shang'hai'da'xue", "上海大学", 9_000)
            .insert("shang'hao", "上好", 8_500)
            .insert("zhi'shi", "知识", 8_000)
            .insert("zhong'hua'shi", "中华市", 7_500)
            .insert("zhi'shi'hai", "知识海", 7_500)
            .insert("cheng'shi", "城市", 7_000)
            .insert("zhong'guo", "中国", 6_000)
            .insert("ni'hao", "你好", 5_000)
            .insert("wei'shen'me", "为什么", 4_000)
            .insert("zhi", "知", 3_000)
            .insert("chi", "吃", 3_000)
            .insert("shi", "是", 3_000);
        let mut engine = QuanpinEngine::new(&fixture.paths);
        for (input, expected) in [
            ("zhrmghg", "中华人民共和国"),
            ("z'hrmghg", "中华人民共和国"),
            ("shdx", "上海大学"),
            ("s'hdx", "上海大学"),
            ("zhsh", "知识"),
            ("chsh", "城市"),
            ("zhg", "中国"),
            ("zh'g", "中国"),
            ("nh", "你好"),
            ("wsm", "为什么"),
            ("zhongguo", "中国"),
            ("zhi", "知"),
            ("chi", "吃"),
            ("shi", "是"),
        ] {
            let mut req = request(input);
            req.enable_quanpin_autocorrect_transposition = false;
            req.enable_quanpin_autocorrect_neighbor = false;
            let candidates = engine.query(&req);
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.word == expected);
            assert!(
                candidate.is_some(),
                "{input} 缺少 {expected}: {candidates:?}"
            );
            let candidate = candidate.unwrap();
            assert_eq!(candidate.pinyin.replace('\'', ""), input.replace('\'', ""));
            assert!(candidate.corrected_from.is_empty());
            if matches!(input, "zhongguo" | "zhi" | "chi" | "shi") {
                assert_eq!(candidates[0].word, expected, "完整全拼必须优先");
            }
        }
        let mixed = engine.query(&request("zhsh"));
        assert!(words(&mixed).contains(&"中华市"));
        assert!(words(&mixed).contains(&"知识海"));
        let manual = engine.query(&request("zh'sh"));
        assert!(words(&manual).contains(&"知识"));
        assert!(!words(&manual).contains(&"中华市"));
        assert!(!words(&manual).contains(&"知识海"));
        for input in ["shnaghao", "sahnghao"] {
            let mut req = request(input);
            req.enable_quanpin_autocorrect_transposition = false;
            req.enable_quanpin_autocorrect_neighbor = false;
            QuanpinScheme::apply_literal_segmentation(&mut req);
            let candidates = engine.query(&req);
            assert!(
                !words(&candidates).contains(&"上好"),
                "{input}: {candidates:?}"
            );
        }
    }

    #[test]
    fn invalid_request_answers_nothing() {
        let fixture = Fixture::new();
        let mut engine = QuanpinEngine::new(&fixture.paths);
        assert!(engine.query(&QueryRequest::default()).is_empty());
    }

    #[test]
    fn expansion_gate_needs_a_quanpin_single_letter() {
        let fixture = Fixture::new();
        for i in 0..30 {
            fixture.insert("ni", &format!("n{i}"), 1_000 - i);
        }
        let mut engine = QuanpinEngine::new(&fixture.paths);
        let single = request("n");
        let mut rows = engine.query(&single);
        assert_eq!(rows.len(), 24);

        let mut shuangpin = single.clone();
        shuangpin.scheme = SchemeType::Shuangpin;
        assert!(!engine.expand_initial_candidates(&shuangpin, &mut rows));
        assert!(!engine.expand_initial_candidates(&request("ni"), &mut rows));
        assert!(engine.expand_initial_candidates(&single, &mut rows));
        assert_eq!(rows.len(), 30);
    }
}
