#pragma once
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <InputMethodKit/InputMethodKit.h>
#import "../core/AXPrecedingContext.h"
#import "VoiceContextCapture.h"

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
    BOOL axHasEditableAncestor = NO;
    BOOL axValueSettable = NO;
    BOOL axHasSelectedTextRange = NO;
    BOOL clientSelectedRangeValid = NO;
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

/// 纯函数判定目标是否可接收文本输入，避免语音结束时往非输入框（桌面、空白网页、按钮、已切走的窗口）静默丢字。
static inline MSIMEVoiceTargetEditableState MSIMEClassifyVoiceTargetEditability(
    const MSIMEVoiceTargetEditableInput &input) {
    // 1. 前台应用与 IMK 客户端不一致时（例如已切到 Finder/桌面/其他非输入法窗口但迟迟未收到 deactivateServer:），确定不可直接上屏。
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

    // 4. 已授权 AX 但应用当前没有任何焦点 UI 元素（如点击了空白标题栏/桌面/只读容器）。
    if (!input.hasFocusedAXElement) {
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    // 5. 明确具备可编辑祖先（WebKit/Chromium `AXEditableAncestor`）或 `AXValue` 可写时判定为可编辑。
    if (input.axHasEditableAncestor || input.axValueSettable) {
        return MSIMEVoiceTargetEditableStateEditable;
    }

    NSString *role = input.axRole ?: @"";
    if ([role isEqualToString:(__bridge NSString *)kAXTextFieldRole] ||
        [role isEqualToString:(__bridge NSString *)kAXTextAreaRole] ||
        [role isEqualToString:(__bridge NSString *)kAXComboBoxRole] ||
        [role isEqualToString:@"AXSearchField"]) {
        // 某些浏览器/Electron 输入框在 AX 中不把 AXValue 标为 settable，但仍具备角色与选区。
        return MSIMEVoiceTargetEditableStateEditable;
    }

    // 6. 明确的非文本交互/展示控件：不具备可编辑祖先且 AXValue 不可写时，绝不静默丢入 IMK。
    static NSSet<NSString *> *nonEditableRoles = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        nonEditableRoles = [NSSet setWithArray:@[
            @"AXButton",
            @"AXLink",
            @"AXImage",
            @"AXScrollArea",
            @"AXWindow",
            @"AXApplication",
            @"AXToolbar",
            @"AXTabGroup",
            @"AXMenuBar",
            @"AXMenu",
            @"AXMenuItem",
            @"AXCheckBox",
            @"AXRadioButton",
            @"AXPopUpButton",
            @"AXSlider",
            @"AXOutline",
            @"AXTable",
            @"AXList",
            @"AXRow",
            @"AXCell",
            @"AXSplitter",
            @"AXStaticText",
            @"AXHeading",
            @"AXSheet",
            @"AXDialog",
            @"AXDockItem",
            @"AXSplitGroup",
            @"AXLayoutArea",
        ]];
    });
    if ([nonEditableRoles containsObject:role]) {
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    // 7. 网页容器 / 通用分组：没有可编辑祖先、AXValue 不可写且无有效文本选区范围时，通常是点击了页面空白处或只读正文。
    if ([role isEqualToString:@"AXWebArea"] ||
        [role isEqualToString:@"AXGroup"] ||
        [role isEqualToString:@"AXUnknown"] ||
        !role.length) {
        if (input.axHasSelectedTextRange) {
            return MSIMEVoiceTargetEditableStateEditable;
        }
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    return input.axHasSelectedTextRange
        ? MSIMEVoiceTargetEditableStateEditable
        : MSIMEVoiceTargetEditableStateUnknown;
}

/// 探测给定 IMK 客户端当前是否处于真实可输入状态；总 AX 超时硬限制 15 ms，不弹窗申请权限。
/// 对无 `bundleIdentifier` 的合成测试夹具返回 Unknown（放行既有单测路径）。
static inline MSIMEVoiceTargetEditableState MSIMEProbeVoiceTargetEditability(id _Nullable client) {
    if (!client || ![client respondsToSelector:@selector(bundleIdentifier)]) {
        return MSIMEVoiceTargetEditableStateUnknown;
    }
    NSString *clientBundleID = nil;
    @try {
        id raw = [client bundleIdentifier];
        if ([raw isKindOfClass:NSString.class]) clientBundleID = [(NSString *)raw copy];
    } @catch (NSException *exception) {
        (void)exception;
    }
    if (!clientBundleID.length) {
        return MSIMEVoiceTargetEditableStateUnknown;
    }

    NSRunningApplication *frontmost = NSWorkspace.sharedWorkspace.frontmostApplication;
    MSIMEVoiceTargetEditableInput input;
    input.frontmostBundleID = frontmost.bundleIdentifier;
    input.clientBundleID = clientBundleID;

    // 前台应用已经切走时不必再做任何跨进程 AX 查询。
    if (input.frontmostBundleID.length &&
        [input.frontmostBundleID caseInsensitiveCompare:clientBundleID] != NSOrderedSame) {
        return MSIMEVoiceTargetEditableStateNotEditable;
    }

    if (MSIMEIsCustomInputTargetBundleID(input.frontmostBundleID ?: clientBundleID)) {
        return MSIMEVoiceTargetEditableStateEditable;
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
    input.axBlacklisted = MSIMEIsAXBlacklistedBundleIdentifier(input.frontmostBundleID ?: clientBundleID);
    if (!input.axTrusted || input.axBlacklisted || !frontmost || frontmost.processIdentifier <= 0) {
        return MSIMEClassifyVoiceTargetEditability(input);
    }

    AXUIElementRef appElement = AXUIElementCreateApplication(frontmost.processIdentifier);
    if (!appElement) {
        return MSIMEClassifyVoiceTargetEditability(input);
    }
    AXUIElementSetMessagingTimeout(appElement, 0.015f);

    CFTypeRef rawFocused = nullptr;
    const AXError focusErr = AXUIElementCopyAttributeValue(appElement, kAXFocusedUIElementAttribute, &rawFocused);
    CFRelease(appElement);
    if (focusErr != kAXErrorSuccess || !rawFocused || CFGetTypeID(rawFocused) != AXUIElementGetTypeID()) {
        if (rawFocused) CFRelease(rawFocused);
        // 明确返回 NoValue / CannotComplete(无焦点) 时标记为无焦点元素；若超时（kAXErrorCannotComplete 以外的 IPC 失败）保持保守放行。
        if (focusErr == kAXErrorNoValue || focusErr == kAXErrorAttributeUnsupported) {
            input.hasFocusedAXElement = NO;
            return MSIMEClassifyVoiceTargetEditability(input);
        }
        return MSIMEVoiceTargetEditableStateUnknown;
    }

    AXUIElementRef focused = static_cast<AXUIElementRef>(rawFocused);
    AXUIElementSetMessagingTimeout(focused, 0.015f);
    input.hasFocusedAXElement = YES;

    CFTypeRef rawRole = nullptr;
    if (AXUIElementCopyAttributeValue(focused, kAXRoleAttribute, &rawRole) == kAXErrorSuccess && rawRole) {
        id roleObj = CFBridgingRelease(rawRole);
        if ([roleObj isKindOfClass:NSString.class]) input.axRole = (NSString *)roleObj;
    } else if (rawRole) {
        CFRelease(rawRole);
    }

    CFTypeRef rawSubrole = nullptr;
    if (AXUIElementCopyAttributeValue(focused, kAXSubroleAttribute, &rawSubrole) == kAXErrorSuccess && rawSubrole) {
        id subroleObj = CFBridgingRelease(rawSubrole);
        if ([subroleObj isKindOfClass:NSString.class]) input.axSubrole = (NSString *)subroleObj;
    } else if (rawSubrole) {
        CFRelease(rawSubrole);
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
    return MSIMEClassifyVoiceTargetEditability(input);
}
