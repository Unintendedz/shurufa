#!/usr/bin/env bash
# Shurufa 本地门禁脚本（纯 macOS 单产品）：
#   scripts/verify-local.sh --quick   快速门禁（决策笔记 + 文本契约 + 工作区编译检查 + macOS 原生构建）
#   scripts/verify-local.sh           完整验证（含 Rust 单元测试、Clippy 与全量 macOS CTest）
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ -d "/opt/homebrew/opt/python@3.14/bin" ]]; then
  export PATH="/opt/homebrew/opt/python@3.14/bin:$PATH"
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/macos-cargo}"
# 防止长路径 TMPDIR 超过 Unix 域套接字 `SUN_LEN`（104 字节）上限导致本地 IPC 单测失败。
if [[ -d "/private/tmp" && "${#TMPDIR}" -gt 60 ]]; then
  export TMPDIR="/private/tmp"
fi

quick=0
run_tier1=0
for argument in "$@"; do
  case "$argument" in
    --quick) quick=1 ;;
    --tier1) run_tier1=1 ;;
    *) echo "unknown option: $argument" >&2; exit 2 ;;
  esac
done

: "${MSIME_MACOS_BUILD:=target/macos-isolated}"
: "${MSIME_BUILD_JOBS:=${CMAKE_BUILD_PARALLEL_LEVEL:-${CARGO_BUILD_JOBS:-6}}}"
if [[ -z "${SDKROOT:-}" && -d "/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk" ]]; then
  export SDKROOT="/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk"
fi

note() { printf '\n=== %s ===\n' "$1"; }

note "agent notes"
node .agents/skills/write-notes-like-deepseek/scripts/verify-agent-note-tree.ts
node .agents/skills/write-notes-like-deepseek/scripts/verify-agent-note-format.ts
node .agents/skills/write-notes-like-deepseek/scripts/verify-archived-agent-notes.ts

note "contract checks"
bash scripts/run-checks.sh

note "compile: rust workspace"
cargo check --workspace

if [[ -d "$MSIME_MACOS_BUILD" ]]; then
  note "compile: macos host"
  cargo build --release -p msime-host-api --locked
  cmake --build "$MSIME_MACOS_BUILD" --target MSIMEClientInputMethod --parallel "$MSIME_BUILD_JOBS"
fi

if [[ "$quick" -eq 1 ]]; then
  if [[ "$run_tier1" -eq 1 ]]; then
    note "tier1 neural eval gate"
    bash scripts/eval-tier1.sh --require
  fi
  printf '\nquick check passed\n'
  exit 0
fi

note "rust tests"
cargo test --workspace --lib

note "clippy"
cargo clippy --workspace --lib --no-deps -- -D warnings -A clippy::chunks_exact_to_as_chunks

if [[ -d "$MSIME_MACOS_BUILD" ]]; then
  note "macos ctest"
  cmake --build "$MSIME_MACOS_BUILD" --parallel "$MSIME_BUILD_JOBS"
  ctest --test-dir "$MSIME_MACOS_BUILD" -E '^(textstream-real|textstream-protocol)$' --parallel 4 --output-on-failure
fi

note "tier1 neural eval gate"
bash scripts/eval-tier1.sh

printf '\nverification passed\n'
