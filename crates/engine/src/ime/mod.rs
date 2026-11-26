//! `ImeSession`：持有当前激活的拼音方案与候选提供者注册表，负责逐键构建请求与候选查询。

pub mod online_batch;
pub mod personal_rerank;
pub mod queries;
pub mod registry;
pub mod scheme;

use std::sync::Arc;

use crate::assets;
use crate::error::Result;
use crate::paths::RuntimePaths;
use crate::pinyin::autocorrect::autocorrect_suppression_key;
use crate::quanpin::{QuanpinEngine, QuanpinScheme};
use crate::shuangpin::{ShuangpinProfile, ShuangpinScheme};
use crate::types::{
    autocorrect_type, CandidateSource, FirstPageRows, FuzzyPinyinOptions, QueryRequest, SchemeKey,
    SchemeSet, SchemeType, SentenceAssociationOptions, WordItem,
};
use crate::user_dictionary::typo_profile::PersonalTypoProfile;

use registry::ProviderRegistry;
use scheme::Scheme;

/// 当前组字查询的提供者输出状态。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompositionState {
    pub preedit: String,
    pub request: QueryRequest,
    pub candidates: Vec<WordItem>,
}

pub struct ImeSession {
    scheme: Scheme,
    registry: ProviderRegistry,
    state: CompositionState,
    /// 光标前缀查询复用的临时请求存储，避免每次分页重新分配请求字段。
    scratch_request: QueryRequest,
    profile: &'static ShuangpinProfile,
    autocorrect_types: u32,
    fuzzy: FuzzyPinyinOptions,
    sentence_alternatives: bool,
    /// 首页行数随每个请求交给各方案引擎；只影响词库要不要并入补全词。
    first_page_rows: FirstPageRows,
    sentence_association: SentenceAssociationOptions,
    rescoring_context: String,
    typo_profile: Arc<PersonalTypoProfile>,
}

impl ImeSession {
    pub fn new(
        scheme: SchemeType,
        enabled: SchemeSet,
        profile: &'static ShuangpinProfile,
        paths: &RuntimePaths,
    ) -> Result<Self> {
        let mut registry = ProviderRegistry::new(enabled, profile, paths);
        registry.activate(scheme)?;
        Ok(Self {
            scheme: Scheme::new(scheme, profile)?,
            registry,
            state: CompositionState::default(),
            scratch_request: QueryRequest::default(),
            profile,
            autocorrect_types: 0,
            fuzzy: FuzzyPinyinOptions::default(),
            sentence_alternatives: false,
            first_page_rows: FirstPageRows::default(),
            sentence_association: SentenceAssociationOptions::default(),
            rescoring_context: String::new(),
            typo_profile: PersonalTypoProfile::shared(&paths.user(assets::USER_JOURNAL)),
        })
    }

    pub fn candidates(&self) -> &[WordItem] {
        &self.state.candidates
    }

    pub fn request(&self) -> &QueryRequest {
        &self.state.request
    }

    pub fn preedit(&self) -> &str {
        &self.state.preedit
    }

    pub fn current_scheme_type(&self) -> SchemeType {
        self.scheme.scheme_type()
    }

    pub fn handle_key(&mut self, key: SchemeKey) -> bool {
        let changed = if key != SchemeKey::Requery {
            self.scheme.handle_key(key)
        } else {
            false
        };
        self.refresh_candidates();
        changed
    }

    pub fn activate(&mut self, scheme: SchemeType) -> Result<()> {
        self.registry.activate(scheme)
    }

    pub fn switch_scheme(&mut self, scheme: SchemeType) -> Result<()> {
        self.activate(scheme)?;
        self.scheme = Scheme::new(scheme, self.profile)?;
        self.state = CompositionState::default();
        Ok(())
    }

    pub fn reset(&mut self) {
        self.scheme.reset();
        self.state = CompositionState::default();
    }

    pub fn replace_active_raw_input(&mut self, raw: &str, raw_with_cases: &str) {
        self.scheme.set_raw_input(raw, raw_with_cases);
        self.refresh_candidates();
    }

    /// 光标前缀解码查询，不修改实时组字状态。
    pub fn query_raw_candidates(&mut self, raw: &str, raw_with_cases: &str) -> Vec<WordItem> {
        self.with_raw_request(
            |session, request| session.build_raw_request(raw, raw_with_cases, request),
            |session, request| {
                if request.valid {
                    session.registry.query(request)
                } else {
                    Vec::new()
                }
            },
        )
    }

    pub fn query_raw_candidates_with_request(
        &mut self,
        raw: &str,
        raw_with_cases: &str,
    ) -> (Vec<WordItem>, QueryRequest) {
        self.with_raw_request(
            |session, request| session.build_raw_request(raw, raw_with_cases, request),
            |session, request| {
                let candidates = if request.valid {
                    session.registry.query(request)
                } else {
                    Vec::new()
                };
                (candidates, request.clone())
            },
        )
    }

    pub fn expand_current_raw_prefix_initial_candidates(
        &mut self,
        raw: &str,
        prefix_end: usize,
        candidates: &mut Vec<WordItem>,
    ) -> bool {
        self.expand_current_raw_range_initial_candidates(raw, 0..prefix_end, candidates)
    }

    pub fn expand_current_raw_range_initial_candidates(
        &mut self,
        raw: &str,
        range: std::ops::Range<usize>,
        candidates: &mut Vec<WordItem>,
    ) -> bool {
        let raw_with_cases = if self.state.request.raw_input_with_cases.is_empty() {
            &self.state.request.raw_input
        } else {
            &self.state.request.raw_input_with_cases
        };
        if raw_with_cases.get(range.clone()).is_none() {
            return false;
        }
        self.with_raw_request(
            |session, request| {
                let raw_with_cases = if session.state.request.raw_input_with_cases.is_empty() {
                    &session.state.request.raw_input
                } else {
                    &session.state.request.raw_input_with_cases
                };
                let raw_with_cases = &raw_with_cases[range];
                session.build_raw_request(raw, raw_with_cases, request);
            },
            |session, request| {
                request.valid
                    && session
                        .registry
                        .expand_initial_candidates(request, candidates)
            },
        )
    }

    fn build_raw_request(&self, raw: &str, raw_with_cases: &str, request: &mut QueryRequest) {
        match self.current_scheme_type() {
            SchemeType::Quanpin => {
                QuanpinScheme::build_request_from_raw_into(raw, raw_with_cases, request)
            }
            SchemeType::Shuangpin => ShuangpinScheme::build_request_from_raw_into(
                self.profile,
                raw,
                raw_with_cases,
                request,
            ),
            _ => {
                *request = QueryRequest::default();
                return;
            }
        }
        self.apply_request_options(request);
        self.apply_autocorrect_suppression(request);
    }

    fn with_raw_request<R>(
        &mut self,
        build: impl FnOnce(&Self, &mut QueryRequest),
        use_request: impl FnOnce(&mut Self, &QueryRequest) -> R,
    ) -> R {
        let mut request = std::mem::take(&mut self.scratch_request);
        build(self, &mut request);
        let result = use_request(self, &request);
        self.scratch_request = request;
        result
    }

    pub fn set_autocorrect_types(&mut self, types: u32) {
        self.autocorrect_types = types;
    }

    pub fn set_fuzzy_pinyin_options(&mut self, options: FuzzyPinyinOptions) {
        self.fuzzy = options;
    }

    pub fn set_sentence_alternatives(&mut self, enabled: bool) {
        self.sentence_alternatives = enabled;
    }

    pub fn set_sentence_association(&mut self, options: SentenceAssociationOptions) {
        self.sentence_association = options;
    }

    pub fn set_first_page_rows(&mut self, rows: FirstPageRows) {
        self.first_page_rows = rows;
    }

    pub fn set_rescoring_context(&mut self, context: &str) {
        context.clone_into(&mut self.rescoring_context);
    }

    /// 最近 64 字窗口内是否含有汉字，不含时不触发中文首组字典搭配消歧。
    pub fn has_han_rescoring_context(&self) -> bool {
        crate::text::last_characters(
            &self.rescoring_context,
            crate::lattice::octagram::CONTEXT_CHARACTERS,
        )
        .chars()
        .any(crate::text::is_han)
    }

    pub fn has_dictionary_reranker(&self, scheme: SchemeType) -> bool {
        self.registry.has_dictionary_reranker(scheme)
    }

    pub(crate) fn pick_dictionary_homophone(
        &mut self,
        scheme: SchemeType,
        items: &[&WordItem],
        effective_weights: Option<&[f64]>,
    ) -> Option<usize> {
        self.registry.pick_dictionary_homophone(
            scheme,
            items,
            &self.rescoring_context,
            effective_weights,
        )
    }

    pub(crate) fn quanpin_mut(&mut self) -> &mut QuanpinEngine {
        self.registry.quanpin_mut()
    }

    pub(crate) fn create_word_from_canonical_pinyin(
        &mut self,
        canonical_pinyin: &str,
        word: &str,
    ) -> Result<()> {
        self.registry
            .quanpin_mut()
            .create_word_from_canonical_pinyin(canonical_pinyin, word)?;
        self.registry.reset_all_caches();
        Ok(())
    }

    pub fn reset_cache(&mut self) {
        self.registry.reset_all_caches();
        self.refresh_candidates();
    }

    pub fn expand_initial_candidates(&mut self) -> bool {
        self.registry
            .expand_initial_candidates(&self.state.request, &mut self.state.candidates)
    }

    pub fn apply_dynamic_candidates(&mut self, words: &[String], source: CandidateSource) -> bool {
        if !self
            .registry
            .cache_dynamic_candidates_for_request(&self.state.request, words, source)
        {
            return false;
        }
        self.refresh_candidates();
        true
    }

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        if !source.is_online() {
            return;
        }
        self.registry.clear_online_candidates(source);
        self.refresh_candidates();
    }

    fn refresh_candidates(&mut self) {
        let mut request = std::mem::take(&mut self.state.request);
        self.scheme.build_request_into(&mut request);
        self.apply_request_options(&mut request);
        self.apply_autocorrect_suppression(&mut request);
        reuse_request_preedit(&request, &mut self.state.preedit);
        if !request.valid {
            self.state.request = request;
            self.state.candidates.clear();
            return;
        }
        let candidates = self.registry.query(&request);
        self.state.request = request;
        self.state.candidates = candidates;
    }

    fn apply_request_options(&self, request: &mut QueryRequest) {
        request.enable_shuangpin_helpcode = false;
        request.end_helpcodes.clear();
        request.enable_quanpin_helpcode = false;
        request.strict_quanpin_helpcode = false;
        request.sentence_alternatives = self.sentence_alternatives;
        request.enable_quanpin_autocorrect_transposition =
            self.autocorrect_types & autocorrect_type::TRANSPOSITION != 0;
        request.enable_quanpin_autocorrect_neighbor =
            self.autocorrect_types & autocorrect_type::NEIGHBOR != 0;
        request.fuzzy_pinyin = self.fuzzy;
        request.sentence_association = self.sentence_association.clone();
        request.first_page_rows = self.first_page_rows;
        request
            .rescoring_context
            .clone_from(&self.rescoring_context);
    }

    fn apply_autocorrect_suppression(&self, request: &mut QueryRequest) {
        if request.scheme != SchemeType::Quanpin || self.autocorrect_types == 0 {
            return;
        }
        self.typo_profile.refresh_if_changed();
        let key = autocorrect_suppression_key(&request.raw_input, &request.raw_input);
        if !self.typo_profile.suppressed(&key) {
            return;
        }
        request.enable_quanpin_autocorrect_transposition = false;
        request.enable_quanpin_autocorrect_neighbor = false;
        QuanpinScheme::apply_literal_segmentation(request);
    }
}

fn reuse_request_preedit(request: &QueryRequest, destination: &mut String) {
    request.raw_input_with_cases.clone_into(destination);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_preedit_reuses_existing_storage() {
        let request = QueryRequest {
            scheme: SchemeType::Quanpin,
            raw_input_with_cases: "NiHao".to_owned(),
            normalized_segmentation: "ni'hao".to_owned(),
            ..QueryRequest::default()
        };
        let mut destination = String::with_capacity(request.raw_input_with_cases.len());
        destination.push_str("old");
        let pointer = destination.as_ptr();

        let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
            reuse_request_preedit(&request, &mut destination);
        });

        assert_eq!(allocations, 0);
        assert_eq!(destination, "NiHao");
        assert_eq!(destination.as_ptr(), pointer);
    }
}
