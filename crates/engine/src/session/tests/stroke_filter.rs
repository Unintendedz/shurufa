//! Tab 笔画筛选（`SessionOptions::tab_stroke_filter`）：Tab 进入，`hspnzd` + `x` 按第一个字的笔顺、
//! 其余小写字母按拆字部件拼音/部首俗称筛候选，Backspace 逐笔退回再退出，选词和编辑原文结束筛选。词库和笔顺均使用合成数据。

use super::*;

fn stroke_dictionary(directory: &Path) -> PathBuf {
    let path = directory.join("msime-stroke.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE metadata(name TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;\
             CREATE TABLE syllables(syllable TEXT PRIMARY KEY) WITHOUT ROWID;\
             CREATE TABLE entries(key TEXT NOT NULL, text TEXT NOT NULL, weight INTEGER NOT NULL, PRIMARY KEY(key, text)) WITHOUT ROWID;\
             CREATE INDEX entries_by_key_weight ON entries(key, weight DESC);",
        )
        .unwrap();
    for (name, value) in [
        ("format_version", "1"),
        ("source_commit", "0000000000000000000000000000000000000000"),
        ("license", "LGPL-3.0-only"),
    ] {
        connection
            .execute("INSERT INTO metadata VALUES (?1, ?2)", (name, value))
            .unwrap();
    }
    for stroke in ['h', 's', 'p', 'n', 'z'] {
        connection
            .execute("INSERT INTO syllables VALUES (?1)", (stroke.to_string(),))
            .unwrap();
    }
    for (key, text, weight) in [
        ("h", "一", 9000_i64),
        ("hh", "二", 5000),
        ("hhh", "三", 4000),
        ("hs", "十", 4500),
        ("hsh", "土", 2000),
        ("hshh", "土", 10),
        ("hhsh", "王", 2500),
        ("hhs", "干", 1500),
        ("hpn", "大", 5500),
        ("pn", "人", 6000),
        ("szh", "口", 3500),
        ("sz", "乚", 0),
    ] {
        connection
            .execute(
                "INSERT INTO entries VALUES (?1, ?2, ?3)",
                (key, text, weight),
            )
            .unwrap();
    }
    path
}

/// `dajia` 的候选：大家、打架两个词，大、打、达三个字。
const DAJIA_FIXTURE: &str = "CREATE TABLE tbl_2_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_d VALUES('da''jia','dj','大家',2000),('da''jia','dj','打架',1500);\
CREATE TABLE tbl_1_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_d VALUES('da','d','大',1000),('da','d','打',900),('da','d','达',800);\
CREATE TABLE tbl_1_j(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_j VALUES('jia','j','家',1000),('jia','j','架',900);";

/// 补几个字的合成笔顺：大 `hpn` 已在内，打 `hshhs`、达 `hhpnz` 也以横起笔，只有打的第二笔是竖。
fn filter_dictionary(directory: &Path) -> PathBuf {
    let path = stroke_dictionary(directory);
    Connection::open(&path)
        .and_then(|connection| {
            connection.execute_batch(
                "INSERT INTO entries VALUES('hshhs','打',3000),('hhpnz','达',2000),('pnzhh','家',2500);",
            )
        })
        .expect("extra strokes");
    path
}

fn filter_session(fixture: &Fixture) -> Session {
    fixture.session_with(|options| {
        options.tab_stroke_filter = true;
        options.stroke_dictionary = filter_dictionary(fixture.path());
    })
}

fn tab(session: &mut Session) -> bool {
    session.character(b'\t', false).handled
}

#[test]
fn tab_enters_the_filter_and_strokes_narrow_by_the_first_character() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = filter_session(&fixture);
    assert!(!session.snapshot().spelling_symbols.contains('\t'));
    assert!(!tab(&mut session), "空闲时 Tab 归宿主");
    type_text(&mut session, "dajia");
    let all = words(&session);
    assert_eq!(all, ["大家", "打架", "大", "打", "达"]);
    assert!(session.snapshot().spelling_symbols.contains('\t'));
    assert!(tab(&mut session));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "dajia｜");
    assert_eq!(snapshot.editing_text, "dajia");
    assert_eq!(snapshot.caret_position, 5);
    assert_eq!(words(&session), all);
    type_text(&mut session, "h");
    assert_eq!(words(&session), all);
    type_text(&mut session, "s");
    assert_eq!(words(&session), ["打架", "打"]);
    assert_eq!(session.snapshot().preedit, "dajia｜hs");
    assert_eq!(session.snapshot().editing_text, "dajia");
    for key in *b"naJ\t" {
        assert!(session.character(key, false).handled);
        assert_eq!(words(&session), ["打架", "打"]);
        assert_eq!(session.snapshot().preedit, "dajia｜hs");
    }
    type_text(&mut session, "x");
    assert_eq!(words(&session), ["打架", "打"]);
    assert_eq!(session.snapshot().preedit, "dajia｜hsx");
    assert!(session.snapshot().spelling_symbols.contains('\t'));
}

#[test]
fn backspace_removes_strokes_then_leaves_the_filter_then_edits_the_pinyin() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "dajia");
    let all = words(&session);
    assert!(tab(&mut session));
    type_text(&mut session, "hs");
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().preedit, "dajia｜h");
    assert_eq!(words(&session), all);
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().preedit, "dajia｜");
    assert!(session.command(Command::Backspace).handled);
    assert_eq!(session.snapshot().preedit, "dajia");
    assert_eq!(words(&session), all);
    session.command(Command::Backspace);
    assert_eq!(session.snapshot().editing_text, "daji");
    type_text(&mut session, "a");
    assert_eq!(session.snapshot().editing_text, "dajia");
}

#[test]
fn choosing_a_row_ends_the_filter_and_the_rest_composes_unfiltered() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "dajia");
    assert!(tab(&mut session));
    type_text(&mut session, "hs");
    let result = session.select(index_of(&session, "打"));
    assert_eq!(result.commit.as_deref(), Some("打"));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.editing_text, "jia");
    assert_eq!(snapshot.preedit, "jia");
    assert_eq!(words(&session), ["家", "架"]);
    assert!(snapshot.spelling_symbols.contains('\t'));
    assert!(tab(&mut session));
    type_text(&mut session, "p");
    assert_eq!(words(&session), ["家"]);
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("家")
    );
    assert!(!session.input.has_composition());
}

#[test]
fn space_commits_the_first_filtered_row_and_cancel_drops_everything() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "dajia");
    assert!(tab(&mut session));
    type_text(&mut session, "hs");
    assert_eq!(
        session.command(Command::CommitCandidate).commit.as_deref(),
        Some("打架")
    );
    type_text(&mut session, "dajia");
    assert_eq!(session.snapshot().preedit, "dajia");
    assert!(tab(&mut session));
    type_text(&mut session, "h");
    assert!(session.command(Command::Cancel).handled);
    assert!(!session.input.has_composition());
    assert!(session.input.stroke_filter.is_none());
}

#[test]
fn moving_the_caret_ends_the_filter() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "dajia");
    let all = words(&session);
    assert!(tab(&mut session));
    type_text(&mut session, "hs");
    session.command(Command::MoveLeft);
    let snapshot = session.snapshot();
    assert_eq!(snapshot.preedit, "dajia");
    assert_eq!(snapshot.caret_position, 4);
    assert!(!snapshot.spelling_symbols.contains('\t'));
    assert_ne!(words(&session), ["打架", "打"]);
    session.command(Command::MoveEnd);
    assert_eq!(words(&session), all);
    assert!(session.snapshot().spelling_symbols.contains('\t'));
}

#[test]
fn tab_stays_with_the_host_when_the_filter_cannot_run() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let off = fixture.session_with(|options| {
        options.stroke_dictionary = filter_dictionary(fixture.path());
    });
    let no_dictionary = fixture.session_with(|options| options.tab_stroke_filter = true);
    for mut session in [off, no_dictionary] {
        type_text(&mut session, "dajia");
        assert!(!session.snapshot().spelling_symbols.contains('\t'));
        assert!(!tab(&mut session));
        assert_eq!(session.snapshot().preedit, "dajia");
    }
}

#[test]
fn a_dictionary_that_will_not_open_reports_it_and_keeps_the_composition() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.tab_stroke_filter = true;
        options.stroke_dictionary = fixture.path().join("missing-stroke.db");
    });
    type_text(&mut session, "dajia");
    let result = session.character(b'\t', false);
    assert!(result.handled);
    assert_eq!(
        result.diagnostic.as_deref(),
        Some(crate::diagnostics::LANGUAGE_DICTIONARY_UNAVAILABLE)
    );
    assert_eq!(session.snapshot().preedit, "dajia");
    type_text(&mut session, "n");
    assert_eq!(session.snapshot().editing_text, "dajian");
}

#[test]
fn shuangpin_filters_too_and_pauses_neural_v2_metadata_while_active() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.tab_stroke_filter = true;
        options.stroke_dictionary = filter_dictionary(fixture.path());
    });
    type_text(&mut session, "dajx");
    assert_eq!(words(&session)[..2], ["大家", "打架"]);
    assert!(session.neural_v2_metadata().is_some());
    assert!(tab(&mut session));
    assert!(
        session.neural_v2_metadata().is_none(),
        "进入 Tab 筛选后不得再派发异步神经重排或注入未筛选束搜索候选"
    );
    type_text(&mut session, "hs");
    assert_eq!(words(&session), ["打架", "打"]);
    assert_eq!(session.snapshot().editing_text, "dajx");
    assert!(session.neural_v2_metadata().is_none());
    session.command(Command::Backspace);
    session.command(Command::Backspace);
    session.command(Command::Backspace);
    assert!(session.neural_v2_metadata().is_some());
}

#[test]
fn a_row_found_by_strokes_is_learned_at_its_real_rank() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.tab_stroke_filter = true;
        options.stroke_dictionary = filter_dictionary(fixture.path());
        options.frequency.mode = FrequencyAdjustmentMode::Promote;
    });
    type_text(&mut session, "da");
    assert_eq!(words(&session), ["大", "打", "达"]);
    assert!(tab(&mut session));
    type_text(&mut session, "hh");
    assert_eq!(words(&session), ["达"]);
    assert_eq!(session.select(0).commit.as_deref(), Some("达"));
    type_text(&mut session, "da");
    assert_eq!(words(&session), ["大", "达", "打"]);
}

const MA_FIXTURE: &str = "CREATE TABLE tbl_1_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_m VALUES('ma','m','妈',1000),('ma','m','吗',900),('ma','m','马',800),('ma','m','嘛',700),('ma','m','码',600);";

#[test]
fn letters_match_the_initials_of_the_components() {
    let fixture = Fixture::new(MA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "ma");
    assert_eq!(words(&session), ["妈", "吗", "马", "嘛", "码"]);
    assert!(tab(&mut session));
    type_text(&mut session, "k");
    assert_eq!(words(&session), ["吗", "嘛"]);
    assert_eq!(session.snapshot().preedit, "ma｜k");
    type_text(&mut session, "m");
    assert_eq!(words(&session), ["吗", "嘛"]);
    type_text(&mut session, "k");
    assert_eq!(session.snapshot().preedit, "ma｜km");
    session.command(Command::Backspace);
    session.command(Command::Backspace);
    type_text(&mut session, "n");
    assert_eq!(words(&session), ["妈"]);
    session.command(Command::Backspace);
    type_text(&mut session, "s");
    assert_eq!(words(&session), ["码"]);
    session.command(Command::Backspace);
    type_text(&mut session, "d");
    assert_eq!(words(&session), ["码"]);
}

#[test]
fn d_is_the_dot_stroke_like_n() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "dajia");
    assert!(tab(&mut session));
    type_text(&mut session, "hpd");
    assert_eq!(words(&session), ["大家", "大"]);
    assert_eq!(session.snapshot().preedit, "dajia｜hpd");
}

const RADICAL_USABILITY_FIXTURE: &str = "CREATE TABLE tbl_1_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_d VALUES('dao','d','到',1200),('dao','d','倒',1100),('dao','d','道',1000),('dao','d','刀',900),('dao','d','岛',800);\
CREATE TABLE tbl_1_q(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_q VALUES('qian','q','前',1000),('qian','q','钱',900),('qian','q','欠',800);\
CREATE TABLE tbl_1_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_g VALUES('gei','g','给',1000);\
CREATE TABLE tbl_1_x(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_x VALUES('xin','x','信',1000),('xin','x','心',900),('xin','x','忻',800);";

#[test]
fn radical_filter_accepts_full_pinyin_aliases_trailing_and_pure_x() {
    let fixture = Fixture::new(RADICAL_USABILITY_FIXTURE);
    let mut session = filter_session(&fixture);

    type_text(&mut session, "qian");
    assert!(tab(&mut session));
    type_text(&mut session, "jin");
    assert_eq!(words(&session), ["钱"]);
    assert_eq!(session.snapshot().preedit, "qian｜jin");
    session.command(Command::Cancel);

    type_text(&mut session, "gei");
    assert!(tab(&mut session));
    type_text(&mut session, "jiao");
    assert_eq!(words(&session), ["给"]);
    session.command(Command::Cancel);

    type_text(&mut session, "dao");
    assert!(tab(&mut session));
    type_text(&mut session, "l");
    assert_eq!(words(&session), ["到", "倒", "刀"]);
    session.command(Command::Backspace);
    type_text(&mut session, "d");
    assert_eq!(words(&session), ["到", "倒", "刀"]);
    session.command(Command::Backspace);
    type_text(&mut session, "baoshan");
    assert_eq!(words(&session), ["岛"]);
    session.command(Command::Cancel);

    let ma_fixture = Fixture::new(MA_FIXTURE);
    let mut ma_session = filter_session(&ma_fixture);
    type_text(&mut ma_session, "ma");
    assert!(tab(&mut ma_session));
    type_text(&mut ma_session, "ma");
    assert_eq!(words(&ma_session), ["妈", "吗", "马", "嘛", "码"]);

    type_text(&mut session, "xin");
    assert!(tab(&mut session));
    type_text(&mut session, "x");
    assert_eq!(words(&session), ["心", "忻"]);
    assert!(!words(&session).contains(&"信".to_owned()));

    let dajia_fixture = Fixture::new(DAJIA_FIXTURE);
    let mut dajia_session = filter_session(&dajia_fixture);
    type_text(&mut dajia_session, "dajia");
    assert!(tab(&mut dajia_session));
    type_text(&mut dajia_session, "hsx");
    assert_eq!(words(&dajia_session), ["打架", "打"]);
}

const LONG_SENTENCE_FILTER_FIXTURE: &str =
    "CREATE TABLE tbl_2_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_d VALUES('dao''bi','db','倒闭',2000),('dao''bi','db','倒逼',1500);\
CREATE TABLE tbl_1_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_d VALUES('dao','d','到',3000),('dao','d','倒',1000),('dao','d','佻',1);\
CREATE TABLE tbl_1_b(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_b VALUES('bi','b','闭',1000),('bi','b','逼',900),('bi','b','必',800);\
CREATE TABLE tbl_1_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_m VALUES('ma','m','吗',1000),('ma','m','妈',900),('ma','m','嘛',800);";

#[test]
fn filtered_long_sentence_caps_runner_ups_and_promotes_tail_sentences() {
    let fixture = Fixture::new(LONG_SENTENCE_FILTER_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.tab_stroke_filter = true;
        options.stroke_dictionary = filter_dictionary(fixture.path());
        options.sentence_alternatives = true;
    });
    type_text(&mut session, "daobima");
    let total_sentences_before = session
        .snapshot()
        .candidates
        .iter()
        .filter(|item| item.word.chars().count() == 3)
        .count();
    assert!(total_sentences_before > 2);
    assert!(tab(&mut session));
    type_text(&mut session, "r");
    let filtered = words(&session);
    let sentences: Vec<_> = filtered
        .iter()
        .filter(|word| word.chars().count() == 3)
        .collect();
    assert_eq!(
        sentences.len(),
        2,
        "长句筛选最多保留 2 条整句: {filtered:?}"
    );
    let phrase_pos = filtered.iter().position(|word| word == "倒闭").unwrap();
    let single_pos = filtered.iter().position(|word| word == "倒").unwrap();
    assert!(phrase_pos < single_pos && single_pos <= 4, "{filtered:?}");
    session.command(Command::Cancel);

    type_text(&mut session, "daobima");
    assert!(tab(&mut session));
    type_text(&mut session, "r");
    let make_sentence = |text: &str, runner_up: bool| {
        let mut item = crate::types::WordItem::new(
            "dao'bi'ma",
            text,
            -1000,
            CandidateSource::Generated,
            "dao'bi'ma",
        );
        item.sentence_association = true;
        item.keyboard_runner_up = runner_up;
        item
    };
    let make_dict = |pinyin: &str, text: &str| {
        crate::types::WordItem::new(pinyin, text, 1000, CandidateSource::Database, pinyin)
    };
    session.input.ranking_candidates = None;
    session.input.mixed_candidates = vec![
        make_dict("dao'bi", "倒闭"),
        make_dict("dao", "倒"),
        make_dict("dao", "佻"),
        make_sentence("倒逼吗", true),
        make_sentence("倒闭吗", true),
        make_sentence("倒逼妈", true),
    ];
    session.input.apply_stroke_filter();
    assert_eq!(words(&session), ["倒逼吗", "倒闭吗", "倒闭", "倒", "佻"]);
    assert!(session
        .input
        .mixed_candidates
        .iter()
        .take(2)
        .all(|item| !item.keyboard_runner_up));

    let two_syl_fixture = Fixture::new(LONG_SENTENCE_FILTER_FIXTURE);
    let mut two_syl = filter_session(&two_syl_fixture);
    type_text(&mut two_syl, "daobi");
    assert!(tab(&mut two_syl));
    type_text(&mut two_syl, "r");
    let make_two = |pinyin: &str, text: &str, sentence: bool, runner_up: bool| {
        let mut item = crate::types::WordItem::new(
            pinyin,
            text,
            1000,
            if sentence {
                CandidateSource::Generated
            } else {
                CandidateSource::Database
            },
            pinyin,
        );
        item.sentence_association = sentence;
        item.keyboard_runner_up = runner_up;
        item
    };
    two_syl.input.ranking_candidates = None;
    two_syl.input.mixed_candidates = vec![
        make_two("dao'bi", "倒闭", false, false),
        make_two("dao'bi", "倒逼", false, false),
        make_two("dao", "倒", false, false),
        make_two("dao", "佻", false, false),
        make_two("dao'bi", "倒必", true, true),
    ];
    two_syl.input.apply_stroke_filter();
    assert_eq!(words(&two_syl), ["倒闭", "倒逼", "倒", "佻"]);

    two_syl.input.ranking_candidates = None;
    two_syl.input.mixed_candidates = vec![
        make_two("dao", "倒", false, false),
        make_two("dao", "佻", false, false),
        make_two("dao'bi", "倒必", true, true),
    ];
    two_syl.input.apply_stroke_filter();
    assert_eq!(words(&two_syl), ["倒必", "倒", "佻"]);
    assert!(!two_syl.input.mixed_candidates[0].keyboard_runner_up);
}
