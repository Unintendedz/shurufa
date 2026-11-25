//! Shurufa v2 Tier 1 两阶段异步神经解码的增量 C ABI 入口。
//!
//! 只做参数边界校验、调用 `Runtime` 编排方法并序列化 JSON 响应，不含任何融合或排序数学。

use crate::*;

const MAX_NEURAL_RESPONSE_BYTES: usize = 256 * 1024;
const FOLLOWING_CONTEXT_LIMIT: usize = 4_096;

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
/// 开启时自动解除旧版 `settled_lock_leader` 停顿锁首策略，改由 22 ms 延迟绘制与 80 ms 槽 0 锁控制防闪烁。
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

/// 读取当前会话完整保留标点/空格的 512 字上文，供宿主在空闲 50 ms 后预热端侧 KV-Cache。
#[no_mangle]
pub extern "C" fn msime_client_neural_prefill_context(handle: u64) -> *mut c_char {
    response(|| {
        with_session(handle, |session| {
            Ok(json!({ "context": session.runtime.neural_prefill_context() }))
        })
    })
}

/// 将 Tier 1 神经服务或离线后端返回的 `NeuralDecodeResponse` JSON 交给纯 Rust 引擎执行统一概率融合。
/// - `lock_slot_0 == true` 表示响应超过 80 ms 迟到，严格锁定槽 0 不跳动，仅升级槽 1 及以后；
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
