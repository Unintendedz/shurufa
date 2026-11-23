#pragma once
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <InputMethodKit/InputMethodKit.h>
#import "../core/AXPrecedingContext.h"
#import "VoiceContextCapture.h"
#include <time.h>

/// 当前焦点目标是否能够接收语音文本直接上屏。
typedef NS_ENUM(NSInteger, MSIMEVoiceTargetEditableState) {
    MSIMEVoiceTargetEditableStateEditable = 0,
    MSIMEVoiceTargetEditableStateNotEditable = 1,
    MSIMEVoiceTargetEditableStateUnknown = 2,
};

/// 目标可编辑性判定的纯输入快照，便于在无 GUI 环境下用单元测试覆盖所有边界条件。
struct MSIMEVoiceTargetEditableInput {
    NSString * _Nullable frontmostBundleID = nil;
    NSString * _Nullable clientBundleID = nil;
    BOOL axTrusted = NO;
    BOOL axBlacklisted = NO;
    BOOL hasFocusedAXElement = NO;
    NSString * _Nullable axRole = nil;
    NSString * _Nullable axSubrole = nil;
    BOOL axEnabled = YES;
    BOOL axHasEditableAncestor = NO;
    BOOL axValueSettable = NO;
    BOOL axHasSelectedTextRange = NO;
    BOOL clientSelectedRangeValid = NO;
};

/// 附带诊断信息的探测结果（仅记录状态与角色/错误码，绝不含用户输入内容）。
struct MSIMEVoiceTargetEditabilityProbeResult {
    MSIMEVoiceTargetEditableState state = MSIMEVoiceTargetEditableStateUnknown;
    BOOL axTrusted = NO;
    BOOL frontmostMatchesClient = YES;
    AXError focusError = kAXErrorSuccess;
    NSString * _Nullable axRole = nil;
};

/// 终端与自绘编辑器不总是按标准 AXTextField 暴露可编辑属性；当前台应用与 IMK 客户端一致时默认放行。
static inline BOOL MSIMEIsCustomInputTargetBundleID(NSString * _Nullable bundleID) {
    if (!bundleID.length) return NO;
    if (msime::mac::IsTerminalBundleIdentifier(bundleID)) return YES;
    NSString *lower = bundleID.lowercaseString;
    if ([lower hasPrefix:@"com.jetbrains."]) return YES;
    return [lower isEqualToString:@"com.github.wez.wezterm"] ||
           [lower isEqualToString:@"com.sublimetext.4"] ||
           [lower isEqualToString:@"com.sublimetext.3"] ||
           [lower isEqualToString:@"org.gnu.emacs"] ||
           [lower isEqualToString:@"org.vim.macvim"] ||
           [lower isEqualToString:@"com.neovide.neovide"] ||
           [lower isEqualToString:@"dev.zed.zed"] ||
           [lower isEqualToString:@"dev.zed.zed-preview"] ||
           [lower isEqualToString:@"org.blenderfoundation.blender"] ||
           [lower isEqualToString:@"org.xquartz.x11"];
}

/// 纯函数判定目标是否可接收文本输入：在已授权且非黑名单的 AX 路径采用“正向可编辑白名单”，
/// 彻底杜绝 Chromium/Electron/WebKit 只读页面（带 AXSelectedTextRange 的 AXWebArea/AXGroup）或 Finder 桌面静默吞字。
static inline MSIMEVoiceTargetEditableState MSIMEClassifyVoiceTargetEditability(
    const MSIMEVoiceTargetEditableInput &input) {
    // 1. 前台应用与 IMK 客户端不一致时（例如已切到 Finder/桌面/其他窗口但迟迟未收到 deactivateServer:），确定不可直接上屏。
    if (input.frontmostBundleID.length && input.clientBundleID.length &&
        [input.frontmostBundleID caseInsensitiveCompare:input.clientBundleID] != NSOrderedSame) {
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    // 2. 终端与自绘编辑器：只要前台应用匹配，不依赖标准 AX 角色即可安全接收 IMK 上屏。
    if (MSIMEIsCustomInputTargetBundleID(input.frontmostBundleID ?: input.clientBundleID)) {
        return MSIMEVoiceTargetEditableStateEditable;
    }

    // 3. 无辅助功能权限或命中 AX 防卡黑名单时保持保守未知（允许由匹配前台的 IMK 客户端尝试上屏）。
    if (!input.axTrusted || input.axBlacklisted) {
        return MSIMEVoiceTargetEditableStateUnknown;
    }

    // 4. 已授权 AX 但应用当前没有任何焦点 UI 元素（例如点击了桌面、Finder 空白处、窗口标题栏）。
    if (!input.hasFocusedAXElement) {
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    // 5. 控件明确处于禁用状态时不可输入。
    if (!input.axEnabled) {
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    NSString *role = input.axRole ?: @"";
    NSString *subrole = input.axSubrole ?: @"";

    // 6. 正向证明 A：明确的文本输入角色或子角色。
    if ([role isEqualToString:(__bridge NSString *)kAXTextFieldRole] ||
        [role isEqualToString:(__bridge NSString *)kAXTextAreaRole] ||
        [role isEqualToString:(__bridge NSString *)kAXComboBoxRole] ||
        [role isEqualToString:@"AXSearchField"] ||
        [subrole isEqualToString:@"AXSearchField"] ||
        [subrole isEqualToString:(__bridge NSString *)kAXSecureTextFieldSubrole]) {
        return MSIMEVoiceTargetEditableStateEditable;
    }

    // 7. 正向证明 B：WebKit / Chromium / Electron 富文本或 contenteditable 具备 AXEditableAncestor。
    if (input.axHasEditableAncestor) {
        return MSIMEVoiceTargetEditableStateEditable;
    }

    // 8. 正向证明 C：AXValue 可写，且排除数值/开关/选项类非文本控件（滑块、复选框、下拉按钮、滚动条等）。
    if (input.axValueSettable) {
        static NSSet<NSString *> *nonTextValueRoles = nil;
        static dispatch_once_t once;
        dispatch_once(&once, ^{
            nonTextValueRoles = [NSSet setWithArray:@[
                @"AXSlider",
                @"AXCheckBox",
                @"AXRadioButton",
                @"AXPopUpButton",
                @"AXColorWell",
                @"AXSplitter",
                @"AXScrollBar",
                @"AXTabGroup",
                @"AXStepper",
                @"AXIncrementor",
                @"AXSwitch",
                @"AXProgressIndicator",
                @"AXBusyIndicator",
                @"AXValueIndicator",
                @"AXDisclosureTriangle",
            ]];
        });
        if (![nonTextValueRoles containsObject:role]) {
            return MSIMEVoiceTargetEditableStateEditable;
        }
    }

    // 9. 其余全部（AXWebArea、AXDocument、AXGroup、AXStaticText、AXButton、AXIcon、AXBrowser、AXWindow 等，
    // 即使只读页面暴露了 AXSelectedTextRange 用于划词复制）一律判定为不可编辑，防止静默丢字。
    return MSIMEVoiceTargetEditableStateNotEditable;
}

/// 探测给定 IMK 客户端当前是否处于真实可输入状态（带完整诊断信息）；总 AX 超时硬限制 25 ms，不弹窗申请权限。
/// 对无 `bundleIdentifier` 的合成测试夹具返回 Unknown（放行既有单测路径）。
static inline MSIMEVoiceTargetEditabilityProbeResult MSIMEProbeVoiceTargetEditabilityDetailed(id _Nullable client) {
    MSIMEVoiceTargetEditabilityProbeResult result;
    if (!client || ![client respondsToSelector:@selector(bundleIdentifier)]) {
        return result;
    }
    NSString *clientBundleID = nil;
    @try {
        id raw = [client bundleIdentifier];
        if ([raw isKindOfClass:NSString.class]) clientBundleID = [(NSString *)raw copy];
    } @catch (NSException *exception) {
        (void)exception;
    }
    if (!clientBundleID.length) {
        return result;
    }

    NSRunningApplication *frontmost = NSWorkspace.sharedWorkspace.frontmostApplication;
    MSIMEVoiceTargetEditableInput input;
    input.frontmostBundleID = frontmost.bundleIdentifier;
    input.clientBundleID = clientBundleID;

    // 前台应用已经切走时不必再做任何跨进程 AX 查询。
    if (input.frontmostBundleID.length &&
        [input.frontmostBundleID caseInsensitiveCompare:clientBundleID] != NSOrderedSame) {
        result.frontmostMatchesClient = NO;
        result.state = MSIMEVoiceTargetEditableStateNotEditable;
        return result;
    }

    if (MSIMEIsCustomInputTargetBundleID(input.frontmostBundleID ?: clientBundleID)) {
        result.state = MSIMEVoiceTargetEditableStateEditable;
        return result;
    }

    if ([client respondsToSelector:@selector(selectedRange)]) {
        @try {
            const NSRange sel = [(id<IMKTextInput>)client selectedRange];
            input.clientSelectedRangeValid = (sel.location != NSNotFound);
        } @catch (NSException *exception) {
            (void)exception;
        }
    }

    input.axTrusted = AXIsProcessTrusted() ? YES : NO;
    result.axTrusted = input.axTrusted;
    input.axBlacklisted = MSIMEIsAXBlacklistedBundleIdentifier(input.frontmostBundleID ?: clientBundleID);
    if (!input.axTrusted || input.axBlacklisted || !frontmost || frontmost.processIdentifier <= 0) {
        result.state = MSIMEClassifyVoiceTargetEditability(input);
        return result;
    }

    const pid_t frontPID = frontmost.processIdentifier;
    AXUIElementRef appElement = AXUIElementCreateApplication(frontPID);
    if (!appElement) {
        result.state = MSIMEClassifyVoiceTargetEditability(input);
        return result;
    }
    AXUIElementSetMessagingTimeout(appElement, 0.025f);

    const uint64_t queryStartNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
    CFTypeRef rawFocused = nullptr;
    AXError focusErr = AXUIElementCopyAttributeValue(appElement, kAXFocusedUIElementAttribute, &rawFocused);
    CFRelease(appElement);

    // 部分系统面板/Finder 场景应用级查询会返回无值，兜底向 SystemWide 查询一次并校验 PID 归属。
    if (focusErr != kAXErrorSuccess || !rawFocused || CFGetTypeID(rawFocused) != AXUIElementGetTypeID()) {
        if (rawFocused) { CFRelease(rawFocused); rawFocused = nullptr; }
        AXUIElementRef sysWide = AXUIElementCreateSystemWide();
        if (sysWide) {
            AXUIElementSetMessagingTimeout(sysWide, 0.020f);
            CFTypeRef sysFocused = nullptr;
            const AXError sysErr = AXUIElementCopyAttributeValue(sysWide, kAXFocusedUIElementAttribute, &sysFocused);
            CFRelease(sysWide);
            if (sysErr == kAXErrorSuccess && sysFocused && CFGetTypeID(sysFocused) == AXUIElementGetTypeID()) {
                pid_t focusPID = 0;
                if (AXUIElementGetPid(static_cast<AXUIElementRef>(sysFocused), &focusPID) == kAXErrorSuccess &&
                    focusPID == frontPID) {
                    rawFocused = sysFocused;
                    focusErr = kAXErrorSuccess;
                } else {
                    CFRelease(sysFocused);
                }
            } else if (sysFocused) {
                CFRelease(sysFocused);
            }
        }
    }

    const uint64_t elapsedMs = (clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - queryStartNs) / 1000000ULL;
    result.focusError = focusErr;
    if (focusErr != kAXErrorSuccess || !rawFocused || CFGetTypeID(rawFocused) != AXUIElementGetTypeID()) {
        if (rawFocused) CFRelease(rawFocused);
        // NoValue / AttributeUnsupported 或非超时的快速 CannotComplete（< 18 ms）均说明目标窗口当前无焦点输入元素。
        if (focusErr == kAXErrorNoValue ||
            focusErr == kAXErrorAttributeUnsupported ||
            (focusErr == kAXErrorCannotComplete && elapsedMs < 18)) {
            input.hasFocusedAXElement = NO;
            result.state = MSIMEClassifyVoiceTargetEditability(input);
            return result;
        }
        result.state = MSIMEVoiceTargetEditableStateUnknown;
        return result;
    }

    AXUIElementRef focused = static_cast<AXUIElementRef>(rawFocused);
    AXUIElementSetMessagingTimeout(focused, 0.020f);
    input.hasFocusedAXElement = YES;

    CFTypeRef rawRole = nullptr;
    if (AXUIElementCopyAttributeValue(focused, kAXRoleAttribute, &rawRole) == kAXErrorSuccess && rawRole) {
        id roleObj = CFBridgingRelease(rawRole);
        if ([roleObj isKindOfClass:NSString.class]) input.axRole = (NSString *)roleObj;
    } else if (rawRole) {
        CFRelease(rawRole);
    }
    result.axRole = input.axRole;

    CFTypeRef rawSubrole = nullptr;
    if (AXUIElementCopyAttributeValue(focused, kAXSubroleAttribute, &rawSubrole) == kAXErrorSuccess && rawSubrole) {
        id subroleObj = CFBridgingRelease(rawSubrole);
        if ([subroleObj isKindOfClass:NSString.class]) input.axSubrole = (NSString *)subroleObj;
    } else if (rawSubrole) {
        CFRelease(rawSubrole);
    }

    CFTypeRef rawEnabled = nullptr;
    if (AXUIElementCopyAttributeValue(focused, kAXEnabledAttribute, &rawEnabled) == kAXErrorSuccess && rawEnabled) {
        id enabledObj = CFBridgingRelease(rawEnabled);
        if ([enabledObj isKindOfClass:NSNumber.class]) input.axEnabled = [(NSNumber *)enabledObj boolValue];
    } else if (rawEnabled) {
        CFRelease(rawEnabled);
    }

    CFTypeRef rawAncestor = nullptr;
    if (AXUIElementCopyAttributeValue(focused, CFSTR("AXEditableAncestor"), &rawAncestor) == kAXErrorSuccess && rawAncestor) {
        input.axHasEditableAncestor = YES;
        CFRelease(rawAncestor);
    } else if (rawAncestor) {
        CFRelease(rawAncestor);
    }

    Boolean valueSettable = false;
    if (AXUIElementIsAttributeSettable(focused, kAXValueAttribute, &valueSettable) == kAXErrorSuccess && valueSettable) {
        input.axValueSettable = YES;
    }

    CFTypeRef rawRange = nullptr;
    if (AXUIElementCopyAttributeValue(focused, kAXSelectedTextRangeAttribute, &rawRange) == kAXErrorSuccess && rawRange) {
        input.axHasSelectedTextRange = YES;
        CFRelease(rawRange);
    } else if (rawRange) {
        CFRelease(rawRange);
    }

    CFRelease(focused);
    result.state = MSIMEClassifyVoiceTargetEditability(input);
    return result;
}

static inline MSIMEVoiceTargetEditableState MSIMEProbeVoiceTargetEditability(id _Nullable client) {
    return MSIMEProbeVoiceTargetEditabilityDetailed(client).state;
}
