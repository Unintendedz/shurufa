#!/usr/bin/env bash
# 只安装 Shurufa；不替换水杉，不结束其他进程，也不选择当前输入源。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
source_bundle="${1:-$root/target/macos-isolated/Shurufa.app}"
identifier="$(/usr/libexec/PlistBuddy -c Print:CFBundleIdentifier "$source_bundle/Contents/Info.plist")"
[[ "$identifier" == dev.shurufa.inputmethod.Shurufa ]] || { echo "拒绝安装非 Shurufa bundle" >&2; exit 1; }
destination_root="$HOME/Library/Input Methods"
destination="$destination_root/Shurufa.app"
mkdir -p "$destination_root"
staging="$(mktemp -d "$destination_root/.shurufa-install.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
ditto "$source_bundle" "$staging/Shurufa.app"
codesign --force --deep --sign - --entitlements "$root/platforms/macos/resources/VoiceInput.entitlements" "$staging/Shurufa.app"
codesign --verify --deep --strict "$staging/Shurufa.app"
if [[ -e "$destination" ]]; then
  backup="$HOME/Library/Application Support/Shurufa/installation-backups/$(date +%Y%m%d-%H%M%S)"
  mkdir -p "$backup"
  mv "$destination" "$backup/Shurufa.app"
  printf 'Previous Shurufa preserved: %s\n' "$backup/Shurufa.app"
fi
mv "$staging/Shurufa.app" "$destination"
executable="$destination/Contents/MacOS/Shurufa"
"$executable" --prepare-host
set +e
"$executable" --register-input-source
registration=$?
"$executable" --input-source-registered
listed=$?
set -e
printf 'Installed: %s\nRegistration exit: %s; registry query exit: %s\n' "$destination" "$registration" "$listed"
if [[ "$listed" -ne 0 ]]; then
  echo "Shurufa 已安装；当前登录会话还未列出新输入源，需要注销并重新登录后在系统设置中添加。" >&2
  exit 2
fi
"$executable" --enabled-input-modes
