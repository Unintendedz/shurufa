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
  # 直接以非 .app 目录名做全量备份并改名 SavedContents，全程不把已安装的 Shurufa.app 移走、
  # 也不对它执行 lsregister -u，避免 macOS 26/27 LaunchServices 收到注销事件而把 Shurufa 移出输入法菜单。
  ditto "$destination" "$backup/Shurufa.bundle-backup"
  mv "$backup/Shurufa.bundle-backup/Contents" "$backup/Shurufa.bundle-backup/SavedContents"
  # 保留 ~/Library/Input Methods/Shurufa.app 路径与目录 inode 原地同步更新。
  rsync -a --delete "$staging/Shurufa.app/" "$destination/"
else
  mv "$staging/Shurufa.app" "$destination"
fi
codesign --verify --deep --strict "$destination"
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
python3 "$root/platforms/macos/scripts/shurufa_installation.py" unregister "$source_bundle"
"$lsregister" -f "$destination"
executable="$destination/Contents/MacOS/Shurufa"
"$executable" --prepare-host
"$executable" --migrate-caps-lock
"$executable" --migrate-voice-shortcut
"$executable" --migrate-voice-presentation
"$executable" --migrate-quanpin-helpcode
wanxiang_gram="$HOME/Library/Application Support/Shurufa/resource-packs/grammar-model-wanxiang/wanxiang-lts-zh-hans.gram"
if [[ ! -f "$wanxiang_gram" ]]; then
  CARGO_TARGET_DIR="$root/target/macos-cargo" cargo run --quiet -p msime-client-providers --example install_resource_pack --locked -- "$HOME/Library/Application Support/Shurufa" grammar-model-wanxiang
fi
"$executable" --migrate-grammar-model
"$executable" --migrate-inline-emoji
set +e
# 登记新包并确保父容器与至少一个有效模式在实时 TIS 已启用列表中在线（绝不停用、绝不抢切当前输入源）：
# 若此前因系统升级或旧安装器导致父容器处于 disabled 状态，--ensure-input-source-enabled 会即时补齐，免去手动删加。
"$executable" --register-input-source
registration=$?
if [[ "$registration" -eq 0 ]]; then
  "$executable" --ensure-input-source-enabled
  registration=$?
fi
"$executable" --input-source-registered
listed=$?
set -e
printf 'Installed: %s\nRegistration exit: %s; registry query exit: %s\n' "$destination" "$registration" "$listed"
printf 'Source revision: %s\n' "$(/usr/libexec/PlistBuddy -c Print:MSIMEInputSourceRevision "$destination/Contents/Info.plist" 2>/dev/null || echo 未记录)"
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
