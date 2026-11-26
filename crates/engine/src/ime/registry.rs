//! 方案候选提供者注册表：仅承载全拼与双拼两套中文拼音引擎。

use crate::diagnostics;
use crate::error::{EngineError, Result};
use crate::paths::RuntimePaths;
use crate::quanpin::QuanpinEngine;
use crate::shuangpin::{ShuangpinEngine, ShuangpinProfile};
use crate::types::{CandidateSource, QueryRequest, SchemeSet, SchemeType, WordItem};

pub struct ProviderRegistry {
    /// 会话允许运行的方案；不在其中的方案没有 provider，`activate` 拒绝它们。
    enabled: SchemeSet,
    paths: RuntimePaths,
    quanpin: Option<QuanpinEngine>,
    shuangpin: Option<ShuangpinEngine>,
}

impl ProviderRegistry {
    pub fn new(
        enabled: SchemeSet,
        profile: &'static ShuangpinProfile,
        paths: &RuntimePaths,
    ) -> Self {
        Self {
            enabled,
            paths: paths.clone(),
            quanpin: enabled
                .contains(SchemeType::Quanpin)
                .then(|| QuanpinEngine::new(paths)),
            shuangpin: enabled
                .contains(SchemeType::Shuangpin)
                .then(|| ShuangpinEngine::new(profile, paths)),
        }
    }

    pub fn activate(&mut self, scheme: SchemeType) -> Result<()> {
        if !self.enabled.contains(scheme) || !scheme.is_pinyin() {
            return Err(EngineError::invalid(diagnostics::INPUT_SCHEME_NOT_ENABLED));
        }
        Ok(())
    }

    /// 供全拼/双拼统一复用的单一全拼引擎实例，消除双连接缓存脑裂。
    pub(crate) fn quanpin_mut(&mut self) -> &mut QuanpinEngine {
        let paths = &self.paths;
        self.quanpin
            .get_or_insert_with(|| QuanpinEngine::new(paths))
    }

    fn reset_pinyin_caches(&mut self) {
        if let Some(quanpin) = &mut self.quanpin {
            quanpin.reset_cache();
        }
        if let Some(shuangpin) = &mut self.shuangpin {
            shuangpin.reset_cache();
        }
    }

    pub fn query(&mut self, request: &QueryRequest) -> Vec<WordItem> {
        if !request.valid {
            return Vec::new();
        }
        let mut candidates = match request.scheme {
            SchemeType::Quanpin => match &mut self.quanpin {
                Some(quanpin) => quanpin.query(request),
                None => return Vec::new(),
            },
            SchemeType::Shuangpin => match &mut self.shuangpin {
                Some(shuangpin) => shuangpin.query(request),
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        for item in &mut candidates {
            item.scheme = request.scheme;
        }
        candidates
    }

    /// 方案词典是否已挂载搭配模型用于同音词语境消歧。
    pub fn has_dictionary_reranker(&self, scheme: SchemeType) -> bool {
        match scheme {
            SchemeType::Quanpin => self
                .quanpin
                .as_ref()
                .is_some_and(|quanpin| quanpin.has_dictionary_reranker()),
            SchemeType::Shuangpin => self
                .shuangpin
                .as_ref()
                .is_some_and(|shuangpin| shuangpin.has_dictionary_reranker()),
            _ => false,
        }
    }

    /// 用搭配模型按 `context` 在同音字典候选中选出应旋至首位的下标。
    pub fn pick_dictionary_homophone(
        &mut self,
        scheme: SchemeType,
        items: &[&WordItem],
        context: &str,
        effective_weights: Option<&[f64]>,
    ) -> Option<usize> {
        match scheme {
            SchemeType::Quanpin => {
                self.quanpin
                    .as_mut()?
                    .pick_dictionary_homophone(items, context, effective_weights)
            }
            SchemeType::Shuangpin => self.shuangpin.as_mut()?.pick_dictionary_homophone(
                items,
                context,
                effective_weights,
            ),
            _ => None,
        }
    }

    /// 只读查询给定拼音方案在 `.gram` 模型上的搭配净增益。
    pub fn query_collocation_bonus(
        &self,
        scheme: SchemeType,
        context_tail: &str,
        word: &str,
        is_rear: bool,
    ) -> Option<f64> {
        match scheme {
            SchemeType::Quanpin => {
                self.quanpin
                    .as_ref()?
                    .query_collocation_bonus(context_tail, word, is_rear)
            }
            SchemeType::Shuangpin => {
                self.shuangpin
                    .as_ref()?
                    .query_collocation_bonus(context_tail, word, is_rear)
            }
            _ => None,
        }
    }

    pub fn reset_all_caches(&mut self) {
        self.reset_pinyin_caches();
    }

    pub fn find_candidate(&self, scheme: SchemeType, key: &str, value: &str) -> Option<WordItem> {
        match scheme {
            SchemeType::Quanpin => self.quanpin.as_ref()?.find_candidate(key, value),
            SchemeType::Shuangpin => self.shuangpin.as_ref()?.find_candidate(key, value),
            _ => None,
        }
    }

    pub fn cache_dynamic_candidates_for_request(
        &mut self,
        request: &QueryRequest,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        match request.scheme {
            SchemeType::Quanpin => self
                .quanpin
                .as_mut()
                .is_some_and(|quanpin| quanpin.insert_online_words(request, words, source)),
            SchemeType::Shuangpin => self
                .shuangpin
                .as_mut()
                .is_some_and(|shuangpin| shuangpin.insert_online_words(request, words, source)),
            _ => false,
        }
    }

    pub fn clear_online_candidates(&mut self, source: CandidateSource) {
        if let Some(quanpin) = &mut self.quanpin {
            quanpin.clear_online_candidates(source);
        }
        if let Some(shuangpin) = &mut self.shuangpin {
            shuangpin.clear_online_candidates(source);
        }
    }

    pub fn expand_initial_candidates(
        &mut self,
        request: &QueryRequest,
        candidates: &mut Vec<WordItem>,
    ) -> bool {
        match request.scheme {
            SchemeType::Quanpin => self
                .quanpin
                .as_mut()
                .is_some_and(|quanpin| quanpin.expand_initial_candidates(request, candidates)),
            SchemeType::Shuangpin => self
                .shuangpin
                .as_mut()
                .is_some_and(|shuangpin| shuangpin.expand_initial_candidates(request, candidates)),
            _ => false,
        }
    }
}
