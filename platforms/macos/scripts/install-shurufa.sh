#!/usr/bin/env bash
# 只安装 Shurufa；结束所有 Shurufa 副本，不替换其他输入法，也不选择当前输入源。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
source_bundle="${1:-$root/target/macos-isolated/Shurufa.app}"
identifier="$(/usr/libexec/PlistBuddy -c Print:CFBundleIdentifier "$source_bundle/Contents/Info.plist")"
[[ "$identifier" == dev.shurufa.inputmethod.Shurufa ]] || { echo "拒绝安装非 Shurufa bundle" >&2; exit 1; }
destination_root="$HOME/Library/Input Methods"
destination="$destination_root/Shurufa.app"
mkdir -p "$destination_root"
codesign --verify --deep --strict "$source_bundle"
staging="$(mktemp -d "$destination_root/.shurufa-install.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
ditto "$source_bundle" "$staging/Shurufa.app"
codesign --verify --deep --strict "$staging/Shurufa.app"
python3 "$root/platforms/macos/scripts/shurufa_installation.py" prepare "$destination"
if [[ -e "$destination" ]]; then
  backup="$HOME/Library/Application Support/Shurufa Installation Backups/$(date +%Y%m%d-%H%M%S)"
  mkdir -p "$backup"
  mv "$destination" "$backup/Shurufa.app"
  python3 "$root/platforms/macos/scripts/shurufa_installation.py" unregister "$backup/Shurufa.app"
  python3 "$root/platforms/macos/scripts/shurufa_installation.py" preserve "$backup/Shurufa.app"
fi
mv "$staging/Shurufa.app" "$destination"
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
python3 "$root/platforms/macos/scripts/shurufa_installation.py" unregister "$source_bundle"
"$lsregister" -f "$destination"
executable="$destination/Contents/MacOS/Shurufa"
"$executable" --prepare-host
"$executable" --migrate-caps-lock
"$executable" --migrate-voice-shortcut
"$executable" --migrate-voice-presentation
"$executable" --migrate-quanpin-helpcode
set +e
# 只登记新包，不停用再启用：macOS 27 上 TISEnableInputSource 对键盘输入模式返回 noErr 却不生效，
# 先停用会让 Shurufa 从输入菜单消失，用户只能手动删掉再添加。已启用的模式按 bundle ID 记录，原地替换后照常可用。
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
if [[ "$registration" -ne 0 ]]; then
  echo "Shurufa 输入源刷新失败；请保留前后状态与备份，不把登记可见当作刷新成功。" >&2
  exit "$registration"
fi
"$executable" --enabled-input-modes
python3 "$root/platforms/macos/scripts/shurufa_installation.py" retire-caches "$(getconf DARWIN_USER_CACHE_DIR)" \
  "$HOME/Library/Application Support/Shurufa Installation Backups/input-source-caches-$(date +%Y%m%d-%H%M%S)"
# 系统已有实例时 open 会复用它，不直接派生第二个服务器。
open -g "$destination"
# 让菜单栏输入法菜单按当前登记重读，不必手动删掉再添加。
"$executable" --announce-input-source-change
python3 "$root/platforms/macos/scripts/shurufa_installation.py" verify "$destination"
