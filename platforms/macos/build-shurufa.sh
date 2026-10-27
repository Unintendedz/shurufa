#!/usr/bin/env bash
# 只构建 arm64 原生独立版，不构建 Tauri，也不安装或切换输入源。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk
[[ -d "$SDKROOT" ]] || { echo "需要 Command Line Tools 的 MacOSX26.5.sdk" >&2; exit 1; }
export CARGO_TARGET_DIR="$root/target/macos-cargo"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
export CFLAGS=-mmacosx-version-min=13.0
export CXXFLAGS=-mmacosx-version-min=13.0
export CMAKE_OSX_DEPLOYMENT_TARGET=13.0
export CMAKE_PREFIX_PATH="$(brew --prefix)"
cargo build -p msime-host-api --locked
resources="$(cargo run --quiet -p msime-client-core --example install_resources --locked -- target/resources)"
MSIME_EDITION=shurufa bash platforms/macos/stage-resources.sh "$resources"
cmake -G Ninja -S platforms/macos -B target/macos-isolated \
  -DMSIME_EDITION=shurufa -DMSIME_ENABLE_SPARKLE=OFF \
  -DMSIME_HOST_LIBRARY="$CARGO_TARGET_DIR/debug/libmsime_host_api.a" \
  -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
  -DCMAKE_OSX_SYSROOT="$SDKROOT" -DCMAKE_PREFIX_PATH="$CMAKE_PREFIX_PATH" \
  -DPython3_EXECUTABLE="$(brew --prefix python@3.14)/bin/python3.14"
cmake --build target/macos-isolated --target MSIMEClientInputMethod --parallel "$CARGO_BUILD_JOBS"
bundle="$root/target/macos-isolated/Shurufa.app"
codesign --force --deep --sign - --entitlements platforms/macos/resources/VoiceInput.entitlements "$bundle"
codesign --verify --deep --strict "$bundle"
printf 'Built: %s\n' "$bundle"
