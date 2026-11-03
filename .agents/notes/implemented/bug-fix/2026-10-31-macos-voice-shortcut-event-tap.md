# Agent Note: macOS 语音组合键在系统层吞键

Status: implemented

## Problem

语音快捷键绑定为右 Option+← 时，在 Electron 应用（现场是 DeepSeek Harness）里按下，光标先按词左移，识别结果随后插在前一个词左边。

IMK 路径对这次按键返回了 YES，但 Chromium 的 `RenderWidgetHostViewCocoa keyEvent:` 不读输入法的返回值：`interpretKeyEvents:` 之后只要没有 marked text、也没有插入多于一个字符，就照常 `ForwardKeyboardEventWithCommands` 把 RawKeyDown 交给网页（`content/app_shim_remote_cocoa/render_widget_host_view_cocoa.mm`）。Blink 的 Mac 默认按键表把 Option+← 映射为 `MoveWordLeft`（`third_party/blink/renderer/core/editing/editing_behavior.cc`）。所以输入法只靠 `handleEvent:` 返回值吞不掉「修饰键+普通键」，原生 Cocoa 控件之外的客户端都可能先执行应用自己的动作。裸修饰键绑定（默认右 Option）不产生按键动作，不受影响。

## Decision

主键不是修饰键的有效自定义语音绑定，由进程级的主动 CGEventTap（`kCGSessionEventTap`、`kCGEventTapOptionDefault`，只订阅 KeyDown/KeyUp）在按键到达任何应用之前丢弃，丢弃的事件以快照异步交给主线程的 `observeGlobalVoiceShortcutEvent:`，沿用原有状态机与 `MSIMEVoiceEventReceipts` 去重。实现在 `platforms/macos/src/voice/VoiceShortcutTap.h`。

### 何时武装

`MSIMERefreshVoiceShortcutTap()` 只在主线程运行。语音开启、`MSIMEVoiceSwallowBinding` 返回绑定，并且 `MSIMEFocusedController` 或 `MSIMEVoiceRecordingOwner` 存在时武装，否则 `CGEventTapEnable(tap, false)`。水杉不是当前输入源、也没在录音时 must never 吞键。刷新点是 `activateServer:`、`deactivateServer:`、`voiceProviderSettingsChanged:`、录音所有者的设置与释放，以及进程内一次注册的 `NSUserDefaultsDidChangeNotification`。

### 丢弃规则

`MSIMEVoiceShortcutSwallow` 是 tap 线程上的纯判定，与状态机共用 `MSIMEVoiceComboMatches`（键码、四个修饰键位、`right_modifiers` 侧别），两边认定的永远是同一个组合：

- 匹配的 KeyDown（含自动重复）丢弃并记下主键；修饰键先松开后，同一主键的自动重复仍丢弃。
- 记下的主键的 KeyUp 丢弃并清空，不看当时的修饰键；吞住期间按下的其他键照常放行、不清空，因为应用没见过主键的 KeyDown。
- 同一主键新的非重复且不匹配的 KeyDown 清空并放行。FlagsChanged 和带 `MSIMEVoiceCommitEventTag` 的自投事件一律放行。

### 线程与权限

tap 的 run loop source 挂在专用线程上；回调只读原子状态、做判定、复制事件并 `dispatch_async` 到主线程，never 同步等待主线程，主线程卡住时全系统按键不跟着卡。被系统因超时禁用时，仍武装就在回调线程直接重新启用。

键盘事件的主动 tap 要两项授权：「辅助功能」（`AXIsProcessTrusted`）和「输入监控」（`CGPreflightListenEventAccess`）。只有辅助功能时 `CGEventTapCreate` 对 KeyDown/KeyUp 返回空，而同一进程里鼠标和 FlagsChanged 的主动 tap 都能建成（2026-11-03 用 lldb 在已安装的进程里实测，macOS 27）。最初只检查辅助功能，装机后 tap 从未建成，右 Option+← 照旧按词左移。判定与请求在 `platforms/macos/src/voice/VoiceShortcutPermissions.h`：缺任何一项时 `ensureTap` 不建 tap，行为退回只靠 IMK 返回值；每个进程只请求一次缺的那几项（`AXIsProcessTrustedWithOptions` 和 `CGRequestListenEventAccess`），之后的刷新在两项都授权后重试创建。不加浮层提示；语音设置的快捷键卡片有「系统权限」一行，显示两项的状态，绑定需要吞键而缺授权时标红，「去授权…」请求缺的授权并打开系统设置里对应的隐私页面，窗口重新成为 key window 时重读。

本篇部分取代[单一语音会话决定](../feature/2026-10-28-shurufa-single-voice-session.md)中「全局只读监听不能吞掉其他应用的快捷键」的取舍：组合键绑定在授权后由系统层吞掉；录音期间的只读监听和它的回调边界保持不变。

## Alternatives considered

- Carbon `RegisterEventHotKey`：成熟、不需要额外权限，系统在分发前直接吞键。但它分不出左右 Option，会把左 Option+← 一并吞掉，推翻[区分左右修饰键](../feature/2026-10-28-macos-configurable-voice-shortcut.md)只让右侧触发、左侧照常按词移动的设计。
- 吞键时临时设置 marked text，让 Chromium 把这次按下改发 `VKEY_PROCESSKEY`：不需要权限，但依赖特定客户端的内部判定，会向网页派发组合事件，属于按客户端区别对待的启发式。
- 不改代码，只建议改用应用不响应的键：零成本，但任何「修饰键+普通键」绑定在 Chromium 系客户端里都会重现同一问题。

## Consequences

收益：组合键绑定在原生、Electron/Chromium 和终端里表现一致，按键不会先触发应用动作；左右侧别判定与状态机共用同一函数，不会分叉。

代价：
- 组合键绑定要求辅助功能和输入监控两项授权，缺一项时仍有原问题。`build-shurufa.sh` 用本机自签名证书签名，重编不再丢授权（见 [Shurufa 本机构建用固定的自签名证书签名](../process/2026-11-03-shurufa-local-signing-identity.md)）；钥匙串里没有证书而退回 ad-hoc 时，每次重新构建仍会要求重新授权。
- 武装期间全系统的 KeyDown/KeyUp 都经过一次 tap 回调。回调只做常数时间判定，线程独立。
- 设置窗口里重录同一组合键时，系统层已经吞掉这次按键，录键框收不到它；重录成别的组合不受影响。

### Testing

`voice-shortcut-swallow` 用合成 flags 覆盖右侧丢弃（按下、重复、松开）、左侧放行、无侧别绑定、修饰键先松开、额外修饰键放行、吞住期间按其他键、FlagsChanged 与自投事件放行、同键不匹配时清空，以及 `MSIMEVoiceSwallowBinding` 对裸修饰键、无侧别和功能键的取值。`configured-voice-shortcut`、`voice-hold-shortcut` 不改并通过，四个语音控制器测试（编译 `InputController.mm`）通过。真实 tap 线程、授权弹窗和 Electron 客户端里光标不再移动，需要装机后用户实测，单测不冒充这部分。
