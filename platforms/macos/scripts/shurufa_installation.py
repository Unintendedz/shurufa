#!/usr/bin/env python3
"""安装前整理同标识的登记、进程与旧备份，不选择输入源。"""
import os
import json
import plistlib
import signal
import subprocess
import sys
import time
from pathlib import Path

IDENTIFIER = "dev.shurufa.inputmethod.Shurufa"
LSREGISTER = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"


def registered_paths(dump):
    paths = []
    for block in dump.split("-" * 80):
        fields = {}
        for line in block.splitlines():
            if line and not line[0].isspace():
                key, separator, value = line.partition(":")
                if separator:
                    fields[key] = value.strip()
        if fields.get("identifier") != IDENTIFIER:
            continue
        value = fields["path"]
        path, separator, address = value.rpartition(" (0x")
        if separator and address.endswith(")"):
            value = path
        paths.append(Path(value))
    return paths


def unregister(path):
    result = subprocess.run([LSREGISTER, "-u", str(path)], capture_output=True, text=True)
    if result.returncode:
        # 没登记过的 bundle 也返回失败；以注销后的登记表为准。
        dump = subprocess.check_output([LSREGISTER, "-dump"], text=True)
        if path in registered_paths(dump):
            raise RuntimeError(f"无法注销：{path}: {result.stderr}")


def preserve_bundle(bundle):
    """保留每个字节；恢复时把目录名和 SavedContents 改回原名。"""
    destination = bundle.with_suffix(".bundle-backup")
    if destination.exists():
        raise RuntimeError(f"备份目标已存在，保留原件并停止：{destination}")
    bundle.rename(destination)
    (destination / "Contents").rename(destination / "SavedContents")
    return destination


def host_processes():
    output = subprocess.check_output(["ps", "-axo", "pid=,comm="], text=True)
    found = []
    for line in output.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) != 2 or not parts[1].endswith("/Contents/MacOS/Shurufa"):
            continue
        executable = Path(parts[1])
        info = plistlib.loads((executable.parents[1] / "Info.plist").read_bytes())
        if info.get("CFBundleIdentifier") == IDENTIFIER:
            found.append((int(parts[0]), executable))
    return found


def migrate_backups(legacy_root, backup_root):
    """把旧备份移出可卸载的数据目录，保留每个目录和字节，不覆盖同名目标。"""
    if not legacy_root.exists():
        return
    directories = sorted(path for path in legacy_root.iterdir() if path.is_dir())
    for path in directories:
        if (backup_root / path.name).exists():
            raise RuntimeError(f"备份目标已存在，保留两份并停止：{backup_root / path.name}")
    if directories:
        backup_root.mkdir(parents=True, exist_ok=True)
    for path in directories:
        path.rename(backup_root / path.name)


def prepare(destination):
    dump = subprocess.check_output([LSREGISTER, "-dump"], text=True)
    for path in registered_paths(dump):
        # 安装位置原地替换后由 lsregister -f 更新；先注销它会让系统把 Shurufa 移出输入菜单。
        if path == destination:
            continue
        unregister(path)
        print(f"Unregistered: {path}")
    for pid, executable in host_processes():
        os.kill(pid, signal.SIGTERM)
        print(f"Stopped: {pid} {executable}")
    for _ in range(50):
        if not host_processes():
            break
        time.sleep(0.1)
    else:
        raise RuntimeError("Shurufa 进程未退出，停止安装；保留现有安装和备份")
    legacy_root = Path.home() / "Library/Application Support/Shurufa/installation-backups"
    backup_root = Path.home() / "Library/Application Support/Shurufa Installation Backups"
    for bundle in sorted([*legacy_root.glob("*/Shurufa.app"), *backup_root.glob("*/Shurufa.app")]):
        info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
        if info.get("CFBundleIdentifier") == IDENTIFIER:
            unregister(bundle)
            print(f"Preserved: {preserve_bundle(bundle)}")
    migrate_backups(legacy_root, backup_root)


def installation_matches(paths, processes, destination):
    executable = destination / "Contents/MacOS/Shurufa"
    return set(path.resolve() for path in paths) == {destination.resolve()} and \
        len(processes) == 1 and processes[0][1].resolve() == executable.resolve()


def input_source_preferences_match(entries, declared_modes):
    """只认本 bundle 的有效模式或父来源，拒绝缺失和旧模式残留。"""
    own = [entry for entry in entries if isinstance(entry, dict) and entry.get("Bundle ID") == IDENTIFIER]
    if not own:
        return False
    for entry in own:
        if entry.get("InputSourceKind") == "Input Mode":
            if entry.get("Input Mode") not in declared_modes:
                return False
        elif entry.get("InputSourceKind") != "Keyboard Input Method":
            return False
    return True


INPUT_SOURCE_CACHE_NAMES = ("com.apple.IntlDataCache.le", "com.apple.IntlDataCache.le.kbdx")


def retire_app_input_source_caches(cache_root, backup):
    """沙盒应用各自留一份输入源缓存，系统不会因重装刷新它：旧副本里没有新声明的 Caps Lock 能力、留着已删除的模式，
    微信的副本里甚至没有 Shurufa。移到备份目录后，应用下次启动按当前登记重建；顶层那份系统自己维护，不动。"""
    moved = []
    for directory in sorted(cache_root.iterdir()):
        for name in INPUT_SOURCE_CACHE_NAMES:
            cache = directory / name
            try:
                if directory.is_symlink() or not directory.is_dir() or cache.is_symlink() or not cache.is_file():
                    continue
            except PermissionError:
                # 系统沙盒目录（例如 WebKit、快捷指令的）不允许访问，跳过。
                break
            target = backup / directory.name
            target.mkdir(parents=True, exist_ok=True)
            cache.rename(target / name)
            moved.append(directory.name)
    return sorted(set(moved))


def enabled_input_source_entries(hitoolbox, inputsources):
    """新版 macOS 把第三方输入法记在 com.apple.inputsources 的 AppleEnabledThirdPartyInputSources，两处都要读。"""
    return list(hitoolbox.get("AppleEnabledInputSources", [])) + list(inputsources.get("AppleEnabledThirdPartyInputSources", []))


def verify(destination):
    for _ in range(50):
        processes = host_processes()
        if len(processes) == 1 and processes[0][1] == destination / "Contents/MacOS/Shurufa":
            break
        time.sleep(0.1)
    paths = registered_paths(subprocess.check_output([LSREGISTER, "-dump"], text=True))
    if not installation_matches(paths, processes, destination):
        raise RuntimeError("安装后登记或进程不是唯一的安装版，请保留日志检查；没有切换当前输入源")
    entries = enabled_input_source_entries(*(
        plistlib.loads(subprocess.check_output(["defaults", "export", domain, "-"]))
        for domain in ("com.apple.HIToolbox", "com.apple.inputsources")))
    info = plistlib.loads((destination / "Contents/Info.plist").read_bytes())
    declared = set(info["ComponentInputModeDict"]["tsInputModeListKey"])
    if not input_source_preferences_match(entries, declared):
        raise RuntimeError("TIS 登记可见但系统启用列表缺少有效的 Shurufa 输入源；刷新尚未通过验收")
    print(json.dumps({"registered_paths":[str(path) for path in paths],
        "processes":[{"pid":pid,"executable":str(path)} for pid,path in processes],
        "enabled_input_sources":[entry for entry in entries if entry.get("Bundle ID") == IDENTIFIER]}, ensure_ascii=False))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "prepare":
        prepare(Path(sys.argv[2]).resolve())
    elif len(sys.argv) == 3 and sys.argv[1] == "preserve":
        print(preserve_bundle(Path(sys.argv[2])))
    elif len(sys.argv) == 3 and sys.argv[1] == "unregister":
        unregister(Path(sys.argv[2]).resolve())
    elif len(sys.argv) == 4 and sys.argv[1] == "retire-caches":
        moved = retire_app_input_source_caches(Path(sys.argv[2]), Path(sys.argv[3]))
        print(json.dumps({"retired_input_source_caches": moved}, ensure_ascii=False))
    elif len(sys.argv) == 3 and sys.argv[1] == "verify":
        verify(Path(sys.argv[2]).resolve())
    else:
        raise SystemExit("usage: shurufa_installation.py prepare | preserve <bundle>")
