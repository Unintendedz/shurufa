# Agent Note: Shurufa 录音期间的普通按键不再打断录音

Status: implemented

## Problem

用户开着语音录音时用方向键挪光标、打字或按别的组合键，录音立刻被取消，已经说的话全部丢掉。用户的要求是：录音是「准备说话」的状态，期间干别的事不该影响它，只有语音快捷键和 Esc 能结束。

源码里有两处取消：

- `handleEvent:client:` 在按键进入普通组字流程前，只要 `_doubaoVoiceRequest` 或 `_liveVoiceToken` 存在就调用取消。TextStream 走 `_doubaoVoiceRequest` 这一路，所以任何一次没被快捷键吃掉的 KeyDown 都会取消录音。这是录音和原客户端绑定时代的规则：那时录音中的文字是原客户端里的 marked text，按别的键等于放弃这段组字。[单一语音会话决定](../feature/2026-10-28-shurufa-single-voice-session.md)已经让 Shurufa 的录音由进程级所有者持有、跨窗口不断，这条规则没有跟着改。
- `MSIMEConfiguredVoiceShortcut` 在快捷键手势仍处于按下状态时收到别的键，会对「本次手势开始了录音」的情况返回 `Cancel`。切换模式下，用组合键（如右 Option+←）开始录音后修饰键还按着、主键 KeyUp 没到（IMK 不转发 KeyUp，tap 不可用时更常见）就按 Option+→，录音也会被取消。

## Decision

Shurufa 持有录音（`ownsPersistentVoiceRecording`）时，`handleEvent:client:` 对普通按键不取消任何语音请求，只调用 `detachVoiceMarkedText` 把可能存在的行内实时文字移到浮层，然后按键照常交给组字流程和应用。其他 edition 不持有跨窗口录音，保持原来的「按键取消」行为。

`MSIMEConfiguredVoiceShortcut` 在手势按下期间遇到别的键时，只在按住模式（`hold_`）下返回 `Cancel`：按住右 Option 时又按了别的键，说明用户其实把它当普通修饰键用，录下的内容不是想说的话。切换模式只把这次手势记为已打断（`cancelled_`），按键继续交给应用，录音不受影响。

录音结束的入口因此只剩：语音快捷键（切换模式再按一次、按住模式松开）、Esc、浮层的完成/取消按钮，以及原有的设置变更、关闭语音和失败路径。

## Alternatives considered

- 按键时结束并提交已识别的文字，而不是取消：不丢内容，但用户只是挪一下光标就被迫结束说话，依然违背「快捷键和 Esc 之外不结束」的要求。
- 按住模式也不因其他键取消：规则更统一。但按住右 Option 再按 ← 正是 macOS 按词移动的常用操作，此时录下的多半不是有意的口述，松开后整理并上屏会把杂音写进文档；保留按住模式的取消，切换模式不受这个歧义影响。

## Consequences

收益：切换模式下录音期间可以自由移动光标、打字、使用快捷键；最终文字仍按[单一语音会话决定](../feature/2026-10-28-shurufa-single-voice-session.md)投递到结束时的焦点位置，所以先挪光标再结束，文字就落在新位置。

代价：录音期间误按的键不再顺带取消录音，想放弃必须按 Esc 或点浮层的取消。打字产生的组字会在最终投递前由 `MSIME_FINISH_COMPOSITION` 收尾，与原有投递顺序一致。

### Testing

- `configured-voice-shortcut`：切换模式右 Option+← 开始后，主键未松开时按 Option+→ 返回 `None` 且不吞键；之后普通 ← 不触发，再按组合键才 `Toggle`。原有按住/legacy 模式下其他键返回 `Cancel` 的断言不变。
- `persistent-voice-controller`：真实控制器分别走系统识别（`_liveVoiceToken`）和 TextStream（`_doubaoVoiceRequest`）两路，录音中经 `handleEvent:client:` 发出方向键、字母、删除、回车、Option+→、Command+A，录音持续且所有者不变、未提交；随后 Esc 取消且不提交。
- 真实键盘、各类应用里的光标移动与最终落点需要装机后由用户验收。
