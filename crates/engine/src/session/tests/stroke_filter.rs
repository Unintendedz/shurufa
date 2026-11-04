//! Tab 笔画筛选（`SessionOptions::tab_stroke_filter`）：Tab 进入，`hspnzx` 按第一个字的笔顺、其他字母按部件拼音首字母筛候选，Backspace 逐笔退回再退出，选词和编辑原文结束筛选。词库和笔顺都是合成的。

use super::*;

/// `dajia` 的候选：大家、打架两个词，大、打、达三个字。
const DAJIA_FIXTURE: &str = "CREATE TABLE tbl_2_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_d VALUES('da''jia','dj','大家',2000),('da''jia','dj','打架',1500);\
CREATE TABLE tbl_1_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_d VALUES('da','d','大',1000),('da','d','打',900),('da','d','达',800);\
CREATE TABLE tbl_1_j(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_j VALUES('jia','j','家',1000),('jia','j','架',900);";

/// 在 `stroke::fixture` 之上补几个字的笔顺：大 `hpn` 已经在里面，打 `hshhs`、达 `hhpnz` 也以横起笔，只有打的第二笔是竖。
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
    // 横起笔的大、打、达都在，词跟着它的第一个字走。
    type_text(&mut session, "h");
    assert_eq!(words(&session), all);
    type_text(&mut session, "s");
    assert_eq!(words(&session), ["打架", "打"]);
    assert_eq!(session.snapshot().preedit, "dajia｜hs");
    assert_eq!(session.snapshot().editing_text, "dajia");
    // 筛不出东西的字母不收，大写字母和 Tab 也不进组字。
    for key in *b"naJ\t" {
        assert!(session.character(key, false).handled);
        assert_eq!(words(&session), ["打架", "打"]);
        assert_eq!(session.snapshot().preedit, "dajia｜hs");
    }
    // `x` 是任意一笔。
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
    // 退出后字母又是拼音。
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
    // 字母又是拼音，Tab 可以再开一次筛选。
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
    assert_eq!(
        session.snapshot().preedit,
        "dajia",
        "新的组字不带上一次的筛选"
    );
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
    assert!(
        !snapshot.spelling_symbols.contains('\t'),
        "光标不在末尾时 Tab 不归会话"
    );
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
fn shuangpin_manual_tab_helpcode_keeps_the_tab() {
    let fixture = Fixture::new(QUANPIN_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.shuangpin_helpcode_modes.manual_tab = true;
        options.tab_stroke_filter = true;
        options.stroke_dictionary = filter_dictionary(fixture.path());
    });
    type_text(&mut session, "nihc");
    assert!(tab(&mut session));
    assert_eq!(session.snapshot().editing_text, "nihc|");
    assert!(session.input.stroke_filter.is_none());
}

#[test]
fn shuangpin_without_manual_tab_filters_too() {
    let fixture = Fixture::new(DAJIA_FIXTURE);
    let mut session = fixture.session_with(|options| {
        options.scheme = SchemeType::Shuangpin;
        options.shuangpin_profile = ShuangpinProfileKind::Xiaohe;
        options.tab_stroke_filter = true;
        options.stroke_dictionary = filter_dictionary(fixture.path());
    });
    // 小鹤双拼：da = da，jia = jx。
    type_text(&mut session, "dajx");
    assert_eq!(words(&session)[..2], ["大家", "打架"]);
    assert!(tab(&mut session));
    type_text(&mut session, "hs");
    assert_eq!(words(&session), ["打架", "打"]);
    assert_eq!(session.snapshot().editing_text, "dajx");
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
    // 调频按它在整张列表里的第三名算，前移一位；按筛选后的第一名算就不会动。
    type_text(&mut session, "da");
    assert_eq!(words(&session), ["大", "达", "打"]);
}

/// `ma` 的候选：妈（女、马）、吗 和 嘛（都是口、马/麻）、码（石、马）和独体的 马。部件来自编进引擎的拆字表。
const MA_FIXTURE: &str = "CREATE TABLE tbl_1_m(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_m VALUES('ma','m','妈',1000),('ma','m','吗',900),('ma','m','马',800),('ma','m','嘛',700),('ma','m','码',600);";

#[test]
fn letters_match_the_initials_of_the_components() {
    let fixture = Fixture::new(MA_FIXTURE);
    let mut session = filter_session(&fixture);
    type_text(&mut session, "ma");
    assert_eq!(words(&session), ["妈", "吗", "马", "嘛", "码"]);
    assert!(tab(&mut session));
    // 口 读 kou。
    type_text(&mut session, "k");
    assert_eq!(words(&session), ["吗", "嘛"]);
    assert_eq!(session.snapshot().preedit, "ma｜k");
    type_text(&mut session, "m");
    assert_eq!(words(&session), ["吗", "嘛"]);
    // 两个口的 骂 不在候选里，`kk` 筛不出东西，不收。
    type_text(&mut session, "k");
    assert_eq!(session.snapshot().preedit, "ma｜km");
    session.command(Command::Backspace);
    session.command(Command::Backspace);
    // 女 读 nv：`n` 也是点，笔画库里这几个字都没有笔顺，只按部件留下 妈。
    type_text(&mut session, "n");
    assert_eq!(words(&session), ["妈"]);
    session.command(Command::Backspace);
    // 石 读 shi，`s` 同时是竖；`d`（点）同样读部件，码 还有 dan 的拆法。
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
    // 大 的笔顺是 `hpn`：`hpd` 一样找到它。
    type_text(&mut session, "hpd");
    assert_eq!(words(&session), ["大家", "大"]);
    assert_eq!(session.snapshot().preedit, "dajia｜hpd");
}
