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
