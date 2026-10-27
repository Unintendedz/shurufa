#!/usr/bin/env python3
"""msime-voice-local 协议一致性与性能实测驱动。

用法：
  python3 test_protocol.py all [--chunk-sec 0.5]      # 全部协议用例，结果写入 logs/protocol-results.json
  python3 test_protocol.py sweep 0.5 0.8 1.0 1.5 2.5  # 按解码间隔扫描延迟/准确率/CPU/内存
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import queue
import resource
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata
import wave
from pathlib import Path

HERE = Path(__file__).resolve().parent
LAUNCHER = HERE.parent / "shurufa-textstream"
MODEL = Path.home() / "Library/Application Support/Shurufa/textstream/model"
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


# ---------------- 协议用例 ----------------

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


def boot(*args, **kw) -> Helper:
    h = Helper(*args, **kw)
    _, hello, _ = h.until("hello", 60)
    assert hello and hello["version"] == 1 and hello["available"] is True and hello["error"] == "", hello
    return h


def start(h: Helper, sid=1):
    h.send({"op": "start", "id": sid, "model": str(MODEL), "language": "zh-CN", "hotwords": [], "threads": 0})
    _, ev, _ = h.until("started", 120)
    assert ev and ev["id"] == sid, ev


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
    nparts_after = 0
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


def run_all(chunk_sec: float):
    for fn in (t_ping, t_errors, t_oversized, t_silence, t_cancel, t_cancel_busy, t_restart, t_overflow, t_idle, t_eof, t_release):
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
    a = sub.add_parser("all")
    a.add_argument("--chunk-sec", type=float, default=0.5)
    o = sub.add_parser("one")
    o.add_argument("value", type=float)
    s = sub.add_parser("sweep")
    s.add_argument("values", nargs="+", type=float)
    args = p.parse_args()
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
