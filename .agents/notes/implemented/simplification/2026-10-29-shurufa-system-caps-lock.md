# Agent Note: Shurufa 将 Caps Lock 交还系统

Status: implemented

## Problem

Shurufa 先前开启的 Caps 轻点策略启动 HID 监听和 10 毫秒轮询，撤回系统锁定位并切换内部中英文。这与用户明确选择的系统 ABC 切换方式冲突。用户要求完全停止 Shurufa 对 Caps 的接管。

## Decision

Shurufa 的 Caps 事件取消待定 Shift/Control 轻点后交还系统，避免 Shift 按下、Caps、Shift 松开被误判为单击切换。实体状态读取、HID 启动、轮询、会话状态报告与大写首字母特殊放行均不进入，旧偏好 true 不能恢复。原生按键页不显示 Caps 开关。共享出厂值关闭；安装使用 V2 标记把共享与本机旧值迁为 false，保存失败不写完成标记。其他 edition 保持既有行为。

本决定取代[Shurufa Caps 默认开启](../bug-fix/2026-10-28-shurufa-caps-lock.md)中的默认值和迁移选择，原 Shift 大写修复仍保留。检索发现的[通用语言切换](../feature/2026-10-11-caps-lock-language-switch.md)和 HID 笔记仍持有其他 edition 机制；单一语音快捷键笔记部分重叠，仅同步其 Caps 边界。系统拼音的其余对齐由后续执行者处理。

## Alternatives considered

仅默认关闭可保留用户自选能力，改动较少，但旧配置能重新开启监听，不能满足完全交还系统的目标。

## Consequences

Shurufa 不再提供内部 Caps 中英文切换，系统的 Caps 键输入源设置由用户管理。合成控制器回归覆盖旧 true、Caps 开关事件、轮询入口、零 HID 启动和撤回、零会话报告、设置树控件移除及 V1 到 V2 迁移；物理键盘与系统 ABC 切换仍由用户装机验收。

## System capability declaration

控制器收到 Caps 的 flagsChanged 时已经返回 NO；`recognizedEvents` 保留 flagsChanged 是为了单修饰键语音绑定，不代表消费每个 Caps 事件。实际包缺少 macOS 10.15 起支持的 `TICapsLockLanguageSwitchCapable`，系统开关因而没有本产品的能力声明。Shurufa 生成的顶层 Info.plist 现在声明 true，系统设置继续决定 Caps 与 ABC 切换，不新增手动 TISSelectInputSource 路径，也不修改锁定位或大写灯。原生旧 true/Caps 合成回归与产物声明检查共同守住这条路径。

依据：[Apple 属性声明](https://developer.apple.com/documentation/bundleresources/information-property-list/ticapslocklanguageswitchcapable)。物理 Caps 和系统设置仍需装机后由用户验收。
