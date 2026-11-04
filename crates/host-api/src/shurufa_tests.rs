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
        view = value(msime_client_character(handle, byte, false))["view"].clone();
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
        assert!(session.runtime.has_reranker(), "随包神经模型必须实际加载");
        assert_eq!(session.statistics_enabled(), Some(false));
    });
    let view = type_keys(handle, "nihao");
    assert_eq!(choose(handle, &view, "你好")["commit"], "你好");
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
    let mut legacy = msime_client_core::preferences::Preferences::default();
    legacy.paired_punctuation = true;
    legacy.smart_punctuation = true;
    legacy.smart_punctuation_repeat = true;
    legacy.smart_punctuation_space_convert = true;
    legacy.mixed_input.english = true;
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
            assert!(!session.options.mixed_english && !session.options.quick_phrase_candidates);
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
fn embedded_shurufa_options_default_missing_helpcode_off() {
    for supplied in [
        None,
        Some(json!({"schema":"ziranma","show_in_candidate_window":false})),
        Some(json!({"enabled":true,"schema":"ziranma","show_in_candidate_window":false})),
        Some(json!({"enabled":false,"schema":"ziranma","show_in_candidate_window":false})),
    ] {
        let mut document = json!({"api_version":1,"edition":"shurufa","resources":"/synthetic/resources",
            "user_data":"/synthetic/user","cache":"/synthetic/cache","dictionaries":"/synthetic/dictionaries","preferences":Preferences::for_edition(Edition::by_id("shurufa").unwrap())});
        if let Some(value) = &supplied {
            document["preferences"]["quanpin_helpcode"] = value.clone();
        } else {
            document["preferences"]
                .as_object_mut()
                .unwrap()
                .remove("quanpin_helpcode");
        }
        let options = HostOptions::from_document(document).unwrap();
        let expected = supplied
            .as_ref()
            .and_then(|value| value.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(options.preferences.quanpin_helpcode.enabled, expected);
    }
}
