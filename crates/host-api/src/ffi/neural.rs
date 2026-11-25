//! Shurufa v2 Tier 1 两阶段异步神经解码与纯 Rust UDS 客户端的 C ABI 入口。
//!
//! 负责管理共享后台 `NeuralServiceClient` 生命周期、防抖/防闪唤醒回调路由、两阶段锁槽状态机轮询，
//! 并保留底层请求/融合校验接口供集成测试使用。

use crate::*;
use msime_input_runtime::{
    NeuralClientConfig, NeuralServiceClient, NeuralWakeupFn, PresentationPhase,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_NEURAL_CONFIG_BYTES: usize = 64 * 1024;
const MAX_NEURAL_RESPONSE_BYTES: usize = 256 * 1024;
const FOLLOWING_CONTEXT_LIMIT: usize = 4_096;

/// 宿主传入的神经服务唤醒回调函数签名（可能从后台线程触发，宿主须转回会话主线程轮询）。
pub type MsimeNeuralWakeupFn = extern "C" fn(*mut c_void, u64, u64);

#[derive(Clone, Copy)]
struct WakeupTarget {
    func: MsimeNeuralWakeupFn,
    opaque: usize,
}

static NEURAL_WAKEUPS: OnceLock<Mutex<HashMap<u64, WakeupTarget>>> = OnceLock::new();

#[derive(Clone, PartialEq, Eq)]
struct SharedClientKey {
    helper_path: Option<PathBuf>,
    model_directory: Option<PathBuf>,
    pinyin_db_path: Option<PathBuf>,
    extra_launch_arguments: Vec<String>,
    require_model_file: bool,
}

type SharedNeuralSlot = OnceLock<Mutex<Option<(SharedClientKey, Arc<NeuralServiceClient>)>>>;

static SHARED_STRONG_DEFAULT_CLIENT: SharedNeuralSlot = OnceLock::new();

fn wakeup_map() -> &'static Mutex<HashMap<u64, WakeupTarget>> {
    NEURAL_WAKEUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dispatch_wakeup(session_id: u64, generation: u64) {
    let target = {
        let guard = wakeup_map()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get(&session_id).copied()
    };
    if let Some(target) = target {
        (target.func)(target.opaque as *mut c_void, session_id, generation);
    }
}

pub(crate) fn register_wakeup(
    session_id: u64,
    wakeup: Option<MsimeNeuralWakeupFn>,
    opaque: *mut c_void,
) {
    let mut guard = wakeup_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match wakeup {
        Some(func) => {
            guard.insert(
                session_id,
                WakeupTarget {
                    func,
                    opaque: opaque as usize,
                },
            );
        }
        None => {
            guard.remove(&session_id);
        }
    }
}

pub(crate) fn unregister_wakeup(session_id: u64) {
    let mut guard = wakeup_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.remove(&session_id);
}

/// 在输入法退出或内存压力释放过程级缓存时，有序关闭进程内共享神经子进程。
pub(crate) fn shutdown_shared_neural_client() {
    if let Some(slot) = SHARED_STRONG_DEFAULT_CLIENT.get() {
        let client = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .map(|(_, client)| client);
        if let Some(client) = client {
            client.stop();
        }
    }
}

fn shared_or_create_client(config: NeuralClientConfig) -> Arc<NeuralServiceClient> {
    let wakeup: NeuralWakeupFn = Arc::new(dispatch_wakeup);
    // 显式指定了 `socket_path`（测试隔离实例）时独占独立客户端，不进入进程共享单实例。
    if config.socket_path.is_some() {
        return NeuralServiceClient::new(config, Some(wakeup));
    }

    let key = SharedClientKey {
        helper_path: config.helper_path.clone(),
        model_directory: config.model_directory.clone(),
        pinyin_db_path: config.pinyin_db_path.clone(),
        extra_launch_arguments: config.extra_launch_arguments.clone(),
        require_model_file: config.require_model_file,
    };

    let slot = SHARED_STRONG_DEFAULT_CLIENT.get_or_init(|| Mutex::new(None));
    let mut previous_to_stop: Option<Arc<NeuralServiceClient>> = None;
    let client = {
        let mut guard = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((existing_key, existing)) = guard.as_ref() {
            if *existing_key == key {
                existing.set_resources_directory(config.resources_directory.clone());
                return Arc::clone(existing);
            }
        }
        if let Some((_, old)) = guard.take() {
            previous_to_stop = Some(old);
        }
        let created = NeuralServiceClient::new(config, Some(wakeup));
        *guard = Some((key, Arc::clone(&created)));
        created
    };
    if let Some(old) = previous_to_stop {
        old.stop();
    }
    client
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConfigureNeuralRequest {
    helper_path: Option<String>,
    model_directory: Option<String>,
    pinyin_db_path: Option<String>,
    resources_directory: Option<String>,
    socket_path: Option<String>,
    extra_launch_arguments: Option<Vec<String>>,
    idle_unload_seconds: Option<f64>,
    request_timeout_ms: Option<u64>,
    require_model_file: Option<bool>,
    enabled: Option<bool>,
}

/// 配置并启用或关闭 Shurufa 端侧神经服务（由纯 Rust 运行时管理子进程、UDS 二进制帧、
/// 50 ms 上文 Prefill 防抖、~28 ms Provisional 防闪窗及热态 Space 有界等待）。
///
/// # Safety
/// `length` 不为 0 时，`config_json` 必须指向至少 `length` 字节的可读 UTF-8 JSON。
/// `wakeup` 可为空；若非空则需在会话销毁前保持可安全跨线程调用（宿主应转回主线程后再轮询）。
#[no_mangle]
pub unsafe extern "C" fn msime_client_configure_neural_service(
    handle: u64,
    config_json: *const u8,
    length: usize,
    wakeup: Option<MsimeNeuralWakeupFn>,
    opaque: *mut c_void,
) -> *mut c_char {
    response(|| {
        if length > MAX_NEURAL_CONFIG_BYTES || (length != 0 && config_json.is_null()) {
            return Err("invalid neural configuration buffer".into());
        }
        let parsed: ConfigureNeuralRequest = if length == 0 {
            ConfigureNeuralRequest::default()
        } else {
            // SAFETY: 已在上方校验非空指针与长度上限。
            let bytes = unsafe { std::slice::from_raw_parts(config_json, length) };
            serde_json::from_slice(bytes)
                .map_err(|error| format!("invalid neural configuration JSON: {error}"))?
        };
        with_session(handle, |session| {
            let want_enabled = parsed.enabled.unwrap_or(true) && session.edition.id == "shurufa";
            let session_id = session.runtime.session_id();
            if !want_enabled {
                session.runtime.set_neural_v2_enabled(false);
                if let Some(client) = session.neural_client.take() {
                    client.cancel_pending_decode_for_session(session_id);
                }
                session.last_neural_prefilled_context = None;
                unregister_wakeup(session_id);
                return Ok(json!({
                    "available": false,
                    "enabled": false,
                    "pid": 0,
                }));
            }

            let config = NeuralClientConfig {
                helper_path: parsed
                    .helper_path
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                model_directory: parsed
                    .model_directory
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                pinyin_db_path: parsed
                    .pinyin_db_path
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                resources_directory: parsed
                    .resources_directory
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        (!session.options.resources.is_empty())
                            .then(|| PathBuf::from(&session.options.resources))
                    }),
                socket_path: parsed
                    .socket_path
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                extra_launch_arguments: parsed.extra_launch_arguments.unwrap_or_default(),
                idle_unload_seconds: parsed.idle_unload_seconds.unwrap_or(600.0),
                request_timeout: Duration::from_millis(
                    parsed.request_timeout_ms.unwrap_or(250).max(50),
                ),
                require_model_file: parsed.require_model_file.unwrap_or(true),
            };

            register_wakeup(session_id, wakeup, opaque);
            let client = shared_or_create_client(config);
            let available = client.is_available();
            let pid = client.service_pid();
            session.neural_client = Some(client);
            session.runtime.set_neural_v2_enabled(available);
            session.last_neural_prefilled_context = None;
            session.schedule_neural_prefill_if_idle();
            Ok(json!({
                "available": available,
                "enabled": available,
                "pid": pid,
            }))
        })
    })
}

/// 主线程收到唤醒回调后轮询：完成两阶段防闪与槽 0 锁状态推进，并视需要返回最新 `View`。
#[no_mangle]
pub extern "C" fn msime_client_poll_neural(handle: u64) -> *mut c_char {
    response(|| {
        with_session(handle, |session| {
            let gen = session.runtime.generation();
            let session_id = session.runtime.session_id();
            let pid = session
                .neural_client
                .as_ref()
                .map_or(0, |client| client.service_pid());
            if !session.neural_v2_active() || session.runtime.is_idle() {
                session.runtime.flush_provisional();
                return Ok(json!({
                    "ready": false,
                    "moved": false,
                    "provisional": false,
                    "pid": pid,
                }));
            }

            let completed = session
                .neural_client
                .as_ref()
                .and_then(|client| client.take_completed_decode(session_id, gen));
            let phase = session.runtime.presentation_phase();
            let was_provisional = matches!(
                phase,
                PresentationPhase::Provisional { generation: g, .. } if g == gen
            );

            match completed {
                Some(Some(parsed)) => {
                    let lock_slot_0 = !was_provisional;
                    let pending = session.lifecycle_begin(lifecycle::Call::SideChannel);
                    let outcome = session.runtime.apply_neural_response(&parsed, lock_slot_0);
                    let pid = session
                        .neural_client
                        .as_ref()
                        .map_or(0, |client| client.service_pid());
                    if outcome.moved || was_provisional {
                        let view = session.runtime.view();
                        session.lifecycle_view(pending, &view);
                        Ok(json!({
                            "ready": true,
                            "applied": outcome.applied,
                            "moved": outcome.moved,
                            "generation": outcome.generation,
                            "slot0_locked": outcome.slot0_locked,
                            "tier1_rescued": outcome.tier1_rescued,
                            "provisional": false,
                            "pid": pid,
                            "view": view,
                        }))
                    } else {
                        Ok(json!({
                            "ready": false,
                            "applied": outcome.applied,
                            "moved": false,
                            "generation": outcome.generation,
                            "slot0_locked": outcome.slot0_locked,
                            "tier1_rescued": outcome.tier1_rescued,
                            "provisional": false,
                            "pid": pid,
                        }))
                    }
                }
                Some(None) => {
                    if was_provisional {
                        session.runtime.mark_presented(gen);
                        Ok(json!({
                            "ready": true,
                            "applied": false,
                            "moved": false,
                            "provisional": false,
                            "pid": pid,
                            "view": session.runtime.view(),
                        }))
                    } else {
                        Ok(json!({
                            "ready": false,
                            "applied": false,
                            "moved": false,
                            "provisional": false,
                            "pid": pid,
                        }))
                    }
                }
                None => {
                    if let PresentationPhase::Provisional {
                        generation: g,
                        deadline,
                    } = phase
                    {
                        if g == gen && Instant::now() + Duration::from_millis(1) >= deadline {
                            session.runtime.mark_presented(gen);
                            return Ok(json!({
                                "ready": true,
                                "applied": false,
                                "moved": false,
                                "provisional": false,
                                "pid": pid,
                                "view": session.runtime.view(),
                            }));
                        }
                    }
                    Ok(json!({
                        "ready": false,
                        "applied": false,
                        "moved": false,
                        "provisional": session.runtime.is_provisional(),
                        "pid": pid,
                    }))
                }
            }
        })
    })
}

/// 在 ~28 ms Provisional 防闪窗内提前冲刷并展示当前 Tier 0 候选（如非字母导航键或面板展开）。
#[no_mangle]
pub extern "C" fn msime_client_flush_provisional_neural(handle: u64) -> *mut c_char {
    response(|| {
        with_session(handle, |session| {
            let flushed = session.runtime.flush_provisional();
            if flushed {
                Ok(json!({
                    "flushed": true,
                    "provisional": false,
                    "view": session.runtime.view(),
                }))
            } else {
                Ok(json!({
                    "flushed": false,
                    "provisional": session.runtime.is_provisional(),
                }))
            }
        })
    })
}

/// 组字开始前同步光标右侧同行现成后文（供句中补字双向消歧，最多 4096 字节 UTF-8）；
/// 隐私会话、安全输入字段清空上下文并返回 `false`；正在组字时返回 `false` 不改状态。
///
/// # Safety
/// `length` 不为 0 时，`context` 必须指向至少 `length` 个可读字节。
#[no_mangle]
pub unsafe extern "C" fn msime_client_set_following_context(
    handle: u64,
    context: *const u8,
    length: usize,
) -> *mut c_char {
    if length > FOLLOWING_CONTEXT_LIMIT || (length != 0 && context.is_null()) {
        return response(|| Err("invalid following context".into()));
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(context, length) }
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return response(|| Err("invalid following context".into()));
    };
    response(|| {
        with_session(handle, |session| {
            let private_traits = session.lifecycle.as_deref().is_some_and(|state| {
                state.context().traits & (lifecycle::TRAIT_SECURE | lifecycle::TRAIT_PRIVATE) != 0
            });
            if session.statistics_private || private_traits {
                session.runtime.clear_context();
                return Ok(Value::Bool(false));
            }
            if session.runtime.has_composition() {
                return Ok(Value::Bool(false));
            }
            session.runtime.set_following_context(text);
            Ok(Value::Bool(true))
        })
    })
}

/// 启用或关闭会话的 Tier 1 两阶段异步神经解码模式。
#[no_mangle]
pub extern "C" fn msime_client_set_neural_v2_enabled(handle: u64, enabled: bool) -> *mut c_char {
    response(|| {
        with_session(handle, |session| {
            session.runtime.set_neural_v2_enabled(enabled);
            Ok(json!({ "enabled": session.runtime.neural_v2_enabled() }))
        })
    })
}

/// 取出当前代次的 Tier 1 神经解码请求（含完整保留标点/空格的 512 字上文、音节 DAG 路径与 Top-32 候选）。
/// 若当前空闲、未启用或不属于中文拼音组字态，返回 `{"ok": true, "value": null}`。
#[no_mangle]
pub extern "C" fn msime_client_take_neural_request(handle: u64) -> *mut c_char {
    response(|| {
        with_session(handle, |session| {
            let request = session.runtime.take_neural_request();
            serde_json::to_value(request).map_err(|error| error.to_string())
        })
    })
}

/// 读取当前会话完整保留标点/空格的 512 字上文，供预热端侧 KV-Cache。
#[no_mangle]
pub extern "C" fn msime_client_neural_prefill_context(handle: u64) -> *mut c_char {
    response(|| {
        with_session(handle, |session| {
            Ok(json!({ "context": session.runtime.neural_prefill_context() }))
        })
    })
}

/// 将 Tier 1 神经服务或离线后端返回的 `NeuralDecodeResponse` JSON 交给纯 Rust 引擎执行统一概率融合。
/// - `lock_slot_0 == true` 表示进入 `Presented` 后迟到，严格锁定槽 0 不跳动，仅升级槽 1 及以后；
/// - 若 `generation` 已过期，安全丢弃并返回 `applied: false, moved: false`。
///
/// # Safety
/// `response_json` 须指向至少 `length` 字节的可读 UTF-8 JSON。
#[no_mangle]
pub unsafe extern "C" fn msime_client_apply_neural_response(
    handle: u64,
    response_json: *const u8,
    length: usize,
    lock_slot_0: bool,
) -> *mut c_char {
    response(|| {
        let parsed: msime_engine::neural_v2::NeuralDecodeResponse = unsafe {
            super::with_bounded_bytes(
                response_json,
                length,
                MAX_NEURAL_RESPONSE_BYTES,
                "invalid neural response buffer",
                |bytes| {
                    serde_json::from_slice(bytes)
                        .map_err(|error| format!("invalid neural response JSON: {error}"))
                },
            )
        }?;
        with_session(handle, |session| {
            let pending = session.lifecycle_begin(lifecycle::Call::SideChannel);
            let outcome = session.runtime.apply_neural_response(&parsed, lock_slot_0);
            if outcome.moved {
                let view = session.runtime.view();
                session.lifecycle_view(pending, &view);
                Ok(json!({
                    "applied": outcome.applied,
                    "moved": true,
                    "generation": outcome.generation,
                    "slot0_locked": outcome.slot0_locked,
                    "tier1_rescued": outcome.tier1_rescued,
                    "view": view,
                }))
            } else {
                Ok(json!({
                    "applied": outcome.applied,
                    "moved": false,
                    "generation": outcome.generation,
                    "slot0_locked": outcome.slot0_locked,
                    "tier1_rescued": outcome.tier1_rescued,
                }))
            }
        })
    })
}
