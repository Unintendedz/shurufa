//! 用随包资源验证独立版，不访问已安装输入法的数据，也不向前台应用发送按键。
use super::*;
use std::ffi::{c_char, CString};

fn value(pointer: *mut c_char) -> Value {
    assert!(!pointer.is_null());
    // SAFETY: 响应由同一 Host API 分配，只在这里接收和释放一次。
    let response = unsafe { CString::from_raw(pointer) };
    let parsed: Value = serde_json::from_slice(response.as_bytes()).unwrap();
    assert_eq!(parsed["ok"], true, "{parsed}");
    parsed["value"].clone()
}

fn type_keys(handle: u64, keys: &str) -> Value {
    value(msime_client_command(handle, 3));
    let mut view = value(msime_client_view(handle));
    for byte in keys.bytes() {
        view = value(msime_client_character(
            handle,
            byte,
            byte.is_ascii_uppercase(),
        ))["view"]
            .clone();
    }
    view
}

fn choose(handle: u64, view: &Value, text: &str) -> Value {
    let candidate = view["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["text"] == text)
        .expect("目标候选必须在本页");
    value(msime_client_select(
        handle,
        view["generation"].as_u64().unwrap(),
        candidate["id"]["index"].as_u64().unwrap() as usize,
    ))
}

#[test]
fn bundled_input_learning_and_neural_model() {
    let Some(resources) = std::env::var_os("SHURUFA_TEST_RESOURCES") else {
        eprintln!("skipped: set SHURUFA_TEST_RESOURCES to the staged Shurufa resource directory");
        return;
    };
    let resources = std::fs::canonicalize(resources).unwrap();
    let state = tempfile::tempdir().unwrap();
    let request = json!({"resources": resources, "state_root": state.path(), "edition":"shurufa"})
        .to_string();
    // SAFETY: UTF-8 缓冲区在同步调用期间有效。
    let options = value(unsafe { msime_client_prepare_host(request.as_ptr(), request.len()) });
    assert_eq!(options["edition"], "shurufa");
    assert_eq!(options["preferences"]["usage_reporting"], false);
    assert_eq!(options["preferences"]["cloud_candidates"], false);
    assert_eq!(options["preferences"]["ai_assistant"]["enabled"], false);
    assert_eq!(
        options["preferences"]["voice_input"]["asr_provider"],
        "textstream"
    );
    assert_eq!(
        options["preferences"]["voice_input"]["polish_enabled"],
        true
    );
    assert_eq!(options["preferences"]["voice_input"]["polish_text"], true);
    assert_eq!(
        options["preferences"]["sentence_association"]["neural_keyboard"],
        true
    );
    let document = options.to_string();
    let created = value(unsafe { msime_client_create(document.as_ptr(), document.len()) });
    let handle = created["session"].as_u64().unwrap();
    value(msime_client_focus(handle, true));
    SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let session = sessions.get_mut(&handle).unwrap();
        assert_eq!(session.statistics_enabled(), Some(false));
    });
    let view = type_keys(handle, "nihao");
    assert_eq!(choose(handle, &view, "你好")["commit"], "你好");
    let dajia = type_keys(handle, "dajia");
    assert_eq!(dajia["candidates"][0]["text"], "大家");
    for typed in ["dj", "cd"] {
        let view = type_keys(handle, typed);
        assert_eq!(view["candidates"][0]["source"], 0, "{typed}: {view}");
        assert!(
            view["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["source"] == 4)
                .count()
                <= 1,
            "{typed} 两字母简拼最多只展示一条完整英文匹配：{view}"
        );
    }
    let conf = type_keys(handle, "conf");
    let conf_texts: Vec<&str> = conf["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect();
    assert!(
        conf_texts.contains(&"configuration"),
        "conf 首页应包含 configuration：{conf_texts:?}"
    );
    assert_eq!(value(msime_client_command(handle, 2))["commit"], "conf");
    let inter = type_keys(handle, "inter");
    assert_eq!(inter["candidates"][0]["text"], "inter");
    assert_eq!(inter["candidates"][1]["text"], "international");
    let interf = type_keys(handle, "interf");
    assert_eq!(choose(handle, &interf, "interface")["commit"], "interface");
    let cla = type_keys(handle, "Cla");
    assert_eq!(cla["candidates"][0]["text"], "Cla");
    assert_eq!(cla["candidates"][1]["text"], "Class");
    assert_eq!(value(msime_client_command(handle, 2))["commit"], "Cla");
    let clau = type_keys(handle, "Clau");
    assert_eq!(choose(handle, &clau, "Claude")["commit"], "Claude");
    println!("quanpin: nihao -> 你好; neural model attached; statistics disabled");

    let before = type_keys(handle, "shi");
    let rows = before["candidates"].as_array().unwrap();
    let learned = rows[2]["text"].as_str().unwrap().to_owned();
    for _ in 0..12 {
        let view = type_keys(handle, "shi");
        assert_eq!(choose(handle, &view, &learned)["commit"], learned);
    }
    let after = type_keys(handle, "shi");
    let rank = after["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .position(|candidate| candidate["text"] == learned)
        .unwrap();
    assert!(rank < 2, "学习后候选应从第三位前移，实际为 {}", rank + 1);
    println!(
        "learning: synthetic shi candidate moved from rank 3 to {}",
        rank + 1
    );
    value(msime_client_destroy(handle));

    let mut double = options;
    double["preferences"]["scheme"] = json!("shuangpin");
    double["preferences"]["shuangpin_profile"] = json!("xiaohe");
    let document = double.to_string();
    let created = value(unsafe { msime_client_create(document.as_ptr(), document.len()) });
    let handle = created["session"].as_u64().unwrap();
    value(msime_client_focus(handle, true));
    let view = type_keys(handle, "nihc");
    assert_eq!(choose(handle, &view, "你好")["commit"], "你好");
    println!("xiaohe shuangpin: nihc -> 你好");
    value(msime_client_destroy(handle));
}

#[test]
fn removed_input_modes_stay_unreachable_after_legacy_preferences_and_updates() {
    let root = tempfile::tempdir().unwrap();
    let path = |name| {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    };
    let resources = path("resources");
    let dictionaries = path("dictionaries");
    for directory in [&resources, &dictionaries] {
        rusqlite::Connection::open(directory.join("msime-pinyin.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
             INSERT INTO tbl_2_n VALUES('ni''hao','nh','拟好',100);
             CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);",
            )
            .unwrap();
    }
    let mut legacy = msime_client_core::preferences::Preferences {
        paired_punctuation: true,
        smart_punctuation: true,
        smart_punctuation_repeat: true,
        smart_punctuation_space_convert: true,
        ..Default::default()
    };
    legacy.mixed_input.enabled = false;
    legacy.mixed_input.english = false;
    legacy.mixed_input.first_page_count = 1;
    legacy.mixed_input.emoji = true;
    legacy.mixed_input.kaomoji = true;
    legacy.mixed_input.symbols = true;
    legacy.mixed_input.japanese = true;
    legacy.mixed_input.quick_phrase = true;
    legacy.local_modes.expression = true;
    legacy.local_modes.command = true;
    legacy.local_modes.mention = true;
    legacy.local_modes.chaizi = Some(true);
    legacy.navigation.mouse_wheel = false;
    let request = json!({"api_version":1,"edition":"shurufa","resources":resources,
        "dictionaries":dictionaries,"user_data":path("user"),"cache":path("cache"),"preferences":legacy}).to_string();
    // SAFETY: 合成配置缓冲区在同步调用期间有效。
    let created = value(unsafe { msime_client_create(request.as_ptr(), request.len()) });
    let handle = created["session"].as_u64().unwrap();
    value(msime_client_focus(handle, true));
    for revision in 0..2 {
        if revision > 0 {
            let snapshot =
                json!({"format_version":1,"revision":revision,"preferences":legacy}).to_string();
            // SAFETY: 合成偏好缓冲区在同步调用期间有效。
            value(unsafe {
                msime_client_update_preferences(handle, snapshot.as_ptr(), snapshot.len())
            });
        }
        SESSIONS.with(|sessions| {
            let sessions = sessions.borrow();
            let session = &sessions[&handle];
            assert!(
                !session.options.paired_punctuation && !session.options.local_shift_letter_entry
            );
            assert!(!session.options.local_temporary_english && !session.options.local_url);
            assert!(
                !session.options.local_date_time_inline && !session.options.local_date_time_menu
            );
            assert!(!session.options.local_command && !session.options.local_mention);
            assert!(
                session.options.local_expression
                    && session.options.local_expression_lowercase_entry
            );
            assert!(
                session.options.mixed_candidates
                    && session.options.mixed_english
                    && session.options.english_first_page_count == 3
                    && session.options.english_short_prefix_completion
            );
            assert!(
                !session.options.mixed_emoji
                    && !session.options.mixed_kaomoji
                    && !session.options.mixed_symbols
                    && !session.options.quick_phrase_candidates
            );
            assert!(!session.applied.smart_punctuation && !session.applied.word_character.enabled);
            assert!(session.applied.navigation.brackets && session.applied.navigation.mouse_wheel);
        });
        for key in b"UTKEMJYRVCS/@" {
            value(msime_client_command(handle, 3));
            let transition = value(msime_client_character(handle, *key, true));
            assert_eq!(transition["view"]["local_mode"], "none");
        }
        // 计算只从小写 `v` 进入。
        let view = type_keys(handle, "v1+2");
        assert_eq!(view["local_mode"], "expression");
        assert_eq!(view["candidates"][0]["text"], "3");
        let view = type_keys(handle, "www");
        assert!(!view["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["text"] == "www."));
        let marked = value(msime_client_character(handle, b'.', false));
        assert_eq!(marked["view"]["local_mode"], "none");
        value(msime_client_command(handle, 3));
        let composing = type_keys(handle, "nihao");
        let english = value(msime_client_set_english_mode(handle, true));
        assert_eq!(english["editing_text"], composing["editing_text"]);
        assert_eq!(english["dedicated_english"], false);
        value(msime_client_command(handle, 3));
        value(msime_client_set_paired_punctuation(handle, true));
        for preceding in ['1', 'a'] {
            let punctuation = value(msime_client_punctuation_with_context(
                handle,
                b',',
                preceding as u32,
            ));
            assert_eq!(punctuation["commit"], "，");
        }
        let quote = value(msime_client_punctuation_with_context(handle, b'(', 0));
        assert_eq!(quote["commit"], "（");
    }
    value(msime_client_destroy(handle));
}

#[test]
fn shurufa_english_autocomplete_gestures_and_helpcode_gate() {
    let root = tempfile::tempdir().unwrap();
    let path = |name| {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    };
    let resources = path("resources");
    let dictionaries = path("dictionaries");
    for directory in [&resources, &dictionaries] {
        rusqlite::Connection::open(directory.join("msime-pinyin.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);
                 INSERT INTO tbl_1_n VALUES('ni','n','你',300);
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
                 INSERT INTO tbl_2_c VALUES('cheng''du','cd','成都',300);
                 CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);
                 INSERT INTO quick_parases VALUES('conf','不应出现的短语',999);",
            )
            .unwrap();
        msime_engine::ensure_english_schema(&directory.join("msime-english.db")).unwrap();
        rusqlite::Connection::open(directory.join("msime-english.db"))
            .unwrap()
            .execute_batch(
                "INSERT INTO english_words(word,display,weight) VALUES
                 ('conf','conf',50),
                 ('config','config',250),
                 ('configuration','configuration',300),
                 ('inter','inter',100),
                 ('interface','interface',300),
                 ('claude','Claude',300),
                 ('class','class',250),
                 ('nice','nice',300),
                 ('dj','DJ',200),
                 ('django','Django',150),
                 ('cd','CD',200),
                 ('cdn','CDN',150);",
            )
            .unwrap();
    }
    let prefs = Preferences::for_edition(Edition::by_id("shurufa").unwrap());
    let request = json!({
        "api_version": 1,
        "edition": "shurufa",
        "resources": resources,
        "dictionaries": dictionaries,
        "user_data": path("user"),
        "cache": path("cache"),
        "preferences": prefs,
    })
    .to_string();
    // SAFETY: 合成配置缓冲区在同步调用期间有效。
    let created = value(unsafe { msime_client_create(request.as_ptr(), request.len()) });
    let handle = created["session"].as_u64().unwrap();
    let idle = value(msime_client_focus(handle, true))["view"].clone();
    assert_eq!(idle["editing_text"], "");
    assert_eq!(idle["candidates"].as_array().unwrap().len(), 0);

    // 中文全拼与常见简拼保持中文首选，两字母简拼不展开英文补全。
    for (typed, expected) in [
        ("nihao", "你好"),
        ("dajia", "大家"),
        ("dj", "大家"),
        ("cd", "成都"),
    ] {
        let view = type_keys(handle, typed);
        assert_eq!(view["candidates"][0]["text"], expected, "{typed}");
        assert_eq!(value(msime_client_command(handle, 1))["commit"], expected);
    }

    // 4 字母不可拆前缀：首页展示英文补全，且快捷短语不会混入；回车原样上屏，方向键加空格可提交补全词。
    let conf = type_keys(handle, "conf");
    let conf_words: Vec<&str> = conf["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect();
    assert!(conf_words.contains(&"configuration") && conf_words.contains(&"config"));
    assert!(!conf_words.contains(&"不应出现的短语"));
    assert_eq!(value(msime_client_command(handle, 2))["commit"], "conf");

    let conf = type_keys(handle, "conf");
    let target_pos = conf["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row["text"] == "configuration")
        .unwrap();
    for _ in 0..target_pos {
        value(msime_client_command(handle, 102));
    }
    assert_eq!(
        value(msime_client_command(handle, 1))["commit"],
        "configuration"
    );

    // 5 字母完全匹配领先并紧跟补全词；大写起手 `Cla` 保留原文居首且展示 `Claude`、`Class`。
    let inter = type_keys(handle, "inter");
    assert_eq!(inter["candidates"][0]["text"], "inter");
    assert_eq!(inter["candidates"][1]["text"], "interface");
    assert_eq!(choose(handle, &inter, "interface")["commit"], "interface");

    let cla = type_keys(handle, "Cla");
    assert_eq!(cla["candidates"][0]["text"], "Cla");
    assert_eq!(cla["candidates"][1]["text"], "Claude");
    assert_eq!(cla["candidates"][2]["text"], "Class");
    assert_eq!(value(msime_client_command(handle, 2))["commit"], "Cla");
    let cla = type_keys(handle, "Cla");
    assert_eq!(choose(handle, &cla, "Claude")["commit"], "Claude");

    // `niC` 走大写英文补全，不受辅助码分支干扰。
    let nic = type_keys(handle, "niC");
    assert_eq!(nic["candidates"][1]["text"], "niCe");
    value(msime_client_destroy(handle));
}

#[test]
fn shurufa_slot_allocator_projects_curated_inline_emoji_with_mru_and_clean_toggle() {
    let root = tempfile::tempdir().unwrap();
    let path = |name| {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    };
    let resources = path("resources");
    let dictionaries = path("dictionaries");
    let user_data = path("user");
    for directory in [&resources, &dictionaries] {
        rusqlite::Connection::open(directory.join("msime-pinyin.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE tbl_1_b(key TEXT,jp TEXT,value TEXT,weight INTEGER);
                 INSERT INTO tbl_1_b VALUES('bu','b','不',900000);
                 INSERT INTO tbl_1_b VALUES('bu','b','步',50000);
                 CREATE TABLE tbl_2_s(key TEXT,jp TEXT,value TEXT,weight INTEGER);
                 INSERT INTO tbl_2_s VALUES('si''kao','sk','思考',600000);
                 INSERT INTO tbl_2_s VALUES('si''kao','sk','思靠',20000);
                 INSERT INTO tbl_2_s VALUES('si''kao','sk','私考',10000);
                 CREATE TABLE tbl_2_d(key TEXT,jp TEXT,value TEXT,weight INTEGER);
                 INSERT INTO tbl_2_d VALUES('da''xiao','dx','大小',600000);
                 INSERT INTO tbl_2_d VALUES('da''xiao','dx','大笑',500000);
                 INSERT INTO tbl_2_d VALUES('da''xiao','dx','打小',20000);
                 INSERT INTO tbl_2_d VALUES('da''xiao','dx','打消',10000);
                 INSERT INTO tbl_2_d VALUES('da''xiao','dx','大校',9000);
                 INSERT INTO tbl_2_d VALUES('da''xiao','dx','大孝',8000);
                 CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);",
            )
            .unwrap();
    }
    let preferences = Preferences::for_edition(Edition::by_id("shurufa").unwrap());
    assert!(preferences.mixed_input.emoji);
    let request = json!({
        "api_version": 1,
        "edition": "shurufa",
        "resources": resources,
        "dictionaries": dictionaries,
        "user_data": user_data,
        "cache": path("cache"),
        "preferences": preferences,
    })
    .to_string();
    // SAFETY: 合成配置缓冲区在同步调用期间有效。
    let created = value(unsafe { msime_client_create(request.as_ptr(), request.len()) });
    let handle = created["session"].as_u64().unwrap();
    value(msime_client_focus(handle, true));

    // 引擎旧 queries.rs 混排仍关闭，内联 Emoji 完全经 SlotAllocator 投影。
    SESSIONS.with(|sessions| {
        let sessions = sessions.borrow();
        assert!(!sessions[&handle].options.mixed_emoji);
    });

    // `sikao` -> 第 1 位「思考」，第 3 位（下标 2）「🤔」，点选「🤔」直接上屏。
    let sikao = type_keys(handle, "sikao");
    assert_eq!(sikao["candidates"][0]["text"], "思考");
    assert_eq!(sikao["candidates"][1]["text"], "思靠");
    assert_eq!(sikao["candidates"][2]["text"], "🤔");
    assert_eq!(sikao["candidates"][2]["source"], 6);
    assert_eq!(choose(handle, &sikao, "🤔")["commit"], "🤔");

    // 高频虚字 `bu`（「不」）绝不触发内联 Emoji。
    let bu = type_keys(handle, "bu");
    assert!(!bu["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["source"] == 6));

    // `daxiao`：Top-2「大笑」在第 3 位投「😂」，第 2 页开头提供备选「🤣」，且原下标 2 的「打小」完整保留；
    // 选中「🤣」后本地 MRU 升至第 3 位。
    let daxiao = type_keys(handle, "daxiao");
    assert_eq!(daxiao["candidates"][0]["text"], "大小");
    assert_eq!(daxiao["candidates"][1]["text"], "大笑");
    assert_eq!(daxiao["candidates"][2]["text"], "😂");
    assert_eq!(daxiao["candidates"][3]["text"], "打小");
    let page2 = value(msime_client_command(handle, 100))["view"].clone();
    assert_eq!(page2["candidates"][0]["text"], "🤣");
    assert_eq!(choose(handle, &page2, "🤣")["commit"], "🤣");

    let daxiao_after = type_keys(handle, "daxiao");
    assert_eq!(daxiao_after["candidates"][0]["text"], "大小");
    assert_eq!(daxiao_after["candidates"][1]["text"], "大笑");
    assert_eq!(daxiao_after["candidates"][2]["text"], "🤣");
    assert_eq!(daxiao_after["candidates"][3]["text"], "打小");

    // 关闭开关后，Emoji 立即消失，中文候选相对顺序与关闭前主序逐条一致。
    let mut disabled = Preferences::for_edition(Edition::by_id("shurufa").unwrap());
    disabled.mixed_input.emoji = false;
    let snapshot = json!({
        "format_version": 1,
        "revision": 1,
        "preferences": disabled,
    })
    .to_string();
    // SAFETY: 合成偏好缓冲区在同步调用期间有效。
    value(unsafe { msime_client_update_preferences(handle, snapshot.as_ptr(), snapshot.len()) });
    let sikao_off = type_keys(handle, "sikao");
    let words_off: Vec<&str> = sikao_off["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect();
    assert_eq!(words_off, vec!["思考", "思靠", "私考"]);
    value(msime_client_destroy(handle));
}

#[test]
fn shurufa_emoji_projection_refresh_and_backspace_never_drop_or_miscommit_candidates() {
    let root = tempfile::tempdir().unwrap();
    let path = |name: &str| {
        let dir = root.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    };
    let resources = path("resources");
    let dictionaries = path("dictionaries");
    let user_data = path("user_data");
    {
        let db = rusqlite::Connection::open(
            std::path::Path::new(&dictionaries).join(msime_engine::assets::MAIN_DICTIONARY),
        )
        .unwrap();
        db.execute_batch(
            "CREATE TABLE tbl_1_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_c VALUES('chu','c','出',900000);\
             INSERT INTO tbl_1_c VALUES('chu','c','处',800000);\
             INSERT INTO tbl_1_c VALUES('chu','c','楚',700000);\
             INSERT INTO tbl_1_c VALUES('chu','c','除',600000);\
             INSERT INTO tbl_1_c VALUES('chu','c','初',500000);\
             INSERT INTO tbl_1_c VALUES('chu','c','触',400000);\
             CREATE TABLE tbl_2_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','出发',590000);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','处罚',580000);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','触发',550000);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','除法',10000);\
             CREATE TABLE quick_parases(key TEXT,value TEXT,weight INTEGER);",
        )
        .unwrap();
    }
    let preferences = Preferences::for_edition(Edition::by_id("shurufa").unwrap());
    let request = json!({
        "api_version": 1,
        "edition": "shurufa",
        "resources": resources,
        "dictionaries": dictionaries,
        "user_data": user_data,
        "cache": path("cache"),
        "preferences": preferences,
    })
    .to_string();
    // SAFETY: 合成配置缓冲区在同步调用期间有效。
    let created = value(unsafe { msime_client_create(request.as_ptr(), request.len()) });
    let handle = created["session"].as_u64().unwrap();
    value(msime_client_focus(handle, true));

    // `chuf` 投影 `🚀` 后继续键入 `a` -> `chufa`：「触发」完整保留在第 4 位（下标 3），非 Emoji 列表与关开关逐条一致。
    let chufa = type_keys(handle, "chufa");
    let chufa_on: Vec<&str> = chufa["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["source"] != 6)
        .map(|row| row["text"].as_str().unwrap())
        .collect();
    assert!(
        chufa_on.contains(&"触发"),
        "chufa 必须保留「触发」: {chufa_on:?}"
    );
    assert!(
        chufa_on.contains(&"除法"),
        "chufa 必须保留「除法」: {chufa_on:?}"
    );

    // `chuf` 退格到 `chu`：不能丢掉下标 2 的「处」，且选第 3 项「楚」必须精确上屏「楚」。
    let chuf = type_keys(handle, "chuf");
    assert_eq!(chuf["candidates"][2]["text"], "🚀");
    let chu = value(msime_client_command(handle, 0))["view"].clone();
    let chu_words: Vec<&str> = chu["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect();
    assert_eq!(&chu_words[..4], ["出", "处", "楚", "除"]);
    assert_eq!(choose(handle, &chu, "楚")["commit"], "楚");

    // 关掉内联 Emoji 后再打 `chufa`，主候选序列必须与开着时剥离 Emoji 后完全一致。
    let mut disabled = Preferences::for_edition(Edition::by_id("shurufa").unwrap());
    disabled.mixed_input.emoji = false;
    let snapshot = json!({
        "format_version": 1,
        "revision": 1,
        "preferences": disabled,
    })
    .to_string();
    // SAFETY: 合成偏好缓冲区在同步调用期间有效。
    value(unsafe { msime_client_update_preferences(handle, snapshot.as_ptr(), snapshot.len()) });
    let chufa_off_view = type_keys(handle, "chufa");
    let chufa_off: Vec<&str> = chufa_off_view["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect();
    assert_eq!(chufa_on, &chufa_off[..chufa_on.len()]);
    value(msime_client_destroy(handle));
}
