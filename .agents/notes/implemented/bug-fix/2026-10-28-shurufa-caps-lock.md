# Agent Note: Shurufa 开启 Caps Lock 切换并保留 Shift 大写

Status: implemented

## Problem

现有 Shurufa 安装的共享偏好和运行时偏好中 `switch_language_caps_lock` 均为 false，本地 NSUserDefaults 没有单独设置，故 Caps Lock 不能切换。即使打开开关，控制器撤回系统锁定后仍会把 Shift+A 的 A 强制降为 a；新增真实控制器合成回归在修复前失败。

## Decision

默认开启与安装迁移现由[交还系统 Caps Lock](../simplification/2026-10-29-shurufa-system-caps-lock.md)取代；以下保留当时理由，Shift 大写修复仍适用。

仅 Shurufa 的新装默认打开 Caps Lock 轻点切换。安装脚本执行一次有版本标记的偏好迁移，通过现有共享偏好 API 只改该字段，再同步本地 NSUserDefaults，并重新生成运行时快照；保存失败不标记完成。此后用户在设置页关闭该功能，重复安装保留关闭选择。

英文输入撤回旧 Caps Lock 位时，显式按住 Shift 的字母不再降为小写。沿用已有确定性的实体按键状态、300 毫秒阈值和先撤回锁定再切换的规则。轻点切换后系统大写锁定关闭，长按保留大写锁定；已有锁定时再次按下只关闭锁定。不模拟键盘输入，不改用户系统的 Caps Lock 输入源设置。

### 既有笔记

[Caps Lock 语言切换](../feature/2026-10-11-caps-lock-language-switch.md)、[HID 状态](2026-10-24-macos-caps-lock-hid-state.md)仍持有跨平台判定与设备回退决定；本篇只增加用户明确授权的 Shurufa 默认值与安装迁移，并修正 Shift 输出。

## Alternatives considered

- 每次启动强制打开：实现最少，但会覆盖设置页里的关闭选择。
- 改所有 edition 的默认值：其他用户也能直接使用，但超出本次产品范围。

## Consequences

新装和这次升级都会启用切换，其他偏好不变。非 Shurufa 默认仍关闭。自动化覆盖原生控制器轻点、长按、松开、撤回失败、Shift 大写和迁移失败/重复执行；不替代物理键盘验收。系统自身的“使用大写锁定键切换 ABC”会先处理该键；不报实体状态且没有输入监控权限的外接键盘仍保留系统大写锁定，这两个既有边界没有被合成测试证明消失。
