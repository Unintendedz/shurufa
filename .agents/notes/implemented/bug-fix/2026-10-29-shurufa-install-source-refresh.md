# Agent Note: Shurufa 重装刷新输入源并核对系统启用记录

Status: implemented

## Problem

用户报告重装后系统设置保留 Shurufa 条目，但系统输入菜单只有拼音和 ABC，必须手动删除再添加。当前安装器注销 LaunchServices、换包和重新登记，但通用 `MSIMERegisterAndEnableInputSources` 在发现已有输入源时直接返回，不执行启用。安装验收只检查可枚举、唯一登记与唯一进程，无法区分系统设置中残留条目和实际可选的模式。

本轮只读证据中，TIS 的 Hans 报 enabled/select-capable，父来源 enabled/not-select-capable，图标 URL 与 LaunchServices 都指向安装目录；HIToolbox 的 `AppleEnabledInputSources` 却没有本 bundle。当前 host 范围没有另一份 HIToolbox 域。用户截图也显示设置条目存在、菜单无 Shurufa。旧 Roman 在新包登记后已从 TIS 消失，不能把 Roman 的移除单独判为所有重装问题的根因。未取得手动删加前后的完整系统通知，缺少的事实不作推断。

## Decision

Shurufa 安装器在注销旧登记之前只读保存已启用模式，然后调用新的安装专用刷新命令：先停用旧模式再停用父来源、读回停用状态、登记新包、重新取得来源对象、先启用父来源，再恢复安装前仍被新包声明的模式。没有可恢复模式时启用主模式 Hans。已退役模式不恢复，新可选模式不擅自加入。普通启动与通用重新登记继续保留原语义。

所有操作只通过 TIS 接口完成，不直接改写 HIToolbox plist，不调用选择输入源接口，不重启系统进程。API 返回 noErr 还必须读回启用和可选择属性；刷新失败不能被“注册表可枚举”掩盖。安装后另查 `AppleEnabledInputSources`，要求存在本 bundle 的有效父来源或已声明模式，拒绝旧 Roman 等无效模式条目。父来源是模式容器，正常情况下不可选择；验收的是其已启用以及实际模式可选择。

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
