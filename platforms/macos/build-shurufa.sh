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
# Tab 笔画筛选要 msime-stroke.db：按 resources/language-dictionaries.lock.json 下载并校验，已有且一致时不动。
"$(brew --prefix python@3.14)/bin/python3.14" scripts/fetch_language_dictionaries.py
MSIME_EDITION=shurufa bash platforms/macos/stage-resources.sh "$resources"
# 工作区有未提交的改动时 bundle 不能声称自己来自某个提交；显式传空值也会清掉缓存里上一次的修订。
revision="$(git rev-parse HEAD)"
if [[ -n "$(git status --porcelain)" ]]; then
  echo "工作区有未提交的改动，bundle 不写源码修订" >&2
  revision=""
fi
cmake -G Ninja -S platforms/macos -B target/macos-isolated \
  -DMSIME_EDITION=shurufa -DMSIME_ENABLE_SPARKLE=OFF \
  -DMSIME_INPUT_SOURCE_REVISION="$revision" \
  -DMSIME_HOST_LIBRARY="$CARGO_TARGET_DIR/debug/libmsime_host_api.a" \
  -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
  -DCMAKE_OSX_SYSROOT="$SDKROOT" -DCMAKE_PREFIX_PATH="$CMAKE_PREFIX_PATH" \
  -DPython3_EXECUTABLE="$(brew --prefix python@3.14)/bin/python3.14"
cmake --build target/macos-isolated --target MSIMEClientInputMethod --parallel "$CARGO_BUILD_JOBS"
bundle="$root/target/macos-isolated/Shurufa.app"
# 按本机签名证书的 SHA-1 签名：designated requirement 绑定证书而不是 cdhash，重编后麦克风等授权仍然有效。
# 钥匙串里还没有证书（新的 Mac）时先建一张；建不成才退回 ad-hoc，那样每次重编都要重新授权。
signing_name="${SHURUFA_SIGNING_NAME:-Shurufa Local Signing}"
find_identity() { security find-certificate -c "$signing_name" -Z 2>/dev/null | awk '/^SHA-1 hash:/{print $3; exit}'; }
identity="$(find_identity)"
if [[ -z "$identity" ]]; then
  SHURUFA_SIGNING_NAME="$signing_name" bash platforms/macos/scripts/create-shurufa-signing-identity.sh || true
  identity="$(find_identity)"
fi
if [[ -z "$identity" ]]; then
  echo "没能在钥匙串里建「$signing_name」，按 ad-hoc 签名" >&2
  identity=-
fi
codesign --force --deep --sign "$identity" --entitlements platforms/macos/resources/VoiceInput.entitlements "$bundle"
codesign --verify --deep --strict "$bundle"
printf 'Built: %s\n' "$bundle"
