//! The port of the removed C++ bridge's `crates/engine-bridge/src/tests.rs` (tests-inventory.md §4.2) against the Rust facade, plus the two annotation tests of `engine-bridge/src/dictionary_stage/tests.rs` that exercise the host snapshot rule (fixtures written straight into the generation instead of staged), and unit tests of the bridge-local helpers.

use std::path::Path;







use rusqlite::Connection;







use super::glosses::{candidate_gloss_display, candidate_gloss_key};






use super::*;






use crate::types::SchemeType;

fn options(root: &Path) -> EngineOptions {
    let path = |name| {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.to_str().unwrap().to_owned()
    };
    EngineOptions {
        resources: path("resources"),
        user_data: path("user"),
        cache: path("cache"),
        dictionaries: path("dictionaries"),
        first_page_rows: crate::types::DEFAULT_FIRST_PAGE_ROWS,
        scheme: 0,
        enabled_schemes: crate::types::SchemeSet::ALL,
        shuangpin_profile: 0,
        shuangpin_custom_profile: None,
        shuangpin_preedit_uses_raw: true,
        single_character_only: false,
        caret_edit_suffix: false,
        learning: false,
        decoupled_personal_learning: false,
        learn_english_on_enter: true,
        next_word_prediction: false,
        autocorrect_transposition: true,
        autocorrect_neighbor: true,
        fuzzy_pinyin_rules: 0,
        chinese_punctuation: true,
        paired_punctuation: true,
        punctuation_lock: 0,
        corner_quotes: false,
        hollow_brackets: false,
        enumeration_comma_backslash: true,
        enumeration_comma_slash: false,
        frequency_mode: "promote".into(),
        frequency_trigger_count: 1,
        frequency_linear_step: 1,
        mixed_candidates: true,
        mixed_english: true,
        english_minimum_prefix: 5,
        english_first_page_count: 1,
        english_short_prefix_completion: false,
        mixed_emoji: false,
        mixed_kaomoji: false,
        mixed_symbols: false,
        local_unicode: true,
        local_date_time: true,
        local_date_time_inline: true,
        local_date_time_menu: true,
        local_quick_phrase: true,
        local_emoji: true,
        local_kaomoji: true,
        local_super_jianpin: true,
        local_temporary_english: true,
        local_url: true,
        local_expression: false,
        local_expression_lowercase_entry: false,
        local_command: false,
        local_mention: false,
        local_symbol: true,
        local_shift_letter_entry: true,
        command_table: Vec::new(),
        mention_entries: Vec::new(),
        quick_phrase_table: Vec::new(),
        quick_phrase_candidates: false,
        quick_phrase_frequency: true,
        sentence_association: SentenceAssociationOptions {
            word_lattice: true,
            show_next_on_duplicate: false,
            ..Default::default()
        },
        rescoring_context: String::new(),
        sentence_alternatives: true,
    }
}







fn type_text(session: &mut Session, text: &[u8]) {
    for character in text {
        session.character(*character, false).unwrap();
    }
}







const ENGLISH_SCHEMA: &str = "CREATE TABLE english_words(word TEXT,display TEXT,weight INTEGER);
    CREATE TABLE en_zh_glosses(english TEXT PRIMARY KEY,chinese_gloss TEXT);
    CREATE TABLE zh_en_glosses(chinese TEXT PRIMARY KEY,english_gloss TEXT);";







#[test]
fn quanpin_host_preedit_keeps_literal_and_local_modes_unsegmented() {
    let root = tempfile::tempdir().unwrap();
    let options = options(root.path());
    let mut session = Session::new(&options).unwrap();
    session.set_dedicated_english(true).unwrap();
    type_text(&mut session, b"nihao");
    assert_eq!(session.snapshot().unwrap().preedit, "nihao");
    session.command(Command::Cancel).unwrap();
    session.set_dedicated_english(false).unwrap();
    for raw in [b"Ynihao".as_slice(), b"https://example.test".as_slice()] {
        for byte in raw {
            assert!(
                session
                    .character(*byte, byte.is_ascii_uppercase())
                    .unwrap()
                    .handled
            );
        }
        let view = session.snapshot().unwrap();
        assert_eq!(view.preedit.as_bytes(), raw);
        session.command(Command::Cancel).unwrap();
    }
}







#[test]
fn quanpin_host_preedit_preserves_case_and_shuangpin_display_preference() {
    let root = tempfile::tempdir().unwrap();
    let mut options = options(root.path());
    let mut session = Session::new(&options).unwrap();
    for byte in b"niHao" {
        assert!(
            session
                .character(*byte, byte.is_ascii_uppercase())
                .unwrap()
                .handled
        );
    }
    assert_eq!(session.snapshot().unwrap().preedit, "niHao");
    assert_eq!(session.snapshot().unwrap().editing_text, "niHao");
    drop(session);
    options.scheme = SchemeType::Shuangpin as u8;
    for (raw_display, expected) in [(true, "nihc"), (false, "ni'hao")] {
        options.shuangpin_preedit_uses_raw = raw_display;
        let mut session = Session::new(&options).unwrap();
        type_text(&mut session, b"nihc");
        assert_eq!(session.snapshot().unwrap().preedit, expected);
        assert_eq!(session.snapshot().unwrap().editing_text, "nihc");
    }
}







#[test]
fn normalizes_full_pinyin_using_the_expected_word_length() {
    assert_eq!(normalize_full_pinyin("xian", 1), "xian");
    assert_eq!(normalize_full_pinyin("xian", 2), "xi'an");
    assert_eq!(
        normalize_full_pinyin("a'ba'la'ti'ya'yun'hai", 7),
        "a'ba'la'ti'ya'yun'hai"
    );
    assert_eq!(normalize_full_pinyin("xian", 3), "");
    assert_eq!(normalize_full_pinyin("ni'hao'", 2), "");
}







#[test]
fn validates_and_normalizes_personal_dictionary_entries() {
    let normalized = dictionary_validate(&DictionaryEntry {
        kind: DictionaryKind::Pinyin,
        key: "NI HAO".into(),
        value: "拟好".into(),
        weight: 100_000,
    })
    .unwrap();
    assert_eq!(normalized.key, "ni'hao");
    let english = dictionary_validate(&DictionaryEntry {
        kind: DictionaryKind::English,
        key: "dont".into(),
        value: "don't".into(),
        weight: 100_000,
    })
    .unwrap();
    assert_eq!(english.key, "dont");
    assert_eq!(english.value, "don't");
    assert!(dictionary_validate(&DictionaryEntry {
        kind: DictionaryKind::English,
        key: "wrong_code".into(),
        value: "Word".into(),
        weight: 100_000,
    })
    .is_err());
}







#[test]
fn learned_glosses_survive_unavailable_packaged_dictionary() {
    let resources = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let resources_path = resources.path().to_str().unwrap();
    let user_path = user.path().to_str().unwrap();
    let candidates = vec![
        ("测试".into(), 0),
        ("Synthetic".into(), 4),
        ("Missing".into(), 4),
    ];
    let error = candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap_err();
    assert_eq!(error.to_string(), "Candidate gloss dictionary unavailable");
    assert!(save_candidate_gloss(
        user_path,
        true,
        "测试",
        "synthetic gloss"
    ));
    assert!(save_candidate_gloss(
        user_path,
        false,
        "synthetic",
        "合成释义"
    ));
    let expected = vec![
        "synthetic gloss".to_owned(),
        "合成释义".to_owned(),
        String::new(),
    ];
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        expected
    );
    let packaged = resources.path().join("msime-english.db");
    std::fs::write(&packaged, "synthetic damaged database").unwrap();
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        expected
    );
    assert!(candidate_glosses(resources_path, &candidates).is_err());
    std::fs::remove_file(&packaged).unwrap();
    Connection::open(&packaged)
        .unwrap()
        .execute_batch(&format!(
            "{ENGLISH_SCHEMA}
            INSERT INTO en_zh_glosses VALUES('missing','发布释义');
            INSERT INTO zh_en_glosses VALUES('测试','packaged gloss');"
        ))
        .unwrap();
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        vec!["synthetic gloss", "合成释义", "发布释义"]
    );
    std::fs::write(
        user.path().join("translation-glosses.db"),
        "synthetic damaged database",
    )
    .unwrap();
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        vec!["packaged gloss", "", "发布释义"]
    );
}







// The user's own file wins through an emergent route: translation-glosses.db sits in the directory the settings page writes custom_translations.txt to, and the learned store reads its sidecar from beside itself. Moving either file, or giving the learned store an explicit translations path, would silently drop the user's glosses to the bottom; this test is what would notice.
#[test]
fn hand_written_glosses_outrank_learned_and_packaged_ones() {
    let resources = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    Connection::open(resources.path().join("msime-english.db"))
        .unwrap()
        .execute_batch(&format!(
            "{ENGLISH_SCHEMA} INSERT INTO zh_en_glosses VALUES('测试','packaged gloss');"
        ))
        .unwrap();
    let resources_path = resources.path().to_str().unwrap();
    let user_path = user.path().to_str().unwrap();
    let candidates = vec![("测试".into(), 0)];

    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        vec!["packaged gloss"]
    );
    assert!(save_candidate_gloss(
        user_path,
        true,
        "测试",
        "learned gloss"
    ));
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        vec!["learned gloss"]
    );
    std::fs::write(
        user.path().join("custom_translations.txt"),
        "测试\thand written gloss\n",
    )
    .unwrap();
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        vec!["hand written gloss"]
    );
}







/// The user's own `custom_translations.txt` outranks every automatic gloss even before any online gloss was saved, which is when `translation-glosses.db` first appears; the file Settings writes must not wait for that store.
#[test]
fn hand_written_glosses_apply_without_a_learned_store() {
    let resources = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    Connection::open(resources.path().join("msime-english.db"))
        .unwrap()
        .execute_batch(&format!(
            "{ENGLISH_SCHEMA} INSERT INTO zh_en_glosses VALUES('测试','packaged gloss');
             INSERT INTO en_zh_glosses VALUES('hello','打招呼');"
        ))
        .unwrap();
    std::fs::write(
        user.path().join("custom_translations.txt"),
        "测试\thand written gloss\nhello\t你好\n",
    )
    .unwrap();
    let resources_path = resources.path().to_str().unwrap();
    let user_path = user.path().to_str().unwrap();
    let candidates = vec![("测试".into(), 0), ("hello".into(), 4)];
    assert!(!user.path().join("translation-glosses.db").exists());
    assert_eq!(
        candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
        vec!["hand written gloss", "你好"]
    );
    // The macOS learned-gloss path asks for the user overlay alone.
    assert_eq!(
        candidate_glosses_with_user("", user_path, &candidates).unwrap(),
        vec!["hand written gloss", "你好"]
    );
    assert!(!user.path().join("translation-glosses.db").exists());
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        candidate_glosses_with_user("", empty.path().to_str().unwrap(), &candidates)
            .unwrap_err()
            .to_string(),
        crate::diagnostics::CANDIDATE_GLOSS_UNAVAILABLE
    );
}







#[cfg(unix)]
#[test]
fn learned_glosses_reject_a_symlinked_database() {
    use std::os::unix::fs::symlink;

    let resources = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    Connection::open(resources.path().join("msime-english.db"))
        .unwrap()
        .execute_batch(&format!(
            "{ENGLISH_SCHEMA} INSERT INTO zh_en_glosses VALUES('测试','packaged gloss');"
        ))
        .unwrap();
    assert!(save_candidate_gloss(
        external.path().to_str().unwrap(),
        true,
        "测试",
        "external gloss",
    ));
    symlink(
        external.path().join("translation-glosses.db"),
        user.path().join("translation-glosses.db"),
    )
    .unwrap();

    let candidates = vec![("测试".into(), 0)];
    assert_eq!(
        candidate_glosses_with_user(
            resources.path().to_str().unwrap(),
            user.path().to_str().unwrap(),
            &candidates,
        )
        .unwrap(),
        vec!["packaged gloss"]
    );
}







#[cfg(unix)]
#[test]
fn saving_learned_glosses_rejects_a_symlinked_database() {
    use std::os::unix::fs::symlink;

    let user = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("external.db");
    symlink(&external, user.path().join("translation-glosses.db")).unwrap();

    assert!(!save_candidate_gloss(
        user.path().to_str().unwrap(),
        true,
        "测试",
        "external gloss",
    ));
    assert!(!external.exists());
}







#[test]
fn unsafe_learned_glosses_fall_back_to_packaged_values() {
    let resources = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    Connection::open(resources.path().join("msime-english.db"))
        .unwrap()
        .execute_batch(&format!(
            "{ENGLISH_SCHEMA} INSERT INTO zh_en_glosses VALUES('测试','packaged gloss');"
        ))
        .unwrap();
    let resources_path = resources.path().to_str().unwrap();
    let user_path = user.path().to_str().unwrap();
    let candidates = vec![("测试".into(), 0)];

    for codepoint in (1..=0x1f).chain(0x7f..=0x9f) {
        let control = char::from_u32(codepoint).unwrap();
        if matches!(control, '\t' | '\n' | '\r') {
            continue;
        }
        assert!(save_candidate_gloss(
            user_path,
            true,
            "测试",
            &format!("before{control}after"),
        ));
        assert_eq!(
            candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
            vec!["packaged gloss"]
        );
    }
    for whitespace in ['\t', '\n', '\r'] {
        assert!(save_candidate_gloss(
            user_path,
            true,
            "测试",
            &format!("learned{whitespace}gloss"),
        ));
        assert_eq!(
            candidate_glosses_with_user(resources_path, user_path, &candidates).unwrap(),
            vec!["learned gloss"]
        );
    }
}







fn offline_gloss_database(path: &Path, language: &str, version: i32) {
    Connection::open(path)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TABLE zh_glosses(chinese TEXT PRIMARY KEY, gloss TEXT NOT NULL, source TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
             INSERT INTO meta VALUES('target_language', '{language}');
             INSERT INTO zh_glosses VALUES('猫', 'chat, chatte; félin', 'cat; cat');
             INSERT INTO zh_glosses VALUES('天', 'jour; ciel; firmament', 'day; sky; heaven');
             PRAGMA user_version = {version};"
        ))
        .unwrap();
}







#[test]
fn offline_target_glosses_answer_chinese_candidates_only() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("zh-fr.db");
    offline_gloss_database(&path, "fr", 1);
    let path = path.to_str().unwrap();
    let candidates = vec![
        ("猫".into(), 0),
        ("天".into(), 0),
        ("狗".into(), 0),
        ("cat".into(), 4),
        ("猫".into(), 6),
    ];
    // A third sense is cut by the same display rule as the English glosses; Latin candidates and emoji sources get nothing.
    assert_eq!(
        candidate_target_glosses(path, "fr", &candidates).unwrap(),
        vec!["chat, chatte; félin", "jour; ciel", "", "", ""]
    );
}







#[test]
fn offline_target_glosses_refuse_a_file_for_another_language_or_version() {
    let directory = tempfile::tempdir().unwrap();
    let candidates = vec![("猫".into(), 0)];
    let message = |path: &Path, language| {
        candidate_target_glosses(path.to_str().unwrap(), language, &candidates)
            .unwrap_err()
            .to_string()
    };
    let renamed = directory.path().join("zh-ja.db");
    offline_gloss_database(&renamed, "fr", 1);
    assert_eq!(
        message(&renamed, "ja"),
        "Offline gloss dictionary language mismatch"
    );
    let future = directory.path().join("zh-de.db");
    offline_gloss_database(&future, "de", 2);
    assert_eq!(
        message(&future, "de"),
        "Offline gloss dictionary version unsupported"
    );
    let missing = directory.path().join("zh-ko.db");
    assert_eq!(
        message(&missing, "ko"),
        "Offline gloss dictionary unavailable"
    );
    assert!(
        !missing.exists(),
        "a read-only open must not create the file"
    );
    let damaged = directory.path().join("zh-es.db");
    std::fs::write(&damaged, "synthetic damaged database").unwrap();
    assert!(candidate_target_glosses(damaged.to_str().unwrap(), "es", &candidates).is_err());
}







fn pronunciation_database(path: &Path, kind: &str, version: i32) {
    let database = Connection::open(path).unwrap();
    database
        .execute_batch(&format!(
            "CREATE TABLE en_phonetics(word TEXT PRIMARY KEY, phonetic TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
             INSERT INTO meta VALUES('kind', '{kind}');
             INSERT INTO en_phonetics VALUES('love', 'lʌv');
             INSERT INTO en_phonetics VALUES('sky', 'skaɪ');
             PRAGMA user_version = {version};"
        ))
        .unwrap();
}







#[test]
fn english_phonetics_answer_known_words_in_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("en-phonetic.db");
    pronunciation_database(&path, "en_phonetic", 1);
    let words = vec!["sky".to_owned(), "unknown".to_owned(), "love".to_owned()];
    assert_eq!(
        english_phonetics(path.to_str().unwrap(), &words).unwrap(),
        vec!["skaɪ", "", "lʌv"]
    );
    assert!(english_phonetics(path.to_str().unwrap(), &[])
        .unwrap()
        .is_empty());
}







#[test]
fn english_phonetics_refuse_another_kind_version_or_a_missing_file() {
    let directory = tempfile::tempdir().unwrap();
    let words = vec!["love".to_owned()];
    let gloss = directory.path().join("gloss.db");
    pronunciation_database(&gloss, "zh_gloss", 1);
    assert!(english_phonetics(gloss.to_str().unwrap(), &words).is_err());
    let future = directory.path().join("future.db");
    pronunciation_database(&future, "en_phonetic", 2);
    assert!(english_phonetics(future.to_str().unwrap(), &words).is_err());
    let missing = directory.path().join("missing.db");
    assert!(english_phonetics(missing.to_str().unwrap(), &words).is_err());
    assert!(english_phonetics("", &words).is_err());
    assert!(
        !missing.exists(),
        "a read-only open must not create the file"
    );
}







#[cfg(unix)]
#[test]
fn english_phonetics_rejects_a_linked_database() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("en-phonetic.db");
    pronunciation_database(&external, "en_phonetic", 1);
    let linked = directory.path().join("en-phonetic.db");
    symlink(&external, &linked).unwrap();

    assert!(english_phonetics(linked.to_str().unwrap(), &["love".to_owned()]).is_err());
}







#[cfg(any(unix, windows))]
#[test]
fn english_phonetics_rejects_a_hard_linked_database() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("en-phonetic.db");
    pronunciation_database(&external, "en_phonetic", 1);
    let linked = directory.path().join("en-phonetic.db");
    std::fs::hard_link(&external, &linked).unwrap();
    // 别的用户也能写的目录里，多链接文件可能是他们放的；Windows 上多链接一律拒绝。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    }

    assert!(english_phonetics(linked.to_str().unwrap(), &["love".to_owned()]).is_err());
}







/// 发音库是只读的随包资源；Nix 的 store 去重把它合并成硬链接后，所在目录只有属主能写，照常查询（#6386）。
#[cfg(unix)]
#[test]
fn english_phonetics_reads_a_hard_linked_database_in_a_closed_directory() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    // 只有属主能写；临时目录的权限取决于 umask。
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let original = directory.path().join("en-phonetic.db");
    pronunciation_database(&original, "en_phonetic", 1);
    std::fs::hard_link(&original, directory.path().join("deduplicated.db")).unwrap();

    assert!(english_phonetics(original.to_str().unwrap(), &["love".to_owned()]).is_ok());
}







#[test]
fn reset_learned_data_restores_packaged_dictionaries_and_clears_journal() {
    for component in ["ascii", "陆傲天"] {
        reset_learned_data_under_root(component);
    }
}






fn reset_learned_data_under_root(component: &str) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(component);
    std::fs::create_dir_all(&root).unwrap();
    let value = options(&root);
    let resources = Path::new(&value.resources);
    let dictionaries = Path::new(&value.dictionaries);
    Connection::open(resources.join("msime-pinyin.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
             INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',100);
             CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);",
        )
        .unwrap();
    Connection::open(resources.join("msime-english.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE english_words(word TEXT,display TEXT,weight INTEGER);
             CREATE TABLE en_zh_glosses(english TEXT PRIMARY KEY,chinese_gloss TEXT);
             CREATE TABLE zh_en_glosses(chinese TEXT PRIMARY KEY,english TEXT);
             INSERT INTO english_words VALUES('word','word',100);",
        )
        .unwrap();
    std::fs::copy(
        resources.join("msime-pinyin.db"),
        dictionaries.join("msime-pinyin.db"),
    )
    .unwrap();
    std::fs::copy(
        resources.join("msime-english.db"),
        dictionaries.join("msime-english.db"),
    )
    .unwrap();
    let journal = Path::new(&value.user_data).join("msime_user.db");
    Connection::open(&journal)
        .unwrap()
        .execute_batch(
            "CREATE TABLE user_dictionary_operations(dictionary TEXT,key TEXT,value TEXT,operation TEXT,weight INTEGER,display TEXT,user_inserted INTEGER);
             CREATE TABLE personal_dictionary_receipts(request_id TEXT PRIMARY KEY,payload TEXT);
             CREATE TABLE candidate_selection_state(context_key TEXT,entry_key TEXT,value TEXT,selection_count INTEGER);
             INSERT INTO user_dictionary_operations VALUES('pinyin','ni''hao','你好','upsert',1,'',1);
             INSERT INTO candidate_selection_state VALUES('ni''hao','ni''hao','你好',7);",
        )
        .unwrap();
    Connection::open(dictionaries.join("msime-pinyin.db"))
        .unwrap()
        .execute("UPDATE tbl_2_n SET weight=1", [])
        .unwrap();

    reset_learned_data(&value).unwrap();

    let weight: i64 = Connection::open(dictionaries.join("msime-pinyin.db"))
        .unwrap()
        .query_row(
            "SELECT weight FROM tbl_2_n WHERE key='ni''hao'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(weight, 100);
    let journal = Connection::open(journal).unwrap();
    let count = |table: &str| -> i64 {
        journal
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(count("user_dictionary_operations"), 0);
    assert_eq!(count("candidate_selection_state"), 0);
}







#[test]
fn reset_refuses_to_reset_the_packaged_bundle_in_place() {
    let root = tempfile::tempdir().unwrap();
    let mut value = options(root.path());
    value.dictionaries = value.resources.clone();
    assert_eq!(
        reset_learned_data(&value).unwrap_err().to_string(),
        "Cannot reset packaged dictionaries in place"
    );
    let value = options(root.path());
    assert_eq!(
        reset_learned_data(&value).unwrap_err().to_string(),
        "Packaged dictionary is unavailable"
    );
}







#[test]
fn dictionary_revision_uses_real_journal_and_rejects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let before = dictionary_state_revision(&value).unwrap();
    assert_eq!(before.len(), 64);
    assert_eq!(before, dictionary_state_revision(&value).unwrap());
    let journal = Path::new(&value.user_data).join("msime_user.db");
    assert!(!journal.exists());
    std::fs::write(&journal, b"synthetic invalid database").unwrap();
    assert!(dictionary_state_revision(&value).is_err());
    assert_eq!(
        std::fs::read(&journal).unwrap(),
        b"synthetic invalid database"
    );
}







#[test]
fn staging_rejects_a_zero_limit_and_reports_transport_failures() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let generation = dir.path().join("generation");
    let generation = generation.to_str().unwrap();
    let error =
        stage_dictionary_state(&value, generation, "zero", 0, std::iter::empty()).unwrap_err();
    assert_eq!(error.to_string(), "Invalid snapshot record limit");
    let error = stage_dictionary_state(
        &value,
        generation,
        "broken",
        10,
        std::iter::once(Err(SnapshotReadError)),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "Snapshot record stream failed");
    assert!(!Path::new(generation).join("broken").exists());
}

#[test]
fn invalid_options_return_errors_instead_of_unwinding_into_rust() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = options(dir.path());
    let message = |value: &EngineOptions| Session::new(value).err().unwrap().to_string();
    value.scheme = 255;
    assert_eq!(message(&value), "Unsupported input scheme");
    value.scheme = 0;
    value.shuangpin_profile = 255;
    assert_eq!(message(&value), "Unsupported shuangpin profile");
    value.shuangpin_profile = 0;
    value.frequency_mode = "sometimes".into();
    assert_eq!(message(&value), "Unsupported frequency mode");
    value.frequency_mode = "promote".into();
    assert!(Session::new(&value).is_ok());
    value.resources = "relative".into();
    assert_eq!(message(&value), "Runtime directories must be absolute");
}







#[test]
fn microsoft_profile_accepts_semicolon_as_an_ing_final() {
    let dir = tempfile::tempdir().unwrap();
    for profile in 0..4 {
        let mut options = options(dir.path());
        options.scheme = 1;
        options.shuangpin_profile = profile;
        let mut session = Session::new(&options).unwrap();
        type_text(&mut session, b"b;");
        let snapshot = session.snapshot().unwrap();
        assert_eq!(snapshot.editing_text, if profile == 3 { "b;" } else { "b" });
        assert_eq!(snapshot.microsoft_shuangpin, profile == 3);
    }
}







/// 小鹤的键位，zh 换到 a，ing 换到 `;`，零声母改用 o 引导。
fn custom_table() -> crate::types::ShuangpinCustomTable {
    let pairs = |entries: &[(&str, &str)]| -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(unit, key)| ((*unit).to_owned(), (*key).to_owned()))
            .collect()
    };
    crate::types::ShuangpinCustomTable {
        initials: pairs(&[("zh", "a"), ("ch", "i"), ("sh", "u")]),
        finals: pairs(&[
            ("iu", "q"),
            ("ei", "w"),
            ("e", "e"),
            ("uan", "r"),
            ("ue", "t"),
            ("ve", "t"),
            ("un", "y"),
            ("u", "u"),
            ("i", "i"),
            ("uo", "o"),
            ("o", "o"),
            ("ie", "p"),
            ("a", "a"),
            ("ong", "s"),
            ("iong", "s"),
            ("ai", "d"),
            ("en", "f"),
            ("eng", "g"),
            ("ang", "h"),
            ("an", "j"),
            ("uai", "k"),
            ("ing", ";"),
            ("uang", "l"),
            ("iang", "l"),
            ("ou", "z"),
            ("ua", "x"),
            ("ia", "x"),
            ("ao", "c"),
            ("ui", "v"),
            ("v", "v"),
            ("in", "b"),
            ("iao", "n"),
            ("ian", "m"),
        ]),
        zero_initials: pairs(&[
            ("a", "oa"),
            ("ai", "od"),
            ("an", "oj"),
            ("ang", "oh"),
            ("ao", "oc"),
            ("e", "oe"),
            ("ei", "ow"),
            ("en", "of"),
            ("eng", "og"),
            ("er", "or"),
            ("o", "oo"),
            ("ou", "oz"),
        ]),
    }
}







#[test]
fn custom_profile_runs_the_users_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = options(dir.path());
    options.scheme = 1;
    options.shuangpin_profile = 4;
    options.shuangpin_custom_profile = Some(custom_table());
    options.shuangpin_preedit_uses_raw = false;
    let mut session = Session::new(&options).unwrap();
    type_text(&mut session, b"ahx;oa");
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.shuangpin_profile, "custom");
    // `;` 上有韵母，宿主要把它当字母键送进来。
    assert!(snapshot.microsoft_shuangpin);
    assert_eq!(snapshot.editing_text, "ahx;oa");
    assert_eq!(snapshot.segment_raw_boundaries, vec![0, 2, 4, 6]);
    assert_eq!(snapshot.preedit, "zhang'xing'a");
    // 偶数位置上的 `;` 不是一个音节的第二个键，不收。
    let mut session = Session::new(&options).unwrap();
    type_text(&mut session, b"ah;");
    assert_eq!(session.snapshot().unwrap().editing_text, "ah");
}







/// 韵母都不在 `;` 上、只有零声母编码用 `;` 做第二个键时，会话照样收 `;`，并告诉宿主把它当字母键送进来。
#[test]
fn custom_profile_accepts_semicolon_used_only_by_a_zero_initial_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut table = custom_table();
    for (unit, key) in &mut table.finals {
        if unit == "ing" {
            *key = "k".to_owned();
        }
    }
    for (unit, key) in &mut table.zero_initials {
        if unit == "a" {
            *key = "o;".to_owned();
        }
    }
    let mut options = options(dir.path());
    options.scheme = 1;
    options.shuangpin_profile = 4;
    options.shuangpin_custom_profile = Some(table);
    options.shuangpin_preedit_uses_raw = false;
    let mut session = Session::new(&options).unwrap();
    type_text(&mut session, b"xko;");
    let snapshot = session.snapshot().unwrap();
    assert!(snapshot.microsoft_shuangpin);
    assert_eq!(snapshot.editing_text, "xko;");
    assert_eq!(snapshot.preedit, "xing'a");
}







#[test]
fn custom_profile_needs_a_valid_table_only_when_shuangpin_is_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = options(dir.path());
    options.shuangpin_profile = 4;
    let message = |value: &EngineOptions| Session::new(value).err().unwrap().to_string();
    assert_eq!(
        message(&options),
        "Invalid custom shuangpin profile: no table"
    );
    let mut table = custom_table();
    table.finals.retain(|(unit, _)| unit != "iong");
    options.shuangpin_custom_profile = Some(table);
    assert_eq!(
        message(&options),
        "Invalid custom shuangpin profile: finals unit \"iong\" has no key"
    );
    // 没有双拼的会话用不上这份设置，按小鹤建起来。
    options.enabled_schemes = crate::types::SchemeSet::of(&[crate::types::SchemeType::Quanpin]);
    let session = Session::new(&options).unwrap();
    assert_eq!(session.snapshot().unwrap().shuangpin_profile, "xiaohe");
}







#[test]
fn microsoft_profile_keeps_trailing_semicolon_in_segment_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    for (input, expected) in [
        (&b"nihkb;"[..], vec![0, 2, 4, 6]),
        // `cb` is a valid Microsoft shuangpin pair and must win over pairing the final `b` with the trailing semicolon.
        (&b"nihcb;"[..], vec![0, 2, 3, 5, 6]),
    ] {
        let mut options = options(dir.path());
        options.scheme = 1;
        options.shuangpin_profile = 3;
        let mut session = Session::new(&options).unwrap();
        type_text(&mut session, input);
        assert_eq!(session.snapshot().unwrap().segment_raw_boundaries, expected);
    }
}







/// A keyboard face labels its letter keys with the units they carry, read from the profile the session runs: this asks the session for its profile name instead of assuming the option index and the name agree.
#[test]
fn shuangpin_key_hints_describe_the_profile_the_session_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut seen: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for profile in 0..4 {
        let mut options = options(dir.path());
        options.scheme = 1;
        options.shuangpin_profile = profile;
        let session = Session::new(&options).unwrap();
        let name = session.snapshot().unwrap().shuangpin_profile;
        let hints: Vec<(String, String)> = shuangpin_key_hints(&name)
            .into_iter()
            .map(|entry| (entry.key, entry.hint))
            .collect();
        assert!(
            hints.len() >= 26,
            "{name} labelled only {} keys",
            hints.len()
        );
        for (key, hint) in &hints {
            assert!(
                key.len() == 1 && ("A"..="Z").contains(&key.as_str()) || key == ";",
                "{name} produced a hint for {key:?}, which is not a letter key"
            );
            assert!(!hint.is_empty(), "{name} key {key} carries an empty hint");
            // " / " separates initials from finals, so it appears at most once.
            assert!(
                hint.matches(" / ").count() <= 1,
                "{name} key {key} hint {hint:?} reads as more than two sides"
            );
        }
        seen.push((name, hints));
    }
    for (index, (name, hints)) in seen.iter().enumerate() {
        for (other_name, other_hints) in seen.iter().skip(index + 1) {
            assert_ne!(
                hints, other_hints,
                "{name} and {other_name} produced the same key face"
            );
        }
    }
}







/// Xiaohe keeps two finals on K, and a host-side copy of the keymap once listed only one of them.
#[test]
fn shuangpin_key_hints_keep_every_unit_a_key_carries() {
    let hints: std::collections::HashMap<String, String> = shuangpin_key_hints("xiaohe")
        .into_iter()
        .map(|entry| (entry.key, entry.hint))
        .collect();
    assert_eq!(hints.get("K").map(String::as_str), Some("ing uai"));
    assert_eq!(hints.get("V").map(String::as_str), Some("zh / ui ü"));
}







#[test]
fn shuangpin_key_hints_reject_an_unknown_profile() {
    assert!(shuangpin_key_hints("").is_empty());
    assert!(shuangpin_key_hints("xiaohe-v2").is_empty());
    assert!(shuangpin_key_hints("quanpin").is_empty());
}







#[test]
fn all_shuangpin_profiles_accept_yo_as_one_syllable() {
    let dir = tempfile::tempdir().unwrap();
    for profile in 0..4 {
        let mut options = options(dir.path());
        options.scheme = 1;
        options.shuangpin_profile = profile;
        let mut session = Session::new(&options).unwrap();
        type_text(&mut session, b"yo");
        let snapshot = session.snapshot().unwrap();
        assert_eq!(snapshot.editing_text, "yo");
        assert_eq!(snapshot.preedit, "yo", "yo was split: {snapshot:?}");
    }
}







#[test]
fn cloud_query_preserves_manual_quanpin_segmentation() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(&options(dir.path())).unwrap();
    for character in b"qi'e'huan" {
        assert!(session.character(*character, false).unwrap().handled);
    }
    let query = session.online_query().unwrap();
    assert!(query.available);
    assert_eq!(query.query_text, "qi'e'huan");
    // A source other than cloud or AI, an unavailable query, and a scheme no session issues are refused before the engine sees them.
    assert!(!session.apply_online_candidate(&query, "企鹅换", 2).unwrap());
    let unavailable = OnlineQuerySnapshot {
        available: false,
        ..query.clone()
    };
    assert!(!session
        .apply_online_candidate(&unavailable, "企鹅换", 0)
        .unwrap());
    let foreign = OnlineQuerySnapshot {
        scheme: 200,
        ..query.clone()
    };
    assert!(!session
        .apply_online_candidates(&foreign, &["企鹅换".to_owned()], 1)
        .unwrap());
    session.command(Command::Cancel).unwrap();
    assert_eq!(
        session.online_query().unwrap(),
        OnlineQuerySnapshot::default()
    );
}







#[test]
fn real_engine_handles_unicode_mode_without_a_dictionary_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(&options(dir.path())).unwrap();
    assert!(session.character(b'U', true).unwrap().handled);
    for character in b"4e2d" {
        assert!(session.character(*character, false).unwrap().handled);
    }
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.local_mode, "unicode");
    assert!(snapshot
        .candidates
        .iter()
        .any(|candidate| candidate == "中"));
    let result = session.select(0).unwrap();
    assert!(result.has_commit);
    assert_eq!(result.commit, "中");
    assert!(session.snapshot().unwrap().preedit.is_empty());
    assert_eq!(session.snapshot().unwrap().local_mode, "none");
}







#[test]
fn commit_raw_applies_windows_english_learning_policy() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    session.set_dedicated_english(true).unwrap();
    for character in b"hello" {
        assert!(session.character(*character, false).unwrap().handled);
    }
    let result = session.command(Command::CommitRaw).unwrap();
    assert_eq!(result.commit, "hello");
    assert_eq!(result.diagnostic, "");
    let learned: String = Connection::open(Path::new(&value.dictionaries).join("msime-english.db"))
        .unwrap()
        .query_row(
            "SELECT display FROM english_words WHERE word='hello'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(learned, "hello");
}







fn english_word_count(value: &EngineOptions, word: &str) -> i64 {
    let database = Path::new(&value.dictionaries).join("msime-english.db");
    if !database.exists() {
        return 0;
    }
    let database = Connection::open(database).unwrap();
    let tables: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='english_words'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if tables == 0 {
        return 0;
    }
    database
        .query_row(
            "SELECT COUNT(*) FROM english_words WHERE word=?1",
            [word],
            |row| row.get(0),
        )
        .unwrap()
}







#[test]
fn raw_commit_without_learning_leaves_the_english_dictionary_alone() {
    for dedicated in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let value = options(dir.path());
        let mut session = Session::new(&value).unwrap();
        session.set_dedicated_english(dedicated).unwrap();
        for character in b"hello" {
            assert!(session.character(*character, false).unwrap().handled);
        }
        let result = session.command(Command::CommitRawWithoutLearning).unwrap();
        assert!(result.handled && result.has_commit);
        assert_eq!(result.commit, "hello");
        assert!(session.snapshot().unwrap().preedit.is_empty());
        assert_eq!(english_word_count(&value, "hello"), 0);
    }
}







#[test]
fn complete_pinyin_raw_commit_does_not_learn_as_english() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    for character in b"ni" {
        assert!(session.character(*character, false).unwrap().handled);
    }
    assert_eq!(session.command(Command::CommitRaw).unwrap().commit, "ni");
    let database = Path::new(&value.dictionaries).join("msime-english.db");
    if database.exists() {
        let count: i64 = Connection::open(database)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='english_words'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}







#[test]
fn incomplete_pinyin_raw_commit_is_learned_as_an_english_word() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    type_text(&mut session, b"xyz");
    let result = session.command(Command::CommitRaw).unwrap();
    assert_eq!(result.commit, "xyz");
    assert_eq!(result.diagnostic, "");
    assert_eq!(english_word_count(&value, "xyz"), 1);
}







/// bridge.cpp:1332-1359: Enter in any local mode learns the committed letters as an English word. Local modes are entered only from the pinyin schemes, whose segmentation is empty while one is active, so the incomplete-pinyin rule would learn the word too; this pins the outcome the two rules share.
#[test]
fn local_mode_raw_commit_is_learned_as_an_english_word() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    assert!(session.character(b'Y', true).unwrap().handled);
    type_text(&mut session, b"rustacean");
    assert_eq!(session.snapshot().unwrap().local_mode, "temporary_english");
    let result = session.command(Command::CommitRaw).unwrap();
    assert!(result.has_commit);
    // 回车上屏的是所见预编辑：临时英文的入口字母 `Y` 算文本（#7616）；进词库的词仍不带它。
    assert_eq!(result.commit, "Yrustacean");
    assert_eq!(result.diagnostic, "");
    assert_eq!(english_word_count(&value, "rustacean"), 1);
}







/// 网址不是英文单词：网址模式下 Enter 上屏整段网址，不写进英文词库。
#[test]
fn url_enter_is_not_learned_as_english() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    type_text(&mut session, b"https");
    assert!(session.punctuation(b':').unwrap().handled);
    type_text(&mut session, b"a");
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.local_mode, "url");
    assert_eq!(snapshot.preedit, "https:a");
    let result = session.command(Command::CommitRaw).unwrap();
    assert!(result.has_commit);
    assert_eq!(result.commit, "https:a");
    assert_eq!(result.diagnostic, "");
    assert_eq!(english_word_count(&value, "https"), 0);
    assert_eq!(english_word_count(&value, "https:a"), 0);
}







/// 网址模式下 Esc 丢弃网址，不上屏，回到没有本地模式的状态。
#[test]
fn url_escape_discards_without_commit() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    type_text(&mut session, b"www");
    assert!(session.punctuation(b'.').unwrap().handled);
    type_text(&mut session, b"a");
    assert_eq!(session.snapshot().unwrap().local_mode, "url");
    let result = session.command(Command::Cancel).unwrap();
    assert!(result.handled);
    assert!(!result.has_commit, "{:?}", result.commit);
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.local_mode, "none");
    assert!(snapshot.preedit.is_empty());
}







/// Windows `input.enter_learns_english_word` 出厂为开：回车上屏输入的英文词写进英文词库；
/// 关掉后回车照常上屏，只是不学词。
#[test]
fn enter_learns_english_word_follows_the_preference() {
    for enabled in [false, true] {
        for (route, typed, committed, learned) in [
            ("dedicated", "hello", "hello", "hello"),
            ("incomplete_pinyin", "xyz", "xyz", "xyz"),
            ("temporary_english", "Yrustacean", "Yrustacean", "rustacean"),
            ("complete_pinyin", "ni", "ni", "ni"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut value = options(dir.path());
            value.learn_english_on_enter = enabled;
            let mut session = Session::new(&value).unwrap();
            session.set_dedicated_english(route == "dedicated").unwrap();
            for byte in typed.bytes() {
                assert!(
                    session
                        .character(byte, byte.is_ascii_uppercase())
                        .unwrap()
                        .handled
                );
            }
            let result = session.command(Command::CommitRaw).unwrap();
            assert!(result.handled && result.has_commit, "{route}");
            assert_eq!(result.commit, committed, "{route}");
            assert_eq!(result.diagnostic, "", "{route}");
            assert_eq!(
                english_word_count(&value, learned),
                i64::from(enabled && route != "complete_pinyin"),
                "{route}"
            );
        }
    }
}







/// bridge.cpp:1349-1357: a local-mode commit that is not an English word (here the Unicode mode's hex digits) is still committed; the failed learning is reported beside it rather than undoing it.
#[test]
fn unlearnable_local_mode_raw_commit_reports_the_diagnostic_but_still_commits() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let mut session = Session::new(&value).unwrap();
    assert!(session.character(b'U', true).unwrap().handled);
    type_text(&mut session, b"4e2d");
    let result = session.command(Command::CommitRaw).unwrap();
    assert!(result.handled && result.has_commit);
    assert!(!result.commit.is_empty());
    assert_eq!(
        result.diagnostic,
        crate::diagnostics::ENGLISH_WORD_NOT_LEARNED
    );
    assert_eq!(result.diagnostic, "English word could not be learned.");
    assert!(session.snapshot().unwrap().preedit.is_empty());
}

#[test]
fn host_argument_checks_use_the_bridge_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(&options(dir.path())).unwrap();
    let message = |error: crate::EngineError| error.to_string();
    assert_eq!(
        message(session.character(200, false).unwrap_err()),
        "Engine character must be ASCII"
    );
    assert_eq!(
        message(session.punctuation(128).unwrap_err()),
        "Engine punctuation must be ASCII"
    );
    assert_eq!(
        message(
            session
                .balance_paired_punctuation_after_auto_close(0xE3)
                .unwrap_err()
        ),
        "Paired punctuation opening must be ASCII"
    );
    assert_eq!(
        message(session.set_punctuation_lock(3).unwrap_err()),
        "Invalid punctuation lock"
    );
    session.set_punctuation_lock(2).unwrap();
}

#[test]
fn session_options_map_every_host_field() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = options(dir.path());
    value.scheme = 1;
    value.shuangpin_profile = 2;
    value.shuangpin_preedit_uses_raw = false;
    value.single_character_only = true;
    value.learning = true;
    value.autocorrect_transposition = false;
    value.autocorrect_neighbor = true;
    value.fuzzy_pinyin_rules = u32::MAX;
    value.chinese_punctuation = false;
    value.paired_punctuation = false;
    value.punctuation_lock = 2;
    value.frequency_mode = "linear".into();
    value.frequency_trigger_count = 3;
    value.frequency_linear_step = 4;
    value.mixed_english = false;
    value.english_minimum_prefix = 7;
    value.mixed_emoji = true;
    value.mixed_kaomoji = true;
    value.mixed_symbols = true;
    value.local_date_time = false;
    value.rescoring_context = "上文".into();
    value.sentence_alternatives = false;
    let mapped = super::options::session_options(&value).unwrap();
    assert_eq!(mapped.paths.dictionaries, Path::new(&value.dictionaries));
    assert_eq!(mapped.scheme, crate::SchemeType::Shuangpin);
    assert_eq!(
        mapped.shuangpin_profile,
        crate::ShuangpinProfileKind::Shoudao
    );
    assert!(!mapped.shuangpin_preedit_uses_raw);
    assert!(mapped.single_character_only);
    assert!(mapped.learning);
    assert_eq!(mapped.autocorrect_types, crate::autocorrect_type::NEIGHBOR);
    assert_eq!(mapped.fuzzy_pinyin.rules, crate::fuzzy_rule::ALL);
    assert!(!mapped.chinese_punctuation);
    assert!(!mapped.paired_punctuation);
    assert_eq!(mapped.punctuation_lock, 2);
    assert_eq!(
        mapped.frequency.mode,
        crate::FrequencyAdjustmentMode::Linear
    );
    assert_eq!(mapped.frequency.trigger_count, 3);
    assert_eq!(mapped.frequency.linear_step, 4);
    assert!(!mapped.english.mixed_candidates);
    assert_eq!(mapped.english.minimum_prefix, 7);
    assert!(mapped.expressive.emoji_candidates && mapped.expressive.kaomoji_candidates);
    assert!(mapped.expressive.symbol_candidates);
    assert!(!mapped.local_modes.date_time);
    assert!(mapped.local_modes.unicode && mapped.local_modes.temporary_english);
    assert_eq!(mapped.rescoring_context, "上文");
    assert!(!mapped.sentence_alternatives);
    assert!(mapped.personal_context);
    assert_eq!(mapped.enabled_schemes, crate::SchemeSet::ALL);
}







#[test]
fn translation_sidecar_prefers_the_user_file_and_is_removed_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let target = Path::new(&value.dictionaries).join("custom_translations.txt");
    let user = Path::new(&value.user_data).join("custom_translations.txt");
    let resource = Path::new(&value.resources).join("custom_translations.txt");
    std::fs::write(&resource, "天\tpackaged\n").unwrap();
    super::options::prepare_translation_sidecar(&value).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "天\tpackaged\n");
    std::fs::write(&user, "天\tmine\n").unwrap();
    super::options::prepare_translation_sidecar(&value).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "天\tmine\n");
    std::fs::remove_file(&user).unwrap();
    std::fs::remove_file(&resource).unwrap();
    Session::new(&value).unwrap();
    assert!(!target.exists());
}







#[cfg(unix)]
#[test]
fn translation_sidecar_publication_keeps_open_readers_on_complete_old_content() {
    use std::io::Read;

    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let target = Path::new(&value.dictionaries).join("custom_translations.txt");
    let resource = Path::new(&value.resources).join("custom_translations.txt");
    let user = Path::new(&value.user_data).join("custom_translations.txt");
    std::fs::write(&resource, b"fixture\told\n").unwrap();
    super::options::prepare_translation_sidecar(&value).unwrap();
    let mut previous = std::fs::File::open(&target).unwrap();

    std::fs::write(&user, b"fixture\tnew\n").unwrap();
    super::options::prepare_translation_sidecar(&value).unwrap();

    let mut old_contents = String::new();
    previous.read_to_string(&mut old_contents).unwrap();
    assert_eq!(old_contents, "fixture\told\n");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "fixture\tnew\n");
}







#[test]
fn translation_sidecar_copy_rejects_an_oversized_source() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let resource = Path::new(&value.resources).join("custom_translations.txt");
    std::fs::File::create(&resource)
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();

    assert_eq!(
        super::options::prepare_translation_sidecar(&value)
            .unwrap_err()
            .to_string(),
        "Unable to prepare custom translation sidecar"
    );
    assert!(!Path::new(&value.dictionaries)
        .join("custom_translations.txt")
        .exists());
}







#[cfg(unix)]
#[test]
fn translation_sidecar_copy_rejects_a_symlinked_user_file() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let target = Path::new(&value.dictionaries).join("custom_translations.txt");
    let user = Path::new(&value.user_data).join("custom_translations.txt");
    let resource = Path::new(&value.resources).join("custom_translations.txt");
    std::fs::write(&resource, "天\tpackaged\n").unwrap();
    std::fs::write(
        outside.path().join("custom_translations.txt"),
        "天\texternal\n",
    )
    .unwrap();
    symlink(outside.path().join("custom_translations.txt"), &user).unwrap();

    super::options::prepare_translation_sidecar(&value).unwrap();
    assert_eq!(std::fs::read_to_string(target).unwrap(), "天\tpackaged\n");
}







#[cfg(unix)]
#[test]
fn translation_sidecar_copy_rejects_a_symlinked_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let target = Path::new(&value.dictionaries).join("custom_translations.txt");
    let user = Path::new(&value.user_data).join("custom_translations.txt");
    let external = outside.path().join("target.txt");
    std::fs::write(&user, "天\tuser\n").unwrap();
    std::fs::write(&external, "keep\n").unwrap();
    symlink(&external, &target).unwrap();

    assert_eq!(
        super::options::prepare_translation_sidecar(&value)
            .unwrap_err()
            .to_string(),
        "Unable to prepare custom translation sidecar"
    );
    assert_eq!(std::fs::read_to_string(external).unwrap(), "keep\n");
}







#[cfg(unix)]
#[test]
fn translation_sidecar_writer_does_not_follow_a_replaced_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = dir.path().join("custom_translations.txt");
    let external = outside.path().join("target.txt");
    std::fs::write(&external, "keep\n").unwrap();
    symlink(&external, &target).unwrap();

    assert!(super::options::write_private_file_no_follow(&target, b"overwrite\n").is_err());
    assert_eq!(std::fs::read_to_string(external).unwrap(), "keep\n");
}







#[cfg(any(unix, windows))]
#[test]
fn translation_sidecar_writer_rejects_a_hard_linked_target() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = dir.path().join("custom_translations.txt");
    let external = outside.path().join("target.txt");
    std::fs::write(&external, "keep\n").unwrap();
    std::fs::hard_link(&external, &target).unwrap();

    assert!(super::options::write_private_file_no_follow(&target, b"overwrite\n").is_err());
    assert_eq!(std::fs::read_to_string(external).unwrap(), "keep\n");
}







#[cfg(unix)]
#[test]
fn translation_sidecar_copy_rejects_a_symlinked_target_parent() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    let dictionaries = Path::new(&value.dictionaries).to_path_buf();
    std::fs::remove_dir(&dictionaries).unwrap();
    symlink(outside.path(), &dictionaries).unwrap();
    std::fs::write(
        Path::new(&value.resources).join("custom_translations.txt"),
        "天\tpackaged\n",
    )
    .unwrap();

    assert_eq!(
        super::options::prepare_translation_sidecar(&value)
            .unwrap_err()
            .to_string(),
        "Unable to prepare custom translation sidecar"
    );
    assert!(!outside.path().join("custom_translations.txt").exists());
}







#[test]
fn english_completions_validate_and_lowercase_the_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let resources = dir.path().to_str().unwrap();
    let message = |prefix: &str, limit| {
        english_completions(resources, prefix, limit)
            .unwrap_err()
            .to_string()
    };
    assert_eq!(message("he", 0), "Invalid English completion limit");
    assert_eq!(message("he", 33), "Invalid English completion limit");
    assert_eq!(message("he1", 5), "Invalid English completion prefix");
    assert_eq!(
        english_completions(resources, "", 5).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(message("he", 5), "English dictionary unavailable");
    Connection::open(dir.path().join("msime-english.db"))
        .unwrap()
        .execute_batch(&format!(
            "{ENGLISH_SCHEMA}
             INSERT INTO english_words VALUES('hello','Hello',10);
             INSERT INTO english_words VALUES('help','help',20);
             INSERT INTO english_words VALUES('world','world',30);"
        ))
        .unwrap();
    assert_eq!(
        english_completions(resources, "HEL", 32).unwrap(),
        vec!["help", "Hello"]
    );
    assert_eq!(
        english_completions(resources, "hel", 1).unwrap(),
        vec!["help"]
    );
}







#[test]
fn gloss_keys_follow_the_bridge_rules() {
    assert_eq!(
        candidate_gloss_key("Don't Stop-Me", 4),
        Some(("don't stop-me".to_owned(), false))
    );
    assert_eq!(
        candidate_gloss_key("天气", 0),
        Some(("天气".to_owned(), true))
    );
    // Mixed text with a Han character is looked up verbatim as Chinese.
    assert_eq!(
        candidate_gloss_key("A股", 0),
        Some(("A股".to_owned(), true))
    );
    assert_eq!(candidate_gloss_key("天气", 6), None);
    assert_eq!(candidate_gloss_key("天气", 7), None);
    // 日文混输行的汉字是日文，不当中文查释义。
    assert_eq!(candidate_gloss_key("私", 13), None);
    // 脚本插件的行不交给任何释义来源，汉字和英文都一样。
    assert_eq!(candidate_gloss_key("天气", 14), None);
    assert_eq!(candidate_gloss_key("hello", 14), None);
    assert_eq!(candidate_gloss_key("- '", 4), None);
    assert_eq!(candidate_gloss_key("123", 0), None);
    assert_eq!(candidate_gloss_key("〇", 0), None);
}







#[test]
fn gloss_display_keeps_two_senses_and_withholds_control_characters() {
    assert_eq!(candidate_gloss_display("a; b; c"), "a; b");
    let joined = candidate_gloss_display("alpha;beta");
    assert_eq!(joined, "alpha; beta");
    assert_eq!(joined.capacity(), joined.len());
    assert_eq!(candidate_gloss_display("甲；乙;丙"), "甲; 乙");
    assert_eq!(
        candidate_gloss_display(";; first ;  ; second"),
        "first; second"
    );
    assert_eq!(
        candidate_gloss_display("  spaced \t out\r\nwords  "),
        "spaced out words"
    );
    assert_eq!(candidate_gloss_display("only"), "only");
    assert_eq!(candidate_gloss_display(""), "");
    assert_eq!(candidate_gloss_display("bad\u{7}bell"), "");
    assert_eq!(candidate_gloss_display("bad\u{85}next"), "");
    // A control character in the third sense is cut before the check sees it.
    assert_eq!(candidate_gloss_display("a; b; \u{1}"), "a; b");
}







#[test]
fn emoji_catalog_wrappers_page_through_others_db() {
    let dir = tempfile::tempdir().unwrap();
    let resources = dir.path().to_str().unwrap();
    Connection::open(dir.path().join("msime-others.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE emoji(emoji TEXT, category TEXT, keywords TEXT, pinyin TEXT, sort_order INTEGER);
             INSERT INTO emoji VALUES('😀','smileys','grin','xiao',1),('😀','smileys','grin','xiao',2),('🐱','animals','cat','mao',3);
             CREATE TABLE kaomoji_catalog(kaomoji TEXT, keywords TEXT, sort_order INTEGER);
             INSERT INTO kaomoji_catalog VALUES('(^_^)','smile',1);
             CREATE TABLE symbol_catalog(symbol TEXT, category TEXT, parent_category TEXT, keywords TEXT, sort_order INTEGER);
             INSERT INTO symbol_catalog VALUES('→','arrows','math','right',1);",
        )
        .unwrap();
    let page = emoji_catalog_filtered_page(resources, "", "", "", 0, 10, "").unwrap();
    assert_eq!(
        page.iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["😀", "🐱"]
    );
    let slice = emoji_catalog_slice(resources, "", "", "", 0, 2, "").unwrap();
    assert_eq!(slice.items.len(), 2);
    assert_eq!(slice.next_offset, 2);
    assert!(!slice.complete);
    assert_eq!(
        emoji_catalog_slice(resources, "", "", "", 0, 0, "")
            .unwrap_err()
            .to_string(),
        "Invalid emoji catalog page"
    );
    assert_eq!(
        emoji_catalog_groups(resources, "").unwrap(),
        ["smileys", "animals"]
    );
    assert_eq!(emoji_catalog_groups(resources, "kaomoji").unwrap(), ["All"]);
    let groups = emoji_symbol_groups(resources).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        (groups[0].parent.as_str(), groups[0].title.as_str()),
        ("math", "arrows")
    );
    let missing = tempfile::tempdir().unwrap();
    assert_eq!(
        emoji_symbol_groups(missing.path().to_str().unwrap())
            .unwrap_err()
            .to_string(),
        "Emoji catalog unavailable"
    );
}







#[test]
fn hanzi_to_pinyin_reads_the_generation_dictionary() {
    let dir = tempfile::tempdir().unwrap();
    let value = options(dir.path());
    Connection::open(Path::new(&value.dictionaries).join("msime-pinyin.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
             INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',100);",
        )
        .unwrap();
    assert_eq!(hanzi_to_pinyin(&value, "你好"), "ni'hao");
    assert_eq!(hanzi_to_pinyin(&value, "hello"), "");
}







/// user_dictionary_journal.cpp:445-452: the reference opened msime's journal per call, so no thread kept it open. A one-shot call releases its thread's cached journal on return, a dropped session releases it too, and a live session keeps it for the keystroke path.
#[test]
fn journal_handles_are_released_when_a_thread_is_done_with_them() {
    use crate::user_dictionary::journal::thread_holds_journal;
    let dir = tempfile::tempdir().unwrap();
    let mut value = options(dir.path());
    value.learning = true;
    for directory in [&value.resources, &value.dictionaries] {
        Connection::open(Path::new(directory).join("msime-pinyin.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
                 INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',200),('ni''hao','nh','拟好',100);",
            )
            .unwrap();
    }

    // A pool thread editing the dictionary keeps nothing open afterwards, whatever the call returned.
    let edit = value.clone();
    std::thread::spawn(move || {
        let entry = DictionaryEntry {
            kind: DictionaryKind::Pinyin,
            key: "ni'hao".into(),
            value: "你蒿".into(),
            weight: 100_000,
        };
        dictionary_edit(&edit, None, Some(&entry), "release-test").unwrap();
        assert!(!thread_holds_journal());
        assert!(dictionary_state_revision(&edit).is_ok());
        assert!(!thread_holds_journal());
    })
    .join()
    .unwrap();

    let mut session = Session::new(&value).unwrap();
    type_text(&mut session, b"nihao");
    let snapshot = session.snapshot().unwrap();
    let index = snapshot
        .candidates
        .iter()
        .position(|word| word == "拟好")
        .unwrap();
    session.select(index).unwrap();
    assert!(thread_holds_journal());
    drop(session);
    assert!(!thread_holds_journal());
}







/// Hosts keep their sessions in a thread-local map, so a session can be dropped while its thread exits, after that thread's cached journal is already destroyed. Its drop still flushes and releases without touching the destroyed cache; it used to abort the process (`cannot access a Thread Local Storage value during or after destruction`).
#[test]
fn a_session_dropped_while_its_thread_exits_does_not_abort() {
    use crate::user_dictionary::journal::thread_holds_journal;
    thread_local! {
        static HELD: std::cell::RefCell<Option<Session>> = const { std::cell::RefCell::new(None) };
    }
    let dir = tempfile::tempdir().unwrap();
    let mut value = options(dir.path());
    value.learning = true;
    for directory in [&value.resources, &value.dictionaries] {
        Connection::open(Path::new(directory).join("msime-pinyin.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
                 INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',200),('ni''hao','nh','拟好',100);",
            )
            .unwrap();
    }
    let journal = Path::new(&value.user_data).join("msime_user.db");
    std::thread::spawn(move || {
        // The map is registered before the journal cache, so thread exit destroys the cache first.
        HELD.with(|held| {
            *held.borrow_mut() = Some(Session::new(&value).unwrap());
            let mut held = held.borrow_mut();
            let session = held.as_mut().unwrap();
            type_text(session, b"nihao");
            let index = session
                .snapshot()
                .unwrap()
                .candidates
                .iter()
                .position(|word| word == "拟好")
                .unwrap();
            session.select(index).unwrap();
        });
        assert!(thread_holds_journal());
    })
    .join()
    .unwrap();
    // The drop during thread exit wrote the queued pick, not merely survived.
    let written: i64 = Connection::open(&journal)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM personal_bigram WHERE previous=char(1) AND word='拟好'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(written, 1);
}







pub(super) fn generated_mode_options(root: &Path) -> EngineOptions {
    let mut value = options(root);
    value.local_expression = true;
    value.local_command = true;
    value.local_mention = true;
    value.command_table = vec![CommandTableEntry {
        trigger: "hi".into(),
        title: "问候".into(),
        template: "你好".into(),
    }];
    value.mention_entries = vec![MentionEntry {
        phone: String::new(),
        text: "Alice".into(),
        key: String::new(),
    }];
    value.quick_phrase_table = vec![QuickPhraseEntry {
        key: "dh".into(),
        text: "电话".into(),
    }];
    value
}







#[test]
fn generated_local_modes_map_through_the_options() {
    let dir = tempfile::tempdir().unwrap();
    let resources = dir.path().join("resources");
    std::fs::create_dir_all(&resources).unwrap();
    for name in ["msime-pinyin.db", "msime-english.db"] {
        Connection::open(resources.join(name)).unwrap();
    }
    let defaults = prepare_options(
        resources.to_str().unwrap(),
        dir.path().join("user").to_str().unwrap(),
        dir.path().join("cache").to_str().unwrap(),
        "generated-modes",
    )
    .unwrap();
    assert!(!defaults.local_expression && !defaults.local_command && !defaults.local_mention);
    assert!(defaults.command_table.is_empty() && defaults.mention_entries.is_empty());
    assert!(defaults.quick_phrase_table.is_empty());
    let value = generated_mode_options(dir.path());
    let mapped = super::options::session_options(&value).unwrap();
    assert!(
        mapped.local_modes.expression && mapped.local_modes.command && mapped.local_modes.mention
    );
    assert_eq!(mapped.command_table, value.command_table);
    assert_eq!(mapped.mention_entries, value.mention_entries);
    assert_eq!(mapped.quick_phrase_table, value.quick_phrase_table);
    let off = super::options::session_options(&options(dir.path())).unwrap();
    assert!(!off.local_modes.expression && !off.local_modes.command && !off.local_modes.mention);
}







#[test]
fn generated_local_modes_publish_their_spelling_symbols_and_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(&generated_mode_options(dir.path())).unwrap();
    assert_eq!(session.snapshot().unwrap().spelling_symbols, "/@");
    assert!(session.character(b'V', true).unwrap().handled);
    type_text(&mut session, b"2*3");
    let view = session.snapshot().unwrap();
    assert_eq!(view.local_mode, "expression");
    assert_eq!(view.spelling_symbols, "0123456789+-*/.()%^");
    assert_eq!(view.candidates[0], "6");
    assert!(!session.online_query().unwrap().available);
    session.command(Command::Cancel).unwrap();

    assert!(session.character(b'/', false).unwrap().handled);
    type_text(&mut session, b"h");
    let view = session.snapshot().unwrap();
    assert_eq!(view.local_mode, "command");
    assert_eq!(view.candidates, ["你好"]);
    assert_eq!(view.candidate_codes, ["hi"]);
    assert_eq!(view.candidate_annotations, ["问候"]);
    session.command(Command::Cancel).unwrap();

    assert!(session.character(b'@', false).unwrap().handled);
    type_text(&mut session, b"al");
    assert_eq!(session.snapshot().unwrap().candidates, ["Alice"]);
    session
        .set_mention_entries(&[MentionEntry {
            phone: String::new(),
            text: "Alan".into(),
            key: String::new(),
        }])
        .unwrap();
    assert_eq!(session.snapshot().unwrap().candidates, ["Alan"]);
    session.command(Command::Cancel).unwrap();
    session
        .set_command_table(&[CommandTableEntry {
            trigger: "yo".into(),
            title: "招呼".into(),
            template: "哟".into(),
        }])
        .unwrap();
    session.character(b'/', false).unwrap();
    type_text(&mut session, b"y");
    // 内置的农历命令 `yinli` 也以 y 开头，排在指令表之后；它的文字随真实时钟变，只核对触发词。
    let view = session.snapshot().unwrap();
    assert_eq!(view.candidates[0], "哟");
    assert_eq!(view.candidate_codes, ["yo", "yinli"]);
}







/// Enter in the expression, command and mention modes commits what was typed, and unlike the other local modes learns none of it as an English word: arithmetic, a trigger or a mention key is not a word the user spelled.
#[test]
fn generated_local_mode_raw_commits_are_not_learned() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = generated_mode_options(dir.path());
    value.learning = true;
    let mut session = Session::new(&value).unwrap();
    for (entry, shift, input, expected) in [
        (b'V', true, &b"1+"[..], "V1+"),
        (b'/', false, &b"xyz"[..], "/xyz"),
        (b'@', false, &b"bob"[..], "@bob"),
    ] {
        assert!(session.character(entry, shift).unwrap().handled);
        type_text(&mut session, input);
        let result = session.command(Command::CommitRaw).unwrap();
        assert_eq!(result.commit, expected);
        assert_eq!(result.diagnostic, "");
        for word in [expected, &expected[1..]] {
            assert_eq!(english_word_count(&value, word), 0, "{word}");
        }
    }
}







/// #7616：只按了入口键就回车时，上屏的是入口字母本身，去掉标记后没有正文可学——以前会报「英文词学不了」（`Y`），临时日文还会把补出来的 `r` 写进英文词库。
#[test]
fn a_bare_entry_letter_raw_commit_learns_nothing() {
    for (letter, mode, word) in [
        (b'Y', "temporary_english", "y"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let value = options(dir.path());
        let mut session = Session::new(&value).unwrap();
        assert!(session.character(letter, true).unwrap().handled);
        assert_eq!(session.snapshot().unwrap().local_mode, mode);
        let result = session.command(Command::CommitRaw).unwrap();
        assert_eq!(result.commit, (letter as char).to_string());
        assert_eq!(result.diagnostic, "");
        assert_eq!(english_word_count(&value, word), 0);
    }
}
