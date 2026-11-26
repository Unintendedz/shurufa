//! Unit tests for the parent module, in their own file because the module
//! is large enough that mixing them with the implementation obscured both.
//! Same `mod tests` as before, so `use super::*` still names the parent.

use super::runtime::empty_result;
use super::*;
use serde_json::json;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
#[derive(Default)]
struct Fixture {
    scheme: u8,
    dedicated_english: bool,
    local_mode: String,
    words: Vec<String>,
    codes: Vec<String>,
    answers_key: Vec<bool>,
    text: String,
    snapshot_fails: bool,
    balanced_openings: Vec<u8>,
    cache_resets: usize,
    context_resets: usize,
    withheld: Vec<String>,
    sources: Vec<u8>,
    remaining_after_select: Option<String>,
    reading: String,
    spelling_symbols: String,
    microsoft_shuangpin: bool,
    first_page_rows: usize,
    edge_commits: Option<[String; 2]>,
}

#[cfg(unix)]
fn private_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[cfg(unix)]
#[test]
fn provider_connect_rejects_untrusted_filesystem_endpoints() {
    let root = private_tempdir();
    let socket = root.path().join("provider.sock");
    std::fs::write(&socket, b"synthetic").unwrap();
    assert!(UnixSocketProvider::new(&socket).connect().is_none());
    std::fs::remove_file(&socket).unwrap();

    let target = root.path().join("target.sock");
    let listener = UnixListener::bind(&target).unwrap();
    let alias = root.path().join("alias.sock");
    msime_path_trust::untrusted_symlink(&target, &alias).unwrap();
    assert!(UnixSocketProvider::new(&alias).connect().is_none());
    drop(listener);

    let outside = root.path().join("outside");
    let outside_nested = outside.join("nested");
    std::fs::create_dir_all(&outside_nested).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&outside_nested, std::fs::Permissions::from_mode(0o700)).unwrap();
    let outside_socket = outside_nested.join("provider.sock");
    let listener = UnixListener::bind(&outside_socket).unwrap();
    let inside = root.path().join("inside");
    std::fs::create_dir(&inside).unwrap();
    msime_path_trust::untrusted_symlink(&outside, inside.join("linked")).unwrap();
    assert!(
        UnixSocketProvider::new(inside.join("linked/nested/provider.sock"))
            .connect()
            .is_none()
    );
    drop(listener);

    let socket = root.path().join("private.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(UnixSocketProvider::new(&socket).connect().is_none());
    drop(listener);
}

#[cfg(unix)]
#[test]
fn cloud_dictionary_provider_forwards_bounded_request() {
    let directory = private_tempdir();
    let socket = directory.path().join("cloud-dictionary.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["version"], 1);
        assert_eq!(request["kind"], "cloud_dictionary");
        assert_eq!(request["request"]["operation"], "changes");
        let mut stream = stream;
        std::io::Write::write_all(&mut stream, br#"{"changes":[],"next":0}"#).unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let request = json!({"operation":"changes","after":0,"limit":1});
    let response = UnixSocketProvider::new(socket)
        .cloud_dictionary(request)
        .unwrap();
    assert_eq!(response["next"], 0);
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn credential_test_provider_keeps_request_and_response_bounded() {
    let directory = private_tempdir();
    let socket = directory.path().join("online.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["version"], 1);
        assert_eq!(request["kind"], "credential_test");
        assert_eq!(request["query"]["service"], "ai.assistant");
        assert_eq!(request["query"]["config"]["provider"], "deepseek");
        let mut stream = stream;
        std::io::Write::write_all(
            &mut stream,
            br#"{"ok":true,"message":"configuration accepted"}"#,
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let response = UnixSocketProvider::new(socket)
        .test_credential("ai.assistant", &json!({"provider":"deepseek"}))
        .unwrap();
    assert!(response.ok);
    assert_eq!(response.message, "configuration accepted");
    server.join().unwrap();

    assert!(
        UnixSocketProvider::new(directory.path().join("missing.sock"))
            .test_credential("unknown", &json!({}))
            .is_none()
    );
    assert!(
        UnixSocketProvider::new(directory.path().join("missing.sock"))
            .test_credential("voice.asr", &json!({"value":"x".repeat(16_384)}))
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn online_provider_forwards_the_ai_cache_probe_flag() {
    let mut query: OnlineQuery = serde_json::from_value(json!({
        "scheme": 0, "generation": 3, "identity": "identity", "query_text": "nihao",
        "cache_key": "cache", "pinyin_segments": ["ni", "hao"], "cloud_eligible": true,
        "ai_eligible": true, "cloud_candidates": false, "session_id": 5,
        "ai_assistant": {"enabled": true, "provider": "synthetic", "model": "synthetic-model",
                         "endpoint": "https://ai.invalid/v1/chat/completions"},
    }))
    .unwrap();
    // Absent in every document a host already produces, so only the Linux probe carries it.
    assert!(!query.ai_cache_only);
    assert!(serde_json::to_value(&query)
        .unwrap()
        .get("ai_cache_only")
        .is_none());
    query.ai_cache_only = true;

    let directory = private_tempdir();
    let socket = directory.path().join("online.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["kind"], "online");
        assert_eq!(request["query"]["ai_cache_only"], true);
        let mut stream = stream;
        std::io::Write::write_all(
            &mut stream,
            "{\"candidates\":[{\"text\":\"你好\",\"source\":1}]}\n".as_bytes(),
        )
        .unwrap();
    });
    assert_eq!(
        UnixSocketProvider::new(socket).query_candidates(query),
        Some(vec![("你好".to_owned(), 1)])
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn online_provider_deduplicates_before_enforcing_source_quota() {
    let directory = private_tempdir();
    let socket = directory.path().join("online.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut line,
        )
        .unwrap();
        std::io::Write::write_all(
            &mut stream,
            "{\"candidates\":[{\"text\":\"重复\",\"source\":1},{\"text\":\"重复\",\"source\":1},{\"text\":\"甲\",\"source\":1},{\"text\":\"乙\",\"source\":1}]}\n".as_bytes(),
        )
        .unwrap();
    });
    let query: OnlineQuery = serde_json::from_value(json!({
        "scheme": 0,
        "generation": 1,
        "identity": "identity",
        "query_text": "nihao",
        "cache_key": "cache",
        "pinyin_segments": ["ni", "hao"],
        "cloud_eligible": true,
        "cloud_candidates": false,
        "ai_eligible": true,
        "session_id": 5,
        "ai_assistant": {
            "enabled": true,
            "provider": "synthetic",
            "model": "synthetic-model",
            "endpoint": "https://ai.invalid/v1/chat/completions",
            "candidate_limit": 3
        }
    }))
    .unwrap();
    assert_eq!(
        UnixSocketProvider::new(socket).query_candidates(query),
        Some(vec![
            ("重复".to_owned(), 1),
            ("甲".to_owned(), 1),
            ("乙".to_owned(), 1),
        ])
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn online_provider_uses_shared_custom_response_parser() {
    use msime_client_core::cloud::candidates::{
        build_request, CloudCandidatePreferences, CloudCandidateSource, CloudResponseFormat,
    };
    let directory = private_tempdir();
    let socket = directory.path().join("online.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut line,
        )
        .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["query"]["cloud_request"]["response_path"], "data.0");
        let reply = json!({"candidates": [], "cloud_response": r#"{"data":["合成套接字候选"]}"#})
            .to_string()
            + "\n";
        std::io::Write::write_all(&mut stream, reply.as_bytes()).unwrap();
    });
    let mut query: OnlineQuery = serde_json::from_value(json!({
        "scheme": 0, "generation": 1, "identity": "synthetic", "query_text": "nihao",
        "cache_key": "synthetic", "pinyin_segments": ["ni", "hao"], "cloud_eligible": true,
        "cloud_candidates": true, "ai_eligible": false, "session_id": 5
    }))
    .unwrap();
    query.cloud_request = build_request(
        &CloudCandidatePreferences {
            source: CloudCandidateSource::Custom,
            endpoint: "https://cloud.example.invalid/?q={query}".into(),
            response_format: CloudResponseFormat::JsonPath,
            cache_enabled: false,
            ..Default::default()
        },
        "nihao",
        false,
    );
    assert_eq!(
        UnixSocketProvider::new(socket).query_candidates(query),
        Some(vec![("合成套接字候选".into(), 0)])
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn online_provider_filtered_sources_keep_empty_result_unallocated() {
    let directory = private_tempdir();
    let socket = directory.path().join("online.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut line,
        )
        .unwrap();
        std::io::Write::write_all(
            &mut stream,
            br#"{"candidates":[{"text":"cloud","source":0}]}"#,
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let query: OnlineQuery = serde_json::from_value(json!({
        "scheme": 0,
        "generation": 1,
        "identity": "identity",
        "query_text": "nihao",
        "cache_key": "cache",
        "pinyin_segments": ["ni", "hao"],
        "cloud_eligible": true,
        "cloud_candidates": false,
        "ai_eligible": false,
        "session_id": 5
    }))
    .unwrap();
    let result = UnixSocketProvider::new(socket)
        .query_candidates(query)
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(result.capacity(), 0);
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn online_provider_rejects_control_characters_in_query_fields() {
    let directory = private_tempdir();
    let socket = directory.path().join("missing.sock");
    for field in ["ai_context", "query_text", "identity", "cache_key"] {
        let mut document = json!({
            "scheme": 0,
            "generation": 1,
            "identity": "identity",
            "query_text": "nihao",
            "cache_key": "cache",
            "pinyin_segments": ["ni", "hao"],
            "cloud_eligible": true,
            "ai_eligible": true,
            "session_id": 5,
            "ai_assistant": {"enabled": true, "candidate_limit": 1}
        });
        document[field] = json!("safe\u{0}text");
        let query: OnlineQuery = serde_json::from_value(document).unwrap();
        assert!(
            UnixSocketProvider::new(&socket)
                .query_candidates(query)
                .is_none(),
            "field {field} must reject control characters"
        );
    }
}

#[cfg(unix)]
#[test]
fn translation_provider_rejects_controls_at_the_socket_boundary() {
    let directory = private_tempdir();
    let request_socket = directory.path().join("translation-request.sock");
    let listener = UnixListener::bind(&request_socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_done = done.clone();
    let server_accepted = accepted.clone();
    let server = std::thread::spawn(move || loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                server_accepted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                std::io::Write::write_all(
                    &mut stream,
                    br#"{"translations":[{"text":"safe","translation":"safe"}]}"#,
                )
                .unwrap();
                std::io::Write::write_all(&mut stream, b"\n").unwrap();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if server_done.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("translation fixture failed: {error}"),
        }
    });
    let provider = UnixSocketProvider::new(&request_socket);
    for codepoint in (0..=0x1f).chain(0x7f..=0x9f) {
        let control = char::from_u32(codepoint).unwrap();
        assert!(provider
            .translate(TranslationQuery {
                generation: 1,
                target_language: "en".into(),
                candidates: vec![format!("safe{control}")],
                sentence: false,
                provider: TranslationService::Tencent,
                translation_account: false,
                custom_translation: None,
                niutrans: None,
                ai_assistant: None,
            })
            .is_none());
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    server.join().unwrap();
    assert_eq!(accepted.load(std::sync::atomic::Ordering::Relaxed), 0);

    let response_socket = directory.path().join("translation-response.sock");
    let listener = UnixListener::bind(&response_socket).unwrap();
    let server = std::thread::spawn(move || {
        let reply = |mut stream: std::os::unix::net::UnixStream, response: &str| {
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            std::io::BufRead::read_line(&mut reader, &mut request).unwrap();
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            std::io::Write::write_all(&mut stream, b"\n").unwrap();
        };
        for codepoint in (0..=0x1f).chain(0x7f..=0x9f) {
            let (stream, _) = listener.accept().unwrap();
            let control = char::from_u32(codepoint).unwrap();
            let response = json!({
                "translations": [{"text":"safe","translation":format!("before{control}after")}]
            })
            .to_string();
            reply(stream, &response);
        }
        let (stream, _) = listener.accept().unwrap();
        reply(
            stream,
            r#"{"translations":[{"text":"safe","translation":"translated"}]}"#,
        );
    });
    let provider = UnixSocketProvider::new(response_socket);
    let query = TranslationQuery {
        generation: 1,
        target_language: "en".into(),
        candidates: vec!["safe".into()],
        sentence: false,
        provider: TranslationService::Tencent,
        translation_account: false,
        custom_translation: None,
        niutrans: None,
        ai_assistant: None,
    };
    for _ in (0..=0x1f).chain(0x7f..=0x9f) {
        assert!(provider.translate(query.clone()).is_none());
    }
    assert_eq!(
        provider.translate(query).unwrap(),
        vec![TranslationResult {
            text: "safe".into(),
            translation: "translated".into(),
        }]
    );
    server.join().unwrap();
}

#[test]
fn translation_query_carries_the_selected_service() {
    for (service, name) in [
        (TranslationService::Off, "none"),
        (TranslationService::Account, "account"),
        (TranslationService::Tencent, "tencent"),
        (TranslationService::NiuTrans, "niutrans"),
        (TranslationService::Custom, "custom"),
        (TranslationService::Ai, "ai"),
    ] {
        let document = json!({"generation": 1, "candidates": ["中"], "provider": name});
        let query: TranslationQuery = serde_json::from_value(document).unwrap();
        assert_eq!(query.provider, service);
        assert_eq!(serde_json::to_value(&query).unwrap()["provider"], name);
    }
    assert!(serde_json::from_value::<TranslationQuery>(
        json!({"generation": 1, "candidates": ["中"]})
    )
    .is_err());
    assert!(serde_json::from_value::<TranslationQuery>(
        json!({"generation": 1, "candidates": ["中"], "provider": "deepl"})
    )
    .is_err());
    let sentence: TranslationQuery = serde_json::from_value(json!({
        "generation": 1,
        "candidates": ["这是一个手动触发的整句翻译请求"],
        "sentence": true,
        "provider": "tencent"
    }))
    .unwrap();
    assert!(sentence.sentence);
    assert_eq!(serde_json::to_value(sentence).unwrap()["sentence"], true);
    let account: TranslationQuery = serde_json::from_value(json!({
        "generation": 1,
        "candidates": ["中"],
        "provider": "none",
        "translation_account": true
    }))
    .unwrap();
    assert!(account.translation_account);
    assert_eq!(
        serde_json::to_value(account).unwrap()["translation_account"],
        true
    );
}

#[cfg(unix)]
#[test]
fn sentence_translation_is_single_item_and_bounded() {
    let provider = UnixSocketProvider::new("/this/provider-does-not-exist");
    let too_long = TranslationQuery {
        generation: 1,
        target_language: "en".into(),
        candidates: vec!["中".repeat(513)],
        sentence: true,
        provider: TranslationService::Tencent,
        translation_account: false,
        custom_translation: None,
        niutrans: None,
        ai_assistant: None,
    };
    assert!(provider.translate(too_long).is_none());
    let two_items = TranslationQuery {
        generation: 1,
        target_language: "en".into(),
        candidates: vec!["第一句".into(), "第二句".into()],
        sentence: true,
        provider: TranslationService::Tencent,
        translation_account: false,
        custom_translation: None,
        niutrans: None,
        ai_assistant: None,
    };
    assert!(provider.translate(two_items).is_none());
}

#[cfg(unix)]
#[test]
fn translation_switched_off_never_reaches_the_provider() {
    let directory = private_tempdir();
    let socket = directory.path().join("translation-off.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let query = TranslationQuery {
        generation: 1,
        target_language: "en".into(),
        candidates: vec!["中".into()],
        sentence: false,
        provider: TranslationService::Off,
        translation_account: false,
        custom_translation: None,
        niutrans: None,
        ai_assistant: None,
    };
    assert_eq!(
        UnixSocketProvider::new(&socket).translate(query),
        Some(Vec::new())
    );
    assert!(
        listener.accept().is_err(),
        "a switched-off query connected to the provider"
    );
}

#[cfg(unix)]
#[test]
fn voice_provider_rejects_events_without_generation_binding() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut request,
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, br#"{"text":"stale","type":"final"}"#).unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let provider = UnixSocketProvider::new(socket);
    assert!(provider
        .voice_stream_with_options_feedback(
            "zh-cn",
            7,
            &Value::Null,
            None,
            &mut |_, _| {},
            None,
            None,
        )
        .is_none());
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn voice_provider_callback_cancellation_is_checked_before_connecting() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let provider = UnixSocketProvider::new(socket);
    let mut cancelled = || true;
    assert_eq!(
        provider.voice_stream_with_options_diagnosed_with_callback(
            "zh-cn",
            7,
            &Value::Null,
            Some(&mut cancelled),
            &mut |_, _| {},
            None,
            None,
        ),
        Err(None)
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "a cancelled voice request connected to the provider"
    );
}

#[cfg(unix)]
#[test]
fn voice_provider_callback_cancellation_interrupts_a_waiting_read() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut request,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let _ = std::io::Write::write_all(&mut stream, b"\n");
    });
    let provider = UnixSocketProvider::new(socket);
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let probe_calls = calls.clone();
    let mut cancelled = move || probe_calls.fetch_add(1, Ordering::Relaxed) >= 2;
    assert_eq!(
        provider.voice_stream_with_options_diagnosed_with_callback(
            "zh-cn",
            7,
            &Value::Null,
            Some(&mut cancelled),
            &mut |_, _| {},
            None,
            None,
        ),
        Err(None)
    );
    assert!(calls.load(Ordering::Relaxed) >= 2);
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn voice_provider_rejects_unknown_event_types() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut request,
        )
        .unwrap();
        std::io::Write::write_all(
            &mut stream,
            br#"{"generation":7,"type":"unexpected","text":"synthetic"}"#,
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let provider = UnixSocketProvider::new(socket);
    let mut updates = Vec::new();
    assert!(provider
        .voice_stream_with_options_feedback(
            "zh-cn",
            7,
            &Value::Null,
            None,
            &mut |text, final_result| updates.push((text.to_owned(), final_result)),
            None,
            None,
        )
        .is_none());
    assert!(
        updates.is_empty(),
        "unknown event reached transcript callbacks"
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn voice_provider_rejects_control_characters_in_transcripts() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut request,
        )
        .unwrap();
        std::io::Write::write_all(
            &mut stream,
            br#"{"generation":7,"type":"final","text":"bad\u0000text","ok":true}"#,
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let provider = UnixSocketProvider::new(socket);
    let mut updates = Vec::new();
    assert!(provider
        .voice_stream_with_options_feedback(
            "zh-cn",
            7,
            &Value::Null,
            None,
            &mut |text, final_result| updates.push((text.to_owned(), final_result)),
            None,
            None,
        )
        .is_none());
    assert!(
        updates.is_empty(),
        "control-bearing transcript reached callbacks"
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn emoji_provider_rejects_control_characters_in_items() {
    let directory = private_tempdir();
    let socket = directory.path().join("emoji.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut request,
        )
        .unwrap();
        std::io::Write::write_all(
            &mut stream,
            r#"{"items":[{"text":"😀","annotation":"bad\u0000annotation"}]}"#.as_bytes(),
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let provider = UnixSocketProvider::new(socket);
    assert!(provider
        .emoji(EmojiPanelQuery {
            search: String::new(),
            category: String::new(),
            limit: 1,
        })
        .is_none());
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn voice_provider_rejects_final_events_without_success_envelope() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(stream.try_clone().unwrap()),
            &mut request,
        )
        .unwrap();
        std::io::Write::write_all(
            &mut stream,
            br#"{"generation":7,"type":"final","text":"synthetic"}"#,
        )
        .unwrap();
        std::io::Write::write_all(&mut stream, b"\n").unwrap();
    });
    let provider = UnixSocketProvider::new(socket);
    let mut updates = Vec::new();
    assert!(provider
        .voice_stream_with_options_feedback(
            "zh-cn",
            7,
            &Value::Null,
            None,
            &mut |text, final_result| updates.push((text.to_owned(), final_result)),
            None,
            None,
        )
        .is_none());
    assert!(
        updates.is_empty(),
        "missing ok reached transcript callbacks"
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn voice_provider_names_only_known_missing_dependencies() {
    for (reply, expected) in [
        (
            r#"{"generation":7,"type":"final","text":"","ok":false,"error":"voice_dependency_missing","detail":"websockets"}"#,
            Err(Some("websockets")),
        ),
        (
            r#"{"generation":7,"type":"final","text":"","ok":false,"error":"voice_dependency_missing","detail":"recorder"}"#,
            Err(Some("recorder")),
        ),
        (
            r#"{"generation":7,"type":"final","text":"","ok":false,"error":"voice_dependency_missing","detail":"local_asr"}"#,
            Err(Some("local_asr")),
        ),
        (
            r#"{"generation":7,"type":"final","text":"","ok":false,"error":"voice_dependency_missing","detail":"token=secret"}"#,
            Err(None),
        ),
        (
            r#"{"generation":7,"type":"final","text":"","ok":false}"#,
            Err(None),
        ),
        (
            r#"{"generation":7,"type":"final","text":"水杉","ok":true}"#,
            Ok("水杉".to_owned()),
        ),
    ] {
        let directory = private_tempdir();
        let socket = directory.path().join("voice.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            std::io::BufRead::read_line(
                &mut std::io::BufReader::new(stream.try_clone().unwrap()),
                &mut request,
            )
            .unwrap();
            std::io::Write::write_all(&mut stream, reply.as_bytes()).unwrap();
            std::io::Write::write_all(&mut stream, b"\n").unwrap();
        });
        let result = UnixSocketProvider::new(socket).voice_stream_with_options_diagnosed(
            "zh-cn",
            7,
            &Value::Null,
            None,
            &mut |_, _| {},
            None,
            None,
        );
        assert_eq!(result, expected, "{reply}");
        server.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn dangling_segment_delimiter_cleanup_matches_windows_policy() {
    assert!(needs_dangling_segment_delimiter_backspace("ni'", 3));
    assert!(needs_dangling_segment_delimiter_backspace("ni''ma", 3));
    assert!(!needs_dangling_segment_delimiter_backspace("ni", 2));
    assert!(!needs_dangling_segment_delimiter_backspace("'ma", 0));
    assert!(!needs_dangling_segment_delimiter_backspace("ni'", 2));
    assert!(!needs_dangling_segment_delimiter_backspace("ni'", 9));
}

#[cfg(unix)]
#[test]
fn voice_control_rejects_zero_generation_without_connecting() {
    let directory = private_tempdir();
    let socket = directory.path().join("voice-control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let provider = UnixSocketProvider::new(&socket);
    assert!(!provider.voice_cancel(0));
    assert!(!provider.voice_stop(0));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}
impl InputEngine for Fixture {
    fn reset_context(&mut self) {
        self.context_resets += 1;
    }
    fn set_first_page_rows(&mut self, rows: usize) {
        self.first_page_rows = rows;
    }
    fn reset_cache(&mut self) -> Result<(), RuntimeError> {
        self.cache_resets += 1;
        Ok(())
    }
    fn balance_paired_punctuation_after_auto_close(
        &mut self,
        opening: u8,
    ) -> Result<(), RuntimeError> {
        self.balanced_openings.push(opening);
        Ok(())
    }
    fn set_dedicated_english(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        if self.dedicated_english == enabled {
            return Ok(());
        }
        self.dedicated_english = enabled;
        self.text.clear();
        Ok(())
    }
    fn expand_initial_candidates(&mut self) -> Result<bool, RuntimeError> {
        if self.withheld.is_empty() {
            return Ok(false);
        }
        self.words.append(&mut self.withheld);
        Ok(true)
    }
    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        if value == b'!' {
            return Err(RuntimeError::Engine("injected punctuation failure".into()));
        }
        if value != b',' {
            return Ok(empty_result(false));
        }
        Ok(EngineResult {
            handled: true,
            has_commit: true,
            commit: "，".into(),
            diagnostic: String::new(),
        })
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        if self.text.is_empty() {
            return Ok(empty_result(false));
        }
        let mut result = self.select(index)?;
        result.commit.push_str("-remaining-segments");
        Ok(result)
    }
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        if self.snapshot_fails {
            return Err(RuntimeError::Engine("injected snapshot failure".into()));
        }
        let words = if self.text.is_empty() {
            Vec::new()
        } else {
            self.words.clone()
        };
        let candidates = words
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                let code = self.codes.get(index).cloned().unwrap_or_default();
                let source = self.sources.get(index).copied().unwrap_or(0);
                let answers_key = self.answers_key.get(index).copied().unwrap_or(true);
                let mut entry = msime_engine::CandidateEntry::new(text, code, source);
                entry.annotation = format!("({index})");
                entry.corrected = false;
                entry.answers_key = answers_key;
                entry.origin = msime_engine::CandidateOrigin::EngineRow(index);
                entry
            })
            .collect();
        Ok(EngineSnapshot {
            prediction: false,
            scheme: self.scheme,
            candidates,
            microsoft_shuangpin: self.microsoft_shuangpin,
            shuangpin_profile: "xiaohe".into(),
            local_mode: self.local_mode.clone(),
            spelling_symbols: self.spelling_symbols.clone(),
            dedicated_english: self.dedicated_english,
            preedit: self.text.clone(),
            reading: self.reading.clone(),
            display_reading: self.reading.clone(),
            editing_text: self.text.clone(),
            caret_position: self.text.len(),
            segment_raw_boundaries: vec![],
            ..EngineSnapshot::default()
        })
    }
    fn character(&mut self, value: u8, _shift: bool) -> Result<EngineResult, RuntimeError> {
        if value.is_ascii_digit() || value.is_ascii_punctuation() {
            return Ok(empty_result(false));
        }
        self.text.push(value as char);
        Ok(empty_result(true))
    }
    fn command(&mut self, _command: Command) -> Result<EngineResult, RuntimeError> {
        self.text.clear();
        Ok(empty_result(true))
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        if let Some(remaining) = self.remaining_after_select.take() {
            self.text = remaining;
            self.local_mode = "none".into();
            return Ok(EngineResult {
                handled: true,
                has_commit: true,
                commit: self.words[index].clone(),
                diagnostic: String::new(),
            });
        }
        self.text.clear();
        self.local_mode = "none".into();
        Ok(EngineResult {
            handled: true,
            has_commit: true,
            commit: self.words[index].clone(),
            diagnostic: String::new(),
        })
    }
    fn select_edge(
        &mut self,
        index: usize,
        edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        let edge_commit = self
            .edge_commits
            .as_ref()
            .map(|values| values[edge as usize].clone());
        let mut result = self.select(index)?;
        if let Some(text) = edge_commit {
            result.commit = text;
            return Ok(result);
        }
        result.commit.push_str(match edge {
            CandidateEdge::FirstHan => "-first",
            CandidateEdge::LastHan => "-last",
        });
        Ok(result)
    }
}

fn second_third_runtime(count: usize) -> Runtime<Fixture> {
    let mut runtime = Runtime::new(
        Fixture {
            scheme: 0,
            local_mode: "none".into(),
            words: (0..count).map(|i| format!("合成候选{i}")).collect(),
            ..Fixture::default()
        },
        3,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    runtime.set_second_third_candidate(
        msime_client_core::preferences::SecondThirdCandidatePreferences {
            enabled: true,
            ..Default::default()
        },
    );
    if count > 0 {
        runtime
            .dispatch(Action::Character {
                value: b'a',
                shift: false,
            })
            .unwrap();
    }
    runtime
}

#[test]
fn second_third_candidate_shortcuts_select_current_page_on_all_character_routes() {
    for route in 0..3 {
        for (key, slot) in [(b';', 1), (b'\'', 2)] {
            let mut runtime = second_third_runtime(6);
            runtime.dispatch(Action::NextPage).unwrap();
            let action = match route {
                0 => Action::Character {
                    value: key,
                    shift: false,
                },
                1 => Action::Punctuation(key),
                _ => Action::PunctuationAscii(key),
            };
            let transition = runtime.dispatch(action).unwrap();
            assert_eq!(transition.commit, Some(format!("合成候选{}", 3 + slot)));
            assert!(transition.view.editing_text.is_empty());
            assert!(runtime.facts().selection.is_some());
        }
    }
}

#[test]
fn display_page_moves_shortcuts_and_translation_query_to_the_shown_row() {
    // 宿主按宽度排出的一行跨过固定页（每页 3 个）：显示的是 4–7，`;` `'` 选这一行的第二、三个。
    for (key, slot) in [(b';', 1), (b'\'', 2)] {
        let mut runtime = second_third_runtime(10);
        let generation = runtime.view().generation;
        assert!(runtime.set_display_page(generation, vec![4, 5, 6, 7]));
        let texts: Vec<_> = runtime
            .translation_candidates()
            .unwrap()
            .candidates
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(texts, ["合成候选4", "合成候选5", "合成候选6", "合成候选7"]);
        let transition = runtime.dispatch(Action::PunctuationAscii(key)).unwrap();
        assert_eq!(transition.commit, Some(format!("合成候选{}", 4 + slot)));
    }
    // 显示的一行只有一个候选：没有第二、三个，键照原来的标点路径走，不选固定页里看不见的候选。
    for key in *b";'" {
        let mut runtime = second_third_runtime(10);
        let generation = runtime.view().generation;
        assert!(runtime.set_display_page(generation, vec![8]));
        let transition = runtime.dispatch(Action::PunctuationAscii(key)).unwrap();
        assert!(transition
            .commit
            .as_deref()
            .is_some_and(|text| text.ends_with(char::from(key))));
    }
}

#[test]
fn display_page_only_holds_for_its_own_generation() {
    let mut runtime = second_third_runtime(10);
    let generation = runtime.view().generation;
    // 过期的一代、越界的座位都不接受，原有显示页保持不变。
    assert!(!runtime.set_display_page(generation + 1, vec![4, 5]));
    assert!(!runtime.set_display_page(generation, vec![4, 10]));
    assert!(runtime.display_page.is_none());
    assert!(runtime.set_display_page(generation, vec![4, 5, 6]));
    // 空列表清掉，回到固定页。
    assert!(runtime.set_display_page(generation, Vec::new()));
    assert_eq!(
        runtime.translation_candidates().unwrap().candidates.len(),
        3
    );
    // 组字推进到新的一代后，旧的显示页不再起作用。
    assert!(runtime.set_display_page(generation, vec![4, 5, 6]));
    runtime
        .dispatch(Action::Character {
            value: b'a',
            shift: false,
        })
        .unwrap();
    assert_ne!(runtime.view().generation, generation);
    let texts: Vec<_> = runtime
        .translation_candidates()
        .unwrap()
        .candidates
        .into_iter()
        .map(|candidate| candidate.text)
        .collect();
    assert_eq!(texts, ["合成候选0", "合成候选1", "合成候选2"]);
}

#[test]
fn second_third_candidate_shortcuts_keep_original_route_when_disabled_or_target_is_missing() {
    for count in [0, 1, 2, 3, 4] {
        for key in *b";'" {
            let mut enabled = second_third_runtime(count);
            let mut plain = second_third_runtime(count);
            plain.set_second_third_candidate(Default::default());
            if count > 3 {
                enabled.dispatch(Action::NextPage).unwrap();
                plain.dispatch(Action::NextPage).unwrap();
            }
            let slot = if key == b';' { 1 } else { 2 };
            let on_page = if count > 3 { count - 3 } else { count };
            if slot < on_page {
                continue;
            }
            let original = plain.dispatch(Action::PunctuationAscii(key)).unwrap();
            let actual = enabled.dispatch(Action::PunctuationAscii(key)).unwrap();
            assert_eq!(actual.handled, original.handled);
            assert_eq!(actual.commit, original.commit);
            assert_eq!(actual.view.editing_text, original.view.editing_text);
            if count == 0 {
                assert!(!actual.handled);
            } else {
                // 目标候选不在当前页时键不接管：走原标点路径，上屏组字高亮行后接这个键。
                assert!(actual
                    .commit
                    .as_deref()
                    .is_some_and(|text| text.ends_with(char::from(key))));
            }
        }
    }
}

#[test]
fn second_third_candidate_shortcuts_respect_scheme_mode_focus_and_engine_input() {
    // 非全拼/双拼序号一律不接管。
    for scheme in [2u8, 3, 4, 5, 6, 7, 8, 9] {
        let mut runtime = second_third_runtime(3);
        runtime.cached.scheme = scheme;
        let result = runtime.dispatch(Action::PunctuationAscii(b';')).unwrap();
        assert_ne!(
            result.commit.as_deref(),
            Some("合成候选1"),
            "scheme {scheme} should keep ';' on its original route"
        );
    }
    // 全拼、双拼照常接管。
    for scheme in [0u8, 1] {
        let mut runtime = second_third_runtime(3);
        runtime.cached.scheme = scheme;
        let result = runtime.dispatch(Action::PunctuationAscii(b';')).unwrap();
        assert_eq!(
            result.commit.as_deref(),
            Some("合成候选1"),
            "scheme {scheme} should select with ';'"
        );
    }
    for excluded in 0..3 {
        let mut runtime = second_third_runtime(3);
        match excluded {
            0 => runtime.cached.dedicated_english = true,
            1 => runtime.cached.local_mode = "url".into(),
            _ => {
                runtime.focus(false).unwrap();
            }
        }
        let result = runtime.dispatch(Action::PunctuationAscii(b';')).unwrap();
        assert_ne!(result.commit.as_deref(), Some("合成候选1"));
    }
    // 网址模式或方案拼写里的 `;`/`'` 是输入：引擎在 `spelling_symbols` 里列出时不接管。
    for key in *b";'" {
        let mut runtime = second_third_runtime(3);
        runtime.cached.spelling_symbols = String::from_utf8(vec![key]).unwrap();
        let result = runtime.dispatch(Action::PunctuationAscii(key)).unwrap();
        assert_ne!(result.commit.as_deref(), Some("合成候选1"));
        assert_ne!(result.commit.as_deref(), Some("合成候选2"));
    }
    // 微软双拼声母后（自上一个 `'` 起奇数长度的片段）的 `;` 是韵母 ing；偶数长度后是选词键。
    // 与 Windows `microsoft_shuangpin_final_position`、引擎 `accepts_ing_key` 同一条判断。
    let mut runtime = Runtime::new(
        Fixture {
            scheme: 1,
            microsoft_shuangpin: true,
            local_mode: "none".into(),
            words: (0..3).map(|i| format!("合成候选{i}")).collect(),
            ..Fixture::default()
        },
        3,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    runtime.set_second_third_candidate(
        msime_client_core::preferences::SecondThirdCandidatePreferences {
            enabled: true,
            ..Default::default()
        },
    );
    // 组字是 "n"：刚打完声母的奇数长度片段，`;` 留给引擎当 ing，不会选第二个候选。
    runtime
        .dispatch(Action::Character {
            value: b'n',
            shift: false,
        })
        .unwrap();
    let kept = runtime
        .dispatch(Action::Character {
            value: b';',
            shift: false,
        })
        .unwrap();
    assert_ne!(kept.commit.as_deref(), Some("合成候选1"));
    // 片段自上一个 `'` 重新计数：`ni'i` 的最后一段仍奇数，`;` 还是 ing。
    runtime.engine.text = "ni'i".into();
    runtime.refresh().unwrap();
    let kept_after_apostrophe = runtime
        .dispatch(Action::Character {
            value: b';',
            shift: false,
        })
        .unwrap();
    assert_ne!(kept_after_apostrophe.commit.as_deref(), Some("合成候选1"));
    // 偶数长度的片段：`;` 照常选第二个候选。
    runtime.engine.text = "ni".into();
    runtime.refresh().unwrap();
    let picked = runtime
        .dispatch(Action::Character {
            value: b';',
            shift: false,
        })
        .unwrap();
    assert_eq!(picked.commit.as_deref(), Some("合成候选1"));
}
fn runtime() -> Runtime<Fixture> {
    Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: (0..12).map(|n| format!("candidate-{n}")).collect(),
            ..Fixture::default()
        },
        5,
    )
    .unwrap()
}

#[test]
fn auto_close_balance_accepts_only_the_book_title_opening() {
    let mut runtime = runtime();
    for invalid in [b'(', b'>', b'a', b' ', 0, 128, 255] {
        assert!(matches!(
            runtime.balance_paired_punctuation_after_auto_close(invalid),
            Err(RuntimeError::InvalidPunctuation)
        ));
    }
    assert!(runtime.engine.balanced_openings.is_empty());
    runtime
        .balance_paired_punctuation_after_auto_close(b'<')
        .unwrap();
    assert_eq!(runtime.engine.balanced_openings, vec![b'<']);
}

#[test]
fn cloud_mixing_reserves_first_page_space_before_extra_english_rows() {
    for (count, page_size, ai, promoted, expected) in [
        (
            1,
            5,
            false,
            false,
            vec!["中", "云", "one", "文", "two", "three"],
        ),
        (
            3,
            5,
            false,
            false,
            vec!["中", "云", "one", "two", "three", "文"],
        ),
        (
            3,
            5,
            true,
            false,
            vec!["中", "云", "one", "two", "AI", "文", "three"],
        ),
        (
            3,
            3,
            false,
            false,
            vec!["中", "云", "one", "文", "two", "three"],
        ),
        (
            3,
            3,
            false,
            true,
            vec!["one", "中", "云", "文", "two", "three"],
        ),
        (
            3,
            5,
            true,
            true,
            vec!["one", "two", "中", "云", "AI", "文", "three"],
        ),
    ] {
        let mut words = vec!["中", "one", "文", "two", "three", "云"];
        let mut sources = vec![0, 4, 0, 4, 4, 2];
        if promoted {
            words.swap(0, 1);
            sources.swap(0, 1);
        }
        if ai {
            words.push("AI");
            sources.push(3);
        }
        let mut runtime = Runtime::new(
            Fixture {
                local_mode: "none".into(),
                words: words.iter().map(|word| (*word).into()).collect(),
                codes: (0..words.len())
                    .map(|index| format!("synthetic-{index}"))
                    .collect(),
                sources,
                ..Fixture::default()
            },
            page_size,
        )
        .unwrap();
        runtime.set_english_first_page_count(count);
        runtime.focus(true).unwrap();
        type_key(&mut runtime);
        let actual: Vec<_> = runtime
            .all_candidates()
            .candidates
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(
            actual, expected,
            "英文数={count}，首页={page_size}，AI={ai}，英文领先={promoted}"
        );
    }
}

/// 打拼音出符号的符号行和 emoji 行同一个来源（`CandidateSource::Emoji`），有云候选时按 emoji 的座位坐：第一行符号坐 emoji 座位（云候选之后），其余接在本地候选之后。这里锁定现状，免得以后有人只给 emoji 写特例。
#[test]
fn symbol_rows_take_the_emoji_seat_when_online_candidates_arrive() {
    let seated = |words: &[&str], sources: Vec<u8>| {
        let mut runtime = Runtime::new(
            Fixture {
                scheme: 0,
                local_mode: "none".into(),
                words: words.iter().map(|word| (*word).into()).collect(),
                codes: (0..words.len()).map(|n| format!("code-{n}")).collect(),
                sources,
                ..Fixture::default()
            },
            9,
        )
        .unwrap();
        runtime.focus(true).unwrap();
        type_key(&mut runtime)
            .view
            .candidates
            .into_iter()
            .map(|candidate| candidate.text)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        seated(
            &["平方米", "㎡", "m²", "平方", "云候选"],
            vec![0, 6, 6, 0, 2]
        ),
        vec!["平方米", "云候选", "㎡", "平方", "m²"]
    );
}
#[test]
fn a_phrase_piece_survives_the_reading_being_deleted() {
    let mut runtime = Runtime::new(
        Fixture {
            scheme: 0,
            local_mode: "none".into(),
            words: vec!["海滩".into(), "跑步".into()],
            remaining_after_select: Some("p".into()),
            ..Fixture::default()
        },
        5,
    )
    .unwrap();
    runtime.set_phrase_preedit(true);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let id = runtime.view().candidates[0].id;
    let held = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩");

    let emptied = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert!(emptied.view.editing_text.is_empty());
    assert_eq!(emptied.commit.as_deref(), Some("海滩"));
    assert!(emptied.view.phrase_prefix.is_empty());
}

// Windows `input.escape_keeps_selected_word`（出厂关）：开着时造词中第一次 Esc 只丢掉还没选的拼写，
// 已经选好的词留在组字里，再按一次才整体取消（小狼毫的 ClearNonConfirmedComposition）；关着时照旧整体取消。
#[test]
fn escape_keeps_the_selected_word_when_the_preference_is_on() {
    let start = |keeps: bool| {
        let mut runtime = Runtime::new(
            Fixture {
                scheme: 0,
                local_mode: "none".into(),
                words: vec!["海滩".into(), "跑步".into()],
                remaining_after_select: Some("paobu".into()),
                ..Fixture::default()
            },
            5,
        )
        .unwrap();
        runtime.set_phrase_preedit(true);
        runtime.set_escape_keeps_selected_word(keeps);
        runtime.focus(true).unwrap();
        runtime
    };
    let pick = |runtime: &mut Runtime<Fixture>| {
        let id = runtime.view().candidates[0].id;
        runtime.dispatch(Action::Select(id)).unwrap()
    };

    // 关（Windows 出厂值）：Esc 把拼写和已选词一起丢掉。
    let mut off = start(false);
    type_key(&mut off);
    pick(&mut off);
    let cancelled = off.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert_eq!(cancelled.commit, None);
    assert!(cancelled.view.phrase_prefix.is_empty());

    // 开：第一次 Esc 只丢掉拼写，词留在组字里；第二次 Esc 才整体取消。
    let mut on = start(true);
    type_key(&mut on);
    pick(&mut on);
    assert_eq!(on.view().editing_text, "paobu");
    let kept = on.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert_eq!(kept.commit, None);
    assert!(kept.handled);
    assert_eq!(kept.view.phrase_prefix, "海滩");
    assert!(kept.view.editing_text.is_empty());
    let ended = on.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert_eq!(ended.commit, None);
    assert!(ended.view.phrase_prefix.is_empty());
}

#[test]
fn real_engine_escape_preserves_selection_history_and_followup_actions() {
    for action in [
        Action::Command(Command::Cancel),
        Action::Command(Command::Backspace),
        Action::SegmentBackspace,
        Action::Command(Command::CommitRaw),
        Action::Command(Command::CommitCandidate),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut runtime = ai_answer_runtime(directory.path(), 5);
        rusqlite::Connection::open(
            directory
                .path()
                .join("dictionaries")
                .join(msime_engine::assets::MAIN_DICTIONARY),
        )
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER); \
            INSERT INTO tbl_1_h VALUES('hao','h','好',100);",
        )
        .unwrap();
        runtime.set_phrase_preedit(true);
        runtime.set_escape_keeps_selected_word(true);
        type_characters(&mut runtime, "nihao");
        runtime.set_caret(Some(2)).unwrap();
        let id = runtime
            .view()
            .candidates
            .iter()
            .find(|row| row.text == "你")
            .unwrap()
            .id;
        let selected = runtime.dispatch(Action::Select(id)).unwrap();
        assert_eq!(selected.commit, None);
        assert_eq!(selected.view.phrase_prefix, "你");
        assert_eq!(selected.view.editing_text, "hao");
        let kept = runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
        assert!(kept.handled);
        assert_eq!(kept.commit, None);
        assert_eq!(kept.view.phrase_prefix, "你");
        assert!(kept.view.editing_text.is_empty());
        assert_eq!(runtime.locked_prefix.len(), 1);
        let restored = matches!(action, Action::Command(Command::Backspace));
        let committed = matches!(
            action,
            Action::Command(Command::CommitRaw | Command::CommitCandidate)
        );
        let result = runtime.dispatch(action).unwrap();
        assert!(result.handled);
        assert_eq!(result.commit.as_deref(), committed.then_some("你"));
        assert!(result.view.phrase_prefix.is_empty());
        assert_eq!(result.view.editing_text, if restored { "ni" } else { "" });
        assert!(runtime.locked_prefix.is_empty());
    }
}

#[test]
fn ai_context_keeps_the_recent_tail_on_a_character_boundary() {
    let mut runtime = runtime();
    runtime.focused = true;
    runtime.remember_commit("你好");
    runtime.remember_commit("世界");
    assert_eq!(runtime.ai_context, "你好世界");

    // Bounded at 1024 bytes, because query_candidates refuses anything
    // longer outright rather than trimming it.
    for _ in 0..400 {
        runtime.remember_commit("字");
    }
    assert!(runtime.ai_context.len() <= 1024);
    // The cut lands on a character boundary, so the context is still valid
    // UTF-8 and does not start with half a character.
    assert!(runtime.ai_context.is_char_boundary(0));
    assert!(std::str::from_utf8(runtime.ai_context.as_bytes()).is_ok());
    assert!(runtime.ai_context.ends_with('字'));
    // It is the tail that is kept, not the head.
    assert!(!runtime.ai_context.starts_with("你好"));
}

#[test]
fn ai_context_does_not_leak_between_clients() {
    let mut runtime = runtime();
    runtime.focused = true;
    runtime.remember_commit("上一个应用里的句子");
    assert!(!runtime.ai_context.is_empty());

    // A commit while unfocused is not context at all, and clears what was
    // there: the user has left.
    runtime.focused = false;
    runtime.remember_commit("anything");
    assert!(runtime.ai_context.is_empty());
}
// An engine that models the one thing the phrase rules turn on: a selection takes its reading off
// the front and leaves the rest, and the caret can sit somewhere other than the end.
//
// `Fixture` cannot express either - its `select` replaces the whole reading and its caret is always
// at the end - and a rule about what is left in front of the caret cannot be tested against an
// engine that has no such thing.
struct PhraseEngine {
    reading: String,
    caret: usize,
    /// How much of the reading each selection takes off the front, oldest first. A selection past
    /// the end of this list finishes the composition.
    consumes: Vec<usize>,
    words: Vec<String>,
    /// Published with nothing composed, as the Engine publishes `/` and `@` when their modes are on.
    idle_symbols: String,
    /// 非空时快照报告正在整句改字（`View::conversion`）；`MoveHome` 像真引擎一样先退出改字，把它清空。
    conversion: String,
}

impl PhraseEngine {
    fn new(consumes: Vec<usize>) -> Self {
        Self {
            reading: String::new(),
            caret: 0,
            consumes,
            words: vec!["海滩".into(), "跑步".into()],
            idle_symbols: String::new(),
            conversion: String::new(),
        }
    }
}
impl InputEngine for PhraseEngine {
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        let candidates = if self.reading.is_empty() {
            Vec::new()
        } else {
            self.words
                .iter()
                .enumerate()
                .map(|(index, word)| {
                    let mut entry = msime_engine::CandidateEntry::new(word, "", 0);
                    entry.answers_key = true;
                    entry.origin = msime_engine::CandidateOrigin::EngineRow(index);
                    entry
                })
                .collect()
        };
        Ok(EngineSnapshot {
            prediction: false,
            scheme: 0,
            candidates,
            microsoft_shuangpin: false,
            shuangpin_profile: "xiaohe".into(),
            local_mode: "none".into(),
            spelling_symbols: if self.reading.is_empty() {
                self.idle_symbols.clone()
            } else {
                String::new()
            },
            dedicated_english: false,
            preedit: self.reading.clone(),
            reading: String::new(),
            display_reading: String::new(),
            editing_text: self.reading.clone(),
            caret_position: self.caret.min(self.reading.len()),
            segment_raw_boundaries: Vec::new(),
            conversion: self.conversion.clone(),
            ..EngineSnapshot::default()
        })
    }
    fn character(&mut self, value: u8, _shift: bool) -> Result<EngineResult, RuntimeError> {
        self.reading.push(value as char);
        self.caret = self.reading.len();
        Ok(empty_result(true))
    }
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError> {
        match command {
            Command::MoveLeft => self.caret = self.caret.saturating_sub(1),
            Command::MoveRight => self.caret = (self.caret + 1).min(self.reading.len()),
            Command::MoveHome => {
                self.conversion.clear();
                self.caret = 0;
            }
            Command::MoveEnd => self.caret = self.reading.len(),
            Command::Backspace => {
                if self.caret > 0 {
                    self.reading.remove(self.caret - 1);
                    self.caret -= 1;
                }
            }
            _ => {
                let composing = !self.reading.is_empty();
                self.reading.clear();
                self.caret = 0;
                return Ok(empty_result(composing));
            }
        }
        Ok(empty_result(true))
    }
    fn segment_command(&mut self, command: SegmentCommand) -> Result<EngineResult, RuntimeError> {
        if !matches!(command, SegmentCommand::Backspace) || self.caret == 0 {
            return Ok(empty_result(!self.reading.is_empty()));
        }
        self.reading = self.reading.split_off(self.caret);
        self.caret = 0;
        Ok(empty_result(true))
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        let commit = self.words[index].clone();
        if self.consumes.is_empty() {
            self.reading.clear();
        } else {
            let consumed = self.consumes.remove(0).min(self.reading.len());
            self.reading = self.reading.split_off(consumed);
        }
        self.caret = self.reading.len();
        Ok(EngineResult {
            handled: true,
            has_commit: true,
            commit,
            diagnostic: String::new(),
        })
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        if self.reading.is_empty() {
            return Ok(empty_result(false));
        }
        self.select(index)
    }
    fn punctuation(&mut self, _value: u8) -> Result<EngineResult, RuntimeError> {
        Ok(empty_result(false))
    }
    fn select_edge(
        &mut self,
        index: usize,
        _edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        self.select(index)
    }
}

// A held piece over an emptied reading is a composition: `/` ends it as a mark on every route instead of opening a mode behind it, and a host that reads the View's symbols (Harmony) sees none to compose or pick with.
#[test]
fn a_mode_symbol_behind_a_held_phrase_ends_the_phrase() {
    let emptied = || {
        let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
        runtime.engine.idle_symbols = "/@".into();
        let id = runtime.view().candidates[0].id;
        runtime.dispatch(Action::Select(id)).unwrap();
        let kept = runtime.dispatch(Action::SegmentBackspace).unwrap();
        assert_eq!(kept.view.phrase_prefix, "海滩");
        assert!(kept.view.editing_text.is_empty());
        assert!(kept.view.spelling_symbols.is_empty());
        runtime
    };
    for action in [
        Action::Character {
            value: b'/',
            shift: false,
        },
        Action::Punctuation(b'/'),
        Action::PunctuationAscii(b'/'),
    ] {
        let mut runtime = emptied();
        let ended = runtime.dispatch(action).unwrap();
        assert!(ended.handled);
        assert_eq!(ended.commit.as_deref(), Some("海滩/"));
        assert!(ended.view.phrase_prefix.is_empty());
        assert!(ended.view.editing_text.is_empty());
        assert_eq!(ended.view.spelling_symbols, "/@");
    }
}

// The reading is typed rather than seeded: taking focus cancels the composition, so an engine that
// started with one would lose it before the first key of the test.
fn phrase_runtime(reading: &str, consumes: Vec<usize>) -> Runtime<PhraseEngine> {
    let mut runtime = Runtime::new(PhraseEngine::new(consumes), 5).unwrap();
    runtime.set_phrase_preedit(true);
    runtime.focus(true).unwrap();
    for byte in reading.bytes() {
        runtime
            .dispatch(Action::Character {
                value: byte,
                shift: false,
            })
            .unwrap();
    }
    runtime
}

/// `navigation.left_right`：首选上 ← 编辑输入串，→ 先把光标送回输入串末尾、到了末尾才选下一个；高亮不在首选时左右键只移动候选，与 ↑/↓ 走同一段代码。关着时两个键都还是普通光标命令。
#[test]
fn left_right_edits_only_on_the_first_candidate() {
    let highlighted = |view: &View| {
        view.candidates
            .iter()
            .find(|candidate| candidate.highlighted)
            .map(|candidate| candidate.text.clone())
    };
    // 每页两个，第三个候选是第二页的第一个，但不是首选。
    let mut engine = PhraseEngine::new(Vec::new());
    engine.words = vec!["你好吗".into(), "你好".into(), "妮好吗".into()];
    let mut runtime = Runtime::new(engine, 2).unwrap();
    runtime.focus(true).unwrap();
    for value in *b"nihaoma" {
        runtime
            .dispatch(Action::Character {
                value,
                shift: false,
            })
            .unwrap();
    }
    let mut press = |action: Action| runtime.dispatch(action).unwrap().view;

    // 关着时右键在输入串末尾什么也不做，高亮留在首选。
    let closed = press(Action::Command(Command::MoveRight));
    assert_eq!(closed.caret_position, 7);
    assert_eq!(highlighted(&closed).as_deref(), Some("你好吗"));

    runtime.set_left_right_navigation(true);
    let mut press = |action: Action| runtime.dispatch(action).unwrap().view;

    // 首选、光标在末尾：← 是修改输入串。
    let edited = press(Action::Command(Command::MoveLeft));
    assert_eq!(edited.caret_position, 6);
    assert_eq!(highlighted(&edited).as_deref(), Some("你好吗"));

    // 首选、光标不在末尾：→ 先把光标送回末尾，不选候选。
    let restored = press(Action::Command(Command::MoveRight));
    assert_eq!(restored.caret_position, 7);
    assert_eq!(highlighted(&restored).as_deref(), Some("你好吗"));

    // 首选、光标在末尾：→ 选下一个。
    let next = press(Action::Command(Command::MoveRight));
    assert_eq!(next.caret_position, 7);
    assert_eq!(highlighted(&next).as_deref(), Some("你好"));
    assert_eq!(next.editing_text, "nihaoma");

    // 不在首选：← 回到上一个候选，不动输入串。
    let back = press(Action::Command(Command::MoveLeft));
    assert_eq!(back.caret_position, 7);
    assert_eq!(highlighted(&back).as_deref(), Some("你好吗"));

    // 光标不在末尾时用 ↓ 离开首选，左右键也只移动候选。
    press(Action::Command(Command::MoveLeft));
    let down = press(Action::NextCandidate);
    assert_eq!(down.caret_position, 6);
    assert_eq!(highlighted(&down).as_deref(), Some("你好"));
    let right = press(Action::Command(Command::MoveRight));
    assert_eq!(right.caret_position, 6);
    assert_eq!(highlighted(&right).as_deref(), Some("妮好吗"));

    // 第二页的第一个不是首选：← 翻回上一页的最后一个，而不是编辑输入串。
    let left = press(Action::Command(Command::MoveLeft));
    assert_eq!(left.caret_position, 6);
    assert_eq!(highlighted(&left).as_deref(), Some("你好"));
    let first = press(Action::Command(Command::MoveLeft));
    assert_eq!(first.caret_position, 6);
    assert_eq!(highlighted(&first).as_deref(), Some("你好吗"));

    // 回到首选后 ← 又是编辑输入串，到了开头就停住，不再移高亮。
    press(Action::Command(Command::MoveHome));
    let home = press(Action::Command(Command::MoveLeft));
    assert_eq!(home.caret_position, 0);
    assert_eq!(highlighted(&home).as_deref(), Some("你好吗"));
}

/// 竖排候选窗里左右键与候选的排列方向垂直：`navigation.left_right` 开着也只编辑输入串，高亮交给 ↑/↓。
#[test]
fn vertical_candidates_leave_left_right_to_the_input_string() {
    let highlighted = |view: &View| {
        view.candidates
            .iter()
            .position(|candidate| candidate.highlighted)
    };
    let mut runtime = phrase_runtime("nihaoma", Vec::new());
    runtime.set_left_right_navigation(true);
    runtime.set_vertical_candidates(true);

    // 首选、光标在末尾：→ 不选下一个。
    let right = runtime
        .dispatch(Action::Command(Command::MoveRight))
        .unwrap()
        .view;
    assert_eq!(right.caret_position, 7);
    assert_eq!(highlighted(&right), Some(0));

    // 不在首选：← 仍是编辑输入串，高亮不动。
    runtime.dispatch(Action::NextCandidate).unwrap();
    let left = runtime
        .dispatch(Action::Command(Command::MoveLeft))
        .unwrap()
        .view;
    assert_eq!(left.caret_position, 6);
    assert_eq!(highlighted(&left), Some(1));

    // 换回横排，同一个位置上 ← 回到上一个候选，光标不动。
    runtime.set_vertical_candidates(false);
    let horizontal = runtime
        .dispatch(Action::Command(Command::MoveLeft))
        .unwrap()
        .view;
    assert_eq!(horizontal.caret_position, 6);
    assert_eq!(highlighted(&horizontal), Some(0));
}

/// Home 键分两段：高亮不在首选时先回到首选，光标不动；已经在首选时才把光标移到输入串开头。
#[test]
fn home_key_returns_to_the_first_candidate_before_it_moves_the_caret() {
    let highlighted = |view: &View| {
        view.candidates
            .iter()
            .position(|candidate| candidate.highlighted)
    };
    let mut runtime = phrase_runtime("nihaoma", Vec::new());
    runtime.dispatch(Action::NextCandidate).unwrap();

    let first = runtime.dispatch(Action::FirstCandidateOrHome).unwrap().view;
    assert_eq!(highlighted(&first), Some(0));
    assert_eq!(first.caret_position, 7);

    let home = runtime.dispatch(Action::FirstCandidateOrHome).unwrap().view;
    assert_eq!(highlighted(&home), Some(0));
    assert_eq!(home.caret_position, 0);

    // 日文转换用的 FirstCandidate 不分段：高亮已在首选时光标也不动。
    runtime.dispatch(Action::Command(Command::MoveEnd)).unwrap();
    let plain = runtime.dispatch(Action::FirstCandidate).unwrap().view;
    assert_eq!(plain.caret_position, 7);
}

/// 整句改字时 Home 只回到首选：刚进改字时高亮就在首选，发 `MoveHome` 会让引擎退出改字、丢掉钉住的段。
#[test]
fn home_key_keeps_sentence_conversion() {
    let mut engine = PhraseEngine::new(Vec::new());
    engine.conversion = "你好吗".into();
    let mut runtime = Runtime::new(engine, 5).unwrap();
    runtime.focus(true).unwrap();
    for value in *b"nihaoma" {
        runtime
            .dispatch(Action::Character {
                value,
                shift: false,
            })
            .unwrap();
    }

    let home = runtime.dispatch(Action::FirstCandidateOrHome).unwrap().view;
    assert_eq!(home.conversion, "你好吗");
    assert_eq!(home.caret_position, 7);
    assert!(home.candidates[0].highlighted);
}

#[test]
fn taking_focus_discards_held_phrase_even_if_refresh_fails() {
    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: vec!["海滩".into()],
            remaining_after_select: Some("paobu".into()),
            ..Fixture::default()
        },
        5,
    )
    .unwrap();
    runtime.set_phrase_preedit(true);
    runtime.focus(true).unwrap();
    for value in *b"haitanpaobu" {
        runtime
            .dispatch(Action::Character {
                value,
                shift: false,
            })
            .unwrap();
    }
    let selected = runtime.view().candidates[0].id;
    let held = runtime.dispatch(Action::Select(selected)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩");
    assert_eq!(runtime.locked_prefix.len(), 1);
    runtime.engine.snapshot_fails = true;

    assert!(runtime.focus(true).is_err());

    assert!(runtime.locked_prefix.is_empty());
    runtime.engine.snapshot_fails = false;
    assert!(runtime.focus(true).unwrap().commit.is_none());
}

// A Ctrl+Backspace that empties the reading does not end the phrase: the chosen piece stays in the composition with its selection, as the reference's `keep_creating_word_after_empty_raw` keeps it (MSIME-Windows server/src/ipc/event_listener.cpp, pinned by `ShouldRetreatCreatingWordSelection(true, false, true, 0, 0, 1)` and `ShouldDropCreatingWordSegment(true, false, true, 0, 1)` in test_input_key_policy.cpp). Every follow-up key then acts on that phrase.
#[test]
fn a_segment_backspace_that_empties_the_reading_keeps_the_phrase() {
    let emptied = || {
        let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
        let id = runtime.view().candidates[0].id;
        let held = runtime.dispatch(Action::Select(id)).unwrap();
        assert_eq!(held.view.phrase_prefix, "海滩");
        assert_eq!(held.view.editing_text, "paobu");
        let kept = runtime.dispatch(Action::SegmentBackspace).unwrap();
        assert_eq!(kept.commit, None);
        assert!(kept.handled);
        assert_eq!(kept.view.phrase_prefix, "海滩");
        assert!(kept.view.editing_text.is_empty());
        runtime
    };

    // Backspace takes the selection back: the reading it consumed returns.
    let mut runtime = emptied();
    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(back.commit, None);
    assert!(back.view.phrase_prefix.is_empty());
    assert_eq!(back.view.editing_text, "haitan");

    // A second Ctrl+Backspace deletes the chosen piece, which ends the composition with nothing sent.
    let mut runtime = emptied();
    let dropped = runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert_eq!(dropped.commit, None);
    assert!(dropped.handled);
    assert!(dropped.view.phrase_prefix.is_empty());
    assert!(dropped.view.editing_text.is_empty());

    // Enter sends the phrase, and the key does not also reach the application.
    let mut runtime = emptied();
    let committed = runtime
        .dispatch(Action::Command(Command::CommitRaw))
        .unwrap();
    assert_eq!(committed.commit.as_deref(), Some("海滩"));
    assert!(committed.handled);
    assert!(committed.view.phrase_prefix.is_empty());

    // Escape throws it away.
    let mut runtime = emptied();
    let cancelled = runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert_eq!(cancelled.commit, None);
    assert!(cancelled.handled);
    assert!(cancelled.view.phrase_prefix.is_empty());

    // Leaving the client still sends the phrase.
    let mut runtime = emptied();
    let blurred = runtime.focus(false).unwrap();
    assert_eq!(blurred.commit.as_deref(), Some("海滩"));
}

// Going back into a phrase that is half chosen.
//
// The reference has two rules for it, both in `input_key_policy.h`, and this host had neither: the
// piece the user picked could only be finished or thrown away whole. Picking the wrong word for the
// first half of a phrase is ordinary, and the way out of it was to cancel the composition and type
// the whole thing again.
#[test]
fn the_last_selection_of_a_phrase_can_be_taken_back() {
    // Backspace on the last character of the reading: the selection comes back instead of the
    // composition ending. The reading it consumed is what is on screen afterwards, so the user can
    // pick a different word for it.
    let mut runtime = phrase_runtime("haitanp", vec![6]);
    let id = runtime.view().candidates[0].id;
    let held = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩");
    assert_eq!(held.view.editing_text, "p");

    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(back.commit, None);
    assert!(back.view.phrase_prefix.is_empty());
    assert_eq!(back.view.editing_text, "haitan");
    assert!(!back.view.candidates.is_empty());

    // And it is a stack: only the newest selection comes back, the ones before it stay.
    let mut runtime = phrase_runtime("haitanpaobux", vec![6, 5]);
    let first = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(first)).unwrap();
    let second = runtime.view().candidates[1].id;
    let held = runtime.dispatch(Action::Select(second)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩跑步");
    assert_eq!(held.view.editing_text, "x");
    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(back.view.phrase_prefix, "海滩");
    assert_eq!(back.view.editing_text, "paobu");

    // With more than one character left the key is an ordinary Backspace: the user is editing the
    // reading, not leaving it.
    let mut runtime = phrase_runtime("haitanpa", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let edited = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(edited.view.phrase_prefix, "海滩");
    assert_eq!(edited.view.editing_text, "p");

    // Nothing was ever selected, so there is nothing to go back to and Backspace stays Backspace.
    let mut runtime = phrase_runtime("p", Vec::new());
    let plain = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert!(plain.view.editing_text.is_empty());
    assert!(plain.view.phrase_prefix.is_empty());
}

// Ctrl+Backspace with nothing before the caret deletes the selection itself, and unlike Backspace
// it does not hand the reading back: the user asked to remove that piece of the phrase, not to
// spell it again (the reference's PRD R3).
#[test]
fn a_segment_backspace_with_nothing_before_the_caret_drops_the_selection() {
    let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
    let id = runtime.view().candidates[0].id;
    let held = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩");
    assert_eq!(held.view.editing_text, "paobu");

    runtime
        .dispatch(Action::Command(Command::MoveHome))
        .unwrap();
    let dropped = runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert_eq!(dropped.commit, None);
    assert!(dropped.view.phrase_prefix.is_empty());
    // The reading it consumed is gone for good; what the user typed after it is untouched.
    assert_eq!(dropped.view.editing_text, "paobu");

    // With the caret anywhere else the key is the ordinary segment Backspace and reaches the
    // Engine, which owns the unit boundaries.
    let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let edited = runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert_eq!(edited.view.phrase_prefix, "海滩");
}

#[test]
fn enabling_phrase_preedit_again_preserves_selection_retreat() {
    let mut runtime = phrase_runtime("haitanp", vec![6]);
    let id = runtime.view().candidates[0].id;
    let held = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩");
    assert_eq!(held.view.editing_text, "p");

    assert_eq!(runtime.set_phrase_preedit(true), None);
    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(back.commit, None);
    assert!(back.view.phrase_prefix.is_empty());
    assert_eq!(back.view.editing_text, "haitan");
}

#[test]
fn enabling_phrase_preedit_again_preserves_phrase_after_segment_backspace() {
    let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();

    assert_eq!(runtime.set_phrase_preedit(true), None);
    let kept = runtime.dispatch(Action::SegmentBackspace).unwrap();
    assert_eq!(kept.commit, None);
    assert_eq!(kept.view.phrase_prefix, "海滩");
    assert!(kept.view.editing_text.is_empty());

    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(back.commit, None);
    assert!(back.view.phrase_prefix.is_empty());
    assert_eq!(back.view.editing_text, "haitan");
}

#[test]
fn toggling_phrase_preedit_flushes_each_selection_only_once() {
    let mut runtime = phrase_runtime("haitanp", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();

    assert_eq!(runtime.set_phrase_preedit(false).as_deref(), Some("海滩"));
    assert_eq!(runtime.set_phrase_preedit(false), None);
    assert_eq!(runtime.set_phrase_preedit(true), None);
    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(back.commit, None);
    assert!(back.view.phrase_prefix.is_empty());
    assert!(back.view.editing_text.is_empty());
}

#[test]
fn backspace_can_keep_every_selected_piece_until_spelling_is_deleted() {
    let mut runtime = phrase_runtime("woshi", vec![2]);
    runtime.engine.words = vec!["我".into()];
    runtime.refresh().unwrap();
    runtime.set_backspace_reverts_selection(false);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    for expected in ["sh", "s"] {
        let edited = runtime
            .dispatch(Action::Command(Command::Backspace))
            .unwrap();
        assert_eq!(edited.view.phrase_prefix, "我");
        assert_eq!(edited.view.editing_text, expected);
        assert_eq!(edited.commit, None);
        assert_eq!(runtime.facts.retracted, None);
    }
    let ended = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(ended.commit.as_deref(), Some("我"));
    assert!(ended.handled);
    assert!(ended.view.phrase_prefix.is_empty());
    assert!(ended.view.editing_text.is_empty());
    let next = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(next.commit, None);
    assert!(runtime.locked_prefix.is_empty());

    let mut runtime = phrase_runtime("haitanpaobux", vec![6, 5]);
    runtime.set_backspace_reverts_selection(false);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let id = runtime.view().candidates[1].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    let ended = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(ended.commit.as_deref(), Some("海滩跑步"));
    assert!(ended.view.phrase_prefix.is_empty());
}

#[test]
fn backspace_selection_preference_changes_live_without_losing_history() {
    let mut runtime = phrase_runtime("haitanpa", vec![6]);
    let id = runtime.view().candidates[0].id;
    runtime.dispatch(Action::Select(id)).unwrap();
    runtime.set_backspace_reverts_selection(false);
    let edited = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert_eq!(edited.view.phrase_prefix, "海滩");
    assert_eq!(edited.view.editing_text, "p");
    runtime.set_backspace_reverts_selection(true);
    let restored = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert!(restored.view.phrase_prefix.is_empty());
    assert_eq!(restored.view.editing_text, "haitan");
    assert_eq!(restored.commit, None);
}

#[test]
fn disabling_selection_retreat_keeps_cancel_and_segment_delete_rules() {
    for cancel in [false, true] {
        let mut runtime = phrase_runtime("haitanpaobu", vec![6]);
        runtime.set_backspace_reverts_selection(false);
        let id = runtime.view().candidates[0].id;
        runtime.dispatch(Action::Select(id)).unwrap();
        let ended = if cancel {
            runtime.dispatch(Action::Command(Command::Cancel)).unwrap()
        } else {
            runtime.dispatch(Action::SegmentBackspace).unwrap();
            runtime.dispatch(Action::SegmentBackspace).unwrap()
        };
        assert!(ended.handled);
        assert_eq!(ended.commit, None);
        assert!(ended.view.phrase_prefix.is_empty());
        assert!(ended.view.editing_text.is_empty());
    }
}

fn type_key(runtime: &mut Runtime<Fixture>) -> Transition {
    runtime
        .dispatch(Action::Character {
            value: b'a',
            shift: false,
        })
        .unwrap()
}

#[test]
fn online_provider_worker_is_bounded_and_filters_invalid_results() {
    let query = OnlineQuery {
        mode: msime_engine::OnlineInputMode::Pinyin,
        scheme: 0,
        generation: 4,
        identity: "identity".into(),
        query_text: "nihao".into(),
        cache_key: "cache".into(),
        pinyin_segments: vec!["ni".into(), "hao".into()],
        cloud_eligible: true,
        ai_eligible: true,
        cloud_candidates: true,
        cloud_request: None,
        session_id: 9,
        ai_context: String::new(),
        ai_assistant: None,
        ai_cache_only: false,
    };
    let worker = OnlineProviderWorker::spawn(1, |query| {
        if query.query_text == "nihao" {
            Some(("你好".into(), 0))
        } else {
            Some((String::new(), 7))
        }
    })
    .unwrap();
    assert!(worker.submit(query.clone()));
    let mut result = None;
    for _ in 0..100 {
        result = worker.try_recv();
        if result.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let result = result.expect("provider result");
    assert_eq!(result.query, query);
    assert_eq!(result.text, "你好");
    assert_eq!(result.source, 0);
    worker.shutdown();
}

#[test]
fn online_provider_worker_keeps_only_the_latest_completed_result() {
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&calls);
    let worker = OnlineProviderWorker::spawn(1, move |query| {
        observed.fetch_add(1, Ordering::SeqCst);
        Some((query.query_text.clone(), 0))
    })
    .unwrap();
    let query = |text: &str| OnlineQuery {
        mode: msime_engine::OnlineInputMode::Pinyin,
        scheme: 0,
        generation: 1,
        identity: "identity".into(),
        query_text: text.into(),
        cache_key: text.into(),
        pinyin_segments: vec![],
        cloud_eligible: true,
        ai_eligible: false,
        cloud_candidates: true,
        cloud_request: None,
        session_id: 9,
        ai_context: String::new(),
        ai_assistant: None,
        ai_cache_only: false,
    };
    assert!(worker.submit(query("first")));
    for _ in 0..100 {
        if calls.load(Ordering::SeqCst) >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(worker.submit(query("second")));
    for _ in 0..100 {
        if calls.load(Ordering::SeqCst) >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        worker.try_recv().map(|result| result.text),
        Some("second".into())
    );
    assert!(worker.try_recv().is_none());
    worker.shutdown();
}

#[test]
fn online_provider_worker_debounces_from_the_latest_submission() {
    let (called_tx, called_rx) = std::sync::mpsc::channel();
    let worker =
        OnlineProviderWorker::spawn_with_debounce(1, Duration::from_millis(1_000), move |query| {
            called_tx.send(query.query_text.clone()).unwrap();
            None
        })
        .unwrap();
    let query = |text: &str| OnlineQuery {
        mode: msime_engine::OnlineInputMode::Pinyin,
        scheme: 0,
        generation: 1,
        identity: text.into(),
        query_text: text.into(),
        cache_key: text.into(),
        pinyin_segments: vec![],
        cloud_eligible: true,
        ai_eligible: false,
        cloud_candidates: true,
        cloud_request: None,
        session_id: 9,
        ai_context: String::new(),
        ai_assistant: None,
        ai_cache_only: false,
    };

    // Warm up the thread, so the measured window tests debounce rather than startup.
    assert!(worker.submit(query("warmup")));
    assert_eq!(
        called_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        "warmup"
    );
    assert!(worker.submit(query("first")));
    // Submit 700 ms into a 1000 ms window. An old fixed deadline fires about
    // 300 ms later; a quiet-period debounce waits another full 1000 ms.
    std::thread::sleep(Duration::from_millis(700));
    assert!(worker.submit(query("second")));
    assert!(matches!(
        called_rx.recv_timeout(Duration::from_millis(600)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(
        called_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        "second"
    );
    worker.shutdown();
}

#[test]
fn online_provider_worker_rejects_zero_capacity_and_shutdowns_idle() {
    assert!(OnlineProviderWorker::spawn(0, |_| None).is_err());
    let worker = OnlineProviderWorker::spawn(1, |_| None).unwrap();
    worker.shutdown();
}
#[test]
fn replacement_requires_verified_idle_and_preserves_session_focus() {
    let mut active = runtime();
    active.focus(true).unwrap();
    let old = type_key(&mut active).view;
    assert!(matches!(
        active.replace_engine(runtime().engine, 2),
        Err(RuntimeError::CompositionActive)
    ));
    assert_eq!(active.view().editing_text, old.editing_text);
    active.dispatch(Action::Command(Command::Cancel)).unwrap();
    active.replace_engine(runtime().engine, 2).unwrap();
    let updated = type_key(&mut active).view;
    assert_eq!(updated.session, old.session);
    assert!(updated.focused && updated.generation > old.generation);
    assert_eq!(updated.candidates.len(), 2);
    assert!(matches!(
        active.dispatch(Action::Select(old.candidates[0].id)),
        Err(RuntimeError::StaleCandidate)
    ));
    active.engine.snapshot_fails = true;
    assert!(active.refresh().is_err());
    assert!(active.view().editing_text.is_empty());
    assert!(
        !active.is_idle(),
        "missing snapshot is not proof of idle Engine"
    );
    assert!(matches!(
        active.replace_engine(runtime().engine, 2),
        Err(RuntimeError::CompositionActive)
    ));
}

#[test]
fn online_candidate_changes_keep_only_existing_glosses() {
    for source in [0, 1] {
        for batch in [false, true] {
            for owned in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let session =
                    msime_engine::host::Session::new(&real_engine_options(directory.path()))
                        .unwrap();
                let mut runtime = Runtime::new(session, 5).unwrap();
                runtime.focus(true).unwrap();
                type_characters(&mut runtime, "ni");
                let query = runtime.online_query().unwrap().unwrap();
                assert!(runtime
                    .apply_online_candidate(&query, "合成 AI", 1)
                    .unwrap());
                let query = runtime.online_query().unwrap().unwrap();
                assert!(runtime.apply_online_candidate(&query, "合成云", 0).unwrap());
                let before = runtime.view();
                let retained = if source == 0 {
                    "合成 AI"
                } else {
                    "合成云"
                };
                let removed = if source == 0 {
                    "合成云"
                } else {
                    "合成 AI"
                };
                assert!(runtime.apply_translations(
                    before.generation,
                    [
                        ("合成 AI".into(), "ai-gloss".into()),
                        ("合成云".into(), "cloud-gloss".into()),
                        ("合成新".into(), "unrequested-gloss".into()),
                    ]
                ));
                let mut query = runtime.online_query().unwrap().unwrap();
                query.ai_assistant = Some(
                    serde_json::from_value(json!({"enabled": true, "candidate_limit": 1})).unwrap(),
                );
                let applied = match (batch, owned) {
                    (false, false) => runtime.apply_online_candidate(&query, "合成新", source),
                    (false, true) => runtime.apply_online_candidate_owned(query, "合成新", source),
                    (true, false) => {
                        runtime.apply_online_candidates(&query, &["合成新".into()], source)
                    }
                    (true, true) => {
                        runtime.apply_online_candidates_owned(query, &["合成新".into()], source)
                    }
                };
                assert!(
                    applied.unwrap(),
                    "source={source} batch={batch} owned={owned}"
                );
                let after = runtime.view();
                assert!(after.generation > before.generation);
                assert!(
                    after
                        .candidates
                        .iter()
                        .any(|row| row.text == retained && row.translation.is_some()),
                    "新在线候选不能清空仍在列表中的译文"
                );
                assert!(after
                    .candidates
                    .iter()
                    .any(|row| row.text == "合成新" && row.translation.is_none()));
                assert!(!runtime.translations.contains_key(removed));
                assert!(!runtime.translations.contains_key("合成新"));
                assert!(!runtime
                    .apply_translations(before.generation, [(retained.into(), "stale".into())]));
                assert!(matches!(
                    runtime.dispatch(Action::Select(before.candidates[0].id)),
                    Err(RuntimeError::StaleCandidate)
                ));
                runtime.clear_online_candidates(source).unwrap();
                assert!(runtime
                    .view()
                    .candidates
                    .iter()
                    .any(|row| row.text == retained && row.translation.is_some()));
                runtime
                    .dispatch(Action::Character {
                        value: b'h',
                        shift: false,
                    })
                    .unwrap();
                assert!(runtime.translations.is_empty());
            }
        }
    }
}

#[test]
fn online_gloss_retention_drops_everything_when_refresh_fails() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    let view = type_key(&mut runtime).view;
    assert!(runtime.apply_translations(view.generation, [("candidate-0".into(), "gloss".into())]));
    runtime.engine.snapshot_fails = true;
    assert!(runtime.refresh_online_candidates().is_err());
    assert!(runtime.translations.is_empty());
}

#[test]
fn translations_are_generation_scoped_and_exposed_on_candidates() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    let view = type_key(&mut runtime).view;
    assert!(
        !runtime.apply_translations(view.generation - 1, [("candidate-0".into(), "old".into())])
    );
    assert!(runtime.apply_translations(
        view.generation,
        [("candidate-0".into(), "translated".into())]
    ));
    assert_eq!(
        runtime.view().candidates[0].translation.as_deref(),
        Some("translated")
    );
    assert!(runtime.apply_translations(
        view.generation,
        [("candidate-1".into(), "second-gloss".into())]
    ));
    assert_eq!(
        runtime.view().candidates[0].translation.as_deref(),
        Some("translated"),
        "同代次部分回包不能清掉未回答词条的释义"
    );
    assert!(runtime.apply_translations(
        view.generation,
        [("candidate-0".into(), "updated-gloss".into())]
    ));
    assert_eq!(
        runtime.view().candidates[0].translation.as_deref(),
        Some("updated-gloss")
    );
    assert_eq!(
        runtime.view().candidates[1].translation.as_deref(),
        Some("second-gloss")
    );
    assert!(!runtime.apply_translations(view.generation - 1, []));
    assert!(runtime.view().candidates[0].translation.is_some());
    assert!(runtime.apply_translations(view.generation, []));
    assert!(runtime
        .view()
        .candidates
        .iter()
        .all(|candidate| candidate.translation.is_none()));
    assert!(runtime.apply_translations(
        view.generation,
        [("candidate-0".into(), "after-clear".into())]
    ));
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert!(runtime
        .view()
        .candidates
        .iter()
        .all(|candidate| candidate.translation.is_none()));
}

#[test]
fn special_source_rows_do_not_inherit_a_translation_for_the_same_text() {
    for source in [6, 7, 13, 14] {
        let mut runtime = runtime();
        runtime.engine.words = vec!["相同".into(), "相同".into()];
        runtime.engine.sources = vec![0, source];
        runtime.focus(true).unwrap();
        let view = type_key(&mut runtime).view;
        assert!(runtime.apply_translations(view.generation, [("相同".into(), "meaning".into())]));
        let rows = runtime.view().candidates;
        assert_eq!(rows[0].translation.as_deref(), Some("meaning"));
        assert_eq!(rows[1].translation, None, "source {source}");
    }
}

#[test]
fn translation_merges_do_not_accumulate_non_candidates() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    let view = type_key(&mut runtime).view;
    for (text, gloss, unrelated) in [
        ("candidate-0", "first", "not-requested-0"),
        ("candidate-1", "second", "not-requested-1"),
    ] {
        assert!(runtime.apply_translations(
            view.generation,
            [
                (text.into(), gloss.into()),
                (unrelated.into(), "unused".into())
            ]
        ));
    }
    assert_eq!(
        runtime.view().candidates[0].translation.as_deref(),
        Some("first")
    );
    assert_eq!(
        runtime.view().candidates[1].translation.as_deref(),
        Some("second")
    );
    assert_eq!(
        runtime.translations.len(),
        2,
        "同代次合并不能积累请求外词条"
    );
}

#[test]
fn replacement_snapshot_failure_keeps_the_original_engine() {
    let mut active = runtime();
    active.focus(true).unwrap();
    let generation = active.view().generation;
    let mut replacement = runtime().engine;
    replacement.snapshot_fails = true;
    assert!(active.replace_engine(replacement, 2).is_err());
    assert_eq!(active.view().generation, generation);
    assert_eq!(type_key(&mut active).view.candidates.len(), 5);
}

#[test]
fn paging_and_selection_use_global_engine_indices() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let page = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(page.page, 1);
    assert_eq!(page.page_count, 3);
    assert_eq!(page.candidates[0].annotation, "(5)");
    assert_eq!(page.candidates[0].text, "candidate-5");
    let result = runtime
        .dispatch(Action::Select(page.candidates[2].id))
        .unwrap();
    assert_eq!(result.commit.as_deref(), Some("candidate-7"));
    assert!(result.view.candidates.is_empty());
}

#[test]
fn expanded_panel_selection_accepts_only_any_candidate_from_current_generation() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    let page = type_key(&mut runtime).view;
    let outside_page = runtime.all_candidates().candidates[10].id;
    let generation = page.generation;

    assert!(matches!(
        runtime.dispatch(Action::Select(outside_page)),
        Err(RuntimeError::StaleCandidate)
    ));
    for invalid in [
        CandidateId {
            session: outside_page.session + 1,
            ..outside_page
        },
        CandidateId {
            generation: outside_page.generation + 1,
            ..outside_page
        },
        CandidateId {
            index: 12,
            ..outside_page
        },
    ] {
        assert!(matches!(
            runtime.dispatch(Action::SelectAnyCandidate(invalid)),
            Err(RuntimeError::StaleCandidate)
        ));
        assert_eq!(runtime.view().generation, generation);
    }

    let selected = runtime
        .dispatch(Action::SelectAnyCandidate(outside_page))
        .unwrap();
    assert_eq!(selected.commit.as_deref(), Some("candidate-10"));
    assert!(selected.view.candidates.is_empty());
}

#[test]
fn candidate_list_edges_reach_the_ends_of_the_whole_list() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let highlighted = |view: &View| {
        view.candidates
            .iter()
            .find(|candidate| candidate.highlighted)
            .unwrap()
            .text
            .clone()
    };

    // From the second page, Home goes back to the very first candidate and takes the page with it -
    // the reference answers its Home with SetSelection(0), which readjusts the page. Stopping at the
    // top of the page the user is already looking at is a keystroke that changes almost nothing.
    runtime.dispatch(Action::NextPage).unwrap();
    let first = runtime.dispatch(Action::FirstCandidate).unwrap().view;
    assert_eq!(highlighted(&first), "candidate-0");
    assert_eq!(first.page, 0);

    // End reaches the last candidate there is, page and all. The fixture holds twelve at a page of
    // five, so that is the third page rather than the end of the first.
    let last = runtime.dispatch(Action::LastCandidate).unwrap().view;
    assert_eq!(highlighted(&last), "candidate-11");
    assert_eq!(last.page, 2);
    assert_eq!(last.page_count, 3);

    // Pressing it again stays put rather than walking further.
    let again = runtime.dispatch(Action::LastCandidate).unwrap().view;
    assert_eq!(highlighted(&again), "candidate-11");
}

// The Engine caps what it returns to a short query and hands the rest over when asked. End has to
// ask, or it lands on the last candidate that happened to be cached - and a second press would then
// move further, which is not what an End key does.
#[test]
fn the_last_candidate_is_the_last_one_the_engine_has() {
    let mut runtime = withholding_runtime(5, 4, 5);
    runtime.focus(true).unwrap();
    let page = type_key(&mut runtime).view;
    assert_eq!(page.page_count, 1);

    let last = runtime.dispatch(Action::LastCandidate).unwrap().view;
    assert_eq!(last.page_count, 2);
    assert_eq!(
        last.candidates
            .iter()
            .find(|candidate| candidate.highlighted)
            .unwrap()
            .text,
        "candidate-8"
    );
}

#[test]
fn edge_selection_checks_identity_and_routes_global_index() {
    for edge in [CandidateEdge::FirstHan, CandidateEdge::LastHan] {
        let mut active = runtime();
        active.focus(true).unwrap();
        let first = type_key(&mut active).view.candidates[0].id;
        let page = active.dispatch(Action::NextPage).unwrap().view;
        let id = page.candidates[1].id;
        assert_eq!(id.index, 6);
        let generation = active.view().generation;
        for invalid in [
            first,
            CandidateId {
                session: id.session + 1,
                ..id
            },
            CandidateId { index: 0, ..id },
            CandidateId { index: 10, ..id },
        ] {
            assert!(matches!(
                active.dispatch(Action::SelectEdge(invalid, edge)),
                Err(RuntimeError::StaleCandidate)
            ));
            assert_eq!(active.view().generation, generation);
        }
        let selected = active.dispatch(Action::SelectEdge(id, edge)).unwrap();
        assert_eq!(
            selected.commit.as_deref(),
            Some(match edge {
                CandidateEdge::FirstHan => "candidate-6-first",
                CandidateEdge::LastHan => "candidate-6-last",
            })
        );
        assert!(selected.view.editing_text.is_empty());
    }
}

#[test]
fn punctuation_finishes_highlighted_candidate_and_remaining_segments() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime.dispatch(Action::NextPage).unwrap();
    let result = runtime
        .dispatch(Action::Character {
            value: b',',
            shift: false,
        })
        .unwrap();
    assert_eq!(
        result.commit.as_deref(),
        Some("candidate-5-remaining-segments，")
    );
    assert!(result.handled && result.view.editing_text.is_empty());
}

#[test]
fn ascii_punctuation_finishes_highlighted_candidate_for_keypad_marks() {
    for mark in *b".-+/*" {
        let mut runtime = runtime();
        runtime.focus(true).unwrap();
        type_key(&mut runtime);
        let result = runtime.dispatch(Action::PunctuationAscii(mark)).unwrap();
        let expected = format!("candidate-0-remaining-segments{}", mark as char);
        assert_eq!(result.commit.as_deref(), Some(expected.as_str()));
        assert!(result.handled && result.view.editing_text.is_empty());
    }
}

#[test]
fn unsupported_punctuation_is_appended_only_after_a_composition() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    let idle = runtime
        .dispatch(Action::Character {
            value: b'@',
            shift: false,
        })
        .unwrap();
    assert!(!idle.handled && idle.commit.is_none());
    type_key(&mut runtime);
    let result = runtime
        .dispatch(Action::Character {
            value: b'@',
            shift: false,
        })
        .unwrap();
    assert_eq!(
        result.commit.as_deref(),
        Some("candidate-0-remaining-segments@")
    );
}

#[test]
fn punctuation_failure_does_not_lose_an_already_finished_commit() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let result = runtime
        .dispatch(Action::Character {
            value: b'!',
            shift: false,
        })
        .unwrap();
    assert_eq!(
        result.commit.as_deref(),
        Some("candidate-0-remaining-segments!")
    );
    assert!(result
        .diagnostic
        .unwrap()
        .contains("injected punctuation failure"));
}

#[test]
fn number_keys_select_the_visible_page_and_pass_through_when_idle() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    assert!(
        !runtime
            .dispatch(Action::Character {
                value: b'2',
                shift: false
            })
            .unwrap()
            .handled
    );
    type_key(&mut runtime);
    runtime.dispatch(Action::NextPage).unwrap();
    let result = runtime
        .dispatch(Action::Character {
            value: b'2',
            shift: false,
        })
        .unwrap();
    assert_eq!(result.commit.as_deref(), Some("candidate-6"));
}

#[test]
fn unavailable_numeric_slot_does_not_jump_back_to_first_page() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime.dispatch(Action::NextPage).unwrap();
    runtime.dispatch(Action::NextPage).unwrap();
    let result = runtime
        .dispatch(Action::Character {
            value: b'9',
            shift: false,
        })
        .unwrap();
    assert!(result.handled && result.commit.is_none());
    assert_eq!(result.view.page, 2);
}

#[test]
fn engine_mode_is_authoritative_and_resets_old_highlight() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime.dispatch(Action::NextPage).unwrap();
    runtime.engine.local_mode = "unicode".into();
    let result = runtime
        .dispatch(Action::Character {
            value: b'0',
            shift: false,
        })
        .unwrap();
    assert_eq!(result.view.local_mode, "unicode");
    assert_eq!(result.view.page, 0);
    assert!(!result.view.editing_text.starts_with('U'));
}

#[test]
fn dedicated_english_state_resets_highlight_without_guessing_from_text() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime.dispatch(Action::NextPage).unwrap();
    let text = runtime.view().editing_text;
    assert!(!runtime.view().dedicated_english);
    assert_eq!(runtime.view().page, 1);
    runtime.engine.dedicated_english = true;
    runtime.refresh().unwrap();
    assert!(runtime.view().dedicated_english);
    assert_eq!(runtime.view().page, 0);
    assert_eq!(runtime.view().editing_text, text);
    assert_eq!(runtime.view().local_mode, "none");
    runtime.engine.dedicated_english = false;
    runtime.refresh().unwrap();
    assert!(!runtime.view().dedicated_english);
}

#[test]
fn punctuation_host_context_uses_the_applied_runtime_state() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    assert!(runtime.punctuation_host_context_available(false));
    assert!(!runtime.punctuation_host_context_available(true));

    runtime.engine.dedicated_english = true;
    runtime.refresh().unwrap();
    assert!(!runtime.punctuation_host_context_available(false));

    runtime.engine.dedicated_english = false;
    runtime.engine.local_mode = "unicode".into();
    runtime.refresh().unwrap();
    assert!(!runtime.punctuation_host_context_available(false));
}

#[test]
fn switching_the_language_drops_the_composition_being_spelled() {
    // The source pairs `SetEnglishInputMode` with `ClearState`, and the engine does the same inside
    // `set_dedicated_english_mode`: letters spelled for Chinese are not what the user wants sitting
    // in an English composition. The hosts reach this through `msime_client_set_english_mode`, which
    // is what every mode-switch chord ends up calling -- Shift, a Ctrl tap, Ctrl+Alt+Space and
    // Ctrl+Shift+E alike.
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert!(!runtime.view().editing_text.is_empty());
    runtime.set_dedicated_english(true).unwrap();
    assert!(runtime.view().dedicated_english);
    assert_eq!(runtime.view().editing_text, "");
    // Setting the same mode again is not a change, so there is nothing to reset and nothing to lose.
    type_key(&mut runtime);
    let spelled = runtime.view().editing_text.clone();
    assert!(!spelled.is_empty());
    runtime.set_dedicated_english(true).unwrap();
    assert_eq!(runtime.view().editing_text, spelled);
}

#[test]
fn dedicated_english_caret_does_not_turn_rejected_keys_into_candidate_picks() {
    for value in *b"1! " {
        let directory = tempfile::tempdir().unwrap();
        let session =
            msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        runtime.set_dedicated_english(true).unwrap();
        type_characters(&mut runtime, "synthetic");
        runtime
            .dispatch(Action::Command(Command::MoveLeft))
            .unwrap();
        let before = runtime.view();
        assert_eq!(before.caret_position, "synthetic".len() - 1);

        let result = character(&mut runtime, value);

        assert!(
            result.handled && result.commit.is_none(),
            "{value:?}: {result:?}"
        );
        assert_eq!(result.view.editing_text, before.editing_text);
        assert_eq!(result.view.caret_position, before.caret_position);
        assert_eq!(texts(&result.view), texts(&before));
        assert_eq!(
            runtime
                .dispatch(Action::SelectHighlighted)
                .unwrap()
                .commit
                .as_deref(),
            Some("synthetic")
        );
    }
}

#[test]
fn switching_language_clears_held_phrase_even_if_refresh_fails() {
    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: vec!["海滩".into()],
            remaining_after_select: Some("paobu".into()),
            ..Fixture::default()
        },
        5,
    )
    .unwrap();
    runtime.set_phrase_preedit(true);
    runtime.focus(true).unwrap();
    for value in *b"haitanpaobu" {
        runtime
            .dispatch(Action::Character {
                value,
                shift: false,
            })
            .unwrap();
    }
    let selected = runtime.view().candidates[0].id;
    let held = runtime.dispatch(Action::Select(selected)).unwrap();
    assert_eq!(held.view.phrase_prefix, "海滩");
    assert_eq!(runtime.locked_prefix.len(), 1);
    runtime.engine.snapshot_fails = true;

    assert!(runtime.set_dedicated_english(true).is_err());

    assert!(runtime.engine.dedicated_english);
    assert!(runtime.locked_prefix.is_empty());
}

#[test]
fn commit_context_precedes_mode_reset_for_every_selection_route() {
    for route in 0..6 {
        let mut runtime = runtime();
        runtime.focus(true).unwrap();
        runtime.engine.local_mode = "unicode".into();
        let view = type_key(&mut runtime).view;
        let id = view.candidates[0].id;
        let action = match route {
            0 => Action::Select(id),
            1 => Action::SelectEdge(id, CandidateEdge::FirstHan),
            2 => Action::SelectHighlighted,
            3 => Action::Finish,
            // 高亮的不是插件行时，失焦结束组字和 `Finish` 一样。
            4 => Action::FinishOnFocusLoss,
            _ => Action::Character {
                value: b'1',
                shift: false,
            },
        };
        let committed = runtime.dispatch(action).unwrap();
        assert!(committed.commit.is_some());
        assert_eq!(committed.commit_context.unwrap().local_mode, "unicode");
        assert_eq!(committed.view.local_mode, "none");
    }
}

#[test]
fn finish_preserves_engine_completion_of_remaining_segments() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime.dispatch(Action::NextPage).unwrap();
    let result = runtime.dispatch(Action::Finish).unwrap();
    assert_eq!(
        result.commit.as_deref(),
        Some("candidate-5-remaining-segments")
    );
    assert!(result.view.preedit.is_empty());
}
#[test]
fn stale_views_and_other_sessions_cannot_select() {
    let mut a = runtime();
    let mut b = runtime();
    a.focus(true).unwrap();
    b.focus(true).unwrap();
    let id = type_key(&mut a).view.candidates[0].id;
    type_key(&mut b);
    assert!(matches!(
        b.dispatch(Action::Select(id)),
        Err(RuntimeError::StaleCandidate)
    ));
    a.dispatch(Action::NextCandidate).unwrap();
    assert!(matches!(
        a.dispatch(Action::Select(id)),
        Err(RuntimeError::StaleCandidate)
    ));
}
#[test]
fn focus_changes_end_the_engine_context() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    assert_eq!(runtime.engine.context_resets, 1);
    type_key(&mut runtime);
    runtime.focus(false).unwrap();
    assert_eq!(runtime.engine.context_resets, 2);
    runtime.focus(true).unwrap();
    assert_eq!(runtime.engine.context_resets, 3);
}
#[test]
fn cache_maintenance_reaches_engine_without_acquiring_focus() {
    let mut runtime = runtime();
    let idle = runtime.dispatch(Action::ResetCache).unwrap();
    assert!(idle.handled);
    assert!(idle.commit.is_none());
    assert_eq!(runtime.engine.cache_resets, 1);
    assert!(!runtime.focused);
    assert!(!type_key(&mut runtime).handled);

    runtime.focus(true).unwrap();
    let composed = type_key(&mut runtime);
    let refreshed = runtime.dispatch(Action::ResetCache).unwrap();
    assert_eq!(runtime.engine.cache_resets, 2);
    assert_eq!(refreshed.view.preedit, composed.view.preedit);
    assert!(refreshed.commit.is_none());

    runtime.focus(false).unwrap();
    assert!(runtime.dispatch(Action::ResetCache).unwrap().handled);
    assert_eq!(runtime.engine.cache_resets, 3);
    assert!(!runtime.focused);
    assert!(!type_key(&mut runtime).handled);
}
#[test]
fn unfocused_keys_pass_through_and_blur_cancels_composition() {
    let mut runtime = runtime();
    assert!(!type_key(&mut runtime).handled);
    runtime.focus(true).unwrap();
    let id = type_key(&mut runtime).view.candidates[0].id;
    assert!(runtime.focus(false).unwrap().view.preedit.is_empty());
    assert!(!type_key(&mut runtime).handled);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert!(matches!(
        runtime.dispatch(Action::Select(id)),
        Err(RuntimeError::StaleCandidate)
    ));
}
#[test]
fn character_width_conversion_preserves_non_ascii_and_roundtrips_ascii() {
    let full = crate::character_width::to_fullwidth("A 1!");
    assert_eq!(full, "Ａ　１！");
    assert_eq!(crate::character_width::to_halfwidth(&full), "A 1!");
    assert_eq!(crate::character_width::to_fullwidth("中文"), "中文");
}

/// `move_to_back` is the whole of the demotion rule that can be tested without an engine, and
/// the version this replaced shipped with no test at all — which is how it reached `develop`
/// dropping Japanese katakana and, separately, the model's own runner-up choices.
#[test]
fn demotion_moves_flagged_items_to_the_end_and_keeps_both_orders() {
    let mut items = vec!["a", "b", "c", "d", "e"];
    crate::move_to_back(&mut items, &[false, true, false, true, false]);
    assert_eq!(items, vec!["a", "c", "e", "b", "d"]);
}

#[test]
fn demotion_loses_nothing() {
    // The point of moving rather than removing: every candidate is still reachable by paging.
    let mut items: Vec<u32> = (0..9).collect();
    crate::move_to_back(
        &mut items,
        &[false, true, true, false, true, false, false, true, true],
    );
    let mut sorted = items.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..9).collect::<Vec<u32>>());
    assert_eq!(items.len(), 9);
}

#[test]
fn demotion_with_no_flags_is_identity() {
    let mut items = vec![1, 2, 3];
    crate::move_to_back(&mut items, &[false, false, false]);
    assert_eq!(items, vec![1, 2, 3]);
}

#[test]
fn a_short_flag_list_leaves_the_tail_in_place() {
    // Defensive: the parallel arrays are length-checked before this runs, but a mismatch must
    // not reorder anything it was not told about.
    let mut items = vec![1, 2, 3, 4];
    crate::move_to_back(&mut items, &[true]);
    assert_eq!(items, vec![2, 3, 4, 1]);
}

/// A runtime over an engine answering `rows` (text, source) in that order, with one key typed.
fn lattice_runtime(rows: &[(&str, u8)]) -> Runtime<Fixture> {
    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: rows.iter().map(|(text, _)| (*text).to_owned()).collect(),
            sources: rows.iter().map(|(_, source)| *source).collect(),
            codes: vec![String::new(); rows.len()],
            ..Fixture::default()
        },
        9,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime
}

fn candidate_texts(runtime: &mut Runtime<Fixture>) -> Vec<String> {
    runtime
        .all_candidates()
        .candidates
        .into_iter()
        .map(|candidate| candidate.text)
        .collect()
}

#[test]
fn a_long_sentence_keeps_three_readings_together_on_the_first_page() {
    let mut runtime = lattice_runtime(&[
        ("根据官方新闻稿", 8),
        ("根据", 0),
        ("跟", 0),
        ("根据官房新闻稿", 8),
        ("根据关防新闻稿", 8),
        ("根据官方新闻高", 8),
        ("根据官方新闻告", 8),
    ]);
    assert_eq!(
        candidate_texts(&mut runtime),
        [
            "根据官方新闻稿",
            "根据官房新闻稿",
            "根据关防新闻稿",
            "根据官方新闻高",
            "根据",
            "跟",
            "根据官方新闻告",
        ]
    );
}

#[test]
fn the_kept_readings_follow_the_first_one_wherever_it_sits() {
    let mut runtime = lattice_runtime(&[
        ("根据", 0),
        ("通过虚开发票", 8),
        ("跟", 0),
        ("通过需开发票", 8),
        ("通过虚开发飘", 8),
        ("通过需开发飘", 8),
    ]);
    assert_eq!(
        candidate_texts(&mut runtime),
        [
            "根据",
            "通过虚开发票",
            "通过需开发票",
            "通过虚开发飘",
            "通过需开发飘",
            "跟",
        ]
    );
}

#[test]
fn a_two_character_reading_keeps_only_the_first() {
    let mut runtime =
        lattice_runtime(&[("你好", 8), ("倪好", 8), ("你号", 8), ("你", 0), ("尼", 0)]);
    assert_eq!(
        candidate_texts(&mut runtime),
        ["你好", "你", "尼", "倪好", "你号"]
    );
}

#[test]
fn only_readings_that_answer_the_whole_key_are_counted() {
    // 部分按键生成的片段（answers_key = false）不算整句次选，而回答整码的异长切分（如 NeuralKeyboard）仍可编组。
    let rows = [
        ("根据官方新闻稿", 8, true),
        ("根据官方", 8, false),
        ("根据", 0, false),
        ("根据官房新闻稿", 8, true),
        ("根据关防新闻稿", 8, true),
        ("根据官方新闻高", 8, true),
    ];
    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: rows.iter().map(|(text, _, _)| (*text).to_owned()).collect(),
            sources: rows.iter().map(|(_, source, _)| *source).collect(),
            codes: vec![String::new(); rows.len()],
            answers_key: rows.iter().map(|(_, _, answers)| *answers).collect(),
            ..Fixture::default()
        },
        9,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert_eq!(
        candidate_texts(&mut runtime),
        [
            "根据官方新闻稿",
            "根据官房新闻稿",
            "根据关防新闻稿",
            "根据官方新闻高",
            "根据官方",
            "根据",
        ]
    );

    let mut mixed_widths = lattice_runtime(&[
        ("根据官方词", 8),
        ("根据词", 0),
        ("根剧", 0),
        ("跟据方圆", 11),
        ("跟居", 0),
        ("根", 0),
        ("跟聚芳园", 17),
        ("根据官方辞", 8),
    ]);
    assert_eq!(
        candidate_texts(&mut mixed_widths),
        [
            "根据官方词",
            "跟据方圆",
            "跟聚芳园",
            "根据官方辞",
            "根据词",
            "根剧",
            "跟居",
            "根",
        ]
    );
}

#[test]
fn single_character_lattice_rows_are_left_alone() {
    // Japanese kana: あ and ア are both Generated and both one character.
    let mut runtime = lattice_runtime(&[("あ", 8), ("ア", 8), ("阿", 0)]);
    assert_eq!(candidate_texts(&mut runtime), ["あ", "ア", "阿"]);
}

#[test]
fn a_regrouped_reading_commits_what_its_seat_shows() {
    let mut runtime = lattice_runtime(&[
        ("根据官方新闻稿", 8),
        ("根据", 0),
        ("根据官房新闻稿", 8),
        ("根据关防新闻稿", 8),
        ("根据官方新闻高", 8),
    ]);
    let view = runtime.view();
    assert_eq!(view.candidates[1].text, "根据官房新闻稿");
    assert_eq!(view.candidates[4].text, "根据");
    let transition = runtime
        .dispatch(Action::Select(view.candidates[1].id))
        .unwrap();
    assert_eq!(transition.commit.as_deref(), Some("根据官房新闻稿"));

    let mut runtime = lattice_runtime(&[
        ("根据官方新闻稿", 8),
        ("根据", 0),
        ("根据官房新闻稿", 8),
        ("根据关防新闻稿", 8),
        ("根据官方新闻高", 8),
    ]);
    let view = runtime.view();
    let transition = runtime
        .dispatch(Action::Select(view.candidates[4].id))
        .unwrap();
    assert_eq!(transition.commit.as_deref(), Some("根据"));
}

#[test]
fn regrouping_twice_changes_nothing() {
    // The settle pass runs the same regrouping over a list that already went through it.
    let mut runtime = lattice_runtime(&[
        ("根据官方新闻稿", 8),
        ("根据", 0),
        ("根据官房新闻稿", 8),
        ("根据关防新闻稿", 8),
        ("根据官方新闻高", 8),
    ]);
    let before = candidate_texts(&mut runtime);
    assert!(!runtime.demote_runner_up_readings());
    assert_eq!(candidate_texts(&mut runtime), before);
}

/// Records every rescoring context the runtime hands over.
struct RecordsContext {
    inner: Fixture,
    contexts: Vec<String>,
}

impl InputEngine for RecordsContext {
    fn set_rescoring_context(&mut self, context: &str) {
        self.contexts.push(context.to_owned());
    }
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        self.inner.snapshot()
    }
    fn character(&mut self, value: u8, shift: bool) -> Result<EngineResult, RuntimeError> {
        self.inner.character(value, shift)
    }
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError> {
        self.inner.command(command)
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.inner.select(index)
    }
    fn select_edge(
        &mut self,
        index: usize,
        edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        self.inner.select_edge(index, edge)
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.inner.finish(index)
    }
    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        self.inner.punctuation(value)
    }
}

fn recording_engine() -> RecordsContext {
    RecordsContext {
        inner: Fixture {
            local_mode: "none".into(),
            words: vec!["会议".into(), "回忆".into()],
            ..Fixture::default()
        },
        contexts: Vec::new(),
    }
}

#[test]
fn committed_text_reaches_the_engine_rescoring_context() {
    let mut runtime = Runtime::new(recording_engine(), 5).unwrap();
    runtime.focus(true).unwrap();
    assert_eq!(runtime.engine.contexts.last().map(String::as_str), Some(""));

    runtime.seed_context("明天开");
    assert_eq!(
        runtime.engine.contexts.last().map(String::as_str),
        Some("明天开")
    );

    runtime
        .dispatch(Action::Character {
            value: b'h',
            shift: false,
        })
        .unwrap();
    let transition = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(transition.commit.as_deref(), Some("会议"));
    assert_eq!(
        runtime.engine.contexts.last().map(String::as_str),
        Some("明天开会议")
    );
    assert_eq!(runtime.ai_context, "明天开会议");

    // Leaving the client ends the sentence for the Engine's models as it does for the AI provider.
    runtime.focus(false).unwrap();
    assert_eq!(runtime.engine.contexts.last().map(String::as_str), Some(""));
}

#[test]
fn clearing_the_context_starts_the_next_seed_from_nothing() {
    let mut runtime = Runtime::new(recording_engine(), 5).unwrap();
    runtime.focus(true).unwrap();
    runtime.seed_context("第一句");
    // Cancel keeps what was committed, which is why a per-case seed needs the explicit clear.
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert_eq!(runtime.ai_context, "第一句");

    runtime.clear_context();
    assert!(runtime.ai_context.is_empty());
    assert_eq!(runtime.engine.contexts.last().map(String::as_str), Some(""));

    runtime.seed_context("第二句");
    assert_eq!(runtime.ai_context, "第二句");
    assert_eq!(
        runtime.engine.contexts.last().map(String::as_str),
        Some("第二句")
    );
}

#[test]
fn a_replacement_engine_inherits_the_committed_text() {
    let mut runtime = Runtime::new(recording_engine(), 5).unwrap();
    runtime.focus(true).unwrap();
    runtime.seed_context("上一句");
    runtime.replace_engine(recording_engine(), 5).unwrap();
    assert_eq!(runtime.engine.contexts, ["上一句"]);
}

#[derive(Default)]
struct ContextAndResetEngine {
    inner: Fixture,
    contexts: Vec<String>,
    resets: usize,
}

impl InputEngine for ContextAndResetEngine {
    fn set_rescoring_context(&mut self, context: &str) {
        self.contexts.push(context.to_owned());
    }
    fn reset_context(&mut self) {
        self.resets += 1;
    }
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        self.inner.snapshot()
    }
    fn character(&mut self, value: u8, shift: bool) -> Result<EngineResult, RuntimeError> {
        self.inner.character(value, shift)
    }
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError> {
        self.inner.command(command)
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.inner.select(index)
    }
    fn select_edge(
        &mut self,
        index: usize,
        edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        self.inner.select_edge(index, edge)
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.inner.finish(index)
    }
    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        self.inner.punctuation(value)
    }
}

#[test]
fn surrounding_context_syncs_cleanly_preserves_continuous_suffix_and_resets_on_cursor_jump() {
    let mut runtime = Runtime::new(ContextAndResetEngine::default(), 5).unwrap();
    runtime.focus(true).unwrap();
    let resets_after_focus = runtime.engine.resets;

    // 同步现成文本后空闲态不产生任何候选或预编辑，同时清理附件占位符与控制字符。
    runtime.set_surrounding_context("这是一只\u{fffc}\u{0007}小猫，");
    assert_eq!(runtime.ai_context, "这是一只小猫，");
    assert!(msime_client_core::is_bounded_text_with_options(
        &runtime.ai_context,
        1024,
        true
    ));
    assert_eq!(
        runtime.engine.contexts.last().map(String::as_str),
        Some("这是一只小猫，")
    );
    assert_eq!(runtime.engine.resets, resets_after_focus + 1);
    assert!(runtime.view().candidates.is_empty());
    assert!(runtime.view().editing_text.is_empty());

    // 同一位置连续输入：编辑器报出的较短尾部若与当前已提交上文后缀一致，不切断词序链也不缩短上文。
    runtime.seed_context("它很可爱");
    assert_eq!(runtime.ai_context, "这是一只小猫，它很可爱");
    let resets_before_continue = runtime.engine.resets;
    let contexts_before_continue = runtime.engine.contexts.len();
    runtime.set_surrounding_context("它很可爱");
    assert_eq!(runtime.ai_context, "这是一只小猫，它很可爱");
    assert_eq!(runtime.engine.resets, resets_before_continue);
    assert_eq!(runtime.engine.contexts.len(), contexts_before_continue);

    // 挪动光标到另一段文字或清空文档时，切断旧词序链并替换为当前光标前文本。
    runtime.set_surrounding_context("美好的");
    assert_eq!(runtime.ai_context, "美好的");
    assert_eq!(runtime.engine.resets, resets_before_continue + 1);

    runtime.set_surrounding_context("");
    assert!(runtime.ai_context.is_empty());
    assert_eq!(runtime.engine.resets, resets_before_continue + 2);
}
fn real_engine_options(root: &std::path::Path) -> msime_engine::host::EngineOptions {
    let path = |name: &str| {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.to_str().unwrap().to_owned()
    };
    msime_engine::host::EngineOptions {
        resources: path("resources"),
        user_data: path("user"),
        cache: path("cache"),
        dictionaries: path("dictionaries"),
        first_page_rows: 6,
        scheme: 0,
        enabled_schemes: msime_engine::SchemeSet::ALL,
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
        english_minimum_prefix: 2,
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
        sentence_association: msime_engine::host::SentenceAssociationOptions {
            word_lattice: true,
            show_next_on_duplicate: false,
            ..Default::default()
        },
        rescoring_context: String::new(),
        sentence_alternatives: true,
    }
}

fn prediction_runtime() -> (tempfile::TempDir, Runtime) {
    let directory = tempfile::tempdir().unwrap();
    let mut options = real_engine_options(directory.path());
    options.next_word_prediction = true;
    let database = std::path::Path::new(&options.dictionaries).join("msime-pinyin.db");
    rusqlite::Connection::open(database)
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_1_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_1_c VALUES('chi','c','吃',1000);
         CREATE TABLE tbl_2_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);
         INSERT INTO tbl_2_c VALUES('chi''fan','cf','吃饭',100);
         INSERT INTO tbl_2_c VALUES('chi''hao','ch','吃好',90);",
        )
        .unwrap();
    let engine = msime_engine::host::Session::new(&options).unwrap();
    let mut runtime = Runtime::new(engine, 6).unwrap();
    runtime.focus(true).unwrap();
    for value in b"chi" {
        runtime
            .dispatch(Action::Character {
                value: *value,
                shift: false,
            })
            .unwrap();
    }
    let id = runtime
        .view()
        .candidates
        .iter()
        .find(|row| row.text == "吃")
        .unwrap()
        .id;
    assert_eq!(
        runtime
            .dispatch(Action::Select(id))
            .unwrap()
            .commit
            .as_deref(),
        Some("吃")
    );
    (directory, runtime)
}

#[test]
fn next_word_prediction_is_idle_and_only_explicit_selection_commits_it() {
    let (_directory, mut runtime) = prediction_runtime();
    assert!(runtime.is_idle());
    assert!(!runtime.has_composition());
    assert!(runtime.view().prediction);
    let id = runtime
        .view()
        .candidates
        .iter()
        .find(|row| row.text == "饭")
        .unwrap()
        .id;
    assert_eq!(
        runtime
            .dispatch(Action::Select(id))
            .unwrap()
            .commit
            .as_deref(),
        Some("饭")
    );
}

#[test]
fn next_word_prediction_passes_space_return_and_digits_to_the_editor() {
    for action in [
        Action::SelectHighlighted,
        Action::Command(Command::CommitRaw),
        Action::Character {
            value: b'1',
            shift: false,
        },
        Action::Command(Command::Backspace),
        Action::Finish,
    ] {
        let (_directory, mut runtime) = prediction_runtime();
        let transition = runtime.dispatch(action).unwrap();
        assert!(!transition.handled && transition.commit.is_none());
        assert!(transition.view.candidates.is_empty());
    }
}

#[test]
fn next_word_prediction_does_not_survive_focus_or_a_new_generation() {
    let (_directory, mut runtime) = prediction_runtime();
    let id = runtime.view().candidates[0].id;
    runtime
        .dispatch(Action::Character {
            value: b'n',
            shift: false,
        })
        .unwrap();
    assert!(matches!(
        runtime.dispatch(Action::Select(id)),
        Err(RuntimeError::StaleCandidate)
    ));
    let (_directory, mut runtime) = prediction_runtime();
    let transition = runtime.focus(false).unwrap();
    assert!(transition.commit.is_none() && transition.view.candidates.is_empty());
}

#[test]
fn next_word_prediction_privacy_gate_clears_and_suppresses_suggestions_immediately() {
    let (_directory, mut runtime) = prediction_runtime();
    let id = runtime.view().candidates[0].id;
    runtime.set_prediction_allowed(false).unwrap();
    assert!(runtime.view().candidates.is_empty());
    assert!(matches!(
        runtime.dispatch(Action::Select(id)),
        Err(RuntimeError::StaleCandidate)
    ));
    for value in b"chi" {
        runtime
            .dispatch(Action::Character {
                value: *value,
                shift: false,
            })
            .unwrap();
    }
    let id = runtime
        .view()
        .candidates
        .iter()
        .find(|row| row.text == "吃")
        .unwrap()
        .id;
    assert_eq!(
        runtime
            .dispatch(Action::Select(id))
            .unwrap()
            .commit
            .as_deref(),
        Some("吃")
    );
    assert!(runtime.view().candidates.is_empty());
}

#[test]
fn next_word_prediction_leaves_navigation_and_shortcuts_to_the_editor() {
    for (index, action) in [
        Action::NextCandidate,
        Action::PreviousCandidate,
        Action::NextPage,
        Action::PreviousPage,
        Action::FirstCandidateOrHome,
        Action::LastCandidate,
        Action::Command(Command::MoveRight),
        Action::Command(Command::ConversionLeft),
        Action::SegmentMoveLeft,
        Action::SegmentMoveRight,
    ]
    .into_iter()
    .enumerate()
    {
        let (_directory, mut runtime) = prediction_runtime();
        runtime.set_left_right_navigation(true);
        let transition = runtime.dispatch(action).unwrap();
        assert!(
            !transition.handled && transition.commit.is_none(),
            "action {index}"
        );
        assert!(transition.view.candidates.is_empty(), "action {index}");
    }
}

#[test]
fn next_word_prediction_is_not_held_as_a_phrase_piece() {
    let (_directory, mut runtime) = prediction_runtime();
    runtime.set_phrase_preedit(true);
    let composition_epoch = runtime.composition_epoch();
    let id = runtime.view().candidates[0].id;
    let transition = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(transition.commit.as_deref(), Some("饭"));
    assert!(transition.view.editing_text.is_empty());
    assert!(runtime.is_idle());
    assert_eq!(runtime.composition_epoch(), composition_epoch);
    assert!(runtime.focus(false).unwrap().commit.is_none());
}

#[test]
fn super_jianpin_capital_word_uses_literal_candidates_and_space_commit() {
    for scheme in [0, 1] {
        let directory = tempfile::tempdir().unwrap();
        let mut options = real_engine_options(directory.path());
        options.scheme = scheme;
        let session = msime_engine::host::Session::new(&options).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        for text in ["JIT", "JSON"] {
            for byte in text.bytes() {
                let transition = runtime
                    .dispatch(Action::Character {
                        value: byte,
                        shift: true,
                    })
                    .unwrap();
                assert!(transition.handled && transition.commit.is_none());
            }
            assert_eq!(runtime.view().local_mode, "none");
            assert_eq!(runtime.view().editing_text, text);
            assert_eq!(texts(&runtime.view()), [text]);
            let committed = runtime.dispatch(Action::SelectHighlighted).unwrap();
            assert_eq!(committed.commit.as_deref(), Some(text));
            assert!(committed.view.editing_text.is_empty());
        }
        for byte in b"JIT" {
            runtime
                .dispatch(Action::Character {
                    value: *byte,
                    shift: true,
                })
                .unwrap();
        }
        let erased = runtime
            .dispatch(Action::Command(Command::Backspace))
            .unwrap();
        assert_eq!(erased.view.editing_text, "JI");
        assert_eq!(erased.view.local_mode, "none");
        let committed = runtime
            .dispatch(Action::Command(Command::CommitRaw))
            .unwrap();
        assert_eq!(committed.commit.as_deref(), Some("JI"));
    }
}

#[test]
fn bare_shortcut_rejected_capitals_stay_in_the_runtime_composition() {
    for (entry, continuation) in [(b'U', b'V'), (b'U', b'I'), (b'K', b'A'), (b'T', b'A')] {
        let directory = tempfile::tempdir().unwrap();
        let session =
            msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        runtime
            .dispatch(Action::Character {
                value: entry,
                shift: true,
            })
            .unwrap();
        let transition = runtime
            .dispatch(Action::Character {
                value: continuation,
                shift: true,
            })
            .unwrap();
        let expected = String::from_utf8(vec![entry, continuation]).unwrap();
        assert!(transition.handled);
        assert!(transition.commit.is_none());
        assert_eq!(transition.view.local_mode, "none");
        assert_eq!(transition.view.editing_text, expected);
        let erased = runtime
            .dispatch(Action::Command(Command::Backspace))
            .unwrap();
        assert!(erased.commit.is_none());
        assert_eq!(erased.view.editing_text, (entry as char).to_string());
        assert_eq!(erased.view.local_mode, "none");
        let committed = runtime
            .dispatch(Action::Command(Command::CommitRaw))
            .unwrap();
        assert_eq!(
            committed.commit.as_deref(),
            Some((entry as char).to_string().as_str())
        );
    }
}

#[test]
fn rejected_local_key_commits_the_prefix_and_passes_the_current_key_to_the_host() {
    for (entry, spelling, rejected) in [
        (b'U', "", b'i'),
        (b'U', "0", b'I'),
        (b'K', "a", b'A'),
        (b'T', "r", b'A'),
        (b'E', "", b'1'),
        (b'U', "", b'!'),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let session =
            msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        runtime
            .dispatch(Action::Character {
                value: entry,
                shift: true,
            })
            .unwrap();
        type_characters(&mut runtime, spelling);
        let transition = runtime
            .dispatch(Action::Character {
                value: rejected,
                shift: rejected.is_ascii_uppercase(),
            })
            .unwrap();
        assert!(!transition.handled);
        assert_eq!(
            transition.commit.as_deref(),
            Some(format!("{}{spelling}", entry as char).as_str())
        );
        assert_eq!(transition.view.local_mode, "none");
        assert!(transition.view.editing_text.is_empty());
        assert!(transition.view.candidates.is_empty());
    }
}

#[test]
fn direct_local_caret_preserves_the_mode_through_runtime_edits() {
    let directory = tempfile::tempdir().unwrap();
    let session = msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();
    runtime
        .dispatch(Action::Character {
            value: b'U',
            shift: true,
        })
        .unwrap();
    type_characters(&mut runtime, "0041");
    runtime.set_caret(Some(0)).unwrap();

    let inserted = character(&mut runtime, b'+');

    assert!(
        inserted.handled && inserted.commit.is_none(),
        "{inserted:?}"
    );
    assert_eq!(inserted.view.editing_text, "U+0041");
    assert_eq!(inserted.view.caret_position, 2);
    assert_eq!(texts(&inserted.view)[0], "A");
    runtime.set_caret(Some(0)).unwrap();
    let deleted = runtime
        .dispatch(Action::Command(Command::DeleteForward))
        .unwrap();
    assert!(deleted.handled && deleted.commit.is_none());
    assert_eq!(deleted.view.editing_text, "U0041");
    assert_eq!(deleted.view.caret_position, 1);
    assert_eq!(texts(&deleted.view)[0], "A");
    let committed = runtime
        .dispatch(Action::Command(Command::CommitRaw))
        .unwrap();
    assert_eq!(committed.commit.as_deref(), Some("U0041"));
    assert!(committed.view.editing_text.is_empty());
}

#[test]
fn caret_rejected_local_keys_keep_the_commit_and_pass_the_key_to_the_host() {
    for (entry, spelling, rejected) in [(b'E', "ab", b'1'), (b'U', "0041", b'g')] {
        let directory = tempfile::tempdir().unwrap();
        let session =
            msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        runtime
            .dispatch(Action::Character {
                value: entry,
                shift: true,
            })
            .unwrap();
        for value in spelling.bytes() {
            runtime
                .dispatch(Action::Character {
                    value,
                    shift: false,
                })
                .unwrap();
        }
        runtime
            .dispatch(Action::Command(Command::MoveHome))
            .unwrap();

        let transition = runtime
            .dispatch(Action::Character {
                value: rejected,
                shift: false,
            })
            .unwrap();
        assert!(!transition.handled);
        assert_eq!(
            transition.commit,
            Some(format!("{}{spelling}", entry as char))
        );
        assert_eq!(transition.view.local_mode, "none");
        assert!(transition.view.editing_text.is_empty());
        assert!(transition.view.candidates.is_empty());
    }
}

#[test]
fn local_mode_punctuation_action_still_commits_the_prefix_with_chinese_punctuation() {
    let directory = tempfile::tempdir().unwrap();
    let session = msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();
    runtime
        .dispatch(Action::Character {
            value: b'U',
            shift: true,
        })
        .unwrap();
    let transition = runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    assert!(transition.handled);
    assert_eq!(transition.commit.as_deref(), Some("U。"));
    assert_eq!(transition.view.local_mode, "none");
}

#[test]
fn unicode_mode_digits_compose_a_code_point_rather_than_picking_a_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let session = msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();

    let shift_u = runtime
        .dispatch(Action::Character {
            value: b'U',
            shift: true,
        })
        .unwrap();
    assert_eq!(shift_u.view.local_mode, "unicode");

    for value in *b"4e2d" {
        let transition = runtime
            .dispatch(Action::Character {
                value,
                shift: false,
            })
            .unwrap();
        // A digit read as a candidate index would commit here and leave the mode.
        assert!(
            transition.commit.is_none(),
            "{} committed instead of extending the code point",
            value as char
        );
        assert_eq!(transition.view.local_mode, "unicode");
    }
    assert_eq!(runtime.view().editing_text, "U4e2d");
    assert!(runtime
        .view()
        .candidates
        .iter()
        .any(|candidate| candidate.text == "中"));
}

/// Tab and Shift+Tab page the candidate list, so with no candidates on screen the paging actions must not eat the key: the host has to insert the Tab or move focus as it would without the input method.
#[test]
fn idle_paging_passes_through_on_pinyin() {
    let assert_passes_through = |runtime: &mut Runtime<Fixture>| {
        for action in [Action::NextPage, Action::PreviousPage] {
            let transition = runtime.dispatch(action).unwrap();
            assert!(!transition.handled);
            assert!(transition.commit.is_none());
            assert!(transition.view.candidates.is_empty());
            assert_eq!(transition.view.preedit, "");
            assert_eq!(transition.view.reading, "");
            assert_eq!(transition.view.editing_text, "");
        }
    };

    // Focused with nothing typed.
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    assert_eq!(runtime.view().scheme, 0);
    assert_passes_through(&mut runtime);

    // Right after a commit the list is gone again, and paging must not reach back into it.
    let typed = type_key(&mut runtime);
    assert!(typed.handled);
    assert!(!typed.view.candidates.is_empty());
    let committed = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(committed.commit.as_deref(), Some("candidate-0"));
    assert!(committed.view.candidates.is_empty());
    assert_eq!(committed.view.reading, "");
    assert_passes_through(&mut runtime);
}

fn texts(view: &View) -> Vec<String> {
    view.candidates
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect()
}

/// An Engine whose digits end the composition with a commit and are left to the host, as Korean digits are with the Hanja list closed, while it still shows candidates.
struct DigitCommitsEngine {
    reading: String,
}
impl InputEngine for DigitCommitsEngine {
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        let candidates = if self.reading.is_empty() {
            Vec::new()
        } else {
            ["甲", "乙"]
                .into_iter()
                .enumerate()
                .map(|(index, word)| {
                    let mut entry = msime_engine::CandidateEntry::new(word, &self.reading, 0);
                    entry.answers_key = true;
                    entry.origin = msime_engine::CandidateOrigin::EngineRow(index);
                    entry
                })
                .collect()
        };
        Ok(EngineSnapshot {
            prediction: false,
            scheme: 0,
            candidates,
            microsoft_shuangpin: false,
            shuangpin_profile: "xiaohe".into(),
            local_mode: "none".into(),
            spelling_symbols: String::new(),
            dedicated_english: false,
            preedit: self.reading.clone(),
            reading: self.reading.clone(),
            display_reading: self.reading.clone(),
            editing_text: self.reading.clone(),
            caret_position: self.reading.len(),
            segment_raw_boundaries: Vec::new(),
            ..EngineSnapshot::default()
        })
    }
    fn character(&mut self, value: u8, _shift: bool) -> Result<EngineResult, RuntimeError> {
        if value.is_ascii_digit() {
            return Ok(EngineResult {
                handled: false,
                has_commit: true,
                commit: std::mem::take(&mut self.reading),
                diagnostic: String::new(),
            });
        }
        self.reading.push(value as char);
        Ok(empty_result(true))
    }
    fn command(&mut self, _command: Command) -> Result<EngineResult, RuntimeError> {
        self.reading.clear();
        Ok(empty_result(true))
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.reading.clear();
        Ok(EngineResult {
            handled: true,
            has_commit: true,
            commit: ["甲", "乙"][index].to_owned(),
            diagnostic: String::new(),
        })
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        self.select(index)
    }
    fn punctuation(&mut self, _value: u8) -> Result<EngineResult, RuntimeError> {
        Ok(empty_result(false))
    }
    fn select_edge(
        &mut self,
        index: usize,
        _edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        self.select(index)
    }
}

/// 每页十个时数字键 0 选第十个；每页不到十个时 0 不是选词键，照旧交回宿主，和改动前一样。#6679
#[test]
fn zero_picks_the_tenth_candidate_only_on_a_page_of_ten() {
    let words: Vec<String> = (1..=12).map(|index| format!("词{index}")).collect();
    let type_h = |runtime: &mut Runtime<Fixture>| {
        runtime
            .dispatch(Action::Character {
                value: b'h',
                shift: false,
            })
            .unwrap();
    };
    let zero = Action::Character {
        value: b'0',
        shift: false,
    };

    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: words.clone(),
            ..Fixture::default()
        },
        10,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    type_h(&mut runtime);
    assert_eq!(runtime.view().candidates.len(), 10);
    let picked = runtime.dispatch(zero).unwrap();
    assert!(picked.handled);
    assert_eq!(picked.commit.as_deref(), Some("词10"));

    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words,
            ..Fixture::default()
        },
        9,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    type_h(&mut runtime);
    let passed = runtime
        .dispatch(Action::Character {
            value: b'0',
            shift: false,
        })
        .unwrap();
    assert!(!passed.handled);
    assert!(passed.commit.is_none());
    assert_eq!(runtime.view().candidates.len(), 9);
}

#[test]
fn page_size_accepts_one_to_ten() {
    assert!(matches!(
        Runtime::new(Fixture::default(), 0),
        Err(RuntimeError::InvalidPageSize)
    ));
    assert!(matches!(
        Runtime::new(Fixture::default(), 11),
        Err(RuntimeError::InvalidPageSize)
    ));
    let mut runtime = Runtime::new(Fixture::default(), 10).unwrap();
    assert!(runtime.set_page_size(10).is_ok());
    assert!(matches!(
        runtime.set_page_size(11),
        Err(RuntimeError::InvalidPageSize)
    ));
}

/// 引擎的首屏门槛问的是「首页放得下几行」，和分页同源。宿主在 fcitx5 的「9 个候选」这类菜单里改了每页候选数
/// 时不会重建引擎，不在这里下发，门槛就一直是重建时那一份：每页从 6 改成 9，有 6 到 8 个精确同音词的输入
/// 整组补全词都被丢掉，而窗口其实有 9 个座位。
#[test]
fn resizing_the_page_tells_the_engine_its_first_page_gate() {
    let mut runtime = Runtime::new(Fixture::default(), 6).unwrap();
    assert_eq!(runtime.engine.first_page_rows, 0, "the gate starts unset");
    runtime.set_page_size(9).unwrap();
    assert_eq!(runtime.engine.first_page_rows, 9);
    runtime.set_page_size(3).unwrap();
    assert_eq!(runtime.engine.first_page_rows, 3);
}

/// A digit the Engine already answered with a commit is not also a page selection: selecting would replace the commit, and the text it carried - a Korean syllable - would be lost.
#[test]
fn a_digit_that_already_committed_is_not_also_a_selection() {
    let mut runtime = Runtime::new(
        DigitCommitsEngine {
            reading: String::new(),
        },
        5,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    runtime
        .dispatch(Action::Character {
            value: b'x',
            shift: false,
        })
        .unwrap();
    assert_eq!(runtime.view().candidates.len(), 2);
    let digit = runtime
        .dispatch(Action::Character {
            value: b'1',
            shift: false,
        })
        .unwrap();
    assert!(!digit.handled);
    assert_eq!(digit.commit.as_deref(), Some("x"));
    assert!(digit.view.candidates.is_empty());
}

/// A digit the scheme spells with is never a page selection, even when the Engine lets it go with candidates showing; outside a local mode the spelling symbols are the scheme's own keys (a Zhuyin tone or phonetic key, a VNI mark).
#[test]
fn a_spelling_digit_the_engine_let_go_is_not_a_selection() {
    let mut runtime = runtime();
    runtime.focus(true).unwrap();
    runtime.engine.spelling_symbols = "0123456789".into();
    for value in *b"ab" {
        runtime
            .dispatch(Action::Character {
                value,
                shift: false,
            })
            .unwrap();
    }
    assert!(!runtime.view().candidates.is_empty());
    let digit = runtime
        .dispatch(Action::Character {
            value: b'2',
            shift: false,
        })
        .unwrap();
    assert!(!digit.handled);
    assert!(digit.commit.is_none());
    assert_eq!(digit.view.editing_text, "ab");

    // The same digit picks the row once the scheme no longer spells with it; the next key's snapshot is where the runtime learns that.
    runtime.engine.spelling_symbols.clear();
    runtime
        .dispatch(Action::Character {
            value: b'c',
            shift: false,
        })
        .unwrap();
    let picked = runtime
        .dispatch(Action::Character {
            value: b'2',
            shift: false,
        })
        .unwrap();
    assert_eq!(picked.commit.as_deref(), Some("candidate-1"));
}

fn generated_mode_runtime(directory: &std::path::Path) -> Runtime {
    let mut options = real_engine_options(directory);
    options.local_expression = true;
    options.local_command = true;
    options.local_mention = true;
    options.mention_entries = vec![msime_engine::host::MentionEntry {
        phone: String::new(),
        text: "张三".into(),
        key: "zhang'san".into(),
    }];
    let session = msime_engine::host::Session::new(&options).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();
    runtime
}

fn character(runtime: &mut Runtime, value: u8) -> Transition {
    runtime
        .dispatch(Action::Character {
            value,
            shift: value.is_ascii_uppercase(),
        })
        .unwrap()
}

#[test]
fn literal_capitals_survive_runtime_space_selection_with_lowercase_chinese() {
    for (scheme, input, expected) in [
        (0, "Fnihao", "F你好"),
        (1, "Fnihc", "F你好"),
        (0, "NIHAO", "NIHAO"),
        (1, "NIHC", "NIHC"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut options = real_engine_options(directory.path());
        options.scheme = scheme;
        rusqlite::Connection::open(std::path::Path::new(&options.dictionaries).join(msime_engine::assets::MAIN_DICTIONARY))
            .unwrap()
            .execute_batch("CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER); INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',100);")
            .unwrap();
        let session = msime_engine::host::Session::new(&options).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        for value in input.bytes() {
            let transition = character(&mut runtime, value);
            assert!(transition.handled && transition.commit.is_none());
        }
        assert_eq!(runtime.view().candidates[0].text, expected);
        let selected = runtime.dispatch(Action::SelectHighlighted).unwrap();
        assert_eq!(selected.commit.as_deref(), Some(expected));
        assert!(selected.view.editing_text.is_empty());
    }
}

#[test]
fn mention_places_switch_on_live_and_carry_their_parent() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    character(&mut runtime, b'@');
    for value in *b"shenzhen" {
        character(&mut runtime, value);
    }
    assert!(runtime
        .view()
        .candidates
        .iter()
        .all(|candidate| candidate.text != "深圳市"));
    runtime.set_mention_places(true).unwrap();
    let view = runtime.view();
    assert_eq!(view.candidates[0].text, "深圳市");
    assert_eq!(view.candidates[0].annotation, "广东省");
    assert_eq!(view.candidates[0].code, "shen'zhen'shi");
    runtime.set_mention_places(false).unwrap();
    assert!(runtime
        .view()
        .candidates
        .iter()
        .all(|candidate| candidate.text != "深圳市"));
}

// The operators of the expression mode arrive as punctuation on hosts that classify Shift+= and Shift+8 that way. The runtime finishes a composition before it translates punctuation, which here would commit the half-typed expression; the Engine's `spelling_symbols` is what routes them back to the Engine as characters.
#[test]
fn expression_operators_sent_as_punctuation_extend_the_expression() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    let entered = character(&mut runtime, b'V');
    assert_eq!(entered.view.local_mode, "expression");
    assert_eq!(entered.view.spelling_symbols, "0123456789+-*/.()%^#$&");
    character(&mut runtime, b'1');
    for (action, value) in [
        (Action::Punctuation(b'+'), b'2'),
        (Action::PunctuationAscii(b'*'), b'3'),
        (Action::Punctuation(b'('), b'4'),
    ] {
        let transition = runtime.dispatch(action).unwrap();
        assert!(transition.handled && transition.commit.is_none());
        assert_eq!(transition.view.local_mode, "expression");
        // A digit is input here, never a candidate shortcut.
        let transition = character(&mut runtime, value);
        assert!(transition.commit.is_none(), "{value} picked a candidate");
    }
    runtime.dispatch(Action::Punctuation(b')')).unwrap();
    assert_eq!(runtime.view().editing_text, "V1+2*3(4)");
    assert!(runtime.online_query().unwrap().is_none());

    // A mark the mode does not spell with still ends the composition.
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    character(&mut runtime, b'V');
    for value in *b"2*3" {
        character(&mut runtime, value);
    }
    assert_eq!(runtime.view().candidates[0].text, "6");
    let committed = runtime.dispatch(Action::Punctuation(b',')).unwrap();
    assert_eq!(committed.commit.as_deref(), Some("6，"));
    let context = committed.commit_context.unwrap();
    assert_eq!(context.local_mode, "expression");
    assert!(!context.typing_statistics);
    assert_eq!(committed.view.local_mode, "none");
}

// The apostrophe is no spelling symbol, so a host may send it as punctuation; after a unit it separates the target instead of finishing the expression.
#[test]
fn an_apostrophe_after_a_unit_separates_the_target_on_every_route() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    for action in [
        Action::Punctuation(b'\''),
        Action::PunctuationAscii(b'\''),
        Action::Character {
            value: b'\'',
            shift: false,
        },
    ] {
        character(&mut runtime, b'V');
        for value in *b"3jin" {
            character(&mut runtime, value);
        }
        let separated = runtime.dispatch(action).unwrap();
        assert!(separated.handled && separated.commit.is_none());
        character(&mut runtime, b'g');
        assert_eq!(runtime.view().editing_text, "V3jin'g");
        assert_eq!(runtime.view().candidates[0].text, "1500克");
        assert!(runtime.online_query().unwrap().is_none());
        assert!(runtime.command_translation().is_none());
        runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    }
    // Before any unit the apostrophe is still the mark that ends the expression.
    character(&mut runtime, b'V');
    character(&mut runtime, b'3');
    let highlighted = runtime.view().candidates[0].text.clone();
    let committed = runtime.dispatch(Action::PunctuationAscii(b'\'')).unwrap();
    assert_eq!(committed.commit, Some(format!("{highlighted}'")));
    assert_eq!(committed.view.local_mode, "none");
}

// `/fy` hands its English to the host as a request of its own and takes the answer back as a row that commits it, under the runtime's generation guard.
#[test]
fn the_translate_command_round_trips_through_the_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    character(&mut runtime, b'/');
    assert!(runtime.command_translation().is_none());
    for value in *b"fyhello" {
        character(&mut runtime, value);
    }
    runtime.dispatch(Action::Punctuation(b'\'')).unwrap();
    for value in *b"world" {
        character(&mut runtime, value);
    }
    assert!(runtime.online_query().unwrap().is_none());
    let query = runtime
        .command_translation()
        .expect("a translation request");
    assert_eq!(query.text, "hello world");
    assert_eq!(query.generation, runtime.generation());

    let mut stale = query.clone();
    stale.generation -= 1;
    assert!(!runtime
        .apply_command_translation(&stale, "你好世界")
        .unwrap());
    assert!(!runtime.apply_command_translation(&query, "").unwrap());
    let before = runtime.view().candidates[0].id;
    assert!(runtime
        .apply_command_translation(&query, "你好世界")
        .unwrap());
    let view = runtime.view();
    assert_eq!(view.candidates[0].text, "你好世界");
    assert_eq!(view.candidates[0].annotation, "翻译");
    assert_eq!(view.candidates[1].text, "hello world");
    assert!(view.candidates[0].translation.is_none());
    // The identity moved on, so the old page's first ID cannot pick the new first row.
    assert_ne!(view.candidates[0].id, before);
    // A second answer for the same request is stale.
    assert!(!runtime.apply_command_translation(&query, "你好").unwrap());
    // Answered, the text is not asked for again when the host plans requests for the new view.
    assert!(runtime.command_translation().is_none());
    let picked = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(picked.commit.as_deref(), Some("你好世界"));
    assert_eq!(picked.view.local_mode, "none");

    // A host that forwards the request through its candidate translation path hands the answer back the same way, and it still becomes the row rather than a gloss.
    character(&mut runtime, b'/');
    for value in *b"fycat" {
        character(&mut runtime, value);
    }
    let query = runtime
        .command_translation()
        .expect("a translation request");
    assert!(runtime.apply_translations(query.generation, [("cat".to_owned(), "猫".to_owned())]));
    let view = runtime.view();
    assert_eq!(view.candidates[0].text, "猫");
    assert!(view.candidates.iter().all(|row| row.translation.is_none()));
    // Glosses for other text stay glosses.
    let generation = runtime.generation();
    assert!(runtime.apply_translations(generation, [("猫".to_owned(), "cat".to_owned())]));
    assert_eq!(
        runtime.view().candidates[0].translation.as_deref(),
        Some("cat")
    );
}

#[test]
fn command_translation_does_not_change_engine_after_generation_exhaustion() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    character(&mut runtime, b'/');
    for value in *b"fyhello" {
        character(&mut runtime, value);
    }
    let mut query = runtime.command_translation().unwrap();
    runtime.generation = u64::MAX;
    query.generation = u64::MAX;
    let before = runtime.engine.snapshot().unwrap().candidates;

    assert!(matches!(
        runtime.apply_command_translation(&query, "合成译文"),
        Err(RuntimeError::IdentityExhausted)
    ));
    assert_eq!(runtime.engine.snapshot().unwrap().candidates, before);
}

// A mark on a bare `/` or `@` is punctuation on every route: the mode ends and nothing from its list is committed.
#[test]
fn a_mark_on_a_bare_slash_or_at_is_not_a_pick() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    for (open, action, expected) in [
        (b'/', Action::Punctuation(b'/'), "//"),
        (b'/', Action::Punctuation(b','), "/，"),
        (b'/', Action::PunctuationAscii(b','), "/,"),
        (
            b'/',
            Action::Character {
                value: b'/',
                shift: false,
            },
            "//",
        ),
        (b'@', Action::Punctuation(b'@'), "@@"),
    ] {
        assert_eq!(character(&mut runtime, open).view.editing_text.len(), 1);
        let ended = runtime.dispatch(action).unwrap();
        assert!(ended.handled);
        assert_eq!(ended.commit.as_deref(), Some(expected));
        assert_eq!(ended.view.local_mode, "none");
    }
    // Space still takes the first row, and Enter the literal prefix.
    character(&mut runtime, b'/');
    let first = runtime.view().candidates[0].text.clone();
    let picked = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(picked.commit, Some(first));
    character(&mut runtime, b'/');
    let raw = runtime
        .dispatch(Action::Command(Command::CommitRaw))
        .unwrap();
    assert_eq!(raw.commit.as_deref(), Some("/"));
    // A host flush (focus moving, the host inserting text) keeps the literal prefix too.
    for open in *b"/@" {
        character(&mut runtime, open);
        let flushed = runtime.dispatch(Action::Finish).unwrap();
        assert_eq!(
            flushed.commit.as_deref(),
            Some(&*char::from(open).to_string())
        );
        assert_eq!(flushed.view.local_mode, "none");
    }
}

// Whether `/` and `@` open a mode follows the punctuation mode, so the view the host and the punctuation route read changes with it at once rather than at the next key.
#[test]
fn punctuation_mode_changes_refresh_the_mode_symbols() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    assert_eq!(runtime.view().spelling_symbols, "/@");
    runtime.set_chinese_punctuation_enabled(false).unwrap();
    assert!(runtime.view().spelling_symbols.is_empty());
    runtime.set_chinese_punctuation_enabled(true).unwrap();
    assert_eq!(runtime.view().spelling_symbols, "/@");
    runtime.set_punctuation_lock(2).unwrap();
    assert!(runtime.view().spelling_symbols.is_empty());
    runtime.set_punctuation_lock(0).unwrap();
    let opened = runtime.dispatch(Action::Punctuation(b'/')).unwrap();
    assert_eq!(opened.view.local_mode, "command");
}

#[test]
fn slash_and_at_open_their_modes_only_with_nothing_composed() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = generated_mode_runtime(directory.path());
    assert_eq!(runtime.view().spelling_symbols, "/@");

    // The explicit punctuation route opens the mode as the character route does.
    let opened = runtime.dispatch(Action::Punctuation(b'/')).unwrap();
    assert!(opened.handled && opened.commit.is_none());
    assert_eq!(opened.view.local_mode, "command");
    assert!(opened.view.spelling_symbols.is_empty());
    assert!(!opened.view.candidates.is_empty());
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();

    let opened = character(&mut runtime, b'@');
    assert_eq!(opened.view.local_mode, "mention");
    assert_eq!(opened.view.candidates[0].text, "张三");
    let committed = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(committed.commit.as_deref(), Some("张三"));
    assert!(!committed.commit_context.unwrap().typing_statistics);

    // A literal ASCII mark the host chose after weighing the surrounding text never opens a mode.
    let literal = runtime.dispatch(Action::PunctuationAscii(b'/')).unwrap();
    assert_eq!(literal.view.local_mode, "none");

    // With a composition `/` is punctuation: the composition is committed with the mark after it.
    for value in *b"ab" {
        character(&mut runtime, value);
    }
    assert!(!runtime.view().spelling_symbols.contains('/'));
    let finished = runtime.dispatch(Action::Punctuation(b'/')).unwrap();
    let commit = finished.commit.unwrap();
    assert!(commit.ends_with('/'), "{commit:?}");
    assert_eq!(finished.view.local_mode, "none");
    assert!(finished.commit_context.unwrap().typing_statistics);
}
fn withholding_runtime(offered: usize, withheld: usize, page_size: u8) -> Runtime<Fixture> {
    Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: (0..offered).map(|n| format!("candidate-{n}")).collect(),
            withheld: (offered..offered + withheld)
                .map(|n| format!("candidate-{n}"))
                .collect(),
            ..Fixture::default()
        },
        page_size,
    )
    .unwrap()
}

#[test]
fn paging_reaches_candidates_the_engine_withheld() {
    // Twelve offered at five a page is two full pages and a short third. Without the expansion the
    // third page is the end of the road, and the eight held back are unreachable by any key.
    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert_eq!(runtime.view().page_count, 3);
    let second = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(second.page, 1);
    // Entering the short last page is where the expansion belongs: the page is filled before it is
    // shown, rather than appearing short and then growing under the user.
    let third = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(third.page, 2);
    assert_eq!(third.page_count, 4);
    assert_eq!(third.candidates.len(), 5);
    let fourth = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(fourth.page, 3);
    assert_eq!(
        fourth.candidates.first().map(|c| c.text.as_str()),
        Some("candidate-15")
    );
}

/// 停在不满的末页再按 `+`，展开会把隐藏的候选放出来，末页随之后移。展开之后这一页已经不是
/// 末页了，高亮必须真的前进，否则候选窗换了一帧、页码还停在旧页，数字键 1 选到的是翻页前
/// 那一页的第一个候选（#9517）。
#[test]
fn paging_past_the_short_last_page_moves_the_highlight() {
    // 三个候选、每页五个：只有一页，而且不满。按 `+` 会展开隐藏的七个，候选数从 3 变成 10，
    // 末页从第 0 页后移到第 1 页——这一页已经不再是末页，高亮必须跟着走。
    let mut runtime = withholding_runtime(3, 7, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert_eq!(runtime.view().page_count, 1);
    let past = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(
        past.page, 1,
        "展开长出了新页之后高亮必须前进，否则数字键选的是翻页前那一页"
    );
    // 候选窗看到的第一行，和按 1 选中的第一个候选必须是同一行。
    assert!(!past.candidates.is_empty());
    let shown_first = past.candidates[0].text.clone();
    let first = past.candidates[0].id;
    let committed = runtime.dispatch(Action::Select(first)).unwrap();
    assert_eq!(committed.commit.as_deref(), Some(shown_first.as_str()));
}

/// 云联想的回包在组字没变时改写候选。用户还停在第一条时，高亮留在新的第一条；
/// 已经按 `+` 翻到下一页时，高亮留在那一页，不弹回第一页。再打一个字母仍从第一页开始。
#[test]
fn a_late_cloud_candidate_keeps_the_page_the_user_opened() {
    let mut paged = cloud_page_runtime();
    let second = paged.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(second.page, 1);
    assert_eq!(second.candidates[0].text, "词5");
    push_cloud_candidate(&mut paged);
    paged.refresh().unwrap();
    let kept = paged.view();
    assert_eq!(kept.page, 1);
    assert!(kept
        .candidates
        .iter()
        .any(|candidate| candidate.text == "词5" && candidate.highlighted));
    assert!(kept
        .candidates
        .iter()
        .all(|candidate| candidate.text != "云联想"));
    assert_eq!(paged.cached.candidates[1].text, "云联想");

    let typed = paged
        .dispatch(Action::Character {
            value: b'b',
            shift: false,
        })
        .unwrap()
        .view;
    assert_eq!(typed.page, 0);
    assert!(typed.candidates[0].highlighted);

    let mut stayed = cloud_page_runtime();
    push_cloud_candidate(&mut stayed);
    stayed.refresh().unwrap();
    let home = stayed.view();
    assert_eq!(home.page, 0);
    assert_eq!(home.candidates[0].text, "词0");
    assert!(home.candidates[0].highlighted);
    assert_eq!(home.candidates[1].text, "云联想");
}

/// 高亮那一条若只剩更前一页上的副本，不跟回去。翻开的页留在原地。
#[test]
fn a_promoted_candidate_does_not_pull_an_open_page_back() {
    let mut runtime = cloud_page_runtime();
    runtime.dispatch(Action::NextPage).unwrap();
    runtime.engine.words = (0..12)
        .map(|n| match n {
            1 => "词5".to_owned(),
            5 => "词1".to_owned(),
            n => format!("词{n}"),
        })
        .collect();
    runtime.engine.codes = (0..12)
        .map(|n| match n {
            1 => "ci5".to_owned(),
            5 => "ci1".to_owned(),
            n => format!("ci{n}"),
        })
        .collect();
    runtime.engine.sources = vec![0; 12];
    runtime.refresh().unwrap();
    let view = runtime.view();
    assert_eq!(view.page, 1);
    assert_eq!(view.candidates[0].text, "词1");
    assert!(view.candidates[0].highlighted);
    assert!(view
        .candidates
        .iter()
        .all(|candidate| candidate.text != "词5"));
}

fn cloud_page_runtime() -> Runtime<Fixture> {
    let count = 12;
    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: (0..count).map(|n| format!("词{n}")).collect(),
            codes: (0..count).map(|n| format!("ci{n}")).collect(),
            sources: vec![0; count],
            ..Fixture::default()
        },
        5,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime
}

fn push_cloud_candidate(runtime: &mut Runtime<Fixture>) {
    runtime.engine.words.push("云联想".into());
    runtime.engine.codes.push("yun".into());
    runtime.engine.sources.push(2);
}

#[test]
fn candidate_page_len_matches_the_published_page_without_building_rows() {
    let mut short = withholding_runtime(3, 4, 5);
    short.focus(true).unwrap();
    type_key(&mut short);
    assert_eq!(short.candidate_page_len(), 3);
    let next = short.dispatch(Action::NextPage).unwrap().view;
    // 展开后长出第二页，当前页只有剩余两行；轻量计数必须与发布页一致。
    assert_eq!(next.page, 1);
    assert_eq!(short.candidate_page_len(), 2);
    assert_eq!(short.candidate_page_len(), next.candidates.len());

    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert_eq!(runtime.candidate_page_len(), 5);
    runtime.dispatch(Action::NextPage).unwrap();
    assert_eq!(runtime.candidate_page_len(), 5);
    runtime.dispatch(Action::NextPage).unwrap();
    assert_eq!(runtime.candidate_page_len(), 5);
}

#[test]
fn translation_candidates_match_the_visible_page_without_full_rows() {
    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let light = runtime.translation_candidates().unwrap();
    let view = runtime.view();
    assert_eq!(light.generation, view.generation);
    assert_eq!(light.scheme, view.scheme);
    assert_eq!(light.local_mode, view.local_mode);
    assert_eq!(light.candidates.len(), view.candidates.len());
    for (candidate, visible) in light.candidates.iter().zip(&view.candidates) {
        assert_eq!(candidate.text, visible.text);
        assert_eq!(candidate.source, visible.source);
    }
}

#[test]
fn the_full_list_holds_what_paging_would_have_reached() {
    // Twelve offered and eight held back. Paging to the last page releases the eight; a host that
    // opens the whole list instead never paged, so it used to see only the first twelve and the
    // panel that promises everything was short by the tail of the answer.
    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    assert_eq!(runtime.all_candidates().candidates.len(), 20);
    // Asking again is stable: there is nothing left to release and the list does not shift.
    let repeated = runtime.all_candidates();
    assert_eq!(repeated.candidates.len(), 20);
    assert_eq!(
        repeated.candidates.last().map(|c| c.text.as_str()),
        Some("candidate-19")
    );
    // And it agrees with what paging reaches, which is the behaviour it is standing in for.
    let mut paged = withholding_runtime(12, 8, 5);
    paged.focus(true).unwrap();
    type_key(&mut paged);
    for _ in 0..3 {
        paged.dispatch(Action::NextPage).unwrap();
    }
    assert_eq!(paged.all_candidates().candidates.len(), 20);
}

#[test]
fn releasing_the_held_tail_publishes_a_new_generation() {
    // 放出的尾部会重排座位，旧代次的 ID 若仍有效就会按新座位选中别的候选。release 构建不执行 `debug_assert!` 里的表达式，所以要在 `--release` 下也跑这条。
    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let before = runtime.view().generation;
    let full = runtime.all_candidates();
    assert_eq!(full.candidates.len(), 20);
    assert!(full.generation > before);
    assert_eq!(runtime.view().generation, full.generation);
}

#[test]
fn full_list_does_not_reorder_after_generation_identity_is_exhausted() {
    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    runtime.generation = u64::MAX;
    let before = runtime.view();

    // Releasing the withheld tail would reorder seats but cannot allocate a new
    // generation. Keep the published IDs and their seat mapping stable instead.
    let full = runtime.all_candidates();
    assert_eq!(full.generation, u64::MAX);
    assert_eq!(full.candidates.len(), 12);
    assert_eq!(
        full.candidates
            .iter()
            .take(before.candidates.len())
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>(),
        before
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn clearing_online_candidates_does_not_change_engine_after_generation_exhaustion() {
    let directory = tempfile::tempdir().unwrap();
    let session = msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();
    type_characters(&mut runtime, "ni");
    let query = runtime.online_query().unwrap().unwrap();
    assert!(query.ai_eligible);
    assert!(runtime
        .apply_online_candidate(&query, "合成候选", 1)
        .unwrap());
    runtime.generation = u64::MAX;
    let before = runtime.engine.snapshot().unwrap().candidates;
    assert!(before.iter().any(|candidate| candidate.text == "合成候选"));

    assert!(matches!(
        runtime.clear_online_candidates(1),
        Err(RuntimeError::IdentityExhausted)
    ));
    assert_eq!(runtime.engine.snapshot().unwrap().candidates, before);
}

#[test]
fn online_candidates_do_not_change_engine_after_generation_exhaustion() {
    for batch in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let session =
            msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        type_characters(&mut runtime, "ni");
        let mut query = runtime.online_query().unwrap().unwrap();
        query.ai_assistant =
            Some(serde_json::from_value(json!({"enabled": true, "candidate_limit": 1})).unwrap());
        runtime.generation = u64::MAX;
        let before = runtime.engine.snapshot().unwrap().candidates;

        let result = if batch {
            runtime.apply_online_candidates(&query, &["合成候选".into()], 1)
        } else {
            runtime.apply_online_candidate(&query, "合成候选", 1)
        };
        assert!(matches!(result, Err(RuntimeError::IdentityExhausted)));
        assert_eq!(runtime.engine.snapshot().unwrap().candidates, before);
    }
}

#[test]
fn expansion_that_fills_the_current_page_does_not_advance_past_it() {
    // 展开只补满当前页、没有长出新页时留在原页；长出新页的场景由相邻的翻页回归覆盖。
    let mut runtime = withholding_runtime(3, 2, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    let filled = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(filled.page, 0);
    assert_eq!(filled.page_count, 1);
    assert_eq!(filled.candidates.len(), 5);
    assert_eq!(
        filled.candidates.first().map(|c| c.text.as_str()),
        Some("candidate-0")
    );
    // 所有候选已展开，再按下一页也不能越过唯一一页。
    assert_eq!(runtime.dispatch(Action::NextPage).unwrap().view.page, 0);
}

#[test]
fn walking_the_highlight_off_the_end_reaches_candidates_the_engine_withheld() {
    // Ten offered at five a page is two full pages, so the partial-last-page rule below never fires
    // and this exercises the end of the list on its own. Walking down one candidate at a time - an
    // arrow key, a wheel notch - used to stop dead on the tenth, while page-down on the same query
    // walked past it. The cap is the Engine's single-letter limit, and selection has to release it
    // for the same reason paging does.
    let mut runtime = withholding_runtime(10, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    for _ in 0..9 {
        runtime.dispatch(Action::NextCandidate).unwrap();
    }
    let last_offered = runtime.view();
    assert_eq!(last_offered.page_count, 2);
    let expanded = runtime.dispatch(Action::NextCandidate).unwrap().view;
    assert_eq!(expanded.page, 2);
    assert_eq!(expanded.page_count, 4);
    assert_eq!(
        expanded
            .candidates
            .iter()
            .find(|candidate| candidate.highlighted)
            .map(|candidate| candidate.text.as_str()),
        Some("candidate-10")
    );
}

#[test]
fn stepping_into_the_partial_last_page_fills_it_first() {
    // The page-down path already fills the short last page before entering it. Arriving at the same
    // page by selection has to look the same, or the page appears short and then grows under a
    // highlight that is already sitting in it.
    let mut runtime = withholding_runtime(12, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    for _ in 0..9 {
        runtime.dispatch(Action::NextCandidate).unwrap();
    }
    let entered = runtime.dispatch(Action::NextCandidate).unwrap().view;
    assert_eq!(entered.page, 2);
    assert_eq!(entered.candidates.len(), 5);
    assert_eq!(
        entered.candidates.first().map(|c| c.text.as_str()),
        Some("candidate-10")
    );
}

#[test]
fn the_highlight_stops_at_the_last_candidate_once_nothing_is_withheld() {
    // Expansion is not a wrap: when the Engine has nothing left, the selection stays where it is
    // rather than moving or reordering the list under it.
    let mut runtime = withholding_runtime(3, 0, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    for _ in 0..2 {
        runtime.dispatch(Action::NextCandidate).unwrap();
    }
    let end = runtime.dispatch(Action::NextCandidate).unwrap().view;
    assert_eq!(end.candidates.len(), 3);
    assert_eq!(
        end.candidates
            .iter()
            .position(|candidate| candidate.highlighted),
        Some(2)
    );
}

#[test]
fn walking_the_highlight_backwards_never_asks_for_more() {
    // Only forward motion runs into the cap. Asking the Engine to expand while moving up would
    // reorder the list the user is reading back through.
    let mut runtime = withholding_runtime(10, 8, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    for _ in 0..8 {
        runtime.dispatch(Action::NextCandidate).unwrap();
    }
    runtime.dispatch(Action::PreviousCandidate).unwrap();
    assert_eq!(runtime.view().page_count, 2);
}

#[test]
fn an_engine_withholding_nothing_pages_exactly_as_before() {
    let mut runtime = withholding_runtime(12, 0, 5);
    runtime.focus(true).unwrap();
    type_key(&mut runtime);
    for expected in [1, 2, 2, 2] {
        assert_eq!(
            runtime.dispatch(Action::NextPage).unwrap().view.page,
            expected
        );
    }
    assert_eq!(runtime.view().page_count, 3);
}

// The runtime seats online candidates and reranks the Engine's list, so the page a host renders is not in the Engine's order. A selection names a seat on that page; the Engine has to be asked for the candidate sitting there, not for whatever it holds at the same number. fcitx5-native-ai caught this as an AI candidate shown in slot 1 committing the Engine's own second candidate.
#[test]
fn selecting_a_reseated_candidate_commits_that_candidate() {
    let mut runtime = Runtime::new(
        Fixture {
            local_mode: "none".into(),
            words: vec!["本地一".into(), "本地二".into(), "AI".into()],
            codes: (0..3).map(|n| format!("code-{n}")).collect(),
            sources: vec![0, 0, 3],
            ..Fixture::default()
        },
        // 每页两个：AI 候选坐第一页最后一格，也就是引擎的第三个候选坐到第二格。
        2,
    )
    .unwrap();
    runtime.focus(true).unwrap();
    let page = type_key(&mut runtime).view.candidates;
    assert_eq!(
        page.iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>(),
        vec!["本地一", "AI"]
    );

    let done = runtime.dispatch(Action::Select(page[1].id)).unwrap();
    assert_eq!(done.commit.as_deref(), Some("AI"));
}

#[test]
fn the_full_list_seats_online_candidates_as_paging_does_and_retires_page_ids() {
    // The released tail carries a cloud candidate. Paging seats it second; the panel has to agree,
    // and the page drawn before the release must not select by the reordered seats.
    let with_cloud = || {
        let mut runtime = withholding_runtime(12, 8, 5);
        runtime.engine.sources = (0..20).map(|n| if n == 15 { 2 } else { 0 }).collect();
        runtime.engine.codes = vec![String::new(); 20];
        runtime.focus(true).unwrap();
        runtime
    };
    let mut runtime = with_cloud();
    let page = type_key(&mut runtime).view;
    let panel = runtime.all_candidates();
    assert_eq!(panel.candidates.len(), 20);
    assert_eq!(panel.candidates[1].text, "candidate-15");
    assert_eq!(panel.generation, page.generation + 1);
    assert!(matches!(
        runtime.dispatch(Action::Select(page.candidates[1].id)),
        Err(RuntimeError::StaleCandidate)
    ));

    let mut paged = with_cloud();
    type_key(&mut paged);
    for _ in 0..3 {
        paged.dispatch(Action::NextPage).unwrap();
    }
    let texts = |snapshot: CandidateSnapshot| -> Vec<String> {
        snapshot.candidates.into_iter().map(|c| c.text).collect()
    };
    assert_eq!(texts(paged.all_candidates()), texts(panel));
}

#[test]
fn a_busy_provider_keeps_only_the_newest_completed_result() {
    let query = |text: &str| OnlineQuery {
        mode: msime_engine::OnlineInputMode::Pinyin,
        scheme: 0,
        generation: 1,
        identity: text.into(),
        query_text: text.into(),
        cache_key: text.into(),
        pinyin_segments: vec![],
        cloud_eligible: true,
        ai_eligible: false,
        cloud_candidates: true,
        cloud_request: None,
        session_id: 1,
        ai_context: String::new(),
        ai_assistant: None,
        ai_cache_only: false,
    };
    let (started, first_running) = std::sync::mpsc::channel::<String>();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let worker = OnlineProviderWorker::spawn(1, move |query: &OnlineQuery| {
        started.send(query.query_text.clone()).unwrap();
        if query.query_text == "completion-barrier" {
            return None;
        }
        if query.query_text == "ni" {
            gate.lock().unwrap().recv().unwrap();
        }
        Some((query.query_text.clone(), 0))
    })
    .unwrap();
    assert!(worker.submit(query("ni")));
    assert_eq!(
        first_running.recv_timeout(Duration::from_secs(5)).unwrap(),
        "ni"
    );
    for text in ["nih", "niha", "nihao"] {
        assert!(worker.submit(query(text)));
    }
    release.send(()).unwrap();
    assert_eq!(
        first_running.recv_timeout(Duration::from_secs(5)).unwrap(),
        "nihao"
    );
    // 下一请求开始时，前一请求的结果已写入槽；屏障请求不产生新结果。
    assert!(worker.submit(query("completion-barrier")));
    assert_eq!(
        first_running.recv_timeout(Duration::from_secs(5)).unwrap(),
        "completion-barrier"
    );
    assert_eq!(
        worker.try_recv().map(|result| result.text),
        Some("nihao".into())
    );
    assert!(worker.try_recv().is_none());
    worker.shutdown();
}

// ---- 网址模式 ----

/// 用真实引擎（空词库）建一个指定方案的 runtime。
fn url_runtime(directory: &std::path::Path, scheme: u8) -> Runtime {
    let mut options = real_engine_options(directory);
    options.scheme = scheme;
    let session = msime_engine::host::Session::new(&options).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();
    runtime
}

fn type_characters(runtime: &mut Runtime, text: &str) {
    for value in text.bytes() {
        let transition = character(runtime, value);
        assert!(
            transition.handled && transition.commit.is_none(),
            "{:?} of {text:?}: {transition:?}",
            value as char
        );
    }
}

// runtime 在把标点交给引擎前会先结束组字；只有引擎在 `spelling_symbols` 里列出的触发键才改走 `character`，所以 `www` 后的 `.` 能进入网址模式而不是先上屏。
#[test]
fn url_mode_opens_on_the_punctuation_route() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "www");
    assert_eq!(runtime.view().spelling_symbols, ".@#$%&");
    let opened = runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    assert!(opened.handled && opened.commit.is_none(), "{opened:?}");
    assert_eq!(opened.view.local_mode, "url");
    assert_eq!(opened.view.editing_text, "www.");
    type_characters(&mut runtime, "google");
    let dot = runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    assert!(dot.handled && dot.commit.is_none(), "{dot:?}");
    type_characters(&mut runtime, "com");
    let committed = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(committed.commit.as_deref(), Some("www.google.com"));
    let context = committed.commit_context.unwrap();
    assert_eq!(context.local_mode, "url");
    // 网址是用户打出来的，计入打字统计。
    assert!(context.typing_statistics);
    assert_eq!(committed.view.local_mode, "none");
    assert!(committed.view.editing_text.is_empty());
}

// 宿主把 `:` 当作字符送来（Character 路由）时同样进入网址模式。
#[test]
fn url_mode_opens_on_the_character_route() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "https");
    let colon = character(&mut runtime, b':');
    assert!(colon.handled && colon.commit.is_none(), "{colon:?}");
    assert_eq!(colon.view.local_mode, "url");
    type_characters(&mut runtime, "//x.com:8080/a?b=1");
    let enter = runtime
        .dispatch(Action::Command(Command::CommitRaw))
        .unwrap();
    assert_eq!(enter.commit.as_deref(), Some("https://x.com:8080/a?b=1"));
}

// 字面标点路由（PunctuationAscii）不负责入口：宿主在这条路由上要的是字面符号，组字中的 `.` 照旧结束组字。进入网址模式后，这条路由上的网址符号都是输入。
#[test]
fn url_mode_takes_symbols_on_the_punctuation_ascii_route() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "www");
    let literal = runtime.dispatch(Action::PunctuationAscii(b'.')).unwrap();
    assert!(literal
        .commit
        .as_deref()
        .is_some_and(|text| text.ends_with('.')));
    assert_eq!(literal.view.local_mode, "none");

    type_characters(&mut runtime, "www");
    runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    type_characters(&mut runtime, "a");
    for value in *b"./?=&#" {
        let transition = runtime.dispatch(Action::PunctuationAscii(value)).unwrap();
        assert!(
            transition.handled && transition.commit.is_none(),
            "{:?}: {transition:?}",
            value as char
        );
    }
    assert_eq!(runtime.view().editing_text, "www.a./?=&#");
    assert_eq!(runtime.view().local_mode, "url");
}

#[test]
fn url_digit_is_input_not_a_pick() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "www");
    runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    for value in *b"1234567890" {
        let transition = character(&mut runtime, value);
        assert!(
            transition.handled && transition.commit.is_none(),
            "{value} picked a candidate: {transition:?}"
        );
    }
    assert_eq!(runtime.view().editing_text, "www.1234567890");
    assert_eq!(runtime.view().local_mode, "url");
}

// 网址不收的符号结束网址：先上屏网址，再接上中文标点。
#[test]
fn url_mark_outside_the_url_commits_it_before_the_mark() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "www");
    runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    type_characters(&mut runtime, "a");
    let quote = character(&mut runtime, b'"');
    assert_eq!(quote.commit.as_deref(), Some("www.a\u{201c}"));
    assert_eq!(quote.view.local_mode, "none");
}

// 删掉触发键退回组字。
#[test]
fn url_backspace_past_the_trigger_returns_to_the_composition() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "www");
    runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    let back = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap();
    assert!(back.handled && back.commit.is_none());
    assert_eq!(back.view.local_mode, "none");
    assert_eq!(back.view.editing_text, "www");
    assert_eq!(back.view.spelling_symbols, ".@#$%&");
}

// 空格上屏网址本身，不在末尾带空格；Esc 丢弃网址不上屏，回到没有本地模式的状态。
#[test]
fn url_space_commits_the_url_alone_and_escape_discards_it() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "www");
    runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    type_characters(&mut runtime, "a");
    let space = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert!(space.handled, "{space:?}");
    assert_eq!(space.commit.as_deref(), Some("www.a"));
    assert_eq!(space.view.local_mode, "none");
    assert!(space.view.editing_text.is_empty());

    type_characters(&mut runtime, "www");
    runtime.dispatch(Action::Punctuation(b'.')).unwrap();
    type_characters(&mut runtime, "a");
    let escape = runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert!(escape.commit.is_none(), "{escape:?}");
    assert_eq!(escape.view.local_mode, "none");
    assert!(escape.view.editing_text.is_empty());
}

// 邮箱后缀显示着时，数字仍是网址的一部分而不是选候选；空格上屏的是打的网址，点选后缀才上屏整个邮箱。
#[test]
fn url_email_suffixes_leave_digits_and_space_to_the_address() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "abc");
    assert_eq!(runtime.view().spelling_symbols, "@#$%&");
    let at = runtime.dispatch(Action::Punctuation(b'@')).unwrap();
    assert!(at.handled && at.commit.is_none(), "{at:?}");
    assert_eq!(at.view.local_mode, "url");
    assert_eq!(texts(&at.view)[..3], ["abc@", "abc@qq.com", "abc@163.com"]);
    let digit = character(&mut runtime, b'1');
    assert!(digit.handled && digit.commit.is_none(), "{digit:?}");
    assert_eq!(
        texts(&digit.view),
        [
            "abc@1",
            "abc@163.com",
            "abc@126.com",
            "abc@139.com",
            "abc@189.cn"
        ]
    );
    let space = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(space.commit.as_deref(), Some("abc@1"));

    type_characters(&mut runtime, "abc");
    runtime.dispatch(Action::Punctuation(b'@')).unwrap();
    let id = runtime.view().candidates[1].id;
    let picked = runtime.dispatch(Action::Select(id)).unwrap();
    assert_eq!(picked.commit.as_deref(), Some("abc@qq.com"));
    assert_eq!(picked.view.local_mode, "none");
}

// 选中组字里的 `https://` 不上屏，以它进入网址模式；选中补全了顶级域名的行同样不上屏。
#[test]
fn url_completions_picked_through_the_runtime_keep_composing() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = url_runtime(directory.path(), 0);
    type_characters(&mut runtime, "htt");
    let view = runtime.view();
    let at = texts(&view)
        .iter()
        .position(|text| text == "https://")
        .unwrap();
    let picked = runtime
        .dispatch(Action::Select(view.candidates[at].id))
        .unwrap();
    assert!(picked.handled && picked.commit.is_none(), "{picked:?}");
    assert_eq!(picked.view.local_mode, "url");
    assert_eq!(picked.view.editing_text, "https://");
    type_characters(&mut runtime, "example");
    let id = runtime.view().candidates[1].id;
    let tld = runtime.dispatch(Action::Select(id)).unwrap();
    assert!(tld.handled && tld.commit.is_none(), "{tld:?}");
    assert_eq!(tld.view.editing_text, "https://example.com");
    let committed = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(committed.commit.as_deref(), Some("https://example.com"));
}

/// 整句改字：左右键在整句的汉字之间移动，选中的候选替换光标处那一段，确认后上屏改好的整句。
#[test]
fn the_sentence_is_corrected_in_place_and_committed_whole() {
    let directory = tempfile::tempdir().unwrap();
    let dictionaries = directory.path().join("dictionaries");
    std::fs::create_dir_all(&dictionaries).unwrap();
    rusqlite::Connection::open(dictionaries.join(msime_engine::assets::MAIN_DICTIONARY))
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_1_w(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_w VALUES('wo','w','我',9000);\
             CREATE TABLE tbl_1_q(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_q VALUES('qu','q','去',9000);\
             CREATE TABLE tbl_1_b(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_b VALUES('bei','b','被',9000),('bei','b','北',5000);\
             CREATE TABLE tbl_1_j(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_j VALUES('jing','j','经',9000),('jing','j','京',3000);\
             CREATE TABLE tbl_2_b(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_2_b VALUES('bei''jing','bj','背景',30000),('bei''jing','bj','北京',20000);",
        )
        .unwrap();
    let options = real_engine_options(directory.path());
    let mut runtime = Runtime::new(msime_engine::host::Session::new(&options).unwrap(), 5).unwrap();
    runtime.focus(true).unwrap();
    type_characters(&mut runtime, "woqubeijing");
    assert_eq!(texts(&runtime.view())[0], "我去背景");
    assert!(runtime.view().conversion.is_empty());

    let entered = runtime
        .dispatch(Action::Command(Command::ConversionLeft))
        .unwrap();
    assert_eq!(entered.view.conversion, "我去背景");
    assert_eq!(
        (
            entered.view.conversion_focus_start,
            entered.view.conversion_focus_end
        ),
        (3, 4)
    );
    let json = serde_json::to_value(&entered.view).unwrap();
    assert_eq!(json["conversion"], "我去背景");
    assert_eq!(json["conversion_focus_start"], 3);

    // 换了焦点，高亮回到第一个候选。
    runtime.dispatch(Action::NextCandidate).unwrap();
    let moved = runtime
        .dispatch(Action::Command(Command::ConversionLeft))
        .unwrap();
    assert_eq!(texts(&moved.view)[..2], ["背景", "北京"]);
    assert_eq!(moved.view.candidates[0].id.index, 0);
    assert_eq!(runtime.highlighted, 0);

    // 数字 2 选「北京」：替换这一段，不上屏。
    let picked = character(&mut runtime, b'2');
    assert!(picked.commit.is_none());
    assert_eq!(picked.view.conversion, "我去北京");
    assert_eq!(picked.view.conversion_focus_start, 4);
    assert!(picked.view.candidates.is_empty());
    let committed = runtime.dispatch(Action::SelectHighlighted).unwrap();
    assert_eq!(committed.commit.as_deref(), Some("我去北京"));
    assert!(committed.view.conversion.is_empty());
    assert!(committed.view.editing_text.is_empty());

    // 回车上屏改好的汉字；它不是拼出来的英文单词，不去学英文词库（这里没有英文词库，学了就会报诊断）。
    type_characters(&mut runtime, "woqubeijing");
    runtime
        .dispatch(Action::Command(Command::ConversionLeft))
        .unwrap();
    runtime
        .dispatch(Action::Command(Command::ConversionLeft))
        .unwrap();
    character(&mut runtime, b'2');
    let entered = runtime
        .dispatch(Action::Command(Command::CommitRaw))
        .unwrap();
    assert_eq!(entered.commit.as_deref(), Some("我去北京"));
    assert_eq!(entered.diagnostic, None);
}

/// 读音 ni 有六个本地候选的真 Engine，每页 `page_size` 个候选。
fn ai_answer_runtime(root: &std::path::Path, page_size: u8) -> Runtime {
    let dictionaries = root.join("dictionaries");
    std::fs::create_dir_all(&dictionaries).unwrap();
    rusqlite::Connection::open(dictionaries.join(msime_engine::assets::MAIN_DICTIONARY))
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_1_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_n VALUES('ni','n','你',100),('ni','n','拟',90),('ni','n','妮',80),\
             ('ni','n','倪',70),('ni','n','泥',60),('ni','n','尼',50);",
        )
        .unwrap();
    let session = msime_engine::host::Session::new(&real_engine_options(root)).unwrap();
    let mut runtime = Runtime::new(session, page_size).unwrap();
    runtime.focus(true).unwrap();
    runtime
}

#[test]
fn moving_the_caret_invalidates_candidates_from_the_previous_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 5);
    rusqlite::Connection::open(
        directory
            .path()
            .join("dictionaries")
            .join(msime_engine::assets::MAIN_DICTIONARY),
    )
    .unwrap()
    .execute_batch(
        "CREATE TABLE tbl_1_h(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_1_h VALUES('hao','h','好',100);\
         CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
         INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',1000);",
    )
    .unwrap();

    for caret in [Some(2), None] {
        type_characters(&mut runtime, "nihao");
        if caret.is_none() {
            runtime.set_caret(Some(2)).unwrap();
        }
        let before = runtime.view();
        assert_eq!(
            before.candidates[0].text,
            if caret.is_some() { "你好" } else { "你" }
        );
        let old = before.candidates[0].id;

        runtime.set_caret(caret).unwrap();

        let after = runtime.view();
        assert_ne!(before.candidates[0].text, after.candidates[0].text);
        let stale = runtime.dispatch(Action::Select(old));
        assert!(
            matches!(stale, Err(RuntimeError::StaleCandidate)),
            "{stale:?}"
        );
        assert_eq!(runtime.view().editing_text, "nihao");
        let selected = runtime
            .dispatch(Action::Select(after.candidates[0].id))
            .unwrap();
        assert_eq!(
            selected.commit.as_deref(),
            Some(after.candidates[0].text.as_str())
        );
        assert_eq!(
            selected.view.editing_text,
            if caret.is_some() { "hao" } else { "" }
        );
        runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    }
}

#[test]
fn moving_the_caret_does_not_change_engine_after_generation_exhaustion() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 5);
    type_characters(&mut runtime, "ni");
    runtime.generation = u64::MAX;
    let before = runtime.engine.snapshot().unwrap();

    assert!(matches!(
        runtime.set_caret(Some(1)),
        Err(RuntimeError::IdentityExhausted)
    ));
    assert_eq!(runtime.engine.snapshot().unwrap(), before);
    assert_eq!(runtime.view().caret_position, 2);
}

fn apply_ai_answer(runtime: &mut Runtime, words: &[&str]) -> bool {
    let mut query = runtime.online_query().unwrap().unwrap();
    query.ai_assistant =
        Some(serde_json::from_value(json!({"enabled": true, "candidate_limit": 3})).unwrap());
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    runtime.apply_online_candidates(&query, &words, 1).unwrap()
}

/// #6888：真 Engine 上的 AI 回答。等回答时没有占位行，第一页全是本地候选；回答到达后第一条 AI 候选坐第一页最后一格，前面的候选一个都不挪，原来在最后一格的候选是第二页的第一个；翻页回来它还在那里，数字键按的就是画出来的那一格。
#[test]
fn the_ai_answer_takes_the_last_slot_of_the_first_page_without_moving_the_rest() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 5);
    type_characters(&mut runtime, "ni");
    let before = runtime.view();
    assert_eq!(texts(&before), ["你", "拟", "妮", "倪", "泥"]);
    assert!(before
        .candidates
        .iter()
        .all(|candidate| candidate.source != 3 && candidate.source != 12));

    assert!(apply_ai_answer(
        &mut runtime,
        &["合成一", "合成二", "合成三"]
    ));
    let after = runtime.view();
    assert_eq!(texts(&after), ["你", "拟", "妮", "倪", "合成一"]);
    assert_eq!(after.candidates[4].source, 3);
    let all: Vec<String> = runtime
        .all_candidates()
        .candidates
        .into_iter()
        .map(|candidate| candidate.text)
        .collect();
    assert_eq!(
        all,
        [
            "你",
            "拟",
            "妮",
            "倪",
            "合成一",
            "泥",
            "尼",
            "合成二",
            "合成三"
        ]
    );

    let second = runtime.dispatch(Action::NextPage).unwrap().view;
    assert_eq!(texts(&second)[0], "泥");
    let first = runtime.dispatch(Action::PreviousPage).unwrap().view;
    assert_eq!(texts(&first), texts(&after));
    let digit = character(&mut runtime, b'5');
    assert_eq!(digit.commit.as_deref(), Some("合成一"));
}

/// 第一页排不满时 AI 候选紧跟在本地候选之后，什么都不挪；回答的词都已在列表里时列表不变。
#[test]
fn a_short_first_page_takes_the_ai_answer_after_its_last_row() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 9);
    type_characters(&mut runtime, "ni");
    let before = texts(&runtime.view());
    assert_eq!(before, ["你", "拟", "妮", "倪", "泥", "尼"]);

    assert!(apply_ai_answer(&mut runtime, &["妮"]));
    assert_eq!(texts(&runtime.view()), before);

    assert!(apply_ai_answer(&mut runtime, &["合成一", "合成二"]));
    assert_eq!(
        texts(&runtime.view()),
        ["你", "拟", "妮", "倪", "泥", "尼", "合成一", "合成二"]
    );
}

mod caret_edit;
mod facts;

/// View 报告宿主设下的标点样式，缺省是弯引号、实心括号；换样式不动组字和候选。
#[test]
fn view_reports_the_applied_punctuation_marks() {
    let mut runtime = runtime();
    let json = serde_json::to_value(runtime.view()).unwrap();
    assert_eq!(
        json["punctuation_marks"],
        serde_json::json!({
            "quotes": "curly",
            "brackets": "solid",
            "enumeration_comma_backslash": true,
            "enumeration_comma_slash": false,
        })
    );
    let before = runtime.view().generation;
    runtime.set_punctuation_marks(PunctuationMarks {
        quotes: QuoteMarks::Corner,
        brackets: msime_client_core::preferences::BracketStyle::Hollow,
        ..PunctuationMarks::default()
    });
    let view = runtime.view();
    assert_eq!(view.generation, before);
    assert_eq!(
        serde_json::to_value(&view).unwrap()["punctuation_marks"],
        serde_json::json!({
            "quotes": "corner",
            "brackets": "hollow",
            "enumeration_comma_backslash": true,
            "enumeration_comma_slash": false,
        })
    );
}

#[allow(unsafe_code)]
pub(crate) mod allocations;
mod online_gloss_retention;

mod online_slot_allocations;

mod parallel_order_allocations;

mod online_slot_order_equivalence;

mod translation_reply_allocations;

#[test]
fn cloud_switch_invalidates_candidate_ids_in_both_directions() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 5);
    type_characters(&mut runtime, "ni");
    for enabled in [true, false] {
        let before = runtime.view();
        let old = before.candidates[2].id;
        runtime.set_cloud_candidates_enabled(enabled).unwrap();
        let after = runtime.view();
        assert_eq!(after.candidates.iter().any(|row| row.text == "☁"), enabled);
        assert!(after.generation > before.generation);
        assert!(matches!(
            runtime.dispatch(Action::Select(old)),
            Err(RuntimeError::StaleCandidate)
        ));
        assert_eq!(runtime.view().editing_text, "ni");
    }
    let after = runtime.view();
    let selected = runtime
        .dispatch(Action::Select(after.candidates[0].id))
        .unwrap();
    assert_eq!(
        selected.commit.as_deref(),
        Some(after.candidates[0].text.as_str())
    );
}

#[test]
fn cloud_switch_repeated_value_preserves_candidate_ids() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 5);
    type_characters(&mut runtime, "ni");
    for enabled in [true, false] {
        runtime.set_cloud_candidates_enabled(enabled).unwrap();
        let before = runtime.view();
        runtime.set_cloud_candidates_enabled(enabled).unwrap();
        assert_eq!(
            serde_json::to_value(runtime.view()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
    }
}

#[test]
fn cloud_switch_does_not_change_engine_after_generation_exhaustion() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = ai_answer_runtime(directory.path(), 5);
    type_characters(&mut runtime, "ni");
    runtime.generation = u64::MAX;
    let before = runtime.engine.snapshot().unwrap();
    assert!(matches!(
        runtime.set_cloud_candidates_enabled(true),
        Err(RuntimeError::IdentityExhausted)
    ));
    assert_eq!(runtime.engine.snapshot().unwrap(), before);
    runtime.set_cloud_candidates_enabled(false).unwrap();
    assert_eq!(runtime.engine.snapshot().unwrap(), before);
}
#[test]
fn contacts_stay_first_and_adjacent_with_online_rows() {
    for page_size in [1, 2, 9] {
        let mut runtime = runtime();
        runtime.engine.words = vec![
            "合成甲".into(),
            "合成甲 202-555-0101".into(),
            "字典词".into(),
            "云词".into(),
            "智能词".into(),
            "synthetic".into(),
        ];
        runtime.engine.sources = vec![16, 16, 0, 2, 3, 4];
        runtime.engine.codes = vec!["a".into(); 6];
        runtime.set_page_size(page_size).unwrap();
        runtime.focus(true).unwrap();
        type_key(&mut runtime);
        let rows = runtime.all_candidates().candidates;
        assert_eq!(rows[0].text, "合成甲");
        assert_eq!(rows[1].text, "合成甲 202-555-0101", "page size {page_size}");
        let selected = runtime
            .dispatch(Action::SelectAnyCandidate(rows[1].id))
            .unwrap();
        assert_eq!(selected.commit.as_deref(), Some("合成甲 202-555-0101"));
    }
}

#[test]
fn english_online_runtime_keeps_local_order_places_ai_and_commits_the_answer() {
    for dedicated in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let options = real_engine_options(directory.path());
        let english = std::path::Path::new(&options.dictionaries).join("msime-english.db");
        msime_engine::ensure_english_schema(&english).unwrap();
        let connection = rusqlite::Connection::open(english).unwrap();
        for (index, word) in ["Hello", "Help", "Helmet", "Hero", "Heron", "Hedge", "Herd"]
            .iter()
            .enumerate()
        {
            connection
                .execute(
                    "INSERT INTO english_words VALUES(?1,?2,?3)",
                    rusqlite::params![word.to_lowercase(), word, 100 - index as i64],
                )
                .unwrap();
        }
        drop(connection);
        let session = msime_engine::host::Session::new(&options).unwrap();
        let mut runtime = Runtime::new(session, 5).unwrap();
        runtime.focus(true).unwrap();
        if dedicated {
            runtime.set_dedicated_english(true).unwrap();
        } else {
            runtime
                .dispatch(Action::Character {
                    value: b'Y',
                    shift: true,
                })
                .unwrap();
        }
        type_characters(&mut runtime, "he");
        assert!(runtime.online_query().unwrap().is_none());
        let local = texts(&runtime.view());
        assert!(local.len() >= 5, "dedicated={dedicated}: {local:?}");
        runtime.seed_context("Synthetic preceding sentence.");
        runtime.set_english_online_candidates_enabled(true).unwrap();
        let mut query = runtime.online_query().unwrap().unwrap();
        assert_eq!(query.mode, msime_engine::OnlineInputMode::English);
        assert_eq!(query.ai_context, "Synthetic preceding sentence.");
        query.ai_assistant =
            Some(serde_json::from_value(json!({"enabled": true, "candidate_limit": 2})).unwrap());
        assert!(runtime
            .apply_online_candidates(&query, &["Helpful".into(), "Health".into()], 1)
            .unwrap());
        let page = texts(&runtime.view());
        assert_eq!(&page[..4], &local[..4]);
        assert_eq!(page[4], "Helpful");
        let chosen = runtime.view().candidates[4].id;
        let transition = runtime.dispatch(Action::Select(chosen)).unwrap();
        assert_eq!(transition.commit.as_deref(), Some("Helpful"));
        assert!(!runtime.apply_online_candidate(&query, "Health", 1).unwrap());
    }
}

#[test]
fn english_online_runtime_revocation_and_blur_reject_pending_answers() {
    let directory = tempfile::tempdir().unwrap();
    let session = msime_engine::host::Session::new(&real_engine_options(directory.path())).unwrap();
    let mut runtime = Runtime::new(session, 5).unwrap();
    runtime.focus(true).unwrap();
    runtime.set_dedicated_english(true).unwrap();
    runtime.set_english_online_candidates_enabled(true).unwrap();
    type_characters(&mut runtime, "accessability");
    let query = runtime.online_query().unwrap().unwrap();
    assert!(runtime
        .apply_online_candidate(&query, "accessibility", 1)
        .unwrap());
    runtime
        .set_english_online_candidates_enabled(false)
        .unwrap();
    assert!(runtime.online_query().unwrap().is_none());
    assert!(!texts(&runtime.view()).contains(&"accessibility".into()));
    runtime.set_english_online_candidates_enabled(true).unwrap();
    assert!(!runtime
        .apply_online_candidate(&query, "accessibility", 1)
        .unwrap());
    let query = runtime.online_query().unwrap().unwrap();
    runtime.focus(false).unwrap();
    assert!(!runtime
        .apply_online_candidate(&query, "accessibility", 1)
        .unwrap());
}

#[test]
fn literal_symbol_survives_runtime_punctuation_without_early_commit() {
    for scheme in [0, 1] {
        for symbol in *b"#$%&" {
            for action in [
                Action::Character {
                    value: symbol,
                    shift: false,
                },
                Action::Punctuation(symbol),
                Action::PunctuationAscii(symbol),
            ] {
                let directory = tempfile::tempdir().unwrap();
                let mut options = real_engine_options(directory.path());
                options.scheme = scheme;
                let session = msime_engine::host::Session::new(&options).unwrap();
                let mut runtime = Runtime::new(session, 5).unwrap();
                runtime.focus(true).unwrap();
                character(&mut runtime, b'C');
                let added = runtime.dispatch(action).unwrap();
                assert!(added.handled && added.commit.is_none(), "{added:?}");
                let expected = format!("C{}", char::from(symbol));
                assert_eq!(added.view.editing_text, expected);
                assert_eq!(added.view.candidates.len(), 1);
                assert_eq!(added.view.candidates[0].text, expected);
                let selected = runtime.dispatch(Action::SelectHighlighted).unwrap();
                assert_eq!(selected.commit.as_deref(), Some(expected.as_str()));
                assert!(selected.view.editing_text.is_empty());
            }
        }
    }
}

#[test]
fn projected_emoji_refresh_and_focus_loss_stay_aligned() {
    let directory = tempfile::tempdir().unwrap();
    let dictionaries = directory.path().join("dictionaries");
    std::fs::create_dir_all(&dictionaries).unwrap();
    rusqlite::Connection::open(dictionaries.join(msime_engine::assets::MAIN_DICTIONARY))
        .unwrap()
        .execute_batch(
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
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','除法',50000);\
             CREATE TABLE tbl_3_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_3_c VALUES('chu''fa''dian','cfd','出发点',100000);\
             CREATE TABLE tbl_4_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_4_c VALUES('chu''fa''jue''ding','cfjd','处罚决定',90000);",
        )
        .unwrap();
    let options = real_engine_options(directory.path());
    let session = msime_engine::host::Session::new(&options).unwrap();
    let mut runtime = Runtime::new(session, 6).unwrap();
    runtime.focus(true).unwrap();
    runtime.set_inline_emoji_projection(true, None);

    // 1. `chuf` 触发「出发」投影 `🚀` 在第 3 位，再键入 `a` 变为 `chufa` 时，
    //    `refresh` 必须清空旧 `projected_emojis`，绝不得把新快照下标 2 的「触发」误删。
    type_characters(&mut runtime, "chuf");
    assert_eq!(runtime.view().candidates[2].text, "🚀");
    character(&mut runtime, b'a');
    let all_chufa: Vec<String> = runtime
        .all_candidates()
        .candidates
        .into_iter()
        .map(|row| row.text)
        .collect();
    assert!(
        all_chufa.iter().any(|text| text == "触发"),
        "键入 chuf -> chufa 后不能丢掉「触发」: {all_chufa:?}"
    );
    assert!(
        all_chufa.iter().any(|text| text == "除法"),
        "键入 chuf -> chufa 后不能丢掉「除法」: {all_chufa:?}"
    );

    // 2. 从已投影 Emoji 的 `chuf` 退格到 `chu`：不仅不能误删「处」，选第 3 项「楚」必须精确上屏「楚」。
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    type_characters(&mut runtime, "chuf");
    assert_eq!(runtime.view().candidates[2].text, "🚀");
    let chu_view = runtime
        .dispatch(Action::Command(Command::Backspace))
        .unwrap()
        .view;
    let chu_texts: Vec<&str> = chu_view
        .candidates
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert_eq!(&chu_texts[..4], ["出", "处", "楚", "除"]);
    let pick_third = runtime
        .dispatch(Action::Select(chu_view.candidates[2].id))
        .unwrap();
    assert_eq!(pick_third.commit.as_deref(), Some("楚"));

    // 3. 高亮落在投影 Emoji 席位时，失焦提交与向左整句改字均应回退到真实引擎首选，不得把 Emoji 当失焦词上屏或传入 usize::MAX。
    type_characters(&mut runtime, "chufa");
    runtime.dispatch(Action::NextCandidate).unwrap();
    runtime.dispatch(Action::NextCandidate).unwrap();
    assert_eq!(runtime.view().candidates[2].text, "🚀");
    assert!(runtime.view().candidates[2].highlighted);
    let conv = runtime
        .dispatch(Action::Command(Command::ConversionLeft))
        .unwrap();
    assert!(conv.view.candidates.iter().all(|c| !c.text.is_empty()));
    // 第一次 Cancel 退出整句改字，第二次清空未上屏拼音。
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    assert!(runtime.view().editing_text.is_empty());
    type_characters(&mut runtime, "chufa");
    runtime.dispatch(Action::NextCandidate).unwrap();
    runtime.dispatch(Action::NextCandidate).unwrap();
    assert!(runtime.view().candidates[2].highlighted);
    let lost = runtime.dispatch(Action::FinishOnFocusLoss).unwrap();
    assert_eq!(lost.commit.as_deref(), Some("出发"));

    // 4. 组字中途把内联 Emoji 从关切到开时，应立即生效投影。
    runtime.focus(true).unwrap();
    runtime.set_inline_emoji_projection(false, None);
    type_characters(&mut runtime, "chufa");
    assert!(!runtime.view().candidates.iter().any(|c| c.text == "🚀"));
    runtime.set_inline_emoji_projection(true, None);
    assert_eq!(runtime.view().candidates[2].text, "🚀");
}

#[test]
fn real_engine_pin_to_first_block_and_locked_prefix_backspace_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let dictionaries = directory.path().join("dictionaries");
    std::fs::create_dir_all(&dictionaries).unwrap();
    rusqlite::Connection::open(dictionaries.join(msime_engine::assets::MAIN_DICTIONARY))
        .unwrap()
        .execute_batch(
            "CREATE TABLE tbl_1_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_c VALUES('chu','c','出',900000);\
             INSERT INTO tbl_1_c VALUES('chu','c','处',800000);\
             CREATE TABLE tbl_1_l(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_1_l VALUES('lou','l','楼',900000);\
             CREATE TABLE tbl_2_c(key TEXT,jp TEXT,value TEXT,weight INTEGER);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','出发',950000);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','处罚',500000);\
             INSERT INTO tbl_2_c VALUES('chu''fa','cf','触犯',400000);",
        )
        .unwrap();
    let mut options = real_engine_options(directory.path());
    options.learning = true;
    let session = msime_engine::host::Session::new(&options).unwrap();
    let mut runtime = Runtime::new(session, 6).unwrap();
    runtime.focus(true).unwrap();
    runtime.set_phrase_preedit(true);

    // 1. `PinCandidate` 只置顶到第 1 位并锁定首位，`RemoveCandidate` 屏蔽删除候选。
    type_characters(&mut runtime, "chufa");
    let punish_id = runtime
        .view()
        .candidates
        .iter()
        .find(|c| c.text == "处罚")
        .unwrap()
        .id;
    let pinned_view = runtime
        .dispatch(Action::PinCandidate(punish_id))
        .unwrap()
        .view;
    assert_eq!(pinned_view.candidates[0].text, "处罚");
    assert_eq!(pinned_view.candidates[0].fixed_position, Some(1));

    // 多字词可被屏蔽删除（单字受词典保护不可删除），且删除后重新输入不再出现。
    let chufan_id = pinned_view
        .candidates
        .iter()
        .find(|c| c.text == "触犯")
        .unwrap()
        .id;
    let after_block = runtime
        .dispatch(Action::RemoveCandidate(chufan_id))
        .unwrap()
        .view;
    assert!(!after_block.candidates.iter().any(|c| c.text == "触犯"));
    assert_eq!(after_block.candidates[0].text, "处罚");
    assert_eq!(after_block.candidates[0].fixed_position, Some(1));

    // 2. 分段选词暂存 `locked_prefix` 并进入 `effective_left_context`，退格拆回拼音后可改选并整句上屏学习。
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    type_characters(&mut runtime, "chufalou");
    let punish_prefix_id = runtime
        .view()
        .candidates
        .iter()
        .find(|c| c.text == "处罚")
        .unwrap()
        .id;
    let held = runtime.dispatch(Action::Select(punish_prefix_id)).unwrap();
    assert_eq!(held.commit, None);
    assert_eq!(held.view.phrase_prefix, "处罚");
    assert_eq!(held.view.editing_text, "lou");
    assert_eq!(runtime.cursor_snapshot().effective_left_context(), "处罚");

    // 删空剩余拼音并再按一次退格，拆回已选前缀 `处罚` 恢复 `chufa`。
    for _ in 0..3 {
        runtime
            .dispatch(Action::Command(Command::Backspace))
            .unwrap();
    }
    let rolled_back = runtime.view();
    assert!(rolled_back.phrase_prefix.is_empty());
    assert_eq!(rolled_back.editing_text, "chufa");
    assert!(runtime
        .cursor_snapshot()
        .effective_left_context()
        .is_empty());

    // 改选第 2 项「出发」并补输 `lou` 完成整句上屏（触发整句学习）。
    let depart_id = rolled_back
        .candidates
        .iter()
        .find(|c| c.text == "出发")
        .unwrap()
        .id;
    runtime.dispatch(Action::Select(depart_id)).unwrap();
    // 全串被消耗后会直接上屏；再验证一次两段连选整句上屏学词。
    runtime.dispatch(Action::Command(Command::Cancel)).unwrap();
    type_characters(&mut runtime, "chufalou");
    let depart2_id = runtime
        .view()
        .candidates
        .iter()
        .find(|c| c.text == "出发")
        .unwrap()
        .id;
    let step1 = runtime.dispatch(Action::Select(depart2_id)).unwrap();
    assert_eq!(step1.view.phrase_prefix, "出发");
    assert_eq!(step1.view.editing_text, "lou");
    let lou_id = step1.view.candidates[0].id;
    let final_commit = runtime.dispatch(Action::Select(lou_id)).unwrap();
    assert_eq!(final_commit.commit.as_deref(), Some("出发楼"));
    assert!(final_commit.view.phrase_prefix.is_empty());

    // 新建输入框再打 `chufalou`，刚才整句连选学会的「出发楼」应已入库。
    type_characters(&mut runtime, "chufalou");
    assert!(runtime.view().candidates.iter().any(|c| c.text == "出发楼"));
}

#[test]
fn real_runtime_commit_candidate_commits_slot0_and_learns_injected_beam_phrase() {
    let directory = tempfile::tempdir().unwrap();
    let mut options = real_engine_options(directory.path());
    options.learning = true;
    options.decoupled_personal_learning = true;
    let mut sql = String::from(
        "CREATE TABLE tbl_1_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_a VALUES('ai','a','爱',1516399);\
         CREATE TABLE tbl_1_l(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_1_l VALUES('li','l','里',7329328),('li','l','梨',767900);\
         CREATE TABLE tbl_2_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);\
         INSERT INTO tbl_2_a VALUES('ai''li','al','艾里',2),('ai''li','al','艾丽',1);\
         CREATE TABLE tbl_3_a(key TEXT, jp TEXT, value TEXT, weight INTEGER);",
    );
    for idx in 0..25 {
        let third = (b'a' + (idx as u8 % 26)) as char;
        sql.push_str(&format!(
            "INSERT INTO tbl_3_a VALUES('ai''li''{third}','al{third}','艾词{idx}',{});",
            5000 - idx * 50
        ));
    }
    rusqlite::Connection::open(
        std::path::Path::new(&options.dictionaries).join(msime_engine::assets::MAIN_DICTIONARY),
    )
    .unwrap()
    .execute_batch(&sql)
    .unwrap();

    let session = msime_engine::host::Session::new(&options).unwrap();
    let mut runtime = Runtime::new(session, 6).unwrap();
    runtime.focus(true).unwrap();
    runtime.set_neural_v2_enabled(true);
    runtime.set_surrounding_context("刚才");

    // 1. 输入 `aili`，由受限束搜索将“爱梨”注入并排至呈现第 0 位。
    type_characters(&mut runtime, "aili");
    let req = runtime.take_neural_request().expect("应生成 NeuralV2 请求");
    let resp = msime_engine::neural_v2::NeuralDecodeResponse {
        ok: true,
        session: req.session,
        generation: req.generation,
        candidate_scores: vec![msime_engine::neural_v2::NeuralScoredCandidate {
            text: "艾里".into(),
            neural_log_prob: -18.0,
        }],
        beam_candidates: vec![msime_engine::neural_v2::NeuralBeamCandidate {
            text: "爱梨".into(),
            path_index: 0,
            neural_log_prob: -1.0,
            token_count: 2,
        }],
    };
    let applied = runtime.apply_neural_response(&resp, false);
    assert!(applied.applied);
    assert_eq!(runtime.view().candidates[0].text, "爱梨");

    // 2. 通过 Command::CommitCandidate 提交首选：必须上屏呈现第 0 位的“爱梨”而非原 EngineRow(0)“艾里”。
    let committed = runtime
        .dispatch(Action::Command(Command::CommitCandidate))
        .unwrap();
    assert_eq!(committed.commit.as_deref(), Some("爱梨"));

    // 3. 紧接着不经过任何二次神经响应重打 `aili`，“爱梨”已入个人词库并直接处于第 0 位。
    type_characters(&mut runtime, "aili");
    assert_eq!(runtime.view().candidates[0].text, "爱梨");
}
