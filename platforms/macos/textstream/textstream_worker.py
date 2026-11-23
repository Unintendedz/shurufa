"""TextStream 本地流式识别工作进程，实现 msime-voice-local 的 stdin/stdout JSON 行协议。

协议见 shurufa/shared/voice/README.md 的 "The msime-voice-local helper"。

出处与许可：
- 适配器结构、Metal 内存与 CPU 预算、VAD 门控参考 OpenLess 的 TextStream 适配器
  （OpenLess，AGPL-3.0）。
- 流式解码设置沿用 textstream-asr（199-biotechnologies/textstream-asr，
  rev cacf67acadbd90789b631d9ee8f2e76f1a6cd7c2，MIT）。
- 解码由 mlx-qwen3-asr 0.4.4（Apache-2.0）完成，模型为 Qwen/Qwen3-ASR-1.7B。

本进程不打开麦克风、不联网、不写任何文件。原生推理库只在 hello 阶段导入，模型在 start 时加载。
"""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import math
import os
import sys
import threading
import time
import wave
from pathlib import Path

PROTOCOL_VERSION = 1
SAMPLE_RATE = 16000
MAX_LINE_BYTES = 1 << 20
MAX_QUEUE_BYTES = 8 << 20
DEFAULT_IDLE_EXIT_SECONDS = 600
LEGACY_HOST_IDLE_EXIT_SECONDS = 120
HIGH_MEM_THRESHOLD_BYTES = 32 * 1024**3
HIGH_MEM_MODEL_IDLE_SECONDS = 900
LOW_MEM_MODEL_IDLE_SECONDS = 180
DEFAULT_CPU_THREADS = 2
MAX_CPU_THREADS = 4

# 全程 Silero VAD 与安全切段参数（帧长 512 采样 = 32 ms）
VAD_FRAME = 512
VAD_FRAME_MS = VAD_FRAME * 1000.0 / SAMPLE_RATE
VAD_THRESHOLD = 0.4
PREROLL_MS = 256
PREROLL_SAMPLES = SAMPLE_RATE * PREROLL_MS // 1000  # 4096 采样 = 256 ms
PAUSE_SKIP_MS = 640.0
PAUSE_RESUME_MIN_FRAMES = 2  # 连续 >= 2 帧（64 ms）才从长停顿跳过恢复，防止单帧敲击/咳嗽尖峰误唤醒
SEGMENT_SOFT_MIN_SEC = 18.0
SEGMENT_SOFT_PAUSE_MS = 1200.0
SEGMENT_WARN_MIN_SEC = 24.0
SEGMENT_WARN_PAUSE_MS = 320.0
SEGMENT_MICRO_MIN_SEC = 26.0
SEGMENT_MICRO_PAUSE_MS = 64.0  # 24s 软切与 28s 硬切之间的 2 帧微停顿窗口，优先在词间切段减少硬切劈裂音节
SEGMENT_HARD_MAX_SEC = 28.0

DEFAULT_CHUNK_SEC = float(os.environ.get("SHURUFA_TEXTSTREAM_CHUNK_SEC", "0.5"))
MAX_CONTEXT_SEC = 30.0

_out_file = None
_out_lock = threading.Lock()
_quiet = False


def emit(event: dict) -> None:
    if _quiet or _out_file is None:
        return
    line = json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n"
    with _out_lock:
        _out_file.write(line)
        _out_file.flush()


def claim_stdout() -> None:
    """协议独占原 stdout；其余库往 stdout 打印的内容改走 stderr。"""
    global _out_file
    _out_file = os.fdopen(os.dup(1), "w", encoding="utf-8", buffering=1)
    os.dup2(2, 1)
    sys.stdout = sys.stderr


def physical_memory_bytes() -> int:
    """查询本机物理内存字节数，用于分级决定模型默认驻留时长。"""
    try:
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
        if pages > 0 and page_size > 0:
            return int(pages) * int(page_size)
    except (AttributeError, OSError, ValueError):
        pass
    try:
        import ctypes

        libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        memsize = ctypes.c_uint64(0)
        size = ctypes.c_size_t(ctypes.sizeof(memsize))
        if libc.sysctlbyname(b"hw.memsize", ctypes.byref(memsize), ctypes.byref(size), None, 0) == 0:
            return int(memsize.value)
    except Exception:  # noqa: BLE001
        pass
    return 0


def resolve_model_idle_seconds(
    env_value: str | None = None,
    memory_bytes: int | None = None,
) -> float:
    """解析模型空闲卸载时长（秒）：优先读取 `SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC`，否则按物理内存分级。"""
    raw = os.environ.get("SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC") if env_value is None else env_value
    if raw is not None and raw.strip() != "":
        try:
            parsed = float(raw.strip())
            if math.isfinite(parsed) and parsed >= 0.0:
                return parsed
        except ValueError:
            pass
        print(
            f"invalid SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC={raw!r}; falling back to memory tier",
            file=sys.stderr,
        )
    mem = physical_memory_bytes() if memory_bytes is None else memory_bytes
    if mem >= HIGH_MEM_THRESHOLD_BYTES:
        return float(HIGH_MEM_MODEL_IDLE_SECONDS)
    return float(LOW_MEM_MODEL_IDLE_SECONDS)


def resolve_idle_timeouts(
    cli_idle_exit: int | None,
    env_model_idle: str | None = None,
    memory_bytes: int | None = None,
) -> tuple[float, float]:
    """协调 `--idle-exit` 与 `SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC`，返回 `(idle_exit_sec, model_idle_sec)`。"""
    raw_env = os.environ.get("SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC") if env_model_idle is None else env_model_idle
    has_env_override = raw_env is not None and raw_env.strip() != ""
    model_idle = resolve_model_idle_seconds(raw_env, memory_bytes)

    if cli_idle_exit is None or cli_idle_exit == DEFAULT_IDLE_EXIT_SECONDS:
        # 未显式传 `--idle-exit` 或传入默认值时，按内存分级/环境变量确保进程寿命覆盖模型驻留时长。
        idle_exit = max(float(DEFAULT_IDLE_EXIT_SECONDS), model_idle) if model_idle > 0 else float(DEFAULT_IDLE_EXIT_SECONDS)
        return idle_exit, model_idle

    if cli_idle_exit == 0:
        return 0.0, model_idle

    if cli_idle_exit == LEGACY_HOST_IDLE_EXIT_SECONDS and has_env_override:
        # 兼容旧宿主固定传入 `--idle-exit 120` 并通过环境变量下发内存分级驻留策略。
        idle_exit = max(float(cli_idle_exit), model_idle) if model_idle > 0 else float(cli_idle_exit)
        return idle_exit, model_idle

    # 显式传入较短 `--idle-exit`（如协议单测 `--idle-exit 3`）时严格服从命令行时限。
    idle_exit = float(cli_idle_exit)
    if model_idle > 0:
        model_idle = min(model_idle, idle_exit)
    return idle_exit, model_idle


def metal_memory_limits(memory_bytes: int, recommended_bytes: int) -> tuple[int, int]:
    """给宿主和前台应用留出内存；MLX 的内存上限只是建议值，不是系统配额。"""
    if memory_bytes <= 0 or recommended_bytes <= 0:
        raise ValueError("Metal memory information is unavailable")
    memory_limit = min(8 * 1024**3, memory_bytes // 3, recommended_bytes)
    cache_limit = min(128 * 1024**2, memory_limit // 16)
    return memory_limit, cache_limit


def configure_cpu_threads() -> float:
    """在导入 NumPy/MLX/VAD 之前设定原生库线程数。

    优先读取 `SHURUFA_TEXTSTREAM_CPU_THREADS`（默认 2，上限 4），并向后兼容旧 `SHURUFA_TEXTSTREAM_CPU_CORES`。
    """
    raw = os.environ.get("SHURUFA_TEXTSTREAM_CPU_THREADS")
    if raw is None or raw.strip() == "":
        raw = os.environ.get("SHURUFA_TEXTSTREAM_CPU_CORES")
    if raw is None or raw.strip() == "":
        raw = str(DEFAULT_CPU_THREADS)
    cores = float(raw.strip())
    if not math.isfinite(cores) or cores <= 0:
        raise ValueError("invalid CPU budget")
    cap = float(min(MAX_CPU_THREADS, os.cpu_count() or MAX_CPU_THREADS))
    cores = min(cores, cap)
    threads = str(max(1, min(MAX_CPU_THREADS, math.floor(cores))))
    for name in (
        "OMP_NUM_THREADS",
        "OPENBLAS_NUM_THREADS",
        "MKL_NUM_THREADS",
        "VECLIB_MAXIMUM_THREADS",
        "NUMEXPR_NUM_THREADS",
    ):
        os.environ[name] = threads
    os.environ["TOKENIZERS_PARALLELISM"] = "false"
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    return cores


def _needs_ascii_space(left_char: str, right_char: str) -> bool:
    """判断两段拼接边界是否需要补单空格：仅英文/数字及西文句读接英文/数字时补空格，中文段间不插多余空格。"""
    left_latin = left_char.isascii() and (left_char.isalnum() or left_char in ".,?!;:")
    right_latin = right_char.isascii() and right_char.isalnum()
    return left_latin and right_latin


def join_text_pair(left: str, right: str) -> str:
    """按语种边界规则拼接两段转写文本。"""
    if not left:
        return right
    if not right:
        return left
    if left[-1].isspace() or right[0].isspace():
        return left + right
    if _needs_ascii_space(left[-1], right[0]):
        return f"{left} {right}"
    return left + right


def join_segments(segments: list[str], tail: str = "") -> str:
    """将已提交分段列表与可选尾部文本顺序拼接为完整转写。"""
    result = ""
    for part in segments:
        if not part:
            continue
        result = join_text_pair(result, part)
    if tail:
        result = join_text_pair(result, tail)
    return result


class CpuBudget:
    """协作式 CPU 秒预算；GPU 等待不计入 CPU 工作量。"""

    def __init__(self, cores: float) -> None:
        self.cores = cores
        self.reset()

    def reset(self) -> None:
        self.cpu_start = time.process_time()
        self.wall_start = time.monotonic()

    def checkpoint(self, abort: threading.Event | None = None) -> None:
        debt = max(
            0.0,
            (time.process_time() - self.cpu_start) / self.cores
            - (time.monotonic() - self.wall_start),
        )
        resume = time.monotonic() + debt
        while True:
            remaining = resume - time.monotonic()
            if remaining <= 0 or (abort is not None and abort.is_set()):
                break
            time.sleep(min(remaining, 0.02))
        self.reset()


class Runtime:
    """惰性持有 MLX 与相关库；import 成功即视为运行时可用。"""

    def __init__(self) -> None:
        self.error = ""
        self.mx = None
        self.np = None
        self.Session = None
        self.SileroVAD = None
        self.budget: CpuBudget | None = None

    def load(self) -> bool:
        try:
            self.budget = CpuBudget(configure_cpu_threads())
            import mlx.core as mx

            if not mx.metal.is_available():
                raise RuntimeError("Metal GPU is required; CPU fallback is disabled")
            mx.set_default_device(mx.gpu)
            device = mx.metal.device_info()
            memory_limit, cache_limit = metal_memory_limits(
                int(device["memory_size"]),
                int(device["max_recommended_working_set_size"]),
            )
            mx.set_memory_limit(memory_limit)
            mx.set_cache_limit(cache_limit)
            import numpy as np
            from mlx_qwen3_asr import Session
            from silero_vad_lite import SileroVAD

            self.mx, self.np, self.Session, self.SileroVAD = mx, np, Session, SileroVAD
            return True
        except Exception as error:  # noqa: BLE001
            self.error = f"{type(error).__name__}: {error}"
            return False

    @property
    def available(self) -> bool:
        return self.Session is not None


class Aborted(Exception):
    pass


class DictationSession:
    """一次听写会话：全程 Silero VAD 状态机（SILENCE -> SPEECH -> PAUSE -> SEGMENT_COMMIT）+ 安全切段流式解码。"""

    def __init__(self, rt: Runtime, sid, model_session, language, context, chunk_sec):
        self.rt = rt
        self.id = sid
        self.model_session = model_session
        self.language = language
        self.context = context
        self.chunk_sec = chunk_sec
        np = rt.np
        self.state = self._new_streaming_state()
        self.chunk_samples = self.state.chunk_size_samples
        self.vad = rt.SileroVAD(SAMPLE_RATE)

        # 全程 VAD 状态机字段
        self.vad_state = "SILENCE"  # "SILENCE" | "SPEECH" | "PAUSE"
        self.heard_speech = False
        self.segment_has_speech = False
        self.pause_skipping = False
        self.resume_pending_frames: list = []
        self.silence_ms = 0.0
        self.vad_pending = np.zeros(0, dtype=np.float32)
        self.preroll_buf = np.zeros(0, dtype=np.float32)
        self.feed_buf = np.zeros(0, dtype=np.float32)
        self.segment_fed_samples = 0

        # 多段单调累积结果与观测指标
        self.committed_segments: list[str] = []
        self.last_text = ""
        self.last_finalized = ""
        self.total_audio_samples = 0
        self.speech_samples = 0
        self.asr_elapsed_sec = 0.0

    def _new_streaming_state(self):
        return self.model_session.init_streaming(
            chunk_size_sec=self.chunk_sec,
            max_context_sec=MAX_CONTEXT_SEC,
            sample_rate=SAMPLE_RATE,
            unfixed_chunk_num=2,
            unfixed_token_num=5,
            language=self.language,
            context=self.context,
            finalization_mode="accuracy",
        )

    @property
    def segment_audio_sec(self) -> float:
        return self.segment_fed_samples / SAMPLE_RATE

    def _compose_preview(self) -> tuple[str, str, str]:
        """组装完整累积文本及双态切分，严格保证 `finalized + draft == text` 且已提交段单调不回退。"""
        committed = join_segments(self.committed_segments)
        cur_text = self.state.text if self.segment_has_speech else ""
        cur_stable = self.state.stable_text if self.segment_has_speech else ""
        if cur_text and not cur_text.startswith(cur_stable):
            cur_stable = os.path.commonprefix([cur_stable, cur_text])
        if not cur_text:
            return committed, committed, ""
        full_text = join_text_pair(committed, cur_text)
        if cur_stable:
            finalized = join_text_pair(committed, cur_stable)
            if not full_text.startswith(finalized):
                finalized = os.path.commonprefix([finalized, full_text])
        else:
            finalized = committed
        draft = full_text[len(finalized) :]
        return full_text, finalized, draft

    def _snapshot(self, abort: threading.Event) -> None:
        _ = abort
        text, finalized, draft = self._compose_preview()
        if not text:
            return
        if text == self.last_text and finalized == self.last_finalized:
            return
        self.last_text = text
        self.last_finalized = finalized
        emit(
            {
                "type": "partial",
                "id": self.id,
                "text": text,
                "finalized": finalized,
                "draft": draft,
            }
        )

    def _push_preroll(self, frame) -> None:
        """维护最近 256 ms（4096 采样）环形回补缓冲，避免开口与复讲截断辅音。"""
        np = self.rt.np
        combined = np.concatenate([self.preroll_buf, frame]) if len(self.preroll_buf) else frame
        if len(combined) > PREROLL_SAMPLES:
            combined = combined[-PREROLL_SAMPLES:]
        self.preroll_buf = combined

    def _append_segment_audio(self, samples) -> None:
        if len(samples) == 0:
            return
        np = self.rt.np
        self.feed_buf = np.concatenate([self.feed_buf, samples]) if len(self.feed_buf) else samples
        self.segment_fed_samples += len(samples)

    def _decode_ready(self, abort: threading.Event) -> None:
        rt = self.rt
        while len(self.feed_buf) >= self.chunk_samples:
            if abort.is_set():
                raise Aborted
            piece = self.feed_buf[: self.chunk_samples]
            self.feed_buf = self.feed_buf[self.chunk_samples :]
            t0 = time.monotonic()
            self.state = self.model_session.feed_audio(piece, self.state)
            self.asr_elapsed_sec += time.monotonic() - t0
            rt.mx.clear_cache()
            rt.budget.checkpoint(abort)
            self._snapshot(abort)

    def _should_commit_segment(self) -> bool:
        """判断是否满足安全切段提交条件：自然换气点（>=18s & >=1200ms）、软上限（>=24s & >=320ms）、微停顿窗口（>=26s & >=64ms）或硬上限（>=28s）。"""
        if not self.segment_has_speech:
            return False
        sec = self.segment_audio_sec
        if sec >= SEGMENT_HARD_MAX_SEC:
            return True
        if sec >= SEGMENT_MICRO_MIN_SEC and self.silence_ms >= SEGMENT_MICRO_PAUSE_MS:
            return True
        if sec >= SEGMENT_WARN_MIN_SEC and self.silence_ms >= SEGMENT_WARN_PAUSE_MS:
            return True
        if sec >= SEGMENT_SOFT_MIN_SEC and self.silence_ms >= SEGMENT_SOFT_PAUSE_MS:
            return True
        return False

    def _commit_segment(self, abort: threading.Event, *, reset_state: bool = True) -> None:
        """提交当前分段（调用 `finish_streaming`），并可选重置流式解码器状态准备接收下一段。"""
        if not self.segment_has_speech:
            return
        rt = self.rt
        np = rt.np
        if len(self.feed_buf):
            if abort.is_set():
                raise Aborted
            t0 = time.monotonic()
            self.state = self.model_session.feed_audio(self.feed_buf, self.state)
            self.asr_elapsed_sec += time.monotonic() - t0
            self.feed_buf = np.zeros(0, dtype=np.float32)
        if abort.is_set():
            raise Aborted
        t0 = time.monotonic()
        self.state = self.model_session.finish_streaming(self.state)
        self.asr_elapsed_sec += time.monotonic() - t0
        seg_text = (self.state.text or "").strip()
        if seg_text:
            self.committed_segments.append(seg_text)
        rt.mx.clear_cache()
        rt.budget.checkpoint(abort if reset_state else None)

        # 清空当前段状态，回到 SILENCE 等待下一句开口。
        self.segment_fed_samples = 0
        self.segment_has_speech = False
        self.vad_state = "SILENCE"
        self.silence_ms = 0.0
        self.pause_skipping = False
        self.resume_pending_frames = []
        self.preroll_buf = np.zeros(0, dtype=np.float32)
        if reset_state:
            self.state = self._new_streaming_state()
            self._snapshot(abort)

    def push(self, raw: bytes, abort: threading.Event) -> None:
        np = self.rt.np
        audio = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
        if len(audio) == 0:
            return
        self.total_audio_samples += len(audio)
        buf = np.concatenate([self.vad_pending, audio]) if len(self.vad_pending) else audio

        offset = 0
        total = len(buf)
        while offset + VAD_FRAME <= total:
            if abort.is_set():
                raise Aborted
            frame = np.ascontiguousarray(buf[offset : offset + VAD_FRAME])
            offset += VAD_FRAME
            is_speech = self.vad.process(memoryview(frame.data)) >= VAD_THRESHOLD
            if is_speech:
                self.speech_samples += VAD_FRAME

            if self.vad_state == "SILENCE":
                if is_speech:
                    self.heard_speech = True
                    self.segment_has_speech = True
                    self.vad_state = "SPEECH"
                    self.silence_ms = 0.0
                    self.pause_skipping = False
                    if len(self.preroll_buf):
                        self._append_segment_audio(self.preroll_buf)
                        self.preroll_buf = np.zeros(0, dtype=np.float32)
                    self._append_segment_audio(frame)
                    self._decode_ready(abort)
                    if self._should_commit_segment():
                        self._commit_segment(abort, reset_state=True)
                else:
                    self._push_preroll(frame)
            elif self.vad_state == "SPEECH":
                if is_speech:
                    self.silence_ms = 0.0
                    self._append_segment_audio(frame)
                    self._decode_ready(abort)
                    if self._should_commit_segment():
                        self._commit_segment(abort, reset_state=True)
                else:
                    # 语音转入短停顿：保留前 640 ms 的自然语流静音送给解码器，避免过早截断词尾。
                    self.vad_state = "PAUSE"
                    self.silence_ms = VAD_FRAME_MS
                    self.pause_skipping = False
                    self._append_segment_audio(frame)
                    self._decode_ready(abort)
                    if self._should_commit_segment():
                        self._commit_segment(abort, reset_state=True)
            else:  # self.vad_state == "PAUSE"
                if is_speech:
                    if self.pause_skipping:
                        # 长停顿跳过期间需连续 >= 2 帧（64 ms）语音才确认复讲，防止单帧敲击/咳嗽尖峰误唤醒。
                        self.resume_pending_frames.append(frame)
                        if len(self.resume_pending_frames) < PAUSE_RESUME_MIN_FRAMES:
                            continue
                        self.vad_state = "SPEECH"
                        self.silence_ms = 0.0
                        self.pause_skipping = False
                        # 先回补最近 256 ms 环形缓冲，再按顺序喂入已确认的复讲语音帧。
                        if len(self.preroll_buf):
                            self._append_segment_audio(self.preroll_buf)
                            self.preroll_buf = np.zeros(0, dtype=np.float32)
                        for pending_frame in self.resume_pending_frames:
                            self._append_segment_audio(pending_frame)
                        self.resume_pending_frames = []
                        self._decode_ready(abort)
                        if self._should_commit_segment():
                            self._commit_segment(abort, reset_state=True)
                    else:
                        self.vad_state = "SPEECH"
                        self.silence_ms = 0.0
                        self._append_segment_audio(frame)
                        self._decode_ready(abort)
                        if self._should_commit_segment():
                            self._commit_segment(abort, reset_state=True)
                else:
                    if self.pause_skipping and self.resume_pending_frames:
                        # 单帧瞬态噪声未能延续为有效复讲：退回静音统计并并入环形缓冲，保持跳过状态。
                        for pending_frame in self.resume_pending_frames:
                            self._push_preroll(pending_frame)
                            self.silence_ms += VAD_FRAME_MS
                            self.speech_samples = max(0, self.speech_samples - VAD_FRAME)
                        self.resume_pending_frames = []
                    self.silence_ms += VAD_FRAME_MS
                    if not self.pause_skipping and self.silence_ms > PAUSE_SKIP_MS:
                        # 句中连续静音超过 640 ms：停止向模型喂静音，改用 256 ms 环形缓冲待机，消除停顿幻觉与算力浪费。
                        self.pause_skipping = True
                        self.preroll_buf = frame
                    elif self.pause_skipping:
                        self._push_preroll(frame)
                    else:
                        self._append_segment_audio(frame)
                        self._decode_ready(abort)
                    if self._should_commit_segment():
                        self._commit_segment(abort, reset_state=True)

        self.vad_pending = np.ascontiguousarray(buf[offset:]) if offset < total else np.zeros(0, dtype=np.float32)

    def finish(self, abort: threading.Event) -> str:
        if not self.heard_speech:
            return ""
        np = self.rt.np
        if self.resume_pending_frames:
            # 停录时未凑满滞回帧数的单帧瞬态尖峰不喂给解码器，并同步扣回语音时长统计。
            self.speech_samples = max(0, self.speech_samples - VAD_FRAME * len(self.resume_pending_frames))
            self.resume_pending_frames = []
        # 将末尾不足一帧（< 512 采样）的残留音频归入当前活跃段（长停顿跳过期间忽略残余静音）。
        if self.segment_has_speech and len(self.vad_pending) and not self.pause_skipping:
            self._append_segment_audio(self.vad_pending)
        self.vad_pending = np.zeros(0, dtype=np.float32)
        if self.segment_has_speech:
            self._commit_segment(abort, reset_state=False)
        return join_segments(self.committed_segments)

    def stats(self) -> dict:
        """返回最终帧的可选统计与分段元数据（向后兼容协议 v1）。"""
        return {
            "segments": list(self.committed_segments),
            "audio_ms": round(self.total_audio_samples * 1000 / SAMPLE_RATE),
            "speech_ms": round(self.speech_samples * 1000 / SAMPLE_RATE),
            "asr_ms": round(self.asr_elapsed_sec * 1000),
            "segment_count": len(self.committed_segments),
        }


def model_dir_error(path: str) -> str | None:
    d = Path(path)
    if not (d / "config.json").is_file():
        return f"model files are missing: {path}"
    if (d / "model.safetensors.index.json").is_file():
        # 分片模型不能与旧单文件共存，否则 mlx 会一并加载导致权重覆盖冲突。
        if (d / "model.safetensors").exists():
            return f"model files are missing: {path}"
        shards = (
            d / "model-00001-of-00002.safetensors",
            d / "model-00002-of-00002.safetensors",
        )
        if not all(shard.is_file() for shard in shards):
            return f"model files are missing: {path}"
        return None
    if not (d / "model.safetensors").is_file():
        return f"model files are missing: {path}"
    return None


class Worker:
    def __init__(
        self,
        rt: Runtime,
        idle_exit: float,
        chunk_sec: float,
        model_idle_sec: float | None = None,
    ) -> None:
        self.rt = rt
        self.idle_exit = idle_exit
        self.chunk_sec = chunk_sec
        self.model_idle_sec = resolve_model_idle_seconds() if model_idle_sec is None else float(model_idle_sec)
        self.q_event = threading.Event()
        self.pending: list = []
        self.lock = threading.Lock()
        self.queued_bytes = 0
        self.abort = threading.Event()
        self.session: DictationSession | None = None
        self.model_path: str | None = None
        self.model_session = None
        self.last_request = time.monotonic()
        self.last_session_end = time.monotonic()

    # ---- 读取线程 ----
    def _enqueue(self, item: tuple, size: int) -> None:
        with self.lock:
            self.pending.append((item, size))
            self.queued_bytes += size
            self.q_event.set()

    def read_loop(self) -> None:
        stdin = sys.stdin.buffer
        while True:
            line = stdin.readline(MAX_LINE_BYTES + 2)
            if not line:
                self._push_control(("eof",))
                return
            if not line.endswith(b"\n") and len(line) > MAX_LINE_BYTES:
                while True:
                    rest = stdin.readline(MAX_LINE_BYTES)
                    if not rest or rest.endswith(b"\n"):
                        break
                emit({"type": "error", "message": "request too large"})
                continue
            if len(line.rstrip(b"\r\n")) > MAX_LINE_BYTES:
                emit({"type": "error", "message": "request too large"})
                continue
            if not line.strip():
                continue
            try:
                request = json.loads(line)
                if not isinstance(request, dict):
                    raise ValueError("request is not an object")
            except (ValueError, UnicodeDecodeError) as error:
                emit({"type": "error", "message": f"invalid JSON request: {error}"})
                continue
            self.last_request = time.monotonic()
            op = request.get("op")
            if op == "cancel":
                self._cancel()
            else:
                with self.lock:
                    over = self.queued_bytes + len(line) > MAX_QUEUE_BYTES
                if over:
                    self._overflow()
                else:
                    self._enqueue(("req", request), len(line))

    def _push_control(self, item: tuple) -> None:
        with self.lock:
            self.abort.set()
            self.pending.append((item, 0))
            self.q_event.set()

    def _cancel(self) -> None:
        with self.lock:
            kept = [p for p in self.pending if not (p[0][0] == "req" and p[0][1].get("op") in ("audio", "finish"))]
            self.pending = kept
            self.queued_bytes = sum(size for _, size in kept)
            self.abort.set()
            self.pending.append((("cancel",), 0))
            self.q_event.set()

    def _overflow(self) -> None:
        with self.lock:
            self.pending = []
            self.queued_bytes = 0
            self.abort.set()
            self.pending.append((("overflow",), 0))
            self.q_event.set()

    def _next(self, timeout: float | None):
        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            with self.lock:
                if self.pending:
                    item, size = self.pending.pop(0)
                    self.queued_bytes -= size
                    return item
                self.q_event.clear()
            remaining = None if deadline is None else deadline - time.monotonic()
            if remaining is not None and remaining <= 0:
                return None
            self.q_event.wait(remaining)

    # ---- 工作线程 ----
    def run(self) -> int:
        while True:
            item = self._next(self._wait_timeout())
            if item is None:
                if self._housekeeping():
                    return 0
                continue
            kind = item[0]
            if kind == "eof":
                return 0
            try:
                if kind == "req":
                    self.handle(item[1])
                elif kind == "cancel":
                    self.abort.clear()
                    self._end_session("cancelled")
                elif kind == "overflow":
                    self.abort.clear()
                    self._end_session("error", "request queue full")
            except Aborted:
                pass
            except Exception as error:  # noqa: BLE001
                self._fail(error)

    def _effective_release_seconds(self) -> float:
        if self.model_idle_sec <= 0:
            return 0.0
        if self.idle_exit > 0:
            return min(self.model_idle_sec, self.idle_exit)
        return self.model_idle_sec

    def _wait_timeout(self) -> float:
        now = time.monotonic()
        candidates = []
        if self.session is None:
            release = self._effective_release_seconds()
            if self.model_session is not None and release > 0:
                candidates.append(self.last_session_end + release - now)
            if self.idle_exit > 0:
                candidates.append(self.last_request + self.idle_exit - now)
        return max(0.05, min(candidates)) if candidates else 3600.0

    def _housekeeping(self) -> bool:
        if self.session is not None:
            return False
        now = time.monotonic()
        release = self._effective_release_seconds()
        if self.model_session is not None and release > 0 and now - self.last_session_end >= release:
            self.release()
        if self.idle_exit > 0 and now - self.last_request >= self.idle_exit:
            return True
        return False

    def release(self) -> None:
        self.model_session = None
        self.model_path = None
        if self.rt.available:
            self.rt.mx.clear_cache()

    def _end_session(self, kind: str, message: str = "") -> None:
        session, self.session = self.session, None
        self.last_session_end = time.monotonic()
        if session is None:
            return
        sid = session.id
        if kind == "cancelled":
            emit({"type": "cancelled", "id": sid})
        else:
            emit({"type": "error", "id": sid, "message": message})
        self.rt.mx.clear_cache()

    def _fail(self, error: Exception) -> None:
        if self.session is not None:
            self._end_session("error", f"{type(error).__name__}: {error}")
        else:
            emit({"type": "error", "message": f"{type(error).__name__}: {error}"})

    def handle(self, request: dict) -> None:
        op = request.get("op")
        if op == "ping":
            emit({"type": "pong", "id": request.get("id"), "available": self.rt.available})
        elif op == "release":
            if self.session is None:
                self.release()
        elif op == "start":
            self.start(request)
        elif op == "audio":
            if self.session is None:
                return
            try:
                raw = base64.b64decode(request.get("pcm16", ""), validate=True)
                if len(raw) % 2:
                    raise ValueError("PCM payload has an odd number of bytes")
            except (binascii.Error, ValueError, TypeError) as error:
                self._end_session("error", f"invalid audio payload: {error}")
                return
            try:
                self.session.push(raw, self.abort)
            except Aborted:
                raise
            except Exception as error:  # noqa: BLE001
                self._end_session("error", f"{type(error).__name__}: {error}")
        elif op == "finish":
            if self.session is None:
                return
            session = self.session
            try:
                text = session.finish(self.abort)
            except Aborted:
                raise
            except Exception as error:  # noqa: BLE001
                self._end_session("error", f"{type(error).__name__}: {error}")
                return
            stats = session.stats()
            self.session = None
            self.last_session_end = time.monotonic()
            emit(
                {
                    "type": "final",
                    "id": session.id,
                    "text": text,
                    "segments": stats["segments"],
                    "audio_ms": stats["audio_ms"],
                    "speech_ms": stats["speech_ms"],
                    "asr_ms": stats["asr_ms"],
                    "segment_count": stats["segment_count"],
                }
            )
            self.rt.mx.clear_cache()
        else:
            emit({"type": "error", "message": f"unknown op: {op}"})

    def start(self, request: dict) -> None:
        sid = request.get("id")
        self.session = None
        try:
            if not self.rt.available:
                raise RuntimeError(f"runtime unavailable: {self.rt.error}")
            model = request.get("model")
            if not isinstance(model, str) or not model:
                raise ValueError("model is required")
            problem = model_dir_error(model)
            if problem:
                raise ValueError(problem)
            if self.model_path != model or self.model_session is None:
                self.model_session = None
                self.rt.mx.clear_cache()
                self.model_session = self.rt.Session(model=model)
                self.model_path = model
                self.rt.mx.clear_cache()
            language = request.get("language") or None
            hotwords = [w for w in (request.get("hotwords") or []) if isinstance(w, str) and w]
            self.rt.budget.reset()
            self.session = DictationSession(
                self.rt, sid, self.model_session, language, ", ".join(hotwords), self.chunk_sec
            )
        except Exception as error:  # noqa: BLE001
            emit({"type": "error", "id": sid, "message": f"{type(error).__name__}: {error}"})
            return
        emit({"type": "started", "id": sid})


def parent_watch(parent: int) -> None:
    while True:
        if os.getppid() != parent:
            os._exit(0)
        time.sleep(1)


def read_wav(path: str):
    with wave.open(path, "rb") as w:
        if w.getframerate() != SAMPLE_RATE or w.getnchannels() != 1 or w.getsampwidth() != 2:
            raise ValueError("WAV must be 16 kHz mono 16-bit PCM")
        return w.readframes(w.getnframes())


def transcribe_file(rt: Runtime, args) -> int:
    if not rt.available:
        print(f"runtime unavailable: {rt.error}", file=sys.stderr)
        return 1
    problem = model_dir_error(args.model)
    if problem:
        print(problem, file=sys.stderr)
        return 1
    global _quiet
    pcm = read_wav(args.wav)
    model_session = rt.Session(model=args.model)
    language = args.language or None
    context = ", ".join(args.hotword)
    if args.offline:
        audio = rt.np.frombuffer(pcm, dtype="<i2").astype(rt.np.float32) / 32768.0
        result = model_session.transcribe(audio, language=language, context=context)
        text = result.text
    else:
        _quiet = True
        session = DictationSession(rt, 0, model_session, language, context, args.chunk_sec)
        step = SAMPLE_RATE // 10 * 2
        abort = threading.Event()
        for i in range(0, len(pcm), step):
            session.push(pcm[i : i + step], abort)
        text = session.finish(abort)
    _quiet = False
    _out_file.write(text + "\n")
    _out_file.flush()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime")
    parser.add_argument("--idle-exit", type=int, default=None)
    parser.add_argument("--model")
    parser.add_argument("--wav")
    parser.add_argument("--language")
    parser.add_argument("--hotword", action="append", default=[])
    parser.add_argument("--chunk-sec", type=float, default=DEFAULT_CHUNK_SEC)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    if args.idle_exit is not None and args.idle_exit < 0:
        parser.error("--idle-exit must be non-negative")
    idle_exit, model_idle = resolve_idle_timeouts(args.idle_exit)
    claim_stdout()
    rt = Runtime()
    rt.load()
    if args.model or args.wav:
        if not (args.model and args.wav):
            parser.error("--model and --wav go together")
        return transcribe_file(rt, args)
    emit({"type": "hello", "version": PROTOCOL_VERSION, "available": rt.available, "error": rt.error})
    try:
        os.nice(10)
    except OSError:
        pass
    threading.Thread(target=parent_watch, args=(os.getppid(),), daemon=True).start()
    worker = Worker(rt, idle_exit, args.chunk_sec, model_idle_sec=model_idle)
    reader = threading.Thread(target=worker.read_loop, daemon=True)
    reader.start()
    code = worker.run()
    _out_file.flush()
    os._exit(code)


if __name__ == "__main__":
    raise SystemExit(main())
