//! `Runtime::facts()`、`has_composition()` 和两个 epoch 计数器的单元测试。

use super::super::runtime::empty_result;
use super::super::*;
use super::phrase_runtime;

#[derive(Default)]
struct FactEngine {
    scheme: u8,
    text: String,
    words: Vec<String>,
    sources: Vec<u8>,
    conversion: String,
    spelling_symbols: String,
    local_mode: String,
    remaining_after_select: Option<String>,
    select_commit_override: Option<String>,
    character_commits: Option<String>,
    withheld: Vec<String>,
}

impl FactEngine {
    fn commit(text: impl Into<String>) -> EngineResult {
        EngineResult {
            handled: true,
            has_commit: true,
            commit: text.into(),
            diagnostic: String::new(),
        }
    }
}

impl InputEngine for FactEngine {
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        let composing = !self.text.is_empty() || !self.conversion.is_empty();
        let count = self.words.len();
        Ok(EngineSnapshot {
            prediction: false,
            scheme: self.scheme,
            candidate_codes: vec![String::new(); count],
            candidate_annotations: vec![String::new(); count],
            candidate_sources: if self.sources.len() == count {
                self.sources.clone()
            } else {
                vec![0; count]
            },
            candidate_corrected: vec![false; count],
            candidate_answers_key: vec![true; count],
            shuangpin_profile: "xiaohe".into(),
            local_mode: if self.local_mode.is_empty() {
                "none".into()
            } else {
                self.local_mode.clone()
            },
            spelling_symbols: self.spelling_symbols.clone(),
            preedit: self.text.clone(),
            editing_text: self.text.clone(),
            caret_position: self.text.len(),
            conversion: self.conversion.clone(),
            conversion_focus_start: 0,
            conversion_focus_end: usize::from(!self.conversion.is_empty()),
            candidates: if composing {
                self.words.clone()
            } else {
                Vec::new()
            },
            ..EngineSnapshot::default()
        })
    }
    fn character(&mut self, value: u8, _shift: bool) -> Result<EngineResult, RuntimeError> {
        if self.spelling_symbols.as_bytes().contains(&value) || value.is_ascii_alphabetic() {
            self.text.push(char::from(value));
            if let Some(commit) = self.character_commits.take() {
                return Ok(Self::commit(commit));
            }
            return Ok(empty_result(true));
        }
        Ok(empty_result(false))
    }
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError> {
        let composing = !self.text.is_empty();
        match command {
            Command::Backspace => {
                self.text.pop();
                Ok(empty_result(composing))
            }
            Command::CommitRaw | Command::CommitCandidate if composing => {
                let text = std::mem::take(&mut self.text);
                self.conversion.clear();
                Ok(Self::commit(text))
            }
            _ => {
                self.text.clear();
                self.conversion.clear();
                Ok(empty_result(composing))
            }
        }
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        if !self.conversion.is_empty() {
            self.conversion = format!("{}*", self.conversion);
            return Ok(empty_result(true));
        }
        let commit = self
            .select_commit_override
            .take()
            .unwrap_or_else(|| self.words[index].clone());
        self.text = self.remaining_after_select.take().unwrap_or_default();
        Ok(Self::commit(commit))
    }
    fn select_edge(
        &mut self,
        index: usize,
        edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        let word = &self.words[index];
        let character = match edge {
            CandidateEdge::FirstHan => word.chars().next(),
            CandidateEdge::LastHan => word.chars().next_back(),
        };
        self.text.clear();
        Ok(Self::commit(
            character.map(String::from).unwrap_or_default(),
        ))
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        if self.text.is_empty() && self.conversion.is_empty() {
            return Ok(empty_result(false));
        }
        let commit = self
            .words
            .get(index)
            .cloned()
            .unwrap_or_else(|| self.text.clone());
        self.text.clear();
        self.conversion.clear();
        Ok(Self::commit(commit))
    }
    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        if value == b',' {
            return Ok(Self::commit("，"));
        }
        Ok(empty_result(false))
    }
    fn expand_initial_candidates(&mut self) -> Result<bool, RuntimeError> {
        if self.withheld.is_empty() {
            return Ok(false);
        }
        self.words.append(&mut self.withheld);
        Ok(true)
    }
}

fn pinyin() -> FactEngine {
    FactEngine {
        words: vec!["你好".into(), "拟好".into(), "泥".into()],
        sources: vec![0, 5, 8],
        ..FactEngine::default()
    }
}

fn focused(engine: FactEngine) -> Runtime<FactEngine> {
    let mut runtime = Runtime::new(engine, 5).unwrap();
    runtime.focus(true).unwrap();
    runtime
}

fn type_keys<E: InputEngine>(runtime: &mut Runtime<E>, keys: &[u8]) -> Transition {
    let mut last = None;
    for value in keys {
        last = Some(
            runtime
                .dispatch(Action::Character {
                    value: *value,
                    shift: false,
                })
                .unwrap(),
        );
    }
    last.unwrap()
}

fn fact(seat: u32, source: u8, how: SelectionHow, outcome: SelectionOutcome) -> SelectionFact {
    SelectionFact {
        seat,
        source,
        how,
        outcome,
    }
}

fn selection<E: InputEngine>(runtime: &Runtime<E>) -> Option<SelectionFact> {
    runtime.facts().selection
}

#[test]
fn finish_records_the_highlighted_seat_and_its_source() {
    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    runtime.dispatch(Action::NextCandidate).unwrap();
    let done = runtime.dispatch(Action::Finish).unwrap();
    assert_eq!(done.commit.as_deref(), Some("拟好"));
    assert_eq!(
        selection(&runtime),
        Some(fact(
            1,
            5,
            SelectionHow::Finish,
            SelectionOutcome::Committed
        ))
    );
    assert_eq!(runtime.facts().tail_chars, 0);
}

#[test]
fn punctuation_finishes_on_both_routes_with_the_mark_as_tail() {
    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    let done = runtime.dispatch(Action::Punctuation(b',')).unwrap();
    assert_eq!(done.commit.as_deref(), Some("你好，"));
    let facts = runtime.facts();
    assert_eq!(
        facts.selection,
        Some(fact(
            0,
            0,
            SelectionHow::PunctuationFinish,
            SelectionOutcome::Committed
        ))
    );
    assert_eq!(facts.tail_chars, 1);
    assert_eq!(facts.split_commit("你好，"), ("你好", "，"));

    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    let done = runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    assert_eq!(done.commit.as_deref(), Some("你好."));
    assert_eq!(
        selection(&runtime).map(|fact| fact.how),
        Some(SelectionHow::PunctuationFinish)
    );
    assert_eq!(runtime.facts().tail_chars, 1);

    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    let done = type_keys(&mut runtime, b",");
    assert_eq!(done.commit.as_deref(), Some("你好，"));
    assert_eq!(
        selection(&runtime).map(|fact| fact.how),
        Some(SelectionHow::PunctuationFinish)
    );
    assert_eq!(runtime.facts().tail_chars, 1);

    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    runtime.dispatch(Action::NextCandidate).unwrap();
    let done = runtime.dispatch(Action::PunctuationAscii(b'.')).unwrap();
    assert_eq!(done.commit.as_deref(), Some("拟好."));
    let facts = runtime.facts();
    assert_eq!(
        facts.selection,
        Some(fact(
            1,
            5,
            SelectionHow::PunctuationFinish,
            SelectionOutcome::Committed
        ))
    );
    assert_eq!(facts.tail_chars, 1);
    assert!(!facts.raw);
}

#[test]
fn a_digit_records_the_seat_on_the_visible_page() {
    let mut runtime = Runtime::new(
        FactEngine {
            words: (0..8).map(|n| format!("候选{n}")).collect(),
            sources: (0..8).collect(),
            ..FactEngine::default()
        },
        3,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    type_keys(&mut runtime, b"ni");
    let paged = runtime.dispatch(Action::NextPage).unwrap();
    let shown = &paged.view.candidates[1];
    assert_eq!(shown.id.index, 4);
    assert_eq!(shown.text, "候选6");
    let done = type_keys(&mut runtime, b"2");
    assert_eq!(done.commit.as_deref(), Some("候选6"));
    assert_eq!(
        selection(&runtime),
        Some(fact(
            4,
            shown.source,
            SelectionHow::Digit,
            SelectionOutcome::Committed
        ))
    );
}

#[test]
fn select_and_any_candidate_record_their_own_how() {
    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    let id = runtime.view().candidates[2].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(
        selection(&runtime),
        Some(fact(
            2,
            8,
            SelectionHow::Select,
            SelectionOutcome::Committed
        ))
    );

    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    let id = runtime.all_candidates().candidates[1].id;
    runtime.dispatch(Action::SelectAnyCandidate(id)).unwrap();
    assert_eq!(
        selection(&runtime),
        Some(fact(
            1,
            5,
            SelectionHow::AnyCandidate,
            SelectionOutcome::Committed
        ))
    );
}

#[test]
fn an_edge_pick_matches_on_the_first_or_last_character() {
    for (edge, expected) in [
        (CandidateEdge::FirstHan, "你"),
        (CandidateEdge::LastHan, "好"),
    ] {
        let mut runtime = focused(pinyin());
        type_keys(&mut runtime, b"ni");
        let id = runtime.view().candidates[0].id;
        let done = runtime.dispatch(Action::SelectEdge(id, edge)).unwrap();
        assert_eq!(done.commit.as_deref(), Some(expected));
        assert_eq!(
            selection(&runtime),
            Some(fact(0, 0, SelectionHow::Edge, SelectionOutcome::Committed)),
            "{expected}"
        );
    }
}

#[test]
fn select_highlighted_with_rows_is_a_highlighted_pick() {
    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    runtime.dispatch(Action::LastCandidate).unwrap();
    let done = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(done.commit.as_deref(), Some("泥"));
    assert_eq!(
        selection(&runtime),
        Some(fact(
            2,
            8,
            SelectionHow::Highlighted,
            SelectionOutcome::Committed
        ))
    );
}

#[test]
fn a_held_piece_is_held_and_a_plain_key_only_grows_the_hold() {
    let mut runtime = focused(FactEngine {
        remaining_after_select: Some("hao".into()),
        ..pinyin()
    });
    runtime.set_phrase_preedit(true);
    type_keys(&mut runtime, b"nihao");
    let id = runtime.view().candidates[2].id;
    let held = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(held.commit, None);
    assert_eq!(held.view.phrase_prefix, "泥");
    let facts = runtime.facts();
    assert_eq!(
        facts.selection,
        Some(fact(2, 8, SelectionHow::Select, SelectionOutcome::Held))
    );
    assert!(facts.held_grew);
    assert!(!facts.held_flushed);
    assert_eq!(facts.tail_chars, 0);

    let mut runtime = focused(FactEngine {
        character_commits: Some("甲".into()),
        ..pinyin()
    });
    runtime.set_phrase_preedit(true);
    type_keys(&mut runtime, b"n");
    assert_eq!(runtime.view().phrase_prefix, "甲");
    let facts = runtime.facts();
    assert_eq!(facts.selection, None);
    assert!(facts.held_grew);
}

#[test]
fn the_held_phrase_flushes_with_the_last_pick_and_on_blur() {
    let mut runtime = focused(FactEngine {
        remaining_after_select: Some("hao".into()),
        ..pinyin()
    });
    runtime.set_phrase_preedit(true);
    type_keys(&mut runtime, b"nihao");
    let id = runtime.view().candidates[2].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let id = runtime.view().candidates[0].id;
    let done = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(done.commit.as_deref(), Some("泥你好"));
    let facts = runtime.facts();
    assert_eq!(
        facts.selection,
        Some(fact(
            0,
            0,
            SelectionHow::Select,
            SelectionOutcome::Committed
        ))
    );
    assert!(facts.held_flushed);
    assert!(!facts.held_grew);

    let mut runtime = focused(FactEngine {
        remaining_after_select: Some("hao".into()),
        ..pinyin()
    });
    runtime.set_phrase_preedit(true);
    type_keys(&mut runtime, b"nihao");
    let id = runtime.view().candidates[2].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let blurred = runtime.focus(false).unwrap();
    assert_eq!(blurred.commit.as_deref(), Some("泥"));
    let facts = runtime.facts();
    assert_eq!(facts.selection, None);
    assert!(facts.held_flushed);

    let mut runtime = focused(FactEngine {
        remaining_after_select: Some("hao".into()),
        ..pinyin()
    });
    runtime.set_phrase_preedit(true);
    type_keys(&mut runtime, b"nihao");
    let id = runtime.view().candidates[2].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert!(!runtime.facts().held_flushed);
}

#[test]
fn a_pick_inside_sentence_conversion_pins_without_committing() {
    for action in ["select", "highlighted", "digit"] {
        let mut runtime = focused(pinyin());
        type_keys(&mut runtime, b"ni");
        runtime.engine.conversion = "你好世界".into();
        runtime.refresh().unwrap();
        runtime.dispatch(Action::NextCandidate).unwrap();
        let transition = match action {
            "select" => {
                let id = runtime.view().candidates[1].id;
                runtime.dispatch(Action::Select(id)).unwrap()
            }
            "highlighted" => runtime.dispatch(Action::SelectHighlighted).unwrap(),
            _ => type_keys(&mut runtime, b"2"),
        };
        assert_eq!(transition.commit, None, "{action}");
        assert_eq!(transition.view.conversion, "你好世界*", "{action}");
        let how = match action {
            "select" => SelectionHow::Select,
            "highlighted" => SelectionHow::Highlighted,
            _ => SelectionHow::Digit,
        };
        assert_eq!(
            selection(&runtime),
            Some(fact(1, 5, how, SelectionOutcome::Pinned)),
            "{action}"
        );
    }
}

#[test]
fn a_pick_that_leaves_reading_composing_is_partial() {
    let mut runtime = focused(FactEngine {
        remaining_after_select: Some("hao".into()),
        ..pinyin()
    });
    type_keys(&mut runtime, b"nihao");
    runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(
        selection(&runtime),
        Some(fact(
            0,
            0,
            SelectionHow::Highlighted,
            SelectionOutcome::Partial
        ))
    );
}

#[test]
fn a_commit_that_does_not_carry_the_candidate_is_not_a_selection() {
    let mut runtime = focused(FactEngine {
        select_commit_override: Some("别的".into()),
        ..pinyin()
    });
    type_keys(&mut runtime, b"ni");
    let done = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(done.commit.as_deref(), Some("别的"));
    assert_eq!(selection(&runtime), None);

    let mut runtime = focused(FactEngine::default());
    type_keys(&mut runtime, b"abc");
    let done = runtime.dispatch(Action::Finish).unwrap();
    assert_eq!(done.commit.as_deref(), Some("abc"));
    assert_eq!(selection(&runtime), None);
}

#[test]
fn a_bare_mode_prefix_ends_as_punctuation_or_raw_text_without_a_selection() {
    let bare = |first_row: &str| {
        let mut runtime = focused(FactEngine {
            spelling_symbols: "/".into(),
            words: vec![first_row.into(), "泥".into()],
            ..pinyin()
        });
        type_keys(&mut runtime, b"/");
        runtime.engine.local_mode = "command".into();
        runtime.engine.spelling_symbols.clear();
        runtime.refresh().unwrap();
        assert_eq!(runtime.view().local_mode, "command");
        assert_eq!(runtime.view().editing_text, "/");
        runtime
    };

    let mut runtime = bare("，");
    let done = runtime.dispatch(Action::Punctuation(b',')).unwrap();
    assert_eq!(done.commit.as_deref(), Some("，"));
    assert_eq!(selection(&runtime), None);
    assert!(!runtime.facts().raw);

    let mut runtime = bare("/");
    let done = runtime.dispatch(Action::PunctuationAscii(b',')).unwrap();
    assert_eq!(done.commit.as_deref(), Some("/,"));
    let facts = runtime.facts();
    assert_eq!(facts.selection, None);
    assert!(facts.raw);
    assert_eq!(facts.tail_chars, 1);
    assert_eq!(facts.split_commit("/,"), ("/", ","));

    let mut runtime = bare("/");
    let done = runtime.dispatch(Action::Finish).unwrap();
    assert_eq!(done.commit.as_deref(), Some("/"));
    assert_eq!(selection(&runtime), None);
    assert!(runtime.facts().raw);
}

#[test]
fn enter_commits_the_raw_reading() {
    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    let done = runtime
        .dispatch(Action::Command(Command::CommitRaw))
        .unwrap();
    assert_eq!(done.commit.as_deref(), Some("ni"));
    let facts = runtime.facts();
    assert_eq!(facts.selection, None);
    assert!(facts.raw);
}

#[test]
fn taking_a_selection_back_records_restore_or_drop() {
    let mut runtime = phrase_runtime("haitanp", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(runtime.facts().retracted, None);
    runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    let facts = runtime.facts();
    assert_eq!(facts.retracted, Some(SelectionRetraction::Restore));
    assert_eq!(facts.selection, None);

    let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    runtime
        .dispatch(Action::Command(Command::MoveHome))
        .unwrap();
    assert_eq!(runtime.facts().retracted, None);
    runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert_eq!(runtime.facts().retracted, Some(SelectionRetraction::Drop));

    type_keys(&mut runtime, b"x");
    assert_eq!(runtime.facts(), DispatchFacts::default());
}

#[test]
fn facts_are_reset_at_every_dispatch_and_focus() {
    let mut runtime = focused(pinyin());
    type_keys(&mut runtime, b"ni");
    runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert!(selection(&runtime).is_some());
    type_keys(&mut runtime, b"n");
    assert_eq!(runtime.facts(), DispatchFacts::default());

    runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert!(selection(&runtime).is_some());
    runtime.focus(false).unwrap();
    assert_eq!(runtime.facts(), DispatchFacts::default());

    runtime.focus(true).unwrap();
    type_keys(&mut runtime, b"n");
    runtime.dispatch(Action::SelectHighlighted).unwrap();
    runtime.focused = false;
    runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(runtime.facts(), DispatchFacts::default());
}

#[test]
fn the_split_counts_characters_so_a_fullwidth_commit_splits_in_the_same_place() {
    let mut runtime = focused(FactEngine {
        words: vec!["abc".into()],
        ..FactEngine::default()
    });
    type_keys(&mut runtime, b"abc");
    let done = runtime.dispatch(Action::PunctuationAscii(b'.')).unwrap();
    let commit = done.commit.unwrap();
    assert_eq!(commit, "abc.");
    let facts = runtime.facts();
    assert_eq!(facts.tail_chars, 1);
    assert_eq!(facts.split_commit(&commit), ("abc", "."));

    let wide = character_width::to_fullwidth(&commit);
    assert_eq!(wide, "ａｂｃ．");
    assert_eq!(facts.split_commit(&wide), ("ａｂｃ", "．"));

    let mut runtime = focused(FactEngine {
        remaining_after_select: Some("hao".into()),
        ..pinyin()
    });
    runtime.set_phrase_preedit(true);
    type_keys(&mut runtime, b"nihao");
    let id = runtime.view().candidates[2].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let done = runtime.dispatch(Action::Punctuation(b',')).unwrap();
    let commit = done.commit.unwrap();
    assert_eq!(commit, "泥你好，");
    let facts = runtime.facts();
    assert!(facts.held_flushed);
    assert_eq!(facts.split_commit(&commit), ("泥你好", "，"));

    let all_tail = DispatchFacts {
        tail_chars: 9,
        ..DispatchFacts::default()
    };
    assert_eq!(all_tail.split_commit("，"), ("", "，"));
    assert_eq!(DispatchFacts::default().split_commit("你好"), ("你好", ""));
}

#[test]
fn has_composition_ignores_snapshot_validity_and_counts_a_held_phrase() {
    let mut runtime = focused(pinyin());
    assert!(!runtime.has_composition());
    runtime.snapshot_valid = false;
    assert!(!runtime.is_idle());
    assert!(!runtime.has_composition());
    runtime.snapshot_valid = true;

    type_keys(&mut runtime, b"n");
    assert!(runtime.has_composition());

    let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert!(runtime.view().editing_text.is_empty());
    assert!(runtime.has_composition());
}

#[test]
fn epochs_move_with_what_the_user_sees() {
    let mut runtime = Runtime::new(
        FactEngine {
            words: (0..4).map(|n| format!("候选{n}")).collect(),
            withheld: vec!["候选4".into(), "候选5".into()],
            ..FactEngine::default()
        },
        2,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    let epochs =
        |runtime: &Runtime<FactEngine>| (runtime.composition_epoch(), runtime.candidates_epoch());

    let (composition, candidates) = epochs(&runtime);
    type_keys(&mut runtime, b"n");
    assert!(runtime.composition_epoch() > composition);
    assert!(runtime.candidates_epoch() > candidates);

    let before = epochs(&runtime);
    let ignored = type_keys(&mut runtime, b"0");
    assert!(!ignored.handled);
    assert_eq!(epochs(&runtime), before);

    let before = epochs(&runtime);
    runtime.dispatch(Action::NextCandidate).unwrap();
    assert_eq!(runtime.composition_epoch(), before.0);
    assert!(runtime.candidates_epoch() > before.1);

    let before = epochs(&runtime);
    runtime.dispatch(Action::PreviousPage).unwrap();
    assert_eq!(runtime.composition_epoch(), before.0);
    assert!(runtime.candidates_epoch() > before.1);

    let before = epochs(&runtime);
    runtime.dispatch(Action::PreviousPage).unwrap();
    runtime.dispatch(Action::PreviousCandidate).unwrap();
    assert_eq!(epochs(&runtime), before);

    let before = epochs(&runtime);
    let paged = runtime.dispatch(Action::NextPage).unwrap();
    assert_eq!(paged.view.page, 1);
    assert_eq!(runtime.composition_epoch(), before.0);
    assert!(runtime.candidates_epoch() > before.1);

    let before = epochs(&runtime);
    let last = runtime.dispatch(Action::NextPage).unwrap();
    assert_eq!(last.view.page_count, 3);
    assert!(runtime.candidates_epoch() > before.1);

    let before = epochs(&runtime);
    let generation = runtime.generation();
    assert!(runtime.apply_translations(generation, [("候选4".to_owned(), "gloss".to_owned())]));
    assert_eq!(runtime.composition_epoch(), before.0);
    assert!(runtime.candidates_epoch() > before.1);

    let before = epochs(&runtime);
    assert!(runtime.apply_translations(generation, [("候选4".to_owned(), "gloss".to_owned())]));
    assert!(runtime.apply_translations(generation, [("无关候选".to_owned(), "unused".to_owned())]));
    assert_eq!(epochs(&runtime), before);

    let before = epochs(&runtime);
    runtime.invalidate_translations().unwrap();
    assert_eq!(runtime.composition_epoch(), before.0);
    assert!(runtime.candidates_epoch() > before.1);
    assert!(!runtime.apply_translations(generation, [("候选4".to_owned(), "late".to_owned())]));
    let before = epochs(&runtime);
    runtime.invalidate_translations().unwrap();
    assert_eq!(epochs(&runtime), before);

    let before = epochs(&runtime);
    runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert!(runtime.composition_epoch() > before.0);
    assert!(runtime.candidates_epoch() > before.1);
}

#[test]
fn the_held_phrase_alone_moves_the_composition_epoch() {
    let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    runtime
        .dispatch(Action::Command(Command::MoveHome))
        .unwrap();
    let before = (runtime.composition_epoch(), runtime.candidates_epoch());
    let dropped = runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert_eq!(dropped.view.editing_text, "paobu");
    assert!(dropped.view.phrase_prefix.is_empty());
    assert!(runtime.composition_epoch() > before.0);
    assert_eq!(runtime.candidates_epoch(), before.1);

    let before = runtime.composition_epoch();
    runtime.dispatch(Action::Command(Command::MoveEnd)).unwrap();
    assert!(runtime.composition_epoch() > before);
}
