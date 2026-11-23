#pragma once
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

namespace msime::mac {

/// 终端模拟器的文本缓冲区是整屏网格而非输入框，不读取其现成文本作为组字上文。
inline bool IsTerminalBundleIdentifier(NSString *bundleIdentifier) {
    if (!bundleIdentifier.length) return false;
    NSString *lower = bundleIdentifier.lowercaseString;
    return [lower isEqualToString:@"com.apple.terminal"] ||
           [lower isEqualToString:@"com.googlecode.iterm2"] ||
           [lower isEqualToString:@"com.mitchellh.ghostty"] ||
           [lower isEqualToString:@"net.kovidgoyal.kitty"] ||
           [lower isEqualToString:@"io.alacritty"] ||
           [lower isEqualToString:@"org.alacritty"] ||
           [lower isEqualToString:@"dev.warp.warp-stable"] ||
           [lower isEqualToString:@"co.zeit.hyper"];
}

/// 判断 AX 子角色属性值是否为安全密码输入框（`kAXSecureTextFieldSubrole`）。
inline bool IsSecureTextFieldSubrole(id subrole) {
    return [subrole isKindOfClass:NSString.class] &&
           [(NSString *)subrole isEqualToString:(__bridge NSString *)kAXSecureTextFieldSubrole];
}

/// 检查指定 AX 元素是否为安全密码输入框；空元素或无子角色属性时返回 false。
inline bool IsAXElementSecureTextField(AXUIElementRef element) {
    if (!element) return false;
    CFTypeRef rawSubrole = nullptr;
    if (AXUIElementCopyAttributeValue(element, kAXSubroleAttribute, &rawSubrole) == kAXErrorSuccess && rawSubrole) {
        id subrole = CFBridgingRelease(rawSubrole);
        return IsSecureTextFieldSubrole(subrole);
    }
    if (rawSubrole) CFRelease(rawSubrole);
    return false;
}

inline NSString *TrimSurrogateEdges(NSString *text) {
    if (!text) return nil;
    if (!text.length) return @"";
    NSUInteger start = 0;
    if (CFStringIsSurrogateLowCharacter([text characterAtIndex:0])) start = 1;
    NSUInteger end = text.length;
    if (end > start && CFStringIsSurrogateHighCharacter([text characterAtIndex:end - 1])) --end;
    if (start >= end) return @"";
    if (start == 0 && end == text.length) return text;
    return [text substringWithRange:NSMakeRange(start, end - start)];
}

/// 直接从已验证并配置好超时的焦点 AX 元素读取光标前文本，避免调用方重复跨进程查询应用与焦点元素；
/// 仍执行安全密码框兜底校验。
inline NSString *ReadAXPrecedingContextFromElement(AXUIElementRef focus, NSUInteger maxUTF16Units) {
    if (!focus || !maxUTF16Units || IsAXElementSecureTextField(focus)) return nil;

    CFTypeRef rawRange = nullptr;
    if (AXUIElementCopyAttributeValue(focus, kAXSelectedTextRangeAttribute, &rawRange) != kAXErrorSuccess || !rawRange) {
        if (rawRange) CFRelease(rawRange);
        return nil;
    }
    id rangeObj = CFBridgingRelease(rawRange);
    if (CFGetTypeID((__bridge CFTypeRef)rangeObj) != AXValueGetTypeID()) return nil;
    CFRange selected = CFRangeMake(0, 0);
    if (!AXValueGetValue((__bridge AXValueRef)rangeObj, static_cast<AXValueType>(kAXValueCFRangeType), &selected) ||
        selected.location < 0)
        return nil;
    if (selected.location == 0) return @"";

    const CFIndex limit = static_cast<CFIndex>(maxUTF16Units);
    const CFIndex start = selected.location > limit ? selected.location - limit : 0;
    CFRange slice = CFRangeMake(start, selected.location - start);
    AXValueRef sliceParam = AXValueCreate(static_cast<AXValueType>(kAXValueCFRangeType), &slice);
    if (sliceParam) {
        CFTypeRef rawSlice = nullptr;
        const AXError sliceErr = AXUIElementCopyParameterizedAttributeValue(
            focus, kAXStringForRangeParameterizedAttribute, sliceParam, &rawSlice);
        CFRelease(sliceParam);
        if (sliceErr == kAXErrorSuccess && rawSlice) {
            id sliceObj = CFBridgingRelease(rawSlice);
            if ([sliceObj isKindOfClass:NSString.class])
                return TrimSurrogateEdges((NSString *)sliceObj);
        } else if (rawSlice) {
            CFRelease(rawSlice);
        }
    }

    CFTypeRef rawValue = nullptr;
    if (AXUIElementCopyAttributeValue(focus, kAXValueAttribute, &rawValue) != kAXErrorSuccess || !rawValue) {
        if (rawValue) CFRelease(rawValue);
        return nil;
    }
    id valueObj = CFBridgingRelease(rawValue);
    if (![valueObj isKindOfClass:NSString.class]) return nil;
    NSString *fullText = (NSString *)valueObj;
    const NSUInteger location = static_cast<NSUInteger>(selected.location);
    if (location > fullText.length) return nil;
    const NSUInteger nsStart = location > maxUTF16Units ? location - maxUTF16Units : 0;
    return TrimSurrogateEdges([fullText substringWithRange:NSMakeRange(nsStart, location - nsStart)]);
}

/// 直接从已验证并配置好超时的焦点 AX 元素读取光标后文本（用于双向上下文补全）；安全密码框一律拒绝。
inline NSString *ReadAXFollowingContextFromElement(AXUIElementRef focus, NSUInteger maxUTF16Units) {
    if (!focus || !maxUTF16Units || IsAXElementSecureTextField(focus)) return nil;

    CFTypeRef rawRange = nullptr;
    if (AXUIElementCopyAttributeValue(focus, kAXSelectedTextRangeAttribute, &rawRange) != kAXErrorSuccess || !rawRange) {
        if (rawRange) CFRelease(rawRange);
        return nil;
    }
    id rangeObj = CFBridgingRelease(rawRange);
    if (CFGetTypeID((__bridge CFTypeRef)rangeObj) != AXValueGetTypeID()) return nil;
    CFRange selected = CFRangeMake(0, 0);
    if (!AXValueGetValue((__bridge AXValueRef)rangeObj, static_cast<AXValueType>(kAXValueCFRangeType), &selected) ||
        selected.location < 0 || selected.length < 0)
        return nil;

    const CFIndex afterStart = selected.location + selected.length;
    const CFIndex limit = static_cast<CFIndex>(maxUTF16Units);
    CFRange slice = CFRangeMake(afterStart, limit);
    AXValueRef sliceParam = AXValueCreate(static_cast<AXValueType>(kAXValueCFRangeType), &slice);
    if (sliceParam) {
        CFTypeRef rawSlice = nullptr;
        const AXError sliceErr = AXUIElementCopyParameterizedAttributeValue(
            focus, kAXStringForRangeParameterizedAttribute, sliceParam, &rawSlice);
        CFRelease(sliceParam);
        if (sliceErr == kAXErrorSuccess && rawSlice) {
            id sliceObj = CFBridgingRelease(rawSlice);
            if ([sliceObj isKindOfClass:NSString.class])
                return TrimSurrogateEdges((NSString *)sliceObj);
        } else if (rawSlice) {
            CFRelease(rawSlice);
        }
    }

    CFTypeRef rawValue = nullptr;
    if (AXUIElementCopyAttributeValue(focus, kAXValueAttribute, &rawValue) != kAXErrorSuccess || !rawValue) {
        if (rawValue) CFRelease(rawValue);
        return nil;
    }
    id valueObj = CFBridgingRelease(rawValue);
    if (![valueObj isKindOfClass:NSString.class]) return nil;
    NSString *fullText = (NSString *)valueObj;
    const NSUInteger nsStart = static_cast<NSUInteger>(afterStart);
    if (nsStart >= fullText.length) return @"";
    const NSUInteger nsLen = MIN(maxUTF16Units, fullText.length - nsStart);
    return TrimSurrogateEdges([fullText substringWithRange:NSMakeRange(nsStart, nsLen)]);
}

/// 读取焦点 AX 输入框的完整文本（带硬长度上限，供事后编辑追踪使用）；安全密码框一律拒绝。
inline NSString *ReadAXFullFieldTextFromElement(AXUIElementRef focus, NSUInteger maxUTF16Units) {
    if (!focus || !maxUTF16Units || IsAXElementSecureTextField(focus)) return nil;
    CFTypeRef rawValue = nullptr;
    if (AXUIElementCopyAttributeValue(focus, kAXValueAttribute, &rawValue) != kAXErrorSuccess || !rawValue) {
        if (rawValue) CFRelease(rawValue);
        return nil;
    }
    id valueObj = CFBridgingRelease(rawValue);
    if (![valueObj isKindOfClass:NSString.class]) return nil;
    NSString *fullText = (NSString *)valueObj;
    if (fullText.length <= maxUTF16Units) return TrimSurrogateEdges(fullText);
    return TrimSurrogateEdges([fullText substringToIndex:maxUTF16Units]);
}

/// 当客户端未通过 `IMKTextInput` 返回文档文本（典型如 Electron / Chromium 输入框）时，
/// 在已获辅助功能授权的前提下以 20 毫秒超时读取焦点控件光标前的文本；不弹窗请求权限，安全密码框一律拒绝。
inline NSString *ReadAXPrecedingContext(NSString *clientBundle, NSUInteger maxUTF16Units) {
    if (!clientBundle.length || !maxUTF16Units || IsTerminalBundleIdentifier(clientBundle) || !AXIsProcessTrusted())
        return nil;
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    if (front.processIdentifier <= 0 || ![front.bundleIdentifier isEqualToString:clientBundle])
        return nil;
    AXUIElementRef app = AXUIElementCreateApplication(front.processIdentifier);
    if (!app) return nil;
    AXError timeout = AXUIElementSetMessagingTimeout(app, 0.02f);
    CFTypeRef rawFocus = nullptr;
    AXError error = timeout == kAXErrorSuccess
        ? AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute, &rawFocus) : timeout;
    CFRelease(app);
    if (error != kAXErrorSuccess || !rawFocus) {
        if (rawFocus) CFRelease(rawFocus);
        return nil;
    }
    id focusObj = CFBridgingRelease(rawFocus);
    if (CFGetTypeID((__bridge CFTypeRef)focusObj) != AXUIElementGetTypeID()) return nil;
    AXUIElementRef focus = (__bridge AXUIElementRef)focusObj;
    pid_t focusPID = 0;
    if (AXUIElementGetPid(focus, &focusPID) != kAXErrorSuccess || focusPID != front.processIdentifier ||
        AXUIElementSetMessagingTimeout(focus, 0.02f) != kAXErrorSuccess)
        return nil;

    return ReadAXPrecedingContextFromElement(focus, maxUTF16Units);
}

}
