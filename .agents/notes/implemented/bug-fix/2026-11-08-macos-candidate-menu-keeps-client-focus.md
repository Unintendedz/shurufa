# Agent Note: macOS 候选右键菜单不抢键盘焦点

Status: implemented

## Problem

在 Zen 浏览器里右键候选，候选窗当场消失，菜单虽然弹出来了，点哪一项都没反应；TextEdit 等别的应用不出现。

本机统一日志给出的顺序是确定的（2026-11-08 08:22:18 一次复现）：

1. 08:22:18.072 WindowServer 收到右键按下。
2. 08:22:18.079 `StealKeyFocusReturningID`：WindowServer 告诉 Zen 的连接它把键盘焦点丢给了 Shurufa，`Deferring events from frontmost process (Zen)`。这是 NSMenu 开始跟踪时做的事。
3. 08:22:18.101 输入法进程里 `NSPopupMenuWindow` 挂到 `MSIMECandidatePanel` 下。
4. 08:22:18.111 输入法进程 `(InputMethodKit) Inserting text` 加一次清空组字，08:22:18.120 候选窗 `order window ... op: 0`。
5. 08:22:18.191 右键才松开。

也就是说，候选窗不是被右键本身关掉的，而是 NSMenu 抢走了 Zen 的键盘焦点；Gecko 把失去键盘焦点当成失焦，自己结束组字，输入法跟着收到 `commitComposition:`（Shurufa 版走 `commitShurufaRawComposition` 把字母原样上屏），候选窗收起，`candidateMenuAction:` 看到 `_panel.isVisible` 为假，菜单里的每一项都直接返回。NSTextView 一类客户端失去键盘焦点时不结束组字，所以只有 Gecko 系的应用出这个问题。

## Decision

候选右键菜单不再用 NSMenu 弹出，改由 `MSIMEPointerMenu`（`platforms/macos/src/candidate/CandidatePointerMenu.h`）按同一个 NSMenu 的内容画成一组不能成为 key 的非激活面板。键盘焦点始终留在客户端，WindowServer 不会再发 StealKeyFocus。

- 菜单模型不变：`menuForCandidate:` 照旧建 NSMenu，按需创建和帧内复用的规则见[候选菜单按右键请求构建](2026-10-14-macos-candidate-lazy-menu.md)；选中一项时按 `NSMenuItem` 的 action、target 派发，`candidateMenuAction:` 的身份与 token 校验原样生效。
- 截获点在 `MSIMECandidatePanel sendEvent:`：右键按下和 Control 点按命中候选行时，交给行上的 `candidateMenuPresenter`，不再落到 AppKit 的 NSMenu 路径。`MSIMECandidateButton rightMouseDown:` 也走同一个入口。
- 菜单开在候选窗层级加一，与原来 AppKit 的放法一致（[候选窗高于截图遮罩](2026-10-12-macos-candidate-window-above-overlays.md)）；锚在指针处，右侧或下方放不下时翻到另一侧再夹进可见区域。子菜单（固定排位）在指针移上去时开在右侧，放不下开在左侧。
- 收起：点到菜单以外（本进程的本地监视和其他进程的全局监视，点击本身照常落到它的窗口）、候选窗收起、候选重画换了 token、任何按键。按键仍经 IMK 到 `handleEvent:client:`，其中 Esc 只关菜单并被吞掉，和系统菜单一样；别的键关菜单后照常处理。

## Alternatives considered

- **菜单开着时忽略 `commitComposition:`**：改动最小，NSMenu 的外观和键盘导航都保留。不用它是因为结束组字的是 Gecko 自己：`widget/cocoa/TextInputHandler.mm` 的 `IMEInputHandler::CommitIMEComposition` 先经 `KillIMEComposition` 调输入上下文的 `discardMarkedText`（输入法收到的 `commitComposition:` 就来自这里），组字还在就自己把当前标记文字作为已提交文本写进文档。输入法这边不认账，文档里已经多了一段字母，输入法却以为还在组字，下一个键会在后面另起一段组字，比现在更乱。
- **按客户端 bundle id 只对 Gecko 系换菜单**：改动范围更小，其他应用保持系统菜单。不用它是因为这是按应用名单打补丁的启发式；NSMenu 抢键盘焦点是所有客户端都会经历的事，只是别的客户端恰好不结束组字，统一换成不抢焦点的菜单才是确定的。
- **在候选窗里就地展开操作按钮**：不需要另开窗口。不用它是因为要改候选窗的布局和皮肤画法，牵连每一种皮肤和横竖排，菜单这一件事不值得。

## Consequences

- 菜单只能用指针操作：不能用方向键在菜单里移动、回车选中，也不支持按住右键拖到某项松开即选中（右键松开落在候选窗上）。Esc 和点外部都能关。
- 外观是自己画的：菜单材质的 `NSVisualEffectView`、圆角 10、行高 24、选中行用系统强调色，颜色模式跟 `ApplyMetasequoiaMenuTheme` 给 NSMenu 设的 appearance 一致；和系统菜单在细节上（字距、动画）不完全一样。
- NSMenu 跟踪期间主线程处在事件跟踪模式，异步到达的候选更新会被推迟；现在主线程照常运行，候选在菜单开着时重画就会收起菜单，免得留下点了没反应的项。
- 测试：`candidate-pointer-menu` 断言菜单窗口和子菜单窗口都不能成为 key、带非激活样式、层级与锚点、子菜单的开合与高亮、分隔线和不可用项不派发、左右键松开派发并收起、贴屏幕右下角时翻转后留在可见区域。`shortcut` 断言候选窗收到右键和 Control 点按时开的是这个菜单、层级为候选窗加一、没有上屏，Esc 关菜单且不向会话发命令、候选窗仍在。
- 未验证：自动化测试不经过 WindowServer，证明不了真实客户端不再失焦；装机后需要在 Zen 里右键候选，确认候选窗留着、菜单项生效，统一日志里不再出现 `StealKeyFocusReturningID`。`shortcut` 在这次改动之前就在第 14451 行（自定义取色器的边框色）断言失败，与本改动无关；本地验证时临时跳过那一行跑完了全套。
