use super::*;
use crate::quanpin::fixture::Fixture;

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    for (key, word, weight) in [
        ("fa", "发", 300),
        ("fan", "帆", 200),
        ("nou", "耨", 1),
        ("ou", "偶", 100),
        ("er", "尔", 100),
        ("ou'er", "偶尔", 1000),
    ] {
        fixture.insert(key, word, weight);
    }
    fixture
}

#[test]
fn literal_alternative_series_supplies_prefixes_and_sentences_on_cold_and_hot_queries() {
    let fixture = fixture();
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    for types in [0, TYPO_EDGE_TYPES] {
        for _ in 0..2 {
            let rows = dictionary.query("fanouer", "", types, FuzzyPinyinOptions::default());
            assert_eq!(dictionary.pinyin_segmentation, "fa'nou'er");
            let sentence = rows
                .iter()
                .find(|row| row.word == "帆偶尔")
                .expect("备选切分应组句");
            assert_eq!(sentence.pinyin, "fan'ou'er");
            assert_eq!(sentence.canonical_pinyin, "fan'ou'er");
            assert!(sentence.sentence_association);
            assert!(sentence.corrected_from.is_empty());
            let prefix = rows
                .iter()
                .find(|row| row.word == "帆")
                .expect("备选切分应查前缀");
            assert_eq!(prefix.pinyin, "fan");
            assert_eq!(prefix.canonical_pinyin, "fan");
            assert!(prefix.corrected_from.is_empty());
            assert!(
                rows.iter().position(|row| row.word == "帆偶尔").unwrap()
                    < rows.iter().position(|row| row.word == "帆").unwrap()
            );
            let mut words = HashSet::new();
            assert!(rows.iter().all(|row| words.insert(&row.word)));
        }
    }
}

#[test]
fn literal_alternative_series_respects_manual_boundaries_and_lattice_switch() {
    let fixture = fixture();
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    let rows = dictionary.query_exact("fa'nouer", "", 0);
    assert!(!rows
        .iter()
        .any(|row| row.word == "帆" || row.word == "帆偶尔"));
    dictionary.set_sentence_association(SentenceAssociationOptions {
        word_lattice: false,
        ..SentenceAssociationOptions::default()
    });
    let rows = dictionary.query_exact("fanouer", "", 0);
    assert!(rows.iter().any(|row| row.word == "帆"));
    assert!(!rows.iter().any(|row| row.word == "帆偶尔"));
}

#[test]
fn literal_alternative_series_keeps_complete_dictionary_words_before_generated_sentences() {
    let fixture = fixture();
    fixture.insert("qia'nou'er", "恰耨尔", 3000);
    fixture.insert("qi'a'nou'er", "旗阿耨尔", 2000);
    fixture.insert("qian", "签", 200);
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    let rows = dictionary.query_exact("qianouer", "", 0);
    assert_eq!(rows[0].word, "恰耨尔");
    assert_eq!(rows[1].word, "旗阿耨尔");
    let sentence = rows
        .iter()
        .position(|row| row.word == "签偶尔")
        .expect("完整词存在时仍查备选整句");
    assert!(sentence > 1);
}

#[test]
fn literal_alternative_series_supports_different_syllable_counts_and_boundaries() {
    for (raw, canonical, expected, entries) in [
        (
            "qiefeihong",
            "qi'e'fei'hong",
            "旗鹅飞虹",
            [("qi'e", "旗鹅"), ("fei'hong", "飞虹")],
        ),
        (
            "shananfei",
            "shan'an'fei",
            "山安飞",
            [("shan", "山"), ("an'fei", "安飞")],
        ),
        (
            "quxianqie",
            "qu'xi'an'qie",
            "曲西岸切",
            [("qu", "曲"), ("xi'an'qie", "西岸切")],
        ),
    ] {
        let fixture = Fixture::new();
        for (key, word) in entries {
            fixture.insert(key, word, 100);
        }
        let mut dictionary = QuanpinDictionary::new(&fixture.paths);
        let rows = dictionary.query_exact(raw, "", 0);
        let sentence = rows
            .iter()
            .find(|row| row.word == expected)
            .expect("不同合法切分均可组句");
        assert_eq!(sentence.pinyin, canonical);
        assert_eq!(sentence.canonical_pinyin, canonical);
    }
}

#[test]
fn literal_alternative_series_registers_personal_scoring_for_single_syllable_primary() {
    let fixture = Fixture::new();
    fixture.insert("xi", "西", 100).insert("an", "岸", 100);
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    let rows = dictionary.query_exact("xian", "", 0);
    assert!(rows.iter().any(|row| row.word == "西岸"));
    let slot = dictionary.series_slot(&series_cache_key("xian", "xian", 0));
    assert!(dictionary.personal_scored_keys.contains(&slot));
    dictionary
        .personal
        .record(&[PersonalTransition {
            earlier: String::new(),
            previous: "西".into(),
            word: "岸".into(),
            times: 1,
        }])
        .unwrap();
    dictionary.drop_personal_scored_results();
    assert!(!dictionary.series_cache.contains(&slot));
    assert!(dictionary
        .query_exact("xian", "", 0)
        .iter()
        .any(|row| row.word == "西岸"));
}

#[test]
fn literal_alternative_series_deduplicates_by_coverage_without_comparing_sentence_scores() {
    let row = |pinyin: &str, word: &str, weight: i64, source| {
        WordItem::new(pinyin, word, weight, source, pinyin)
    };
    let mut rows = vec![
        row("fan'ou'er", "合成整词", 1, CandidateSource::Database),
        row("fan", "合成同词", 10000, CandidateSource::Database),
    ];
    merge_series_by_coverage(
        &mut rows,
        vec![
            row("fan'ou'er", "合成同词", -100, CandidateSource::Generated),
            row("fan'ou'er", "合成整句", 9000, CandidateSource::Generated),
        ],
    );
    assert_eq!(
        rows.iter().map(|row| row.word.as_str()).collect::<Vec<_>>(),
        ["合成整词", "合成同词", "合成整句"]
    );
    assert_eq!(rows[1].pinyin, "fan'ou'er");
    assert_eq!(rows[1].canonical_pinyin, "fan'ou'er");
}

#[test]
fn literal_alternative_series_reuses_prefix_rows_across_dense_paths() {
    let fixture = Fixture::new();
    fixture
        .insert("xian", "先", 100)
        .insert("xi'an", "西岸", 90)
        .insert("xi", "西", 80)
        .insert("an", "岸", 80);
    let raw = "xianxianxianxian";
    let primary = "xian'xian'xian'xian";
    let segments = vec!["xian".to_owned(); 4];
    let resolution = resolve_series_query(raw, &segments, 0);
    let alternatives = alternative_segmentations(raw, &segments, &resolution, 0);
    assert!(alternatives.len() >= 8);
    let mut before = QuanpinDictionary::new(&fixture.paths);
    let mut after = QuanpinDictionary::new(&fixture.paths);
    // 对照保留逐路径查询；预热共享表和行缓存，计数只比较重复前缀的字段复制。
    let unshared = |dictionary: &mut QuanpinDictionary, primary_rows| {
        let mut rows =
            dictionary.merge_alternative_full(raw, primary, &segments, &alternatives, primary_rows);
        let mut supplemental = Vec::new();
        let mut alt_sentences = Vec::new();
        let legacy_phrase_bonus_milli =
            (crate::lattice::decode::PHRASE_LENGTH_BONUS * 1000.0).round() as i64;
        let split_cost_milli =
            (crate::pinyin::graph::DAG_HIATUS_SPLIT_COST * 1000.0).round() as i64;
        for path in &alternatives {
            let rows_for_path = dictionary.query_series_with_typos(
                raw,
                &join_segments(path),
                path,
                false,
                None,
                None,
            );
            let extra = path.len().saturating_sub(segments.len());
            let adjust = (extra as i64)
                .saturating_mul(split_cost_milli.saturating_sub(legacy_phrase_bonus_milli));
            for mut item in rows_for_path {
                if item.sentence_association {
                    if extra > 0 {
                        item.weight = item.weight.saturating_add(adjust);
                    }
                    alt_sentences.push(item);
                } else {
                    supplemental.push(item);
                }
            }
        }
        merge_series_by_coverage(&mut rows, supplemental);
        if !alt_sentences.is_empty() {
            merge_alternative_sentences_by_score(&mut rows, raw, &segments, alt_sentences);
        }
        rows
    };
    let primary_rows = before.query_series(raw, primary, &segments);
    drop(unshared(&mut before, primary_rows));
    let primary_rows = after.query_series(raw, primary, &segments);
    drop(after.merge_alternatives(raw, primary, &segments, &alternatives, primary_rows));
    let old_primary = before.query_series(raw, primary, &segments);
    let new_primary = after.query_series(raw, primary, &segments);
    let (expected, old) =
        crate::ime::personal_rerank::allocations::count(|| unshared(&mut before, old_primary));
    let (actual, new) = crate::ime::personal_rerank::allocations::count(|| {
        after.merge_alternatives(raw, primary, &segments, &alternatives, new_primary)
    });
    assert_eq!(actual, expected);
    // 行字段改成 `SharedString` 后，逐路径查询的对照克隆重复前缀行只加引用计数，共享前缀不再省下申请；剩下的差额是前缀表 `queried_prefixes` 自身的哈希表和键。
    assert_eq!(new, old + 15, "共享前缀的额外申请变化：{old}→{new}");
}

#[test]
fn literal_alternative_series_shared_prefixes_keep_the_original_sparse_threshold() {
    let fixture = Fixture::new();
    for word in "甲乙丙丁戊己庚辛".chars() {
        fixture.insert("dia", &word.to_string(), 100);
    }
    fixture.insert("di", "低", 100);
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    dictionary.set_sentence_association(SentenceAssociationOptions {
        word_lattice: false,
        ..SentenceAssociationOptions::default()
    });
    let segments = ["dia", "xi", "an"].map(str::to_owned);
    let control =
        dictionary.query_series_with_typos("diaxian", "dia'xi'an", &segments, false, None, None);
    assert_eq!(control.len(), 8);
    let mut queried = HashMap::from([("dia".to_owned(), control.len())]);
    let shared = dictionary.query_series_with_typos(
        "diaxian",
        "dia'xi'an",
        &segments,
        false,
        Some(&mut queried),
        None,
    );
    assert!(
        shared.is_empty(),
        "已在主路径显示的八行仍满足稀疏门槛，不能额外回退到 di"
    );
}

#[test]
fn extra_syllable_literal_cut_sentences_follow_primary_runner_ups_with_length_deduction() {
    let fixture = Fixture::new();
    for (key, word, weight) in [
        ("zhuan", "转", 500),
        ("zhuan", "专", 400),
        ("ba", "巴", 500),
        ("zhu", "主", 450),
        ("an", "案", 450),
        ("an'ba", "案吧", 600),
    ] {
        fixture.insert(key, word, weight);
    }
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    let rows = dictionary.query_exact("zhuanba", "", 0);
    assert_eq!(dictionary.pinyin_segmentation, "zhuan'ba");

    let primary_pos = rows
        .iter()
        .position(|row| row.word == "转巴" && row.sentence_association)
        .expect("主切分应生成整句");
    let prefix_pos = rows
        .iter()
        .position(|row| row.word == "转" && !row.sentence_association)
        .expect("主切分应包含前缀词");
    let extra_cut = rows
        .iter()
        .position(|row| row.word == "主案吧" && row.sentence_association)
        .expect("多拆音节的字面备选切分仍应生成整句");

    assert!(
        primary_pos < prefix_pos && extra_cut < prefix_pos,
        "主切分与零声母备选整句均应在主切分前缀之前平等竞争: {rows:?}"
    );
    let extra_row = &rows[extra_cut];
    let raw_segments = ["zhu", "an", "ba"].map(str::to_owned);
    let raw_extra_rows = QuanpinDictionary::new(&fixture.paths).query_series_with_typos(
        "zhuanba",
        "zhu'an'ba",
        &raw_segments,
        false,
        None,
        None,
    );
    let raw_extra = raw_extra_rows
        .iter()
        .find(|row| row.word == "主案吧" && row.sentence_association)
        .expect("直接按备选切分查询应得到未扣分的整句");
    let expected_channel_deduction = (-crate::pinyin::graph::DAG_HIATUS_SPLIT_COST * 1000.0).round()
        as i64
        + (crate::lattice::decode::PHRASE_LENGTH_BONUS * 1000.0).round() as i64;
    assert_eq!(extra_row.extra_syllables, 0);
    assert!(!extra_row.keyboard_runner_up);
    assert_eq!(
        raw_extra.weight - extra_row.weight,
        expected_channel_deduction,
        "多出 1 个零声母切分边界应仅扣除小额真实信道代价（-0.60 nats），不再施加 -30 nats 死刑"
    );

    let model = Arc::new(crate::lattice::merge::block_buffer_tests::favouring_model(
        &['转', '专', '巴', '主', '案', '吧'],
        &['转'],
    ));
    dictionary
        .rerankers
        .push(NeuralReranker::new(CandidateSource::NeuralKeyboard, model));
    dictionary.reset_cache();
    let reranked_rows = dictionary.query_exact("zhuanba", "", 0);
    let reranked_prefix_pos = reranked_rows
        .iter()
        .position(|row| row.word == "转" && !row.sentence_association)
        .expect("启用重排器后主切分应包含前缀词");
    let reranked_extra_cut = reranked_rows
        .iter()
        .position(|row| row.word == "主案吧" && row.sentence_association)
        .expect("启用重排器后多拆音节备选整句仍应保留");
    assert!(
        reranked_extra_cut < reranked_prefix_pos,
        "启用重排器后零声母备选整句应与主切分整句一同留在前缀词之前平等竞争: {reranked_rows:?}"
    );
    let reranked_extra = &reranked_rows[reranked_extra_cut];
    assert!(!reranked_extra.keyboard_runner_up);
    assert_eq!(reranked_extra.extra_syllables, 0);
    assert_eq!(
        raw_extra.weight - reranked_extra.weight,
        expected_channel_deduction
    );
}

#[test]
fn mixed_jianpin_and_competing_typo_cuts_produce_full_span_candidates() {
    let fixture = Fixture::new();
    for (key, word, weight) in [
        ("wo", "我", 5_000_000),
        ("wo'xiang'qu", "我想去", 300_000),
        ("chi'fan", "吃饭", 400_000),
        ("ni'zen'me", "你怎么", 200_000),
        ("hai'mei", "还没", 300_000),
        ("lai", "来", 2_000_000),
        ("ming'tian", "明天", 600_000),
        ("zai'shuo", "再说", 300_000),
        ("zao'shang", "早上", 350_000),
        ("xie'xie", "谢谢", 800_000),
        ("mei'wen'ti", "没问题", 500_000),
        ("zhe'ge", "这个", 1_000_000),
        ("zu'ge", "阻隔", 200),
        ("xie", "些", 100_000),
        ("xi", "西", 100_000),
        ("mei", "没", 500_000),
        ("wen", "问", 300_000),
        ("ta", "他", 500_000),
        ("jia", "家", 500_000),
    ] {
        fixture.insert(key, word, weight);
    }
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    let all = autocorrect_type::TRANSPOSITION
        | autocorrect_type::NEIGHBOR
        | autocorrect_type::MISSING_OR_EXTRA;

    let wxq = dictionary.query("wxiangquchifan", "", all, FuzzyPinyinOptions::default());
    assert_eq!(wxq[0].word, "我想去吃饭", "{wxq:?}");
    assert_eq!(wxq[0].canonical_pinyin, "wo'xiang'qu'chi'fan");

    let nzm = dictionary.query("nzmhaimeilai", "", all, FuzzyPinyinOptions::default());
    assert_eq!(nzm[0].word, "你怎么还没来", "{nzm:?}");

    let mt = dictionary.query("mingtianzs", "", all, FuzzyPinyinOptions::default());
    assert!(mt.iter().any(|row| row.word == "明天再说"), "{mt:?}");
    assert!(mt.iter().any(|row| row.word == "明天早上"), "{mt:?}");

    let xiexiw = dictionary.query("xiexiw", "", all, FuzzyPinyinOptions::default());
    assert_eq!(xiexiw[0].word, "谢谢", "{xiexiw:?}");
    assert_eq!(xiexiw[0].corrected_from, "xiexiw");

    let meiwentj = dictionary.query("meiwentj", "", all, FuzzyPinyinOptions::default());
    assert_eq!(meiwentj[0].word, "没问题", "{meiwentj:?}");
    assert_eq!(meiwentj[0].corrected_from, "meiwentj");

    // 真实简拼尾 `zheg -> 这个` 绝不能被邻键误翻成 `阻隔`。
    let zheg = dictionary.query("zheg", "", all, FuzzyPinyinOptions::default());
    assert_eq!(zheg[0].word, "这个", "{zheg:?}");
}

#[test]
fn full_span_typo_upgrades_existing_strict_prefix_row_even_as_runner_up() {
    let fixture = Fixture::new();
    for (key, word, weight) in [
        ("mei'wen", "美文", 500_000),
        ("tui'jian", "推荐", 500_000),
        // 模拟三字词表占位权重较低且已有严格前缀 `mei'wen't` 命中「没问题」的场景。
        ("mei'wen'ti", "没问题", 100),
        ("mei", "没", 2_000_000),
        ("wen'ti", "问题", 500_000),
    ] {
        fixture.insert(key, word, weight);
    }
    let mut dictionary = QuanpinDictionary::new(&fixture.paths);
    let all = autocorrect_type::TRANSPOSITION
        | autocorrect_type::NEIGHBOR
        | autocorrect_type::MISSING_OR_EXTRA;
    let rows = dictionary.query("meiwentj", "", all, FuzzyPinyinOptions::default());
    let pos = rows
        .iter()
        .position(|row| row.word == "没问题")
        .expect("没问题必须留在前排候选池中");
    assert!(pos <= 4, "没问题应位于前排竞争区: {rows:?}");
    assert_eq!(
        rows[pos].corrected_from, "meiwentj",
        "严格前缀 `mei'wen't` 必须升级为消费完整输入的全跨度纠错行: {rows:?}"
    );
    assert_eq!(rows[pos].canonical_pinyin, "mei'wen'ti");
    assert_eq!(rows[pos].source, CandidateSource::Database);
}
