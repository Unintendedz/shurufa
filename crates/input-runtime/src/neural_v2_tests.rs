//! Shurufa v2 Tier 1 两阶段异步神经解码在 `Runtime` 层的集成单测：
//! 校验代次防过期、完整 512 字含标点上文透传、80 ms 槽 0 锁、Tier 1 独有束搜索注入与上屏回流。

use super::*;
use msime_engine::neural_v2::{
    NeuralBeamCandidate, NeuralDecodeResponse, NeuralScoredCandidate, SOURCE_NEURAL_CONSTRAINED,
};
use msime_engine::{CandidateEntry, CandidateSource, SchemeType};

struct StubNeuralEngine {
    snapshot: EngineSnapshot,
    canceled: usize,
}

impl StubNeuralEngine {
    fn composing(raw: &str, preedit: &str, candidates: &[(&str, bool, Option<i64>)]) -> Self {
        let mut snap = EngineSnapshot {
            scheme: SchemeType::Quanpin as u8,
            local_mode: "none".into(),
            preedit: preedit.into(),
            editing_text: raw.into(),
            caret_position: raw.len(),
            ..EngineSnapshot::default()
        };
        for (index, &(text, exact, stat_milli)) in candidates.iter().enumerate() {
            let mut entry =
                CandidateEntry::new(text, preedit, CandidateSource::Generated as u8);
            entry.answers_key = exact;
            entry.static_evidence = stat_milli.map(|m| {
                msime_engine::ordering::StaticEvidence {
                    log_prob_milli_nats: m,
                    characters: text.chars().count(),
                    keyboard_neural: None,
                }
            });
            entry.origin = msime_engine::CandidateOrigin::EngineRow(index);
            snap.candidates.push(entry);
        }
        Self {
            snapshot: snap,
            canceled: 0,
        }
    }
}

impl InputEngine for StubNeuralEngine {
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        Ok(self.snapshot.clone())
    }
    fn character(&mut self, _value: u8, _shift: bool) -> Result<EngineResult, RuntimeError> {
        Ok(empty_result(true))
    }
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError> {
        if command == Command::Cancel {
            self.canceled += 1;
            self.snapshot.editing_text.clear();
            self.snapshot.preedit.clear();
            self.snapshot.candidates.clear();
        }
        Ok(empty_result(true))
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        let word = self
            .snapshot
            .candidates
            .get(index)
            .map(|entry| entry.text.clone())
            .unwrap_or_default();
        self.command(Command::Cancel)?;
        Ok(EngineResult {
            handled: true,
            has_commit: !word.is_empty(),
            commit: word,
            diagnostic: String::new(),
        })
    }
    fn select_edge(
        &mut self,
        _index: usize,
        _edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        Ok(empty_result(false))
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.select(index)
    }
    fn punctuation(&mut self, _value: u8) -> Result<EngineResult, RuntimeError> {
        Ok(empty_result(false))
    }
}

#[test]
fn runtime_neural_v2_preserves_punctuation_context_and_rescues_beam_candidate() {
    let engine = StubNeuralEngine::composing(
        "zhenglexieqian",
        "zheng'le'xie'qian",
        &[
            ("整了血铅", true, Some(-11_400)),
            ("整了些钱", true, Some(-12_100)),
            ("整", false, Some(-4_000)),
        ],
    );
    let mut runtime = Runtime::new(engine, 6).unwrap();
    runtime.focused = true;
    runtime.set_neural_v2_enabled(true);

    let ctx = "昨天刚聊完，这个兄弟 API 2.0 应该多少还是";
    runtime.set_surrounding_context(ctx);
    assert_eq!(runtime.neural_prefill_context(), ctx);

    let req = runtime.take_neural_request().expect("应生成 Tier 1 请求");
    assert_eq!(req.context, ctx);
    // 仅发送等跨度竞争候选（跳过非等跨度单字前缀 "整"），压低前缀树节点数。
    assert_eq!(req.candidates.len(), 2);
    assert!(req.candidates.iter().all(|c| c.exact_span));
    assert_eq!(
        req.syllable_paths[0].syllables,
        vec!["zheng", "le", "xie", "qian"]
    );

    let resp = NeuralDecodeResponse {
        ok: true,
        session: req.session,
        generation: req.generation,
        candidate_scores: vec![
            NeuralScoredCandidate {
                text: "整了血铅".into(),
                neural_log_prob: -24.0,
            },
            NeuralScoredCandidate {
                text: "整了些钱".into(),
                neural_log_prob: -14.8,
            },
        ],
        beam_candidates: vec![NeuralBeamCandidate {
            text: "挣了些钱".into(),
            neural_log_prob: -11.0,
            path_index: 0,
            token_count: 3,
        }],
    };
    let applied = runtime.apply_neural_response(&resp, false);
    assert!(applied.applied);
    assert!(applied.moved);
    assert!(applied.tier1_rescued);
    assert!(!applied.slot0_locked);

    let view = runtime.view();
    assert_eq!(view.candidates[0].text, "挣了些钱");
    assert_eq!(view.candidates[0].source, SOURCE_NEURAL_CONSTRAINED);

    // 选中 Tier 1 独有注入候选应直接上屏并回流写入上文，不越界调用底层引擎。
    let commit = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(commit.commit.as_deref(), Some("挣了些钱"));
    assert!(runtime.neural_prefill_context().ends_with("挣了些钱"));
}

#[test]
fn runtime_neural_v2_rejects_stale_generation_and_honors_slot0_lock() {
    let engine = StubNeuralEngine::composing(
        "zhenglexieqian",
        "zheng'le'xie'qian",
        &[
            ("整了血铅", true, Some(-11_400)),
            ("政了血铅", true, Some(-12_000)),
            ("挣了些钱", true, Some(-12_500)),
        ],
    );
    let mut runtime = Runtime::new(engine, 6).unwrap();
    runtime.focused = true;
    runtime.set_neural_v2_enabled(true);
    let req = runtime.take_neural_request().unwrap();

    // 1. 过期代次回包必须直接丢弃。
    let stale = NeuralDecodeResponse {
        ok: true,
        session: req.session,
        generation: req.generation + 99,
        candidate_scores: vec![],
        beam_candidates: vec![],
    };
    let stale_out = runtime.apply_neural_response(&stale, false);
    assert!(!stale_out.applied);
    assert!(!stale_out.moved);

    // 2. 迟到 >80 ms（lock_slot_0 = true）时严格保留槽 0，槽 1 升级为神经最优。
    let late = NeuralDecodeResponse {
        ok: true,
        session: req.session,
        generation: req.generation,
        candidate_scores: vec![
            NeuralScoredCandidate {
                text: "整了血铅".into(),
                neural_log_prob: -24.0,
            },
            NeuralScoredCandidate {
                text: "政了血铅".into(),
                neural_log_prob: -25.0,
            },
            NeuralScoredCandidate {
                text: "挣了些钱".into(),
                neural_log_prob: -11.0,
            },
        ],
        beam_candidates: vec![],
    };
    let late_out = runtime.apply_neural_response(&late, true);
    assert!(late_out.applied);
    assert!(late_out.moved);
    assert!(late_out.slot0_locked);
    let view = runtime.view();
    assert_eq!(view.candidates[0].text, "整了血铅");
    assert_eq!(view.candidates[1].text, "挣了些钱");
}

#[test]
fn runtime_neural_v2_skips_zero_context_single_char_and_propagates_following_context() {
    let mut engine = StubNeuralEngine::composing(
        "xian",
        "xian",
        &[("先", true, Some(-4_100)), ("现", true, Some(-4_460))],
    );
    for entry in &mut engine.snapshot.candidates {
        entry.source = CandidateSource::Database as u8;
    }
    let mut runtime = Runtime::new(engine, 6).unwrap();
    runtime.focused = true;
    runtime.set_neural_v2_enabled(true);

    // 无前文且无后文时，单音节词典单字不派发冗余推理请求。
    assert!(runtime.take_neural_request().is_none());

    // 句中补字同步前文与后文后，请求完整携带 `context` 与换行截断后的 `right_context`。
    runtime.set_surrounding_context("你发送看看，");
    runtime.set_following_context("看会不会有被拒绝的\n第二行");
    let req = runtime
        .take_neural_request()
        .expect("有双向上下文时应派发请求");
    assert_eq!(req.context, "你发送看看，");
    assert_eq!(req.right_context, "看会不会有被拒绝的");

    // 下一次仅同步前文（光标移到句尾）时自动清空旧后文，杜绝跨位置残留。
    runtime.set_surrounding_context("你发送看看，先看会不会有被拒绝的");
    let req_end = runtime.take_neural_request().expect("有前文时应派发请求");
    assert!(req_end.right_context.is_empty());
}

#[test]
fn locked_prefix_feeds_effective_left_context_and_preserves_pinned_first_across_scorer() {
    let mut engine = StubNeuralEngine::composing(
        "paobu",
        "pao'bu",
        &[
            ("刨步", true, Some(-10_500)),
            ("跑步", true, Some(-11_200)),
            ("泡布", true, Some(-11_800)),
        ],
    );
    // 将第 3 项置顶到首位，并模拟会话内已经锁定前缀段 `去海滩`。
    engine.snapshot.candidates[2].pinned_first = true;
    let mut runtime = Runtime::new(engine, 6).unwrap();
    runtime.focused = true;
    runtime.set_neural_v2_enabled(true);
    runtime.set_surrounding_context("周末一起");
    runtime.locked_prefix.push(msime_engine::LockedSegment {
        reading: "quhaitan".into(),
        text: "去海滩".into(),
    });

    let cursor = runtime.cursor_snapshot();
    assert_eq!(cursor.effective_left_context(), "周末一起去海滩");

    let req = runtime
        .take_neural_request()
        .expect("locked_prefix 应纳入有效左上下文");
    assert_eq!(req.context, "周末一起去海滩");
    assert_eq!(req.pinned_first_index, Some(2));
    // 置顶项不发往前缀树，未置顶项正常参选。
    assert_eq!(req.candidates.len(), 2);

    let resp = NeuralDecodeResponse {
        ok: true,
        session: req.session,
        generation: req.generation,
        candidate_scores: vec![
            NeuralScoredCandidate {
                text: "刨步".into(),
                neural_log_prob: -22.0,
            },
            NeuralScoredCandidate {
                text: "跑步".into(),
                neural_log_prob: -8.0,
            },
        ],
        beam_candidates: vec![],
    };
    let out = runtime.apply_neural_response(&resp, false);
    assert!(out.applied && out.moved);
    let view = runtime.view();
    // 置顶到第 1 位的候选稳居首位 0，且带上 fixed_position = Some(1) 标识；未置顶项按神经得分重排。
    assert_eq!(view.candidates[0].text, "泡布");
    assert_eq!(view.candidates[0].fixed_position, Some(1));
    assert_eq!(view.candidates[1].text, "跑步");
    assert_eq!(view.candidates[1].fixed_position, None);
    assert_eq!(view.candidates[2].text, "刨步");
}
