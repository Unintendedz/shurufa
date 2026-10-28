# Agent Note: macOS 语音快捷键录制和统一触发模式

Status: implemented

## Problem

Shurufa 继承的语音键有两种行为：默认开启的右 Option 按住说话，默认开启的 Control+F9 切换录音；Control+Command、右 Control+右 Option 默认关闭。旧界面虽写 Control+Option+V，实际额外绑定默认空，故本次移除错误说明，不新增默认键。用户需要录制右 Command 等单侧修饰键或组合键，并自行选择按住或切换。

## Decision

共享 `voice_input` 新增 `hotkey_mode`（legacy、hold、toggle）和可空的 `macos_hotkey`（数值 key_code、modifiers）。旧文档缺字段时保持 legacy，没有自定义键。修饰键掩码固定为 Control=1、Option=2、Shift=4、Command=8；主键用 macOS 物理键码区分左右。Rust 校验和原生字典类型校验拒绝无修饰普通字符、Esc、Caps Lock 和畸形数值。录制还以实际输入法偏好拒绝中英文、简繁、全半角、标点、英文候选的切换冲突。

设置沿用现有 AppKit 表单，显示当前绑定、录制和清除按钮，以及三档模式。自定义键覆盖内置键但不清除其选择；清除后恢复内置键。保存先经共享偏好 API 及 revision 校验，再镜像到 NSUserDefaults，失败不报告成功。

设置表单的整理开关优先读取 `MSIMEClientVoicePolish`，再兼容旧 `voiceInput.polishEnabled`，两者缺失时 TextStream 默认开。旧窗口只读私有字典，还用是否有密钥决定默认开关，可能把实际开启的整理显示为关闭。现场白名单查询确认共享文档和 NSUserDefaults 都是 textstream、整理 true，旧字典没有 polishEnabled。原生浅/深截图使用合成值并画完整窗口底色，未展示本机凭据。

legacy 保留所有内置行为。legacy 配自定义单个修饰键时按住说话，配组合键时切换；录制本身不更改模式。hold 和 toggle 对所有入口统一生效，包括全局事件监视器接住的 Control+F9 与额外组合键。裸修饰键的 toggle 等松开才触发，期间有其他键（包括同组另一侧）就取消判定。hold 按下立即启动，期间普通组合键取消录音并原样交回应用，不提交文字，不请求整理；空格仍可锁定按住录音。

完整共享偏好验证检查自定义语音键与模式切换的冲突；原生反向开启 Shift/Control 等开关也受占用限制，并显示中文提示。原有按键页使用中性键名、当前模式说明和与语音页一致的禁用状态。TextStream 整理开启但密钥尚未填写时可以保存识别设置，运行时仍直接上屏原文；填写了密钥后的无效整理地址仍拒绝保存。

录音中 Esc 取消；失焦、换应用、改语音设置沿用原有取消流程。首次权限询问期间松开的按住键不会在授权后突然开始录音。输入法不是当前输入源或没有文本客户端时不启动。

### 既有笔记

[可重绑语音切换键](2026-10-24-custom-voice-toggle-hotkey.md)仍持有跨平台额外组合键的规则；本篇增加 macOS 物理单侧绑定和触发模式。[右 Option 状态](../bug-fix/2026-10-23-macos-voice-option-modifier-flags.md)的事件侧别优先规则保留。原生设置冻结的常规约束在本次 Shurufa 无 Tauri 产品的明确需求下不适用。

## Alternatives considered

- 把出厂模式直接设为 hold：统一实现简单，但会改变 Control+F9 原来的切换行为。保留 legacy 兼容档，用户选择后才统一。
- 单修饰键延迟一段时间再开始：减少普通组合键短暂开麦，但引入时间猜测和额外延迟。使用确定的按键状态；介意短暂开麦的用户可选择 toggle 或普通组合键。

## Consequences

按住模式的单修饰键用于普通快捷键时会短暂开麦并出现浮层，按下其他键立即取消；这是已批准的取舍。合成测试覆盖左右修饰键、切换/按住/兼容、修饰键提前释放、重复事件、Esc、共享偏好往返、取消后迟到转写不落字且整理提交数为零。实际键盘和其他应用的 IMK 分发仍需用户验收。物理绑定随键位，显示的字母采用固定主键表；非 QWERTY 布局的字母标注不等于该布局的字符。
