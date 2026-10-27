#!/usr/bin/env python3
"""shurufa-textstream 的脚本化替身：说 msime-voice-local 协议，不加载任何模型。
MSIME_TS_FIXTURE_MODE: normal（默认）或 unavailable；MSIME_TS_FIXTURE_LOG 记录收到的请求（音频只记长度）。"""
import json
import os
import sys

mode = os.environ.get("MSIME_TS_FIXTURE_MODE", "normal")
log = open(os.environ["MSIME_TS_FIXTURE_LOG"], "a") if os.environ.get("MSIME_TS_FIXTURE_LOG") else None


def send(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


if log:
    log.write(json.dumps({"argv": sys.argv[1:]}) + "\n")
send({"type": "hello", "version": 1, "available": mode != "unavailable",
      "error": "No module named 'mlx'" if mode == "unavailable" else ""})
session = None
audio = 0
for line in sys.stdin:
    request = json.loads(line)
    if log:
        shown = dict(request)
        if "pcm16" in shown:
            shown["pcm16"] = len(shown["pcm16"])
        log.write(json.dumps(shown, ensure_ascii=False) + "\n")
        log.flush()
    op = request.get("op")
    if op == "start":
        if mode == "unavailable":
            send({"type": "error", "id": request["id"], "message": "RuntimeError: runtime unavailable: No module named 'mlx'"})
            continue
        session, audio = request["id"], 0
        send({"type": "started", "id": session})
    elif op == "audio" and session is not None:
        audio += 1
        text = "你好" if audio == 1 else "你好世界"
        send({"type": "partial", "id": session, "text": text, "finalized": text[:2], "draft": text[2:]})
    elif op == "finish" and session is not None:
        send({"type": "final", "id": session, "text": "你好世界。"})
        session = None
    elif op == "cancel" and session is not None:
        send({"type": "cancelled", "id": session})
        session = None
    elif op == "ping":
        send({"type": "pong", "id": request.get("id"), "available": True})
