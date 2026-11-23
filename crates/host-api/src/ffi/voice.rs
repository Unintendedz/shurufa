//! Voice provider wire formats and the streaming recognition exchange.
//!
//! Part of the C ABI; see the parent module for what these shims guarantee.

use crate::*;
use msime_client_core::{is_bounded_chars_without_nul, is_bounded_text};

/// 设置页的语音接口地址提示；与运行时共用 URL 校验，不读写偏好或联网。
///
/// # Safety
/// `endpoint` 必须指向本次调用期间可读的 `length` 个字节；空引用或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_endpoint_issue(
    endpoint: *const u8,
    length: usize,
    websocket: bool,
) -> *mut c_char {
    response(|| unsafe {
        with_bounded_bytes(
            endpoint,
            length,
            65_536,
            "invalid voice endpoint",
            |bytes| {
                let endpoint =
                    std::str::from_utf8(bytes).map_err(|_| "invalid voice endpoint".to_owned())?;
                let issue = msime_client_providers::voice::provider_settings::endpoint_issue(
                    endpoint, websocket,
                );
                Ok(json!({"accepted": issue.is_none(), "issue": issue}))
            },
        )
    })
}

/// 设置页保存语音凭据前的共享预检；不回显凭据。
///
/// # Safety
/// `provider` 与 `value` 必须分别指向本次调用期间可读的 `length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_credential_issue(
    provider: *const u8,
    provider_length: usize,
    value: *const u8,
    value_length: usize,
) -> *mut c_char {
    response(|| unsafe {
        with_bounded_bytes(
            provider,
            provider_length,
            256,
            "invalid voice credential",
            |bytes| {
                let provider = std::str::from_utf8(bytes)
                    .map_err(|_| "invalid voice credential".to_owned())?;
                with_bounded_bytes(
                    value,
                    value_length,
                    16 * 1024,
                    "invalid voice credential",
                    |bytes| {
                        let value = std::str::from_utf8(bytes)
                            .map_err(|_| "invalid voice credential".to_owned())?;
                        let accepted =
                            msime_client_providers::voice::provider::valid_voice_credential(
                                provider, value,
                            );
                        Ok(json!({"accepted": accepted}))
                    },
                )
            },
        )
    })
}

/// 生成 Android 流式 WebSocket 使用的规范化传输描述；不返回原始地址或凭据。
///
/// # Safety
/// `endpoint` 必须指向本次调用期间可读的 `length` 个字节；空引用或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_websocket_endpoint(
    endpoint: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| unsafe {
        with_bounded_bytes(
            endpoint,
            length,
            65_536,
            "invalid voice websocket endpoint",
            |bytes| {
                let endpoint = std::str::from_utf8(bytes)
                    .map_err(|_| "invalid voice websocket endpoint".to_owned())?;
                let descriptor =
                    msime_client_providers::voice::provider::websocket_endpoint_descriptor(
                        endpoint,
                    )
                    .ok_or_else(|| "invalid voice websocket endpoint".to_owned())?;
                serde_json::to_value(descriptor)
                    .map_err(|_| "invalid voice websocket endpoint".to_owned())
            },
        )
    })
}

/// 语音设置的公开服务目录；纯函数，不访问偏好或设备。返回值用标准 JSON 信封封装。
#[no_mangle]
pub extern "C" fn msime_client_voice_provider_catalog() -> *mut c_char {
    response(|| {
        use msime_client_providers::voice::provider_catalog::{
            ASR_PROVIDERS, DOUBAO_STREAM_ENDPOINTS, POLISH_PROVIDERS,
        };
        Ok(json!({
            "asr": ASR_PROVIDERS, "polish": POLISH_PROVIDERS,
            "doubao_stream_endpoints": DOUBAO_STREAM_ENDPOINTS
        }))
    })
}

/// 设置页识别服务行的状态文案；纯计算，不读取偏好或设备状态。
///
/// # Safety
/// `provider` 必须指向本次调用期间可读的 UTF-8 字节；空服务用 `system` 或空串表示。
#[no_mangle]
pub unsafe extern "C" fn msime_client_mobile_voice_status(
    provider: *const u8,
    provider_length: usize,
    resolved: bool,
    platform_available: bool,
    model_ready: bool,
    runtime_ready: bool,
) -> *mut c_char {
    response(|| unsafe {
        with_bounded_bytes(
            provider,
            provider_length,
            256,
            "invalid voice status provider",
            |bytes| {
                let provider = std::str::from_utf8(bytes)
                    .map_err(|_| "invalid voice status provider".to_owned())?;
                let status = if provider == "local" {
                    msime_client_providers::voice::provider::mobile_voice_local_status(
                        model_ready,
                        runtime_ready,
                    )
                } else {
                    msime_client_providers::voice::provider::mobile_voice_provider_status(
                        provider,
                        resolved,
                        platform_available,
                    )
                };
                Ok(json!({"status": status}))
            },
        )
    })
}

/// 设置页润色行的状态文案；纯计算，不读取偏好或设备状态。
#[no_mangle]
pub extern "C" fn msime_client_mobile_voice_polish_status(
    enabled: bool,
    configured: bool,
    has_token: bool,
) -> *mut c_char {
    response(|| {
        let status = msime_client_providers::voice::provider::mobile_voice_polish_status(
            enabled, configured, has_token,
        );
        Ok(json!({"status": status}))
    })
}

/// 设置页语音服务分类和豆包鉴权模式；纯计算，不读取偏好或设备状态。
///
/// # Safety
/// `provider` 与 `auth_mode` 必须分别指向本次调用期间可读的 UTF-8 字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_mobile_voice_policy(
    provider: *const u8,
    provider_length: usize,
    auth_mode: *const u8,
    auth_mode_length: usize,
) -> *mut c_char {
    response(|| unsafe {
        with_bounded_bytes(
            provider,
            provider_length,
            256,
            "invalid voice provider policy",
            |provider_bytes| {
                with_bounded_bytes(
                    auth_mode,
                    auth_mode_length,
                    256,
                    "invalid voice provider policy",
                    |auth_mode_bytes| {
                        let provider = std::str::from_utf8(provider_bytes)
                            .map_err(|_| "invalid voice provider policy".to_owned())?;
                        let auth_mode = std::str::from_utf8(auth_mode_bytes)
                            .map_err(|_| "invalid voice provider policy".to_owned())?;
                        let policy =
                            msime_client_providers::voice::provider::mobile_voice_provider_policy(
                                provider, auth_mode,
                            );
                        Ok(json!({
                            "network": policy.network,
                            "legacy_auth": policy.legacy_auth,
                        }))
                    },
                )
            },
        )
    })
}

#[derive(serde::Deserialize)]
struct VoiceProviderUpdateRequest {
    service: msime_client_providers::voice::provider_settings::ProviderService,
    provider: String,
    current: msime_client_providers::voice::provider_settings::ProviderFields,
    #[serde(default)]
    field_policy: msime_client_providers::voice::provider_settings::ProviderFieldPolicy,
}

/// 计算语音设置的换服务补丁；请求与返回均含凭据，不能记录到日志。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_update(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let query: VoiceProviderUpdateRequest = local_voice_request(request, length, 1 << 20)?;
        Ok(json!(
            msime_client_providers::voice::provider_settings::provider_update(
                query.service,
                &query.provider,
                &query.current,
                query.field_policy,
            )
        ))
    })
}

/// Decode one Doubao v1 response frame for Apple hosts. The returned payload
/// is UTF-8 JSON text; no frame bytes or credentials are retained.
///
/// # Safety
/// `frame` must reference a readable buffer for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn msime_client_doubao_decode_frame(
    frame: *const u8,
    frame_length: usize,
) -> *mut c_char {
    response(|| {
        if frame.is_null() || frame_length == 0 || frame_length > 1_048_576 {
            return Err("invalid Doubao frame buffer".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(frame, frame_length) };
        if let Some((last, _sequence, payload)) = decode_json_frame(bytes) {
            let text = String::from_utf8(payload).map_err(|_| "Doubao payload is not UTF-8")?;
            return Ok(json!({ "last": last, "payload": text }));
        }
        if let Some(code) = decode_error_code(bytes) {
            return Ok(json!({ "error_code": code }));
        }
        Err("invalid Doubao response frame".into())
    })
}

unsafe fn write_doubao_frame(
    frame: Vec<u8>,
    output: *mut u8,
    output_capacity: usize,
    output_length: *mut usize,
) -> bool {
    // A null, zero-capacity output is the sizing probe used by mobile bindings. Report the
    // required length before returning false so the caller can allocate exactly one frame. A null
    // output with nonzero capacity is still an invalid destination and must not be accepted.
    if output_length.is_null() || (output.is_null() && output_capacity != 0) {
        return false;
    }
    *output_length = frame.len();
    if output_capacity < frame.len() {
        return false;
    }
    std::ptr::copy_nonoverlapping(frame.as_ptr(), output, frame.len());
    true
}

/// Build a Doubao start request into caller-owned storage.
///
/// # Safety
/// `boosting_table_id` must point to `boosting_table_id_length` readable bytes when the length is
/// nonzero. `output` must point to `output_capacity` writable bytes and `output_length` must point
/// to a writable `usize`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_doubao_start_frame(
    enable_itn: bool,
    enable_punc: bool,
    enable_ddc: bool,
    boosting_table_id: *const u8,
    boosting_table_id_length: usize,
    output: *mut u8,
    output_capacity: usize,
    output_length: *mut usize,
) -> bool {
    if boosting_table_id_length > 4096
        || (boosting_table_id.is_null() && boosting_table_id_length != 0)
    {
        return false;
    }
    let boosting = if boosting_table_id_length == 0 {
        ""
    } else {
        let bytes =
            unsafe { std::slice::from_raw_parts(boosting_table_id, boosting_table_id_length) };
        match std::str::from_utf8(bytes) {
            Ok(value) => value,
            Err(_) => return false,
        }
    };
    unsafe {
        write_doubao_frame(
            start_frame(enable_itn, enable_punc, enable_ddc, boosting),
            output,
            output_capacity,
            output_length,
        )
    }
}

/// Build a Doubao PCM or final audio frame into caller-owned storage.
///
/// # Safety
/// `pcm` must point to `pcm_length` readable bytes when the length is nonzero. `output` must point
/// to `output_capacity` writable bytes and `output_length` must point to a writable `usize`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_doubao_audio_frame(
    sequence: i32,
    pcm: *const u8,
    pcm_length: usize,
    final_chunk: bool,
    output: *mut u8,
    output_capacity: usize,
    output_length: *mut usize,
) -> bool {
    // `i32::abs()` panics for its sole unrepresentable input. Reject it at
    // the C boundary so hostile sequence values cannot abort the host process.
    if sequence == i32::MIN || pcm_length > 1_048_576 || (pcm.is_null() && pcm_length != 0) {
        return false;
    }
    let bytes = if pcm_length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pcm, pcm_length) }
    };
    unsafe {
        write_doubao_frame(
            audio_frame(sequence, bytes, final_chunk),
            output,
            output_capacity,
            output_length,
        )
    }
}

/// Run one bounded voice capture/ASR request through a user-owned Unix socket.
/// The socket service owns microphone access, credentials and network policy.
/// The query is a bounded JSON object containing `language`, `generation`,
/// and optional non-sensitive voice behavior `options`.
///
/// # Safety
/// All pointers must reference readable buffers of the stated lengths for
/// the duration of this call; the buffers are not retained.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_request(
    query: *const u8,
    query_length: usize,
    socket_path: *const u8,
    socket_length: usize,
) -> *mut c_char {
    response(|| {
        if query.is_null() || socket_path.is_null() || query_length > 16_384 || socket_length > 4096
        {
            return Err("invalid voice provider buffer".into());
        }
        #[derive(Deserialize)]
        struct VoiceQuery {
            language: String,
            generation: u64,
            #[serde(default)]
            options: Value,
        }
        let query = serde_json::from_slice::<VoiceQuery>(unsafe {
            std::slice::from_raw_parts(query, query_length)
        })
        .map_err(|_| "invalid voice query document")?;
        let path = super::parse_absolute_socket_path(unsafe {
            std::slice::from_raw_parts(socket_path, socket_length)
        })?;
        Ok(UnixSocketProvider::new(path)
            .voice_with_options(&query.language, query.generation, &query.options)
            .map(|text| json!({"text": text}))
            .unwrap_or(Value::Null))
    })
}

/// Stream bounded interim/final voice provider updates from a user-owned
/// Unix socket. The callback is invoked synchronously on the calling thread.
///
/// # Safety
/// Buffers must remain readable for the duration of this call. The callback
/// must remain valid and must copy the text before returning.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_stream(
    query: *const u8,
    query_length: usize,
    socket_path: *const u8,
    socket_length: usize,
    callback: Option<unsafe extern "C" fn(*const u8, usize, bool, *mut c_void)>,
    context: *mut c_void,
) -> *mut c_char {
    unsafe {
        msime_client_voice_provider_stream_events(
            query,
            query_length,
            socket_path,
            socket_length,
            callback,
            None,
            context,
        )
    }
}

/// Stream voice text and optional phase notifications (0 recording, 1 recognizing, 2 polishing).
///
/// # Safety
/// Buffers and callbacks must remain valid for this synchronous call. Callbacks must not unwind.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_stream_events(
    query: *const u8,
    query_length: usize,
    socket_path: *const u8,
    socket_length: usize,
    callback: Option<unsafe extern "C" fn(*const u8, usize, bool, *mut c_void)>,
    status_callback: Option<unsafe extern "C" fn(u8, *mut c_void)>,
    context: *mut c_void,
) -> *mut c_char {
    unsafe {
        msime_client_voice_provider_stream_feedback(
            query,
            query_length,
            socket_path,
            socket_length,
            callback,
            status_callback,
            None,
            context,
        )
    }
}

/// Stream voice text, phases and optional normalized microphone levels.
///
/// # Safety
/// Buffers and callbacks must remain valid for this synchronous call. Callbacks must not unwind.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_stream_feedback(
    query: *const u8,
    query_length: usize,
    socket_path: *const u8,
    socket_length: usize,
    callback: Option<unsafe extern "C" fn(*const u8, usize, bool, *mut c_void)>,
    status_callback: Option<unsafe extern "C" fn(u8, *mut c_void)>,
    level_callback: Option<unsafe extern "C" fn(f32, *mut c_void)>,
    context: *mut c_void,
) -> *mut c_char {
    unsafe {
        msime_client_voice_provider_stream_feedback_cancelled(
            query,
            query_length,
            socket_path,
            socket_length,
            callback,
            status_callback,
            level_callback,
            None,
            context,
        )
    }
}

/// Stream voice text, phases and levels while polling a host-owned cancellation callback.
///
/// # Safety
/// Buffers and callbacks must remain valid for the duration of this synchronous call. Callbacks
/// must not unwind; the cancellation callback must be safe to call from the stream worker.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_stream_feedback_cancelled(
    query: *const u8,
    query_length: usize,
    socket_path: *const u8,
    socket_length: usize,
    callback: Option<unsafe extern "C" fn(*const u8, usize, bool, *mut c_void)>,
    status_callback: Option<unsafe extern "C" fn(u8, *mut c_void)>,
    level_callback: Option<unsafe extern "C" fn(f32, *mut c_void)>,
    cancellation_callback: Option<unsafe extern "C" fn(*mut c_void) -> bool>,
    context: *mut c_void,
) -> *mut c_char {
    response(|| {
        if query.is_null() || socket_path.is_null() || query_length > 16_384 || socket_length > 4096
        {
            return Err("invalid voice provider buffer".into());
        }
        #[derive(Deserialize)]
        struct VoiceQuery {
            language: String,
            generation: u64,
            #[serde(default)]
            options: Value,
        }
        let query = serde_json::from_slice::<VoiceQuery>(unsafe {
            std::slice::from_raw_parts(query, query_length)
        })
        .map_err(|_| "invalid voice query document")?;
        let path = super::parse_absolute_socket_path(unsafe {
            std::slice::from_raw_parts(socket_path, socket_length)
        })?;
        let mut update = |text: &str, final_result: bool| {
            if let Some(callback) = callback {
                unsafe {
                    callback(text.as_ptr(), text.len(), final_result, context);
                }
            }
        };
        let mut status = |phase: &str| {
            if let Some(callback) = status_callback {
                let value = match phase {
                    "recording" => 0,
                    "recognizing" => 1,
                    "polishing" => 2,
                    _ => return,
                };
                unsafe {
                    callback(value, context);
                }
            }
        };
        let mut level = |value: f32| {
            if let Some(callback) = level_callback {
                unsafe {
                    callback(value, context);
                }
            }
        };
        let mut cancellation =
            || cancellation_callback.is_some_and(|callback| unsafe { callback(context) });
        let value = UnixSocketProvider::new(path)
            .voice_stream_with_options_diagnosed_with_callback(
                &query.language,
                query.generation,
                &query.options,
                cancellation_callback.map(|_| &mut cancellation as &mut dyn FnMut() -> bool),
                &mut update,
                if status_callback.is_some() {
                    Some(&mut status)
                } else {
                    None
                },
                if level_callback.is_some() {
                    Some(&mut level)
                } else {
                    None
                },
            );
        match value {
            Ok(text) => Ok(json!({"text": text})),
            // A named missing dependency is the one provider failure reported as an error, so hosts can show what to install; older callers see it as any other failed call.
            Err(Some(detail)) => Err(format!("voice_dependency_missing:{detail}")),
            Err(None) => Ok(Value::Null),
        }
    })
}

/// Ask a user-owned Unix socket to stop voice capture for one generation.
///
/// # Safety
/// `socket_path` must reference a readable UTF-8 buffer for this call.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_cancel(
    socket_path: *const u8,
    socket_length: usize,
    generation: u64,
) -> *mut c_char {
    response(|| {
        if socket_path.is_null() || socket_length > 4096 {
            return Err("invalid voice provider socket buffer".into());
        }
        let path = super::parse_absolute_socket_path(unsafe {
            std::slice::from_raw_parts(socket_path, socket_length)
        })?;
        Ok(json!(UnixSocketProvider::new(path).voice_cancel(generation)))
    })
}

/// Ask a user-owned voice socket to finish capture and return its final stream
/// result. The streaming connection remains responsible for delivering text.
///
/// # Safety
/// `socket_path` must reference a readable UTF-8 buffer for this call.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_provider_stop(
    socket_path: *const u8,
    socket_length: usize,
    generation: u64,
) -> *mut c_char {
    response(|| {
        if socket_path.is_null() || socket_length > 4096 {
            return Err("invalid voice provider socket buffer".into());
        }
        let path = super::parse_absolute_socket_path(unsafe {
            std::slice::from_raw_parts(socket_path, socket_length)
        })?;
        Ok(json!(UnixSocketProvider::new(path).voice_stop(generation)))
    })
}

// ---- on-device models and hotwords ----

/// Largest request any of the local-model and hotword calls accepts. The hotword correction request carries a transcript and up to a few hundred hotwords.
const LOCAL_VOICE_REQUEST_LIMIT: usize = 1 << 20;
/// Keep the pinyin matcher bounded even when a caller invokes the C ABI directly instead of using
/// the mobile request validator, which applies these same per-field limits.
const MAX_VOICE_CORRECTION_TEXT_CHARS: usize = 10_000;
const MAX_VOICE_CORRECTION_HOTWORDS: usize = 1_000;
const MAX_VOICE_CORRECTION_HOTWORD_TEXT_BYTES: usize = 256;
const MAX_VOICE_CORRECTION_HOTWORD_PINYIN_BYTES: usize = 1_024;

fn local_voice_request<T: serde::de::DeserializeOwned>(
    request: *const u8,
    length: usize,
    limit: usize,
) -> Result<T, String> {
    if request.is_null() || length == 0 || length > limit {
        return Err("invalid voice request buffer".into());
    }
    // SAFETY: the caller contract of every entry point using this guarantees `length` readable bytes; null and size are checked above.
    let bytes = unsafe { std::slice::from_raw_parts(request, length) };
    serde_json::from_slice(bytes).map_err(|_| "invalid voice request".to_owned())
}

fn local_model_root(root: &str) -> Result<&Path, String> {
    if !is_bounded_text(root, 4096) {
        return Err("invalid local model root".into());
    }
    let path = Path::new(root);
    if !path.is_absolute() {
        return Err("invalid local model root".into());
    }
    Ok(path)
}

/// 一次「从本地文件安装」最多带几个文件。目录里的模型最多要两个（压缩包和一个附加文件），留出用户多选几个无关文件的余地。
const MAX_LOCAL_MODEL_IMPORT_FILES: usize = 16;

/// Cancellation flags of the installs running in this process, by model id.
fn local_model_installs() -> &'static Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>> {
    static INSTALLS: OnceLock<Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>> =
        OnceLock::new();
    INSTALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Hotwords for on-device recognition from the user's own pinyin dictionary words.
///
/// Request `{"options": HostOptions, "limit": 200}`, the same HostOptions `msime_client_dictionary` takes. Response `{"hotwords":[{"text","pinyin"}]}`, highest-weighted words first. Reads through the dictionary list action, so it fails with "dictionary maintenance busy" while a maintenance writer holds the store.
///
/// # Safety
/// `request` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_hotwords(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct HotwordsRequest {
            options: Value,
            #[serde(default)]
            limit: Option<usize>,
        }
        let request: HotwordsRequest =
            local_voice_request(request, length, HOST_OPTIONS_DOCUMENT_LIMIT)?;
        let limit = request
            .limit
            .unwrap_or(msime_client_providers::voice::hotwords::DEFAULT_HOTWORD_LIMIT)
            .min(1_000);
        // Enough rows that the heaviest words can be picked even from a large dictionary, without reading the whole store for every voice session.
        let hotwords = msime_client_providers::voice::hotwords::hotwords_from_dictionary_pages(
            limit,
            1_000,
            5_000,
            |offset, page_size| {
                let page = crate::dictionary_request_json(
                    &serde_json::to_vec(&json!({
                        "options": request.options,
                        "action": {"operation": "list", "offset": offset, "limit": page_size, "kind": "pinyin", "user_only": true},
                    }))
                    .map_err(|_| "invalid voice request")?,
                )?;
                let entries = page["entries"].as_array().cloned().unwrap_or_default();
                let mut hotword_entries = Vec::with_capacity(entries.len());
                hotword_entries.extend(entries.iter().filter_map(|entry| {
                    Some((
                        entry["value"].as_str()?.to_owned(),
                        entry["key"].as_str()?.to_owned(),
                        entry["weight"].as_i64().unwrap_or(0),
                    ))
                }));
                Ok::<Option<msime_client_providers::voice::hotwords::DictionaryHotwordPage>, String>(
                    Some(
                        msime_client_providers::voice::hotwords::DictionaryHotwordPage {
                            entries: hotword_entries,
                            has_more: page["has_more"].as_bool() == Some(true),
                        },
                    ),
                )
            },
        )?;
        Ok(json!({ "hotwords": hotwords }))
    })
}

/// Apply hotwords to a final transcript by pinyin similarity, for models whose manifest says `"hotwords": "pinyin"`. The transcript is limited to 10,000 Unicode scalar values, the list to 1,000 entries, and each entry to 256 bytes of text and 1,024 bytes of pinyin.
///
/// Request `{"text": "...", "hotwords": [{"text","pinyin"}]}` (at most 1 MiB); response `{"text": "..."}`. Pure; no state is read.
///
/// # Safety
/// `request` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_hotword_correct(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct CorrectRequest {
            text: String,
            #[serde(default)]
            hotwords: Vec<msime_client_providers::voice::hotwords::Hotword>,
        }
        let request: CorrectRequest =
            local_voice_request(request, length, LOCAL_VOICE_REQUEST_LIMIT)?;
        if !is_bounded_chars_without_nul(&request.text, MAX_VOICE_CORRECTION_TEXT_CHARS)
            || request.hotwords.len() > MAX_VOICE_CORRECTION_HOTWORDS
            || request.hotwords.iter().any(|hotword| {
                hotword.text.trim().is_empty()
                    || !is_bounded_text(&hotword.text, MAX_VOICE_CORRECTION_HOTWORD_TEXT_BYTES)
                    || !is_bounded_text(&hotword.pinyin, MAX_VOICE_CORRECTION_HOTWORD_PINYIN_BYTES)
            })
        {
            return Err("invalid voice hotword correction request".into());
        }
        Ok(json!({
            "text": msime_client_providers::voice::hotwords::correct(&request.text, &request.hotwords),
        }))
    })
}

/// 本地模型的最终文本里，把汉字写的数字改成数字（二零零二年 → 2002年）。
///
/// 请求 `{"text": "...", "kind": "<msime-model.json 的 kind>"}`，响应 `{"text": "..."}`。只有不自带 ITN 的模型（`voice::numbers::applies_to_kind`）会被改写，其他 kind 原样返回，所以宿主对每个本地模型的最终文本都可以调用它。纯函数；文本上限与 `msime_client_voice_hotword_correct` 相同。
///
/// # Safety
/// `request` 必须指向 `length` 个可读字节；null 会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_normalize_numbers(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct NumbersRequest {
            text: String,
            kind: String,
        }
        let request: NumbersRequest =
            local_voice_request(request, length, LOCAL_VOICE_REQUEST_LIMIT)?;
        if !is_bounded_chars_without_nul(&request.text, MAX_VOICE_CORRECTION_TEXT_CHARS)
            || !is_bounded_text(&request.kind, 64)
        {
            return Err("invalid voice number normalization request".into());
        }
        let text = if msime_client_providers::voice::numbers::applies_to_kind(&request.kind) {
            msime_client_providers::voice::numbers::convert(&request.text)
        } else {
            request.text
        };
        Ok(json!({ "text": text }))
    })
}

/// The on-device model catalog with what is installed under a root.
///
/// Request `{"root": "<absolute dir>"}`; response `{"models": [LocalModelStatus], "default": "<id>"}`. A model's `path` is what `voice_input.asr_model_path` is set to when the user picks it.
///
/// # Safety
/// `request` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_local_models(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct ModelsRequest {
            root: String,
        }
        let request: ModelsRequest = local_voice_request(request, length, 16_384)?;
        let root = local_model_root(&request.root)?;
        Ok(json!({
            "models": msime_client_providers::voice::local_models::list(root),
            "default": msime_client_providers::voice::local_models::default_model_id(),
        }))
    })
}

/// 下载、校验并安装一个目录模型。阻塞到结束，必须在工作线程上调用。
///
/// 请求 `{"root": "<absolute dir>", "id": "<catalog id>", "mirror": ""}`，响应 `{"path": "<root>/<id>"}`。`progress` 可为空，在调用线程上收到 `{"id","stage","downloaded","total"}` JSON，阶段为 download、verify、extract 或 done，缓冲区只在回调期间有效。登记后、访问文件或网络前先报告 download（导入为 import）的零字节进度，回调可重入 `msime_client_voice_local_model_cancel` 取消本次安装。取消入口可从任何线程调用，安装随后报 "local_model_cancelled"。每个 id 同时只允许一次安装，重复调用报 "local_model_install_running"。其余失败使用 "local_model_*" 错误码（network、http_status、size_mismatch、checksum_mismatch、unsafe_archive、missing_file、io、invalid_mirror、unknown）。
///
/// 请求带 `"files": ["<absolute path>", ...]`（至多 16 个）时不联网，改用用户自己下载好的这些文件安装（`msime_client_providers::voice::local_models::import`）：按长度和 SHA-256 认文件，不看文件名，`mirror` 不用；进度里原来的 download 阶段报成 import；缺文件时失败为 "local_model_import_missing: <上游文件名>"，所选文件打不开或读出错时为 "local_model_import_unreadable: <说明>"。和下载共用同一个按 id 的登记，所以取消、互斥和删除时的拒绝都一样。
///
/// # Safety
/// `request` 必须指向 `length` 字节的可读缓冲区。`progress` 必须在整个调用期间有效，在返回前复制缓冲区，且不得展开栈。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_local_model_install(
    request: *const u8,
    length: usize,
    progress: Option<unsafe extern "C" fn(*const u8, usize, *mut std::ffi::c_void)>,
    context: *mut std::ffi::c_void,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct InstallRequest {
            root: String,
            id: String,
            #[serde(default)]
            mirror: String,
            #[serde(default)]
            files: Option<Vec<String>>,
        }
        let request: InstallRequest = local_voice_request(request, length, 16_384)?;
        let root = local_model_root(&request.root)?;
        let files = match &request.files {
            Some(files) => {
                if files.len() > MAX_LOCAL_MODEL_IMPORT_FILES
                    || files
                        .iter()
                        .any(|file| !is_bounded_text(file, 4096) || !Path::new(file).is_absolute())
                {
                    return Err("invalid local model import".into());
                }
                Some(files.iter().map(PathBuf::from).collect::<Vec<_>>())
            }
            None => None,
        };
        let model = msime_client_providers::voice::local_models::catalog()
            .models
            .iter()
            .find(|model| model.id == request.id)
            .ok_or("local_model_unknown")?;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let mut installs = local_model_installs()
                .lock()
                .map_err(|_| "internal runtime failure")?;
            if installs.contains_key(&request.id) {
                return Err("local_model_install_running".into());
            }
            installs.insert(request.id.clone(), cancel.clone());
        }
        struct Registered<'a>(&'a str);
        impl Drop for Registered<'_> {
            fn drop(&mut self) {
                if let Ok(mut installs) = local_model_installs().lock() {
                    installs.remove(self.0);
                }
            }
        }
        let _registered = Registered(&request.id);
        let mut report = |event: msime_client_providers::voice::local_models::InstallProgress| {
            if let Some(callback) = progress {
                let text = json!({
                    "id": request.id,
                    "stage": event.stage,
                    "downloaded": event.downloaded,
                    "total": event.total,
                })
                .to_string();
                unsafe {
                    callback(text.as_ptr(), text.len(), context);
                }
            }
        };
        // 登记锁已释放，宿主可在首次回调中补发登记前的取消，避免先进入阻塞 I/O。
        report(
            msime_client_providers::voice::local_models::InstallProgress {
                stage: if files.is_some() {
                    "import"
                } else {
                    "download"
                },
                downloaded: 0,
                total: model.archive.size,
            },
        );
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("local_model_cancelled".into());
        }
        let path = match &files {
            Some(files) => msime_client_providers::voice::local_models::import(
                root,
                &request.id,
                files,
                &mut report,
                &cancel,
            ),
            None => msime_client_providers::voice::local_models::install(
                root,
                &request.id,
                &request.mirror,
                &mut report,
                &cancel,
            ),
        }
        .map_err(|error| error.to_string())?;
        Ok(json!({ "path": path.to_string_lossy() }))
    })
}

/// Stop a running install. Request `{"id": "<catalog id>"}` cancels that model's install; a null request, or one without `id`, cancels every install in this process. Response value: whether anything was running. The install call itself returns once it notices, between chunks.
///
/// # Safety
/// `request` must be null or point to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_local_model_cancel(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct CancelRequest {
            #[serde(default)]
            id: Option<String>,
        }
        let id = if request.is_null() || length == 0 {
            None
        } else {
            local_voice_request::<CancelRequest>(request, length, 16_384)?.id
        };
        let installs = local_model_installs()
            .lock()
            .map_err(|_| "internal runtime failure")?;
        let mut cancelled = false;
        for (running, flag) in installs.iter() {
            if id.as_ref().is_none_or(|id| id == running) {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                cancelled = true;
            }
        }
        Ok(json!(cancelled))
    })
}

/// Delete an installed model. Request `{"root": "<absolute dir>", "id": "<catalog id>"}`; only catalog ids are accepted. Removing a model that is not installed succeeds. Response value: null.
///
/// # Safety
/// `request` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_local_model_remove(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct RemoveRequest {
            root: String,
            id: String,
        }
        let request: RemoveRequest = local_voice_request(request, length, 16_384)?;
        let root = local_model_root(&request.root)?;
        // Hold the registry across the removal: releasing it after the check would let an
        // install of the same id register and have its staging directory swept mid-download.
        let installs = local_model_installs()
            .lock()
            .map_err(|_| "internal runtime failure")?;
        if installs.contains_key(&request.id) {
            return Err("local_model_install_running".into());
        }
        let removed = msime_client_providers::voice::local_models::remove(root, &request.id);
        drop(installs);
        removed.map_err(|error| error.to_string())?;
        Ok(Value::Null)
    })
}

/// 偏好库会不会接受本地识别的两个字段，与 `Preferences::validate` 的判断相同：`asr_model_mirror` 按 [`msime_client_core::preferences::valid_model_mirror`]，`asr_model_path` 按 [`msime_client_core::preferences::valid_model_path`]。原生设置窗口边输入边保存，只把这里认可的值写进设置，免得一个半截的镜像地址让整份偏好保存失败。
///
/// Request `{"asr_model_mirror"?: string, "asr_model_path"?: string}`（至多 64 KiB）；value 只含请求里给出的键，各为一个布尔值。纯函数，不读任何状态。
///
/// # Safety
/// `request` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_model_setting_check(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct CheckRequest {
            #[serde(default)]
            asr_model_mirror: Option<String>,
            #[serde(default)]
            asr_model_path: Option<String>,
        }
        let request: CheckRequest = local_voice_request(request, length, 64 * 1024)?;
        let mut value = serde_json::Map::new();
        if let Some(mirror) = &request.asr_model_mirror {
            value.insert(
                "asr_model_mirror".into(),
                Value::Bool(msime_client_core::preferences::valid_model_mirror(mirror)),
            );
        }
        if let Some(path) = &request.asr_model_path {
            value.insert(
                "asr_model_path".into(),
                Value::Bool(msime_client_core::preferences::valid_model_path(path)),
            );
        }
        Ok(Value::Object(value))
    })
}

// ---- 确定性语音后处理、跨语种个人词表与本地/云端润色 C ABI ----

const MAX_POLISH_CLEAN_OUTPUT_CHARS: usize = 40_000;
const MAX_VOCABULARY_DICTIONARY_SCAN_ROWS: usize = 5_000;

fn valid_vocabulary_terms(
    terms: &[msime_client_providers::voice::vocabulary::VocabularyTerm],
) -> bool {
    if terms.len() > MAX_VOICE_CORRECTION_HOTWORDS {
        return false;
    }
    terms.iter().all(|term| {
        is_bounded_text(&term.canonical, MAX_VOICE_CORRECTION_HOTWORD_TEXT_BYTES)
            && is_bounded_text(&term.pinyin, MAX_VOICE_CORRECTION_HOTWORD_PINYIN_BYTES)
            && is_bounded_text(&term.category, 64)
            && is_bounded_text(&term.source, 32)
            && term.aliases.len() <= 32
            && term
                .aliases
                .iter()
                .all(|alias| is_bounded_text(alias, MAX_VOICE_CORRECTION_HOTWORD_TEXT_BYTES))
    })
}

fn collect_voice_vocabulary_dictionary_kind(
    options: &Value,
    kind: &str,
) -> Result<Vec<msime_client_providers::voice::vocabulary::DictionaryEntryInput>, String> {
    let mut collected = Vec::new();
    let mut offset = 0usize;
    let page_size = 1_000usize;
    while offset < MAX_VOCABULARY_DICTIONARY_SCAN_ROWS {
        let request = serde_json::to_vec(&json!({
            "options": options,
            "action": {
                "operation": "list",
                "offset": offset,
                "limit": page_size,
                "kind": kind,
                "user_only": true,
            },
        }))
        .map_err(|_| "invalid voice vocabulary request")?;
        let page = crate::dictionary_request_json(&request)?;
        let entries = page["entries"].as_array().cloned().unwrap_or_default();
        let count = entries.len();
        for entry in entries {
            let (Some(value), Some(key)) = (entry["value"].as_str(), entry["key"].as_str()) else {
                continue;
            };
            collected.push(
                msime_client_providers::voice::vocabulary::DictionaryEntryInput {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    weight: entry["weight"].as_i64().unwrap_or(0),
                },
            );
        }
        if page["has_more"].as_bool() != Some(true) || count == 0 {
            break;
        }
        offset = offset.saturating_add(count);
    }
    Ok(collected)
}

/// 无网络确定性语音后处理入口：执行空白与缩进规整、块边界结巴清理、纯语气词/幻觉拦截、
/// 标点规整、双边界保守 ITN、个人词表与技术词大小写、Pangu 空格及句末句号策略。
///
/// 请求 JSON 上限 1 MiB，`text` 上限 10,000 个 Unicode 标量，`vocabulary` 至多 1,000 条。
/// 响应 `value`：`{text, dismissed, dismissed_reason, changed}`。纯函数，不访问磁盘或网络。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节；空指针或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_postprocess(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let input: msime_client_providers::voice::postprocess::PostprocessInput =
            local_voice_request(request, length, LOCAL_VOICE_REQUEST_LIMIT)?;
        if !is_bounded_chars_without_nul(&input.text, MAX_VOICE_CORRECTION_TEXT_CHARS)
            || !is_bounded_text(&input.model_kind, 64)
            || !valid_vocabulary_terms(&input.vocabulary)
        {
            return Err("invalid voice postprocess request".into());
        }
        let output = msime_client_providers::voice::postprocess::postprocess(&input);
        serde_json::to_value(output).map_err(|error| error.to_string())
    })
}

/// 聚合跨语种个人语音词表：只读合并用户词库（`pinyin`/`english`/`quick_phrase`）与
/// `<preferences_directory>/voice-vocabulary.json`，并按需针对 `raw_text` 返回 Top-K 润色词表提示。
/// 若传入 `cached_terms`，则直接基于内存词条执行纯内存 `prompt_hints` 筛选而不触碰磁盘或 SQLite。
/// 单类词库读取失败（如词库维护租约忙）时该类降级为空集合，保留自定义词表与其他词库结果。
///
/// 请求 JSON 上限 1 MiB：`{options?: HostOptions, preferences_directory?: absolute_dir, cached_terms?: [...],
/// limit?: 300, raw_text?: string, app_category?: string, prompt_limit?: 60}`。
/// 响应 `value`：`{version, terms, prompt_hints?}`。涉及磁盘与词库读取时应在工作线程调用。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节；空指针或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_vocabulary(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct VocabularyRequest {
            #[serde(default)]
            options: Option<Value>,
            #[serde(default)]
            preferences_directory: Option<String>,
            #[serde(default)]
            cached_terms: Option<Vec<msime_client_providers::voice::vocabulary::VocabularyTerm>>,
            #[serde(default)]
            limit: Option<usize>,
            #[serde(default)]
            raw_text: Option<String>,
            #[serde(default)]
            app_category: Option<msime_client_providers::voice::postprocess::AppCategory>,
            #[serde(default)]
            prompt_limit: Option<usize>,
        }
        let req: VocabularyRequest =
            local_voice_request(request, length, HOST_OPTIONS_DOCUMENT_LIMIT)?;
        if let Some(raw_text) = req.raw_text.as_deref() {
            if !is_bounded_chars_without_nul(raw_text, MAX_VOICE_CORRECTION_TEXT_CHARS) {
                return Err("invalid voice vocabulary request".into());
            }
        }
        let vocabulary = if let Some(cached_terms) = req.cached_terms {
            if !valid_vocabulary_terms(&cached_terms) {
                return Err("invalid voice vocabulary request".into());
            }
            msime_client_providers::voice::vocabulary::VoiceVocabulary::new(cached_terms)
        } else {
            let pref_dir = req.preferences_directory.as_deref().or_else(|| {
                req.options
                    .as_ref()
                    .and_then(|opts| opts.get("preferences_directory"))
                    .and_then(Value::as_str)
            });
            let custom = if let Some(dir) = pref_dir.filter(|value| !value.is_empty()) {
                let path = local_model_root(dir)
                    .map_err(|_| "invalid voice vocabulary preferences directory".to_owned())?;
                Some(msime_client_providers::voice::vocabulary::load_from_preferences_dir(path)?)
            } else {
                None
            };
            let (pinyin, english, quick_phrase) = match req.options.as_ref() {
                Some(options) if options.is_object() => {
                    // 单类词库读取失败（如词库维护租约忙或该类词库暂未就绪）时局部降级，不拖垮自定义词表与其他词库。
                    let pinyin = collect_voice_vocabulary_dictionary_kind(options, "pinyin")
                        .unwrap_or_default();
                    let english = collect_voice_vocabulary_dictionary_kind(options, "english")
                        .unwrap_or_default();
                    let quick_phrase =
                        collect_voice_vocabulary_dictionary_kind(options, "quick_phrase")
                            .unwrap_or_default();
                    (pinyin, english, quick_phrase)
                }
                Some(_) => return Err("invalid voice vocabulary request".into()),
                None => (Vec::new(), Vec::new(), Vec::new()),
            };
            let limit = req
                .limit
                .unwrap_or(msime_client_providers::voice::vocabulary::DEFAULT_VOCABULARY_LIMIT)
                .min(msime_client_providers::voice::vocabulary::MAX_VOCABULARY_LIMIT);
            msime_client_providers::voice::vocabulary::build_vocabulary(
                custom.as_ref(),
                &pinyin,
                &english,
                &quick_phrase,
                limit,
            )
        };
        let mut value = json!({
            "version": vocabulary.version,
            "terms": vocabulary.terms,
        });
        if let Some(raw_text) = req.raw_text.as_deref() {
            let prompt_limit = req
                .prompt_limit
                .unwrap_or(msime_client_providers::voice::vocabulary::DEFAULT_PROMPT_TERM_LIMIT)
                .min(msime_client_providers::voice::vocabulary::MAX_PROMPT_TERM_LIMIT);
            let hints = msime_client_providers::voice::vocabulary::select_prompt_terms(
                raw_text,
                req.app_category.unwrap_or_default(),
                &vocabulary,
                prompt_limit,
            );
            value["prompt_hints"] = json!(hints);
        }
        Ok(value)
    })
}

/// 管理 `<preferences_directory>/voice-vocabulary.json` 并支持事后编辑自动学词与撤销。
///
/// 请求 JSON 上限 256 KiB：
/// - `action = "list"`: 返回自定义词表 `{version, terms}`；
/// - `action = "upsert"`: 新增或合并更新 `term`，返回 `{term, previous_term, changed}`；
/// - `action = "remove"`: 删除 `canonical`，返回 `{removed}`；
/// - `action = "restore"`: 按 `canonical` 与 `previous_term` 精确撤销上次写入；
/// - `action = "learn"`: 从 `{inserted_text, original_box, edited_box}` 提取纠错词并原子落盘，返回 `{learned}`。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节；空指针或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_vocabulary_manage(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        #[derive(Deserialize)]
        struct VocabularyManageRequest {
            action: String,
            preferences_directory: String,
            #[serde(default)]
            term: Option<msime_client_providers::voice::vocabulary::VocabularyTerm>,
            #[serde(default)]
            canonical: Option<String>,
            #[serde(default)]
            previous_term: Option<msime_client_providers::voice::vocabulary::VocabularyTerm>,
            #[serde(default)]
            inserted_text: Option<String>,
            #[serde(default)]
            original_box: Option<String>,
            #[serde(default)]
            edited_box: Option<String>,
        }
        let req: VocabularyManageRequest = local_voice_request(request, length, 256 * 1024)?;
        let pref_dir = local_model_root(&req.preferences_directory)
            .map_err(|_| "invalid voice vocabulary preferences directory".to_owned())?;
        match req.action.trim() {
            "list" => {
                let vocab =
                    msime_client_providers::voice::vocabulary::load_from_preferences_dir(pref_dir)?;
                serde_json::to_value(vocab).map_err(|error| error.to_string())
            }
            "upsert" => {
                let term = req
                    .term
                    .ok_or_else(|| "missing vocabulary term".to_owned())?;
                if !valid_vocabulary_terms(std::slice::from_ref(&term)) {
                    return Err("invalid voice vocabulary term".into());
                }
                let outcome =
                    msime_client_providers::voice::vocabulary::upsert_term_in_preferences_dir(
                        pref_dir, term,
                    )?;
                serde_json::to_value(outcome).map_err(|error| error.to_string())
            }
            "remove" => {
                let canonical = req.canonical.as_deref().unwrap_or("");
                if !is_bounded_text(canonical, MAX_VOICE_CORRECTION_HOTWORD_TEXT_BYTES) {
                    return Err("invalid voice vocabulary canonical".into());
                }
                let removed =
                    msime_client_providers::voice::vocabulary::remove_term_in_preferences_dir(
                        pref_dir, canonical,
                    )?;
                Ok(json!({"removed": removed}))
            }
            "restore" => {
                let canonical = req.canonical.as_deref().unwrap_or("");
                if !is_bounded_text(canonical, MAX_VOICE_CORRECTION_HOTWORD_TEXT_BYTES) {
                    return Err("invalid voice vocabulary canonical".into());
                }
                if let Some(prev) = req.previous_term.as_ref() {
                    if !valid_vocabulary_terms(std::slice::from_ref(prev)) {
                        return Err("invalid voice vocabulary term".into());
                    }
                }
                msime_client_providers::voice::vocabulary::restore_term_in_preferences_dir(
                    pref_dir,
                    canonical,
                    req.previous_term,
                )?;
                Ok(json!({"restored": true}))
            }
            "learn" => {
                let inserted = req.inserted_text.as_deref().unwrap_or("");
                let original = req.original_box.as_deref().unwrap_or("");
                let edited = req.edited_box.as_deref().unwrap_or("");
                if !is_bounded_chars_without_nul(inserted, MAX_VOICE_CORRECTION_TEXT_CHARS)
                    || !is_bounded_chars_without_nul(original, MAX_VOICE_CORRECTION_TEXT_CHARS)
                    || !is_bounded_chars_without_nul(edited, MAX_VOICE_CORRECTION_TEXT_CHARS)
                {
                    return Err("invalid voice vocabulary learn payload".into());
                }
                let report =
                    msime_client_providers::voice::vocabulary::learn_corrections_in_preferences_dir(
                        pref_dir, inserted, original, edited,
                    )?;
                serde_json::to_value(report).map_err(|error| error.to_string())
            }
            _ => Err("unknown voice vocabulary manage action".into()),
        }
    })
}

/// 清洗并校验 LLM 润色/改写/翻译输出（剥离 `<think>`、解析 `<action>`、防助手元回复/膨胀/塌缩/繁体漂移）。
///
/// 请求 JSON 上限 1 MiB；响应 `value`：`{accepted, text, resolved_action, rejection_reason}`。
/// `accepted` 为 `false` 时宿主应降级使用确定性后处理后的原文。纯函数，不访问磁盘或网络。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节；空指针或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_polish_clean(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let input: msime_client_providers::voice::polish_clean::PolishCleanInput =
            local_voice_request(request, length, LOCAL_VOICE_REQUEST_LIMIT)?;
        if !is_bounded_chars_without_nul(&input.raw_text, MAX_VOICE_CORRECTION_TEXT_CHARS)
            || !is_bounded_chars_without_nul(&input.polished_text, MAX_POLISH_CLEAN_OUTPUT_CHARS)
            || !input.selected_text.as_deref().is_none_or(|sel| {
                is_bounded_chars_without_nul(sel, MAX_VOICE_CORRECTION_TEXT_CHARS)
            })
            || !valid_vocabulary_terms(&input.vocabulary)
        {
            return Err("invalid voice polish clean request".into());
        }
        let output = msime_client_providers::voice::polish_clean::clean_polish_output(&input);
        serde_json::to_value(output).map_err(|error| error.to_string())
    })
}

/// 构造语音润色/改写/翻译的 Sans-IO OpenAI 兼容 HTTP 请求描述器。
///
/// 请求 JSON 上限 256 KiB；密码框下非本机回环端点（云端及局域网）返回错误 `polish_secure_field_blocked`，
/// 本机回环端点自动清空上下文、选区与词表后放行；公网明文地址返回 `polish_invalid_endpoint`，
/// 公网缺凭据返回 `polish_missing_token`。描述器可能含凭据，严禁写日志。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节；空指针或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_polish_http_request(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 {
            return Err("polish_empty_input".into());
        }
        if length > msime_client_providers::voice::polish::MAX_POLISH_REQUEST_BYTES {
            return Err("polish_input_too_large".into());
        }
        // SAFETY: 非空指针与长度上限已在上文校验，满足调用方可读字节切片契约。
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let input: msime_client_providers::voice::polish::PolishHttpRequestInput =
            serde_json::from_slice(bytes).map_err(|_| "invalid voice polish request".to_owned())?;
        let descriptor = msime_client_providers::voice::polish::build_polish_http_request(&input)
            .map_err(|error| error.code().to_owned())?;
        serde_json::to_value(descriptor).map_err(|error| error.to_string())
    })
}

/// 解析润色接口返回的非流式 JSON 响应体或 SSE 流式数据块。
///
/// 非流式响应上限 1 MiB，单次 SSE 载荷上限 64 KiB；响应 `value`：`{text, finish_reason, done}`。
///
/// # Safety
/// `body` 必须非空且指向本次调用期间可读的 `length` 个字节；空指针或超限会被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_polish_parse_response(
    body: *const u8,
    length: usize,
    is_sse: bool,
) -> *mut c_char {
    response(|| {
        if body.is_null() {
            return Err("polish_invalid_response".into());
        }
        let max_bytes = if is_sse {
            msime_client_providers::voice::polish::MAX_POLISH_SSE_CHUNK_BYTES
        } else {
            msime_client_providers::voice::polish::MAX_POLISH_RESPONSE_BYTES
        };
        if length > max_bytes {
            return Err("polish_response_too_large".into());
        }
        // SAFETY: `body` 已校验非空且长度不越界，即使 `length == 0` 也满足切片构造前提。
        let bytes = unsafe { std::slice::from_raw_parts(body, length) };
        let parsed = msime_client_providers::voice::polish::parse_polish_response(bytes, is_sse)
            .map_err(|error| error.code().to_owned())?;
        serde_json::to_value(parsed).map_err(|error| error.to_string())
    })
}

#[cfg(test)]
mod wave2_voice_abi_tests {
    use super::*;
    use std::ffi::CStr;

    fn take_json(ptr: *mut c_char) -> Value {
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        unsafe { crate::msime_client_string_free(ptr) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn postprocess_abi_handles_normal_dismiss_and_invalid_inputs() {
        let null_resp = take_json(unsafe { msime_client_voice_postprocess(std::ptr::null(), 0) });
        assert_eq!(null_resp["ok"], false);

        let bad_utf8 = [0xffu8, 0xfeu8];
        let bad_resp =
            take_json(unsafe { msime_client_voice_postprocess(bad_utf8.as_ptr(), bad_utf8.len()) });
        assert_eq!(bad_resp["ok"], false);

        let filler_req = serde_json::to_vec(&json!({
            "text": "嗯。",
            "app_category": "im"
        }))
        .unwrap();
        let filler_resp = take_json(unsafe {
            msime_client_voice_postprocess(filler_req.as_ptr(), filler_req.len())
        });
        assert_eq!(filler_resp["ok"], true);
        assert_eq!(filler_resp["value"]["dismissed"], true);
        assert_eq!(filler_resp["value"]["dismissed_reason"], "filler_only");

        let normal_req = serde_json::to_vec(&json!({
            "text": "我在用shurufa测试OpenAPI接口。",
            "app_category": "im",
            "vocabulary": [{"canonical": "Shurufa", "aliases": ["shurufa"]}]
        }))
        .unwrap();
        let normal_resp = take_json(unsafe {
            msime_client_voice_postprocess(normal_req.as_ptr(), normal_req.len())
        });
        assert_eq!(normal_resp["ok"], true);
        assert_eq!(normal_resp["value"]["dismissed"], false);
        assert_eq!(
            normal_resp["value"]["text"],
            "我在用 Shurufa 测试 OpenAPI 接口"
        );
    }

    #[test]
    fn vocabulary_abi_loads_custom_file_degrades_unavailable_dict_and_supports_cached_terms() {
        let null_resp = take_json(unsafe { msime_client_voice_vocabulary(std::ptr::null(), 0) });
        assert_eq!(null_resp["ok"], false);

        let rel_req = serde_json::to_vec(&json!({
            "preferences_directory": "relative/dir"
        }))
        .unwrap();
        let rel_resp =
            take_json(unsafe { msime_client_voice_vocabulary(rel_req.as_ptr(), rel_req.len()) });
        assert_eq!(rel_resp["ok"], false);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("voice-vocabulary.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "terms": [
                    {"canonical": "Shurufa", "aliases": ["输入法测试词"], "weight": 500},
                    {"canonical": "Kubernetes", "aliases": ["k8s"], "weight": 300}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        // 即便 options 中的词库路径尚未初始化导致词库读取失败，仍须保留 voice-vocabulary.json。
        let req = serde_json::to_vec(&json!({
            "options": {"preferences_directory": dir.path().to_string_lossy()},
            "preferences_directory": dir.path().to_string_lossy(),
            "raw_text": "今天在 k8s 上部署输入法测试词",
            "app_category": "terminal_ide",
            "limit": 100,
            "prompt_limit": 10
        }))
        .unwrap();
        let resp = take_json(unsafe { msime_client_voice_vocabulary(req.as_ptr(), req.len()) });
        assert_eq!(resp["ok"], true);
        let terms = resp["value"]["terms"].as_array().unwrap();
        assert_eq!(terms.len(), 2);
        let hints = resp["value"]["prompt_hints"].as_array().unwrap();
        assert!(hints.iter().any(|item| item == "Shurufa"));
        assert!(hints.iter().any(|item| item == "Kubernetes"));

        // `cached_terms` 零磁盘路径直接筛选 prompt_hints。
        let cached_req = serde_json::to_vec(&json!({
            "cached_terms": terms,
            "raw_text": "部署 k8s 服务",
            "app_category": "terminal_ide",
            "prompt_limit": 5
        }))
        .unwrap();
        let cached_resp = take_json(unsafe {
            msime_client_voice_vocabulary(cached_req.as_ptr(), cached_req.len())
        });
        assert_eq!(cached_resp["ok"], true);
        let cached_hints = cached_resp["value"]["prompt_hints"].as_array().unwrap();
        assert!(cached_hints.iter().any(|item| item == "Kubernetes"));
    }

    #[test]
    fn polish_clean_abi_strips_think_and_resolves_action() {
        let null_resp = take_json(unsafe { msime_client_voice_polish_clean(std::ptr::null(), 0) });
        assert_eq!(null_resp["ok"], false);

        let rewrite_req = serde_json::to_vec(&json!({
            "raw_text": "把这段改正式一点",
            "polished_text": "<think>内部思考</think><action>rewrite</action>请确认上述接口变更方案是否满足发布要求，并在评审通过后同步排期。",
            "selected_text": "看看这个接口改动行不行，没问题的话后面把排期也发一下",
            "mode": "rewrite_selection",
            "expect_action_tag": true,
            "app_category": "mail"
        }))
        .unwrap();
        let rewrite_resp = take_json(unsafe {
            msime_client_voice_polish_clean(rewrite_req.as_ptr(), rewrite_req.len())
        });
        assert_eq!(rewrite_resp["ok"], true);
        assert_eq!(rewrite_resp["value"]["accepted"], true);
        assert_eq!(
            rewrite_resp["value"]["resolved_action"],
            "replace_selection"
        );
        assert_eq!(
            rewrite_resp["value"]["text"],
            "请确认上述接口变更方案是否满足发布要求，并在评审通过后同步排期。"
        );

        let unclosed_req = serde_json::to_vec(&json!({
            "raw_text": "合成语音原文",
            "polished_text": "<think>只输出了未闭合思考"
        }))
        .unwrap();
        let unclosed_resp = take_json(unsafe {
            msime_client_voice_polish_clean(unclosed_req.as_ptr(), unclosed_req.len())
        });
        assert_eq!(unclosed_resp["ok"], true);
        assert_eq!(unclosed_resp["value"]["accepted"], false);
        assert_eq!(unclosed_resp["value"]["rejection_reason"], "unclosed_think");
    }

    #[test]
    fn polish_http_request_and_parse_response_abi_enforce_gates_and_bounds() {
        let null_req =
            take_json(unsafe { msime_client_voice_polish_http_request(std::ptr::null(), 0) });
        assert_eq!(null_req["ok"], false);
        assert_eq!(null_req["error"], "polish_empty_input");

        let oversized_len = msime_client_providers::voice::polish::MAX_POLISH_REQUEST_BYTES + 1;
        let dummy = b"{}";
        let too_large = take_json(unsafe {
            msime_client_voice_polish_http_request(dummy.as_ptr(), oversized_len)
        });
        assert_eq!(too_large["ok"], false);
        assert_eq!(too_large["error"], "polish_input_too_large");

        // 密码框在云端与局域网端点必须拒绝外发（polish_secure_field_blocked）。
        let secure_cloud_req = serde_json::to_vec(&json!({
            "provider": "openai",
            "endpoint": "https://api.openai.com/v1/chat/completions",
            "model": "gpt-4o-mini",
            "token": "synthetic-key",
            "raw_text": "合成敏感测试",
            "context": {"is_secure_field": true}
        }))
        .unwrap();
        let secure_cloud_resp = take_json(unsafe {
            msime_client_voice_polish_http_request(
                secure_cloud_req.as_ptr(),
                secure_cloud_req.len(),
            )
        });
        assert_eq!(secure_cloud_resp["ok"], false);
        assert_eq!(secure_cloud_resp["error"], "polish_secure_field_blocked");

        let secure_lan_req = serde_json::to_vec(&json!({
            "provider": "custom",
            "endpoint": "http://192.168.1.50:11434/v1/chat/completions",
            "model": "qwen3.5:9b",
            "raw_text": "合成敏感测试",
            "context": {"is_secure_field": true}
        }))
        .unwrap();
        let secure_lan_resp = take_json(unsafe {
            msime_client_voice_polish_http_request(secure_lan_req.as_ptr(), secure_lan_req.len())
        });
        assert_eq!(secure_lan_resp["ok"], false);
        assert_eq!(secure_lan_resp["error"], "polish_secure_field_blocked");

        // 本机回环端点在密码框下允许离线整理，但必须剥离所有窗口标题、前文、选区与词表。
        let secure_loopback_req = serde_json::to_vec(&json!({
            "provider": "local",
            "endpoint": "http://127.0.0.1:18080/v1/chat/completions",
            "model": "local",
            "raw_text": "合成密码框本地整理",
            "vocabulary_hints": ["SyntheticVocabSecret"],
            "context": {
                "is_secure_field": true,
                "app_bundle_id": "com.tencent.xinWeChat",
                "window_title": "SyntheticLoginTitle",
                "preceding_text": "SyntheticPrecedingSecret",
                "selected_text": "SyntheticSelectedSecret"
            }
        }))
        .unwrap();
        let secure_loopback_resp = take_json(unsafe {
            msime_client_voice_polish_http_request(
                secure_loopback_req.as_ptr(),
                secure_loopback_req.len(),
            )
        });
        assert_eq!(secure_loopback_resp["ok"], true);
        assert_eq!(secure_loopback_resp["value"]["app_category"], "general");
        let loopback_body = secure_loopback_resp["value"]["body"].as_str().unwrap();
        assert!(!loopback_body.contains("SyntheticLoginTitle"));
        assert!(!loopback_body.contains("SyntheticPrecedingSecret"));
        assert!(!loopback_body.contains("SyntheticSelectedSecret"));
        assert!(!loopback_body.contains("SyntheticVocabSecret"));

        let ok_req = serde_json::to_vec(&json!({
            "provider": "local",
            "endpoint": "http://127.0.0.1:18080/v1/chat/completions",
            "model": "local",
            "raw_text": "嗯今天下午三点开会",
            "context": {"app_bundle_id": "com.tencent.xinWeChat", "app_name": "WeChat"}
        }))
        .unwrap();
        let ok_resp = take_json(unsafe {
            msime_client_voice_polish_http_request(ok_req.as_ptr(), ok_req.len())
        });
        assert_eq!(ok_resp["ok"], true);
        assert_eq!(ok_resp["value"]["no_proxy"], true);
        assert_eq!(ok_resp["value"]["resolved_mode"], "dictation");
        assert_eq!(ok_resp["value"]["app_category"], "im");

        // 响应解析（非流式、SSE 与边界）。
        let null_parse = take_json(unsafe {
            msime_client_voice_polish_parse_response(std::ptr::null(), 0, false)
        });
        assert_eq!(null_parse["ok"], false);
        assert_eq!(null_parse["error"], "polish_invalid_response");

        let sse_oversized = take_json(unsafe {
            msime_client_voice_polish_parse_response(
                dummy.as_ptr(),
                msime_client_providers::voice::polish::MAX_POLISH_SSE_CHUNK_BYTES + 1,
                true,
            )
        });
        assert_eq!(sse_oversized["ok"], false);
        assert_eq!(sse_oversized["error"], "polish_response_too_large");

        let non_stream =
            r#"{"choices":[{"message":{"content":"今天下午三点开会"},"finish_reason":"stop"}]}"#
                .as_bytes();
        let parsed = take_json(unsafe {
            msime_client_voice_polish_parse_response(non_stream.as_ptr(), non_stream.len(), false)
        });
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["value"]["text"], "今天下午三点开会");
        assert_eq!(parsed["value"]["done"], true);

        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"合成\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n".as_bytes();
        let sse_parsed = take_json(unsafe {
            msime_client_voice_polish_parse_response(sse.as_ptr(), sse.len(), true)
        });
        assert_eq!(sse_parsed["ok"], true);
        assert_eq!(sse_parsed["value"]["text"], "合成");
        assert_eq!(sse_parsed["value"]["done"], true);
    }

    #[test]
    fn vocabulary_manage_abi_supports_crud_learn_and_undo_restore() {
        let dir = tempfile::tempdir().unwrap();
        let dir_str = dir.path().to_str().unwrap();

        let upsert_req = serde_json::to_vec(&json!({
            "action": "upsert",
            "preferences_directory": dir_str,
            "term": {
                "canonical": "T3 Code",
                "aliases": ["t3code"],
                "source": "manual",
                "weight": 1000
            }
        }))
        .unwrap();
        let upsert_resp = take_json(unsafe {
            msime_client_voice_vocabulary_manage(upsert_req.as_ptr(), upsert_req.len())
        });
        assert_eq!(upsert_resp["ok"], true);
        assert_eq!(upsert_resp["value"]["changed"], true);

        let learn_req = serde_json::to_vec(&json!({
            "action": "learn",
            "preferences_directory": dir_str,
            "inserted_text": "用可劳德检查一下",
            "original_box": "用可劳德检查一下",
            "edited_box": "用Claude检查一下"
        }))
        .unwrap();
        let learn_resp = take_json(unsafe {
            msime_client_voice_vocabulary_manage(learn_req.as_ptr(), learn_req.len())
        });
        assert_eq!(learn_resp["ok"], true);
        let learned = learn_resp["value"]["learned"].as_array().unwrap();
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0]["canonical"], "Claude");
        assert_eq!(learned[0]["alias"], "可劳德");

        let list_req = serde_json::to_vec(&json!({
            "action": "list",
            "preferences_directory": dir_str
        }))
        .unwrap();
        let list_resp = take_json(unsafe {
            msime_client_voice_vocabulary_manage(list_req.as_ptr(), list_req.len())
        });
        assert_eq!(list_resp["ok"], true);
        assert_eq!(list_resp["value"]["terms"].as_array().unwrap().len(), 2);

        let restore_req = serde_json::to_vec(&json!({
            "action": "restore",
            "preferences_directory": dir_str,
            "canonical": "Claude",
            "previous_term": learned[0]["previous_term"]
        }))
        .unwrap();
        let restore_resp = take_json(unsafe {
            msime_client_voice_vocabulary_manage(restore_req.as_ptr(), restore_req.len())
        });
        assert_eq!(restore_resp["ok"], true);

        let remove_req = serde_json::to_vec(&json!({
            "action": "remove",
            "preferences_directory": dir_str,
            "canonical": "T3 Code"
        }))
        .unwrap();
        let remove_resp = take_json(unsafe {
            msime_client_voice_vocabulary_manage(remove_req.as_ptr(), remove_req.len())
        });
        assert_eq!(remove_resp["ok"], true);
        assert_eq!(remove_resp["value"]["removed"], true);
    }
}
