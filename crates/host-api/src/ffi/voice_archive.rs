//! 语音存档的 C ABI：录音一边进行一边加密落盘，听写结果、失败和取消追加到同一条记录里。
//!
//! 进程里只有一个存档和至多一条正在录的录音，状态放在全局锁里：音频从宿主的采集队列追加，其余操作在主线程。
//! 存档格式与加密见 `msime_client_providers::voice::archive`；密钥由宿主保管并在打开时交进来。

use crate::*;
use msime_client_core::preferences::{MAX_HISTORY_RETENTION_DAYS, RECORDING_RETENTION_HOURS};
use msime_client_providers::voice::archive::{
    validate_dismissed_reason, Archive, ArchiveError, Recording, TextNoteMetadata,
    MAX_METADATA_TAG_BYTES, MAX_MODEL_NAME_BYTES, MAX_REASON_BYTES, MAX_TEXT_BYTES,
};
use std::ffi::c_void;
use std::sync::Mutex;

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
/// 一次追加最多十秒音频；采集回调每次只交几十毫秒。
const MAX_APPEND_SAMPLES: usize = 160_000;
/// `note` 请求可能同时携带最终文本与原始转写文本（各至多 1 MiB）及短元数据字段。
const MAX_NOTE_REQUEST_BYTES: usize = MAX_TEXT_BYTES * 2 + 8192;

struct State {
    archive: Archive,
    live: Option<Recording>,
    /// 最近一次录音的 id：识别结果可能在录音停止之后才到，仍然记到这一条上。
    last: Option<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(operation: impl FnOnce(&mut State) -> Result<T, String>) -> Result<T, String> {
    let mut guard = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = guard.as_mut().ok_or("voice_archive_closed")?;
    operation(state)
}

fn archive_error(error: ArchiveError) -> String {
    match error {
        ArchiveError::InvalidId => "voice_archive_invalid_id",
        ArchiveError::Corrupt => "voice_archive_corrupt",
        ArchiveError::Random => "voice_archive_random_unavailable",
        ArchiveError::InvalidMetadata => "invalid voice archive request",
        ArchiveError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "voice_archive_not_found"
        }
        ArchiveError::Io(_) => "voice_archive_io",
    }
    .into()
}

unsafe fn json_request_bounded(
    pointer: *const u8,
    length: usize,
    max_bytes: usize,
) -> Result<Value, String> {
    unsafe {
        with_bounded_bytes(
            pointer,
            length,
            max_bytes,
            "invalid voice archive request",
            |bytes| {
                let value: Value = serde_json::from_slice(bytes)
                    .map_err(|_| "invalid voice archive request".to_owned())?;
                if !value.is_object() {
                    return Err("invalid voice archive request".to_owned());
                }
                Ok(value)
            },
        )
    }
}

unsafe fn json_request(pointer: *const u8, length: usize) -> Result<Value, String> {
    unsafe { json_request_bounded(pointer, length, MAX_TEXT_BYTES + 4096) }
}

fn string_field<'a>(request: &'a Value, field: &str) -> Result<&'a str, String> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(value)) if value.len() <= MAX_TEXT_BYTES => Ok(value),
        _ => Err("invalid voice archive request".into()),
    }
}

fn optional_string_field<'a>(
    request: &'a Value,
    field: &str,
    max_bytes: usize,
) -> Result<Option<&'a str>, String> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= max_bytes => Ok(Some(value.as_str())),
        _ => Err("invalid voice archive request".into()),
    }
}

fn optional_bool_field(request: &Value, field: &str) -> Result<Option<bool>, String> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err("invalid voice archive request".into()),
    }
}

fn optional_u64_field(request: &Value, field: &str) -> Result<Option<u64>, String> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| "invalid voice archive request".to_owned()),
        _ => Err("invalid voice archive request".into()),
    }
}

fn parse_text_metadata(request: &Value) -> Result<TextNoteMetadata, String> {
    let metadata = TextNoteMetadata {
        raw_text: optional_string_field(request, "raw_text", MAX_TEXT_BYTES)?
            .map(ToOwned::to_owned),
        mode: optional_string_field(request, "mode", MAX_METADATA_TAG_BYTES)?
            .map(ToOwned::to_owned),
        app_category: optional_string_field(request, "app_category", MAX_METADATA_TAG_BYTES)?
            .map(ToOwned::to_owned),
        polish_provider: optional_string_field(request, "polish_provider", MAX_METADATA_TAG_BYTES)?
            .map(ToOwned::to_owned),
        polish_model: optional_string_field(request, "polish_model", MAX_MODEL_NAME_BYTES)?
            .map(ToOwned::to_owned),
        polish_prompt_id: optional_string_field(
            request,
            "polish_prompt_id",
            MAX_METADATA_TAG_BYTES,
        )?
        .map(ToOwned::to_owned),
        polish_failed: optional_bool_field(request, "polish_failed")?,
        polish_rejected_reason: optional_string_field(
            request,
            "polish_rejected_reason",
            MAX_REASON_BYTES,
        )?
        .map(ToOwned::to_owned),
        asr_ms: optional_u64_field(request, "asr_ms")?,
        polish_ms: optional_u64_field(request, "polish_ms")?,
    };
    metadata.validate().map_err(archive_error)?;
    Ok(metadata)
}

fn now_field(request: &Value) -> Result<u64, String> {
    request
        .get("now_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| "invalid voice archive request".to_owned())
}

fn end_live(state: &mut State, now_ms: u64) -> Result<Option<String>, String> {
    let Some(recording) = state.live.take() else {
        return Ok(None);
    };
    let record = state
        .archive
        .end_capture(recording, now_ms)
        .map_err(archive_error)?;
    Ok(Some(record.id))
}

/// 打开（或换成）`directory` 下的存档。`key` 必须是 32 字节。正在录的录音不受影响。
///
/// # Safety
/// `directory` 与 `key` 必须分别指向本次调用期间可读的 `directory_length`、`key_length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_open(
    directory: *const u8,
    directory_length: usize,
    key: *const u8,
    key_length: usize,
) -> *mut c_char {
    response(|| unsafe {
        let directory = with_bounded_bytes(
            directory,
            directory_length,
            4096,
            "invalid voice archive directory",
            |bytes| {
                parse_absolute_path(
                    bytes,
                    "invalid voice archive directory",
                    "invalid voice archive directory",
                )
                .map(ToOwned::to_owned)
            },
        )?;
        let key: [u8; 32] =
            with_bounded_bytes(key, key_length, 32, "invalid voice archive key", |bytes| {
                bytes
                    .try_into()
                    .map_err(|_| "invalid voice archive key".to_owned())
            })?;
        let archive = Archive::open(directory, &key).map_err(archive_error)?;
        let mut guard = STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (live, last) = guard
            .take()
            .map(|state| (state.live, state.last))
            .unwrap_or_default();
        *guard = Some(State {
            archive,
            live,
            last,
        });
        Ok(json!({"opened": true}))
    })
}

/// 开始一条新录音。请求 `{"now_ms", "app", "provider"}`；上一条还没停时先替它收尾。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_begin(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let request = unsafe { json_request(request, length)? };
        let now_ms = now_field(&request)?;
        let app = string_field(&request, "app")?;
        let provider = string_field(&request, "provider")?;
        with_state(|state| {
            end_live(state, now_ms)?;
            let recording = state
                .archive
                .begin(now_ms, app, provider)
                .map_err(archive_error)?;
            let id = recording.id().to_owned();
            state.live = Some(recording);
            state.last = Some(id.clone());
            Ok(json!({"id": id}))
        })
    })
}

/// 把 16 kHz 单声道浮点样本追加到正在录的录音。没有正在录的录音或写入失败时返回 false。
///
/// # Safety
/// `samples` 必须指向本次调用期间可读的 `count` 个 `f32`。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_append(
    samples: *const f32,
    count: usize,
) -> bool {
    if samples.is_null() || count > MAX_APPEND_SAMPLES {
        return false;
    }
    let samples = unsafe { std::slice::from_raw_parts(samples, count) };
    with_state(|state| {
        let recording = state.live.as_mut().ok_or("voice_archive_idle")?;
        recording.append(samples).map_err(archive_error)
    })
    .is_ok()
}

/// 录音停止：写完剩下的音频并记下时长。没有正在录的录音时 value 的 id 为 null。
#[no_mangle]
pub extern "C" fn msime_client_voice_archive_end_capture(now_ms: u64) -> *mut c_char {
    response(|| with_state(|state| Ok(json!({"id": end_live(state, now_ms)?}))))
}

/// 给一条记录追加结果。请求 `{"kind", "text", "id", ...}`：
/// `kind` 为 `text`（追加一段识别结果，并可选附带 `raw_text`、`mode`、`app_category`、`polish_provider`、
/// `polish_model`、`polish_prompt_id`、`polish_failed`、`polish_rejected_reason`、`asr_ms`、`polish_ms`）、
/// `dismissed`（记下静音/误触/幻觉丢弃原因，取 `text` 或 `dismissed_reason`，长度不超过 128 字节）、
/// `failure`（记下失败原因）、`cancelled`（用户取消）或 `retranscription`（重新听写的结果）；
/// 不给 `id` 时记到最近一次录音上，`retranscription` 必须给 `id`。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_note(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let request = unsafe { json_request_bounded(request, length, MAX_NOTE_REQUEST_BYTES)? };
        let kind = string_field(&request, "kind")?.to_owned();
        let text = string_field(&request, "text")?.to_owned();
        let id = string_field(&request, "id")?.to_owned();
        let text_metadata = if kind == "text" {
            Some(parse_text_metadata(&request)?)
        } else {
            None
        };
        let dismissed_reason = if kind == "dismissed" {
            let fallback = optional_string_field(&request, "dismissed_reason", MAX_REASON_BYTES)?;
            let reason = if !text.is_empty() {
                text.as_str()
            } else {
                fallback.unwrap_or("")
            };
            validate_dismissed_reason(reason).map_err(archive_error)?;
            Some(reason.to_owned())
        } else {
            None
        };
        with_state(|state| {
            let id = if id.is_empty() {
                if kind == "retranscription" {
                    return Err("invalid voice archive request".into());
                }
                state.last.clone().ok_or("voice_archive_idle")?
            } else {
                id
            };
            match kind.as_str() {
                "text" => {
                    let metadata = text_metadata.expect("validated text metadata");
                    if text.is_empty() && metadata.is_empty() {
                        return Ok(json!({"id": id}));
                    }
                    state
                        .archive
                        .note_text_with_metadata(&id, &text, metadata)
                        .map_err(archive_error)?;
                }
                "dismissed" => {
                    let reason = dismissed_reason.expect("validated dismissed reason");
                    state
                        .archive
                        .note_dismissed(&id, &reason)
                        .map_err(archive_error)?;
                }
                "failure" => {
                    state
                        .archive
                        .update(&id, |record| record.failure = Some(text))
                        .map_err(archive_error)?;
                }
                "cancelled" => {
                    state
                        .archive
                        .update(&id, |record| record.cancelled = true)
                        .map_err(archive_error)?;
                }
                "retranscription" => {
                    state
                        .archive
                        .update(&id, |record| record.retranscription = Some(text))
                        .map_err(archive_error)?;
                }
                _ => return Err("invalid voice archive request".into()),
            }
            Ok(json!({"id": id}))
        })
    })
}

/// 全部记录，新的在前。每条是记录字段加上 `status` 和 `has_audio`；`unreadable` 是解不开的记录数。
#[no_mangle]
pub extern "C" fn msime_client_voice_archive_list() -> *mut c_char {
    response(|| {
        with_state(|state| {
            let listing = state.archive.list().map_err(archive_error)?;
            let live = state
                .live
                .as_ref()
                .map(|recording| recording.id().to_owned());
            let entries: Vec<Value> = listing
                .records
                .into_iter()
                .map(|record| {
                    let status = record.status(live.as_deref() == Some(record.id.as_str()));
                    let has_audio = state.archive.has_audio(&record.id);
                    let mut value =
                        serde_json::to_value(&record).expect("a record always serializes");
                    value["status"] = json!(status);
                    value["has_audio"] = json!(has_audio);
                    value
                })
                .collect();
            Ok(json!({"entries": entries, "unreadable": listing.unreadable}))
        })
    })
}

/// 解密后的音频经 `callback` 交给宿主，只调用一次；`format` 为 0 时是 16 kHz 单声道 16 位 WAV，为 1 时是 16 kHz 单声道小端 f32 PCM。
/// 解密结果不落盘；宿主应在回调里复制字节，回调返回后缓冲区即失效。
pub type VoiceArchiveBytesCallback =
    unsafe extern "C" fn(bytes: *const u8, length: usize, context: *mut c_void);

/// # Safety
/// `id` 必须指向本次调用期间可读的 `id_length` 个字节；`callback` 必须可以在调用线程上调用。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_audio(
    id: *const u8,
    id_length: usize,
    format: u32,
    callback: Option<VoiceArchiveBytesCallback>,
    context: *mut c_void,
) -> *mut c_char {
    response(|| unsafe {
        let callback = callback.ok_or("invalid voice archive request")?;
        let id = with_bounded_bytes(id, id_length, 64, "voice_archive_invalid_id", |bytes| {
            std::str::from_utf8(bytes)
                .map(ToOwned::to_owned)
                .map_err(|_| "voice_archive_invalid_id".to_owned())
        })?;
        let bytes = with_state(|state| {
            match format {
                0 => state.archive.wav(&id),
                1 => state.archive.pcm_f32(&id),
                _ => return Err("invalid voice archive request".into()),
            }
            .map_err(archive_error)
        })?;
        callback(bytes.as_ptr(), bytes.len(), context);
        Ok(json!({"bytes": bytes.len()}))
    })
}

/// 删除一条记录和它的音频；正在录的那一条不能删。
///
/// # Safety
/// `id` 必须指向本次调用期间可读的 `id_length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_delete(
    id: *const u8,
    id_length: usize,
) -> *mut c_char {
    response(|| unsafe {
        let id = with_bounded_bytes(id, id_length, 64, "voice_archive_invalid_id", |bytes| {
            std::str::from_utf8(bytes)
                .map(ToOwned::to_owned)
                .map_err(|_| "voice_archive_invalid_id".to_owned())
        })?;
        with_state(|state| {
            if state
                .live
                .as_ref()
                .is_some_and(|recording| recording.id() == id)
            {
                return Err("voice_archive_recording".into());
            }
            state.archive.delete(&id).map_err(archive_error)?;
            if state.last.as_deref() == Some(id.as_str()) {
                state.last = None;
            }
            Ok(json!({"deleted": true}))
        })
    })
}

/// 按保留期限清理。请求 `{"now_ms", "recording_retention_hours", "history_retention_days"}`，取值与共享偏好
/// `voice_input.recording_retention_hours`、`voice_input.history_retention_days` 相同（天数 0 表示记录不限期）。
///
/// # Safety
/// `request` 必须指向本次调用期间可读的 `length` 个字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_voice_archive_purge(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let request = unsafe { json_request(request, length)? };
        let now_ms = now_field(&request)?;
        let hours = request
            .get("recording_retention_hours")
            .and_then(Value::as_u64)
            .and_then(|hours| u32::try_from(hours).ok())
            .filter(|hours| RECORDING_RETENTION_HOURS.contains(hours))
            .ok_or("invalid voice archive request")?;
        let days = request
            .get("history_retention_days")
            .and_then(Value::as_u64)
            .and_then(|days| u32::try_from(days).ok())
            .filter(|days| *days <= MAX_HISTORY_RETENTION_DAYS)
            .ok_or("invalid voice archive request")?;
        let record_ttl = (days > 0).then(|| u64::from(days) * DAY_MS);
        with_state(|state| {
            let keep = state
                .live
                .as_ref()
                .map(|recording| recording.id().to_owned());
            let report = state
                .archive
                .purge(
                    now_ms,
                    Some(u64::from(hours) * HOUR_MS),
                    record_ttl,
                    keep.as_deref(),
                )
                .map_err(archive_error)?;
            Ok(
                json!({"audio_removed": report.audio_removed, "records_removed": report.records_removed}),
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 进程内共享全局 `STATE`，单测按互斥锁串行执行以避免并发互相覆盖。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn clear_state() {
        *STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn open_temp_archive() -> tempfile::TempDir {
        clear_state();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice-archive");
        let path = path.to_str().unwrap();
        let key = [5u8; 32];
        let response = read(unsafe {
            msime_client_voice_archive_open(path.as_ptr(), path.len(), key.as_ptr(), key.len())
        });
        assert_eq!(response["ok"], true, "{response}");
        dir
    }

    fn read(pointer: *mut c_char) -> Value {
        let text = unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { crate::msime_client_string_free(pointer) };
        serde_json::from_str(&text).unwrap()
    }

    fn call(
        function: unsafe extern "C" fn(*const u8, usize) -> *mut c_char,
        request: Value,
    ) -> Value {
        let bytes = request.to_string();
        read(unsafe { function(bytes.as_ptr(), bytes.len()) })
    }

    unsafe extern "C" fn collect(bytes: *const u8, length: usize, context: *mut c_void) {
        let out = unsafe { &mut *context.cast::<Vec<u8>>() };
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
    }

    /// 完整录音与读取生命周期按顺序走一遍。
    #[test]
    fn a_recording_flows_through_the_c_abi() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_state();
        assert_eq!(
            read(msime_client_voice_archive_list())["error"],
            "voice_archive_closed"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice-archive");
        let path = path.to_str().unwrap();
        let short_key = [1u8; 16];
        let response = read(unsafe {
            msime_client_voice_archive_open(
                path.as_ptr(),
                path.len(),
                short_key.as_ptr(),
                short_key.len(),
            )
        });
        assert_eq!(response["error"], "invalid voice archive key");
        let key = [3u8; 32];
        let response = read(unsafe {
            msime_client_voice_archive_open(path.as_ptr(), path.len(), key.as_ptr(), key.len())
        });
        assert_eq!(response["ok"], true, "{response}");

        assert!(!unsafe { msime_client_voice_archive_append([0.1f32].as_ptr(), 1) });
        let begun = call(
            msime_client_voice_archive_begin,
            json!({"now_ms": 5_000, "app": "com.example.editor", "provider": "textstream"}),
        );
        let id = begun["value"]["id"].as_str().unwrap().to_owned();
        let samples = vec![0.5f32; 16_000];
        assert!(unsafe { msime_client_voice_archive_append(samples.as_ptr(), samples.len()) });

        let listing = read(msime_client_voice_archive_list());
        assert_eq!(listing["value"]["entries"][0]["status"], "recording");
        assert_eq!(
            read(msime_client_voice_archive_delete_str(&id))["error"],
            "voice_archive_recording"
        );

        assert_eq!(
            read(msime_client_voice_archive_end_capture(6_000))["value"]["id"],
            id.as_str()
        );
        // 识别结果在录音停止后才到，记到最近一条上。
        call(
            msime_client_voice_archive_note,
            json!({"kind": "text", "text": "第一段"}),
        );
        call(
            msime_client_voice_archive_note,
            json!({"kind": "text", "text": "第二段"}),
        );
        let rejected = call(
            msime_client_voice_archive_note,
            json!({"kind": "retranscription", "text": "x"}),
        );
        assert_eq!(rejected["error"], "invalid voice archive request");
        call(
            msime_client_voice_archive_note,
            json!({"kind": "retranscription", "text": "重听", "id": id}),
        );

        let entry = read(msime_client_voice_archive_list())["value"]["entries"][0].clone();
        assert_eq!(entry["status"], "recognized");
        assert_eq!(entry["segments"], json!(["第一段", "第二段"]));
        assert_eq!(entry["retranscription"], "重听");
        assert_eq!(entry["has_audio"], true);
        assert_eq!(entry["audio_ms"], 1000);
        // 旧版纯文本写入不产生额外可选字段。
        assert!(entry.get("raw_segments").is_none());
        assert!(entry.get("mode").is_none());
        assert!(entry.get("dismissed_reason").is_none());

        let mut wav: Vec<u8> = Vec::new();
        let response = read(unsafe {
            msime_client_voice_archive_audio(
                id.as_ptr(),
                id.len(),
                0,
                Some(collect),
                (&raw mut wav).cast(),
            )
        });
        assert_eq!(response["value"]["bytes"], 44 + 32_000);
        assert_eq!(&wav[..4], b"RIFF");
        let mut pcm: Vec<u8> = Vec::new();
        read(unsafe {
            msime_client_voice_archive_audio(
                id.as_ptr(),
                id.len(),
                1,
                Some(collect),
                (&raw mut pcm).cast(),
            )
        });
        assert_eq!(pcm.len(), 16_000 * 4);

        let invalid = call(
            msime_client_voice_archive_purge,
            json!({"now_ms": 0, "recording_retention_hours": 5, "history_retention_days": 0}),
        );
        assert_eq!(invalid["error"], "invalid voice archive request");
        let purged = call(
            msime_client_voice_archive_purge,
            json!({"now_ms": 5_000 + 25 * HOUR_MS, "recording_retention_hours": 24, "history_retention_days": 0}),
        );
        assert_eq!(
            purged["value"],
            json!({"audio_removed": 1, "records_removed": 0})
        );
        let entry = read(msime_client_voice_archive_list())["value"]["entries"][0].clone();
        assert_eq!(entry["has_audio"], false);
        assert_eq!(entry["segments"], json!(["第一段", "第二段"]));

        assert_eq!(
            read(msime_client_voice_archive_delete_str(&id))["value"]["deleted"],
            true
        );
        assert_eq!(
            read(msime_client_voice_archive_list())["value"]["entries"],
            json!([])
        );
        clear_state();
    }

    #[test]
    fn text_note_with_metadata_round_trips_through_list() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _dir = open_temp_archive();
        let begun = call(
            msime_client_voice_archive_begin,
            json!({"now_ms": 15_000, "app": "com.example.terminal", "provider": "textstream"}),
        );
        let id = begun["value"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            read(msime_client_voice_archive_end_capture(16_200))["ok"],
            true
        );

        let noted = call(
            msime_client_voice_archive_note,
            json!({
                "kind": "text",
                "id": id,
                "text": "查看 API 状态",
                "raw_text": "嗯查看api状态",
                "mode": "dictation",
                "app_category": "terminal_ide",
                "polish_provider": "local",
                "polish_model": "synthetic-qwen-9b",
                "polish_prompt_id": "cleanup",
                "polish_failed": false,
                "asr_ms": 280,
                "polish_ms": 740
            }),
        );
        assert_eq!(noted["ok"], true, "{noted}");

        // 追加第二段并记录一次润色降级原因，同时省略此前已写入的 mode / app_category。
        let fallback = call(
            msime_client_voice_archive_note,
            json!({
                "kind": "text",
                "id": id,
                "text": "再检查日志",
                "raw_text": "再检查日志",
                "polish_failed": true,
                "polish_rejected_reason": "timeout",
                "polish_ms": 8_100
            }),
        );
        assert_eq!(fallback["ok"], true, "{fallback}");

        let entry = read(msime_client_voice_archive_list())["value"]["entries"][0].clone();
        assert_eq!(entry["status"], "recognized");
        assert_eq!(entry["segments"], json!(["查看 API 状态", "再检查日志"]));
        assert_eq!(
            entry["raw_segments"],
            json!(["嗯查看api状态", "再检查日志"])
        );
        assert_eq!(entry["mode"], "dictation");
        assert_eq!(entry["app_category"], "terminal_ide");
        assert_eq!(entry["polish_provider"], "local");
        assert_eq!(entry["polish_model"], "synthetic-qwen-9b");
        assert_eq!(entry["polish_prompt_id"], "cleanup");
        assert_eq!(entry["polish_failed"], true);
        assert_eq!(entry["polish_rejected_reason"], "timeout");
        assert_eq!(entry["asr_ms"], 280);
        assert_eq!(entry["polish_ms"], 8_100);
        clear_state();
    }

    #[test]
    fn dismissed_note_records_reason_and_keeps_no_result_status() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _dir = open_temp_archive();
        let begun = call(
            msime_client_voice_archive_begin,
            json!({"now_ms": 25_000, "app": "com.example.chat", "provider": "textstream"}),
        );
        let id = begun["value"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            read(msime_client_voice_archive_end_capture(25_600))["ok"],
            true
        );

        // 不传 id 时记到最近一条录音上。
        let noted = call(
            msime_client_voice_archive_note,
            json!({"kind": "dismissed", "text": "filler_only"}),
        );
        assert_eq!(noted["ok"], true, "{noted}");
        assert_eq!(noted["value"]["id"], id.as_str());

        let entry = read(msime_client_voice_archive_list())["value"]["entries"][0].clone();
        assert_eq!(entry["status"], "no_result");
        assert_eq!(entry["dismissed_reason"], "filler_only");
        assert_eq!(entry["segments"], json!([]));

        // 也兼容通过 `dismissed_reason` 字段传入原因码。
        let updated = call(
            msime_client_voice_archive_note,
            json!({"kind": "dismissed", "id": id, "dismissed_reason": "silence_hallucination"}),
        );
        assert_eq!(updated["ok"], true, "{updated}");
        let entry = read(msime_client_voice_archive_list())["value"]["entries"][0].clone();
        assert_eq!(entry["dismissed_reason"], "silence_hallucination");
        assert_eq!(entry["status"], "no_result");
        clear_state();
    }

    #[test]
    fn note_rejects_oversized_or_malformed_metadata_and_dismissed_reason() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _dir = open_temp_archive();
        let begun = call(
            msime_client_voice_archive_begin,
            json!({"now_ms": 35_000, "app": "com.example.doc", "provider": "textstream"}),
        );
        let id = begun["value"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            read(msime_client_voice_archive_end_capture(36_000))["ok"],
            true
        );

        let over_128 = "r".repeat(MAX_REASON_BYTES + 1);
        let over_512 = "m".repeat(MAX_MODEL_NAME_BYTES + 1);
        for payload in [
            json!({"kind": "dismissed", "id": id, "text": ""}),
            json!({"kind": "dismissed", "id": id, "text": "   "}),
            json!({"kind": "dismissed", "id": id, "text": over_128}),
            json!({"kind": "dismissed", "id": id, "text": "line\nbreak"}),
            json!({"kind": "dismissed", "id": id, "dismissed_reason": 123}),
            json!({"kind": "text", "id": id, "text": "合法", "mode": over_128}),
            json!({"kind": "text", "id": id, "text": "合法", "polish_model": over_512}),
            json!({"kind": "text", "id": id, "text": "合法", "app_category": "im\r"}),
            json!({"kind": "text", "id": id, "text": "合法", "polish_failed": "yes"}),
            json!({"kind": "text", "id": id, "text": "合法", "asr_ms": -1}),
            json!({"kind": "text", "id": id, "text": "合法", "polish_ms": "100"}),
            json!({"kind": "text", "id": id, "text": "合法", "raw_text": 42}),
            json!({"kind": "unknown", "id": id, "text": "合法"}),
        ] {
            let response = call(msime_client_voice_archive_note, payload.clone());
            assert_eq!(
                response["error"], "invalid voice archive request",
                "预期拒绝非法载荷：{payload}"
            );
        }

        // 恰好 512 字节的 polish_model 与恰好 128 字节的丢弃原因应放行。
        let exact_512 = "m".repeat(MAX_MODEL_NAME_BYTES);
        let ok_model = call(
            msime_client_voice_archive_note,
            json!({"kind": "text", "id": id, "text": "模型上限测试", "polish_model": exact_512}),
        );
        assert_eq!(ok_model["ok"], true, "{ok_model}");
        let exact_128 = "d".repeat(MAX_REASON_BYTES);
        let ok = call(
            msime_client_voice_archive_note,
            json!({"kind": "dismissed", "id": id, "text": exact_128}),
        );
        assert_eq!(ok["ok"], true, "{ok}");
        clear_state();
    }

    fn msime_client_voice_archive_delete_str(id: &str) -> *mut c_char {
        unsafe { msime_client_voice_archive_delete(id.as_ptr(), id.len()) }
    }
}
