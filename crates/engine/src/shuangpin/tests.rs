//! 双拼引擎单测：方案解码、模糊音、词格整句、在线候选缓存、错拼纠错与尾声母补全。

use std::path::PathBuf;

use rusqlite::Connection;
use tempfile::TempDir;

use super::profile::profile;
use super::ShuangpinEngine;
use crate::assets;
use crate::paths::RuntimePaths;
use crate::types::{
    fuzzy_rule, CandidateSource, FuzzyPinyinOptions, QueryRequest, SchemeType,
    SentenceAssociationOptions, ShuangpinProfileKind, WordItem,
};

struct Fixture {
    _root: TempDir,
    paths: RuntimePaths,
}

impl Fixture {
    fn new(sql: &str) -> Self {
        let root = tempfile::tempdir().expect("fixture directory");
        let directory = root.path().to_path_buf();
        let paths = RuntimePaths {
            resources: directory.clone(),
            user_data: directory.clone(),
            cache: directory.clone(),
            dictionaries: directory,
        };
        Connection::open(paths.dictionary(assets::MAIN_DICTIONARY))
            .expect("fixture dictionary")
            .execute_batch(sql)
            .expect("fixture rows");
        Self { _root: root, paths }
    }

    fn engine(&self, kind: ShuangpinProfileKind) -> ShuangpinEngine {
        ShuangpinEngine::new(profile(kind).unwrap(), &self.paths)
    }

    fn weight(&self, table: &str, key: &str, value: &str) -> Option<i64> {
        Connection::open(self.paths.dictionary(assets::MAIN_DICTIONARY))
            .expect("fixture dictionary")
            .query_row(
                &format!("SELECT weight FROM \"{table}\" WHERE key = ?1 AND value = ?2"),
                [key, value],
                |row| row.get(0),
            )
            .ok()
    }
}

fn request(typed: &str) -> QueryRequest {
    QueryRequest {
        scheme: SchemeType::Shuangpin,
        raw_input: typed.to_ascii_lowercase(),
        raw_input_with_cases: typed.to_string(),
        valid: true,
        ..QueryRequest::default()
    }
}

fn words(candidates: &[WordItem]) -> Vec<&str> {
    candidates.iter().map(|item| item.word.as_str()).collect()
}

fn index_of(candidates: &[WordItem], word: &str) -> usize {
    candidates
        .iter()
        .position(|item| item.word == word)
        .unwrap_or_else(|| panic!("{word} missing from {:?}", words(candidates)))
}

#[test]
fn builtin_profiles_find_den_in_a_synthetic_dictionary() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_d(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_d VALUES('den', 'd', '扽', 100), ('dia', 'd', '嗲', 95), ('deng', 'd', '灯', 90);",
    );
    for kind in [
        ShuangpinProfileKind::Xiaohe,
        ShuangpinProfileKind::Ziranma,
        ShuangpinProfileKind::Shoudao,
        ShuangpinProfileKind::Microsoft,
        ShuangpinProfileKind::SmartAbc,
    ] {
        let selected = profile(kind).unwrap();
        let code = if kind == ShuangpinProfileKind::Shoudao {
            assert_eq!(super::utils::cvt_single_sp_to_pinyin("dk", selected), "dia");
            "dk".to_string()
        } else {
            let code = super::utils::encode_pinyin("den", selected)
                .unwrap_or_else(|| panic!("{kind:?} cannot encode den"));
            assert_eq!(
                super::utils::cvt_single_sp_to_pinyin(&code, selected),
                "den"
            );
            code
        };
        let mut engine = fixture.engine(kind);
        let candidates = engine.query(&request(&code));
        assert!(
            words(&candidates).contains(&"扽"),
            "{kind:?}: {code}: {:?}",
            words(&candidates)
        );
        if kind == ShuangpinProfileKind::Shoudao {
            assert!(
                words(&candidates).contains(&"嗲"),
                "手心原有的 `dia` 候选应保留"
            );
        }
    }
}

#[test]
fn warm_queries_do_not_allocate_a_compact_input_only_for_the_empty_check() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER); INSERT INTO tbl_1_n VALUES('ni','n','你',100); CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER); INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',200);",
    );
    let measured = [("ni", 8), ("ni'hc", 12), ("ni''hc'", 12)].map(|(raw, budget)| {
        let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
        let typed = request(raw);
        let expected = engine.query(&typed);
        assert!(!expected.is_empty(), "raw={raw:?}");
        engine.query(&typed);
        let (rows, allocations) =
            crate::ime::personal_rerank::allocations::count(|| engine.query(&typed));
        assert_eq!(rows, expected, "raw={raw:?}");
        (raw, allocations, budget)
    });
    assert!(
        measured
            .iter()
            .all(|(_, allocations, budget)| allocations <= budget),
        "{measured:?}"
    );
}

#[test]
fn delimiter_only_queries_return_without_allocating() {
    let fixture = Fixture::new("");
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    for raw in ["", "'", "'''", "''''''''"] {
        let typed = request(raw);
        let (rows, allocations) =
            crate::ime::personal_rerank::allocations::count(|| engine.query(&typed));
        assert!(rows.is_empty());
        assert_eq!(allocations, 0, "raw={raw:?}");
    }
}

#[test]
fn prefix_groups_list_a_word_once() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_n VALUES('ni', 'n', '你', 10000),('ni', 'n', '拟', 9000);",
    );
    let microsoft = profile(ShuangpinProfileKind::Microsoft).unwrap();
    let mut dictionary = super::dictionary::ShuangpinDictionary::new(microsoft, &fixture.paths);
    let segmentation = super::utils::pinyin_segmentation("ni'nni", microsoft);
    assert_eq!(segmentation, "ni'''nn'i");
    let rows = dictionary.generate_series("ni'nni", &segmentation, "");
    assert_eq!(words(&rows), ["你", "拟"]);
}

#[test]
fn prefix_groups_reserve_their_candidate_rows() {
    let mut sql = String::from(
        "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);",
    );
    for index in 0..10 {
        sql.push_str(&format!(
            "INSERT INTO tbl_1_n VALUES('ni','n','单{index}',{});INSERT INTO tbl_2_n VALUES('ni''hao','nh','双{index}',{});",
            1000 - index,
            2000 - index
        ));
    }
    let fixture = Fixture::new(&sql);
    let mut dictionary = super::dictionary::ShuangpinDictionary::new(
        profile(ShuangpinProfileKind::Xiaohe).unwrap(),
        &fixture.paths,
    );

    let segmentation =
        super::query::segment_input("nihcma", profile(ShuangpinProfileKind::Xiaohe).unwrap());
    let rows = dictionary.generate_series("nihcma", &segmentation, "");

    assert_eq!(rows.len(), 20, "{rows:?}");
    assert_eq!(rows.capacity(), 20);
}

const NIHAO_FIXTURE: &str = "CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '你好', 200);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '拟好', 100);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '𠀀方案𠮷', 90);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', 'C语言 2', 80);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', 'GitHub', 70);CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);INSERT INTO tbl_1_h VALUES('hao','h','好',100);CREATE TABLE tbl_3_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);";

#[test]
fn profiles_decode_their_own_codes() {
    let fixture = Fixture::new(
        "BEGIN;CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '你好', 100);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '拟好', 50);COMMIT;",
    );
    let mut ziranma = fixture.engine(ShuangpinProfileKind::Ziranma);
    let hk = ziranma.query(&request("nihk"));
    assert_eq!(words(&hk), ["你好", "拟好"]);
    assert!(hk.iter().all(|item| item.pinyin == "nihk"));
    let hc = ziranma.query(&request("nihc"));
    assert_eq!(words(&hc), ["你好", "拟好"]);
    assert!(hc.iter().all(|item| item.pinyin == "nih"));

    let mut microsoft = fixture.engine(ShuangpinProfileKind::Microsoft);
    assert!(microsoft.query(&request("b;")).is_empty());
    let semicolon = microsoft.query(&request("nihkb;"));
    assert_eq!(words(&semicolon), ["你好", "拟好"]);
    assert!(semicolon.iter().all(|item| item.pinyin == "nihk"));
    let split = microsoft.query(&request("nihcb;"));
    assert!(split.iter().all(|item| item.pinyin == "nih"));
}

#[test]
fn online_rows_follow_the_query_cache() {
    let fixture = Fixture::new(NIHAO_FIXTURE);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let cloud = vec!["妮好".to_string()];

    let plain = request("ni'hc");
    assert!(engine.insert_online_words(&plain, &cloud, CandidateSource::CloudSuggestion));
    assert_eq!(words(&engine.query(&plain)), ["妮好"]);

    let series = request("nihc");
    engine.query(&series);
    let ai: Vec<String> = ["甲", "乙", "甲"]
        .iter()
        .map(|word| word.to_string())
        .collect();
    assert!(engine.insert_online_words(&series, &ai, CandidateSource::AiSuggestion));
    let listed = engine.query(&series);
    assert_eq!(words(&listed)[..2], ["你好", "拟好"]);
    assert_eq!(words(&listed)[listed.len() - 2..], ["甲", "乙"]);
    assert!(!engine.insert_online_words(&series, &["".to_string()], CandidateSource::AiSuggestion));
}

#[test]
fn initial_letter_expands_past_the_cap() {
    let mut sql =
        String::from("BEGIN;CREATE TABLE tbl_1_s(key TEXT, jp TEXT, value TEXT, weight INTEGER);");
    for index in 0..30 {
        sql.push_str(&format!(
            "INSERT INTO tbl_1_s VALUES('shi', 's', '字{index:02}', {});",
            1000 - index
        ));
    }
    sql.push_str("INSERT INTO tbl_1_s VALUES('si', 's', '四', 1);COMMIT;");
    let fixture = Fixture::new(&sql);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let typed = request("u");
    let mut candidates = engine.query(&typed);
    assert_eq!(candidates.len(), 24);
    assert!(candidates
        .iter()
        .all(|item| item.pinyin == "u" && item.canonical_pinyin == "shi"));
    assert!(engine.expand_initial_candidates(&typed, &mut candidates));
    assert_eq!(candidates.len(), 30);
    assert!(!engine.expand_initial_candidates(&typed, &mut candidates));
    assert!(!engine.expand_initial_candidates(&request("ui"), &mut candidates));
}

#[test]
fn initial_letter_expansion_reuses_warm_candidate_buffer() {
    let mut sql =
        String::from("BEGIN;CREATE TABLE tbl_1_s(key TEXT, jp TEXT, value TEXT, weight INTEGER);");
    for index in 0..30 {
        sql.push_str(&format!(
            "INSERT INTO tbl_1_s VALUES('shi', 's', '字{index:02}', {});",
            1000 - index
        ));
    }
    sql.push_str("COMMIT;");
    let fixture = Fixture::new(&sql);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let typed = request("u");
    let mut candidates = engine.query(&typed);
    candidates.reserve(256);

    assert!(engine.expand_initial_candidates(&typed, &mut candidates));
    assert_eq!(candidates.len(), 30);
    assert!(
        candidates.capacity() >= 256,
        "预留候选容量不应在首字母展开时被丢弃：{}",
        candidates.capacity()
    );
}

#[test]
fn invalid_flat_initial_pairs_still_accept_enabled_fuzzy_partners() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_s VALUES('shuang','s','双',1000);\
CREATE TABLE tbl_2_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_s VALUES('shuang''ren','sr','双人',10000),('shuang''shuang','ss','双双',9000);",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    for input in ["slrf", "slsl", "sl'rf"] {
        let mut typed = request(input);
        let expected = if input == "slsl" { "双双" } else { "双人" };
        assert!(!words(&engine.query(&typed)).contains(&expected));
        typed.fuzzy_pinyin.rules = fuzzy_rule::S_SH;
        let rows = engine.query(&typed);
        let row = &rows[index_of(&rows, expected)];
        assert_eq!(row.pinyin.replace('\'', ""), input.replace('\'', ""));
        assert_eq!(
            row.canonical_pinyin,
            if input == "slsl" {
                "shuang'shuang"
            } else {
                "shuang'ren"
            }
        );
        assert!(row.fuzzy);
        typed.fuzzy_pinyin.rules = fuzzy_rule::Z_ZH;
        assert!(!words(&engine.query(&typed)).contains(&expected));
    }
    let mut separated = request("s'lrf");
    separated.fuzzy_pinyin.rules = fuzzy_rule::S_SH;
    assert!(!words(&engine.query(&separated)).contains(&"双人"));
}

#[test]
fn invalid_fuzzy_initials_use_each_profiles_keys() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_2_s VALUES('shuang''ren','sr','双人',10000),('shuang''ren','sr','双刃',9000);",
    );
    for kind in [
        ShuangpinProfileKind::Xiaohe,
        ShuangpinProfileKind::Ziranma,
        ShuangpinProfileKind::Shoudao,
        ShuangpinProfileKind::Microsoft,
        ShuangpinProfileKind::SmartAbc,
    ] {
        let mut input = super::utils::encode_pinyin("shuang'ren", profile(kind).unwrap()).unwrap();
        input.replace_range(..1, "s");
        let mut typed = request(&input);
        typed.fuzzy_pinyin.rules = fuzzy_rule::S_SH;
        let mut engine = fixture.engine(kind);
        let rows = engine.query(&typed);
        let row = &rows[index_of(&rows, "双人")];
        assert_eq!(row.pinyin.replace('\'', ""), input.replace('\'', ""));
        assert_eq!(row.canonical_pinyin, "shuang'ren");
        assert!(row.fuzzy);
    }
}

#[test]
fn invalid_fuzzy_initial_paths_share_dictionary_weights() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_n VALUES('nen','n','嫩',100);\
CREATE TABLE tbl_1_r(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
INSERT INTO tbl_1_r VALUES('ren','r','人',10000);",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("lf");
    typed.fuzzy_pinyin.rules = fuzzy_rule::N_L | fuzzy_rule::R_L;
    for _ in 0..2 {
        let rows = engine.query(&typed);
        assert_eq!(words(&rows), ["人", "嫩"]);
        assert!(rows.iter().all(|row| row.fuzzy));
    }
}

#[test]
fn fuzzy_rows_take_the_typed_keys() {
    let fixture = Fixture::new(
        "BEGIN;CREATE TABLE tbl_1_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_z VALUES('zi', 'z', '字', 50);INSERT INTO tbl_1_z VALUES('zhi', 'z', '之', 100);COMMIT;",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("zi");
    assert_eq!(words(&engine.query(&typed)), ["字"]);
    typed.fuzzy_pinyin = FuzzyPinyinOptions {
        rules: fuzzy_rule::Z_ZH,
    };
    let fuzzy = engine.query(&typed);
    assert_eq!(words(&fuzzy), ["之", "字"]);
    assert_eq!(fuzzy[0].pinyin, "zi");
    assert!(fuzzy[0].fuzzy);
}

#[test]
fn fuzzy_rows_reserve_unique_results_before_appending() {
    let mut sql = String::from(
        "CREATE TABLE tbl_1_z(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_z VALUES('zi','z','精确',100);",
    );
    for index in 0..20 {
        sql.push_str(&format!(
            "INSERT INTO tbl_1_z VALUES('zhi','z','模糊{index:02}',{});",
            99 - index
        ));
    }
    let fixture = Fixture::new(&sql);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("zi");
    typed.fuzzy_pinyin = FuzzyPinyinOptions {
        rules: fuzzy_rule::Z_ZH,
    };

    let fuzzy = engine.query(&typed);

    assert_eq!(fuzzy.len(), 21);
    assert_eq!(fuzzy.capacity(), fuzzy.len());
}

#[test]
fn empty_fuzzy_rows_keep_exact_order() {
    let fixture = Fixture::new(
        "BEGIN;CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_n VALUES('ni', 'n', '你', 100);CREATE TABLE tbl_2_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_2_n VALUES('ni''hao', 'nh', '你好', 200);COMMIT;",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("nihc");
    let exact = engine.query(&typed);
    typed.fuzzy_pinyin = FuzzyPinyinOptions {
        rules: fuzzy_rule::Z_ZH,
    };

    let with_empty_fuzzy = engine.query(&typed);

    assert_eq!(words(&exact), ["你好", "你"]);
    assert_eq!(words(&with_empty_fuzzy), words(&exact));
    assert!(with_empty_fuzzy.iter().all(|item| !item.fuzzy));
}

#[test]
fn find_candidate_reads_the_canonical_key() {
    let fixture = Fixture::new(NIHAO_FIXTURE);
    let engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let found = engine.find_candidate("ni'hao", "拟好").expect("row");
    assert_eq!(
        Some(found.weight),
        fixture.weight("tbl_2_n", "ni'hao", "拟好")
    );
    assert_eq!(found.pinyin, "ni'hao");
    assert_eq!(found.source, CandidateSource::Database);
    assert!(engine.find_candidate("ni'hao", "妮好").is_none());
}

#[test]
fn word_lattice_switch_controls_the_sentence_row() {
    let fixture = Fixture::new(
        "BEGIN;CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_n VALUES('ni', 'n', '你', 100);CREATE TABLE tbl_1_h(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_h VALUES('hao', 'h', '好', 100);COMMIT;",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("nihc");
    let with_lattice = engine.query(&typed);
    let sentence = with_lattice
        .iter()
        .find(|item| item.source == CandidateSource::Generated)
        .unwrap_or_else(|| panic!("no lattice row in {:?}", words(&with_lattice)));
    assert_eq!(sentence.word, "你好");
    assert!(sentence.sentence_association);
    assert_eq!(sentence.pinyin, "nihc");

    typed.sentence_association = SentenceAssociationOptions {
        word_lattice: false,
        ..SentenceAssociationOptions::default()
    };
    let without = engine.query(&typed);
    assert!(without
        .iter()
        .all(|item| item.source != CandidateSource::Generated));
    assert!(words(&without).contains(&"你"));
}

#[test]
fn exact_entry_leads_the_lattice_sentence() {
    let fixture = Fixture::new(
        "BEGIN;CREATE TABLE tbl_4_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_4_a VALUES('an''quan''bao''wei', 'aqbw', '安全保卫', 6000);CREATE TABLE tbl_2_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_2_a VALUES('an''quan', 'aq', '安全', 9000);CREATE TABLE tbl_2_b(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_2_b VALUES('bao''wei', 'bw', '包围', 9000);COMMIT;",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("anqrbcww");
    let best_only = engine.query(&typed);
    assert_eq!(words(&best_only).first(), Some(&"安全保卫"));
    assert!(best_only
        .iter()
        .all(|item| item.source != CandidateSource::Generated));
    typed.sentence_alternatives = true;
    let candidates = engine.query(&typed);
    assert_eq!(
        words(&candidates).first(),
        Some(&"安全保卫"),
        "{:?}",
        words(&candidates)
    );
    let sentence = candidates
        .iter()
        .find(|item| item.source == CandidateSource::Generated)
        .unwrap_or_else(|| panic!("no lattice row in {:?}", words(&candidates)));
    assert_eq!(sentence.word, "安全包围");
}

#[test]
fn missing_dictionary_answers_empty() {
    let root = tempfile::tempdir().expect("fixture directory");
    let directory: PathBuf = root.path().to_path_buf();
    let paths = RuntimePaths {
        resources: directory.clone(),
        user_data: directory.clone(),
        cache: directory.clone(),
        dictionaries: directory,
    };
    let mut engine = ShuangpinEngine::new(profile(ShuangpinProfileKind::Xiaohe).unwrap(), &paths);
    assert!(engine.query(&request("nihc")).is_empty());
    assert!(engine.query(&request("u")).is_empty());
    assert!(!paths.dictionary(assets::MAIN_DICTIONARY).exists());
    let mut invalid = request("nihc");
    invalid.valid = false;
    assert!(engine.query(&invalid).is_empty());
    assert!(engine.query(&request("'")).is_empty());
}

#[test]
fn real_dictionary_answers_common_readings() {
    let Some(resources) = std::env::var_os("MSIME_EVAL_RESOURCES") else {
        eprintln!("skipped: MSIME_EVAL_RESOURCES is not set to the dict-v2.0.1 resource directory");
        return;
    };
    let user = tempfile::tempdir().expect("user directory");
    let resources = PathBuf::from(resources);
    let paths = RuntimePaths {
        resources: resources.clone(),
        user_data: user.path().to_path_buf(),
        cache: user.path().to_path_buf(),
        dictionaries: resources,
    };
    let mut engine = ShuangpinEngine::new(profile(ShuangpinProfileKind::Xiaohe).unwrap(), &paths);
    let nihao = engine.query(&request("nihc"));
    assert!(
        words(&nihao).iter().take(3).any(|word| *word == "你好"),
        "{:?}",
        words(&nihao)
    );
    let zhongguo = engine.query(&request("vsgo"));
    assert!(
        words(&zhongguo).iter().take(3).any(|word| *word == "中国"),
        "{:?}",
        words(&zhongguo)
    );
}

#[test]
fn ziranma_real_dictionary_keeps_complete_phrases() {
    let Some(resources) = std::env::var_os("MSIME_EVAL_RESOURCES") else {
        eprintln!("跳过：未设置 MSIME_EVAL_RESOURCES，真实词库回归未执行");
        return;
    };
    let user = tempfile::tempdir().unwrap();
    let resources = PathBuf::from(resources);
    let paths = RuntimePaths {
        resources: resources.clone(),
        user_data: user.path().to_path_buf(),
        cache: user.path().to_path_buf(),
        dictionaries: resources,
    };
    let profile = profile(ShuangpinProfileKind::Ziranma).unwrap();
    let mut engine = ShuangpinEngine::new(profile, &paths);
    for _ in 0..2 {
        for (reading, expected) in [
            ("shu'ru'fa", "输入法"),
            ("kao'shi'fen'shu", "考试分数"),
            ("ni'xi'huan'wo'ma", "你喜欢我吗"),
        ] {
            let typed = super::utils::encode_pinyin(reading, profile)
                .unwrap()
                .replace('\'', "");
            let plain = engine.query(&request(&typed));
            assert_eq!(words(&plain)[0], expected, "普通读法：{typed}");
        }
    }
}

#[test]
fn context_changes_keep_the_caches_without_a_model() {
    let fixture = Fixture::new(NIHAO_FIXTURE);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("nihc");
    typed.rescoring_context = "上文".into();
    engine.query(&typed);
    assert!(engine.insert_online_words(&typed, &["甲".to_string()], CandidateSource::AiSuggestion));
    typed.rescoring_context = "另一段上文".into();
    assert!(words(&engine.query(&typed)).contains(&"甲"));
}

#[test]
fn a_changed_personal_model_drops_the_scored_series() {
    use crate::user_dictionary::ngram_store::PersonalNgramStore;
    let fixture = Fixture::new(
        "CREATE TABLE tbl_1_n(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_n VALUES('ni','n','你',10000),('ni','n','拟',9000);CREATE TABLE tbl_1_h(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_h VALUES('hao','h','好',10000),('hao','h','号',9000);",
    );
    let xiaohe = profile(ShuangpinProfileKind::Xiaohe).unwrap();
    let mut dictionary = super::dictionary::ShuangpinDictionary::new(xiaohe, &fixture.paths);
    let segmentation = super::utils::pinyin_segmentation("nihc", xiaohe);
    let before = dictionary.generate_series("nihc", &segmentation, "");
    let store = PersonalNgramStore::for_journal(&fixture.paths.user(assets::USER_JOURNAL));
    let version = store.version();
    let journal = Connection::open(fixture.paths.user(assets::USER_JOURNAL)).unwrap();
    crate::user_dictionary::journal::ensure_schema(&journal).unwrap();
    journal
        .execute_batch("INSERT INTO personal_bigram VALUES(char(1),'拟',400),('拟','号',400);INSERT INTO personal_trigram VALUES(char(1),'拟','号',400);")
        .unwrap();
    store.invalidate_for_tests();
    let after = dictionary.generate_series("nihc", &segmentation, "");
    assert!(
        store.version() > version,
        "the query did not reload the model"
    );
    assert!(
        words(&after).contains(&"拟号") && !words(&before).contains(&"拟号"),
        "the answer scored with the old model was kept: {:?} then {:?}",
        words(&before),
        words(&after)
    );
}

#[test]
fn association_switch_resets_the_series_cache() {
    let fixture = Fixture::new(NIHAO_FIXTURE);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut typed = request("nihc");
    engine.query(&typed);
    assert!(engine.insert_online_words(
        &typed,
        &["甲".to_string()],
        CandidateSource::CloudSuggestion
    ));
    assert!(words(&engine.query(&typed)).contains(&"甲"));
    typed.sentence_association = SentenceAssociationOptions {
        word_lattice: false,
        ..SentenceAssociationOptions::default()
    };
    assert!(!words(&engine.query(&typed)).contains(&"甲"));
    typed.sentence_association = SentenceAssociationOptions::default();
    assert!(!words(&engine.query(&typed)).contains(&"甲"));
}

const TYPO_FIXTURE: &str = "BEGIN;CREATE TABLE tbl_1_m(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_m VALUES('mei','m','没',1000),('mei','m','美',900);CREATE TABLE tbl_1_g(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_g VALUES('gen','g','跟',1000),('guan','g','关',500);CREATE TABLE tbl_1_x(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_x VALUES('xi','x','系',1000);CREATE TABLE tbl_1_c(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_1_c VALUES('ci','c','词',1000);CREATE TABLE tbl_2_g(key TEXT, jp TEXT, value TEXT, weight INTEGER);INSERT INTO tbl_2_g VALUES('guan''xi','gx','关系',100000);CREATE TABLE tbl_2_m(key TEXT, jp TEXT, value TEXT, weight INTEGER);CREATE TABLE tbl_3_m(key TEXT, jp TEXT, value TEXT, weight INTEGER);COMMIT;";

fn autocorrect_request(typed: &str, enabled: bool) -> QueryRequest {
    QueryRequest {
        enable_quanpin_autocorrect_transposition: enabled,
        enable_quanpin_autocorrect_neighbor: enabled,
        ..request(typed)
    }
}

fn typo_row(candidates: &[WordItem]) -> Option<&WordItem> {
    candidates
        .iter()
        .find(|item| item.sentence_association && !item.corrected_from.is_empty())
}

fn assert_corrected(candidates: &[WordItem], typed: &str) {
    let row = typo_row(candidates)
        .unwrap_or_else(|| panic!("no typo sentence for {typed} in {:?}", words(candidates)));
    assert_eq!(row.word, "没关系");
    assert_eq!(row.corrected_from, typed);
    assert_eq!(row.pinyin, typed);
    assert_eq!(row.canonical_pinyin, "mei'guan'xi");
    assert_eq!(row.source, CandidateSource::Generated);
    let literal = candidates
        .iter()
        .position(|item| item.sentence_association && item.corrected_from.is_empty())
        .unwrap_or_else(|| panic!("no literal sentence in {:?}", words(candidates)));
    assert!(
        literal < index_of(candidates, "没关系"),
        "{:?}",
        words(candidates)
    );
    assert!(candidates[0].corrected_from.is_empty());
}

#[test]
fn xiaohe_neighbour_key_typo_is_corrected() {
    let fixture = Fixture::new(TYPO_FIXTURE);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let corrected = engine.query(&autocorrect_request("mwgfxi", true));
    assert_corrected(&corrected, "mwgfxi");
    assert_eq!(corrected[0].word, "没跟系");
}

#[test]
fn ziranma_neighbour_key_typo_is_corrected() {
    let fixture = Fixture::new(TYPO_FIXTURE);
    let mut engine = fixture.engine(ShuangpinProfileKind::Ziranma);
    let corrected = engine.query(&autocorrect_request("mzgrci", true));
    assert_corrected(&corrected, "mzgrci");
}

#[test]
fn correct_input_keeps_its_first_candidate() {
    let fixture = Fixture::new(TYPO_FIXTURE);
    for (kind, correct, typo) in [
        (ShuangpinProfileKind::Xiaohe, "mwgrxi", "mwgfxi"),
        (ShuangpinProfileKind::Ziranma, "mzgrxi", "mzgrci"),
    ] {
        let mut engine = fixture.engine(kind);
        let on = engine.query(&autocorrect_request(correct, true));
        let mut plain = fixture.engine(kind);
        let off = plain.query(&autocorrect_request(correct, false));
        assert_eq!(on[0].word, "没关系", "{kind:?} {:?}", words(&on));
        assert_eq!(words(&on), words(&off), "{kind:?}");
        assert!(typo_row(&on).is_none(), "{kind:?} {:?}", words(&on));

        let switched_off = engine.query(&autocorrect_request(typo, false));
        assert!(typo_row(&switched_off).is_none(), "{kind:?}");
        assert!(!words(&switched_off).contains(&"没关系"), "{kind:?}");
        let delimited = format!("{}'{}", &typo[..2], &typo[2..]);
        let literal = engine.query(&autocorrect_request(&delimited, true));
        assert!(
            typo_row(&literal).is_none(),
            "{kind:?} {:?}",
            words(&literal)
        );
    }
}

#[test]
fn the_switch_is_read_on_every_query() {
    let fixture = Fixture::new(TYPO_FIXTURE);
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    assert!(typo_row(&engine.query(&autocorrect_request("mwgfxi", true))).is_some());
    assert!(typo_row(&engine.query(&autocorrect_request("mwgfxi", false))).is_none());
    assert!(typo_row(&engine.query(&autocorrect_request("mwgfxi", true))).is_some());
    let neighbor_only = QueryRequest {
        enable_quanpin_autocorrect_transposition: false,
        ..autocorrect_request("mwgfxi", true)
    };
    assert!(typo_row(&engine.query(&neighbor_only)).is_some());
}

#[test]
fn each_profile_corrects_its_own_codes() {
    let fixture = Fixture::new(TYPO_FIXTURE);
    let mut xiaohe = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut ziranma = fixture.engine(ShuangpinProfileKind::Ziranma);
    for _ in 0..2 {
        assert_corrected(
            &xiaohe.query(&autocorrect_request("mwgfxi", true)),
            "mwgfxi",
        );
        assert_corrected(
            &ziranma.query(&autocorrect_request("mzgrci", true)),
            "mzgrci",
        );
        let foreign = xiaohe.query(&autocorrect_request("mzgrci", true));
        assert!(
            !words(&foreign).contains(&"没关系"),
            "{:?}",
            words(&foreign)
        );
        let foreign = ziranma.query(&autocorrect_request("mwgfxi", true));
        assert!(
            !words(&foreign).contains(&"没关系"),
            "{:?}",
            words(&foreign)
        );
    }
}

#[test]
fn trailing_initial_completes_a_generated_sentence_with_canonical_reading() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',1000);\
         CREATE TABLE tbl_2_k(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_2_k VALUES('ke''ji','kj','科技',900);\
         CREATE TABLE tbl_1_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_1_g VALUES('gong','g','工',800);\
         CREATE TABLE tbl_2_g(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_2_g VALUES('gong''ju','gj','工具',700),\
         ('gong''ji','gj','公鸡',100),('gong''zuo','gz','工作',9000);",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    for _ in 0..2 {
        for typed in ["nihckejigsj", "ni'hc'ke'ji'gs'j"] {
            let rows = engine.query(&request(typed));
            assert_eq!(words(&rows)[0], "你好科技工具", "{typed}");
            assert_eq!(rows[0].canonical_pinyin, "ni'hao'ke'ji'gong'ju");
            assert_eq!(rows[0].pinyin, "nihckejigsj");
            assert!(rows[0].corrected_from.is_empty());
            assert_eq!(rows[0].source, CandidateSource::Generated);
            assert!(words(&rows).contains(&"你好科技工"));
            assert_eq!(
                rows.iter().filter(|row| row.word == "你好科技工具").count(),
                1
            );
        }
    }
}

#[test]
fn incomplete_tail_keeps_generated_prefix_and_its_input_span() {
    let fixture = Fixture::new(
        "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',1000);\
         CREATE TABLE tbl_2_k(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_2_k VALUES('ke''ji','kj','科技',900);\
         CREATE TABLE tbl_3_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_3_n VALUES('ni''hao''ke','nhk','你好可',800);\
         CREATE TABLE tbl_1_q(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_1_q VALUES('qing','q','清',700);",
    );
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    for _ in 0..2 {
        for (typed, expected, consumed) in [
            ("nihckeji", "你好科技", "nihckeji"),
            ("nihckejix", "你好科技", "nihckeji"),
            ("nihckejiqkl", "你好科技清", "nihckejiqk"),
            ("ni'hc'ke'ji'x", "你好科技", "nihckeji"),
            ("ni'hc'ke'ji'qk'l", "你好科技清", "nihckejiqk"),
        ] {
            let rows = engine.query(&request(typed));
            assert_eq!(words(&rows)[0], expected, "{typed}");
            assert_eq!(rows[0].source, CandidateSource::Generated);
            assert_eq!(rows[0].pinyin, consumed, "{typed}");
            assert_eq!(rows.iter().filter(|row| row.word == expected).count(), 1);
        }
    }
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let mut disabled = request("nihckejiq");
    disabled.sentence_association.word_lattice = false;
    let rows = engine.query(&disabled);
    assert_eq!(words(&rows)[0], "你好可");
    assert!(!rows
        .iter()
        .any(|row| row.source == CandidateSource::Generated));

    let rows = engine.query(&request("n'hc'ke'ji'q"));
    assert!(!rows
        .iter()
        .any(|row| row.source == CandidateSource::Generated));

    Connection::open(fixture.paths.dictionary(assets::MAIN_DICTIONARY))
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_5_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_5_n VALUES('ni''hao''ke''ji''qing','nhkjq','你好科技清',600);",
        )
        .unwrap();
    let mut engine = fixture.engine(ShuangpinProfileKind::Xiaohe);
    let rows = engine.query(&request("nihckejiq"));
    assert_eq!(words(&rows)[0], "你好科技清");
    assert_eq!(rows[0].source, CandidateSource::Database);
    assert_eq!(words(&rows)[1], "你好科技");
}
