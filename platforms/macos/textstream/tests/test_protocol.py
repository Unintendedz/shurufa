#!/usr/bin/env python3
"""msime-voice-local 协议一致性与性能实测驱动。

用法：
  python3 test_protocol.py unit                       # 纯确定性单元测试（无需已安装 MLX 运行时/模型）
  python3 test_protocol.py all [--chunk-sec 0.5]      # 单元测试 + 全部真实 worker 协议用例，结果写入 logs/protocol-results.json
  python3 test_protocol.py sweep 0.5 0.8 1.0 1.5 2.5  # 按解码间隔扫描延迟/准确率/CPU/内存
"""

from __future__ import annotations

import argparse
import array
import base64
import io
import json
import os
import queue
import resource
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata
import wave
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORKER_DIR = HERE.parent
LAUNCHER = WORKER_DIR / "shurufa-textstream"
ROOT = Path(os.environ.get("SHURUFA_TEXTSTREAM_ROOT") or Path.home() / "Library/Application Support/Shurufa/textstream")
MODEL = ROOT / "model"
LOGS = Path(os.environ.get("TEXTSTREAM_TEST_LOGS") or tempfile.mkdtemp(prefix="textstream-test-"))
SENTENCES = [
    "今天天气很好，我们一起去公园散步吧。",
    "请把这份文件发给张经理，明天上午十点开会。",
    "输入法的语音识别功能需要在本地运行，不能把音频上传到服务器。",
]
LEAD_SILENCE_SEC = 1.0


def read_pcm(name: str) -> bytes:
    with wave.open(str(HERE / "audio" / name), "rb") as w:
        assert (w.getframerate(), w.getnchannels(), w.getsampwidth()) == (16000, 1, 2)
        return w.readframes(w.getnframes())


def normalize(text: str) -> str:
    return "".join(c for c in text if unicodedata.category(c)[0] in "LN")


def edit_distance(a: str, b: str) -> int:
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def cer(ref: str, hyp: str) -> float:
    ref, hyp = normalize(ref), normalize(hyp)
    return edit_distance(ref, hyp) / max(1, len(ref))


class Helper:
    def __init__(self, *args: str, env: dict | None = None):
        e = dict(os.environ)
        e.update(env or {})
        self.proc = subprocess.Popen(
            [str(LAUNCHER), *args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=open(LOGS / "worker-stderr.log", "ab"),
            env=e,
        )
        self.t0 = time.monotonic()
        self.events: queue.Queue = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            self.events.put((time.monotonic(), line.rstrip(b"\n")))
        self.events.put((time.monotonic(), None))

    def send_raw(self, data: bytes):
        self.proc.stdin.write(data)
        self.proc.stdin.flush()

    def send(self, obj: dict):
        self.send_raw(json.dumps(obj).encode() + b"\n")

    def audio(self, pcm: bytes):
        self.send({"op": "audio", "pcm16": base64.b64encode(pcm).decode()})

    def get(self, timeout=60.0):
        try:
            t, line = self.events.get(timeout=timeout)
        except queue.Empty:
            return None, None
        if line is None:
            return t, "EOF"
        return t, json.loads(line)

    def until(self, kind: str, timeout=60.0):
        """读取直到指定类型；返回 (事件列表, 命中事件, 命中时间)。"""
        seen = []
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            t, ev = self.get(max(0.01, end - time.monotonic()))
            if ev is None:
                break
            seen.append((t, ev))
            if ev == "EOF" or ev.get("type") == kind:
                return seen, ev, t
        return seen, None, None

    def close(self) -> dict:
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        return {}


def realtime_feed(h: Helper, pcm: bytes, block_ms=100, on_tick=None):
    step = 16000 * 2 * block_ms // 1000
    start = time.monotonic()
    for i, off in enumerate(range(0, len(pcm), step)):
        h.audio(pcm[off : off + step])
        if on_tick:
            on_tick(off + step)
        target = start + (i + 1) * block_ms / 1000
        delay = target - time.monotonic()
        if delay > 0:
            time.sleep(delay)
    return start


def sample_rss(pid: int, stop: threading.Event, out: list):
    while not stop.is_set():
        r = subprocess.run(["ps", "-o", "rss=,time=", "-p", str(pid)], capture_output=True, text=True)
        if r.stdout.strip():
            rss = int(r.stdout.split()[0]) * 1024
            out.append(rss)
        time.sleep(0.5)


def run_dictation(pcm: bytes, reference: str, chunk_sec: float, label: str) -> dict:
    """按实时速度喂一整段录音，返回延迟、文本、资源指标。"""
    h = Helper("--chunk-sec", str(chunk_sec))
    _, hello, _ = h.until("hello", 60)
    assert hello and hello["available"], hello
    rss: list = []
    stop = threading.Event()
    threading.Thread(target=sample_rss, args=(h.proc.pid, stop, rss), daemon=True).start()
    t_load = time.monotonic()
    h.send({"op": "start", "id": 1, "model": str(MODEL), "language": "zh-CN"})
    _, started, t_started = h.until("started", 120)
    assert started, "no started"
    load_s = t_started - t_load
    cpu0 = proc_cpu(h.proc.pid)
    audio_fed = [0]
    feed_start = realtime_feed(h, pcm, on_tick=lambda n: audio_fed.__setitem__(0, n))
    t_finish = time.monotonic()
    h.send({"op": "finish"})
    seen, final, t_final = h.until("final", 60)
    cpu1 = proc_cpu(h.proc.pid)
    stop.set()
    partials = [(t - feed_start, ev) for t, ev in seen if isinstance(ev, dict) and ev["type"] == "partial"]
    timeline = [(round(t, 2), ev["text"]) for t, ev in partials]
    dup = sum(1 for a, b in zip(timeline, timeline[1:]) if a[1] == b[1])
    speech_onset = LEAD_SILENCE_SEC
    first = partials[0][0] if partials else None
    audio_dur = len(pcm) / 32000
    h.close()
    maxrss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    result = {
        "label": label,
        "chunk_sec": chunk_sec,
        "model_load_and_start_s": round(load_s, 2),
        "audio_s": round(audio_dur, 2),
        "partial_count": len(partials),
        "duplicate_consecutive_partials": dup,
        "first_partial_after_speech_onset_s": None if first is None else round(first - speech_onset, 2),
        "finish_to_final_s": None if final is None else round(t_final - t_finish, 2),
        "final_text": None if final is None else final["text"],
        "reference": reference,
        "cer": None if final is None else round(cer(reference, final["text"]), 4),
        "peak_rss_mb_ps_sampled": round(max(rss) / 1e6, 0) if rss else None,
        "peak_rss_mb_rusage": round(maxrss / 1e6, 0),
        "cpu_s_during_stream": None if cpu0 is None or cpu1 is None else round(cpu1 - cpu0, 2),
        "cpu_pct_of_one_core": None
        if cpu0 is None or cpu1 is None
        else round(100 * (cpu1 - cpu0) / (time.monotonic() - feed_start), 1),
        "timeline": timeline,
    }
    return result


def proc_cpu(pid: int):
    r = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True)
    s = r.stdout.strip()
    if not s:
        return None
    mins, rest = s.split(":", 1)
    return int(mins) * 60 + float(rest)


def combined_pcm() -> bytes:
    sil = lambda s: b"\0" * int(32000 * s)
    return (
        sil(LEAD_SILENCE_SEC)
        + read_pcm("s1.wav")
        + sil(0.8)
        + read_pcm("s2.wav")
        + sil(0.8)
        + read_pcm("s3.wav")
        + sil(1.0)
    )


def sweep(values: list[float]) -> list[dict]:
    out = []
    for v in values:
        proc = subprocess.run([sys.executable, __file__, "one", str(v)], capture_output=True, text=True)
        r = json.loads(proc.stdout.strip().splitlines()[-1])
        print(json.dumps({k: r[k] for k in r if k not in ("timeline",)}, ensure_ascii=False), flush=True)
        out.append(r)
    (LOGS / "sweep-results.json").write_text(json.dumps(out, ensure_ascii=False, indent=1))
    return out


# ---------------- 纯确定性单元测试（无需 GPU / 模型即可秒跑） ----------------


class _FakeFloatArray:
    """轻量 float32 数组替身，使单元测试不依赖宿主 Python 环境安装 NumPy。"""

    def __init__(self, values):
        self._arr = values if isinstance(values, array.array) else array.array("f", values)

    @property
    def data(self):
        return memoryview(self._arr)

    def astype(self, _dtype):
        return self

    def __truediv__(self, denom):
        return _FakeFloatArray([v / denom for v in self._arr])

    def __len__(self):
        return len(self._arr)

    def __getitem__(self, item):
        if isinstance(item, slice):
            return _FakeFloatArray(self._arr[item])
        return self._arr[item]

    def __iter__(self):
        return iter(self._arr)


class _FakeNp:
    float32 = float

    @staticmethod
    def zeros(n: int, dtype=None):
        _ = dtype
        return _FakeFloatArray([0.0] * n)

    @staticmethod
    def frombuffer(raw: bytes, dtype="<i2"):
        _ = dtype
        count = len(raw) // 2
        vals = struct.unpack(f"<{count}h", raw) if count else ()
        return _FakeFloatArray(vals)

    @staticmethod
    def concatenate(parts):
        merged = array.array("f")
        for p in parts:
            merged.extend(p._arr)
        return _FakeFloatArray(merged)

    @staticmethod
    def ascontiguousarray(arr):
        return arr


class _FakeMx:
    def __init__(self) -> None:
        self.clear_calls = 0

    def clear_cache(self) -> None:
        self.clear_calls += 1


class _FakeBudget:
    def reset(self) -> None:
        pass

    def checkpoint(self, abort=None) -> None:
        _ = abort


class _EnergySileroVAD:
    """按帧幅值模拟 Silero VAD：幅值 >= 0.05 视为语音（0.9），否则静音（0.0）。"""

    def __init__(self, sample_rate: int) -> None:
        assert sample_rate == 16000

    def process(self, view: memoryview) -> float:
        floats = array.array("f")
        floats.frombytes(view.tobytes())
        peak = max((abs(x) for x in floats), default=0.0)
        return 0.9 if peak >= 0.05 else 0.0


class _FakeStreamState:
    def __init__(self, seg_index: int, chunk_samples: int) -> None:
        self.seg_index = seg_index
        self.chunk_size_samples = chunk_samples
        self.samples_fed = 0
        self.text = ""
        self.stable_text = ""


class _FakeModelSession:
    """可观测流式解码器替身：记录每段实际喂入的采样数与切段次数，产出合成文本。"""

    def __init__(self, labels: list[str] | None = None, chunk_samples: int = 8000) -> None:
        self.labels = labels or [f"合成第{i}段。" for i in range(1, 10)]
        self.chunk_samples = chunk_samples
        self.init_calls = 0
        self.feed_calls = 0
        self.finish_calls = 0
        self.total_fed_samples = 0
        self.segment_fed_history: list[int] = []

    def init_streaming(self, **kwargs):
        assert kwargs["max_context_sec"] == 30.0
        self.init_calls += 1
        return _FakeStreamState(self.init_calls, self.chunk_samples)

    def feed_audio(self, piece, state: _FakeStreamState):
        self.feed_calls += 1
        n = len(piece)
        state.samples_fed += n
        self.total_fed_samples += n
        label = self.labels[(state.seg_index - 1) % len(self.labels)]
        # 前半段置入 stable_text，后半段留作 draft，检验双态预览拆分。
        split = max(1, len(label) // 2)
        state.stable_text = label[:split]
        state.text = label
        return state

    def finish_streaming(self, state: _FakeStreamState):
        self.finish_calls += 1
        self.segment_fed_history.append(state.samples_fed)
        label = self.labels[(state.seg_index - 1) % len(self.labels)]
        state.stable_text = label
        state.text = label
        return state


def _synth_pcm(seconds: float, amplitude: int) -> bytes:
    """生成指定时长与幅值的 16 kHz 单声道 16-bit 合成 PCM（0 表示纯静音）。"""
    count = int(round(16000 * seconds))
    if amplitude == 0:
        return b"\0\0" * count
    return struct.pack("<h", amplitude) * count


def _load_worker_module():
    if str(WORKER_DIR) not in sys.path:
        sys.path.insert(0, str(WORKER_DIR))
    import textstream_worker  # noqa: PLC0415

    return textstream_worker


def _make_fake_runtime(tw):
    rt = tw.Runtime()
    rt.mx = _FakeMx()
    rt.np = _FakeNp()
    rt.SileroVAD = _EnergySileroVAD
    rt.budget = _FakeBudget()
    rt.Session = _FakeModelSession
    return rt


def _capture_events(tw, fn):
    buf = io.StringIO()
    old_out, old_quiet = tw._out_file, tw._quiet
    tw._out_file = buf
    tw._quiet = False
    try:
        ret = fn()
    finally:
        tw._out_file = old_out
        tw._quiet = old_quiet
    events = [json.loads(line) for line in buf.getvalue().splitlines() if line.strip()]
    return ret, events


# ---------------- 协议用例与单元测试 ----------------

results: list[dict] = []


def case(name):
    def deco(fn):
        def run(*a, **k):
            t = time.monotonic()
            try:
                detail = fn(*a, **k)
                results.append({"case": name, "ok": True, "detail": detail, "s": round(time.monotonic() - t, 2)})
                print(f"PASS {name}: {detail}", flush=True)
            except AssertionError as e:
                results.append({"case": name, "ok": False, "detail": str(e), "s": round(time.monotonic() - t, 2)})
                print(f"FAIL {name}: {e}", flush=True)

        return run

    return deco


@case("unit: >30s multi-segment commit & monotonic finalized/draft")
def u_long_audio_over_30s():
    tw = _load_worker_module()
    rt = _make_fake_runtime(tw)
    model = _FakeModelSession(labels=["第一段话，", "第二段话，", "第三段结束。"])
    session = tw.DictationSession(rt, 101, model, "zh-CN", "", 0.5)
    abort = threading.Event()
    # 46.1 s 合成音频：0.5s静音 + 19s语音 + 1.3s停顿（触发>=18s&>=1200ms切段）+ 19s语音 + 1.3s停顿 + 5.0s语音
    pcm = (
        _synth_pcm(0.5, 0)
        + _synth_pcm(19.0, 6000)
        + _synth_pcm(1.3, 0)
        + _synth_pcm(19.0, 6000)
        + _synth_pcm(1.3, 0)
        + _synth_pcm(5.0, 6000)
    )

    def drive():
        step = 3200  # 100 ms
        for off in range(0, len(pcm), step):
            session.push(pcm[off : off + step], abort)
        return session.finish(abort)

    final_text, events = _capture_events(tw, drive)
    stats = session.stats()
    assert stats["segment_count"] == 3, stats
    assert stats["segments"] == ["第一段话，", "第二段话，", "第三段结束。"], stats
    assert final_text == "第一段话，第二段话，第三段结束。", final_text
    assert stats["audio_ms"] == 46100, stats
    assert all(fed < 29 * 16000 for fed in model.segment_fed_history), model.segment_fed_history
    partials = [e for e in events if e["type"] == "partial"]
    assert partials, "expected partial events"
    prev_finalized = ""
    for ev in partials:
        assert ev["finalized"] + ev["draft"] == ev["text"], ev
        assert ev["finalized"].startswith(prev_finalized), (prev_finalized, ev["finalized"])
        prev_finalized = ev["finalized"]
    return f"38s -> {stats['segment_count']} segments, max_seg_samples={max(model.segment_fed_history)}, {len(partials)} partials"


@case("unit: 24s short pause, 26s micro-pause & 28s hard cap commit")
def u_warn_and_hard_cap():
    tw = _load_worker_module()
    rt = _make_fake_runtime(tw)
    # 场景 1：24.5s 连续语音 + 0.4s 短停顿（>=320ms）触发软上限切段
    model1 = _FakeModelSession(labels=["Hello world.", "Next sentence."])
    s1 = tw.DictationSession(rt, 102, model1, "en", "", 0.5)
    abort = threading.Event()
    pcm1 = _synth_pcm(24.5, 6000) + _synth_pcm(0.4, 0) + _synth_pcm(2.0, 6000)
    text1, _ = _capture_events(tw, lambda: (s1.push(pcm1, abort), s1.finish(abort))[1])
    assert s1.stats()["segment_count"] == 2, s1.stats()
    # 英文段间自动补单空格
    assert text1 == "Hello world. Next sentence.", text1

    # 场景 2：26.2s 连续语音 + 96ms 微停顿（>=64ms）优先在词间切段，避免拖到 28s 硬切
    model_micro = _FakeModelSession(labels=["微停顿前，", "微停顿后。"])
    s_micro = tw.DictationSession(rt, 106, model_micro, "zh-CN", "", 0.5)
    pcm_micro = _synth_pcm(26.2, 6000) + _synth_pcm(0.096, 0) + _synth_pcm(2.0, 6000)
    text_micro, _ = _capture_events(tw, lambda: (s_micro.push(pcm_micro, abort), s_micro.finish(abort))[1])
    assert s_micro.stats()["segment_count"] == 2, s_micro.stats()
    assert text_micro == "微停顿前，微停顿后。", text_micro
    assert model_micro.segment_fed_history[0] < int(27.0 * 16000), model_micro.segment_fed_history

    # 场景 3：32s 零停顿连续语音，必须在 28s 硬上限处强制切段，永不触碰 30s MAX_CONTEXT_SEC
    model2 = _FakeModelSession(labels=["连续段一，", "连续段二。"])
    s2 = tw.DictationSession(rt, 103, model2, "zh-CN", "", 0.5)
    pcm2 = _synth_pcm(32.0, 6000)
    text2, _ = _capture_events(tw, lambda: (s2.push(pcm2, abort), s2.finish(abort))[1])
    assert s2.stats()["segment_count"] == 2, s2.stats()
    assert text2 == "连续段一，连续段二。", text2
    assert all(fed <= int(28.5 * 16000) for fed in model2.segment_fed_history), model2.segment_fed_history
    return f"warn-cap ok (en join={text1!r}); micro-cap ok ({model_micro.segment_fed_history}); hard-cap ok ({model2.segment_fed_history})"


@case("unit: mid-utterance >640ms silence skipped with 256ms preroll & 1-frame spike ignored")
def u_mid_utterance_silence_skip():
    tw = _load_worker_module()
    rt = _make_fake_runtime(tw)
    model = _FakeModelSession(labels=["停顿前后完整句子。"])
    session = tw.DictationSession(rt, 104, model, "zh-CN", "", 0.5)
    abort = threading.Event()
    # 帧对齐构造（每帧 32 ms = 512 采样）：开口 3.2s + 静默 2.56s + 单帧 32ms 瞬态敲击尖峰 + 静默 2.56s + 再开口 3.2s（总 11.552s）
    pcm = (
        _synth_pcm(3.2, 6000)
        + _synth_pcm(2.56, 0)
        + _synth_pcm(0.032, 6000)
        + _synth_pcm(2.56, 0)
        + _synth_pcm(3.2, 6000)
    )
    text, _ = _capture_events(tw, lambda: (session.push(pcm, abort), session.finish(abort))[1])
    stats = session.stats()
    assert text == "停顿前后完整句子。", text
    assert stats["segment_count"] == 1 and model.finish_calls == 1, stats
    assert 11500 <= stats["audio_ms"] <= 11600, stats
    assert 6200 <= stats["speech_ms"] <= 6600, stats
    # 单帧尖峰不得打断 pause_skipping 灌入额外静音：实际喂给模型应保持在 ~7.30s（若误唤醒会跃升至 ~8.22s）
    fed_sec = model.total_fed_samples / 16000.0
    assert 7.0 <= fed_sec <= 7.6, f"expected ~7.30s fed (silence & 1-frame spike skipped), got {fed_sec:.2f}s"
    return f"11.55s audio (with 32ms transient spike) -> {fed_sec:.2f}s fed to model, speech_ms={stats['speech_ms']}"


@case("unit: pure silence zero feed & zero partials")
def u_pure_silence():
    tw = _load_worker_module()
    rt = _make_fake_runtime(tw)
    model = _FakeModelSession()
    session = tw.DictationSession(rt, 105, model, "zh-CN", "", 0.5)
    abort = threading.Event()
    pcm = _synth_pcm(5.0, 0)
    text, events = _capture_events(tw, lambda: (session.push(pcm, abort), session.finish(abort))[1])
    assert text == "" and not events, (text, events)
    assert model.feed_calls == 0 and model.finish_calls == 0
    assert session.stats() == {"segments": [], "audio_ms": 5000, "speech_ms": 0, "asr_ms": 0, "segment_count": 0}
    return "5s pure silence -> 0 feeds, 0 partials, empty final"


@case("unit: env SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC / CPU_THREADS & release")
def u_env_and_release():
    tw = _load_worker_module()
    # 内存分级默认：>=32 GiB -> 900s，<32 GiB -> 180s
    assert tw.resolve_model_idle_seconds("", 64 * 1024**3) == 900.0
    assert tw.resolve_model_idle_seconds("", 32 * 1024**3) == 900.0
    assert tw.resolve_model_idle_seconds("", 16 * 1024**3) == 180.0
    # 环境变量显式覆盖
    assert tw.resolve_model_idle_seconds("45", 64 * 1024**3) == 45.0
    assert tw.resolve_idle_timeouts(None, "", 64 * 1024**3) == (900.0, 900.0)
    assert tw.resolve_idle_timeouts(None, "", 16 * 1024**3) == (600.0, 180.0)
    # 兼容宿主固定 --idle-exit 120 + 分级环境变量
    assert tw.resolve_idle_timeouts(120, "900", 64 * 1024**3) == (900.0, 900.0)
    # 显式短 --idle-exit（如 3s）严格生效
    assert tw.resolve_idle_timeouts(3, "", 64 * 1024**3) == (3.0, 3.0)

    # CPU 线程数：默认 2、上限 4、兼容旧 SHURUFA_TEXTSTREAM_CPU_CORES
    saved_threads = os.environ.pop("SHURUFA_TEXTSTREAM_CPU_THREADS", None)
    saved_cores = os.environ.pop("SHURUFA_TEXTSTREAM_CPU_CORES", None)
    try:
        assert tw.configure_cpu_threads() == float(min(2, os.cpu_count() or 2))
        os.environ["SHURUFA_TEXTSTREAM_CPU_CORES"] = "1.5"
        assert tw.configure_cpu_threads() == 1.5
        os.environ["SHURUFA_TEXTSTREAM_CPU_THREADS"] = "8"
        assert tw.configure_cpu_threads() <= 4.0
        assert int(os.environ["OMP_NUM_THREADS"]) <= 4
    finally:
        os.environ.pop("SHURUFA_TEXTSTREAM_CPU_THREADS", None)
        os.environ.pop("SHURUFA_TEXTSTREAM_CPU_CORES", None)
        if saved_threads is not None:
            os.environ["SHURUFA_TEXTSTREAM_CPU_THREADS"] = saved_threads
        if saved_cores is not None:
            os.environ["SHURUFA_TEXTSTREAM_CPU_CORES"] = saved_cores

    # Worker release 在空闲时释放权重、在活跃会话期间不打断
    rt = _make_fake_runtime(tw)
    w = tw.Worker(rt, idle_exit=600, chunk_sec=0.5, model_idle_sec=2.0)
    w.model_session = _FakeModelSession()
    w.model_path = "/synthetic/model"
    w.handle({"op": "release"})
    assert w.model_session is None and w.model_path is None and rt.mx.clear_calls >= 1
    return "memory tiering (900s/180s), env overrides, CPU threads (default 2, cap 4), release ok"


def boot(*args, **kw) -> Helper:
    h = Helper(*args, **kw)
    _, hello, _ = h.until("hello", 60)
    assert hello and hello["version"] == 1 and hello["available"] is True and hello["error"] == "", hello
    return h


def start(h: Helper, sid=1):
    h.send({"op": "start", "id": sid, "model": str(MODEL), "language": "zh-CN", "hotwords": [], "threads": 0})
    _, ev, _ = h.until("started", 120)
    assert ev and ev["id"] == sid, ev


def feed_with_backpressure(h: Helper, pcm: bytes, chunk_bytes: int = 16000, sync_every: int = 8) -> list[dict]:
    """分块发送较长音频并定期用 ping/pong 背压，避免触发 8 MiB 队列上限。"""
    collected: list[dict] = []
    for idx, off in enumerate(range(0, len(pcm), chunk_bytes), 1):
        h.audio(pcm[off : off + chunk_bytes])
        if idx % sync_every == 0:
            pid = f"bp-{idx}"
            h.send({"op": "ping", "id": pid})
            seen, ev, _ = h.until("pong", 120)
            assert ev and ev.get("id") == pid, (pid, seen)
            for _, item in seen:
                if isinstance(item, dict) and item.get("type") != "pong":
                    collected.append(item)
    return collected


@case("hello+ping")
def t_ping():
    h = boot()
    h.send({"op": "ping", "id": "p1"})
    _, ev, _ = h.until("pong", 10)
    assert ev == {"type": "pong", "id": "p1", "available": True}, ev
    h.close()
    return "hello ok, pong ok"


@case("bad JSON / unknown op / missing model")
def t_errors():
    h = boot()
    h.send_raw(b"not json\n")
    _, ev, _ = h.until("error", 10)
    assert ev and "id" not in ev, ev
    h.send({"op": "bogus"})
    _, ev, _ = h.until("error", 10)
    assert ev and "id" not in ev, ev
    h.send({"op": "start", "id": 7, "model": "/nonexistent"})
    _, ev, _ = h.until("error", 10)
    assert ev and ev["id"] == 7, ev
    h.send({"op": "audio", "pcm16": "AAAA"})
    h.send({"op": "finish"})
    h.send({"op": "cancel"})
    h.send({"op": "ping", "id": 9})
    seen, ev, _ = h.until("pong", 10)
    assert ev and [e for _, e in seen[:-1]] == [], f"unexpected replies without session: {seen}"
    h.close()
    return "errors carry/omit id as documented; audio/finish/cancel without session are silent"


@case("oversized line")
def t_oversized():
    h = boot()
    h.send_raw(b'{"op":"audio","pcm16":"' + b"A" * (1 << 20) + b'"}\n')
    _, ev, _ = h.until("error", 10)
    assert ev == {"type": "error", "message": "request too large"}, ev
    h.send({"op": "ping", "id": 1})
    _, ev, _ = h.until("pong", 10)
    assert ev, "no pong after oversized line"
    h.close()
    return "error 'request too large', next request still served"


@case("silence finish returns empty final quickly")
def t_silence():
    h = boot()
    start(h)
    pcm = b"\0" * 160000
    h.audio(pcm)
    t = time.monotonic()
    h.send({"op": "finish"})
    seen, ev, t2 = h.until("final", 30)
    assert ev and ev["text"] == "" and ev["id"] == 1, ev
    assert not [e for _, e in seen if e["type"] == "partial"], seen
    dt = t2 - t
    assert dt < 1.0, dt
    h.close()
    return f"empty final in {dt*1000:.0f} ms, no partials (5 s digital silence)"


@case("cancel mid-stream: exactly one cancelled, then reusable")
def t_cancel():
    h = boot()
    start(h, 1)
    pcm = combined_pcm()
    h.audio(pcm[: 32000 * 6])
    time.sleep(0.2)
    t = time.monotonic()
    h.send({"op": "cancel"})
    h.send({"op": "cancel"})
    h.send({"op": "ping", "id": "sync"})
    seen, ev, t2 = h.until("pong", 30)
    kinds = [e["type"] for _, e in seen]
    assert kinds.count("cancelled") == 1, kinds
    cancel_ev = [e for _, e in seen if e["type"] == "cancelled"][0]
    assert cancel_ev["id"] == 1
    assert "final" not in kinds and "error" not in kinds, kinds
    dt = t2 - t
    h.audio(read_pcm("s1.wav"))
    h.send({"op": "finish"})
    _, fin, _ = h.until("final", 30)
    assert fin is None, "audio/finish after cancel must be ignored"
    start(h, 2)
    h.audio(b"\0" * 6400)
    h.send({"op": "finish"})
    _, fin, _ = h.until("final", 30)
    assert fin and fin["id"] == 2
    h.close()
    return f"cancel->pong in {dt*1000:.0f} ms with 6 s of audio queued; kinds before pong={kinds}"


@case("cancel while decoder is busy (burst of audio then cancel)")
def t_cancel_busy():
    h = boot()
    start(h, 1)
    pcm = combined_pcm()
    for off in range(0, len(pcm), 32000):
        h.audio(pcm[off : off + 32000])
    time.sleep(0.3)
    t = time.monotonic()
    h.send({"op": "cancel"})
    h.send({"op": "ping", "id": "sync"})
    seen, ev, t2 = h.until("pong", 60)
    kinds = [e["type"] for _, e in seen]
    assert kinds.count("cancelled") == 1 and kinds[-1] == "pong", kinds
    h.close()
    return f"cancel->pong {1000*(t2-t):.0f} ms while ~19 s audio was queued/decoding"


@case("new start replaces open session silently")
def t_restart():
    h = boot()
    start(h, 1)
    start(h, 2)
    h.send({"op": "finish"})
    seen, ev, _ = h.until("final", 30)
    assert ev and ev["id"] == 2, ev
    assert not any(e.get("type") in ("cancelled", "error") for _, e in seen), seen
    h.close()
    return "no reply for replaced session 1; final carries id 2"


@case("queue budget (8 MiB) overflow ends session with 'request queue full'")
def t_overflow():
    h = boot()
    start(h, 1)
    # 12 行 ~700 KB 的 base64 音频，总计 > 8 MiB；工作线程先被一整行有声音频占住。
    pcm = (read_pcm("s3.wav") * 8)[: 525_000]
    for _ in range(14):
        h.audio(pcm)
    seen, ev, _ = h.until("error", 120)
    msgs = [e.get("message") for _, e in seen if isinstance(e, dict) and e.get("type") == "error"]
    h.send({"op": "ping", "id": "after"})
    _, pong, _ = h.until("pong", 120)
    h.close()
    assert pong, "process unresponsive after overflow"
    assert "request queue full" in msgs, f"no overflow error (worker kept up?): {msgs}"
    return f"error {msgs}, process still serves ping"


@case("idle exit")
def t_idle():
    h = boot("--idle-exit", "3")
    t = time.monotonic()
    code = None
    try:
        code = h.proc.wait(timeout=8)
    except subprocess.TimeoutExpired:
        pass
    dt = time.monotonic() - t
    h.close()
    assert code == 0 and 2.5 < dt < 6, (code, dt)
    return f"exited code 0 after {dt:.1f} s with --idle-exit 3"


@case("EOF exits")
def t_eof():
    h = boot()
    h.proc.stdin.close()
    code = h.proc.wait(timeout=10)
    assert code == 0, code
    return "exit 0 on stdin EOF"


@case("model released after idle (--idle-exit shorter than 120 s) and reloaded")
def t_release():
    h = boot("--idle-exit", "4")
    start(h, 1)
    h.send({"op": "finish"})
    h.until("final", 30)
    h.send({"op": "release"})
    start(h, 2)
    h.send({"op": "cancel"})
    h.send({"op": "ping", "id": 1})
    seen, ev, _ = h.until("pong", 30)
    assert [e["type"] for _, e in seen] == ["cancelled", "pong"]
    h.close()
    return "release op ok, start reloads"


@case("long audio >30s commits multiple segments without truncation")
def t_long_audio_over_30s():
    h = boot()
    start(h, 10)
    sil = lambda s: b"\0" * int(32000 * s)
    s1, s2, s3 = read_pcm("s1.wav"), read_pcm("s2.wav"), read_pcm("s3.wav")
    # 两轮完整三句 + 1.2s 停顿 + 尾部补一段 s1，总时长 > 36s，覆盖自然停顿切段并突破 30s 窗口。
    pcm = (
        sil(0.5)
        + s1
        + sil(0.4)
        + s2
        + sil(0.4)
        + s3
        + sil(1.2)
        + s1
        + sil(0.4)
        + s2
        + sil(0.4)
        + s3
        + sil(1.2)
        + s1
        + sil(0.5)
    )
    reference = "".join(SENTENCES) + "".join(SENTENCES) + SENTENCES[0]
    earlier = feed_with_backpressure(h, pcm)
    h.send({"op": "finish"})
    seen, final, _ = h.until("final", 120)
    h.close()
    assert final and final["id"] == 10, final
    assert final.get("segment_count", 0) >= 2, final
    assert len(final.get("segments", [])) == final["segment_count"], final
    assert final.get("audio_ms", 0) >= 35000, final
    assert final.get("speech_ms", 0) > 0, final
    partials = [e for e in earlier if e.get("type") == "partial"] + [
        e for _, e in seen if isinstance(e, dict) and e.get("type") == "partial"
    ]
    assert partials, "expected partials for >30s audio"
    for ev in partials:
        assert "finalized" in ev and "draft" in ev, ev
        assert ev["finalized"] + ev["draft"] == ev["text"], ev
    score = cer(reference, final["text"])
    assert score < 0.25, f"CER too high ({score:.3f}): {final['text']!r}"
    return f"audio={final['audio_ms']}ms, segments={final['segment_count']}, asr={final.get('asr_ms')}ms, cer={score:.3f}"


@case("mid-utterance long silence skipped without hallucination")
def t_mid_utterance_long_silence():
    h = boot()
    start(h, 11)
    sil = lambda s: b"\0" * int(32000 * s)
    pcm = sil(0.5) + read_pcm("s1.wav") + sil(5.0) + read_pcm("s2.wav") + sil(0.5)
    feed_with_backpressure(h, pcm)
    h.send({"op": "finish"})
    _, final, _ = h.until("final", 60)
    h.close()
    assert final and final["id"] == 11, final
    # 至少 5s 静音（前后+句中共 6s）未被计入 speech_ms。
    assert final.get("speech_ms", 0) < final.get("audio_ms", 0) - 4000, final
    score = cer(SENTENCES[0] + SENTENCES[1], final["text"])
    assert score < 0.25, f"CER too high ({score:.3f}): {final['text']!r}"
    return f"audio={final['audio_ms']}ms, speech={final['speech_ms']}ms, cer={score:.3f}"


@case("env SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC & CPU_THREADS override + release")
def t_env_overrides():
    h = boot(
        env={
            "SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC": "2",
            "SHURUFA_TEXTSTREAM_CPU_THREADS": "2",
        }
    )
    start(h, 12)
    h.audio(b"\0" * 32000)
    h.send({"op": "finish"})
    _, fin, _ = h.until("final", 30)
    assert fin and fin["id"] == 12 and fin["text"] == "", fin
    # 等待超过 2s 触发自动卸载，再显式发 release 并重新 start 验证热恢复。
    time.sleep(2.4)
    h.send({"op": "release"})
    start(h, 13)
    h.send({"op": "finish"})
    _, fin2, _ = h.until("final", 30)
    assert fin2 and fin2["id"] == 13, fin2
    h.close()
    return "env idle=2s + cpu_threads=2 + release + reload ok"


UNIT_CASES = (
    u_long_audio_over_30s,
    u_warn_and_hard_cap,
    u_mid_utterance_silence_skip,
    u_pure_silence,
    u_env_and_release,
)


def run_unit() -> int:
    for fn in UNIT_CASES:
        fn()
    failed = [c for c in results if not c["ok"]]
    print(f"\n{len(results) - len(failed)}/{len(results)} unit cases passed")
    return 1 if failed else 0


def run_all(chunk_sec: float):
    for fn in (
        *UNIT_CASES,
        t_ping,
        t_errors,
        t_oversized,
        t_silence,
        t_cancel,
        t_cancel_busy,
        t_restart,
        t_overflow,
        t_idle,
        t_eof,
        t_release,
        t_long_audio_over_30s,
        t_mid_utterance_long_silence,
        t_env_overrides,
    ):
        fn()
    r = run_dictation(combined_pcm(), "".join(SENTENCES), chunk_sec, "combined-realtime")
    print(json.dumps({k: r[k] for k in r if k != "timeline"}, ensure_ascii=False, indent=1))
    for t, text in r["timeline"]:
        print(f"  {t:6.2f}s  {text}")
    out = {"protocol_cases": results, "dictation": r}
    (LOGS / "protocol-results.json").write_text(json.dumps(out, ensure_ascii=False, indent=1))
    failed = [c for c in results if not c["ok"]]
    print(f"\n{len(results) - len(failed)}/{len(results)} protocol cases passed")
    return 1 if failed else 0


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("unit")
    a = sub.add_parser("all")
    a.add_argument("--chunk-sec", type=float, default=0.5)
    o = sub.add_parser("one")
    o.add_argument("value", type=float)
    s = sub.add_parser("sweep")
    s.add_argument("values", nargs="+", type=float)
    args = p.parse_args()
    if args.cmd == "unit":
        return run_unit()
    if not (ROOT / ".ready").exists():
        print("TextStream runtime is not installed (run install-runtime.sh); skipping")
        return 77
    LOGS.mkdir(parents=True, exist_ok=True)
    if args.cmd == "one":
        print(json.dumps(run_dictation(combined_pcm(), "".join(SENTENCES), args.value, f"combined-{args.value}"), ensure_ascii=False))
        return 0
    if args.cmd == "sweep":
        sweep(args.values)
        return 0
    return run_all(args.chunk_sec)


if __name__ == "__main__":
    sys.exit(main())
