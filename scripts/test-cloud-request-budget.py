#!/usr/bin/env python3
"""验证 macOS 宿主与 C 头文件读取 client-core 声明的共享云候选默认时限，不私设超时常数。"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SHARED = ROOT / "crates/client-core/src/cloud/candidates.rs"
HEADER = ROOT / "crates/host-api/include/msime_client.h"
HOSTS = [
    (
        ROOT / "platforms/macos/src/cloud/CloudCandidateRequest.mm",
        ["MSIME_CLOUD_REQUEST_TIMEOUT_MS"],
        # 仅检查云候选初始化器；同一文件中的翻译与 AI 初始化器读取各自描述符校验后的截止时间。
        (r"- \(instancetype\)initWithURL:", r"\n- \(", r"_timeout = ([0-9.]+)\s*;"),
    ),
]
REFERENCE = {"CONNECT": 2000, "REQUEST": 2000}


def declared(path: pathlib.Path, pattern: str) -> dict[str, int]:
    text = path.read_text(encoding="utf-8")
    return {name: int(value) for name, value in re.findall(pattern, text)}


def main() -> int:
    if not SHARED.is_file() or not HEADER.is_file():
        print("skipped: the cloud candidate contract is not present")
        return 0
    failures: list[str] = []

    shared = declared(SHARED, r"pub const (CONNECT|REQUEST)_TIMEOUT_MS: u64 = (\d+)")
    header = declared(HEADER, r"#define MSIME_CLOUD_(CONNECT|REQUEST)_TIMEOUT_MS (\d+)")
    for name, expected in REFERENCE.items():
        if shared.get(name) != expected:
            failures.append(
                f"client-core's {name}_TIMEOUT_MS is {shared.get(name)}, and the reference asks for {expected}"
            )
        if header.get(name) != expected:
            failures.append(
                f"the C header's MSIME_CLOUD_{name}_TIMEOUT_MS is {header.get(name)}, and the reference asks for {expected}"
            )

    for path, required, (scope, scope_end, literal) in HOSTS:
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        for name in required:
            if name not in text:
                failures.append(f"{path.relative_to(ROOT)} does not read {name}")
        if scope:
            match = re.search(scope, text)
            if not match:
                failures.append(f"{path.relative_to(ROOT)} no longer has the request this checks")
                continue
            rest = text[match.end() :]
            end = re.search(scope_end, rest)
            text = rest[: end.start()] if end else rest
        for value in re.findall(literal, text):
            failures.append(
                f"{path.relative_to(ROOT)} sets this request's deadline to a literal {value}"
            )

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    print("cloud request budget: connect 2000 ms, total 2000 ms, macOS host and C header match client-core")
    return 0


if __name__ == "__main__":
    sys.exit(main())
