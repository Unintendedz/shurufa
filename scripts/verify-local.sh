#!/usr/bin/env bash
# Shurufa 本地门禁脚本（纯 macOS 单产品）：
#   scripts/verify-local.sh --quick   快速门禁（决策笔记 + 源码编译检查 + macOS 原生构建）
#   scripts/verify-local.sh           完整验证（含 Rust 单元测试、Clippy 与 macOS CTest）
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

quick=0
for argument in "$@"; do
  case "$argument" in
    --quick) quick=1 ;;
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

note "conflict markers & tracked symlinks"
python3 scripts/test-conflict-markers.py
python3 scripts/test-tracked-symlinks.py

note "compile: rust workspace"
cargo check --workspace

if [[ -d "$MSIME_MACOS_BUILD" ]]; then
  note "compile: macos host"
  CARGO_TARGET_DIR="$root/target/macos-cargo" cargo build --release -p msime-host-api --locked
  cmake --build "$MSIME_MACOS_BUILD" --target MSIMEClientInputMethod --parallel "$MSIME_BUILD_JOBS"
fi

if [[ "$quick" -eq 1 ]]; then
  printf '\nquick check passed\n'
  exit 0
fi

note "rust tests"
cargo test --workspace --lib

note "clippy"
cargo clippy --workspace --lib --no-deps -- -D warnings -A clippy::chunks_exact_to_as_chunks

if [[ -d "$MSIME_MACOS_BUILD" ]]; then
  note "macos core tests"
  cmake --build "$MSIME_MACOS_BUILD" --target neural-service-ipc-test shared-uninstaller-test --parallel "$MSIME_BUILD_JOBS"
  ctest --test-dir "$MSIME_MACOS_BUILD" -R "neural-service-ipc|shared-uninstaller|text-client|smart-punctuation-shadow" --output-on-failure
fi

printf '\nverification passed\n'
