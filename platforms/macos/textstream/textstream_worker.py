"""TextStream 本地流式识别工作进程，实现 msime-voice-local 的 stdin/stdout JSON 行协议。

协议见 shurufa/shared/voice/README.md 的 "The msime-voice-local helper"。

出处与许可：
- 适配器结构、Metal 内存与 CPU 预算、VAD 门控参考 OpenLess 的 TextStream 适配器
  （OpenLess，AGPL-3.0）。
- 流式解码设置沿用 textstream-asr（199-biotechnologies/textstream-asr，
  rev cacf67acadbd90789b631d9ee8f2e76f1a6cd7c2，MIT）。
- 解码由 mlx-qwen3-asr 0.4.4（Apache-2.0）完成，模型为 Qwen/Qwen3-ASR-0.6B。

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
MODEL_RELEASE_SECONDS = 120
VAD_FRAME = 512
VAD_THRESHOLD = 0.4
PREROLL_SAMPLES = SAMPLE_RATE // 2
DEFAULT_CHUNK_SEC = float(os.environ.get("SHURUFA_TEXTSTREAM_CHUNK_SEC", "0.5"))
MAX_CONTEXT_SEC = 30.0

_out_file = None
_out_lock = threading.Lock()
_quiet = False


def emit(event: dict) -> None:
    if _quiet:
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


def metal_memory_limits(memory_bytes: int, recommended_bytes: int) -> tuple[int, int]:
    """给宿主和前台应用留出内存；MLX 的内存上限只是建议值，不是系统配额。"""
    if memory_bytes <= 0 or recommended_bytes <= 0:
        raise ValueError("Metal memory information is unavailable")
    memory_limit = min(4 * 1024**3, memory_bytes // 3, recommended_bytes)
    cache_limit = min(128 * 1024**2, memory_limit // 16)
    return memory_limit, cache_limit


def configure_cpu_threads() -> float:
    """在导入 NumPy/MLX/VAD 之前设定原生库线程数。"""
    cores = float(os.environ.get("SHURUFA_TEXTSTREAM_CPU_CORES", "1"))
    if not math.isfinite(cores) or not 0 < cores <= (os.cpu_count() or 1):
        raise ValueError("invalid CPU budget")
    cores = min(cores, 2.0)
    threads = str(max(1, min(2, math.floor(cores))))
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
    """一次听写：VAD 门控 + 解码器流式状态。"""

    def __init__(self, rt: Runtime, sid, model_session, language, context, chunk_sec):
        self.rt = rt
        self.id = sid
        self.model_session = model_session
        np = rt.np
        self.state = model_session.init_streaming(
            chunk_size_sec=chunk_sec,
            max_context_sec=MAX_CONTEXT_SEC,
            sample_rate=SAMPLE_RATE,
            unfixed_chunk_num=2,
            unfixed_token_num=5,
            language=language,
            context=context,
            finalization_mode="accuracy",
        )
        self.chunk_samples = self.state.chunk_size_samples
        self.vad = rt.SileroVAD(SAMPLE_RATE)
        self.heard_speech = False
        self.pre = np.zeros(0, dtype=np.float32)
        self.vad_pos = 0
        self.feed_buf = np.zeros(0, dtype=np.float32)
        self.last_text = ""

    def _snapshot(self, abort: threading.Event) -> None:
        text = self.state.text
        if text == self.last_text:
            return
        self.last_text = text
        stable = self.state.stable_text
        if not text.startswith(stable):
            stable = os.path.commonprefix([stable, text])
        emit(
            {
                "type": "partial",
                "id": self.id,
                "text": text,
                "finalized": stable,
                "draft": text[len(stable):],
            }
        )

    def _decode_ready(self, abort: threading.Event) -> None:
        rt = self.rt
        while len(self.feed_buf) >= self.chunk_samples:
            if abort.is_set():
                raise Aborted
            piece = self.feed_buf[: self.chunk_samples]
            self.feed_buf = self.feed_buf[self.chunk_samples :]
            self.state = self.model_session.feed_audio(piece, self.state)
            rt.mx.clear_cache()
            rt.budget.checkpoint(abort)
            self._snapshot(abort)

    def push(self, raw: bytes, abort: threading.Event) -> None:
        rt = self.rt
        np = rt.np
        audio = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
        if self.heard_speech:
            self.feed_buf = np.concatenate([self.feed_buf, audio])
            self._decode_ready(abort)
            return
        self.pre = np.concatenate([self.pre, audio])
        while self.vad_pos + VAD_FRAME <= len(self.pre):
            frame = np.ascontiguousarray(self.pre[self.vad_pos : self.vad_pos + VAD_FRAME])
            hit = self.vad.process(memoryview(frame.data)) >= VAD_THRESHOLD
            start = self.vad_pos
            self.vad_pos += VAD_FRAME
            if hit:
                self.heard_speech = True
                self.feed_buf = self.pre[max(0, start - PREROLL_SAMPLES) :]
                self.pre = np.zeros(0, dtype=np.float32)
                self._decode_ready(abort)
                return
        drop = len(self.pre) - self.vad_pos - PREROLL_SAMPLES
        if drop > 0:
            self.pre = self.pre[drop:]
            self.vad_pos -= drop

    def finish(self, abort: threading.Event) -> str:
        if not self.heard_speech:
            return ""
        rt = self.rt
        if len(self.feed_buf):
            if abort.is_set():
                raise Aborted
            self.state = self.model_session.feed_audio(self.feed_buf, self.state)
            self.feed_buf = self.feed_buf[:0]
        if abort.is_set():
            raise Aborted
        self.state = self.model_session.finish_streaming(self.state)
        rt.mx.clear_cache()
        rt.budget.checkpoint()
        return self.state.text


def model_dir_error(path: str) -> str | None:
    d = Path(path)
    if not (d / "config.json").is_file() or not (d / "model.safetensors").is_file():
        return f"model files are missing: {path}"
    return None


class Worker:
    def __init__(self, rt: Runtime, idle_exit: int, chunk_sec: float) -> None:
        self.rt = rt
        self.idle_exit = idle_exit
        self.chunk_sec = chunk_sec
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

    # ---- reader thread ----
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

    # ---- worker thread ----
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

    def _wait_timeout(self) -> float:
        now = time.monotonic()
        candidates = []
        if self.session is None:
            if self.model_session is not None:
                release = MODEL_RELEASE_SECONDS
                if self.idle_exit:
                    release = min(release, self.idle_exit)
                candidates.append(self.last_session_end + release - now)
            if self.idle_exit:
                candidates.append(self.last_request + self.idle_exit - now)
        return max(0.05, min(candidates)) if candidates else 3600.0

    def _housekeeping(self) -> bool:
        if self.session is not None:
            return False
        now = time.monotonic()
        release = MODEL_RELEASE_SECONDS
        if self.idle_exit:
            release = min(release, self.idle_exit)
        if self.model_session is not None and now - self.last_session_end >= release:
            self.release()
        if self.idle_exit and now - self.last_request >= self.idle_exit:
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
            self.session = None
            self.last_session_end = time.monotonic()
            emit({"type": "final", "id": session.id, "text": text})
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
    parser.add_argument("--idle-exit", type=int, default=DEFAULT_IDLE_EXIT_SECONDS)
    parser.add_argument("--model")
    parser.add_argument("--wav")
    parser.add_argument("--language")
    parser.add_argument("--hotword", action="append", default=[])
    parser.add_argument("--chunk-sec", type=float, default=DEFAULT_CHUNK_SEC)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    if args.idle_exit < 0:
        parser.error("--idle-exit must be non-negative")
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
    worker = Worker(rt, args.idle_exit, args.chunk_sec)
    reader = threading.Thread(target=worker.read_loop, daemon=True)
    reader.start()
    code = worker.run()
    _out_file.flush()
    os._exit(code)


if __name__ == "__main__":
    raise SystemExit(main())
