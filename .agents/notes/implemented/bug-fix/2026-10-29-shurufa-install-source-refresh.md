# Agent Note: Shurufa 重装刷新输入源并核对系统启用记录

Status: implemented

## Problem

用户报告重装后系统设置保留 Shurufa 条目，但系统输入菜单只有拼音和 ABC，必须手动删除再添加。当前安装器注销 LaunchServices、换包和重新登记，但通用 `MSIMERegisterAndEnableInputSources` 在发现已有输入源时直接返回，不执行启用。安装验收只检查可枚举、唯一登记与唯一进程，无法区分系统设置中残留条目和实际可选的模式。

本轮只读证据中，TIS 的 Hans 报 enabled/select-capable，父来源 enabled/not-select-capable，图标 URL 与 LaunchServices 都指向安装目录；HIToolbox 的 `AppleEnabledInputSources` 却没有本 bundle。当前 host 范围没有另一份 HIToolbox 域。用户截图也显示设置条目存在、菜单无 Shurufa。旧 Roman 在新包登记后已从 TIS 消失，不能把 Roman 的移除单独判为所有重装问题的根因。未取得手动删加前后的完整系统通知，缺少的事实不作推断。

## Decision

### 2026-11-25 更新：原地覆盖安装 + 实时 TIS 父输入源在线自愈（替代「移走再注销」路径）

在 macOS 27 实机复测中定位到「每次重编重装后必须到系统设置删除 Shurufa 再重新添加」的直接根因：

1. `install-shurufa.sh` 先把已安装的 `~/Library/Input Methods/Shurufa.app` `mv` 到备份目录，再对那份唯一的已注册 bundle 执行 `lsregister -u`，导致 LaunchServices 发出注销事件，macOS（`TextInputMenuAgent`）随之把父输入源 `dev.shurufa.inputmethod.Shurufa` 置为 `enabled=false` 并从实时已启用列表 `TISCreateInputSourceList(nil, false)` 中移除；
2. 随后的 `--register-input-source` 在 `includeAllInstalled=true` 下仍能看到子模式 `.Hans` 呈默认 `enabled=true`，于是判定 `previouslyInstalled == YES` 早返回，不再重新启用父容器；而原有的 `verify` 只查持久 plist 未查实时 TIS 父源，导致父源离线被漏检。

为此将安装流程更新为两层根治：

1. **全程保留安装路径与 inode 原地同步（`install-shurufa.sh`、`shurufa_installation.py`）**：直接用 `ditto` 备份到非 `.app` 目录 `Shurufa.bundle-backup` 并改名 `SavedContents`，再用 `rsync -a --delete` 原地同步新包内容到 `~/Library/Input Methods/Shurufa.app/` 并 `codesign --verify --deep --strict`。整个安装过程不再移走已安装的 `Shurufa.app`，也绝不对已安装副本执行 `lsregister -u`；`unregister()` 同步增加先查 `registered_paths` 仅在确实登记时才调 `lsregister -u` 的保护，避免对未登记的构建目录发出多余注销事件。
2. **只补启用、永不停用的实时 TIS 自愈与双层验收（`input_method_main.mm`、`shurufa_installation.py`）**：新增 `--ensure-input-source-enabled`，在 `--register-input-source` 之后检查实时 `TISCreateInputSourceList(bundleFilter, false)`；仅当父容器 `dev.shurufa.inputmethod.Shurufa` 或全部有效模式缺失于实时已启用列表时，对父容器及当前已启用且在新包 `Info.plist` 声明范围内的模式（无则回落主模式 `.Hans`）调用 `TISEnableInputSource` 并发出 `AppleInstalled/EnabledInputSourcesChangedNotification` 通知（永不调用 `TISDisableInputSource`，永不抢切当前输入源）。`verify` 阶段同时轮询 `cfprefsd` 落盘（最长 3 秒，检查 `com.apple.HIToolbox` 与 `com.apple.inputsources`）并强制核验 `Shurufa --input-source-enabled == true`，确保父容器实时在线才算安装通过。

### 早期基线决策（2026-10-29）

早期曾引入 `--refresh-installed-input-sources` 的 disable/register/enable 刷新接口并保留其单测，但实测在 macOS 27 上先调 `TISDisableInputSource` 反而会令输入源掉出菜单，后续已改为保留在代码中的兼容入口、由安装脚本走不调用 `TISDisableInputSource` 的原地替换与在线自愈路径。

保留既有 `mode_sync` 安全日志。用户已停止微信调查和事件采样，本批不增加为该调查准备的激活/停用日志，也不再启动事件监听。

## Historical audit

- [保留用户删除的输入源](2026-10-13-macos-preserve-input-source-selection.md)：部分重叠。通用重新登记与普通启动保持，本篇仅取代 Shurufa 明确安装操作的早返回策略。
- [首次安装需要重新登录](2026-10-09-macos-first-install-login-required.md)：保留首次登记仍不可枚举时的限制。当前源可枚举，不能据此要求用户重新登录或重启系统服务。
- [系统拼音边界](../simplification/2026-10-29-shurufa-system-pinyin.md)：保留运行时永不主动选源；安装期刷新也不恢复选中来源。

## Alternatives considered

- 仅再次调用 TISRegisterInputSource：不改变模式选择，改动最小；但现有流程已经这样做，不能补齐缺失的启用刷新。
- 直接写 AppleEnabledInputSources 或重启系统进程：可能迫使界面重建，但绕过 TIS 状态管理、影响其他输入源，不采用。
- 每次普通启动都重新启用所有模式：容易让入口重现，却会恢复用户删除的可选模式，因此只在明确安装时恢复之前的有效选择。

## Testing

合成 TIS 来源验证 disable/register/enable 顺序、登记后重新取得对象、去除 Roman、保留双拼选择、首次安装 Hans、静默 noErr 未生效、登记失败与零选择调用。纯 Python 测试验证持久启用列表缺失、合法父来源/模式和退役模式残留。所有用例均不触碰系统输入源。

## Consequences

安装成功的要求增加了实际启用状态与持久记录，失败会保留输出和备份并停止，不能再把文件替换成功当作菜单可用。API 在目标 macOS 上能否完成与手动删加相同的刷新，仍需经授权装机与用户菜单验收；合成测试不能代替该层证据。微信回退是否另有系统或客户端原因，按实际事件链单独判断。
