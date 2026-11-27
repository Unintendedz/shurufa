//! 纯 Rust 端侧神经解码服务客户端。
//!
//! 负责按 `SHRN` v1 二进制帧协议（16 字节大端头 + <= 512 KiB UTF-8 JSON 载荷）
//! 通过 Unix 域套接字与独立后台子进程 `ShurufaNeuralService` 异步通信，并内置：
//! - 缺失模型文件时静默降级（零子进程、零后台线程）；
//! - 最新代次解码请求合并（Decode 严格优先于空闲 Prefill）；
//! - 独立定时器线程驱动 `~28 ms` Provisional 防闪窗唤醒与 `50 ms` 上文 Prefill 防抖；
//! - 暖态快速敲击空格时的 `Condvar` 有界同步等待（未暖机 0 ms 直接跳过）；
//! - 后台子进程崩溃 / EOF 检测与指数退避（1s..30s，连续稳定 60s 清零）自动自愈重启。

use msime_engine::neural_v2::{NeuralDecodeRequest, NeuralDecodeResponse};
use serde_json::Value;
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const NEURAL_FRAME_MAGIC: u32 = 0x5348_524E; // 'SHRN'
pub const NEURAL_PROTOCOL_VERSION: u8 = 1;
pub const NEURAL_FRAME_HEADER_BYTES: usize = 16;
pub const NEURAL_MAX_FRAME_PAYLOAD_BYTES: usize = 512 * 1024;
pub const NEURAL_FLAG_OK: u16 = 1 << 0;

/// 无感防闪延迟绘制窗口（约 `~70 ms`，覆盖 16 候选前缀树 Stage A 与短句受限束搜索 Stage B 端侧热推理耗时）。
pub const NEURAL_PROVISIONAL_WINDOW: Duration = Duration::from_millis(70);
/// 空闲上文 Prefill 防抖延迟。
pub const NEURAL_PREFILL_DEBOUNCE: Duration = Duration::from_millis(50);

const AVAILABILITY_CACHE_TTL: Duration = Duration::from_secs(5);
const COLD_LOAD_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_FAST_SPACE_WAIT: Duration = Duration::from_millis(55);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const HEALTHY_UPTIME_RESET: Duration = Duration::from_secs(60);

static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeuralOpCode {
    Health = 1,
    Prefill = 2,
    Decode = 3,
    Unload = 4,
    Shutdown = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeuralFrameHeader {
    pub magic: u32,
    pub version: u8,
    pub op: u8,
    pub flags: u16,
    pub payload_len: u32,
    pub request_id: u32,
}

impl NeuralFrameHeader {
    pub fn encode(self) -> [u8; NEURAL_FRAME_HEADER_BYTES] {
        let mut out = [0u8; NEURAL_FRAME_HEADER_BYTES];
        out[0..4].copy_from_slice(&self.magic.to_be_bytes());
        out[4] = self.version;
        out[5] = self.op;
        out[6..8].copy_from_slice(&self.flags.to_be_bytes());
        out[8..12].copy_from_slice(&self.payload_len.to_be_bytes());
        out[12..16].copy_from_slice(&self.request_id.to_be_bytes());
        out
    }

    pub fn decode(raw: &[u8; NEURAL_FRAME_HEADER_BYTES]) -> Option<Self> {
        let header = Self {
            magic: u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]),
            version: raw[4],
            op: raw[5],
            flags: u16::from_be_bytes([raw[6], raw[7]]),
            payload_len: u32::from_be_bytes([raw[8], raw[9], raw[10], raw[11]]),
            request_id: u32::from_be_bytes([raw[12], raw[13], raw[14], raw[15]]),
        };
        if header.magic == NEURAL_FRAME_MAGIC
            && header.version == NEURAL_PROTOCOL_VERSION
            && (header.payload_len as usize) <= NEURAL_MAX_FRAME_PAYLOAD_BYTES
        {
            Some(header)
        } else {
            None
        }
    }
}

/// 唤醒宿主主线程回调（参数：`(session_id, generation)`）。
pub type NeuralWakeupFn = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// 端侧神经服务客户端配置。
#[derive(Clone, Debug, PartialEq)]
pub struct NeuralClientConfig {
    pub helper_path: Option<PathBuf>,
    pub model_directory: Option<PathBuf>,
    pub pinyin_db_path: Option<PathBuf>,
    pub resources_directory: Option<PathBuf>,
    pub socket_path: Option<PathBuf>,
    pub extra_launch_arguments: Vec<String>,
    pub idle_unload_seconds: f64,
    pub request_timeout: Duration,
    pub require_model_file: bool,
}

impl Default for NeuralClientConfig {
    fn default() -> Self {
        Self {
            helper_path: None,
            model_directory: None,
            pinyin_db_path: None,
            resources_directory: None,
            socket_path: None,
            extra_launch_arguments: Vec::new(),
            idle_unload_seconds: 600.0,
            request_timeout: Duration::from_millis(250),
            require_model_file: true,
        }
    }
}

#[derive(Clone, Debug)]
struct PendingDecode {
    session_id: u64,
    generation: u64,
    payload_json: Vec<u8>,
}

#[derive(Clone, Debug)]
struct CompletedDecode {
    session_id: u64,
    generation: u64,
    response: Option<NeuralDecodeResponse>,
}

struct SharedQueue {
    config: NeuralClientConfig,
    availability_cache: Option<(Instant, bool)>,
    pending_decode: Option<PendingDecode>,
    latest_decode_gen_by_session: Option<(u64, u64)>,
    pending_prefill: Option<String>,
    pending_health: Vec<mpsc::Sender<Option<Value>>>,
    provisional_deadline: Option<(u64, u64, Instant)>,
    debounced_prefill: Option<(Instant, String)>,
    worker_started: bool,
    stopped: bool,
}

struct WaitState {
    completed: Option<CompletedDecode>,
}

struct ServiceProcess {
    child: Option<Child>,
    stream: Option<UnixStream>,
    next_request_id: u32,
    service_warmed: bool,
    service_started_at: Option<Instant>,
    consecutive_failures: u32,
    backoff_until: Option<Instant>,
}

impl ServiceProcess {
    fn new() -> Self {
        Self {
            child: None,
            stream: None,
            next_request_id: 0,
            service_warmed: false,
            service_started_at: None,
            consecutive_failures: 0,
            backoff_until: None,
        }
    }
}

/// 纯 Rust 端侧神经解码异步服务客户端。
pub struct NeuralServiceClient {
    socket_path: PathBuf,
    queue: Arc<(Mutex<SharedQueue>, Condvar, Condvar)>,
    wait: Arc<(Mutex<WaitState>, Condvar)>,
    atomic_warmed: Arc<AtomicBool>,
    atomic_pid: Arc<AtomicU32>,
    wakeup: Option<NeuralWakeupFn>,
    worker_handles: Mutex<Option<(JoinHandle<()>, JoinHandle<()>)>>,
}

impl NeuralServiceClient {
    /// 构建客户端；若缺失模型文件则在调用前保持零线程、零子进程。
    pub fn new(config: NeuralClientConfig, wakeup: Option<NeuralWakeupFn>) -> Arc<Self> {
        let tag = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        let socket_path = config
            .socket_path
            .clone()
            .unwrap_or_else(|| allocate_private_socket_path(tag));
        let queue_state = SharedQueue {
            config,
            availability_cache: None,
            pending_decode: None,
            latest_decode_gen_by_session: None,
            pending_prefill: None,
            pending_health: Vec::new(),
            provisional_deadline: None,
            debounced_prefill: None,
            worker_started: false,
            stopped: false,
        };
        Arc::new(Self {
            socket_path,
            queue: Arc::new((Mutex::new(queue_state), Condvar::new(), Condvar::new())),
            wait: Arc::new((Mutex::new(WaitState { completed: None }), Condvar::new())),
            atomic_warmed: Arc::new(AtomicBool::new(false)),
            atomic_pid: Arc::new(AtomicU32::new(0)),
            wakeup,
            worker_handles: Mutex::new(None),
        })
    }

    pub fn service_pid(&self) -> u32 {
        self.atomic_pid.load(Ordering::Relaxed)
    }

    pub fn is_warmed(&self) -> bool {
        self.atomic_warmed.load(Ordering::Relaxed)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn set_resources_directory(&self, resources: Option<PathBuf>) {
        let mut queue = self
            .queue
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.config.resources_directory != resources {
            queue.config.resources_directory = resources;
            queue.availability_cache = None;
        }
    }

    /// 检查辅助进程与 GGUF 模型是否就绪（5 秒缓存，受 `SHURUFA_DISABLE_NEURAL_V2=1` 关断）。
    pub fn is_available(&self) -> bool {
        if std::env::var_os("SHURUFA_DISABLE_NEURAL_V2").is_some_and(|v| v == "1") {
            return false;
        }
        let now = Instant::now();
        let config_snapshot = {
            let queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return false;
            }
            if let Some((cached_at, ready)) = queue.availability_cache {
                if now.saturating_duration_since(cached_at) < AVAILABILITY_CACHE_TTL {
                    return ready;
                }
            }
            queue.config.clone()
        };

        let ready = check_config_available(&config_snapshot);
        let mut queue = self
            .queue
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.availability_cache = Some((now, ready));
        ready
    }

    /// 入队一次即时上文 Prefill；服务或模型不可用时静默忽略。
    pub fn prefill_context(self: &Arc<Self>, context: &str) {
        if !self.is_available() {
            return;
        }
        let mut start_threads = false;
        {
            let mut queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return;
            }
            queue.debounced_prefill = None;
            queue.pending_prefill = Some(context.to_owned());
            if !queue.worker_started {
                queue.worker_started = true;
                start_threads = true;
            }
            self.queue.1.notify_all();
        }
        if start_threads {
            self.spawn_background_threads();
        }
    }

    /// 防抖调度空闲 Prefill（通常为 commit 后 50 ms）；新 Decode 到来会自动覆盖并取消之。
    pub fn schedule_prefill(self: &Arc<Self>, context: String, delay: Duration) {
        if !self.is_available() {
            return;
        }
        let mut start_threads = false;
        {
            let mut queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return;
            }
            queue.debounced_prefill = Some((Instant::now() + delay, context));
            if !queue.worker_started {
                queue.worker_started = true;
                start_threads = true;
            }
            self.queue.2.notify_all();
        }
        if start_threads {
            self.spawn_background_threads();
        }
    }

    /// 取消属于指定会话的待执行 Decode 与防闪定时器（焦点离开、退出组字时调用）。
    pub fn cancel_pending_decode_for_session(&self, session_id: u64) {
        {
            let mut queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue
                .pending_decode
                .as_ref()
                .is_some_and(|pending| pending.session_id == session_id)
            {
                queue.pending_decode = None;
            }
            if queue
                .provisional_deadline
                .is_some_and(|(sid, _, _)| sid == session_id)
            {
                queue.provisional_deadline = None;
            }
            if queue
                .latest_decode_gen_by_session
                .is_some_and(|(sid, _)| sid == session_id)
            {
                queue.latest_decode_gen_by_session = None;
            }
            self.queue.2.notify_all();
        }
        let mut wait = self
            .wait
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if wait
            .completed
            .as_ref()
            .is_some_and(|completed| completed.session_id == session_id)
        {
            wait.completed = None;
        }
    }

    /// 异步入队当前代次的 Decode 请求，同时武装 `provisional_window`（`~28 ms`）超时唤醒。
    /// 最新代次覆盖队列中尚未开始的旧请求。
    pub fn enqueue_decode(
        self: &Arc<Self>,
        session_id: u64,
        request: &NeuralDecodeRequest,
        provisional_window: Duration,
    ) -> bool {
        let Ok(payload_json) = serde_json::to_vec(request) else {
            return false;
        };
        if !self.is_available() {
            return false;
        }
        let generation = request.generation;
        let deadline = Instant::now() + provisional_window;
        let mut start_threads = false;
        {
            let mut queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return false;
            }
            queue.debounced_prefill = None;
            queue.pending_decode = Some(PendingDecode {
                session_id,
                generation,
                payload_json,
            });
            queue.latest_decode_gen_by_session = Some((session_id, generation));
            queue.provisional_deadline = Some((session_id, generation, deadline));
            if !queue.worker_started {
                queue.worker_started = true;
                start_threads = true;
            }
        }
        {
            let mut wait = self
                .wait
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if wait
                .completed
                .as_ref()
                .is_some_and(|c| c.session_id != session_id || c.generation < generation)
            {
                wait.completed = None;
            }
        }
        if start_threads {
            self.spawn_background_threads();
        }
        self.queue.1.notify_all();
        self.queue.2.notify_all();
        true
    }

    /// 取出并消费指定 `(session_id, generation)` 已完成的神经解码结果；仍在飞或代次不对返回 `None`。
    /// 外层 `Some(None)` 表示后端本次失败 / 退避中，外层 `Some(Some(resp))` 为有效响应。
    pub fn take_completed_decode(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Option<Option<NeuralDecodeResponse>> {
        let mut wait = self
            .wait
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let matches = wait
            .completed
            .as_ref()
            .is_some_and(|item| item.session_id == session_id && item.generation == generation);
        if matches {
            wait.completed.take().map(|item| item.response)
        } else {
            None
        }
    }

    /// 暖机状态下快敲空格提交时，有界等待在飞神经结果（未暖机时 0 ms 立即返回 `None`）。
    pub fn wait_for_decode_generation(
        &self,
        session_id: u64,
        generation: u64,
        timeout: Duration,
    ) -> Option<NeuralDecodeResponse> {
        if generation == 0 || timeout.is_zero() || !self.atomic_warmed.load(Ordering::Relaxed) {
            return None;
        }
        let bounded = timeout.min(MAX_FAST_SPACE_WAIT);
        let mut wait = self
            .wait
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ready_now = wait
            .completed
            .as_ref()
            .is_some_and(|item| item.session_id == session_id && item.generation >= generation);
        if !ready_now {
            let (guard, _) = self
                .wait
                .1
                .wait_timeout_while(wait, bounded, |state| {
                    if !self.atomic_warmed.load(Ordering::Relaxed) {
                        return false;
                    }
                    let done = state.completed.as_ref().is_some_and(|item| {
                        item.session_id == session_id && item.generation >= generation
                    });
                    !done
                })
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            wait = guard;
        }
        if wait
            .completed
            .as_ref()
            .is_some_and(|item| item.session_id == session_id && item.generation == generation)
        {
            wait.completed.take().and_then(|item| item.response)
        } else {
            None
        }
    }

    /// 同步发送一次 `Health` 探测（复用串行 IO worker 单连接，主要供集成测试与诊断检查）。
    pub fn check_health_sync(self: &Arc<Self>, timeout: Duration) -> Option<Value> {
        if !self.is_available() {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let mut start_threads = false;
        {
            let mut queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return None;
            }
            queue.pending_health.push(tx);
            if !queue.worker_started {
                queue.worker_started = true;
                start_threads = true;
            }
            self.queue.1.notify_all();
        }
        if start_threads {
            self.spawn_background_threads();
        }
        rx.recv_timeout(timeout).ok().flatten()
    }

    /// 同步停止后台线程与独立子进程，不残留孤儿进程与套接字文件。
    pub fn stop(&self) {
        {
            let mut queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return;
            }
            queue.stopped = true;
            queue.pending_decode = None;
            queue.pending_prefill = None;
            queue.provisional_deadline = None;
            queue.debounced_prefill = None;
            self.queue.1.notify_all();
            self.queue.2.notify_all();
        }
        {
            let mut wait = self
                .wait
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            wait.completed = None;
            self.atomic_warmed.store(false, Ordering::Relaxed);
            self.wait.1.notify_all();
        }
        let handles = self
            .worker_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some((io_handle, timer_handle)) = handles {
            let _ = io_handle.join();
            let _ = timer_handle.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }

    fn spawn_background_threads(self: &Arc<Self>) {
        let mut guard = self
            .worker_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_some() {
            return;
        }
        let io_client = Arc::clone(self);
        let io_handle = thread::Builder::new()
            .name("shurufa-neural-io".into())
            .spawn(move || io_client.worker_loop())
            .ok();
        let timer_client = Arc::clone(self);
        let timer_handle = thread::Builder::new()
            .name("shurufa-neural-timer".into())
            .spawn(move || timer_client.timer_loop())
            .ok();
        if let (Some(io_handle), Some(timer_handle)) = (io_handle, timer_handle) {
            *guard = Some((io_handle, timer_handle));
        }
    }

    fn timer_loop(&self) {
        loop {
            let mut wakeup_target: Option<(u64, u64)> = None;
            {
                let mut queue = self
                    .queue
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if queue.stopped {
                    return;
                }
                let now = Instant::now();
                if let Some((sid, gen, deadline)) = queue.provisional_deadline {
                    if now >= deadline {
                        queue.provisional_deadline = None;
                        wakeup_target = Some((sid, gen));
                    }
                }
                if wakeup_target.is_none() {
                    if let Some((deadline, _)) = &queue.debounced_prefill {
                        if now >= *deadline {
                            if let Some((_, ctx)) = queue.debounced_prefill.take() {
                                queue.pending_prefill = Some(ctx);
                                self.queue.1.notify_one();
                            }
                        }
                    }
                    let next_deadline = match (queue.provisional_deadline, &queue.debounced_prefill)
                    {
                        (Some((_, _, d1)), Some((d2, _))) => Some(d1.min(*d2)),
                        (Some((_, _, d1)), None) => Some(d1),
                        (None, Some((d2, _))) => Some(*d2),
                        (None, None) => None,
                    };
                    match next_deadline {
                        Some(deadline) => {
                            let timeout = deadline.saturating_duration_since(Instant::now());
                            if !timeout.is_zero() {
                                drop(
                                    self.queue
                                        .2
                                        .wait_timeout(queue, timeout)
                                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                                );
                            }
                        }
                        None => {
                            drop(
                                self.queue
                                    .2
                                    .wait(queue)
                                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                            );
                        }
                    }
                }
            }
            if let (Some((sid, gen)), Some(wakeup)) = (wakeup_target, &self.wakeup) {
                wakeup(sid, gen);
            }
        }
    }

    fn worker_loop(&self) {
        let mut proc_state = ServiceProcess::new();
        loop {
            enum Work {
                Decode(PendingDecode, NeuralClientConfig),
                Health(mpsc::Sender<Option<Value>>, NeuralClientConfig),
                Prefill(String, NeuralClientConfig),
                Stop,
            }
            let work = {
                let mut queue = self
                    .queue
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                loop {
                    if queue.stopped {
                        break Work::Stop;
                    }
                    if let Some(decode) = queue.pending_decode.take() {
                        break Work::Decode(decode, queue.config.clone());
                    }
                    if !queue.pending_health.is_empty() {
                        let tx = queue.pending_health.remove(0);
                        break Work::Health(tx, queue.config.clone());
                    }
                    if let Some(prefill) = queue.pending_prefill.take() {
                        break Work::Prefill(prefill, queue.config.clone());
                    }
                    queue = self
                        .queue
                        .1
                        .wait(queue)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };

            match work {
                Work::Stop => {
                    self.tear_down_process(&mut proc_state);
                    return;
                }
                Work::Health(tx, config) => {
                    let mut info = None;
                    if self.ensure_connected(&mut proc_state, &config) {
                        proc_state.next_request_id = proc_state.next_request_id.wrapping_add(1);
                        let req_id = proc_state.next_request_id.max(1);
                        let timeout = Duration::from_secs(1);
                        let ok = if let Some(stream) = proc_state.stream.as_mut() {
                            match write_frame(stream, NeuralOpCode::Health, 0, req_id, &[], timeout)
                            {
                                Ok(()) => match read_frame(stream, timeout) {
                                    Ok((hdr, payload)) if hdr.request_id == req_id => {
                                        if (hdr.flags & NEURAL_FLAG_OK) != 0 && !payload.is_empty()
                                        {
                                            info = serde_json::from_slice(&payload).ok();
                                        }
                                        true
                                    }
                                    _ => false,
                                },
                                Err(_) => false,
                            }
                        } else {
                            false
                        };
                        if !ok {
                            self.handle_transport_failure(&mut proc_state);
                        }
                    }
                    let _ = tx.send(info);
                }
                Work::Decode(decode, config) => {
                    let mut response: Option<NeuralDecodeResponse> = None;
                    if self.ensure_connected(&mut proc_state, &config) {
                        let timeout = if proc_state.service_warmed {
                            config.request_timeout.max(Duration::from_millis(50))
                        } else {
                            COLD_LOAD_TIMEOUT.max(config.request_timeout)
                        };
                        proc_state.next_request_id = proc_state.next_request_id.wrapping_add(1);
                        let req_id = proc_state.next_request_id.max(1);
                        let ok = if let Some(stream) = proc_state.stream.as_mut() {
                            match write_frame(
                                stream,
                                NeuralOpCode::Decode,
                                0,
                                req_id,
                                &decode.payload_json,
                                timeout,
                            ) {
                                Ok(()) => match read_frame(stream, timeout) {
                                    Ok((hdr, payload)) if hdr.request_id == req_id => {
                                        if (hdr.flags & NEURAL_FLAG_OK) != 0 && !payload.is_empty()
                                        {
                                            response = serde_json::from_slice(&payload).ok();
                                        }
                                        true
                                    }
                                    _ => false,
                                },
                                Err(_) => false,
                            }
                        } else {
                            false
                        };
                        if ok {
                            proc_state.service_warmed = true;
                            self.atomic_warmed.store(true, Ordering::Relaxed);
                        } else {
                            self.handle_transport_failure(&mut proc_state);
                        }
                    }

                    // 写入已完成结果并通知 Fast-Space 等待方。
                    {
                        let mut wait = self
                            .wait
                            .0
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        wait.completed = Some(CompletedDecode {
                            session_id: decode.session_id,
                            generation: decode.generation,
                            response,
                        });
                        self.wait.1.notify_all();
                    }
                    // 清除此代次的 28 ms Provisional 超时唤醒。
                    {
                        let mut queue = self
                            .queue
                            .0
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if queue.provisional_deadline.is_some_and(|(sid, gen, _)| {
                            sid == decode.session_id && gen == decode.generation
                        }) {
                            queue.provisional_deadline = None;
                            self.queue.2.notify_all();
                        }
                    }
                    if let Some(wakeup) = &self.wakeup {
                        wakeup(decode.session_id, decode.generation);
                    }
                }
                Work::Prefill(context, config) => {
                    if self.ensure_connected(&mut proc_state, &config) {
                        let timeout = if proc_state.service_warmed {
                            Duration::from_secs(1)
                        } else {
                            COLD_LOAD_TIMEOUT
                        };
                        proc_state.next_request_id = proc_state.next_request_id.wrapping_add(1);
                        let req_id = proc_state.next_request_id.max(1);
                        let ok = if let Some(stream) = proc_state.stream.as_mut() {
                            write_frame(
                                stream,
                                NeuralOpCode::Prefill,
                                0,
                                req_id,
                                context.as_bytes(),
                                timeout,
                            )
                            .is_ok()
                                && read_frame(stream, timeout)
                                    .is_ok_and(|(hdr, _)| hdr.request_id == req_id)
                        } else {
                            false
                        };
                        if ok {
                            proc_state.service_warmed = true;
                            self.atomic_warmed.store(true, Ordering::Relaxed);
                        } else {
                            self.handle_transport_failure(&mut proc_state);
                        }
                    }
                }
            }
        }
    }

    fn tear_down_process(&self, proc_state: &mut ServiceProcess) {
        if let Some(mut stream) = proc_state.stream.take() {
            proc_state.next_request_id = proc_state.next_request_id.wrapping_add(1);
            let req_id = proc_state.next_request_id.max(1);
            let _ = write_frame(
                &mut stream,
                NeuralOpCode::Shutdown,
                0,
                req_id,
                &[],
                Duration::from_millis(200),
            );
        }
        if let Some(mut child) = proc_state.child.take() {
            let deadline = Instant::now() + Duration::from_millis(400);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        proc_state.service_warmed = false;
        self.atomic_pid.store(0, Ordering::Relaxed);
        self.atomic_warmed.store(false, Ordering::Relaxed);
        self.wait.1.notify_all();
        let _ = fs::remove_file(&self.socket_path);
    }

    fn record_failure_and_backoff(&self, proc_state: &mut ServiceProcess) {
        let now = Instant::now();
        if proc_state
            .service_started_at
            .is_some_and(|started| now.saturating_duration_since(started) >= HEALTHY_UPTIME_RESET)
        {
            proc_state.consecutive_failures = 0;
        }
        proc_state.consecutive_failures = proc_state.consecutive_failures.saturating_add(1);
        let shift = (proc_state.consecutive_failures - 1).min(5);
        let delay = BACKOFF_BASE.saturating_mul(1u32 << shift).min(BACKOFF_MAX);
        proc_state.backoff_until = Some(now + delay);
    }

    fn handle_transport_failure(&self, proc_state: &mut ServiceProcess) {
        proc_state.stream = None;
        if let Some(child) = proc_state.child.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
                proc_state.child = None;
                proc_state.service_warmed = false;
                self.atomic_pid.store(0, Ordering::Relaxed);
                self.atomic_warmed.store(false, Ordering::Relaxed);
                self.wait.1.notify_all();
                self.record_failure_and_backoff(proc_state);
            }
        }
    }

    fn ensure_connected(
        &self,
        proc_state: &mut ServiceProcess,
        config: &NeuralClientConfig,
    ) -> bool {
        {
            let queue = self
                .queue
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.stopped {
                return false;
            }
        }
        if let (Some(_), Some(child)) = (&proc_state.stream, proc_state.child.as_mut()) {
            if matches!(child.try_wait(), Ok(None)) {
                return true;
            }
        }
        proc_state.stream = None;
        if let Some(child) = proc_state.child.as_mut() {
            if !matches!(child.try_wait(), Ok(None)) {
                proc_state.child = None;
                proc_state.service_warmed = false;
                self.atomic_pid.store(0, Ordering::Relaxed);
                self.atomic_warmed.store(false, Ordering::Relaxed);
                self.wait.1.notify_all();
                self.record_failure_and_backoff(proc_state);
            }
        }

        let now = Instant::now();
        if proc_state.backoff_until.is_some_and(|until| now < until) {
            return false;
        }

        if proc_state.child.is_none() {
            let Some(helper) = discover_neural_service_binary(config.helper_path.as_deref()) else {
                return false;
            };
            let model_dir = default_neural_model_directory(config.model_directory.as_deref());
            if config.require_model_file && !directory_has_gguf_model(&model_dir) {
                return false;
            }
            let _ = fs::remove_file(&self.socket_path);
            let idle = if config.idle_unload_seconds > 0.0 {
                config.idle_unload_seconds
            } else {
                600.0
            };
            let mut cmd = Command::new(&helper);
            cmd.arg("--socket")
                .arg(&self.socket_path)
                .arg("--model-dir")
                .arg(&model_dir)
                .arg("--idle-seconds")
                .arg(format!("{idle:.1}"))
                .arg("--parent-pid")
                .arg(std::process::id().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(pinyin_db) = resolved_pinyin_db_path(config) {
                cmd.arg("--pinyin-db").arg(pinyin_db);
            }
            cmd.args(&config.extra_launch_arguments);
            match cmd.spawn() {
                Ok(child) => {
                    let pid = child.id();
                    proc_state.child = Some(child);
                    proc_state.service_started_at = Some(now);
                    proc_state.service_warmed = false;
                    self.atomic_warmed.store(false, Ordering::Relaxed);
                    self.atomic_pid.store(pid, Ordering::Relaxed);
                }
                Err(_) => {
                    self.record_failure_and_backoff(proc_state);
                    return false;
                }
            }
        }

        let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
        while Instant::now() < connect_deadline {
            let still_alive = proc_state
                .child
                .as_mut()
                .is_some_and(|child| matches!(child.try_wait(), Ok(None)));
            if !still_alive {
                self.handle_transport_failure(proc_state);
                return false;
            }
            if let Ok(stream) = UnixStream::connect(&self.socket_path) {
                configure_socket_nosigpipe(&stream);
                proc_state.stream = Some(stream);
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }

        self.tear_down_process(proc_state);
        self.record_failure_and_backoff(proc_state);
        false
    }
}

impl Drop for NeuralServiceClient {
    fn drop(&mut self) {
        self.stop();
    }
}

fn configure_socket_nosigpipe(stream: &UnixStream) {
    #[cfg(target_os = "macos")]
    {
        let _ = rustix::net::sockopt::set_socket_nosigpipe(stream, true);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = stream;
    }
}

fn allocate_private_socket_path(tag: u64) -> PathBuf {
    let file_name = format!("shurufa-n-{}-{tag}.sock", std::process::id());
    let temp = std::env::temp_dir().join(&file_name);
    if temp.as_os_str().as_encoded_bytes().len() < 100 {
        return temp;
    }
    PathBuf::from("/tmp").join(file_name)
}

fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(text) = path.to_str().ok_or(()) else {
        return path.to_path_buf();
    };
    if let Some(rest) = text.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

fn is_regular_file_following_symlinks(path: &Path, require_executable: bool) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if require_executable {
        (meta.permissions().mode() & 0o111) != 0
    } else {
        true
    }
}

pub fn directory_has_gguf_model(directory: &Path) -> bool {
    if !directory.is_absolute() {
        return false;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let lower = name_str.to_ascii_lowercase();
        if !lower.ends_with(".gguf") || lower.contains("mmproj") {
            continue;
        }
        if is_regular_file_following_symlinks(&entry.path(), false) {
            return true;
        }
    }
    false
}

fn discover_neural_service_binary(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured {
        let expanded = expand_tilde(path);
        return is_regular_file_following_symlinks(&expanded, true).then_some(expanded);
    }
    if let Some(env_bin) = std::env::var_os("SHURUFA_NEURAL_SERVICE_BIN") {
        if !env_bin.is_empty() {
            let expanded = expand_tilde(Path::new(&env_bin));
            if is_regular_file_following_symlinks(&expanded, true) {
                return Some(expanded);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // 输入法 `.app/Contents/MacOS/Shurufa` -> `.app/Contents/Helpers/ShurufaNeuralService`
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            let bundled = contents.join("Helpers/ShurufaNeuralService");
            if is_regular_file_following_symlinks(&bundled, true) {
                return Some(bundled);
            }
        }
    }
    None
}

fn default_neural_model_directory(configured: Option<&Path>) -> PathBuf {
    if let Some(dir) = configured {
        return expand_tilde(dir);
    }
    if let Some(env_dir) = std::env::var_os("SHURUFA_NEURAL_MODEL_DIR") {
        if !env_dir.is_empty() {
            return expand_tilde(Path::new(&env_dir));
        }
    }
    expand_tilde(Path::new(
        "~/Library/Application Support/Shurufa/resource-packs/neural-lm",
    ))
}

fn resolved_pinyin_db_path(config: &NeuralClientConfig) -> Option<PathBuf> {
    if let Some(path) = config.pinyin_db_path.as_deref() {
        let expanded = expand_tilde(path);
        if is_regular_file_following_symlinks(&expanded, false) {
            return Some(expanded);
        }
    }
    if let Some(res) = config.resources_directory.as_deref() {
        let candidate = expand_tilde(res).join("msime-pinyin.db");
        if is_regular_file_following_symlinks(&candidate, false) {
            return Some(candidate);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            let bundled = contents.join("Resources/EngineResources/msime-pinyin.db");
            if is_regular_file_following_symlinks(&bundled, false) {
                return Some(bundled);
            }
        }
    }
    None
}

fn check_config_available(config: &NeuralClientConfig) -> bool {
    if discover_neural_service_binary(config.helper_path.as_deref()).is_none() {
        return false;
    }
    if config.require_model_file {
        let model_dir = default_neural_model_directory(config.model_directory.as_deref());
        return directory_has_gguf_model(&model_dir);
    }
    true
}

fn write_exact_deadline(stream: &mut UnixStream, data: &[u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "write deadline exceeded",
            ));
        }
        stream.set_write_timeout(Some(deadline.saturating_duration_since(now)))?;
        match stream.write(&data[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::WriteZero,
                    "peer closed while writing",
                ))
            }
            Ok(n) => offset += n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn read_exact_deadline(
    stream: &mut UnixStream,
    buf: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < buf.len() {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "read deadline exceeded",
            ));
        }
        stream.set_read_timeout(Some(deadline.saturating_duration_since(now)))?;
        match stream.read(&mut buf[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "peer closed while reading",
                ))
            }
            Ok(n) => offset += n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub fn write_frame(
    stream: &mut UnixStream,
    op: NeuralOpCode,
    flags: u16,
    request_id: u32,
    payload: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    if payload.len() > NEURAL_MAX_FRAME_PAYLOAD_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "neural payload exceeds 512 KiB",
        ));
    }
    let deadline = Instant::now() + timeout;
    let header = NeuralFrameHeader {
        magic: NEURAL_FRAME_MAGIC,
        version: NEURAL_PROTOCOL_VERSION,
        op: op as u8,
        flags,
        payload_len: payload.len() as u32,
        request_id,
    }
    .encode();
    write_exact_deadline(stream, &header, deadline)?;
    if !payload.is_empty() {
        write_exact_deadline(stream, payload, deadline)?;
    }
    Ok(())
}

pub fn read_frame(
    stream: &mut UnixStream,
    timeout: Duration,
) -> io::Result<(NeuralFrameHeader, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let mut raw_header = [0u8; NEURAL_FRAME_HEADER_BYTES];
    read_exact_deadline(stream, &mut raw_header, deadline)?;
    let header = NeuralFrameHeader::decode(&raw_header)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "invalid SHRN frame header"))?;
    let mut payload = vec![0u8; header.payload_len as usize];
    if !payload.is_empty() {
        read_exact_deadline(stream, &mut payload, deadline)?;
    }
    Ok((header, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use msime_engine::neural_v2::NeuralCandidateInput;

    fn write_mock_service_script(dir: &Path, extra_decode_delay_ms: u64) -> PathBuf {
        let script = dir.join("mock_neural_service.sh");
        let py = format!(
            r#"#!/bin/sh
exec python3 -u - "$@" <<'PYEOF'
import json, os, socket, struct, sys, time

args = sys.argv[1:]
sock_path = None
i = 0
while i < len(args):
    if args[i] == '--socket' and i + 1 < len(args):
        sock_path = args[i + 1]
        i += 2
    else:
        i += 1
if not sock_path:
    sys.exit(2)
if os.path.exists(sock_path):
    os.unlink(sock_path)

srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path)
srv.listen(4)

MAGIC = 0x5348524E
DELAY_SEC = {delay_ms} / 1000.0

def read_exact(conn, n):
    buf = bytearray()
    while len(buf) < n:
        chunk = conn.recv(n - len(buf))
        if not chunk:
            return None
        buf.extend(chunk)
    return bytes(buf)

while True:
    try:
        conn, _ = srv.accept()
    except OSError:
        break
    with conn:
        while True:
            hdr = read_exact(conn, 16)
            if not hdr:
                break
            magic, ver, op, flags, plen, rid = struct.unpack('>IBBHII', hdr)
            if magic != MAGIC or ver != 1:
                break
            payload = read_exact(conn, plen) if plen else b''
            if payload is None:
                break
            if op == 5:
                sys.exit(0)
            if op == 1:
                resp = json.dumps({{"ok": True, "mock": True, "pid": os.getpid()}}).encode()
                conn.sendall(struct.pack('>IBBHII', MAGIC, 1, 1, 1, len(resp), rid) + resp)
            elif op == 2:
                resp = b'{{"ok":true}}'
                conn.sendall(struct.pack('>IBBHII', MAGIC, 1, 2, 1, len(resp), rid) + resp)
            elif op == 3:
                if DELAY_SEC > 0:
                    time.sleep(DELAY_SEC)
                req = json.loads(payload.decode())
                scores = []
                for idx, cand in enumerate(req.get("candidates", [])):
                    text = cand["text"]
                    score = -1.0 if text in ("挣了些钱", "假设") else -25.0 - idx
                    scores.append({{"text": text, "neural_log_prob": score}})
                resp = json.dumps({{
                    "ok": True,
                    "generation": req["generation"],
                    "candidate_scores": scores,
                    "beam_results": [],
                    "elapsed_ms": DELAY_SEC * 1000.0,
                }}).encode()
                conn.sendall(struct.pack('>IBBHII', MAGIC, 1, 3, 1, len(resp), rid) + resp)
PYEOF
"#,
            delay_ms = extra_decode_delay_ms
        );
        fs::write(&script, py).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    fn dummy_request(gen: u64, texts: &[&str]) -> NeuralDecodeRequest {
        NeuralDecodeRequest {
            session: 1,
            generation: gen,
            context: "这个兄弟，应该多少还是".into(),
            right_context: String::new(),
            raw_input: "zhenglexieqian".into(),
            segmentation: "zheng'le'xie'qian".into(),
            syllable_paths: Vec::new(),
            candidates: texts
                .iter()
                .enumerate()
                .map(|(i, text)| NeuralCandidateInput {
                    index: i,
                    text: (*text).to_owned(),
                    pinyin: String::new(),
                    source: 1,
                    char_count: text.chars().count(),
                    exact_span: true,
                    exact_literal_dict: false,
                    stat_log_prob: Some(-12.0),
                    stat_from_lattice: false,
                    channel_log_prob: 0.0,
                    user_log_prob: 0.0,
                    collocation_log_prob: 0.0,
                    pinned: false,
                    has_lexical_head: false,
                })
                .collect(),
            pinned_first_index: None,
            pinned_seats: Vec::new(),
            beam_width: 4,
            max_beam_results: 4,
            enable_constrained_decode: true,
        }
    }

    #[test]
    fn missing_model_fails_open_with_zero_processes() {
        let dir = tempfile::tempdir().unwrap();
        let helper = write_mock_service_script(dir.path(), 0);
        let client = NeuralServiceClient::new(
            NeuralClientConfig {
                helper_path: Some(helper),
                model_directory: Some(dir.path().join("nonexistent-models")),
                socket_path: Some(dir.path().join("sock.sock")),
                require_model_file: true,
                ..Default::default()
            },
            None,
        );
        assert!(!client.is_available());
        assert_eq!(client.service_pid(), 0);
        assert!(!client.enqueue_decode(
            1,
            &dummy_request(1, &["整了血铅", "挣了些钱"]),
            NEURAL_PROVISIONAL_WINDOW
        ));
        assert_eq!(client.service_pid(), 0);
    }

    #[test]
    fn mock_service_handles_health_fast_space_wait_coalescing_and_crash_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let helper = write_mock_service_script(dir.path(), 0);
        let (tx, rx) = mpsc::channel();
        let wakeup: NeuralWakeupFn = Arc::new(move |sid, gen| {
            let _ = tx.send((sid, gen));
        });
        let client = NeuralServiceClient::new(
            NeuralClientConfig {
                helper_path: Some(helper),
                model_directory: Some(dir.path().to_path_buf()),
                socket_path: Some(dir.path().join("ipc.sock")),
                require_model_file: false,
                request_timeout: Duration::from_secs(2),
                ..Default::default()
            },
            Some(wakeup),
        );
        assert!(client.is_available());
        // 未暖机时 wait_for_decode_generation 必须 0 ms 返回 None。
        let t0 = Instant::now();
        assert!(client
            .wait_for_decode_generation(1, 1, Duration::from_millis(35))
            .is_none());
        assert!(t0.elapsed() < Duration::from_millis(10));

        let health = client
            .check_health_sync(Duration::from_secs(3))
            .expect("Health 探测应成功");
        assert_eq!(health["ok"], true);
        assert_eq!(health["mock"], true);
        let first_pid = client.service_pid();
        assert!(first_pid > 0);

        // 先发起一次预热，确保服务进入暖机态。
        client.prefill_context("上文预热");
        let req1 = dummy_request(7, &["整了血铅", "挣了些钱"]);
        assert!(client.enqueue_decode(1, &req1, Duration::from_millis(200)));

        // 等待唤醒回调。
        let (sid, gen) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!((sid, gen), (1, 7));
        let completed = client
            .take_completed_decode(1, 7)
            .expect("第 7 代结果应已完成")
            .expect("响应不应为空");
        assert_eq!(completed.generation, 7);
        assert!(client.is_warmed());

        // 暖机态快敲空格 Condvar 同步等待。
        let req2 = dummy_request(10, &["假设", "建设"]);
        assert!(client.enqueue_decode(1, &req2, Duration::from_millis(200)));
        let waited = client
            .wait_for_decode_generation(1, 10, Duration::from_millis(35))
            .expect("暖机态应由 Condvar 立即带回第 10 代结果");
        assert_eq!(waited.generation, 10);

        // 模拟子进程意外被杀：首次 Decode 静默 fail-open（不崩溃），退避 1s 后自愈换新 PID。
        let _ = rustix::process::kill_process(
            rustix::process::Pid::from_raw(first_pid as i32).unwrap(),
            rustix::process::Signal::KILL,
        );
        thread::sleep(Duration::from_millis(120));
        while rx.try_recv().is_ok() {}
        let req3 = dummy_request(11, &["假设", "建设"]);
        assert!(client.enqueue_decode(1, &req3, Duration::from_millis(50)));
        let (_, gen3) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(gen3, 11);
        assert!(client.take_completed_decode(1, 11).unwrap().is_none());

        // 等 1.1s 退避结束，新请求应自动重起新进程并成功回包。
        thread::sleep(Duration::from_millis(1150));
        while rx.try_recv().is_ok() {}
        let req4 = dummy_request(12, &["整了血铅", "挣了些钱"]);
        assert!(client.enqueue_decode(1, &req4, Duration::from_millis(200)));
        loop {
            let (_, g) = rx.recv_timeout(Duration::from_secs(4)).unwrap();
            if g == 12 {
                if let Some(Some(resp)) = client.take_completed_decode(1, 12) {
                    assert_eq!(resp.generation, 12);
                    break;
                }
            }
        }
        let second_pid = client.service_pid();
        assert!(second_pid > 0 && second_pid != first_pid);
        client.stop();
        assert_eq!(client.service_pid(), 0);
    }

    #[test]
    fn slow_reply_triggers_provisional_deadline_wakeup_before_late_result() {
        let dir = tempfile::tempdir().unwrap();
        let helper = write_mock_service_script(dir.path(), 140);
        let (tx, rx) = mpsc::channel();
        let wakeup: NeuralWakeupFn = Arc::new(move |sid, gen| {
            let _ = tx.send((sid, gen, Instant::now()));
        });
        let client = NeuralServiceClient::new(
            NeuralClientConfig {
                helper_path: Some(helper),
                model_directory: Some(dir.path().to_path_buf()),
                socket_path: Some(dir.path().join("slow.sock")),
                require_model_file: false,
                request_timeout: Duration::from_secs(2),
                ..Default::default()
            },
            Some(wakeup),
        );
        // 先 Health 确保进程已启动并完成冷建联，避免启动耗时混入 140 ms 测量。
        assert!(client.check_health_sync(Duration::from_secs(3)).is_some());
        while rx.try_recv().is_ok() {}

        let started = Instant::now();
        let req = dummy_request(21, &["整了血铅", "挣了些钱"]);
        assert!(client.enqueue_decode(1, &req, NEURAL_PROVISIONAL_WINDOW));

        // 第一次唤醒应在 ~70 ms 到期时触发，此时解码仍在飞行中（take_completed_decode == None）。
        let (sid1, gen1, t1) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!((sid1, gen1), (1, 21));
        assert!(t1.duration_since(started) >= Duration::from_millis(55));
        assert!(client.take_completed_decode(1, 21).is_none());

        // 第二次唤醒应在 140 ms 后携带完成结果到来。
        let (sid2, gen2, _) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((sid2, gen2), (1, 21));
        let done = client
            .take_completed_decode(1, 21)
            .expect("迟到回包应就绪")
            .expect("响应应解析成功");
        assert_eq!(done.generation, 21);
        client.stop();
    }
}
