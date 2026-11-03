//! 语音存档的 C ABI：录音一边进行一边加密落盘，听写结果、失败和取消追加到同一条记录里。
//!
//! 进程里只有一个存档和至多一条正在录的录音，状态放在全局锁里：音频从宿主的采集队列追加，其余操作在主线程。
//! 存档格式与加密见 `msime_client_providers::voice::archive`；密钥由宿主保管并在打开时交进来。

use crate::*;
use msime_client_core::preferences::{MAX_HISTORY_RETENTION_DAYS, RECORDING_RETENTION_HOURS};
use msime_client_providers::voice::archive::{Archive, ArchiveError, Recording};
use std::ffi::c_void;
use std::sync::Mutex;

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
/// 一次追加最多十秒音频；采集回调每次只交几十毫秒。
const MAX_APPEND_SAMPLES: usize = 160_000;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

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
        ArchiveError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "voice_archive_not_found"
        }
        ArchiveError::Io(_) => "voice_archive_io",
    }
    .into()
}

unsafe fn json_request(pointer: *const u8, length: usize) -> Result<Value, String> {
    unsafe {
        with_bounded_bytes(
            pointer,
            length,
            MAX_TEXT_BYTES + 4096,
            "invalid voice archive request",
            |bytes| {
                serde_json::from_slice(bytes)
                    .map_err(|_| "invalid voice archive request".to_owned())
            },
        )
    }
}

fn string_field<'a>(request: &'a Value, field: &str) -> Result<&'a str, String> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(value)) if value.len() <= MAX_TEXT_BYTES => Ok(value),
        _ => Err("invalid voice archive request".into()),
    }
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

/// 给一条记录追加结果。请求 `{"kind", "text", "id"}`：
/// `kind` 为 `text`（追加一段识别结果）、`failure`（记下失败原因）、`cancelled`（用户取消）或 `retranscription`（重新听写的结果）；
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
        let request = unsafe { json_request(request, length)? };
        let kind = string_field(&request, "kind")?.to_owned();
        let text = string_field(&request, "text")?.to_owned();
        let id = string_field(&request, "id")?.to_owned();
        with_state(|state| {
            let id = if id.is_empty() {
                if kind == "retranscription" {
                    return Err("invalid voice archive request".into());
                }
                state.last.clone().ok_or("voice_archive_idle")?
            } else {
                id
            };
            let change: Box<dyn FnOnce(&mut msime_client_providers::voice::archive::Record)> =
                match kind.as_str() {
                    "text" if !text.is_empty() => {
                        Box::new(move |record| record.segments.push(text))
                    }
                    "failure" => Box::new(move |record| record.failure = Some(text)),
                    "cancelled" => Box::new(|record| record.cancelled = true),
                    "retranscription" => {
                        Box::new(move |record| record.retranscription = Some(text))
                    }
                    "text" => return Ok(json!({"id": id})),
                    _ => return Err("invalid voice archive request".into()),
                };
            state.archive.update(&id, change).map_err(archive_error)?;
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

    /// 全局状态只有一份，整条生命周期放在一个测试里按顺序走。
    #[test]
    fn a_recording_flows_through_the_c_abi() {
        *STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
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
        *STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn msime_client_voice_archive_delete_str(id: &str) -> *mut c_char {
        unsafe { msime_client_voice_archive_delete(id.as_ptr(), id.len()) }
    }
}
