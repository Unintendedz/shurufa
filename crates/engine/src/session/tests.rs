//! Session-level tests: the caret-prefix rules of upstream bc46f27, wubi mixed routing (overlays.md §3.3, no C++ oracle), the personal-learning windows through `set_clock`, and ports of the `test_input_session.cpp`, `test_temporary_input_session.cpp` and `test_runtime_isolation.cpp` cases the goldens do not pin down. Every fixture is built inline, as the reference ctests did.

use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex};

use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::editing::quanpin_raw_boundaries;

use super::{Clock, Session, SessionOptions};

use crate::assets;

use crate::local::date_time::LocalDateTime;

use crate::paths::RuntimePaths;

use crate::types::{
    CandidateEdge, CandidateSource, Command, FrequencyAdjustmentMode, FrequencyAdjustmentOptions,
    LocalInputMode, SchemeKey, SchemeType, ShuangpinProfileKind, DEFAULT_FIRST_PAGE_ROWS,
};

use crate::user_dictionary::ngram_store::flush_all;

/// test_input_session.cpp:390-430 (fixture M), the rows the portable-selection, caret and edge cases read.
const QUANPIN_FIXTURE: &str =
    "CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '你好', 200);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '拟好', 100);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '𠀀方案𠮷', 90);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', 'C语言 2', 80);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', 'GitHub', 70);\
CREATE TABLE tbl_2_b(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_b VALUES('bu''hao', 'bh', '不好', 200);\
INSERT INTO tbl_2_b VALUES('bu''hao', 'bh', '补好', 100);\
CREATE TABLE tbl_1_x(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_x VALUES('xi', 'x', '西', 100);\
CREATE TABLE tbl_2_t(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_t VALUES('te''le','tl','特乐',100);\
CREATE TABLE tbl_3_x(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
CREATE TABLE tbl_1_t(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_t VALUES('te','t','特',100);\
CREATE TABLE tbl_1_l(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_l VALUES('le','l','乐',100);\
CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_h VALUES('hao','h','好',100);\
CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100);\
CREATE TABLE tbl_4_x(key TEXT,jp TEXT,value TEXT,weight INTEGER);";

const CARET_PREFIX_FIXTURE: &str =
    "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100);\
INSERT INTO tbl_1_n VALUES('ni','n','拟',90);\
CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',200);\
INSERT INTO tbl_2_n VALUES('ni''hao','nh','拟好',100);\
CREATE TABLE tbl_1_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_s VALUES('shi','sh','是',100);\
CREATE TABLE tbl_1_j(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_j VALUES('jie','j','接',100);";

/// test_personal_context_input_session.cpp:136-149 with test_pick_pair_input_session.cpp:73-94's shan/shui rows.
const CONTEXT_FIXTURE: &str = "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','甲',100),('ni','n','乙',90),('ni','n','丙',80);\
CREATE TABLE tbl_1_h(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_h VALUES('hao','h','子',100),('hao','h','丑',90),('hao','h','寅',80);\
CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',1000);\
CREATE TABLE tbl_1_s(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_s VALUES('shan','s','闪',100),('shan','s','山',90),('shui','s','睡',100),('shui','s','水',90);\
CREATE TABLE tbl_2_s(key TEXT, jp TEXT, value TEXT, weight INTEGER);";

const ENGLISH_FIXTURE: &str = "INSERT INTO english_words VALUES('he','HE',110);\
INSERT INTO english_words VALUES('hello','Hello',100);\
INSERT INTO english_words VALUES('help','Help',90);";

#[test]
fn mixed_candidate_refresh_reuses_rows_for_engine_and_caret_prefix() {
    let fixture = Fixture::new(CARET_PREFIX_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihc");
    let word_pointer = session.input.mixed_candidates[0].word.as_ptr();
    let before = session.snapshot();
    session.input.update_mixed_candidates();
    assert_eq!(
        session.input.mixed_candidates[0].word.as_ptr(),
        word_pointer
    );
    assert_eq!(session.snapshot(), before);

    session.set_caret(Some(2));
    assert!(session.input.prefix_active);
    let word_pointer = session.input.mixed_candidates[0].word.as_ptr();
    let before = session.snapshot();
    session.input.update_mixed_candidates();
    assert_eq!(
        session.input.mixed_candidates[0].word.as_ptr(),
        word_pointer
    );
    assert_eq!(session.snapshot(), before);
}

#[test]
fn journal_path_lookup_reuses_the_session_path() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let session = fixture.session();

    let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
        assert_eq!(session.input.journal_path(), session.input.journal_path());
    });

    assert_eq!(
        allocations, 0,
        "journal path lookup allocated {allocations} buffers"
    );
}

#[test]
fn ordinary_candidate_positions_skip_missing_journal_work() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let mut candidates = session.input.engine.candidates().to_vec();

    let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.input.apply_candidate_positions(&mut candidates);
    });

    assert_eq!(
        allocations, 0,
        "ordinary position refresh allocated {allocations} buffers"
    );
}

#[test]
fn personal_rerank_refresh_reuses_shown_rows_and_preserves_learning_order() {
    let fixture = Fixture::new(CONTEXT_FIXTURE);
    let mut session = fixture.session();
    for _ in 0..4 {
        type_text(&mut session, "hao");
        select_word(&mut session, "子");
        type_text(&mut session, "ni");
        select_word(&mut session, "乙");
        session.punctuation(b',');
    }
    type_text(&mut session, "hao");
    select_word(&mut session, "子");
    type_text(&mut session, "ni");

    for prefix in [false, true] {
        if prefix {
            type_text(&mut session, "hao");
            session.set_caret(Some(2));
        }
        assert!(session.input.personal_reranked);
        assert_eq!(session.input.mixed_candidates[0].word, "乙");
        let before = session.snapshot();
        let ranking = session.input.ranking_list().to_vec();
        let mut string_pointers: Vec<_> = session
            .input
            .mixed_candidates
            .iter()
            .map(|row| row.word.as_ptr())
            .collect();
        string_pointers.sort_unstable();

        session.input.update_mixed_candidates();

        assert_eq!(session.snapshot(), before);
        assert_eq!(session.input.ranking_list(), ranking);
        let mut refreshed_pointers: Vec<_> = session
            .input
            .mixed_candidates
            .iter()
            .map(|row| row.word.as_ptr())
            .collect();
        refreshed_pointers.sort_unstable();
        assert_eq!(refreshed_pointers, string_pointers);
        let original_index = session.input.ranking_index(0).unwrap();
        assert_ne!(original_index, 0);
        assert_eq!(session.input.ranking_list()[original_index].word, "乙");
    }
}

/// One directory standing in for all four runtime roots, as the reference session tests used.
struct Fixture {
    directory: tempfile::TempDir,
}

fn prediction_fixture() -> Fixture {
    Fixture::new(
        "CREATE TABLE tbl_1_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
INSERT INTO tbl_1_c VALUES('chi','c','吃',1000);
CREATE TABLE tbl_2_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
INSERT INTO tbl_2_c VALUES('chi''fan','cf','吃饭',100);
CREATE TABLE tbl_3_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
INSERT INTO tbl_3_c VALUES('chi''dong''xi','cdx','吃东西',80);
INSERT INTO tbl_3_c VALUES('chi''fan''cai','cfc','吃饭菜',60);
CREATE TABLE tbl_1_f(key TEXT,jp TEXT,value TEXT,weight INTEGER);
INSERT INTO tbl_1_f VALUES('fan','f','饭',1000);
CREATE TABLE tbl_2_f(key TEXT,jp TEXT,value TEXT,weight INTEGER);
INSERT INTO tbl_2_f VALUES('fan''hou','fh','饭后',50);
CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
INSERT INTO tbl_1_n VALUES('ni','n','你',1000);",
    )
}

#[test]
fn next_word_prediction_is_opt_in_and_has_no_composing_text() {
    let fixture = prediction_fixture();
    let mut off = fixture.session_with(|options| options.learning = false);
    type_text(&mut off, "chi");
    assert_eq!(select_word(&mut off, "吃").commit.as_deref(), Some("吃"));
    assert!(off.snapshot().candidates.is_empty());
    let mut on = fixture.session_with(|options| {
        options.next_word_prediction = true;
        options.learning = false;
    });
    type_text(&mut on, "chi");
    assert_eq!(select_word(&mut on, "吃").commit.as_deref(), Some("吃"));
    let snapshot = on.snapshot();
    assert!(snapshot.prediction);
    assert!(snapshot.preedit.is_empty() && snapshot.editing_text.is_empty());
    assert!(snapshot.candidates.iter().any(|row| row.word == "饭"));
    assert!(snapshot.candidates.iter().any(|row| row.word == "东西"));
    assert!(snapshot.candidates.iter().all(|row| row.pinyin.is_empty()));
    assert_eq!(select_word(&mut on, "饭").commit.as_deref(), Some("饭"));
    assert!(on.snapshot().candidates.iter().any(|row| row.word == "菜"));
}

#[test]
fn next_word_prediction_never_commits_on_space_return_delete_or_finish() {
    let fixture = prediction_fixture();
    for command in [
        Command::CommitCandidate,
        Command::CommitRaw,
        Command::Backspace,
        Command::DeleteForward,
    ] {
        let mut session = fixture.session_with(|options| options.next_word_prediction = true);
        type_text(&mut session, "chi");
        select_word(&mut session, "吃");
        let result = session.command(command);
        assert!(!result.handled && result.commit.is_none(), "{command:?}");
        assert!(!session.snapshot().prediction);
    }
    let mut session = fixture.session_with(|options| options.next_word_prediction = true);
    type_text(&mut session, "chi");
    select_word(&mut session, "吃");
    assert!(session.finish(0).commit.is_none());
    assert!(session.snapshot().candidates.is_empty());
}

#[test]
fn next_word_prediction_clears_on_direct_candidate_key_and_conversion_navigation() {
    let fixture = prediction_fixture();
    for conversion in [false, true] {
        let mut session = fixture.session_with(|options| options.next_word_prediction = true);
        type_text(&mut session, "chi");
        select_word(&mut session, "吃");
        let result = if conversion {
            session.conversion_left_from(0)
        } else {
            session.candidate_key(b'1')
        };
        assert!(!result.handled && result.commit.is_none());
        assert!(!session.snapshot().prediction);
    }
}

#[test]
fn next_word_prediction_uses_learned_transitions_only_when_learning_is_enabled() {
    let fixture = prediction_fixture();
    let store =
        crate::user_dictionary::ngram_store::PersonalNgramStore::for_journal(&fixture.journal());
    store
        .record(&[crate::lattice::personal::PersonalTransition {
            earlier: String::new(),
            previous: "吃".into(),
            word: "合成词".into(),
            times: 3,
        }])
        .unwrap();
    for learning in [false, true] {
        let mut session = fixture.session_with(|options| {
            options.next_word_prediction = true;
            options.learning = learning;
        });
        type_text(&mut session, "chi");
        select_word(&mut session, "吃");
        assert_eq!(
            session
                .snapshot()
                .candidates
                .iter()
                .any(|row| row.word == "合成词"),
            learning
        );
    }
}

#[test]
fn next_word_prediction_reset_does_not_learn_across_the_discarded_context() {
    let fixture = prediction_fixture();
    let mut session = fixture.session_with(|options| options.next_word_prediction = true);
    type_text(&mut session, "chi");
    select_word(&mut session, "吃");
    session.reset_context();
    type_text(&mut session, "fan");
    select_word(&mut session, "饭");
    let store =
        crate::user_dictionary::ngram_store::PersonalNgramStore::for_journal(&fixture.journal());
    assert!(store.next_words("吃", 8).is_empty());
}

impl Fixture {
    fn new(main_sql: &str) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        Connection::open(directory.path().join(assets::MAIN_DICTIONARY))
            .and_then(|connection| connection.execute_batch(main_sql))
            .expect("fixture msime-pinyin.db");
        Self { directory }
    }

    fn with_english(self, sql: &str) -> Self {
        let path = self.path().join(assets::ENGLISH_DICTIONARY);
        crate::ensure_english_schema(&path).expect("english schema");
        Connection::open(&path)
            .and_then(|connection| connection.execute_batch(sql))
            .expect("fixture msime-english.db");
        self
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn paths(&self) -> RuntimePaths {
        let root = self.path().to_path_buf();
        RuntimePaths {
            resources: root.clone(),
            user_data: root.clone(),
            cache: root.clone(),
            dictionaries: root,
        }
    }

    fn options(&self) -> SessionOptions {
        let mut options = SessionOptions::new(self.paths());
        // Frequency learning off, as the reference session tests configured it, so only the behaviour under test moves the order.
        options.frequency = FrequencyAdjustmentOptions {
            mode: FrequencyAdjustmentMode::Disabled,
            trigger_count: 1,
            linear_step: 1,
        };
        options
    }

    fn session(&self) -> Session {
        Session::new(self.options()).expect("session")
    }

    fn session_with(&self, configure: impl FnOnce(&mut SessionOptions)) -> Session {
        let mut options = self.options();
        configure(&mut options);
        Session::new(options).expect("session")
    }

    fn main_db(&self) -> PathBuf {
        self.path().join(assets::MAIN_DICTIONARY)
    }

    fn journal(&self) -> PathBuf {
        self.path().join(assets::USER_JOURNAL)
    }
}

fn type_text(session: &mut Session, text: &str) {
    for byte in text.bytes() {
        assert!(
            session.character(byte, false).handled,
            "{:?} of {text:?} was not handled",
            byte as char
        );
    }
}

fn words(session: &Session) -> Vec<String> {
    session
        .snapshot()
        .candidates
        .into_iter()
        .map(|item| item.word.into_string())
        .collect()
}

fn index_of(session: &Session, word: &str) -> usize {
    words(session)
        .iter()
        .position(|candidate| candidate == word)
        .unwrap_or_else(|| panic!("{word} is not a candidate of {:?}", words(session)))
}

fn select_word(session: &mut Session, word: &str) -> crate::types::KeyResult {
    let index = index_of(session, word);
    session.select(index)
}

fn count(database: &Path, sql: &str) -> i64 {
    Connection::open(database)
        .and_then(|connection| connection.query_row(sql, [], |row| row.get(0)))
        .unwrap_or(0)
}

/// A steady clock tests move by hand, and a pinned wall clock.
#[derive(Clone)]
struct TestClock {
    now: Arc<Mutex<Instant>>,
}

impl TestClock {
    fn install(session: &mut Session) -> Self {
        let clock = Self {
            now: Arc::new(Mutex::new(Instant::now())),
        };
        let steady = clock.now.clone();
        session.set_clock(Clock {
            steady: Box::new(move || *steady.lock().expect("clock")),
            local: Arc::new(|| LocalDateTime {
                year: 2026,
                month: 8,
                day: 9,
                weekday: 0,
                hour: 14,
                minute: 30,
                second: 0,
            }),
        });
        clock
    }

    fn advance(&self, by: Duration) {
        *self.now.lock().expect("clock") += by;
    }
}

// ---- pure helpers ----

#[test]
fn quanpin_boundaries_follow_the_display_separators() {
    assert_eq!(
        quanpin_raw_boundaries("nihaoma", "ni'hao'ma"),
        vec![0, 2, 5, 7]
    );
    assert_eq!(quanpin_raw_boundaries("ni'hao", "ni'hao"), vec![0, 3, 6]);
    assert_eq!(quanpin_raw_boundaries("ni'", "ni'"), vec![0, 3]);
    assert_eq!(quanpin_raw_boundaries("sahng", "sahng"), vec![0, 5]);
    // A display that does not spell the raw letters maps nothing rather than an arbitrary span.
    assert!(quanpin_raw_boundaries("nihao", "ni'ha").is_empty());
    assert!(quanpin_raw_boundaries("'", "'").is_empty());
}

// ---- caret prefix (upstream bc46f27) ----

#[test]
fn caret_prefix_decodes_only_the_complete_units_before_the_caret() {
    let fixture = Fixture::new(CARET_PREFIX_FIXTURE);
    let typed = |text: &str| {
        let mut other = fixture.session();
        type_text(&mut other, text);
        words(&other)
    };
    let mut session = fixture.session();
    let sentence = "ni'hao'shi'jie";
    type_text(&mut session, sentence);
    let full = words(&session);
    assert_eq!(session.prefix_end(), sentence.len());
    assert_eq!(session.pending_suffix(), "");

    for caret in [5, 6] {
        session.set_caret(Some(caret));
        assert_eq!(session.prefix_end(), 3, "caret {caret}");
        assert_eq!(session.pending_suffix(), "hao'shi'jie");
        assert!(words(&session).contains(&"你".to_owned()));
        assert_eq!(words(&session), typed("ni"));
    }

    session.set_caret(Some(7));
    assert_eq!(session.prefix_end(), 7);
    assert_eq!(session.pending_suffix(), "shi'jie");
    let at_hao = words(&session);
    assert_eq!(at_hao, typed("ni'hao"));
    session.set_caret(Some(9));
    assert_eq!(session.prefix_end(), 7);
    assert_eq!(words(&session), at_hao);

    // A caret inside the first unit has no whole unit before it, so the whole-input list stays (904bd0976) instead of an empty prefix query.
    for caret in [0, 1, 2] {
        session.set_caret(Some(caret));
        assert_eq!(words(&session), full, "caret {caret}");
        assert_eq!(session.prefix_end(), 0, "caret {caret}");
        assert_eq!(session.pending_suffix(), sentence, "caret {caret}");
    }
    session.set_caret(Some(0));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.editing_text, sentence);
    assert_eq!(snapshot.caret_position, 0);
    assert_eq!(snapshot.preedit, sentence);

    session.set_caret(Some(sentence.len() + 10));
    assert_eq!(session.snapshot().caret_position, sentence.len());
    assert_eq!(session.prefix_end(), sentence.len());
    assert_eq!(words(&session), full);
}

#[test]
fn caret_prefix_floors_to_a_shuangpin_unit() {
    let fixture = Fixture::new(CARET_PREFIX_FIXTURE);
    let microsoft = |options: &mut SessionOptions| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Microsoft;
    };
    let mut session = fixture.session_with(microsoft);
    type_text(&mut session, "nihkb;");
    assert_eq!(session.prefix_end(), 6);
    session.set_caret(Some(3));
    assert_eq!(session.prefix_end(), 2);
    assert_eq!(session.pending_suffix(), "hkb;");
    let mut prefix = fixture.session_with(microsoft);
    type_text(&mut prefix, "ni");
    assert_eq!(words(&session), words(&prefix));
}

#[test]
fn caret_commands_redecode_and_selection_returns_to_the_end() {
    let fixture = Fixture::new(CARET_PREFIX_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "i");
    session.command(Command::MoveHome);
    assert!(session.character(b'n', false).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.editing_text, "ni");
    assert_eq!(snapshot.caret_position, 1);
    // The caret sits inside the only unit, so there is no complete prefix to decode and the whole input answers.
    let whole: Vec<String> = snapshot
        .candidates
        .into_iter()
        .map(|item| item.word.into_string())
        .collect();
    assert_eq!(whole.first().map(String::as_str), Some("你"));
    session.command(Command::MoveEnd);
    assert_eq!(words(&session), whole);
    session.command(Command::Cancel);

    type_text(&mut session, "nihao");
    let full = words(&session);
    let prefix_index = index_of(&session, "你");
    session.command(Command::MoveHome);
    assert_eq!(words(&session), full);
    session.command(Command::MoveRight);
    assert_eq!(words(&session), full);
    session.command(Command::MoveEnd);
    let selected = session.select(prefix_index);
    assert_eq!(selected.commit.as_deref(), Some("你"));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.editing_text, "hao");
    assert_eq!(snapshot.caret_position, 3);
}

#[test]
fn selecting_a_prefix_candidate_exits_prefix_mode() {
    let fixture = Fixture::new(CARET_PREFIX_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihaoshijie");
    session.set_caret(Some(6));
    assert_eq!(session.prefix_end(), 5);
    let result = select_word(&mut session, "你好");
    assert_eq!(result.commit.as_deref(), Some("你好"));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.editing_text, "shijie");
    assert_eq!(snapshot.caret_position, snapshot.editing_text.len());
    assert_eq!(session.prefix_end(), "shijie".len());
    assert!(words(&session).contains(&"是".to_owned()));
}

/// 904bd0976: the prefix list goes through the same personal context reorder as the whole input, and a pinned leader keeps its seat there too.
#[test]
fn caret_prefix_follows_the_personal_context_order_and_its_pins() {
    let fixture = Fixture::new(CONTEXT_FIXTURE);
    let mut session = fixture.session();
    // 子 is followed by 乙, never by 甲, however often.
    for _ in 0..4 {
        type_text(&mut session, "hao");
        select_word(&mut session, "子");
        type_text(&mut session, "ni");
        select_word(&mut session, "乙");
        session.punctuation(b',');
    }
    type_text(&mut session, "ni");
    assert_eq!(words(&session).first().map(String::as_str), Some("甲"));
    session.command(Command::Cancel);

    type_text(&mut session, "hao");
    select_word(&mut session, "子");
    type_text(&mut session, "nihao");
    session.set_caret(Some(2));
    assert_eq!(session.prefix_end(), 2);
    assert_eq!(
        words(&session).first().map(String::as_str),
        Some("乙"),
        "after 子 the prefix ni leads with 乙: {:?}",
        words(&session)
    );
    let result = session.pin(index_of(&session, "甲"));
    assert!(result.handled && result.diagnostic.is_none(), "{result:?}");
    assert_eq!(session.prefix_end(), 2);
    assert_eq!(
        words(&session).first().map(String::as_str),
        Some("甲"),
        "the pinned leader was moved: {:?}",
        words(&session)
    );
}

/// Thirty `ni` characters, more than the 24 a lone initial lists before expansion, plus the `ni'hao` rows.
fn initial_fixture() -> Fixture {
    let rows: Vec<String> = (0..30)
        .map(|i| {
            format!(
                "('ni','n','{}',{})",
                char::from_u32(0x4e00 + i).unwrap(),
                1000 - i
            )
        })
        .collect();
    Fixture::new(&format!(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES{};\
CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_h VALUES('hao','h','好',100);\
CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',5000);",
        rows.join(",")
    ))
}

/// Paging past a lone initial's capped prefix list widens that list, the one on screen, not the hidden whole-input list.
#[test]
fn caret_prefix_expands_its_own_initial_list() {
    let fixture = initial_fixture();
    let mut session = fixture.session();
    type_text(&mut session, "nhao");
    let full = words(&session);
    session.set_caret(Some(1));
    assert_eq!(session.prefix_end(), 1);
    let capped = session.snapshot().candidates;
    assert_eq!(capped.len(), 24);
    assert!(capped.iter().all(|item| item.pinyin == "n"));

    let (grew, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.expand_initial_candidates());
    assert!(grew);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(allocations, 207, "光标前缀展开申请次数：{allocations}");
    let widened = words(&session);
    assert_eq!(widened.len(), 30);
    assert!(widened.contains(&"丝".to_owned()), "{widened:?}");
    assert_eq!(session.prefix_end(), 1);
    assert!(
        !session.expand_initial_candidates(),
        "the widened list is not capped any more"
    );

    session.command(Command::MoveEnd);
    assert_eq!(words(&session), full);
    assert_eq!(full.first().map(String::as_str), Some("你好"));
}

/// Expanding the whole input's list writes the whole input's series slot, not the slot of the caret prefix decoded just before it on the same dictionary.
#[test]
fn a_whole_input_expansion_leaves_the_prefix_slot_alone() {
    let fixture = initial_fixture();
    let mut session = fixture.session();
    type_text(&mut session, "nhao");
    session.set_caret(Some(1));
    let prefix = words(&session);
    assert_eq!(prefix.len(), 24);
    session.command(Command::MoveEnd);
    assert!(session.expand_initial_candidates());
    assert!(words(&session).len() > prefix.len() + 1);

    session.set_caret(Some(1));
    assert_eq!(words(&session), prefix);
    session.command(Command::Cancel);
    type_text(&mut session, "n");
    assert_eq!(words(&session), prefix);
}

/// Mixed English rows are looked up for the decoded prefix, not for the whole raw input.
#[test]
fn caret_prefix_mixes_english_for_the_prefix_only() {
    let fixture = Fixture::new(CARET_PREFIX_FIXTURE)
        .with_english("INSERT INTO english_words VALUES('nice','nice',100);");
    let mut session = fixture.session_with(|options| options.english.mixed_candidates = true);
    type_text(&mut session, "nihao");
    assert!(!words(&session).contains(&"nice".to_owned()));
    session.set_caret(Some(2));
    assert_eq!(session.prefix_end(), 2);
    assert!(
        words(&session).contains(&"nice".to_owned()),
        "{:?}",
        words(&session)
    );
    session.command(Command::MoveEnd);
    assert!(!words(&session).contains(&"nice".to_owned()));
}

/// 真实词库里 `hello` 拆不开（he'l'lo），全拼只能给出 和、何 这类前半截的字；英文库里正好有这个词。
const HELLO_FIXTURE: &str = "CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_h VALUES('he','h','和',200);\
INSERT INTO tbl_1_h VALUES('he','h','何',100);\
CREATE TABLE tbl_1_l(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_l VALUES('lo','l','咯',100);";

fn candidate_sources(session: &Session) -> Vec<CandidateSource> {
    session
        .snapshot()
        .candidates
        .into_iter()
        .map(|item| item.source)
        .collect()
}

/// 拼音拆不开、英文完全匹配整段输入时英文占首位，空格上屏英文；结束组字的标点也整段上屏这个英文词。能切成完整音节的 `he` 照旧是中文首选。
#[test]
fn an_exact_english_word_leads_input_pinyin_cannot_spell() {
    let fixture = Fixture::new(HELLO_FIXTURE).with_english(
        "INSERT INTO english_words VALUES('hello','hello',100);\
INSERT INTO english_words VALUES('he','he',500);",
    );
    let mut session = fixture.session_with(|options| options.english.mixed_candidates = true);
    type_text(&mut session, "hello");
    assert_eq!(words(&session)[0], "hello", "{:?}", words(&session));
    assert_eq!(
        candidate_sources(&session)[0],
        CandidateSource::EnglishDictionary
    );
    assert!(
        candidate_sources(&session)[1..]
            .iter()
            .any(|source| *source != CandidateSource::EnglishDictionary),
        "{:?}",
        words(&session)
    );
    // 空格选的是首行。
    let result = session.select(0);
    assert_eq!(result.commit.as_deref(), Some("hello"), "{result:?}");

    type_text(&mut session, "hello");
    assert_eq!(words(&session)[0], "hello", "{:?}", words(&session));
    let result = session.punctuation(b',');
    assert_eq!(result.commit.as_deref(), Some("hello，"), "{result:?}");

    type_text(&mut session, "he");
    assert_eq!(words(&session)[0], "和", "{:?}", words(&session));
    assert_eq!(words(&session)[1], "he", "{:?}", words(&session));
}

#[test]
fn capital_pinyin_spelling_keeps_the_typed_latin_word() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for scheme in [SchemeType::Quanpin, SchemeType::Shuangpin] {
        let mut session = fixture.session_with(|options| {
            options.scheme = scheme;
        });
        let text = if scheme == SchemeType::Quanpin {
            "NIHAO"
        } else {
            "NIHC"
        };
        for byte in text.bytes() {
            assert!(session.character(byte, true).handled);
        }
        assert_eq!(words(&session).first().map(String::as_str), Some(text));
        assert!(!words(&session).iter().any(|word| word.contains("你好")));
        assert_eq!(session.select(0).commit.as_deref(), Some(text));
    }
}

#[test]
fn capital_literal_prefix_keeps_the_following_lowercase_chinese_conversion() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (scheme, tail) in [
        (SchemeType::Quanpin, "nihao"),
        (SchemeType::Shuangpin, "nihc"),
    ] {
        let mut session = fixture.session_with(|options| {
            options.scheme = scheme;
        });
        assert!(session.character(b'F', true).handled);
        type_text(&mut session, tail);
        assert!(
            words(&session).contains(&"F你好".to_owned()),
            "{:?}",
            words(&session)
        );
        assert!(!words(&session).contains(&"你好".to_owned()));
        let index = words(&session)
            .iter()
            .position(|word| word == "F你好")
            .unwrap();
        assert_eq!(session.select(index).commit.as_deref(), Some("F你好"));
    }
}

#[test]
fn shifted_uppercase_starts_pinyin_and_mixes_the_cased_english_word() {
    let fixture = Fixture::new(HELLO_FIXTURE)
        .with_english("INSERT INTO english_words VALUES('observable','observable',100);");
    let mut session = fixture.session_with(|options| {
        options.english.mixed_candidates = true;
        options.english.minimum_prefix = 1;
    });

    assert!(session.character(b'O', true).handled);
    assert_eq!(session.snapshot().preedit, "O");
    type_text(&mut session, "bservable");
    assert!(words(&session).contains(&"Observable".to_owned()));
    assert_eq!(session.snapshot().preedit, "Observable");
}

#[test]
fn punctuation_after_unspellable_shifted_word_commits_typed_text() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.english.mixed_candidates = true;
        options.english.minimum_prefix = 1;
    });

    assert!(session.character(b'N', true).handled);
    type_text(&mut session, "ih");

    let result = session.punctuation(b'<');
    assert_eq!(result.commit.as_deref(), Some("Nih《"), "{result:?}");
}

#[test]
fn punctuation_after_capitalized_spelling_keeps_literal_capital() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();

    assert!(session.character(b'N', true).handled);
    type_text(&mut session, "i");

    let result = session.punctuation(b'<');
    assert_eq!(result.commit.as_deref(), Some("Ni《"), "{result:?}");
}

/// 英文领先时紧跟着的第二行才是拼音的首选，选它仍是默认选择：不调词频，不写日志。
#[test]
fn picking_the_chinese_row_behind_a_leading_english_word_is_the_default_pick() {
    let fixture = Fixture::new(HELLO_FIXTURE)
        .with_english("INSERT INTO english_words VALUES('hello','hello',100);");
    let mut session = fixture.session_with(|options| {
        options.english.mixed_candidates = true;
        options.personal_context = false;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });
    type_text(&mut session, "hello");
    assert_eq!(
        candidate_sources(&session)[0],
        CandidateSource::EnglishDictionary
    );
    let index = session.input.ranking_index(1).unwrap();
    assert_eq!(index, 1);
    assert!(session.input.ranking_list()[1].source.is_dictionary());

    let (learned, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.input.learn_candidate(index));

    assert_eq!(learned, None);
    assert_eq!(allocations, 0, "默认选择走了调词频：{allocations} 次分配");
    assert!(!fixture.journal().exists());
}

/// The prefix request carries the session's fuzzy rules.
#[test]
fn caret_prefix_applies_fuzzy_pinyin() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_z(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_z VALUES('zhi','z','之',100);\
CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_h VALUES('hao','h','好',100);",
    );
    for (rules, expected) in [(crate::types::fuzzy_rule::Z_ZH, true), (0, false)] {
        let mut session = fixture.session_with(|options| options.fuzzy_pinyin.rules = rules);
        type_text(&mut session, "zihao");
        session.set_caret(Some(2));
        assert_eq!(session.prefix_end(), 2, "rules {rules}");
        assert_eq!(
            words(&session).contains(&"之".to_owned()),
            expected,
            "rules {rules}: {:?}",
            words(&session)
        );
    }
}

/// The prefix request carries the autocorrect switches, and the suppression a raw commit of the prefix recorded.
#[test]
fn caret_prefix_applies_autocorrect_and_its_suppression() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100);\
CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_h VALUES('hao','h','好',100);",
    );
    let transposition = |options: &mut SessionOptions| {
        options.autocorrect_types = crate::types::autocorrect_type::TRANSPOSITION;
    };
    let mut session = fixture.session_with(transposition);
    type_text(&mut session, "hoani");
    session.set_caret(Some(3));
    assert_eq!(session.prefix_end(), 3);
    assert!(
        words(&session).contains(&"好".to_owned()),
        "{:?}",
        words(&session)
    );
    session.command(Command::Cancel);

    let mut plain = fixture.session();
    type_text(&mut plain, "hoani");
    plain.set_caret(Some(3));
    assert!(
        !words(&plain).contains(&"好".to_owned()),
        "{:?}",
        words(&plain)
    );

    type_text(&mut session, "hoa");
    assert!(words(&session).contains(&"好".to_owned()));
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("hoa")
    );
    type_text(&mut session, "hoani");
    session.set_caret(Some(3));
    assert_eq!(session.prefix_end(), 3);
    assert!(
        !words(&session).contains(&"好".to_owned()),
        "{:?}",
        words(&session)
    );
}

/// Microsoft `ni'nni` (golden ri_microsoft_semicolon_editing step 18): every shorter prefix group answers 你 and 拟, and the trailing `i` is also a single helpcode whose answer appends the whole-input series again, so the reference listed each word four times. A word keeps its first seat (decision 2026-09-30).
#[test]
fn shuangpin_lists_a_word_once() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);INSERT INTO tbl_1_n VALUES('ni','n','你',10000),('ni','n','拟',9000);",
    );
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Microsoft;
    });
    type_text(&mut session, "ni'nni");
    assert_eq!(words(&session), ["你", "拟"]);
}

/// #6034：双拼会话读同一个「拼音纠错」开关。合成词库里只有关系是词，小鹤把 guan（gr）的 r 按成邻键 f 就是 gen；自然码把 xi 的 x 按成邻键 c 就是 ci。
const SHUANGPIN_TYPO_FIXTURE: &str =
    "CREATE TABLE tbl_1_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_m VALUES('mei','m','没',1000),('mei','m','美',900);\
CREATE TABLE tbl_1_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_g VALUES('gen','g','跟',1000),('guan','g','关',500);\
CREATE TABLE tbl_1_x(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_x VALUES('xi','x','系',1000);\
CREATE TABLE tbl_1_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_c VALUES('ci','c','词',1000);\
CREATE TABLE tbl_2_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_g VALUES('guan''xi','gx','关系',100000);\
CREATE TABLE tbl_2_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
CREATE TABLE tbl_3_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);";

/// 生产词库里字面整句可能比纠错词更高频；纠错候选仍应出现在候选列表中，而不是只在它胜过字面整句时出现。
const SHUANGPIN_NONLEADING_TYPO_FIXTURE: &str =
    "CREATE TABLE tbl_1_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_m VALUES('mei','m','没',1000000);\
CREATE TABLE tbl_1_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_g VALUES('gen','g','跟',1000000),('guan','g','关',1000);\
CREATE TABLE tbl_1_x(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_x VALUES('xi','x','系',1000000);\
CREATE TABLE tbl_2_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_g VALUES('guan''xi','gx','关系',1);\
CREATE TABLE tbl_3_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);";

const QUANPIN_EXTRA_LETTER_FIXTURE: &str =
    "CREATE TABLE tbl_1_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_s VALUES('shang','s','上',1000);\
CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_h VALUES('hao','h','好',1000);\
CREATE TABLE tbl_2_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_s VALUES('shang''hao','sh','上好',10000);";

#[test]
fn quanpin_correction_consumes_extra_typed_letter() {
    let fixture = Fixture::new(QUANPIN_EXTRA_LETTER_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.autocorrect_types = crate::types::autocorrect_type::TRANSPOSITION
            | crate::types::autocorrect_type::NEIGHBOR;
    });
    type_text(&mut session, "shhanghao");
    let corrected = session
        .snapshot()
        .candidates
        .into_iter()
        .find(|item| item.word == "上好")
        .unwrap_or_else(|| panic!("{:?}", words(&session)));
    assert_eq!(corrected.corrected_from, "shhanghao");
    assert!(session.snapshot().candidate_answers_key[index_of(&session, "上好")]);

    assert_eq!(
        select_word(&mut session, "上好").commit.as_deref(),
        Some("上好")
    );
    assert_eq!(session.snapshot().preedit, "");
}

#[test]
fn converted_quanpin_correction_consumes_extra_typed_letter() {
    let fixture = Fixture::new(QUANPIN_EXTRA_LETTER_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.autocorrect_types = crate::types::autocorrect_type::TRANSPOSITION
            | crate::types::autocorrect_type::NEIGHBOR;
    });
    type_text(&mut session, "shhanghao");
    let corrected = index_of(&session, "上好");

    assert!(session
        .input
        .move_conversion_from(Command::ConversionLeft, corrected)
        .is_some());
    assert_eq!(session.snapshot().conversion, "上好");
    assert!(session.select(index_of(&session, "好")).handled);
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("上好")
    );
    assert_eq!(session.snapshot().preedit, "");
}

/// 纠错整句排在字面整句之后，选中它按全部按键上屏；开关关着时不出。
#[test]
fn shuangpin_offers_the_typo_sentence_under_the_autocorrect_switch() {
    let fixture = Fixture::new(SHUANGPIN_TYPO_FIXTURE);
    let shuangpin = |types: u32| {
        move |options: &mut SessionOptions| {
            options.scheme = SchemeType::Shuangpin;
            options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
            options.autocorrect_types = types;
        }
    };
    let mut session = fixture.session_with(shuangpin(
        crate::types::autocorrect_type::TRANSPOSITION | crate::types::autocorrect_type::NEIGHBOR,
    ));
    type_text(&mut session, "mwgfxi");
    let snapshot = session.snapshot();
    let corrected = snapshot
        .candidates
        .iter()
        .position(|item| item.word == "没关系")
        .unwrap_or_else(|| panic!("{:?}", words(&session)));
    assert!(corrected > 0, "{:?}", words(&session));
    assert_eq!(snapshot.candidates[corrected].corrected_from, "mwgfxi");
    assert_eq!(
        select_word(&mut session, "没关系").commit.as_deref(),
        Some("没关系")
    );

    let mut plain = fixture.session_with(shuangpin(0));
    type_text(&mut plain, "mwgfxi");
    assert!(
        !words(&plain).contains(&"没关系".to_owned()),
        "{:?}",
        words(&plain)
    );
}

#[test]
fn shuangpin_offers_a_nonleading_typo_sentence_from_a_full_dictionary() {
    let fixture = Fixture::new(SHUANGPIN_NONLEADING_TYPO_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.autocorrect_types = crate::types::autocorrect_type::TRANSPOSITION
            | crate::types::autocorrect_type::NEIGHBOR;
    });
    type_text(&mut session, "mwgfxi");
    let snapshot = session.snapshot();
    let corrected = snapshot
        .candidates
        .iter()
        .position(|item| item.word == "没关系")
        .unwrap_or_else(|| panic!("{:?}", words(&session)));
    assert!(corrected > 0, "{:?}", words(&session));
    assert_eq!(snapshot.candidates[corrected].corrected_from, "mwgfxi");
}

/// 双拼方案在宿主重建会话时切换。重建后的会话按新方案的编码纠错：自然码下 `mzgrci` 纠回没关系，小鹤的误触串 `mwgfxi` 不再纠；切回小鹤后反过来。
#[test]
fn a_rebuilt_session_corrects_with_its_new_profile() {
    let fixture = Fixture::new(SHUANGPIN_TYPO_FIXTURE);
    let shuangpin = |kind: ShuangpinProfileKind| {
        move |options: &mut SessionOptions| {
            options.scheme = SchemeType::Shuangpin;
            options.shuangpin_profile = kind;
            options.autocorrect_types = crate::types::autocorrect_type::TRANSPOSITION
                | crate::types::autocorrect_type::NEIGHBOR;
        }
    };
    for (kind, own, foreign) in [
        (ShuangpinProfileKind::Xiaohe, "mwgfxi", "mzgrci"),
        (ShuangpinProfileKind::Ziranma, "mzgrci", "mwgfxi"),
        (ShuangpinProfileKind::Xiaohe, "mwgfxi", "mzgrci"),
    ] {
        let mut session = fixture.session_with(shuangpin(kind));
        type_text(&mut session, own);
        let listed = words(&session);
        let corrected = listed.iter().position(|word| word == "没关系");
        assert!(
            corrected.is_some_and(|index| index > 0),
            "{kind:?} {own}: {listed:?}"
        );
        session.command(Command::Cancel);
        type_text(&mut session, foreign);
        assert!(
            !words(&session).contains(&"没关系".to_owned()),
            "{kind:?} {foreign}: {:?}",
            words(&session)
        );
    }
}

#[test]
fn ignored_scheme_key_does_not_clone_preedit_for_change_detection() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni'");

    let (result, allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.input.handle_character(b'\'', false)
    });

    assert!(!result.handled);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(allocations, 19);
}

#[test]
fn typing_at_the_end_does_not_build_the_preedit_twice_for_caret_detection() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");

    let (result, allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.input.handle_character(b'h', false)
    });

    assert!(result.handled);
    let matched_rows = session
        .snapshot()
        .candidates
        .iter()
        .filter(|row| row.pinyin == "ni'h")
        .count();
    assert_eq!(matched_rows, 5);
    // SharedString 克隆只增加引用计数，候选行重建的分配预算低于旧实现。
    assert_eq!(allocations, 98);
}

#[test]
fn backspacing_at_the_end_does_not_build_the_preedit_twice_for_caret_detection() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.command(Command::Backspace));

    assert!(result.handled);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(allocations, 11);
}

#[test]
fn url_entry_readiness_at_the_end_does_not_build_the_preedit_twice() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.character(b'.', false));

    assert!(result.handled);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(allocations, 6);
}

#[test]
fn url_trigger_reversion_reuses_the_remaining_text_buffer() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    assert!(session.punctuation(b'.').handled);

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.command(Command::Backspace));

    assert!(result.handled);
    assert_eq!(session.snapshot().preedit, "www");
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    assert!(
        allocations <= 33,
        "URL trigger reversion allocations: {allocations}"
    );
}

#[test]
fn prefix_end_does_not_build_editing_text_to_clamp_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    session.set_caret(Some(2));

    let (prefix_end, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.prefix_end());

    assert_eq!(prefix_end, 2);
    assert_eq!(allocations, 1);
}

#[test]
fn setting_the_caret_does_not_build_editing_text_to_clamp_it() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");

    let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.set_caret(Some(2));
    });

    assert_eq!(session.snapshot().caret_position, 2);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(allocations, 18);
}

#[test]
fn moving_the_caret_does_not_build_the_preedit_twice() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.command(Command::MoveLeft));

    assert!(result.handled);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(
        allocations, 18,
        "光标移动复用编辑文本长度后的分配次数：{allocations}"
    );
}

#[test]
fn typing_at_a_caret_reuses_the_editing_text_length() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    session.command(Command::MoveLeft);

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.character(b'x', false));

    assert!(result.handled);
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert_eq!(allocations, 164, "光标处输入的分配次数：{allocations}");
}

#[test]
fn selecting_a_quanpin_candidate_clones_only_needed_request_fields() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.learning = false;
        options.personal_context = false;
    });
    type_text(&mut session, "nihao");
    let index = index_of(&session, "你好");

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(index));

    assert_eq!(result.commit.as_deref(), Some("你好"));
    assert!(allocations <= 14, "候选选择分配次数：{allocations}");
}

#[test]
fn selecting_a_dictionary_candidate_does_not_build_context_row_vec() {
    let fixture = Fixture::new(CONTEXT_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.learning = true;
        options.personal_context = true;
    });
    type_text(&mut session, "ni");
    let index = index_of(&session, "甲");

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(index));

    assert_eq!(result.commit.as_deref(), Some("甲"));
    assert!(
        allocations <= 71,
        "个人上下文记录的中间行分配了 {allocations} 次"
    );
}

#[test]
fn selecting_a_candidate_does_not_clone_unused_sentence_words() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.learning = false;
        options.personal_context = false;
    });
    type_text(&mut session, "nihao");
    let index = index_of(&session, "你好");
    session.input.mixed_candidates[index].sentence_words = vec!["你".to_owned(), "好".to_owned()];

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(index));

    assert_eq!(result.commit.as_deref(), Some("你好"));
    assert!(
        allocations <= 35,
        "选择候选不应复制未使用的行字段，却产生了 {allocations} 次分配"
    );
}

#[test]
fn selecting_a_shuangpin_candidate_does_not_clone_the_full_request() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.learning = false;
        options.personal_context = false;
    });
    type_text(&mut session, "ni'hc");
    let index = index_of(&session, "你好");

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(index));

    assert_eq!(result.commit.as_deref(), Some("你好"));
    // 候选行字段是 `SharedString`：克隆行只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert!(allocations <= 14, "双拼候选选择分配次数：{allocations}");
}

#[test]
fn selecting_an_unsupported_candidate_does_not_clone_its_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert!(session.character(b'Y', true).handled);
    let before = session.snapshot();
    assert_eq!(before.candidate_sources, [CandidateSource::Fallback]);

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(0));

    assert!(result.handled && result.diagnostic.is_none());
    assert_eq!(result.commit.as_deref(), Some("Y"));
    assert!(session.snapshot().preedit.is_empty());
    assert!(
        allocations <= 3,
        "选择不可编辑候选产生了 {allocations} 次分配"
    );
}

#[test]
fn selecting_the_top_candidate_does_not_clone_an_unused_learning_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.personal_context = false;
    });
    type_text(&mut session, "nihao");
    let index = index_of(&session, "你好");
    assert_eq!(index, 0);

    let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
        assert_eq!(session.input.learn_candidate(index), None);
    });

    assert_eq!(
        allocations, 0,
        "top-candidate learning allocated {allocations} unused buffers"
    );
}

#[test]
fn learning_a_non_top_candidate_does_not_clone_the_whole_ranking_list() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.personal_context = false;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });
    // 固定音节边界，单独约束学习字段复制，不把备选切分冷查询计入该预算。
    type_text(&mut session, "ni'hao");
    let index = index_of(&session, "拟好");
    assert_eq!(index, 1);

    let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
        assert_eq!(session.input.learn_candidate(index), None);
    });

    assert!(
        allocations <= 411,
        "non-top learning allocated {allocations} buffers"
    );
    assert!(
        count(
            &fixture.main_db(),
            "SELECT weight FROM tbl_2_n WHERE value='拟好'"
        ) > 200
    );
}

#[test]
fn frequency_learning_borrows_an_existing_entry_key() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.personal_context = false;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });
    // 固定音节边界，单独约束学习字段复制，不把备选切分冷查询计入该预算。
    type_text(&mut session, "ni'hao");
    let index = index_of(&session, "拟好");
    assert_eq!(index, 1);

    let ((), allocations) = crate::ime::personal_rerank::allocations::count(|| {
        assert_eq!(session.input.learn_candidate(index), None);
    });

    assert!(
        allocations <= 410,
        "entry-key learning allocated {allocations} buffers"
    );
}

#[test]
fn removing_an_unsupported_candidate_does_not_clone_its_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert!(session.character(b'Y', true).handled);
    let before = session.snapshot();
    assert_eq!(before.candidate_sources, [CandidateSource::Fallback]);

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.remove(0));

    assert!(!result.handled && result.commit.is_none() && result.diagnostic.is_none());
    assert_eq!(session.snapshot(), before);
    assert_eq!(allocations, 0, "拒绝删除候选产生了 {allocations} 次分配");
}

#[test]
fn pinning_a_candidate_does_not_clone_the_full_learning_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| options.local_modes.temporary_english = true);
    assert!(session.character(b'Y', true).handled);
    assert_eq!(
        session.snapshot().candidates[0].source,
        CandidateSource::Fallback
    );

    let (result, allocations) = crate::ime::personal_rerank::allocations::count(|| session.pin(0));

    assert!(!result.handled && result.diagnostic.is_none(), "{result:?}");
    assert_eq!(
        allocations, 0,
        "pinning an unsupported candidate allocated {allocations} buffers"
    );
}

#[test]
fn removing_an_unsupported_candidate_does_not_clone_the_full_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| options.local_modes.temporary_english = true);
    assert!(session.character(b'Y', true).handled);
    assert_eq!(
        session.snapshot().candidates[0].source,
        CandidateSource::Fallback
    );

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.remove(0));

    assert!(!result.handled && result.diagnostic.is_none(), "{result:?}");
    assert_eq!(
        allocations, 0,
        "removing an unsupported candidate allocated {allocations} buffers"
    );
}

#[test]
fn position_context_borrows_normalized_pinyin() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "n");

    assert!(matches!(
        session.input.position_context(false),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn pinyin_segmentation_borrows_explicit_segmentation_when_normalized_input_is_empty() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "n'");

    assert!(matches!(
        session.input.pinyin_segmentation(),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn complete_pinyin_check_reuses_segmentation_storage() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");

    let (complete, allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.input.is_all_complete_pure_pinyin()
    });
    assert!(complete);
    assert_eq!(allocations, 0, "完整拼音判定不应复制已有切分");
}

#[test]
fn pinyin_ranking_context_borrows_normalized_segmentation() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "n");

    assert!(matches!(
        session.input.pinyin_ranking_context(),
        std::borrow::Cow::Borrowed(_)
    ));
}

/// test_candidate_removal.cpp:166-194: a removal the journal refuses is reported, commits nothing, and leaves the composition and the row where they were, for pinyin and English rows alike.
#[test]
fn a_removal_the_journal_refuses_reports_and_keeps_the_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let before = session.snapshot();
    crate::user_dictionary::journal::ensure_user_database(&fixture.journal()).expect("journal");
    Connection::open(fixture.journal())
        .and_then(|journal| journal.execute_batch("CREATE TRIGGER reject_removal BEFORE INSERT ON user_dictionary_operations BEGIN SELECT RAISE(ABORT,'fixture rejection'); END;"))
        .expect("trigger");
    let result = session.remove(index_of(&session, "拟好"));
    assert!(result.handled);
    assert_eq!(result.commit, None);
    assert_eq!(
        result.diagnostic.as_deref(),
        Some(crate::diagnostics::REMOVAL_NOT_PERSISTED)
    );
    assert_eq!(session.snapshot(), before);
    assert_eq!(
        count(
            &fixture.main_db(),
            "SELECT count(*) FROM tbl_2_n WHERE value='拟好'"
        ),
        1
    );
    assert_eq!(
        count(
            &fixture.journal(),
            "SELECT count(*) FROM user_dictionary_operations"
        ),
        0
    );
    session.command(Command::Cancel);

    session.set_dedicated_english(true);
    type_text(&mut session, "help");
    let before = session.snapshot();
    let result = session.remove(index_of(&session, "Help"));
    assert!(result.handled);
    assert_eq!(result.commit, None);
    assert_eq!(
        result.diagnostic.as_deref(),
        Some(crate::diagnostics::REMOVAL_NOT_PERSISTED)
    );
    assert_eq!(session.snapshot(), before);
    let english = fixture.path().join(assets::ENGLISH_DICTIONARY);
    assert_eq!(
        count(
            &english,
            "SELECT count(*) FROM english_words WHERE word='help'"
        ),
        1
    );
}

// ---- personal learning windows (set_clock) ----

#[test]
fn a_short_pause_keeps_the_chain_and_a_long_one_breaks_it() {
    let fixture = Fixture::new(CONTEXT_FIXTURE);
    for (pause, keeps) in [(3, true), (30, false)] {
        let mut session = fixture.session();
        let clock = TestClock::install(&mut session);
        type_text(&mut session, "ni");
        assert_eq!(
            select_word(&mut session, "甲").commit.as_deref(),
            Some("甲")
        );
        assert_eq!(session.input.chain.previous.as_deref(), Some("甲"));
        clock.advance(Duration::from_secs(pause));
        type_text(&mut session, "hao");
        assert_eq!(
            session.input.chain.previous.is_some(),
            keeps,
            "pause of {pause} s"
        );
        assert_eq!(
            select_word(&mut session, "丑").commit.as_deref(),
            Some("丑")
        );
        assert_eq!(
            session.input.chain.earlier.as_deref(),
            keeps.then_some("甲"),
            "pause of {pause} s"
        );
    }
}

#[test]
fn prefix_rows_do_not_displace_the_whole_key_leader_in_the_personal_context() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_g(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_g VALUES('gei','g','给',100),('ge','g','个',1000);\
CREATE TABLE tbl_1_w(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_w VALUES('wo','w','我',100);",
    );
    let mut session = fixture.session();
    let clock = TestClock::install(&mut session);
    for (code, word) in [("wo", "我"), ("gei", "给"), ("wo", "我")] {
        type_text(&mut session, code);
        select_word(&mut session, word);
        clock.advance(Duration::from_secs(1));
    }
    type_text(&mut session, "gei");
    let list = words(&session);
    assert!(list.iter().any(|word| word == "个"), "{list:?}");
    assert_eq!(list.first().map(String::as_str), Some("给"), "{list:?}");
}

#[test]
fn the_pause_is_measured_from_the_last_commit() {
    let fixture = Fixture::new(CONTEXT_FIXTURE);
    let mut session = fixture.session();
    let clock = TestClock::install(&mut session);
    type_text(&mut session, "ni");
    select_word(&mut session, "甲");
    clock.advance(Duration::from_secs(8));
    type_text(&mut session, "hao");
    assert!(
        session.input.chain.previous.is_some(),
        "eight seconds is not yet a pause"
    );
    session.command(Command::Cancel);
    clock.advance(Duration::from_millis(1));
    type_text(&mut session, "hao");
    assert!(session.input.chain.previous.is_none());
}

#[test]
fn picks_within_ten_seconds_become_a_phrase() {
    let fixture = Fixture::new(CONTEXT_FIXTURE);
    // The gap runs from one pick to the next, so it can outgrow ten seconds while the second word is being composed, without the eight second pause before a new composition ever applying.
    let pair = |session: &mut Session, clock: &TestClock, compose_after: u64, pick_after: u64| {
        type_text(session, "shan");
        select_word(session, "山");
        clock.advance(Duration::from_secs(compose_after));
        type_text(session, "shui");
        clock.advance(Duration::from_secs(pick_after));
        select_word(session, "水");
        session.reset_context();
    };
    let phrase = "SELECT count(*) FROM tbl_2_s WHERE key='shan''shui' AND value='山水'";

    let mut session = fixture.session();
    let clock = TestClock::install(&mut session);
    for _ in 0..3 {
        pair(&mut session, &clock, 5, 6);
    }
    assert_eq!(count(&fixture.main_db(), phrase), 0, "picks 11 s apart");

    for _ in 0..3 {
        pair(&mut session, &clock, 5, 5);
    }
    assert_eq!(count(&fixture.main_db(), phrase), 1, "picks 10 s apart");
}

// ---- test_input_session.cpp ports ----

#[test]
fn nihao_selection_and_commit() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.raw_segmentation, "ni'hao");
    assert_eq!(words(&session)[..2], ["你好".to_owned(), "拟好".to_owned()]);
    assert!(!session.select(99).handled);
    assert_eq!(session.select(1).commit.as_deref(), Some("拟好"));
    assert!(session.snapshot().preedit.is_empty());

    type_text(&mut session, "nihao");
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("你好")
    );
    type_text(&mut session, "nihao");
    assert_eq!(session.punctuation(b',').commit.as_deref(), Some("你好，"));
    type_text(&mut session, "nihao");
    assert_eq!(session.candidate_key(b'2').commit.as_deref(), Some("拟好"));
}

#[test]
fn portable_selection_keeps_the_rest_and_learns_the_phrase() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "xi'te'le");
    assert_eq!(
        select_word(&mut session, "西").commit.as_deref(),
        Some("西")
    );
    assert_eq!(session.snapshot().preedit, "te'le");
    let result = select_word(&mut session, "特乐");
    assert_eq!(result.commit.as_deref(), Some("特乐"));
    assert!(session.snapshot().preedit.is_empty());
    assert_eq!(
        count(
            &fixture.main_db(),
            "SELECT count(*) FROM tbl_3_x WHERE key='xi''te''le' AND value='西特乐'"
        ),
        1
    );

    type_text(&mut session, "xi'te'le");
    select_word(&mut session, "西");
    assert_eq!(session.punctuation(b',').commit.as_deref(), Some("特乐，"));
}

#[test]
fn backspacing_away_the_rest_abandons_the_phrase() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "xi'te'le");
    select_word(&mut session, "西");
    while !session.snapshot().preedit.is_empty() {
        assert!(session.command(Command::Backspace).handled);
    }
    type_text(&mut session, "nihao");
    assert_eq!(
        select_word(&mut session, "你好").commit.as_deref(),
        Some("你好")
    );
    assert_eq!(
        count(
            &fixture.main_db(),
            "SELECT count(*) FROM tbl_3_x WHERE value='西你好'"
        ),
        0
    );

    type_text(&mut session, "xi'te'le");
    select_word(&mut session, "西");
    assert!(session.command(Command::Cancel).handled);
    assert!(session.snapshot().preedit.is_empty());
}

#[test]
fn selection_completion_prediction_matches_the_selection() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut probe = fixture.session();
    type_text(&mut probe, "xi'te'le");
    let count = probe.snapshot().candidates.len();
    for index in 0..count {
        let mut session = fixture.session();
        type_text(&mut session, "xi'te'le");
        let predicted = session.snapshot().candidate_answers_key[index];
        session.select(index);
        assert_eq!(
            session.snapshot().preedit.is_empty(),
            predicted,
            "candidate {index}"
        );
    }
}

#[test]
fn edge_selection_commits_one_han_character() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    for (word, first, last) in [
        ("拟好", "拟", "好"),
        ("𠀀方案𠮷", "𠀀", "𠮷"),
        ("C语言 2", "语", "言"),
    ] {
        type_text(&mut session, "nihao");
        let index = index_of(&session, word);
        let result = session.select_edge(index, CandidateEdge::FirstHan);
        assert_eq!(result.commit.as_deref(), Some(first));
        assert!(session.snapshot().preedit.is_empty());
        type_text(&mut session, "nihao");
        let result = session.select_edge(index, CandidateEdge::LastHan);
        assert_eq!(result.commit.as_deref(), Some(last));
    }
    type_text(&mut session, "nihao");
    let github = index_of(&session, "GitHub");
    assert!(!session.select_edge(github, CandidateEdge::FirstHan).handled);
    assert_eq!(session.snapshot().preedit, "nihao");
}

#[test]
fn backspace_commit_raw_cancel_and_scheme_switch() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().preedit, "niha");
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("niha")
    );
    type_text(&mut session, "nihao");
    let cancelled = session.command(Command::Cancel);
    assert!(cancelled.handled && cancelled.commit.is_none());
    assert!(session.snapshot().preedit.is_empty());
    type_text(&mut session, "nihao");
    session.switch_scheme(SchemeType::Shuangpin).unwrap();
    assert!(session.snapshot().preedit.is_empty());
    assert_eq!(session.snapshot().scheme, SchemeType::Shuangpin);
}

#[test]
fn idle_keys_pass_through() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert!(!session.character(b'1', false).handled);
    assert!(!session.command(Command::Backspace).handled);
    assert!(!session.command(Command::CommitRaw).handled);
    assert!(!session.select(0).handled);
    assert!(!session.character(b'\'', false).handled);
    assert!(!session.character(b'N', false).handled);
    assert!(!session.candidate_key(b'1').handled);
}

#[test]
fn caret_editing_keeps_the_local_marker() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert!(session.character(b'K', true).handled);
    type_text(&mut session, "ab");
    session.command(Command::MoveHome);
    assert_eq!(session.snapshot().caret_position, 1);
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().editing_text, "Kab");
    session.command(Command::MoveLeft);
    assert_eq!(session.snapshot().caret_position, 1);
    session.command(Command::Cancel);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    type_text(&mut session, "ni");
    session.command(Command::MoveHome);
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().editing_text, "ni");
    session.command(Command::MoveRight);
    session.character(b'\'', false);
    session.character(b'\'', false);
    assert_eq!(session.snapshot().editing_text, "n'i");
    session.command(Command::MoveEnd);
    assert!(session.command(Command::DeleteForward).handled);
    assert_eq!(session.snapshot().editing_text, "n'i");
}

#[test]
fn an_edit_rejects_the_answer_to_the_old_composition() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    assert_eq!(query.query_text, "ni");
    assert_eq!(query.identity, "0:ni");
    assert_eq!(query.identity.capacity(), query.identity.len());
    assert!(query.cloud_eligible && query.ai_eligible);
    session.command(Command::MoveHome);
    session.command(Command::DeleteForward);
    session.character(b'n', false);
    session.command(Command::MoveEnd);
    assert_eq!(session.snapshot().editing_text, "ni");
    assert!(!session.apply_online_candidate(&query, "旧响应", CandidateSource::CloudSuggestion));
    let live = session.online_query().expect("a quanpin query");
    assert!(session.apply_online_candidate(&live, "妮", CandidateSource::CloudSuggestion));
    assert!(words(&session).contains(&"妮".to_owned()));
}

#[test]
fn accepting_an_online_answer_does_not_rebuild_the_live_query() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    let (accepted, allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.apply_online_candidate(&query, "妮", CandidateSource::CloudSuggestion)
    });

    assert!(accepted);
    assert!(
        allocations <= 41,
        "在线回包校验不应重建完整查询，仍申请了 {allocations} 次分配"
    );
}

/// test_runtime_isolation.cpp:257-268: moving the caret changes no query field, so an answer to the unchanged composition is still accepted.
#[test]
fn caret_only_movement_keeps_the_online_generation() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    for command in [
        Command::MoveHome,
        Command::MoveRight,
        Command::MoveLeft,
        Command::MoveEnd,
    ] {
        session.command(command);
        assert_eq!(
            session.online_query().map(|live| live.generation),
            Some(query.generation),
            "{command:?}"
        );
    }
    session.command(Command::MoveHome);
    // Nothing precedes the caret, so Backspace edits nothing (C++ :265-266).
    session.command(Command::Backspace);
    assert_eq!(session.snapshot().editing_text, "ni");
    assert_eq!(
        session.online_query().map(|live| live.generation),
        Some(query.generation)
    );
    session.command(Command::MoveEnd);
    assert!(session.apply_online_candidate(&query, "妮", CandidateSource::CloudSuggestion));
    assert!(words(&session).contains(&"妮".to_owned()));
}

/// While a caret prefix is decoded, an answer to the whole input goes into the whole input's list, which is not on screen: the call reports false, and the row shows once the caret returns to the end.
#[test]
fn caret_prefix_defers_a_whole_input_answer_until_move_end() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let query = session.online_query().expect("a quanpin query");
    session.set_caret(Some(2));
    assert_eq!(session.prefix_end(), 2);
    assert!(!session.apply_online_candidate(&query, "妮好", CandidateSource::CloudSuggestion));
    assert!(!words(&session).contains(&"妮好".to_owned()));
    session.command(Command::MoveEnd);
    let snapshot = session.snapshot();
    let row = snapshot
        .candidates
        .iter()
        .find(|item| item.word == "妮好")
        .expect("the answer shows at the end");
    assert_eq!(row.source, CandidateSource::CloudSuggestion);
}

/// test_online_input_session.cpp:149-176 through the singular entry every cloud response takes: cloud at the second seat, AI at the end (the reference seated it third; input-runtime now shows the first AI row in the last slot of page one, #6888), a new cloud answer replaces the old one, and a word the list already holds or a non-online source is refused without touching the list.
#[test]
fn a_single_online_answer_takes_its_slot_and_refuses_duplicates() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100),('ni','n','拟',90),('ni','n','妮',80),('ni','n','倪',70);",
    );
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    let at = |session: &Session, slot: usize| {
        let item = session.snapshot().candidates[slot].clone();
        (item.word.into_string(), item.source)
    };

    assert!(session.apply_online_candidate(&query, "泥", CandidateSource::CloudSuggestion));
    assert_eq!(
        at(&session, 1),
        ("泥".to_owned(), CandidateSource::CloudSuggestion)
    );
    assert!(session.apply_online_candidate(&query, "逆", CandidateSource::AiSuggestion));
    assert_eq!(
        at(&session, 5),
        ("逆".to_owned(), CandidateSource::AiSuggestion)
    );
    assert_eq!(words(&session), ["你", "泥", "拟", "妮", "倪", "逆"]);
    assert!(session.apply_online_candidate(&query, "呢", CandidateSource::CloudSuggestion));
    assert_eq!(
        at(&session, 1),
        ("呢".to_owned(), CandidateSource::CloudSuggestion)
    );
    assert!(!words(&session).contains(&"泥".to_owned()));

    let before = session.snapshot().candidates;
    assert!(!session.apply_online_candidate(&query, "你", CandidateSource::CloudSuggestion));
    assert_eq!(session.snapshot().candidates, before);
    assert!(!session.apply_online_candidate(&query, "腻", CandidateSource::Database));
    assert_eq!(session.snapshot().candidates, before);
}

#[test]
fn cloud_answer_replaces_its_fixed_pending_seat() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100),('ni','n','拟',90),('ni','n','妮',80);",
    );
    let mut session = fixture.session();
    session.set_cloud_candidates_enabled(true);
    type_text(&mut session, "ni");
    let before = session.snapshot();
    assert_eq!(before.candidates[1].word, "☁");
    assert_eq!(before.candidate_sources[1], CandidateSource::CloudPending);

    let query = session.online_query().expect("a cloud query");
    assert!(session.apply_online_candidate(&query, "泥", CandidateSource::CloudSuggestion));
    let after = session.snapshot();
    assert_eq!(after.candidates[1].word, "泥");
    assert_eq!(after.candidate_sources[1], CandidateSource::CloudSuggestion);
    assert_eq!(&after.candidates[2..], &before.candidates[2..]);
}

#[test]
fn clearing_one_online_source_removes_cached_rows_but_keeps_the_other_source() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100),('ni','n','拟',90);",
    );
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    assert!(session.apply_online_candidate(&query, "云候选", CandidateSource::CloudSuggestion));
    assert!(session.apply_online_candidate(&query, "AI候选", CandidateSource::AiSuggestion));
    assert!(words(&session).contains(&"云候选".to_owned()));
    assert!(words(&session).contains(&"AI候选".to_owned()));

    session.clear_online_candidates(CandidateSource::CloudSuggestion);
    assert!(!words(&session).contains(&"云候选".to_owned()));
    assert!(words(&session).contains(&"AI候选".to_owned()));

    session.command(Command::Cancel);
    type_text(&mut session, "ni");
    assert!(!words(&session).contains(&"云候选".to_owned()));
    assert!(words(&session).contains(&"AI候选".to_owned()));
}

/// test_input_session.cpp (longer phrases): a row that continues past the typed syllables keeps the typed reading as its pinyin, and selecting it commits the whole phrase and ends the composition.
#[test]
fn selecting_a_longer_phrase_commits_it_whole() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',8000),('ni','n','泥',7000);\
CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',10000),('ni''hao','nh','拟好',30);\
CREATE TABLE tbl_3_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_3_n VALUES('ni''hao''ma','nhm','你好吗',900);",
    );
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let index = index_of(&session, "你好吗");
    let row = &session.snapshot().candidates[index];
    assert_eq!(row.pinyin, "ni'hao");
    assert_eq!(row.canonical_pinyin, "ni'hao'ma");
    let result = session.select(index);
    assert!(result.handled);
    assert_eq!(result.commit.as_deref(), Some("你好吗"));
    assert!(session.snapshot().preedit.is_empty());
}

/// test_runtime_isolation.cpp:367-413: twenty threads, each with its own session on one of two roots, read only their own root's dictionary.
#[test]
fn concurrent_sessions_read_only_their_own_dictionary() {
    let isolation_root = |own: &str| {
        Fixture::new(&format!(
            "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
             INSERT INTO tbl_1_n VALUES('ni', 'n', '{own}', 100);"
        ))
    };
    let roots = [isolation_root("你"), isolation_root("妮")];
    std::thread::scope(|scope| {
        for thread in 0..20 {
            let (root, own, other) = if thread % 2 == 0 {
                (&roots[0], "你", "妮")
            } else {
                (&roots[1], "妮", "你")
            };
            scope.spawn(move || {
                let mut session = root.session();
                type_text(&mut session, "ni");
                let listed = words(&session);
                assert_eq!(listed[0], own, "thread {thread}");
                assert!(
                    !listed.iter().any(|word| word == other),
                    "thread {thread}: {listed:?}"
                );
            });
        }
    });
}

// ---- local mode fallback rows and temporary modes (overlays.md §8.1, test_temporary_input_session.cpp) ----

#[test]
fn temporary_english_shows_its_prefix_until_a_word_matches() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    assert!(session.character(b'Y', true).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "Y");
    assert_eq!(snapshot.local_mode, LocalInputMode::TemporaryEnglish);
    assert_eq!(snapshot.candidates.len(), 1);
    assert_eq!(snapshot.candidates[0].word, "Y");
    assert_eq!(snapshot.candidates[0].source, CandidateSource::Fallback);
    assert_eq!(session.punctuation(b',').commit.as_deref(), Some("Y，"));

    session.character(b'Y', true);
    type_text(&mut session, "he");
    let snapshot = session.snapshot();
    assert!(snapshot
        .candidates
        .iter()
        .all(|item| item.source != CandidateSource::Fallback));
    assert_eq!(snapshot.candidates[0].source, CandidateSource::Generated);
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("Yhe")
    );

    session.character(b'Y', true);
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("Y")
    );
    session.character(b'Y', true);
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
}

#[test]
fn selecting_a_local_generated_candidate_does_not_clone_its_learning_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    assert!(session.character(b'Y', true).handled);
    type_text(&mut session, "he");
    assert_eq!(
        session.snapshot().candidates[0].source,
        CandidateSource::Generated
    );

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(0));

    assert_eq!(result.commit.as_deref(), Some("he"));
    assert!(result.diagnostic.is_none());
    assert!(session.snapshot().preedit.is_empty());
    assert!(
        allocations <= 3,
        "本地生成候选提交产生了 {allocations} 次分配"
    );
}

#[test]
fn selecting_a_temporary_english_candidate_does_not_clone_full_ranking_rows() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.frequency = FrequencyAdjustmentOptions {
            mode: FrequencyAdjustmentMode::Promote,
            trigger_count: 1,
            linear_step: 1,
        };
    });
    assert!(session.character(b'Y', true).handled);
    type_text(&mut session, "he");
    assert_eq!(
        session.snapshot().candidates[0].source,
        CandidateSource::Generated
    );
    assert_eq!(
        session.snapshot().candidates[2].source,
        CandidateSource::EnglishDictionary
    );

    let (result, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.select(2));

    assert_eq!(result.commit.as_deref(), Some("Help"));
    assert!(result.diagnostic.is_none(), "{result:?}");
    assert!(session.snapshot().preedit.is_empty());
    let english = fixture.path().join(assets::ENGLISH_DICTIONARY);
    assert!(
        count(
            &english,
            "SELECT weight FROM english_words WHERE word='help' AND display='Help'"
        ) > 90
    );
    assert!(
        allocations <= 67,
        "temporary English frequency selection allocations: {allocations}"
    );
}

#[test]
fn shuangpin_snapshot_does_not_build_unused_raw_preedit() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_preedit_uses_raw = false;
    });
    type_text(&mut session, "nihc");

    let (snapshot, allocations) =
        crate::ime::personal_rerank::allocations::count(|| session.snapshot());

    assert_eq!(snapshot.preedit, "ni'hao");
    // 邮箱入口 `@` 及追加的字面符号各占一次分配。快照克隆候选行时 `SharedString` 字段只加引用计数，新建行的每个非空字段多一个 `Arc`。
    assert!(
        allocations <= 32,
        "shuangpin snapshot allocations: {allocations}"
    );
}

#[test]
fn an_unmatched_date_keyword_commits_as_typed() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    TestClock::install(&mut session);
    assert!(session.character(b'T', true).handled);
    type_text(&mut session, "xin");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "Txin");
    assert_eq!(snapshot.candidates.len(), 1);
    assert_eq!(snapshot.candidates[0].pinyin, "Txin");
    assert_eq!(snapshot.candidates[0].source, CandidateSource::Fallback);
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("Txin")
    );
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    session.character(b'T', true);
    type_text(&mut session, "rq");
    let dates = words(&session);
    assert_eq!(dates.first().map(String::as_str), Some("2026年8月9日"));
}

/// test_input_session.cpp:1388-1401: the whole date list on the pinned clock, lunar date last, and a selected row commits and leaves the mode.
#[test]
fn a_date_row_commits_and_leaves_date_time_mode() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    TestClock::install(&mut session);
    assert!(session.character(b'T', true).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::DateTime);
    type_text(&mut session, "rq");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "Trq");
    assert_eq!(snapshot.candidates.len(), 20);
    assert_eq!(snapshot.candidates[0].word, "2026年8月9日");
    assert_eq!(snapshot.candidates[16].word, "丙午年六月二十七日");
    assert!(snapshot
        .candidates
        .iter()
        .all(|item| item.source == CandidateSource::Generated));
    let result = session.select(0);
    assert_eq!(result.commit.as_deref(), Some("2026年8月9日"));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::None);
    assert!(snapshot.preedit.is_empty());

    // 农历关键词：首行是农历日期，后面是带星期的写法。
    session.character(b'T', true);
    type_text(&mut session, "nongli");
    assert_eq!(
        words(&session),
        [
            "丙午年六月二十七日",
            "丙午年六月二十七日 星期日",
            "丙午年六月二十七日 周日"
        ]
    );
    assert_eq!(
        session.select(1).commit.as_deref(),
        Some("丙午年六月二十七日 星期日")
    );
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    // The pinned clock reads 14:30:00, as the reference fixture's did.
    session.character(b'T', true);
    type_text(&mut session, "sj");
    assert!(
        words(&session).contains(&"2026-08-09 14:30:00".to_owned()),
        "{:?}",
        words(&session)
    );
}

#[test]
fn date_time_mode_restores_keyword_letters_at_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (keyword, index) in [
        ("rq", 0),
        ("riqi", 1),
        ("date", 2),
        ("sj", 0),
        ("shijian", 3),
        ("time", 1),
        ("xq", 0),
        ("xingqi", 2),
        ("week", 1),
        ("nl", 0),
        ("nongli", 2),
        ("yinli", 3),
    ] {
        let mut session = fixture.session();
        TestClock::install(&mut session);
        assert!(session.character(b'T', true).handled);
        type_text(&mut session, keyword);
        let expected = words(&session);
        assert!(!expected.is_empty(), "{keyword}");
        session.command(Command::MoveHome);
        for _ in 0..index {
            session.command(Command::MoveRight);
        }
        assert_eq!(session.snapshot().caret_position, index + 1);
        assert!(session.command(Command::DeleteForward).handled);
        let mut missing = keyword.to_owned();
        let letter = missing.remove(index);
        assert_eq!(session.snapshot().preedit, format!("T{missing}"));

        let result = session.character(letter as u8, false);

        assert!(result.handled, "{keyword}: {result:?}");
        assert!(result.commit.is_none(), "{keyword}: {result:?}");
        let snapshot = session.snapshot();
        assert_eq!(snapshot.local_mode, LocalInputMode::DateTime);
        assert_eq!(snapshot.preedit, format!("T{keyword}"));
        assert_eq!(snapshot.caret_position, index + 2);
        assert_eq!(words(&session), expected);
        assert!(snapshot
            .candidates
            .iter()
            .all(|item| item.source == CandidateSource::Generated));
        assert_eq!(
            session.select(0).commit.as_deref(),
            Some(expected[0].as_str())
        );
        assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
        assert!(session.snapshot().preedit.is_empty());
    }
}

#[test]
fn date_time_mode_still_rejects_non_lowercase_insertions() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for value in *b"A'1;" {
        let mut session = fixture.session();
        TestClock::install(&mut session);
        session.character(b'T', true);
        type_text(&mut session, "rq");
        session.command(Command::MoveLeft);
        let before = session.snapshot();

        let result = session.character(value, false);

        assert!(!result.handled, "{result:?}");
        assert_eq!(result.commit.as_deref(), Some(before.preedit.as_str()));
        assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
        assert!(session.snapshot().editing_text.is_empty());
    }
}

/// 组字里的日期时间行（#5952）用的词：每个关键词的锚定词，加上同一串字母、数字的其他读法。
const INLINE_DATE_TIME_FIXTURE: &str = "CREATE TABLE tbl_1_r(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_r VALUES('ri','r','日',100);\
CREATE TABLE tbl_2_r(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_r VALUES('ri''qi','rq','日期',500);\
CREATE TABLE tbl_2_p(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_p VALUES('pi''qi','pq','脾气',900);\
CREATE TABLE tbl_2_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_s VALUES('shou''ji','sj','手机',900),('shi''jian','sj','时间',800),('shi''jie','sj','世界',700);\
CREATE TABLE tbl_2_x(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_x VALUES('xing''qi','xq','星期',500);\
CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_n VALUES('nong''li','nl','农历',500);";

/// 26 键打 `riqi`、`ri'qi`、`sj`、`xingqi`、`nongli`：当前日期、时间、星期、农历接在对应的词后面，词前后的其他候选不动。
#[test]
fn date_time_keywords_add_rows_after_their_word() {
    let fixture = Fixture::new(INLINE_DATE_TIME_FIXTURE);
    let mut session = fixture.session();
    TestClock::install(&mut session);
    let date = [
        "日期",
        "2026年8月9日",
        "📅日期",
        "2026-08-09",
        "2026年8月9日 星期日",
    ];
    for keyword in ["riqi", "ri'qi"] {
        type_text(&mut session, keyword);
        assert_eq!(words(&session)[..5], date, "{keyword}");
        let snapshot = session.snapshot();
        assert_eq!(snapshot.candidates[1].source, CandidateSource::Generated);
        assert_eq!(snapshot.candidates[1].pinyin, keyword);
        assert!(snapshot.candidate_answers_key[1]);
        session.command(Command::Cancel);
    }

    type_text(&mut session, "sj");
    let sj = words(&session);
    let at = sj.iter().position(|word| word == "时间").expect("时间");
    assert_eq!(sj[..at], ["手机"]);
    assert_eq!(
        sj[at + 1..at + 5],
        ["14:30", "🕒时间", "14:30:00", "下午2:30"]
    );
    session.command(Command::Cancel);

    type_text(&mut session, "xingqi");
    assert_eq!(words(&session)[..4], ["星期", "星期日", "📅星期", "周日"]);
    session.command(Command::Cancel);

    type_text(&mut session, "nongli");
    assert_eq!(
        words(&session)[..3],
        ["农历", "丙午年六月二十七日", "📅日期"]
    );
    session.command(Command::Cancel);

    // 不是关键词的组字、关键词的一部分都不加。
    for typed in ["ri", "riq", "shou"] {
        type_text(&mut session, typed);
        assert!(
            words(&session).iter().all(|word| !word.contains("2026")
                && !word.contains("14:30")
                && !word.starts_with("星期日")),
            "{typed}: {:?}",
            words(&session)
        );
        session.command(Command::Cancel);
    }
}

/// `utility.date_time_candidates`：行内日期时间混入与 T 模式是两个开关。混入关着时关键词只出它本来的词，T 模式仍照常；T 模式关着而混入开着时日期行照出。
#[test]
fn inline_date_time_is_a_separate_switch_from_the_mode() {
    let fixture = Fixture::new(INLINE_DATE_TIME_FIXTURE);
    let mut session = fixture.session_with(|options| options.local_modes.date_time_inline = false);
    TestClock::install(&mut session);
    type_text(&mut session, "riqi");
    assert!(
        !words(&session)
            .iter()
            .any(|word| word.contains("2026") || word.contains("14:30") || word.starts_with("📅")),
        "{:?}",
        words(&session)
    );
    session.command(Command::Cancel);

    let mut session = fixture.session_with(|options| {
        options.local_modes.date_time = false;
        options.local_modes.date_time_inline = true;
    });
    TestClock::install(&mut session);
    type_text(&mut session, "riqi");
    assert_eq!(
        words(&session)[..5],
        [
            "日期",
            "2026年8月9日",
            "📅日期",
            "2026-08-09",
            "2026年8月9日 星期日"
        ]
    );
    session.command(Command::Cancel);
}

/// 选中日期行上屏它的文字并结束组字；它不是拼音读出的词，打开学习时也不写进词库和日志。
#[test]
fn a_date_time_row_commits_without_learning() {
    let fixture = Fixture::new(INLINE_DATE_TIME_FIXTURE);
    let mut session = fixture.session_with(|options| options.learning = true);
    TestClock::install(&mut session);
    type_text(&mut session, "riqi");
    let result = select_word(&mut session, "2026-08-09");
    assert_eq!(result.commit.as_deref(), Some("2026-08-09"));
    assert!(session.snapshot().preedit.is_empty());
    assert_eq!(
        count(
            &fixture.journal(),
            "SELECT count(*) FROM user_dictionary_operations"
        ),
        0
    );
    assert_eq!(count(&fixture.main_db(), "SELECT count(*) FROM tbl_2_r"), 1);
}

/// `utility.date_time_menu`：「📅日期」入口跟在第一条行内日期行后面（锚定词在首位时是第三位），选中它不上屏、候选窗换成这一组的全部格式；再选一条格式上屏并结束组字；其他键先换回原来的候选再照常处理。
#[test]
fn date_time_menu_opens_the_full_format_list() {
    let fixture = Fixture::new(INLINE_DATE_TIME_FIXTURE);
    let mut session = fixture.session();
    TestClock::install(&mut session);
    type_text(&mut session, "riqi");
    let first = words(&session);
    assert_eq!(first[2], "📅日期", "{first:?}");
    let menu = &session.snapshot().candidates[2];
    assert_eq!(menu.source, CandidateSource::Generated);
    assert_eq!(menu.pinyin, "menu:riqi");

    // 选中入口：不上屏，候选窗换成 T 模式的整组格式，组字原样留着。
    let result = session.select(2);
    assert!(result.handled);
    assert!(result.commit.is_none());
    let page = words(&session);
    assert_eq!(page.len(), 20, "{page:?}");
    assert_eq!(page[0], "2026年8月9日");
    assert_eq!(page[16], "丙午年六月二十七日");
    assert!(!session.snapshot().preedit.is_empty());

    // 页里选一条格式：照常上屏并结束组字。
    assert_eq!(session.select(1).commit.as_deref(), Some("2026-08-09"));
    assert!(session.snapshot().preedit.is_empty());

    // 非选词键先换回原来的候选再照常处理：退格删掉最后一个字母，候选跟着变。
    type_text(&mut session, "riqi");
    session.select(2);
    assert_eq!(words(&session).len(), 20);
    assert!(session.command(Command::Backspace).handled);
    let after = words(&session);
    assert!(
        !after.contains(&"丙午年六月二十七日".to_owned()),
        "{after:?}"
    );
    assert!(!after.contains(&"📅日期".to_owned()), "{after:?}");
    session.command(Command::Cancel);

    // 星期、时间的入口文案与参考一致；锚定词不在首位时入口仍在第一条行内行后面。
    type_text(&mut session, "sj");
    let sj = words(&session);
    let at = sj.iter().position(|word| word == "时间").expect("时间");
    assert_eq!(sj[at + 2], "🕒时间", "{sj:?}");
    session.command(Command::Cancel);
    type_text(&mut session, "xingqi");
    assert_eq!(words(&session)[2], "📅星期");
    session.command(Command::Cancel);
    type_text(&mut session, "nongli");
    assert_eq!(words(&session)[2], "📅日期");
    session.command(Command::Cancel);

    // 关掉开关时只有行内日期行，没有入口。
    let mut session = fixture.session_with(|options| options.local_modes.date_time_menu = false);
    TestClock::install(&mut session);
    type_text(&mut session, "riqi");
    assert_eq!(
        words(&session)[..4],
        ["日期", "2026年8月9日", "2026-08-09", "2026年8月9日 星期日"]
    );
}

#[test]
fn super_jianpin_bare_entry_keeps_capital_words_literal() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for scheme in [SchemeType::Quanpin, SchemeType::Shuangpin] {
        for text in ["JI", "JIT", "JNH", "JPEG", "JSON"] {
            let mut session = fixture.session_with(|options| options.scheme = scheme);
            for byte in text.bytes() {
                let result = session.character(byte, true);
                assert!(result.handled && result.commit.is_none());
            }
            let snapshot = session.snapshot();
            assert_eq!(
                snapshot.local_mode,
                LocalInputMode::None,
                "{scheme:?} {text}"
            );
            assert_eq!(snapshot.editing_text, text);
            assert_eq!(words(&session), [text]);
            assert_eq!(session.select(0).commit.as_deref(), Some(text));
        }
    }
}

#[test]
fn super_jianpin_lowercase_entry_and_unicode_hex_stay_available() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for scheme in [SchemeType::Quanpin, SchemeType::Shuangpin] {
        let mut session = fixture.session_with(|options| options.scheme = scheme);
        session.character(b'J', true);
        type_text(&mut session, "nh");
        assert_eq!(session.snapshot().local_mode, LocalInputMode::SuperJianpin);
        assert!(words(&session).contains(&"你好".to_owned()));
        let at = words(&session)
            .iter()
            .position(|word| word == "你好")
            .unwrap();
        assert_eq!(session.select(at).commit.as_deref(), Some("你好"));

        session.character(b'U', true);
        session.character(b'F', true);
        assert_eq!(session.snapshot().local_mode, LocalInputMode::Unicode);
        type_text(&mut session, "F21");
        assert_eq!(session.snapshot().local_mode, LocalInputMode::Unicode);
        assert!(words(&session).contains(&"Ａ".to_owned()));
    }
}

#[test]
fn rejected_capital_after_bare_shortcut_keeps_both_letters_composing() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for scheme in [SchemeType::Quanpin, SchemeType::Shuangpin] {
        for (entry, continuation) in [
            (b'U', b'V'),
            (b'U', b'I'),
            (b'T', b'A'),
            (b'K', b'A'),
            (b'V', b'Z'),
        ] {
            let mut session = fixture.session_with(|options| {
                options.scheme = scheme;
                options.local_modes.expression = true;
            });
            assert!(session.character(entry, true).handled);
            assert_ne!(session.snapshot().local_mode, LocalInputMode::None);
            let result = session.character(continuation, true);
            assert!(
                result.handled,
                "{scheme:?}: {}{}",
                entry as char, continuation as char
            );
            assert!(result.commit.is_none());
            let expected = String::from_utf8(vec![entry, continuation]).unwrap();
            let snapshot = session.snapshot();
            assert_eq!(snapshot.local_mode, LocalInputMode::None);
            assert_eq!(snapshot.preedit, expected);
            assert_eq!(
                session.command(Command::CommitRaw).commit.as_deref(),
                Some(expected.as_str())
            );
        }
    }
}

#[test]
fn direct_local_caret_inserts_after_the_entry_marker() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (entry, spelling, value) in [(b'U', "0041", b'+'), (b'Y', "ab", b'x')] {
        let mut session = fixture.session();
        assert!(session.character(entry, true).handled);
        type_text(&mut session, spelling);
        let mode = session.snapshot().local_mode;
        session.set_caret(Some(0));

        let result = session.character(value, false);

        assert!(
            result.handled && result.commit.is_none(),
            "{entry}: {result:?}"
        );
        let snapshot = session.snapshot();
        assert_eq!(
            snapshot.editing_text,
            format!("{}{}{spelling}", entry as char, value as char)
        );
        assert_eq!(snapshot.local_mode, mode);
        assert_eq!(snapshot.caret_position, 2);
    }
}

#[test]
fn direct_caret_keeps_zero_for_modes_without_an_entry_marker() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (typed, dedicated, expected) in [
        ("ni", false, "i"),
        ("ab", true, "b"),
        ("www.example", false, "ww.example"),
    ] {
        let mut session = fixture.session();
        session.set_dedicated_english(dedicated);
        type_text(&mut session, typed);
        let mode = session.snapshot().local_mode;
        session.set_caret(Some(0));
        assert_eq!(session.snapshot().caret_position, 0);

        let result = session.command(Command::DeleteForward);

        assert!(result.handled && result.commit.is_none());
        let snapshot = session.snapshot();
        assert_eq!(snapshot.editing_text, expected);
        assert_eq!(snapshot.caret_position, 0);
        assert_eq!(snapshot.local_mode, mode);
    }
}

#[test]
fn direct_local_caret_keeps_end_clamping_and_bare_marker_exit() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    session.character(b'U', true);
    session.set_caret(Some(0));
    assert_eq!(session.snapshot().caret_position, 1);
    assert!(session.command(Command::DeleteForward).handled);
    assert_eq!(session.snapshot().editing_text, "U");
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    assert!(session.snapshot().editing_text.is_empty());

    session.character(b'U', true);
    type_text(&mut session, "0041");
    session.set_caret(Some(usize::MAX));
    assert_eq!(session.snapshot().caret_position, 5);
    session.set_caret(Some(0));
    session.set_caret(None);
    assert_eq!(session.snapshot().caret_position, 5);
    let result = session.character(b'2', false);
    assert!(result.handled && result.commit.is_none());
    assert_eq!(session.snapshot().editing_text, "U00412");
}

#[test]
fn accepted_local_mode_characters_keep_spelling_until_enter() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (entry, spelling) in [(b'U', "+4e2d"), (b'T', "rq"), (b'K', "ab")] {
        let mut session = fixture.session();
        assert!(session.character(entry, true).handled);
        for value in spelling.bytes() {
            let result = session.character(value, value.is_ascii_uppercase());
            assert!(result.handled);
            assert!(result.commit.is_none());
        }
        let raw = format!("{}{spelling}", entry as char);
        assert_eq!(session.snapshot().preedit, raw);
        assert_eq!(
            session.command(Command::CommitRaw).commit.as_deref(),
            Some(raw.as_str())
        );
    }
}

#[test]
fn temporary_local_modes_keep_their_composition_when_returning_a_key() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (entry, caret) in [(b'Y', None), (b'R', None), (b'Y', Some(1)), (b'R', Some(1))] {
        let mut session = fixture.session();
        assert!(session.character(entry, true).handled);
        type_text(&mut session, "ka");
        session.set_caret(caret);
        let before = session.snapshot();
        let result = session.character(b'0', false);
        assert!(!result.handled);
        assert!(result.commit.is_none());
        assert_eq!(session.input.preedit(), before.preedit);
        assert_eq!(session.snapshot().local_mode, before.local_mode);
        assert_eq!(session.snapshot().caret_position, before.caret_position);
    }
}

#[test]
fn disabled_local_modes_leave_the_key_to_pinyin() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.local_modes.temporary_english = false;
        options.local_modes.unicode = false;
    });
    for &letter in b"YU" {
        assert!(session.character(letter, true).handled);
        assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
        assert_eq!(session.snapshot().preedit, (letter as char).to_string());
        session.command(Command::Cancel);
    }
    assert!(session.character(b'K', true).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::QuickPhrase);
}

#[test]
fn dedicated_english_offers_the_typed_word_when_unknown() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    session.set_dedicated_english(true);
    // Every non-letter is swallowed, even with nothing composed.
    assert!(session.character(b'1', false).handled);
    type_text(&mut session, "HE");
    assert_eq!(words(&session)[..2], ["HE".to_owned(), "Hello".to_owned()]);
    session.command(Command::Cancel);
    type_text(&mut session, "Codex");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.candidates.len(), 1);
    assert_eq!(snapshot.candidates[0].word, "Codex");
    assert_eq!(snapshot.candidates[0].source, CandidateSource::Generated);
    let result = session.command(Command::CommitRaw);
    assert_eq!(result.commit.as_deref(), Some("Codex"));
    assert!(result.diagnostic.is_none(), "{result:?}");
    assert!(session.snapshot().dedicated_english);
}

#[test]
fn english_online_requires_opt_in_and_answers_enter_the_visible_list() {
    for dedicated in [true, false] {
        let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
        let mut session = generated_modes_session(&fixture);
        if dedicated {
            session.set_dedicated_english(true);
        } else {
            session.character(b'Y', true);
        }
        type_text(&mut session, "accessability");
        assert!(session.online_query().is_none());
        let local = words(&session);
        session.set_english_online_candidates_enabled(true);
        let query = session.online_query().unwrap();
        assert_eq!(query.mode, crate::OnlineInputMode::English);
        assert_eq!(query.query_text, "accessability");
        assert_eq!(query.cache_key, "english:accessability");
        assert!(query.pinyin_segments.is_empty());
        assert!(query.ai_eligible && query.cloud_eligible);
        assert!(session.apply_online_candidates(
            &query,
            &[
                "accessibility".into(),
                "accessibility".into(),
                "accessible".into()
            ],
            CandidateSource::AiSuggestion,
        ));
        assert_eq!(&words(&session)[..local.len()], local);
        assert_eq!(
            &words(&session)[local.len()..],
            ["accessibility", "accessible"]
        );
        let chosen = session.select(local.len());
        assert_eq!(chosen.commit.as_deref(), Some("accessibility"));
        assert!(!session.apply_online_candidate(
            &query,
            "accessible",
            CandidateSource::AiSuggestion
        ));
        assert!(session.online_query().is_none());
    }
}

#[test]
fn english_online_edits_revocation_and_mode_changes_reject_old_answers() {
    for dedicated in [true, false] {
        let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
        let mut session = generated_modes_session(&fixture);
        session.set_english_online_candidates_enabled(true);
        if dedicated {
            session.set_dedicated_english(true);
        } else {
            session.character(b'Y', true);
        }
        type_text(&mut session, "accessability");
        let old = session.online_query().unwrap();
        assert!(session.apply_online_candidate(
            &old,
            "accessibility",
            CandidateSource::AiSuggestion
        ));
        session.character(b'x', false);
        assert!(!words(&session).contains(&"accessibility".to_owned()));
        session.command(Command::Backspace);
        assert!(!session.apply_online_candidate(
            &old,
            "accessibility",
            CandidateSource::AiSuggestion
        ));
        let current = session.online_query().unwrap();
        assert!(session.apply_online_candidate(
            &current,
            "accessibility",
            CandidateSource::AiSuggestion
        ));
        session.command(Command::MoveHome);
        session.command(Command::DeleteForward);
        assert!(!words(&session).contains(&"accessibility".to_owned()));
        assert!(!session.apply_online_candidate(
            &current,
            "accessibility",
            CandidateSource::AiSuggestion
        ));
        let current = session.online_query().unwrap();
        assert!(session.apply_online_candidate(
            &current,
            "accessible",
            CandidateSource::AiSuggestion
        ));
        session.set_english_online_candidates_enabled(false);
        assert!(session.online_query().is_none());
        assert!(!words(&session).contains(&"accessible".to_owned()));
        assert!(!session.apply_online_candidate(
            &current,
            "accessible",
            CandidateSource::AiSuggestion
        ));
        session.set_english_online_candidates_enabled(true);
        assert!(!session.apply_online_candidate(
            &current,
            "accessible",
            CandidateSource::AiSuggestion
        ));
        let current = session.online_query().unwrap();
        session.set_dedicated_english(!dedicated);
        assert!(!session.apply_online_candidate(
            &current,
            "accessible",
            CandidateSource::AiSuggestion
        ));
    }
}

#[test]
fn english_online_batches_replace_sources_and_validate_query_mode() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    session.set_english_online_candidates_enabled(true);
    session.set_dedicated_english(true);
    type_text(&mut session, "HE");
    let query = session.online_query().unwrap();
    let mut wrong_mode = query.clone();
    wrong_mode.mode = crate::OnlineInputMode::Pinyin;
    assert!(!session.apply_online_candidate(&wrong_mode, "Helper", CandidateSource::AiSuggestion));
    assert!(session.apply_online_candidates(
        &query,
        &["Helper".into()],
        CandidateSource::AiSuggestion
    ));
    assert!(session.apply_online_candidate(&query, "Hero", CandidateSource::CloudSuggestion));
    assert!(!session.apply_online_candidates(&query, &[], CandidateSource::AiSuggestion));
    assert!(words(&session).contains(&"Helper".to_owned()));
    // 同源批次即使只重复本地词，也必须撤回旧在线行。
    assert!(session.apply_online_candidates(
        &query,
        &["Hello".into()],
        CandidateSource::AiSuggestion
    ));
    assert!(!words(&session).contains(&"Helper".to_owned()));
    assert_eq!(
        words(&session)
            .iter()
            .filter(|word| *word == "Hello")
            .count(),
        1
    );
    session.clear_online_candidates(CandidateSource::CloudSuggestion);
    assert!(!words(&session).contains(&"Hero".to_owned()));
    assert!(!session.apply_online_candidate(&query, "Hero", CandidateSource::CloudSuggestion));
}

#[test]
fn dedicated_english_refresh_reuses_candidate_row_storage() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut old_style = fixture.session();
    old_style.set_dedicated_english(true);
    type_text(&mut old_style, "HE");
    let (_, old_allocations) = crate::ime::personal_rerank::allocations::count(|| {
        let prefix =
            crate::ime::queries::lowercase_prefix(&old_style.input.dedicated_english_preedit);
        let mut candidates = old_style
            .input
            .queries
            .english_dictionary()
            .query_prefix(&prefix, crate::ime::queries::MODE_ENGLISH_LIMIT);
        old_style.input.apply_candidate_positions(&mut candidates);
        old_style.input.dedicated_english_candidates = candidates;
    });

    let mut session = fixture.session();
    session.set_dedicated_english(true);
    type_text(&mut session, "HE");
    assert!(session.input.dedicated_english_candidates.len() >= 2);
    let pointer = session.input.dedicated_english_candidates.as_ptr();
    let capacity = session.input.dedicated_english_candidates.capacity();
    let words = crate::ime::personal_rerank::allocations::unshared_rows(
        &session.input.dedicated_english_candidates,
    );

    let (_, allocations) = crate::ime::personal_rerank::allocations::count(|| {
        session.input.update_dedicated_english_candidates();
    });

    assert_eq!(session.input.dedicated_english_candidates, words);
    assert_eq!(session.input.dedicated_english_candidates.as_ptr(), pointer);
    assert_eq!(
        session.input.dedicated_english_candidates.capacity(),
        capacity
    );
    assert!(
        allocations < old_allocations,
        "专用英文热刷新分配 {allocations} 次，旧路径为 {old_allocations} 次"
    );
}

#[test]
fn dedicated_english_caret_rejects_nonletters_without_releasing_the_key() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    session.set_dedicated_english(true);
    type_text(&mut session, "HE");
    session.command(Command::MoveLeft);
    let before = session.snapshot();
    assert_eq!(before.caret_position, 1);

    for value in *b"0123456789 !\t" {
        let result = session.character(value, false);
        assert!(result.handled, "{value:?}: {result:?}");
        assert!(result.commit.is_none(), "{value:?}: {result:?}");
        assert_eq!(session.snapshot(), before, "{value:?}");
    }

    assert!(session.character(b'L', true).handled);
    assert_eq!(session.snapshot().preedit, "HLE");
    assert_eq!(session.snapshot().caret_position, 2);
}

#[test]
fn dedicated_english_writes_ascii_punctuation_unless_locked_to_chinese() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = fixture.session();
    session.set_dedicated_english(true);
    // 组字中：结束组字，接半角句号，不是「。」。
    type_text(&mut session, "HE");
    let period = session.punctuation(b'.');
    assert!(period.handled);
    let commit = period.commit.unwrap_or_default();
    assert!(commit.ends_with('.') && !commit.contains('。'), "{commit}");
    assert!(!session.input.has_composition());
    // 空闲：交还宿主，由宿主插入按键本身。
    let idle = session.punctuation(b'.');
    assert!(!idle.handled && idle.commit.is_none(), "{idle:?}");
    assert!(!session.punctuation(b',').handled);

    // 「始终使用中文标点」（lock 1）在英文模式下也给中文标点。
    session.set_punctuation_lock(1).unwrap();
    assert_eq!(session.punctuation(b'.').commit.as_deref(), Some("。"));
    // 「始终使用英文标点」（lock 2）同样是半角。
    session.set_punctuation_lock(2).unwrap();
    assert!(!session.punctuation(b'.').handled);

    // 中文模式不受影响：默认跟随中英文状态时仍是中文标点。
    session.set_punctuation_lock(0).unwrap();
    session.set_dedicated_english(false);
    assert_eq!(session.punctuation(b'.').commit.as_deref(), Some("。"));
}

/// Rows in every table of the user journal, 0 when nothing ever opened it.
fn journal_rows(journal: &Path) -> i64 {
    let Ok(connection) = Connection::open(journal) else {
        return 0;
    };
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .unwrap_or_default();
    tables
        .iter()
        .map(|table| count(journal, &format!("SELECT count(*) FROM \"{table}\"")))
        .sum()
}

// ---- expression, command and mention modes ----

fn generated_modes_session(fixture: &Fixture) -> Session {
    let mut session = fixture.session_with(|options| {
        options.local_modes.expression = true;
        options.local_modes.command = true;
        options.local_modes.mention = true;
        options.command_table = vec![crate::types::CommandTableEntry {
            trigger: "sig".to_owned(),
            title: "签名".to_owned(),
            template: "张三 {date}".to_owned(),
        }];
        options.mention_entries = vec![
            crate::types::MentionEntry {
                phone: String::new(),
                text: "张三".to_owned(),
                key: "zhang'san".to_owned(),
            },
            crate::types::MentionEntry {
                phone: String::new(),
                text: "深圳市".to_owned(),
                key: "shen'zhen'shi".to_owned(),
            },
        ];
    });
    TestClock::install(&mut session);
    session
}

#[test]
fn generated_modes_are_off_by_default() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert!(session.snapshot().spelling_symbols.is_empty());
    assert!(session.character(b'V', true).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    assert_eq!(session.snapshot().preedit, "V");
    session.command(Command::Cancel);
    assert!(!session.character(b'/', false).handled);
    assert!(!session.character(b'@', false).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    // `/` still has no Chinese mark and goes to the host as typed.
    assert!(!session.punctuation(b'/').handled);
}

#[test]
fn expression_mode_evaluates_what_follows_shift_v() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    assert!(session.character(b'V', true).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Expression);
    assert_eq!(snapshot.spelling_symbols, "0123456789+-*/.()%^#$&");
    assert_eq!(words(&session), ["V"]);
    // 数字之前的字母不能表示单位，提交入口字母后交还当前键。
    let rejected = session.character(b'x', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some("V"));
    assert!(session.snapshot().preedit.is_empty());

    session.character(b'V', true);
    assert!(session.character(b'1', false).handled);
    // An operator reported as punctuation extends the expression instead of finishing it.
    let plus = session.punctuation(b'+');
    assert!(plus.handled && plus.commit.is_none(), "{plus:?}");
    assert!(session.character(b'2', false).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "V1+2");
    assert_eq!(words(&session), ["3", "1+2=3", "叁元整"]);
    assert!(snapshot.candidate_annotations.iter().all(String::is_empty));
    assert!(session.online_query().is_none());

    let result = session.select(1);
    assert_eq!(result.commit.as_deref(), Some("1+2=3"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
}

#[test]
fn expression_mode_commits_finite_results_near_the_float_limit() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    for (input, expected) in [
        ("(2^1023)*1.9999999999999998", "1.79769313486232e308"),
        ("(2^1023)*-1.9999999999999998", "-1.79769313486232e308"),
    ] {
        assert!(session.character(b'V', true).handled);
        type_text(&mut session, input);
        assert_eq!(
            words(&session),
            [expected.to_owned(), format!("{input}={expected}")]
        );
        assert_eq!(session.select(0).commit.as_deref(), Some(expected));
    }
}

#[test]
fn expression_mode_converts_units_after_a_number() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.character(b'V', true);
    // 单位字母之前的撇号不是分隔符，提交原文并退出。
    let rejected = session.character(b'\'', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some("V"));
    session.character(b'V', true);
    type_text(&mut session, "3jin");
    // A host that reports the apostrophe as punctuation still gets the separator.
    let apostrophe = session.punctuation(b'\'');
    assert!(
        apostrophe.handled && apostrophe.commit.is_none(),
        "{apostrophe:?}"
    );
    // 第二个撇号同样交还宿主；重新进入后继续验证正常单位转换。
    let rejected = session.character(b'\'', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some("V3jin'"));
    session.character(b'V', true);
    type_text(&mut session, "3jin'g");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "V3jin'g");
    assert_eq!(words(&session), ["1500克", "1500", "3斤=1500克"]);
    assert!(snapshot
        .candidates
        .iter()
        .all(|candidate| candidate.source == CandidateSource::Generated));
    assert!(session.online_query().is_none());
    assert_eq!(session.select(2).commit.as_deref(), Some("3斤=1500克"));

    // Input the unit table cannot read keeps the expression path and its raw fallback.
    session.character(b'V', true);
    type_text(&mut session, "3xyz");
    assert_eq!(words(&session), ["V3xyz"]);
}

#[test]
fn expression_mode_converts_signed_measurements() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    for (input, expected) in [
        ("-10cf", vec!["14℉", "14", "-10℃=14℉"]),
        ("+0.5kg'g", vec!["500克", "500", "+0.5千克=500克"]),
    ] {
        session.character(b'V', true);
        for byte in input.bytes() {
            let result = if matches!(byte, b'-' | b'+' | b'.' | b'\'') {
                session.punctuation(byte)
            } else {
                session.character(byte, false)
            };
            assert!(
                result.handled && result.commit.is_none(),
                "{input}: {result:?}"
            );
        }
        assert_eq!(session.snapshot().preedit, format!("V{input}"));
        assert_eq!(words(&session), expected);
        assert_eq!(
            session.select(2).commit.as_deref(),
            expected.get(2).copied()
        );
    }
}

#[test]
fn expression_mode_keeps_an_unfinished_input_as_raw_text_and_bounds_it() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.character(b'V', true);
    type_text(&mut session, "12*");
    assert_eq!(words(&session), ["V12*"]);
    assert_eq!(
        session.snapshot().candidates[0].source,
        CandidateSource::Fallback
    );
    // The caret inserts spelling symbols too.
    session.command(Command::Cancel);
    session.character(b'V', true);
    type_text(&mut session, "12");
    session.set_caret(Some(2));
    assert!(session.character(b'+', false).handled);
    assert_eq!(session.snapshot().preedit, "V1+2");
    assert_eq!(words(&session)[0], "3");

    session.command(Command::Cancel);
    session.character(b'V', true);
    for _ in 1..crate::local::GENERATED_MODE_INPUT_LIMIT {
        assert!(session.character(b'9', false).handled);
    }
    let raw = session.snapshot().preedit;
    assert_eq!(raw.len(), crate::local::GENERATED_MODE_INPUT_LIMIT);
    let rejected = session.character(b'9', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some(raw.as_str()));
    assert!(session.snapshot().preedit.is_empty());
}

#[test]
fn expression_mode_inserts_unit_letters_at_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.character(b'V', true);
    type_text(&mut session, "3g");
    session.set_caret(Some(2));

    let result = session.character(b'k', false);

    assert!(result.handled, "{result:?}");
    assert!(result.commit.is_none(), "{result:?}");
    assert_eq!(session.snapshot().preedit, "V3kg");
    assert_eq!(session.snapshot().caret_position, 3);
    assert!(words(&session).contains(&"3000克".to_owned()));
    session.command(Command::Backspace);
    assert_eq!(session.snapshot().preedit, "V3g");
    assert_eq!(session.snapshot().caret_position, 2);
}

#[test]
fn expression_mode_inserts_a_unit_separator_at_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (text, caret, raw, expected) in [
        ("3jinkg", 5, "V3jin'kg", ["1.5千克", "1.5", "3斤=1.5千克"]),
        ("3kgjin", 4, "V3kg'jin", ["6斤", "6", "3千克=6斤"]),
        ("3gkg", 3, "V3g'kg", ["0.003千克", "0.003", "3克=0.003千克"]),
    ] {
        for punctuation in [false, true] {
            let mut session = generated_modes_session(&fixture);
            session.character(b'V', true);
            type_text(&mut session, text);
            session.set_caret(Some(caret));
            assert!(session.takes_local_separator());

            let result = if punctuation {
                session.punctuation(b'\'')
            } else {
                session.character(b'\'', false)
            };

            assert!(result.handled, "{result:?}");
            assert!(result.commit.is_none(), "{result:?}");
            assert_eq!(session.snapshot().preedit, raw);
            assert_eq!(session.snapshot().caret_position, caret + 1);
            assert_eq!(words(&session), expected);
            assert!(!session.takes_local_separator());
            assert_eq!(session.select(2).commit.as_deref(), Some(expected[2]));

            session.character(b'V', true);
            type_text(&mut session, &raw[1..]);
            session.set_caret(Some(caret + 1));
            let rejected = session.character(b'\'', false);
            assert!(!rejected.handled);
            assert_eq!(rejected.commit.as_deref(), Some(raw));
            assert!(session.snapshot().editing_text.is_empty());
        }
    }
}

#[test]
fn expression_mode_does_not_accept_a_unit_letter_before_the_number_at_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    for (text, caret) in [("3g", 1), ("-3g", 2)] {
        session.character(b'V', true);
        type_text(&mut session, text);
        session.set_caret(Some(caret));
        let before = session.snapshot();

        let result = session.character(b'k', false);

        assert!(!result.handled, "{result:?}");
        assert_eq!(result.commit.as_deref(), Some(before.preedit.as_str()));
        assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
        assert!(session.snapshot().editing_text.is_empty());
        assert!(!session.takes_local_separator());
        session.command(Command::Cancel);
    }
}

#[test]
fn expression_mode_limits_unit_insertions_at_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for value in *b"k'" {
        let mut session = generated_modes_session(&fixture);
        session.character(b'V', true);
        type_text(&mut session, "3g");
        while session.snapshot().preedit.len() < crate::local::GENERATED_MODE_INPUT_LIMIT {
            assert!(session.character(b'g', false).handled);
        }
        session.set_caret(Some(3));
        let before = session.snapshot();
        let result = session.character(value, false);
        assert!(!result.handled);
        assert_eq!(result.commit.as_deref(), Some(before.preedit.as_str()));
        assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
        assert!(session.snapshot().editing_text.is_empty());
    }
}

fn lowercase_v_session(fixture: &Fixture, lowercase_entry: bool) -> Session {
    let mut session = fixture.session_with(|options| {
        options.local_modes.expression = true;
        options.local_modes.expression_lowercase_entry = lowercase_entry;
        // 独立版关掉了全部 Shift+字母入口，小写 `v` 不经过这个总开关。
        options.local_modes.shift_letter_entry = false;
    });
    TestClock::install(&mut session);
    session
}

#[test]
fn lowercase_v_opens_the_expression_mode_in_quanpin() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = lowercase_v_session(&fixture, true);
    assert!(session.character(b'v', false).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Expression);
    assert_eq!(snapshot.preedit, "v");
    assert_eq!(words(&session), ["v"]);
    type_text(&mut session, "12*3");
    assert_eq!(session.snapshot().preedit, "v12*3");
    assert_eq!(words(&session), ["36", "12*3=36", "叁拾陆元整"]);
    assert_eq!(session.select(2).commit.as_deref(), Some("叁拾陆元整"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    // 写到一半的算式按回车原样上屏，入口字母照用户打的小写。
    session.character(b'v', false);
    type_text(&mut session, "12*");
    assert_eq!(words(&session), ["v12*"]);
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("v12*")
    );

    // 只有 `v` 时空格上屏 `v`，和原来一样。
    session.character(b'v', false);
    assert_eq!(session.select(0).commit.as_deref(), Some("v"));

    // Shift+字母总开关关着，Shift+V 仍是大写字母。
    session.character(b'V', true);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
}

#[test]
fn a_letter_after_lowercase_v_types_the_letters_as_before() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for text in ["vi", "vs", "vV", "video"] {
        let mut entry = lowercase_v_session(&fixture, true);
        let mut plain = lowercase_v_session(&fixture, false);
        for byte in text.bytes() {
            let shift = byte.is_ascii_uppercase();
            let (a, b) = (entry.character(byte, shift), plain.character(byte, shift));
            assert_eq!((a.handled, a.commit), (b.handled, b.commit), "{text:?}");
        }
        let (a, b) = (entry.snapshot(), plain.snapshot());
        assert_eq!(a.local_mode, LocalInputMode::None, "{text:?}");
        assert_eq!(a.preedit, b.preedit, "{text:?}");
        assert_eq!(words(&entry), words(&plain), "{text:?}");
    }
}

#[test]
fn lowercase_v_needs_quanpin_an_empty_composition_and_both_switches() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    // 组字里的 v 是 ü。
    let mut session = lowercase_v_session(&fixture, true);
    type_text(&mut session, "lv");
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    let mut off = lowercase_v_session(&fixture, false);
    off.character(b'v', false);
    assert_eq!(off.snapshot().local_mode, LocalInputMode::None);

    let mut expression_off = fixture.session_with(|options| {
        options.local_modes.expression_lowercase_entry = true;
    });
    expression_off.character(b'v', false);
    assert_eq!(expression_off.snapshot().local_mode, LocalInputMode::None);

    // 双拼的 v 是声母（小鹤 zh）。
    let mut shuangpin = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.local_modes.expression = true;
        options.local_modes.expression_lowercase_entry = true;
    });
    shuangpin.character(b'v', false);
    assert_eq!(shuangpin.snapshot().local_mode, LocalInputMode::None);
}

#[test]
fn slash_opens_the_command_list_and_letters_filter_it() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    assert_eq!(session.snapshot().spelling_symbols, "/@");
    assert!(session.character(b'/', false).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Command);
    assert!(snapshot.spelling_symbols.is_empty());
    assert_eq!(
        words(&session),
        [
            "张三 2026-08-09",
            "2026年8月9日",
            "14:30",
            "星期日",
            "丙午年六月二十七日",
            "2026年第3季度",
            "已过220天/剩145天",
            "已过31周/剩22周",
            "马年"
        ]
    );
    assert_eq!(
        snapshot.candidate_annotations,
        ["签名", "日期", "时间", "星期", "农历", "季度", "天数", "周", "生肖"]
    );

    type_text(&mut session, "si");
    assert_eq!(words(&session), ["张三 2026-08-09"]);
    let result = session.select(0);
    assert_eq!(result.commit.as_deref(), Some("张三 2026-08-09"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    // 内置日期触发词输入完整后，列出全部格式。
    session.character(b'/', false);
    type_text(&mut session, "rq");
    assert_eq!(words(&session).len(), 20);

    // Nothing matches: the literal text is the only row, and Enter commits it too.
    session.command(Command::Cancel);
    session.character(b'/', false);
    type_text(&mut session, "zz");
    assert_eq!(words(&session), ["/zz"]);
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("/zz")
    );
}

#[test]
fn slash_after_a_composition_is_still_the_typed_character() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    type_text(&mut session, "ni");
    assert!(!session.snapshot().spelling_symbols.contains('/'));
    assert!(!session.character(b'/', false).handled);
    // A runtime finishes the composition before it asks for the mark, so the session is empty by the time the mark is translated; it must not open the mode then.
    let finished = session.finish(0);
    assert_eq!(finished.commit.as_deref(), Some("你"));
    assert!(!session.punctuation(b'/').handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
}

#[test]
fn a_mark_on_a_bare_prefix_types_punctuation_instead_of_a_row() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.character(b'/', false);
    let result = session.punctuation(b'/');
    let commit = result.commit.expect("bare slash commits");
    assert_eq!(commit, "//");
    assert_eq!(commit.capacity(), commit.len());
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    session.character(b'/', false);
    assert_eq!(session.punctuation(b',').commit.as_deref(), Some("/，"));
    session.character(b'/', false);
    assert_eq!(session.punctuation(b'\\').commit.as_deref(), Some("/、"));
    session.character(b'@', false);
    assert_eq!(session.punctuation(b'@').commit.as_deref(), Some("@@"));
    // The character route reaches the same answer instead of swallowing the key.
    session.character(b'@', false);
    assert_eq!(
        session.character(b'.', false).commit.as_deref(),
        Some("@。")
    );
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    // 有关键词后不接受的标点先提交原文，再交还当前键。
    session.character(b'/', false);
    type_text(&mut session, "rq");
    let rejected = session.character(b',', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some("/rq"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    // Space still picks the first row and Enter the literal prefix.
    session.command(Command::Cancel);
    session.character(b'/', false);
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("张三 2026-08-09")
    );
    session.character(b'/', false);
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("/")
    );
}

#[test]
fn symbol_entries_follow_the_punctuation_mode() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.set_chinese_punctuation_enabled(false);
    assert!(session.snapshot().spelling_symbols.is_empty());
    assert!(!session.character(b'/', false).handled);
    session.set_chinese_punctuation_enabled(true);
    session.set_punctuation_lock(2).unwrap();
    assert!(!session.character(b'@', false).handled);
    session.set_punctuation_lock(0).unwrap();
    assert!(session.character(b'@', false).handled);
    // Shift+V opens the expression mode whatever the punctuation mode is.
    session.command(Command::Cancel);
    session.set_chinese_punctuation_enabled(false);
    assert!(session.character(b'V', true).handled);
}

#[test]
fn at_lists_the_mention_list_and_letters_filter_it() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    assert!(session.character(b'@', false).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::Mention);
    assert_eq!(words(&session), ["张三", "深圳市"]);
    // Rows are names, not spellings: no helpcode beside them.
    assert!(session
        .snapshot()
        .candidate_annotations
        .iter()
        .all(String::is_empty));
    type_text(&mut session, "szs");
    assert_eq!(words(&session), ["深圳市"]);
    assert_eq!(session.select(0).commit.as_deref(), Some("深圳市"));

    // The list can be replaced while the mode is open.
    session.character(b'@', false);
    session.set_mention_entries(&[crate::types::MentionEntry {
        phone: String::new(),
        text: "李四".to_owned(),
        key: "li'si".to_owned(),
    }]);
    assert_eq!(words(&session), ["李四"]);
    session.command(Command::Cancel);
    session.character(b'/', false);
    session.set_command_table(&[]);
    assert_eq!(
        words(&session),
        [
            "2026年8月9日",
            "14:30",
            "星期日",
            "丙午年六月二十七日",
            "2026年第3季度",
            "已过220天/剩145天",
            "已过31周/剩22周",
            "马年"
        ]
    );
}

#[test]
fn mention_mode_offers_places_after_the_list_when_switched_on() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.character(b'@', false);
    type_text(&mut session, "chongqing");
    assert_eq!(words(&session), ["@chongqing"]);

    // Switching on while the mode is open refreshes the list.
    assert_eq!(session.set_mention_places(true), None);
    assert_eq!(words(&session), ["重庆市"]);
    assert_eq!(session.snapshot().candidate_annotations, [""]);
    session.command(Command::Cancel);

    session.character(b'@', false);
    type_text(&mut session, "sz");
    let snapshot = session.snapshot();
    let candidates: Vec<&str> = snapshot
        .candidates
        .iter()
        .map(|candidate| candidate.word.as_str())
        .collect();
    // The user's own 深圳市 leads and is not repeated by the place of the same name.
    assert_eq!(candidates[0], "深圳市");
    assert_eq!(
        candidates.iter().filter(|text| **text == "深圳市").count(),
        1
    );
    assert_eq!(snapshot.candidate_annotations[0], "");
    let suzhou = candidates
        .iter()
        .position(|text| *text == "苏州市")
        .unwrap();
    assert_eq!(snapshot.candidate_annotations[suzhou], "江苏省");
    assert_eq!(session.select(suzhou).commit.as_deref(), Some("苏州市"));

    session.set_mention_places(false);
    session.character(b'@', false);
    type_text(&mut session, "sz");
    assert_eq!(words(&session), ["深圳市"]);
}

#[test]
fn mention_place_annotation_uses_the_shown_row_when_a_list_name_does_not_match() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.set_mention_entries(&[crate::types::MentionEntry {
        phone: String::new(),
        text: "深圳市".to_owned(),
        key: "other".to_owned(),
    }]);
    session.set_mention_places(true);
    session.character(b'@', false);
    type_text(&mut session, "shenzhenshi");

    let snapshot = session.snapshot();
    let shenzhen = snapshot
        .candidates
        .iter()
        .position(|candidate| candidate.word == "深圳市")
        .expect("embedded place should be offered");
    assert_eq!(snapshot.candidates[shenzhen].pinyin, "shen'zhen'shi");
    assert_eq!(snapshot.candidate_annotations[shenzhen], "广东省");
}

#[test]
fn generated_mode_commits_are_never_learned() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.local_modes.expression = true;
        options.local_modes.command = true;
        options.local_modes.mention = true;
        options.frequency = FrequencyAdjustmentOptions {
            mode: FrequencyAdjustmentMode::Promote,
            trigger_count: 1,
            linear_step: 1,
        };
        options.mention_entries = vec![crate::types::MentionEntry {
            phone: String::new(),
            text: "张三".to_owned(),
            key: "zhang'san".to_owned(),
        }];
    });
    TestClock::install(&mut session);
    session.character(b'V', true);
    type_text(&mut session, "123");
    assert_eq!(session.select(2).commit.as_deref(), Some("壹佰贰拾叁元整"));
    session.character(b'/', false);
    assert!(session.select(1).commit.is_some());
    session.character(b'@', false);
    assert_eq!(session.select(0).commit.as_deref(), Some("张三"));
    session.character(b'V', true);
    type_text(&mut session, "1+");
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("V1+")
    );
    assert_eq!(
        count(
            &fixture.journal(),
            "SELECT COUNT(*) FROM user_dictionary_operations"
        ),
        0
    );
}

/// 进入符号模式并打出 `typed`，返回候选。
fn symbol_mode_rows(fixture: &Fixture, typed: &str) -> Vec<String> {
    let mut session = fixture.session();
    assert!(session.character(b'S', true).handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::Symbol);
    type_text(&mut session, typed);
    session
        .snapshot()
        .candidates
        .iter()
        .map(|item| item.word.to_string())
        .collect()
}

/// 符号模式（Shift+S）的分组码 `xl` 出希腊字母。
#[test]
fn symbol_mode_group_xl_gives_greek_letters() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let rows = symbol_mode_rows(&fixture, "xl");
    assert_eq!(rows[..3], ["α", "β", "γ"], "{rows:?}");
}

/// 分组码 `sx` 出数学符号。
#[test]
fn symbol_mode_group_sx_gives_math_symbols() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let rows = symbol_mode_rows(&fixture, "sx");
    assert!(rows.iter().any(|text| text == "∑"), "{rows:?}");
    assert!(rows.iter().any(|text| text == "√"), "{rows:?}");
}

/// 分组码 `jt` 出箭头。
#[test]
fn symbol_mode_group_jt_gives_arrows() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let rows = symbol_mode_rows(&fixture, "jt");
    assert!(rows.iter().any(|text| text == "→"), "{rows:?}");
}

/// 符号名 `alpha` 出 α；符号名在分组码之后给，不重复。
#[test]
fn symbol_mode_name_alpha_gives_the_letter() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let rows = symbol_mode_rows(&fixture, "alpha");
    assert_eq!(rows[0], "α", "{rows:?}");
    assert_eq!(rows.iter().filter(|text| *text == "α").count(), 1);
}

/// 选中符号行整段上屏并结束组字；与 emoji 行一样不学习、不调频。
#[test]
fn symbol_mode_commits_the_row_and_ends_the_composition() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert!(session.character(b'S', true).handled);
    type_text(&mut session, "xl");
    // 符号模式只收字母，数字交还宿主：选中高亮那一行走 `CommitCandidate`。
    let result = session.command(Command::CommitCandidate);
    assert_eq!(result.commit.as_deref(), Some("α"), "{result:?}");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::None);
    assert!(snapshot.preedit.is_empty());
    assert_eq!(
        count(
            &fixture.journal(),
            "SELECT COUNT(*) FROM user_dictionary_operations"
        ),
        0
    );
}

/// 关掉偏好后 Shift+S 不再进入符号模式，照常打出一个大写 S（或开始拼音组字）。
#[test]
fn disabled_symbol_mode_leaves_shift_s_to_the_scheme() {
    let fixture = Fixture::new(HELLO_FIXTURE);
    // 同一份夹具下开着偏好时确实进模式，关掉后那条断言才不是空的。
    let mut on = fixture.session();
    assert!(on.character(b'S', true).handled);
    assert_eq!(on.snapshot().local_mode, LocalInputMode::Symbol);
    let mut session = fixture.session_with(|options| options.local_modes.symbol = false);
    assert!(session.character(b'S', true).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::None);
    assert_eq!(snapshot.preedit, "S");
    assert!(snapshot.candidates.iter().all(|item| item.word != "α"));
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("S")
    );
}

#[test]
fn local_modes_never_ask_for_online_candidates() {
    let fixture = Fixture::new(QUANPIN_FIXTURE).with_english(ENGLISH_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    // The same session does ask while composing pinyin, so the checks below are not vacuous.
    type_text(&mut session, "ni");
    assert!(session.online_query().is_some());
    session.command(Command::Cancel);
    for (entry, shift, input) in [
        (b'U', true, "4e2d"),
        (b'T', true, "rq"),
        (b'K', true, "ab"),
        (b'E', true, "xl"),
        (b'M', true, "hx"),
        (b'J', true, "nh"),
        (b'Y', true, "hello"),
        (b'V', true, "1+2"),
        (b'V', true, "3jin'g"),
        (b'/', false, "si"),
        (b'@', false, "zs"),
        // The translate trigger means nothing outside `/`.
        (b'@', false, "fyhello"),
        (b'Y', true, "fyhello"),
        (b'E', true, "fyhello"),
        (b'/', false, "fyhello'world"),
    ] {
        assert!(session.character(entry, shift).handled, "{}", entry as char);
        let mode = session.snapshot().local_mode;
        assert_ne!(mode, LocalInputMode::None, "{}", entry as char);
        assert!(session.online_query().is_none(), "{mode:?} on entry");
        assert!(
            session.command_translation_query().is_none(),
            "{mode:?} on entry"
        );
        for byte in input.bytes() {
            session.character(byte, false);
            assert!(session.online_query().is_none(), "{mode:?} after {input:?}");
            // `/fy` is the one local input with a request of its own: a translation, never cloud or AI candidates.
            let translation = session.command_translation_query();
            let typed = session.snapshot().preedit;
            assert_eq!(
                translation.is_some(),
                mode == LocalInputMode::Command && typed.len() > "/fy".len(),
                "{mode:?} at {typed:?}"
            );
        }
        session.command(Command::Cancel);
    }
    // 网址模式从组字进入，不在上面的入口键里。
    type_text(&mut session, "www");
    assert!(session.punctuation(b'.').handled);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::Url);
    assert!(session.online_query().is_none(), "Url on entry");
    type_text(&mut session, "github");
    assert!(session.online_query().is_none(), "Url after typing");
    assert!(session.command_translation_query().is_none());
}

#[test]
fn translate_command_inserts_word_separators_at_the_caret() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (text, caret, expected) in [
        ("fyhelloworld", 8, "hello world"),
        ("fanyihelloworld", 11, "hello world"),
        ("translatehelloworld", 15, "hello world"),
        ("fyab", 4, "a b"),
        ("fyhelloworld'", 8, "hello world"),
    ] {
        for punctuation in [false, true] {
            let mut session = generated_modes_session(&fixture);
            session.character(b'/', false);
            type_text(&mut session, text);
            session.set_caret(Some(caret));
            assert!(session.takes_local_separator(), "{text} at {caret}");

            let result = if punctuation {
                session.punctuation(b'\'')
            } else {
                session.character(b'\'', false)
            };

            assert!(result.handled, "{result:?}");
            assert!(result.commit.is_none(), "{result:?}");
            let mut raw = format!("/{text}");
            raw.insert(caret, '\'');
            assert_eq!(session.snapshot().preedit, raw);
            assert_eq!(session.snapshot().caret_position, caret + 1);
            assert_eq!(session.command_translation_query().unwrap().text, expected);
            assert_eq!(words(&session), [expected]);
            session.command(Command::Backspace);
            assert_eq!(session.snapshot().preedit, format!("/{text}"));
            assert_eq!(session.snapshot().caret_position, caret);
        }
    }
}

#[test]
fn translate_command_checks_the_caret_before_accepting_a_separator() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (text, caret) in [
        ("fyhello", 1),
        ("fyhello", 2),
        ("fyhello", 3),
        ("fanyihello", 6),
        ("translatehello", 10),
        ("fyhello'world", 9),
    ] {
        let mut session = generated_modes_session(&fixture);
        session.character(b'/', false);
        type_text(&mut session, text);
        session.set_caret(Some(caret));
        assert!(!session.takes_local_separator(), "{text} at {caret}");
        let before = session.snapshot();
        let rejected = session.character(b'\'', false);
        assert!(!rejected.handled);
        assert_eq!(rejected.commit.as_deref(), Some(before.preedit.as_str()));
        assert!(session.snapshot().editing_text.is_empty());
    }
}

#[test]
fn translate_command_preserves_separator_insertion_limits() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for punctuation in [false, true] {
        let mut session = generated_modes_session(&fixture);
        session.character(b'/', false);
        type_text(&mut session, "fyhello'world");
        session.set_caret(Some(8));
        let before = session.snapshot();
        let result = if punctuation {
            session.punctuation(b'\'')
        } else {
            session.character(b'\'', false)
        };
        assert!(result.handled && result.commit.is_none(), "{result:?}");
        assert_eq!(session.snapshot(), before);

        session.command(Command::Cancel);
        session.character(b'/', false);
        type_text(&mut session, "fyhello");
        while session.snapshot().preedit.len() < crate::local::GENERATED_MODE_INPUT_LIMIT {
            assert!(session.character(b'a', false).handled);
        }
        session.set_caret(Some(8));
        let before = session.snapshot();
        let rejected = session.character(b'\'', false);
        assert!(!rejected.handled);
        assert_eq!(rejected.commit.as_deref(), Some(before.preedit.as_str()));
        assert!(session.snapshot().editing_text.is_empty());
    }

    for (entry, text, caret) in [(b'@', "fyhello", 4), (b'/', "sighello", 5)] {
        let mut session = generated_modes_session(&fixture);
        session.character(entry, false);
        type_text(&mut session, text);
        session.set_caret(Some(caret));
        assert!(!session.takes_local_separator());
        let before = session.snapshot();
        let rejected = session.character(b'\'', false);
        assert!(!rejected.handled);
        assert_eq!(rejected.commit.as_deref(), Some(before.preedit.as_str()));
        assert!(session.snapshot().editing_text.is_empty());
    }
}

#[test]
fn the_translate_command_asks_for_its_english_and_commits_the_answer() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = generated_modes_session(&fixture);
    session.character(b'/', false);
    type_text(&mut session, "fyhello");
    // 宿主当标点送来的撇号同样能分隔单词；连续撇号则提交原文并退出。
    let apostrophe = session.punctuation(b'\'');
    assert!(
        apostrophe.handled && apostrophe.commit.is_none(),
        "{apostrophe:?}"
    );
    let rejected = session.character(b'\'', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some("/fyhello'"));
    assert!(session.command_translation_query().is_none());
    session.character(b'/', false);
    type_text(&mut session, "fyhello'world");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "/fyhello'world");
    assert_eq!(words(&session), ["hello world"]);
    assert_eq!(snapshot.candidate_annotations, ["翻译"]);
    let query = session
        .command_translation_query()
        .expect("a translation request");
    assert_eq!(query.text, "hello world");

    // Answers that do not fit a row, or that belong to another session or another text, are refused.
    for translation in ["", "  ", "你好\n世界", &"字".repeat(200)] {
        assert!(
            !session.apply_command_translation(&query, translation),
            "{translation:?}"
        );
    }
    let mut other_session = query.clone();
    other_session.session_id += 1;
    assert!(!session.apply_command_translation(&other_session, "你好世界"));
    let mut other_text = query.clone();
    other_text.text = "hello".to_owned();
    assert!(!session.apply_command_translation(&other_text, "你好"));
    assert_eq!(words(&session), ["hello world"]);

    assert!(session.apply_command_translation(&query, "你好世界"));
    assert_eq!(words(&session), ["你好世界", "hello world"]);
    assert_eq!(session.snapshot().candidate_annotations, ["翻译", "翻译"]);
    // The same answer twice adds nothing.
    assert!(!session.apply_command_translation(&query, "你好世界"));
    assert_eq!(session.select(0).commit.as_deref(), Some("你好世界"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    // The mode is gone, and with it the request.
    assert!(!session.apply_command_translation(&query, "你好世界"));

    // A late answer for text since edited is refused.
    session.character(b'/', false);
    type_text(&mut session, "fyhello");
    let query = session
        .command_translation_query()
        .expect("a translation request");
    session.character(b'x', false);
    assert!(!session.apply_command_translation(&query, "你好"));
    assert_eq!(words(&session), ["hellox"]);
}

#[test]
fn pinyin_candidate_refresh_reuses_preedit_storage() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let preedit_pointer = session.input.engine.preedit().as_ptr();

    session.input.engine.handle_key(SchemeKey::Requery);

    assert_eq!(session.input.engine.preedit().as_ptr(), preedit_pointer);
}

#[test]
fn pinyin_refresh_reuses_request_strings() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let request = session.input.engine.request();
    let pointers = [
        request.raw_input.as_ptr(),
        request.raw_input_with_cases.as_ptr(),
        request.normalized_input.as_ptr(),
        request.raw_segmentation.as_ptr(),
        request.normalized_segmentation.as_ptr(),
        request.segmentation.as_ptr(),
    ];
    session.input.engine.handle_key(SchemeKey::Requery);
    let request = session.input.engine.request();
    assert_eq!(
        [
            request.raw_input.as_ptr(),
            request.raw_input_with_cases.as_ptr(),
            request.normalized_input.as_ptr(),
            request.raw_segmentation.as_ptr(),
            request.normalized_segmentation.as_ptr(),
            request.segmentation.as_ptr(),
        ],
        pointers
    );
}

#[test]
fn shuangpin_refresh_reuses_request_strings() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| options.scheme = SchemeType::Shuangpin);
    type_text(&mut session, "nihc");
    let request = session.input.engine.request();
    let pointers = [
        request.raw_input.as_ptr(),
        request.raw_input_with_cases.as_ptr(),
        request.normalized_input.as_ptr(),
        request.raw_segmentation.as_ptr(),
        request.normalized_segmentation.as_ptr(),
        request.segmentation.as_ptr(),
    ];
    session.input.engine.handle_key(SchemeKey::Requery);
    let request = session.input.engine.request();
    assert_eq!(
        [
            request.raw_input.as_ptr(),
            request.raw_input_with_cases.as_ptr(),
            request.normalized_input.as_ptr(),
            request.raw_segmentation.as_ptr(),
            request.normalized_segmentation.as_ptr(),
            request.segmentation.as_ptr(),
        ],
        pointers
    );
}

/// K 模式先列数据库里的短语，再接上宿主短语表里编码匹配的行；文本重复的不再列出。短语表可以在 K 模式打开时实时替换。
#[test]
fn quick_phrase_mode_appends_the_host_table_after_the_database_rows() {
    let fixture = Fixture::new(
        "CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);\
INSERT INTO quick_parases VALUES('dh','电话',10);",
    );
    let phrase = |key: &str, text: &str| crate::types::QuickPhraseEntry {
        key: key.into(),
        text: text.into(),
    };
    let mut session = fixture.session_with(|options| {
        options.quick_phrase_table = vec![phrase("dh", "电话"), phrase("dhhm", "电话号码")];
    });
    assert!(session.character(b'K', true).handled);
    type_text(&mut session, "dh");
    assert_eq!(words(&session), ["电话", "电话号码"]);
    assert!(session
        .snapshot()
        .candidates
        .iter()
        .all(|row| row.source == CandidateSource::QuickPhrase));

    assert_eq!(
        session.set_quick_phrase_table(&[phrase("dhh", "大户号")]),
        None
    );
    assert_eq!(words(&session), ["电话", "大户号"]);
    assert_eq!(session.set_quick_phrase_table(&[]), None);
    assert_eq!(words(&session), ["电话"]);
}

/// macOS 的普通组字把编码匹配的快捷短语追加到原有候选之后，选中后直接上屏。
#[test]
fn macos_quick_phrase_candidates_join_regular_composition() {
    let fixture = Fixture::new(
        "CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);\
INSERT INTO quick_parases VALUES('dh','电话',20);",
    );
    let mut session = fixture.session_with(|options| {
        options.quick_phrase_candidates = true;
    });
    type_text(&mut session, "dh");
    assert!(words(&session).contains(&"电话".to_owned()));
    let phrase = session
        .snapshot()
        .candidates
        .iter()
        .position(|item| item.word == "电话")
        .expect("quick phrase candidate");
    assert_eq!(session.select(phrase).commit.as_deref(), Some("电话"));
}

/// 普通组字只认整个编码：K 模式按前缀列表，普通候选若也按前缀，单个字母就会带出以它开头的所有短语。数据库行和宿主短语表都一样。
#[test]
fn regular_composition_matches_quick_phrase_keys_exactly() {
    let fixture = Fixture::new(
        "CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);\
INSERT INTO quick_parases VALUES('dh','电话',20);\
INSERT INTO quick_parases VALUES('dhx','电话线',30);",
    );
    let phrase = |key: &str, text: &str| crate::types::QuickPhraseEntry {
        key: key.into(),
        text: text.into(),
    };
    let mut session = fixture.session_with(|options| {
        options.quick_phrase_candidates = true;
        options.quick_phrase_table = vec![phrase("d", "地址"), phrase("dhh", "电话号码")];
    });
    type_text(&mut session, "dh");
    let regular = words(&session);
    assert!(regular.contains(&"电话".to_owned()), "{regular:?}");
    for prefixed in ["电话线", "电话号码", "地址"] {
        assert!(!regular.contains(&prefixed.to_owned()), "{regular:?}");
    }
    session.command(Command::Cancel);

    type_text(&mut session, "d");
    let single = words(&session);
    assert!(single.contains(&"地址".to_owned()), "{single:?}");
    assert!(!single.contains(&"电话".to_owned()), "{single:?}");
}

/// 设置页的「快捷短语」开关同时管两种入口：关掉后普通候选里也不出现短语。
#[test]
fn the_quick_phrase_switch_also_keeps_phrases_out_of_regular_candidates() {
    let fixture = Fixture::new(
        "CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);\
INSERT INTO quick_parases VALUES('dh','电话',20);",
    );
    let mut session = fixture.session_with(|options| {
        options.quick_phrase_candidates = true;
        options.local_modes.quick_phrase = false;
    });
    type_text(&mut session, "dh");
    assert!(!words(&session).contains(&"电话".to_owned()));
}

/// `quick_phrase_frequency` 开着（`mixed_input.quick_phrase_frequency`，Windows `utility.quick_phrase_frequency`）：选中组内快捷短语让整组按调频规则前移，越过它选普通候选让整组后退一格。
#[test]
fn quick_phrase_frequency_moves_the_group_on_selection() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '你好', 200);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '拟好', 100);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '逆好', 90);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '泥号', 80);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '溺号', 70);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '腻号', 60);\
CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);\
INSERT INTO quick_parases VALUES('nihao','号码一',10);\
INSERT INTO quick_parases VALUES('nihao','号码二',20);",
    );
    let mut session = fixture.session_with(|options| {
        options.quick_phrase_candidates = true;
        options.quick_phrase_frequency = true;
        options.frequency = FrequencyAdjustmentOptions {
            mode: FrequencyAdjustmentMode::Promote,
            trigger_count: 1,
            linear_step: 1,
        };
        options.first_page_rows = 10;
        options.personal_context = false;
    });
    let group_at = |session: &Session| {
        session
            .snapshot()
            .candidates
            .iter()
            .position(|item| item.word == "号码一")
            .expect("quick phrase group")
    };
    type_text(&mut session, "nihao");
    let before = group_at(&session);
    assert_eq!(
        before, 7,
        "普通候选不满一页，整组接在它们之后（组内按权重排序）"
    );

    // 选中组内一条：整组按调频规则前移（Promote 把组的位置拉到第四格），选中的这条升到同码最高权重、排到组首。
    assert_eq!(session.select(before).commit.as_deref(), Some("号码一"));
    type_text(&mut session, "nihao");
    let promoted = group_at(&session);
    assert_eq!(promoted, 4, "整组前移后落在第五格");
    assert_eq!(
        words(&session)[promoted..promoted + 2],
        ["号码一", "号码二"],
        "整组一起挪"
    );

    // 越过整组选排在后面的普通候选：整组后退一格。
    session.select(7);
    type_text(&mut session, "nihao");
    assert_eq!(group_at(&session), 5, "越过组选普通候选后整组后退一格");

    // 再越过一次：组退到末尾，再往后没有普通候选可越。
    session.select(7);
    type_text(&mut session, "nihao");
    assert_eq!(group_at(&session), 6, "整组退到末尾");
    assert_eq!(words(&session)[6..], ["号码一", "号码二"]);
}

/// `quick_phrase_frequency` 关着时，整组固定在普通候选末尾：选中短语、越过它选普通候选，位置都不变。
#[test]
fn the_group_stays_at_the_end_when_quick_phrase_frequency_is_off() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '你好', 200);\
INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '拟好', 100);\
CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);\
INSERT INTO quick_parases VALUES('nihao','号码一',10);",
    );
    let mut session = fixture.session_with(|options| {
        options.quick_phrase_candidates = true;
        options.quick_phrase_frequency = false;
        options.frequency = FrequencyAdjustmentOptions {
            mode: FrequencyAdjustmentMode::Promote,
            trigger_count: 1,
            linear_step: 1,
        };
        options.personal_context = false;
    });
    let group_at = |session: &Session| {
        session
            .snapshot()
            .candidates
            .iter()
            .position(|item| item.word == "号码一")
            .expect("quick phrase group")
    };
    type_text(&mut session, "nihao");
    let position = group_at(&session);
    assert_eq!(session.select(position).commit.as_deref(), Some("号码一"));
    type_text(&mut session, "nihao");
    assert_eq!(group_at(&session), position, "关掉调频后选中短语不挪组");
    let last = session.snapshot().candidates.len() - 1;
    session.select(last);
    type_text(&mut session, "nihao");
    assert_eq!(group_at(&session), position, "关掉调频后越过组不挪组");
}

/// 小鹤双拼 `zj`（zan）的合成词库：`ordinary` 个单字读 zan，权重递减，第一个是 U+4E00；再加 `phrases` 里编码为 `zj` 的快捷短语（权重递减，组内按这个顺序排）。#9702 实测这个码有 83 个普通候选。
fn zan_fixture(ordinary: usize, phrases: &[&str]) -> Fixture {
    let mut sql = String::from("CREATE TABLE tbl_1_z(key TEXT,jp TEXT,value TEXT,weight INTEGER);");
    for index in 0..ordinary {
        let word = char::from_u32(0x4E00 + index as u32).unwrap();
        sql.push_str(&format!(
            "INSERT INTO tbl_1_z VALUES('zan','z','{word}',{});",
            10_000 - index
        ));
    }
    sql.push_str("CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);");
    for (index, phrase) in phrases.iter().enumerate() {
        sql.push_str(&format!(
            "INSERT INTO quick_parases VALUES('zj','{phrase}',{});",
            100 - index
        ));
    }
    Fixture::new(&sql)
}

/// 快捷短语组在列表里的起点。
fn quick_phrase_group_at(session: &Session) -> usize {
    session
        .snapshot()
        .candidates
        .iter()
        .position(|item| item.source == CandidateSource::QuickPhrase)
        .expect("quick phrase group")
}

fn zan_session(fixture: &Fixture, page: usize, frequency: FrequencyAdjustmentMode) -> Session {
    fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.quick_phrase_candidates = true;
        options.quick_phrase_frequency = true;
        options.frequency = FrequencyAdjustmentOptions {
            mode: frequency,
            trigger_count: 1,
            linear_step: 1,
        };
        options.first_page_rows = page;
        options.personal_context = false;
    })
}

/// #9702：普通候选有一页以上时，没有学习记录的快捷短语落在首页最后一格，而不是排到 83 个普通候选之后（每页 6 个时在第 14 页）。首选仍是普通候选，每页 5、6、9 个都一样；调频关着时也是这个默认位置。
#[test]
fn an_unlearned_quick_phrase_takes_the_last_slot_of_the_first_page() {
    let fixture = zan_fixture(83, &["「」"]);
    for frequency in [
        FrequencyAdjustmentMode::Promote,
        FrequencyAdjustmentMode::Disabled,
    ] {
        for page in [5, 6, 9] {
            let mut session = zan_session(&fixture, page, frequency);
            type_text(&mut session, "zj");
            let words = words(&session);
            assert_eq!(words.len(), 84, "{frequency:?} {page}: {words:?}");
            assert_eq!(
                quick_phrase_group_at(&session),
                page - 1,
                "{frequency:?} 每页 {page} 个"
            );
            assert_eq!(words[page - 1], "「」");
            assert_eq!(words[0], "一", "首选仍是普通候选");
            // 被挤下去的普通候选接在短语后面，相对顺序不变。
            let ordinary: Vec<&String> = words.iter().filter(|word| *word != "「」").collect();
            assert_eq!(ordinary.len(), 83);
            assert!(ordinary.iter().enumerate().all(|(index, word)| **word
                == char::from_u32(0x4E00 + index as u32).unwrap().to_string()));
        }
    }
}

/// 默认落位之后，学习规则照旧：选中短语整组前移（Promote 从首页末格拉到第 4 个普通候选之后，再选一次到第 3 个之后）；越过整组选后面的普通候选，整组最多退回首页末位，不会再退到首页之外。
#[test]
fn learning_still_moves_the_quick_phrase_group_from_its_default_slot() {
    let fixture = zan_fixture(83, &["「」"]);
    let mut session = zan_session(&fixture, 6, FrequencyAdjustmentMode::Promote);
    type_text(&mut session, "zj");
    assert_eq!(quick_phrase_group_at(&session), 5);

    assert_eq!(session.select(5).commit.as_deref(), Some("「」"));
    type_text(&mut session, "zj");
    assert_eq!(quick_phrase_group_at(&session), 4, "选中一次前移一格");
    assert_eq!(session.select(4).commit.as_deref(), Some("「」"));
    type_text(&mut session, "zj");
    assert_eq!(quick_phrase_group_at(&session), 3, "再选一次继续前移");

    // 越过整组选后面的普通候选：在首页末位时不再后退；前移过一格的组退回首页末位就停。
    let fixture = zan_fixture(83, &["「」"]);
    let mut session = zan_session(&fixture, 6, FrequencyAdjustmentMode::Promote);
    type_text(&mut session, "zj");
    session.select(20);
    type_text(&mut session, "zj");
    assert_eq!(quick_phrase_group_at(&session), 5, "默认位置已是首页末位");
    assert_eq!(session.select(5).commit.as_deref(), Some("「」"));
    type_text(&mut session, "zj");
    assert_eq!(quick_phrase_group_at(&session), 4);
    for _ in 0..2 {
        session.select(20);
        type_text(&mut session, "zj");
        assert_eq!(quick_phrase_group_at(&session), 5, "最多退回首页末位");
    }
}

/// 同码有多条短语：组内顺序不变、整组连在一起，首页最多让出一半（向下取整）的格子给它们，普通候选的前一半（向上取整）留在首页；放不下的接在下一页开头。每页只有 1 个时首页不让格子，整组紧跟首选。
#[test]
fn several_quick_phrases_share_the_tail_of_the_first_page() {
    let phrases = ["甲短语", "乙短语", "丙短语", "丁短语", "戊短语"];
    // (每页候选数, 短语条数, 组的起点)
    for (page, count, start) in [
        (6, 3, 3),
        (5, 3, 3),
        (9, 3, 6),
        (9, 5, 5),
        (6, 2, 4),
        (2, 3, 1),
        (1, 1, 1),
    ] {
        let fixture = zan_fixture(83, &phrases[..count]);
        let mut session = zan_session(&fixture, page, FrequencyAdjustmentMode::Disabled);
        type_text(&mut session, "zj");
        let words = words(&session);
        assert_eq!(
            words[start..start + count],
            phrases[..count],
            "每页 {page} 个、{count} 条短语: {words:?}"
        );
        assert_eq!(words[0], "一");
    }
}

/// 普通候选不满一页时短语照旧接在普通候选之后，不把任何普通候选挤到下一页。
#[test]
fn quick_phrases_follow_fewer_than_a_page_of_candidates() {
    let phrases = ["甲短语", "乙短语", "丙短语"];
    for (ordinary, count) in [(3, 1), (3, 3), (5, 3), (0, 1)] {
        let fixture = zan_fixture(ordinary, &phrases[..count]);
        let mut session = zan_session(&fixture, 6, FrequencyAdjustmentMode::Promote);
        type_text(&mut session, "zj");
        let words = words(&session);
        let start = words.len() - count;
        assert_eq!(
            words[start..],
            phrases[..count],
            "{ordinary} 个普通候选: {words:?}"
        );
        assert_eq!(quick_phrase_group_at(&session), start);
    }
}

/// K 模式只列快捷短语，不经过混输落位，每页候选数也不改变它的顺序。
#[test]
fn quick_phrase_mode_is_unaffected_by_the_first_page_default() {
    let fixture = zan_fixture(83, &["甲短语", "乙短语", "丙短语"]);
    for page in [1, 5, 6, 9] {
        let mut session = zan_session(&fixture, page, FrequencyAdjustmentMode::Promote);
        assert!(session.character(b'K', true).handled);
        type_text(&mut session, "zj");
        assert_eq!(
            words(&session),
            ["甲短语", "乙短语", "丙短语"],
            "每页 {page} 个"
        );
    }
}

/// 依次送入网址的各个字符：字母和数字走 `character`，符号走 `punctuation`，与宿主把符号报成标点时一样。
fn type_url(session: &mut Session, text: &str) {
    for byte in text.bytes() {
        let result = if byte.is_ascii_alphanumeric() {
            session.character(byte, byte.is_ascii_uppercase())
        } else {
            session.punctuation(byte)
        };
        assert!(
            result.handled && result.commit.is_none(),
            "{:?} of {text:?}: {result:?}",
            byte as char
        );
    }
}

#[test]
fn url_scheme_colon_in_quanpin_and_shuangpin() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for scheme in [SchemeType::Quanpin, SchemeType::Shuangpin] {
        let mut session = fixture.session_with(|options| options.scheme = scheme);
        type_text(&mut session, "https");
        assert_eq!(session.snapshot().spelling_symbols, ":@#$%&", "{scheme:?}");
        type_url(&mut session, "://github.com/a?b=1");
        assert_eq!(session.snapshot().local_mode, LocalInputMode::Url);
        let committed = session.command(Command::CommitRaw);
        assert_eq!(
            committed.commit.as_deref(),
            Some("https://github.com/a?b=1"),
            "{scheme:?}"
        );
        assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

        // `ftp` 用 `.` 和 `:` 都能进入。
        for key in *b".:" {
            type_text(&mut session, "ftp");
            assert!(session.punctuation(key).handled);
            assert_eq!(session.snapshot().preedit, format!("ftp{}", key as char));
            session.command(Command::Cancel);
        }
    }
}

#[test]
fn url_mark_outside_the_url_ends_it() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    type_url(&mut session, ".a.com");
    // 网址不收的键作为字符时交还宿主，组字不变；runtime 接着走标点路径。
    let typed = session.character(b'"', false);
    assert!(!typed.handled && typed.commit.is_none(), "{typed:?}");
    assert_eq!(session.snapshot().preedit, "www.a.com");
    let quote = session.punctuation(b'"');
    assert!(quote.handled);
    assert_eq!(quote.commit.as_deref(), Some("www.a.com\u{201c}"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);

    // 空格也不是网址的一部分。
    type_text(&mut session, "www");
    type_url(&mut session, ".a");
    assert!(!session.character(b' ', false).handled);
    assert_eq!(session.snapshot().preedit, "www.a");
}

/// 退回组字只是进入的逆操作：删掉中间的 `.` 后剩下的 `wwwexample` 不是触发词，留在网址模式。
#[test]
fn url_deleting_a_middle_dot_keeps_the_mode() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    type_url(&mut session, ".example");
    for _ in 0.."example".len() {
        assert!(session.command(Command::MoveLeft).handled);
    }
    assert_eq!(session.snapshot().caret_position, 4);
    assert!(session.command(Command::Backspace).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Url);
    assert_eq!(snapshot.preedit, "wwwexample");
}

/// 光标在中间且网址已到长度上限时，网址收的键被吞掉而不是交还宿主；网址不收的键照旧交还。
#[test]
fn url_caret_insert_at_the_limit_is_swallowed() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    assert!(session.punctuation(b'.').handled);
    while session.snapshot().preedit.len() < crate::local::url::INPUT_LIMIT {
        assert!(session.character(b'a', false).handled);
    }
    assert!(session.command(Command::MoveLeft).handled);
    let before = session.snapshot().preedit;
    let digit = session.character(b'1', false);
    assert!(digit.handled && digit.commit.is_none(), "{digit:?}");
    assert!(session.punctuation(b'/').handled);
    assert_eq!(session.snapshot().preedit, before);
    assert!(!session.character(b'|', false).handled);
}

#[test]
fn url_backspace_keeps_the_mode_when_the_rest_is_not_lowercase_letters() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    type_url(&mut session, ".A.");
    assert!(session.command(Command::Backspace).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Url);
    assert_eq!(snapshot.preedit, "www.A");
}

#[test]
fn url_digits_and_uppercase_are_kept() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    assert!(session.punctuation(b'.').handled);
    assert!(session.character(b'1', false).handled);
    assert!(session.character(b'G', true).handled);
    assert_eq!(session.snapshot().preedit, "www.1G");
    // Shift+数字行的符号全是输入，不选候选。
    type_url(&mut session, "!@#$%^&*()");
    assert_eq!(session.snapshot().preedit, "www.1G!@#$%^&*()");
    // 数字作为选候选键时同样是输入。
    assert!(session.character(b'2', false).handled);
    assert_eq!(session.snapshot().preedit, "www.1G!@#$%^&*()2");
}

#[test]
fn url_input_commits_at_the_limit_and_returns_the_overflow_key() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    assert!(session.punctuation(b'.').handled);
    for _ in 4..crate::local::url::INPUT_LIMIT {
        assert!(session.character(b'a', false).handled);
    }
    let raw = session.snapshot().preedit;
    assert_eq!(raw.len(), crate::local::url::INPUT_LIMIT);
    let rejected = session.character(b'a', false);
    assert!(!rejected.handled);
    assert_eq!(rejected.commit.as_deref(), Some(raw.as_str()));
    assert!(session.snapshot().preedit.is_empty());
}

/// 网址模式里补全了邮箱后缀或顶级域名的行：整段网址那一行之后的全部行。
fn url_completions(session: &Session) -> Vec<String> {
    words(session).split_off(1)
}

#[test]
fn url_at_after_a_local_part_offers_email_suffixes() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "zhangsan");
    assert_eq!(session.snapshot().spelling_symbols, "@#$%&");
    let at = session.punctuation(b'@');
    assert!(at.handled && at.commit.is_none(), "{at:?}");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Url);
    assert_eq!(snapshot.preedit, "zhangsan@");
    // 打的网址留在第一行，空格和结束网址的标点上屏的仍是它；后缀按表的顺序接在后面。
    let expected: Vec<String> = crate::local::url::EMAIL_SUFFIXES
        .iter()
        .map(|suffix| format!("zhangsan{suffix}"))
        .collect();
    assert_eq!(words(&session)[0], "zhangsan@");
    assert_eq!(url_completions(&session), expected);
    assert!(snapshot
        .candidate_sources
        .iter()
        .all(|source| *source == CandidateSource::Fallback));
    assert!(snapshot.candidate_answers_key[0]);
    assert!(!snapshot.candidate_answers_key[1..].contains(&true));

    // 后缀显示着时数字照样是网址的一部分，不选候选，并按已打的域名缩小范围。
    let digit = session.character(b'1', false);
    assert!(digit.handled && digit.commit.is_none(), "{digit:?}");
    assert_eq!(
        url_completions(&session),
        [
            "zhangsan@163.com",
            "zhangsan@126.com",
            "zhangsan@139.com",
            "zhangsan@189.cn"
        ]
    );
    // 已经打成一个完整后缀时不再补全。
    type_url(&mut session, "63.com");
    assert_eq!(words(&session), ["zhangsan@163.com"]);

    // 选中后缀上屏整个邮箱并结束网址模式。
    for _ in 0..".com".len() {
        assert!(session.command(Command::Backspace).handled);
    }
    assert_eq!(words(&session), ["zhangsan@163", "zhangsan@163.com"]);
    let picked = session.select(1);
    assert_eq!(picked.commit.as_deref(), Some("zhangsan@163.com"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    assert!(session.snapshot().preedit.is_empty());

    // 空格不是网址的一部分：交还 runtime，由它上屏高亮的第一行，也就是打的网址。
    type_text(&mut session, "abc");
    assert!(session.character(b'@', true).handled);
    assert!(!session.character(b' ', false).handled);
    assert_eq!(session.select(0).commit.as_deref(), Some("abc@"));
}

#[test]
fn url_email_backspace_over_the_at_returns_to_the_composition() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    assert!(session.punctuation(b'@').handled);
    assert_eq!(session.snapshot().preedit, "nihao@");
    // 删掉 `@` 退回组字，误触发后还能选回“你好”。
    assert!(session.command(Command::Backspace).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::None);
    assert_eq!(snapshot.editing_text, "nihao");
    assert_eq!(words(&session)[0], "你好");
    assert_eq!(session.select(0).commit.as_deref(), Some("你好"));

    // 只有删掉末尾的 `@` 才是进入的逆操作：`abc@def` 删掉中间的 `@` 留在网址模式。
    type_text(&mut session, "abc");
    assert!(session.punctuation(b'@').handled);
    type_url(&mut session, "def");
    for _ in 0.."def".len() {
        assert!(session.command(Command::MoveLeft).handled);
    }
    assert!(session.command(Command::Backspace).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Url);
    assert_eq!(snapshot.preedit, "abcdef");
}

#[test]
fn url_at_needs_a_plain_local_part() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    // 带撇号的组字不是邮箱：`@` 照旧结束组字。
    type_text(&mut session, "ni'hao");
    assert!(!session.snapshot().spelling_symbols.contains('@'));
    assert!(!session.character(b'@', true).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::None);
    assert_eq!(snapshot.editing_text, "ni'hao");
}

#[test]
fn url_scheme_completions_enter_url_mode_when_picked() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "htt");
    // 第一行是词典里的词（`h` 的“好”）时补全排在它后面。
    assert_eq!(words(&session)[..3], ["好", "https://", "http://"]);
    let snapshot = session.snapshot();
    assert!(!snapshot.candidate_answers_key[1] && !snapshot.candidate_answers_key[2]);

    // 选中补全不上屏，以它为预编辑进入网址模式，接着打网址。
    let picked = session.select(1);
    assert!(picked.handled && picked.commit.is_none(), "{picked:?}");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Url);
    assert_eq!(snapshot.preedit, "https://");
    assert_eq!(snapshot.caret_position, "https://".len());
    assert_eq!(words(&session), ["https://"]);
    type_url(&mut session, "github");
    assert_eq!(
        url_completions(&session),
        [
            "https://github.com",
            "https://github.cn",
            "https://github.net",
            "https://github.org",
            "https://github.com.cn"
        ]
    );
    // 选中顶级域名同样不上屏，还可以接着打路径。
    let tld = session.select(1);
    assert!(tld.handled && tld.commit.is_none(), "{tld:?}");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::Url);
    assert_eq!(snapshot.preedit, "https://github.com");
    assert_eq!(words(&session), ["https://github.com"]);
    type_url(&mut session, "/a");
    let committed = session.select(0);
    assert_eq!(committed.commit.as_deref(), Some("https://github.com/a"));

    for (raw, completions) in [
        ("ht", &["https://", "http://"][..]),
        ("http", &["https://", "http://"]),
        ("https", &["https://"]),
        ("ww", &["www."]),
        ("www", &["www."]),
        ("ft", &["ftp://"]),
    ] {
        type_text(&mut session, raw);
        let shown = words(&session);
        assert!(
            shown
                .windows(completions.len())
                .any(|window| window == completions),
            "{raw}: {shown:?}"
        );
        session.command(Command::Cancel);
    }
    // 一个字母、别的拼音都不给。
    for raw in ["h", "w", "nihao"] {
        type_text(&mut session, raw);
        assert!(
            !words(&session)
                .iter()
                .any(|word| word.contains("://") || word == "www."),
            "{raw}"
        );
        session.command(Command::Cancel);
    }
}

#[test]
fn plain_pinyin_period_is_still_chinese() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    assert_eq!(session.punctuation(b'.').commit.as_deref(), Some("你好。"));
    // 触发词后面的其他标点照旧结束组字。
    type_text(&mut session, "www");
    let comma = session.punctuation(b',');
    assert!(comma.handled);
    assert!(comma
        .commit
        .as_deref()
        .is_some_and(|text| text.ends_with('，')));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
}

#[test]
fn url_caret_editing_reaches_the_first_character_and_an_empty_url_leaves_the_mode() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "www");
    // 删掉 `.` 时剩下的不是触发词，留在网址模式，才能删到空。
    type_url(&mut session, ".A");
    session.command(Command::MoveHome);
    assert_eq!(session.snapshot().caret_position, 0);
    // 光标在中间时插入同样按网址规则：数字、符号接受，网址不收的键交还。
    assert!(session.character(b'1', false).handled);
    assert!(session.character(b'\'', false).handled);
    assert!(session.character(b'\'', false).handled);
    assert!(!session.character(b'|', false).handled);
    assert_eq!(session.snapshot().preedit, "1''www.A");
    session.command(Command::MoveHome);
    for _ in 0..7 {
        assert!(session.command(Command::DeleteForward).handled);
    }
    assert_eq!(session.snapshot().local_mode, LocalInputMode::Url);
    assert_eq!(session.snapshot().preedit, "A");
    assert!(session.command(Command::DeleteForward).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.local_mode, LocalInputMode::None);
    assert!(snapshot.preedit.is_empty());
    assert!(snapshot.spelling_symbols.is_empty());
    // 退出后数字不再被吞掉。
    assert!(!session.character(b'1', false).handled);
}

// ---- 打拼音出符号（local/symbol_candidates.rs） ----

/// 正负（zheng'fu）排在 政府 后面；大于 供部分选择后再选符号；派（pai）可以由 pa 加辅助码 I 拼出。
const SYMBOL_FIXTURE: &str = "CREATE TABLE tbl_1_z(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_z VALUES('zheng','z','正',300),('zheng','z','政',200);\
CREATE TABLE tbl_1_f(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_f VALUES('fu','f','负',300),('fu','f','府',200);\
CREATE TABLE tbl_1_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_d VALUES('da','d','大',300);\
CREATE TABLE tbl_1_y(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_y VALUES('yu','y','于',300);\
CREATE TABLE tbl_1_p(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_p VALUES('pai','p','派',300),('pa','p','怕',300);\
INSERT INTO tbl_1_p VALUES('ping','p','平',300);\
CREATE TABLE tbl_1_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_m VALUES('mi','m','米',300);\
CREATE TABLE tbl_2_p(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_p VALUES('ping''mi','pm','平米',500);\
CREATE TABLE tbl_2_z(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_z VALUES('zheng''fu','zf','政府',1000),('zheng''fu','zf','正负',500);\
CREATE TABLE tbl_2_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_d VALUES('da''yu','dy','大于',900);\
CREATE TABLE tbl_4_z(key TEXT,jp TEXT,value TEXT,weight INTEGER);";

fn symbol_session(fixture: &Fixture, configure: impl FnOnce(&mut SessionOptions)) -> Session {
    fixture.session_with(|options| {
        options.expressive.symbol_candidates = true;
        configure(options);
    })
}

/// 列表里文字为 `symbol` 的符号行（来源 Emoji）的位置。
fn symbol_index(session: &Session, symbol: &str) -> Option<usize> {
    session
        .snapshot()
        .candidates
        .iter()
        .position(|item| item.source == CandidateSource::Emoji && item.word == symbol)
}

#[test]
fn a_whole_symbol_name_offers_its_symbol_after_the_name() {
    let fixture = Fixture::new(SYMBOL_FIXTURE);
    let mut session = symbol_session(&fixture, |_| {});
    type_text(&mut session, "zhengfu");
    assert_eq!(words(&session)[..3], ["政府", "正负", "±"]);
    assert_eq!(symbol_index(&session, "±"), Some(2));
    session.command(Command::Cancel);
    // 没打完整的名称不出。
    type_text(&mut session, "zhengf");
    assert_eq!(symbol_index(&session, "±"), None, "{:?}", words(&session));
    session.command(Command::Cancel);

    // 双拼小鹤 vgfu 归一成 zhengfu。
    let mut shuangpin = symbol_session(&fixture, |options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
    });
    type_text(&mut shuangpin, "vgfu");
    assert!(
        symbol_index(&shuangpin, "±").is_some(),
        "{:?}",
        words(&shuangpin)
    );

    // 开关关着时没有符号行。
    let mut off = fixture.session();
    type_text(&mut off, "zhengfu");
    assert_eq!(symbol_index(&off, "±"), None);
}

#[test]
fn microsoft_semicolon_spelling_offers_the_symbol_after_its_name() {
    let fixture = Fixture::new(SYMBOL_FIXTURE);
    let mut session = symbol_session(&fixture, |options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Microsoft;
    });
    type_text(&mut session, "p;mi");
    let name = words(&session)
        .iter()
        .position(|word| *word == "平米")
        .unwrap();
    assert_eq!(symbol_index(&session, "㎡"), Some(name + 1));
}

#[test]
fn a_symbol_after_a_partial_pick_makes_no_word() {
    let fixture = Fixture::new(SYMBOL_FIXTURE);
    let mut session = symbol_session(&fixture, |_| {});
    type_text(&mut session, "zhengfudayu");
    select_word(&mut session, "正负");
    assert!(!session.snapshot().editing_text.is_empty());
    flush_all();
    let before = journal_rows(&fixture.journal());
    let index = symbol_index(&session, "＞").unwrap_or_else(|| panic!("{:?}", words(&session)));
    let result = session.select(index);
    assert!(
        result
            .commit
            .as_deref()
            .is_some_and(|text| text.ends_with('＞')),
        "{result:?}"
    );
    assert!(session.snapshot().editing_text.is_empty());
    flush_all();
    assert_eq!(journal_rows(&fixture.journal()), before);
    type_text(&mut session, "zhengfudayu");
    assert!(!words(&session).iter().any(|word| word.contains('＞')));
}

// ---- 整句改字（session/conversion.rs） ----

/// 「woqubeijing」的首选整句是「我去背景」，想要的「我去北京」不在候选里。
const CONVERSION_FIXTURE: &str =
    "CREATE TABLE tbl_1_w(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_w VALUES('wo','w','我',9000);\
CREATE TABLE tbl_1_q(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_q VALUES('qu','q','去',9000);\
CREATE TABLE tbl_1_b(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_b VALUES('bei','b','被',9000);\
INSERT INTO tbl_1_b VALUES('bei','b','北',5000);\
INSERT INTO tbl_1_b VALUES('bei','b','背',4000);\
CREATE TABLE tbl_1_j(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_j VALUES('jing','j','经',9000);\
INSERT INTO tbl_1_j VALUES('jing','j','京',3000);\
INSERT INTO tbl_1_j VALUES('jing','j','景',2000);\
CREATE TABLE tbl_2_b(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_b VALUES('bei''jing','bj','背景',30000);\
INSERT INTO tbl_2_b VALUES('bei''jing','bj','北京',20000);\
CREATE TABLE tbl_2_w(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
CREATE TABLE tbl_4_w(key TEXT,jp TEXT,value TEXT,weight INTEGER);";

fn conversion_session(fixture: &Fixture) -> Session {
    let mut session = fixture.session_with(|options| options.personal_context = false);
    type_text(&mut session, "woqubeijing");
    assert_eq!(words(&session)[0], "我去背景");
    session
}

#[test]
fn the_left_key_enters_the_sentence_and_moves_by_character() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = conversion_session(&fixture);
    assert!(session.command(Command::ConversionLeft).handled);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.conversion, "我去背景");
    // 光标在最后一个字「景」前，候选替换的是从它到词尾这一段，先列当前的字。
    assert_eq!(snapshot.conversion_focus, (3, 4));
    assert_eq!(words(&session), ["景", "经", "京"]);
    // 组字原文和字母光标不动。
    assert_eq!(snapshot.editing_text, "woqubeijing");
    assert_eq!(snapshot.caret_position, 11);

    session.command(Command::ConversionLeft);
    assert_eq!(session.snapshot().conversion_focus, (2, 4));
    // 光标在词首：先是整个词，再是更短的单字。
    assert_eq!(words(&session), ["背景", "北京", "被", "北", "背"]);

    for _ in 0..5 {
        session.command(Command::ConversionLeft);
    }
    assert_eq!(session.snapshot().conversion_focus, (0, 1));
    for _ in 0..6 {
        session.command(Command::ConversionRight);
    }
    // 句末没有候选，空格（选择或 CommitCandidate）上屏整句。
    assert_eq!(session.snapshot().conversion_focus, (4, 4));
    assert!(words(&session).is_empty());
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("我去背景")
    );
    assert!(session.snapshot().conversion.is_empty());
    assert!(session.snapshot().preedit.is_empty());
}

#[test]
fn choosing_a_word_pins_it_and_commits_the_sentence() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = conversion_session(&fixture);
    session.command(Command::ConversionLeft);
    session.command(Command::ConversionLeft);
    let picked = session.select(index_of(&session, "北京"));
    assert!(picked.handled && picked.commit.is_none(), "{picked:?}");
    let snapshot = session.snapshot();
    assert_eq!(snapshot.conversion, "我去北京");
    // 选完光标移到这段后面，正好是句末。
    assert_eq!(snapshot.conversion_focus, (4, 4));

    // 回车上屏改好的汉字，不是拼音。
    let committed = session.command(Command::CommitRaw);
    assert_eq!(committed.commit.as_deref(), Some("我去北京"));
    assert_eq!(committed.diagnostic, None);
    assert!(session.snapshot().preedit.is_empty());
    // 改好的整句与选中整句候选一样存成个人词条。
    assert_eq!(
        count(
            &fixture.journal(),
            "SELECT count(*) FROM user_dictionary_operations WHERE key='wo''qu''bei''jing' AND value='我去北京' AND user_inserted=1"
        ),
        1
    );
}

#[test]
fn a_pinned_character_inside_a_word_reconverts_its_neighbours() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = conversion_session(&fixture);
    session.command(Command::ConversionLeft);
    assert!(session.select(index_of(&session, "京")).handled);
    // 「背景」被拆开，「bei」单独重新转换；光标到了句末。
    assert_eq!(session.snapshot().conversion, "我去被京");
    assert_eq!(session.snapshot().conversion_focus, (4, 4));
    session.command(Command::ConversionLeft);
    session.command(Command::ConversionLeft);
    assert_eq!(session.snapshot().conversion_focus, (2, 3));
    assert!(session.select(index_of(&session, "北")).handled);
    assert_eq!(session.snapshot().conversion, "我去北京");
    assert_eq!(session.snapshot().conversion_focus, (3, 4));
    // 标点和失去焦点一样，上屏的是改好的整句。
    let finished = session.punctuation(b',');
    assert_eq!(finished.commit.as_deref(), Some("我去北京，"));
}

#[test]
fn typing_or_escaping_leaves_the_sentence_and_keeps_the_letters() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = conversion_session(&fixture);
    session.command(Command::ConversionLeft);
    session.command(Command::ConversionLeft);
    session.select(index_of(&session, "北京"));

    // Esc 和退格回到拼音，组字和原来的候选都在，钉住的段丢掉。
    assert!(session.command(Command::Cancel).handled);
    let snapshot = session.snapshot();
    assert!(snapshot.conversion.is_empty());
    assert_eq!(snapshot.conversion_focus, (0, 0));
    assert_eq!(snapshot.editing_text, "woqubeijing");
    assert_eq!(words(&session)[0], "我去背景");
    session.command(Command::ConversionLeft);
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().editing_text, "woqubeijing");
    assert!(session.snapshot().conversion.is_empty());

    // 数字和空格不是组字的键：交回去由选择处理，不退出改字。
    session.command(Command::ConversionLeft);
    assert!(!session.character(b'1', false).handled);
    assert!(!session.character(b' ', false).handled);
    assert_eq!(session.snapshot().conversion, "我去背景");
    // 字母退出改字，接在拼音末尾。
    assert!(session.character(b'a', false).handled);
    assert!(session.snapshot().conversion.is_empty());
    assert_eq!(session.snapshot().editing_text, "woqubeijinga");
    session.command(Command::Backspace);

    // 字母光标命令（Ctrl+左右）退出改字后照常移字母光标。
    session.command(Command::ConversionLeft);
    assert!(session.command(Command::MoveLeft).handled);
    assert!(session.snapshot().conversion.is_empty());
    assert_eq!(session.snapshot().caret_position, 10);
}

#[test]
fn without_a_sentence_the_conversion_keys_move_the_letter_caret() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = fixture.session();
    // 读音不完整，首选不是一个字对一个完整音节的整句。
    type_text(&mut session, "woqub");
    assert!(session.command(Command::ConversionLeft).handled);
    let snapshot = session.snapshot();
    assert!(snapshot.conversion.is_empty());
    assert_eq!(snapshot.caret_position, 4);
    assert!(session.command(Command::ConversionRight).handled);
    assert_eq!(session.snapshot().caret_position, 5);
    session.command(Command::Cancel);

    // 右移不进入改字；单字也不进入。
    type_text(&mut session, "woqubeijing");
    session.command(Command::ConversionRight);
    assert!(session.snapshot().conversion.is_empty());
    session.command(Command::Cancel);
    type_text(&mut session, "wo");
    session.command(Command::ConversionLeft);
    assert!(session.snapshot().conversion.is_empty());
    assert_eq!(session.snapshot().caret_position, 1);
    session.command(Command::Cancel);

    // 光标在中间时首选是光标前那段的候选，不覆盖整个组字，也不进入改字。
    type_text(&mut session, "woqubeijing");
    session.command(Command::MoveLeft);
    session.command(Command::MoveLeft);
    session.command(Command::MoveLeft);
    session.command(Command::MoveLeft);
    session.command(Command::ConversionLeft);
    assert!(session.snapshot().conversion.is_empty());
}

#[test]
fn shuangpin_enters_the_same_sentence() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.personal_context = false;
    });
    // 小鹤双拼：wo qu bw(bei) jk(jing)
    type_text(&mut session, "woqubwjk");
    assert_eq!(words(&session)[0], "我去背景");
    session.command(Command::ConversionLeft);
    session.command(Command::ConversionLeft);
    session.select(index_of(&session, "北京"));
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("我去北京")
    );
}

#[test]
fn the_conversion_starts_from_the_highlighted_row() {
    let fixture = Fixture::new(CONVERSION_FIXTURE);
    let mut session = conversion_session(&fixture);
    // 另一条整句排在第二位（宿主高亮着它，比如 runtime 把它重排到了前面）。
    let mut second = session.input.mixed_candidates[0].clone();
    second.word = "我去北京".into();
    second.sentence_words = vec!["我".into(), "去".into(), "北京".into()];
    session.input.mixed_candidates.insert(1, second);
    assert!(session.conversion_left_from(1).handled);
    assert_eq!(session.snapshot().conversion, "我去北京");
    assert_eq!(session.snapshot().conversion_focus, (3, 4));
    // 已在改字里时与普通的左移相同，不再换行。
    session.conversion_left_from(0);
    assert_eq!(session.snapshot().conversion, "我去北京");
    assert_eq!(session.snapshot().conversion_focus, (2, 4));
    // 什么都没改就上屏时，上屏的是进入改字时的那一行。
    session.command(Command::Cancel);
    session.conversion_left_from(1);
    for _ in 0..4 {
        session.command(Command::ConversionRight);
    }
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("我去北京")
    );

    // 高亮的不是覆盖整个组字的句子时进不了改字，按字母光标左移。
    type_text(&mut session, "woqubeijing");
    let prefix = index_of(&session, "我");
    assert!(session.conversion_left_from(prefix).handled);
    assert!(session.snapshot().conversion.is_empty());
    assert_eq!(session.snapshot().caret_position, 10);
}

/// #6888 的 AI 回答：读音 ni 有四个本地候选。
const AI_ANSWER_FIXTURE: &str = "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('ni','n','你',100),('ni','n','拟',90),('ni','n','妮',80),('ni','n','倪',70);";

fn sources(session: &Session) -> Vec<(String, CandidateSource)> {
    session
        .snapshot()
        .candidates
        .into_iter()
        .map(|item| (item.word.into_string(), item.source))
        .collect()
}

/// 不再有 AI 占位行：等回答时列表里只有本地候选，第 2 位就是第二个本地候选。
#[test]
fn no_ai_placeholder_is_ever_inserted() {
    let fixture = Fixture::new(AI_ANSWER_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "n");
    session.character(b'i', false);
    assert!(session
        .online_query()
        .is_some_and(|query| query.ai_eligible));
    assert_eq!(
        sources(&session),
        [
            ("你".to_owned(), CandidateSource::Database),
            ("拟".to_owned(), CandidateSource::Database),
            ("妮".to_owned(), CandidateSource::Database),
            ("倪".to_owned(), CandidateSource::Database),
        ]
    );
}

/// AI 回答到达时整批接在本地候选之后，本地候选一个都不挪；第一条显示在第一页最后一格是 input-runtime 的事（#6888）。
#[test]
fn an_ai_answer_goes_after_the_local_rows_without_moving_them() {
    let fixture = Fixture::new(AI_ANSWER_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    let batch = ["智一", "智二", "智三"].map(str::to_owned);
    assert!(session.apply_online_candidates(&query, &batch, CandidateSource::AiSuggestion));
    assert_eq!(
        sources(&session),
        [
            ("你".to_owned(), CandidateSource::Database),
            ("拟".to_owned(), CandidateSource::Database),
            ("妮".to_owned(), CandidateSource::Database),
            ("倪".to_owned(), CandidateSource::Database),
            ("智一".to_owned(), CandidateSource::AiSuggestion),
            ("智二".to_owned(), CandidateSource::AiSuggestion),
            ("智三".to_owned(), CandidateSource::AiSuggestion),
        ]
    );
}

#[test]
fn an_answer_of_listed_words_changes_nothing() {
    let fixture = Fixture::new(AI_ANSWER_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    assert!(session.apply_online_candidates(
        &query,
        &["妮".to_owned()],
        CandidateSource::AiSuggestion
    ));
    assert_eq!(words(&session), ["你", "拟", "妮", "倪"]);
}

#[test]
fn a_cloud_answer_takes_slot_one_and_ai_rows_stay_last() {
    let fixture = Fixture::new(AI_ANSWER_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "ni");
    let query = session.online_query().expect("a quanpin query");
    assert!(session.apply_online_candidates(
        &query,
        &["智一".to_owned(), "智二".to_owned()],
        CandidateSource::AiSuggestion
    ));
    assert!(session.apply_online_candidate(&query, "云", CandidateSource::CloudSuggestion));
    assert_eq!(
        words(&session),
        ["你", "云", "拟", "妮", "倪", "智一", "智二"]
    );
}

#[test]
fn first_page_rows_reaches_the_dictionary_through_the_session_options() {
    // 首页行数从会话选项一路走到词库：默认 6 下 6 条精确同音词把补全词挤到首页之后，设成 8 后补全词按权重并进来。
    // 词库级测试直接调 `set_first_page_rows`，这条钉住的是 `SessionOptions` → 请求 → 引擎这段接线。
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_q(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_2_q VALUES('qi''shi','qs','其实',500);\
         INSERT INTO tbl_2_q VALUES('qi''shi','qs','歧视',400);\
         INSERT INTO tbl_2_q VALUES('qi''shi','qs','岂是',300);\
         INSERT INTO tbl_2_q VALUES('qi''shi','qs','气势',200);\
         INSERT INTO tbl_2_q VALUES('qi''shi','qs','启示',100);\
         INSERT INTO tbl_2_q VALUES('qi''shi','qs','骑士',90);\
         CREATE TABLE tbl_5_q(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_5_q VALUES('qi''shi''ye''dan''wei','qsydw','企事业单位',600);",
    );
    let mut narrow = fixture.session();
    type_text(&mut narrow, "qishi");
    let first_page = &words(&narrow)[..DEFAULT_FIRST_PAGE_ROWS];
    assert_eq!(
        first_page,
        ["其实", "歧视", "岂是", "气势", "启示", "骑士"],
        "每页 6 行时补全词占进了首页: {:?}",
        words(&narrow)
    );

    let mut wide = fixture.session_with(|options| options.first_page_rows = 8);
    type_text(&mut wide, "qishi");
    assert_eq!(
        index_of(&wide, "企事业单位"),
        1,
        "每页 8 行时补全词应按权重并入: {:?}",
        words(&wide)
    );
}

#[test]
fn a_zero_first_page_rows_reads_as_one_row() {
    // `QueryRequest` 派生 `Default`，裸 `usize` 会默认成 0，而 0 会让 `exact_rows < 首页行数` 永远不成立，
    // 与全键盘「0 当 1」的做法相反。归一只在 `FirstPageRows::new` 做一次，两个键盘都从这里过。
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_q(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_2_q VALUES('qi''shi''yuan','qsy','歧视源',100);\
         CREATE TABLE tbl_5_q(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_5_q VALUES('qi''shi''ye''dan''wei','qsydw','企事业单位',600);",
    );
    let mut zero = fixture.session_with(|options| options.first_page_rows = 0);
    type_text(&mut zero, "qishi");
    let mut one = fixture.session_with(|options| options.first_page_rows = 1);
    type_text(&mut one, "qishi");
    assert_eq!(words(&zero), words(&one), "首页行数 0 没有按 1 处理");
    assert_eq!(
        index_of(&one, "企事业单位"),
        0,
        "精确同音词一行都没有时补全词应按权重排在最前: {:?}",
        words(&one)
    );
}

// ---- 引号与方头括号样式 ----

const CORNER_HOLLOW: crate::punctuation::PunctuationMarks = crate::punctuation::PunctuationMarks {
    quotes: crate::punctuation::QuoteMarks::Corner,
    brackets: crate::punctuation::BracketMarks::Hollow,
    enumeration_comma_backslash: true,
    enumeration_comma_slash: false,
};

#[test]
fn punctuation_marks_reach_the_bare_prefix_route() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.punctuation_marks = CORNER_HOLLOW;
        options.local_modes.command = true;
    });
    session.character(b'/', false);
    assert_eq!(session.punctuation(b'\'').commit.as_deref(), Some("/「"));
    assert_eq!(session.snapshot().local_mode, LocalInputMode::None);
    assert_eq!(session.punctuation(b'\'').commit.as_deref(), Some("」"));
    assert_eq!(session.punctuation(b'[').commit.as_deref(), Some("〖"));
    // 组字后带标点也按样式出字。
    type_text(&mut session, "nihao");
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("你好『"));
}

#[test]
fn set_punctuation_marks_applies_live_without_resetting_alternation() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("“"));
    session.set_punctuation_marks(CORNER_HOLLOW);
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("』"));
    assert_eq!(session.punctuation(b'[').commit.as_deref(), Some("〖"));
    session.set_punctuation_marks(crate::punctuation::PunctuationMarks::default());
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("“"));
    assert_eq!(session.punctuation(b'[').commit.as_deref(), Some("【"));
}

#[test]
fn idle_backspace_undoes_a_deleted_quote_but_not_an_older_one() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("“"));
    assert!(!session.command(Command::Backspace).handled);
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("“"));

    assert_eq!(session.punctuation(b':').commit.as_deref(), Some("："));
    assert!(!session.command(Command::Backspace).handled);
    assert_eq!(session.punctuation(b'"').commit.as_deref(), Some("”"));
}

#[test]
fn contacts_lead_with_name_and_combination_without_learning() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (scheme, input) in [
        (SchemeType::Quanpin, "nihao"),
        (SchemeType::Quanpin, "ni'hao"),
        (SchemeType::Quanpin, "nh"),
        (SchemeType::Shuangpin, "nihc"),
    ] {
        for slot in 0..2 {
            let mut session = fixture.session_with(|options| {
                options.scheme = scheme;
                options.frequency.mode = FrequencyAdjustmentMode::Promote;
                options.mention_entries = vec![crate::types::MentionEntry {
                    text: "拟好".into(),
                    key: "ni'hao".into(),
                    phone: "202-555-0101".into(),
                }];
            });
            session.set_cloud_candidates_enabled(true);
            type_text(&mut session, input);
            let expected = ["拟好", "拟好 202-555-0101"];
            let view = session.snapshot();
            assert_eq!(&words(&session)[..2], expected, "{scheme:?}: {input}");
            assert!(view.candidate_sources[..2]
                .iter()
                .all(|source| *source == CandidateSource::Contact));
            assert!(view.candidate_answers_key[..2].iter().all(|answer| !answer));
            assert_eq!(
                words(&session)
                    .iter()
                    .filter(|word| *word == "拟好")
                    .count(),
                1
            );
            if let Some(query) = session.online_query() {
                session.apply_online_candidate(
                    &query,
                    "合成云词",
                    CandidateSource::CloudSuggestion,
                );
                assert_eq!(&words(&session)[..2], expected);
            }
            assert_eq!(session.select(slot).commit.as_deref(), Some(expected[slot]));
            assert!(session.snapshot().editing_text.is_empty());
            assert!(!fixture.journal().exists(), "联系人选择不得写入个人词库");
        }
    }
}

#[test]
fn contacts_refresh_live_and_do_not_replace_caret_prefixes() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    session.set_mention_entries(&[crate::types::MentionEntry {
        text: "拟好".into(),
        key: "ni'hao".into(),
        phone: "202-555-0102".into(),
    }]);
    assert_eq!(&words(&session)[..2], ["拟好", "拟好 202-555-0102"]);
    session.set_caret(Some(2));
    assert!(!session
        .snapshot()
        .candidate_sources
        .contains(&CandidateSource::Contact));
    session.set_caret(None);
    assert_eq!(words(&session)[0], "拟好");
    session.set_mention_entries(&[]);
    assert!(!session
        .snapshot()
        .candidate_sources
        .contains(&CandidateSource::Contact));
    assert_eq!(words(&session)[0], "你好");
}

#[test]
fn literal_symbol_keeps_pinyin_uncommitted_with_one_raw_candidate() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for scheme in [SchemeType::Quanpin, SchemeType::Shuangpin] {
        for prefix in ["C", "Something", "nihao"] {
            for symbol in *b"#$%&" {
                for punctuation_route in [false, true] {
                    let mut session = fixture.session_with(|options| {
                        options.scheme = scheme;
                        // `S` 是已启用的符号入口；本用例验证普通英文字母串，模式自身内容另测。
                        options.local_modes.symbol = false;
                    });
                    for value in prefix.bytes() {
                        assert!(session.character(value, value.is_ascii_uppercase()).handled);
                    }
                    let result = if punctuation_route {
                        session.punctuation(symbol)
                    } else {
                        session.character(symbol, false)
                    };
                    assert!(
                        result.handled && result.commit.is_none(),
                        "{prefix} {symbol} {result:?}"
                    );
                    let expected = format!("{prefix}{}", char::from(symbol));
                    assert_eq!(session.snapshot().editing_text, expected);
                    assert_eq!(words(&session), vec![expected.clone()]);
                    assert_eq!(session.select(0).commit.as_deref(), Some(expected.as_str()));
                    assert!(session.snapshot().editing_text.is_empty());
                }
            }
        }
    }
}

#[test]
fn literal_symbol_deletion_and_caret_editing_restore_pinyin() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    for (scheme, raw) in [
        (SchemeType::Quanpin, "nihao"),
        (SchemeType::Shuangpin, "nihc"),
    ] {
        let mut session = fixture.session_with(|options| options.scheme = scheme);
        type_text(&mut session, raw);
        assert!(session.character(b'#', false).handled);
        assert_eq!(words(&session), vec![format!("{raw}#")]);
        session.command(Command::Backspace);
        assert_eq!(session.snapshot().editing_text, raw);
        assert_eq!(words(&session)[0], "你好");
        session.command(Command::MoveHome);
        assert!(session.punctuation(b'#').commit.is_none());
        assert_eq!(session.snapshot().editing_text, format!("#{raw}"));
        session.command(Command::MoveHome);
        session.command(Command::DeleteForward);
        session.command(Command::MoveEnd);
        assert_eq!(words(&session)[0], "你好");
    }
}

#[test]
fn literal_symbol_stops_chinese_queries_and_does_not_learn() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "nihao");
    let query = session.online_query().unwrap();
    assert!(session.character(b'#', false).handled);
    type_text(&mut session, "nihao");
    assert_eq!(words(&session), vec!["nihao#nihao"]);
    assert!(session.online_query().is_none());
    assert!(!session.apply_online_candidate(
        &query,
        "合成旧响应",
        CandidateSource::CloudSuggestion
    ));
    let journal_before = journal_rows(&fixture.journal());
    assert!(session.snapshot().candidate_answers_key[0]);
    assert!(!session.pin(0).handled);
    assert!(!session.remove(0).handled);
    assert_eq!(session.finish(0).commit.as_deref(), Some("nihao#nihao"));
    assert_eq!(journal_rows(&fixture.journal()), journal_before);
}

// 顿号开关不能抢占词首的命令模式；组字和裸前缀提交仍按相同的标点表出字。
#[test]
fn enumeration_comma_respects_command_prefix_and_composition() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.local_modes.command = true;
        options.punctuation_marks.enumeration_comma_slash = true;
    });
    session.character(b'/', false);
    assert_eq!(session.snapshot().local_mode, LocalInputMode::Command);
    assert_eq!(session.punctuation(b',').commit.as_deref(), Some("、，"));
    type_text(&mut session, "nihao");
    assert_eq!(session.punctuation(b'/').commit.as_deref(), Some("你好、"));
    let mut disabled = fixture.session_with(|options| {
        options.local_modes.command = false;
        options.punctuation_marks.enumeration_comma_slash = true;
    });
    assert_eq!(disabled.punctuation(b'/').commit.as_deref(), Some("、"));
}

const ANSWERS_KEY_FIXTURE: &str = "\
CREATE TABLE tbl_1_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_z VALUES('zhe','z','这',900);\
CREATE TABLE tbl_1_m(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_m VALUES('me','m','么',900);\
CREATE TABLE tbl_1_f(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_f VALUES('fa','f','发',900);\
CREATE TABLE tbl_2_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_z VALUES('zhe''me','zm','这么',800);\
CREATE TABLE tbl_3_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_3_z VALUES('zhe''me''fan','zmf','这么烦',700);\
CREATE TABLE tbl_4_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_4_z VALUES('zhe''me''fa''zhan','zmfz','这么发展',700);\
CREATE TABLE tbl_6_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_6_z VALUES('zhe''me''fa''zhan''xia''qu','zmfzxq','这么发展下去',700);";

fn answers_key(session: &Session, word: &str) -> bool {
    let snapshot = session.snapshot();
    let index = snapshot
        .candidates
        .iter()
        .position(|item| item.word.as_ref() == word)
        .unwrap_or_else(|| panic!("{word} is not a candidate of {:?}", words(session)));
    snapshot.candidate_answers_key[index]
}

#[test]
fn completion_and_prefix_range_rows_do_not_answer_the_typed_key() {
    let fixture = Fixture::new(ANSWERS_KEY_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "zhemefa");
    assert!(answers_key(&session, "这么发"), "{:?}", words(&session));
    for word in ["这么发展下去", "这么发展", "这么烦"] {
        assert!(
            !answers_key(&session, word),
            "{word} in {:?}",
            words(&session)
        );
    }
}

#[test]
fn an_incomplete_final_piece_is_answered_by_its_prefix_range_row() {
    let fixture = Fixture::new(ANSWERS_KEY_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "zhemef");
    assert!(answers_key(&session, "这么烦"), "{:?}", words(&session));
}

#[test]
fn jianpin_and_ambiguous_segmentations_still_answer_the_typed_key() {
    let fixture = Fixture::new(ANSWERS_KEY_FIXTURE);
    let mut session = fixture.session();
    type_text(&mut session, "zmf");
    assert!(answers_key(&session, "这么烦"), "{:?}", words(&session));

    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_x(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_1_x VALUES('xian','x','先',900);\
CREATE TABLE tbl_2_x(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
INSERT INTO tbl_2_x VALUES('xi''an','xa','西安',800);",
    );
    let mut session = fixture.session();
    type_text(&mut session, "xian");
    assert!(answers_key(&session, "先"), "{:?}", words(&session));
    assert!(answers_key(&session, "西安"), "{:?}", words(&session));
}

#[test]
fn shurufa_short_prefix_completion_handles_uppercase_and_lowercase_boundaries() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_1_n VALUES('ni','n','你',300),('ni','n','泥',200);
         CREATE TABLE tbl_1_l(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_1_l VALUES('la','l','啦',300),('la','l','拉',200);
         CREATE TABLE tbl_1_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_1_c VALUES('cong','c','从',300);
         CREATE TABLE tbl_1_f(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_1_f VALUES('fa','f','发',300);
         CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',300);
         CREATE TABLE tbl_2_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_2_d VALUES('da''jia','dj','大家',300);
         CREATE TABLE tbl_2_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_2_c VALUES('cheng''du','cd','成都',300);",
    )
    .with_english(
        "INSERT INTO english_words VALUES
         ('claude','Claude',200),
         ('class','class',300),
         ('classic','classic',150),
         ('claim','claim',100),
         ('python','python',200),
         ('python','Python',199),
         ('pythagoras','Pythagoras',100),
         ('mac','Mac',200),
         ('machine','machine',300),
         ('machines','machines',150),
         ('conf','conf',50),
         ('conference','conference',300),
         ('configuration','configuration',200),
         ('interface','interface',300),
         ('inter','inter',100),
         ('nice','nice',300),
         ('nick','nick',200),
         ('dj','DJ',200),
         ('django','Django',150),
         ('cd','CD',200),
         ('cdn','CDN',150);",
    );

    let mut session = fixture.session_with(|options| {
        options.first_page_rows = 6;
        options.mixed_candidates = true;
        options.english = crate::types::EnglishInputOptions {
            mixed_candidates: true,
            minimum_prefix: 5,
            first_page_count: 3,
            short_prefix_completion: true,
        };
        options.local_modes.shift_letter_entry = false;
        options.local_modes.temporary_english = false;
        options.local_modes.expression = true;
        options.local_modes.expression_lowercase_entry = true;
    });

    assert!(session.character(b'C', true).handled);
    type_text(&mut session, "la");
    assert_eq!(&words(&session)[..4], ["Cla", "Class", "Claude", "Classic"],);
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("Cla")
    );
    assert!(session.character(b'C', true).handled);
    type_text(&mut session, "la");
    assert_eq!(session.select(2).commit.as_deref(), Some("Claude"));

    assert!(session.character(b'P', true).handled);
    type_text(&mut session, "yth");
    assert_eq!(&words(&session)[..3], ["Pyth", "Python", "Pythagoras"]);
    session.command(Command::Cancel);
    assert!(session.character(b'M', true).handled);
    type_text(&mut session, "ac");
    assert_eq!(&words(&session)[..3], ["Mac", "Machine", "Machines"]);
    session.command(Command::Cancel);

    assert!(session.character(b'F', true).handled);
    type_text(&mut session, "nihao");
    assert_eq!(words(&session)[0], "F你好");
    session.command(Command::Cancel);

    type_text(&mut session, "ni");
    assert!(session.character(b'C', true).handled);
    assert_eq!(&words(&session)[..3], ["niC", "niCe", "niCk"]);
    session.command(Command::Cancel);

    for (typed, first_choice) in [
        ("nihao", "你好"),
        ("dajia", "大家"),
        ("dj", "大家"),
        ("cd", "成都"),
    ] {
        session.command(Command::Cancel);
        type_text(&mut session, typed);
        assert_eq!(words(&session)[0], first_choice, "{typed}");
        assert!(
            !words(&session).contains(&"Django".to_owned())
                && !words(&session).contains(&"CDN".to_owned()),
            "{typed}: {:?}",
            words(&session)
        );
    }
    session.command(Command::Cancel);
    type_text(&mut session, "conf");
    assert_eq!(
        &words(&session)[1..4],
        ["conf", "conference", "configuration"]
    );
    assert_eq!(
        session.command(Command::CommitRaw).commit.as_deref(),
        Some("conf")
    );
}

#[test]
fn decoupled_personal_learning_overlay_protects_static_weights_recalls_rare_nouns_and_undoes_on_backspace(
) {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_y(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','一个',508964);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','一格',20030);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','一哥',15000);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','衣阁',5000);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','移格',3000);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','艺阁',2000);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','依格',1000);\
         INSERT INTO tbl_2_y VALUES('yi''ge','yg','义戈',50);",
    );
    let mut session = fixture.session_with(|options| {
        options.learning = true;
        options.decoupled_personal_learning = true;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });

    let static_weight_of = |word: &str| -> i64 {
        let conn =
            rusqlite::Connection::open(fixture.path().join(assets::MAIN_DICTIONARY)).unwrap();
        conn.query_row(
            "SELECT weight FROM tbl_2_y WHERE key='yi''ge' AND value=?1",
            [word],
            |row| row.get(0),
        )
        .unwrap()
    };

    // 1. 空格接受原首位「一个」不记录明确改选事件，静态权重保持不变。
    type_text(&mut session, "yige");
    assert_eq!(words(&session)[0], "一个");
    assert_eq!(words(&session)[7], "义戈");
    assert_eq!(session.select(0).commit.as_deref(), Some("一个"));
    assert_eq!(static_weight_of("一个"), 508_964);
    assert_eq!(static_weight_of("义戈"), 50);

    // 2. 首次明确选择冷门专名「义戈」：不击穿高频核心词「一个」，且下次同码「义戈」立即升入首屏第 2 位。
    type_text(&mut session, "yige");
    assert_eq!(
        select_word(&mut session, "义戈").commit.as_deref(),
        Some("义戈")
    );
    // 紧接退格删光刚上屏的 2 个字：确定性回滚该次学习事件！
    assert!(!session.command(Command::Backspace).handled);
    assert!(!session.command(Command::Backspace).handled);
    type_text(&mut session, "yige");
    assert_eq!(words(&session)[0], "一个");
    assert_eq!(words(&session)[7], "义戈");
    session.command(Command::Cancel);

    // 3. 再次明确选择「义戈」一次且不退格：「一个」仍守第 1 位，「义戈」首屏召回到第 2 位，且主库静态权重纹丝不动。
    type_text(&mut session, "yige");
    assert_eq!(
        select_word(&mut session, "义戈").commit.as_deref(),
        Some("义戈")
    );
    type_text(&mut session, "yige");
    assert_eq!(&words(&session)[..2], ["一个", "义戈"]);
    assert_eq!(static_weight_of("义戈"), 50);
    session.command(Command::Cancel);

    // 4. 累计 3 次明确选择「义戈」后，后验质量超过「一个」并自然升至首位；再显式置顶「一个」作为 L4 硬约束立即锁定首位。
    for _ in 0..2 {
        type_text(&mut session, "yige");
        assert_eq!(
            select_word(&mut session, "义戈").commit.as_deref(),
            Some("义戈")
        );
    }
    type_text(&mut session, "yige");
    assert_eq!(words(&session)[0], "义戈");
    assert_eq!(static_weight_of("义戈"), 50);
    // 显式置顶「一个」（此刻在第 2 位，下标 1）作为 L4 硬约束。
    let pin_idx = words(&session).iter().position(|w| w == "一个").unwrap();
    session.pin(pin_idx);
    session.command(Command::Cancel);
    type_text(&mut session, "yige");
    assert_eq!(words(&session)[0], "一个");
    // 主词库静态权重全程零改写！
    assert_eq!(static_weight_of("一个"), 508_964);
    assert_eq!(static_weight_of("义戈"), 50);

    // 5. 显式撤销接口可立刻撤销最近一次明确选词事件。
    session.command(Command::Cancel);
    type_text(&mut session, "yige");
    assert_eq!(
        select_word(&mut session, "一格").commit.as_deref(),
        Some("一格")
    );
    assert!(session.undo_last_personal_learning());
    assert!(!session.undo_last_personal_learning());
}

#[test]
fn decoupled_personal_learning_works_across_interleaved_completions_and_survives_neutral_context() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_d(key TEXT, jp TEXT, value TEXT, weight INTEGER);         INSERT INTO tbl_2_d VALUES('deng''lu','dl','登录',502000);         INSERT INTO tbl_2_d VALUES('deng''lu','dl','登陆',5005);         CREATE TABLE tbl_3_d(key TEXT, jp TEXT, value TEXT, weight INTEGER);         INSERT INTO tbl_3_d VALUES('deng''lu''ming','dlm','登录名',200000);         CREATE TABLE tbl_4_d(key TEXT, jp TEXT, value TEXT, weight INTEGER);         INSERT INTO tbl_4_d VALUES('deng''lu''shi''jian','dlsj','登录时间',400000);         CREATE TABLE tbl_3_c(key TEXT, jp TEXT, value TEXT, weight INTEGER);         INSERT INTO tbl_3_c VALUES('chu''fa''le','cfl','出发了',18000);         INSERT INTO tbl_3_c VALUES('chu''fa''le','cfl','触发了',100);",
    );
    let mut session = fixture.session_with(|options| {
        options.learning = true;
        options.decoupled_personal_learning = true;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });

    type_text(&mut session, "denglu");
    let before = words(&session);
    assert_eq!(before[0], "登录");
    let initial_pos = before.iter().position(|w| w == "登陆").unwrap();
    assert!(initial_pos >= 2, "补全词应夹在登录与登陆之间: {before:?}");
    assert_eq!(
        select_word(&mut session, "登陆").commit.as_deref(),
        Some("登陆")
    );
    type_text(&mut session, "denglu");
    assert_eq!(&words(&session)[..2], ["登录", "登陆"]);
    session.command(Command::Cancel);
    for _ in 0..2 {
        type_text(&mut session, "denglu");
        assert_eq!(
            select_word(&mut session, "登陆").commit.as_deref(),
            Some("登陆")
        );
    }
    type_text(&mut session, "denglu");
    assert_eq!(&words(&session)[..2], ["登陆", "登录"]);
    session.command(Command::Cancel);

    for _ in 0..3 {
        session.set_rescoring_context("");
        type_text(&mut session, "chufale");
        assert_eq!(
            select_word(&mut session, "触发了").commit.as_deref(),
            Some("触发了")
        );
    }
    session.set_rescoring_context("");
    type_text(&mut session, "chufale");
    assert_eq!(&words(&session)[..2], ["触发了", "出发了"]);
    session.command(Command::Cancel);

    session.set_rescoring_context("前文");
    type_text(&mut session, "chufale");
    assert_eq!(&words(&session)[..2], ["触发了", "出发了"],);
}

#[test]
fn single_char_personal_learning_keeps_leader_while_contested_words_confirm_on_space() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_l(key TEXT, jp TEXT, value TEXT, weight INTEGER);         INSERT INTO tbl_1_l VALUES('li','l','里',7329328);         INSERT INTO tbl_1_l VALUES('li','l','力',3298612);         INSERT INTO tbl_1_l VALUES('li','l','理',1903915);         INSERT INTO tbl_1_l VALUES('li','l','李',1462547);         CREATE TABLE tbl_2_c(key TEXT, jp TEXT, value TEXT, weight INTEGER);         INSERT INTO tbl_2_c VALUES('chu''fa','cf','出发',593690);         INSERT INTO tbl_2_c VALUES('chu''fa','cf','处罚',587462);         INSERT INTO tbl_2_c VALUES('chu''fa','cf','触发',550097);         INSERT INTO tbl_2_c VALUES('chu''fa','cf','除法',10000);",
    );
    let mut session = fixture.session_with(|options| {
        options.learning = true;
        options.decoupled_personal_learning = true;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });

    for _ in 0..3 {
        session.reset_context();
        session.set_rescoring_context("");
        type_text(&mut session, "li");
        assert_eq!(
            select_word(&mut session, "李").commit.as_deref(),
            Some("李")
        );
    }
    session.reset_context();
    session.set_rescoring_context("");
    type_text(&mut session, "li");
    assert_eq!(&words(&session)[..2], ["里", "李"]);
    session.command(Command::Cancel);

    for _ in 0..2 {
        session.reset_context();
        type_text(&mut session, "chufa");
        assert_eq!(
            select_word(&mut session, "除法").commit.as_deref(),
            Some("除法")
        );
    }
    for _ in 0..2 {
        session.reset_context();
        type_text(&mut session, "chufa");
        assert_eq!(words(&session)[0], "出发");
        assert_eq!(session.select(0).commit.as_deref(), Some("出发"));
    }
    session.reset_context();
    type_text(&mut session, "chufa");
    assert_eq!(
        select_word(&mut session, "除法").commit.as_deref(),
        Some("除法")
    );
    session.reset_context();
    type_text(&mut session, "chufa");
    assert_eq!(words(&session)[0], "出发");
}

#[test]
fn quanpin_composed_phrase_immediately_recalls_across_context_shift_and_weak_stubs() {
    let mut sql = String::from(
        "CREATE TABLE tbl_1_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_a VALUES('ai','a','爱',1516399);\
         CREATE TABLE tbl_1_l(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_l VALUES('li','l','里',7329328),('li','l','梨',767900);\
         CREATE TABLE tbl_2_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_2_a VALUES('ai''li','al','艾里',2),('ai''li','al','艾丽',1);\
         CREATE TABLE tbl_3_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         CREATE TABLE tbl_4_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);",
    );
    for idx in 0..25 {
        let third = (b'a' + (idx as u8 % 26)) as char;
        sql.push_str(&format!(
            "INSERT INTO tbl_3_a VALUES('ai''li''{third}','al{third}','艾词{idx}',{});",
            5000 - idx * 50
        ));
    }
    for idx in 0..20 {
        let fourth = (b'a' + (idx as u8 % 26)) as char;
        sql.push_str(&format!(
            "INSERT INTO tbl_4_a VALUES('ai''li''b''{fourth}','alb{fourth}','艾长{idx}',{});",
            4000 - idx * 50
        ));
    }
    let fixture = Fixture::new(&sql);
    let mut session = fixture.session_with(|options| {
        options.learning = true;
        options.decoupled_personal_learning = true;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });

    // 1. 首轮拼出“爱”+“梨”成词上屏。
    session.set_rescoring_context("上一句");
    type_text(&mut session, "aili");
    assert_eq!(
        select_word(&mut session, "爱").commit.as_deref(),
        Some("爱")
    );
    assert_eq!(
        select_word(&mut session, "梨").commit.as_deref(),
        Some("梨")
    );

    // 2. 上下文立即变化后重打 `aili`：必须穿透双缓存并从低权同音残词与 45 条长词补全中稳居首位。
    session.set_rescoring_context("上一句爱梨");
    type_text(&mut session, "aili");
    let recalled = words(&session);
    assert_eq!(recalled[0], "爱梨", "recalled candidates: {recalled:?}");

    let meta = session
        .neural_v2_metadata()
        .expect("neural_v2_metadata should be available");
    assert!(
        meta.candidates[0].exact_span && meta.candidates[0].user_log_prob > 0.0,
        "meta[0] = {:?}",
        meta.candidates[0]
    );
    assert_eq!(
        meta.candidates[0].stat_log_prob, None,
        "新入库用户词条不应被静态低权重拖成负惩罚"
    );

    // 3. 重新组出“爱梨”后紧接按两次 Backspace（5 秒窗口内退格删词），撤销叠加促进，“艾里”重新回到首位。
    session.command(Command::Cancel);
    type_text(&mut session, "aili");
    assert_eq!(
        select_word(&mut session, "爱").commit.as_deref(),
        Some("爱")
    );
    assert_eq!(
        select_word(&mut session, "梨").commit.as_deref(),
        Some("梨")
    );
    session.command(Command::Backspace);
    session.command(Command::Backspace);
    type_text(&mut session, "aili");
    assert_eq!(words(&session)[0], "艾里");
}

#[test]
fn shuangpin_composed_phrase_immediately_recalls_after_context_change() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_n VALUES('ni','n','你',100000);\
         CREATE TABLE tbl_1_h(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_h VALUES('hao','h','好',200000),('hao','h','号',50000);\
         CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_2_n VALUES('ni''hao','nh','拟好',2);",
    );
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.learning = true;
        options.decoupled_personal_learning = true;
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
        options.frequency.trigger_count = 1;
    });

    session.set_rescoring_context("前文");
    type_text(&mut session, "nihc");
    assert_eq!(
        select_word(&mut session, "你").commit.as_deref(),
        Some("你")
    );
    assert_eq!(
        select_word(&mut session, "号").commit.as_deref(),
        Some("号")
    );

    session.set_rescoring_context("前文你号");
    type_text(&mut session, "nihc");
    assert_eq!(words(&session)[0], "你号");
}

mod caret_suffix;
