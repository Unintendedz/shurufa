#pragma once
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreGraphics/CoreGraphics.h>

// 组合键语音快捷键靠主动 CGEventTap 在系统层吞键，键盘事件的 tap 要两项授权：「辅助功能」才能拦下事件，「输入监控」才能收到按键。
// 只有辅助功能时 CGEventTapCreate 对 KeyDown/KeyUp 返回空，鼠标和 FlagsChanged 的 tap 却能建成（2026-11-03 在 macOS 27 的已安装进程里实测）。
struct MSIMEVoiceShortcutPermissions {
    bool accessibility = false;
    bool inputMonitoring = false;
    bool granted() const { return accessibility && inputMonitoring; }
};

static inline MSIMEVoiceShortcutPermissions MSIMECurrentVoiceShortcutPermissions() {
    return {AXIsProcessTrusted() != 0, CGPreflightListenEventAccess()};
}

// 只请求缺的那几项。系统会把输入法加进对应的授权列表并弹出提示，用户在系统设置里打开开关即可。
static inline void MSIMERequestVoiceShortcutPermissions(MSIMEVoiceShortcutPermissions current) {
    if (!current.accessibility)
        AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)@{(__bridge id)kAXTrustedCheckOptionPrompt: @YES});
    if (!current.inputMonitoring) CGRequestListenEventAccess();
}

// 系统设置里第一项缺的授权所在的隐私页面；都已授权时返回 nil。
static inline NSURL *MSIMEVoiceShortcutPermissionsPane(MSIMEVoiceShortcutPermissions current) {
    if (current.granted()) return nil;
    NSString *pane = current.accessibility ? @"Privacy_ListenEvent" : @"Privacy_Accessibility";
    return [NSURL URLWithString:[@"x-apple.systempreferences:com.apple.preference.security?" stringByAppendingString:pane]];
}
