#import "VoiceContextCapture.h"
#import "../core/AXPrecedingContext.h"
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <Carbon/Carbon.h>
#include <memory>
#include <vector>

/// 声明 IMKTextInput 客户端的可选方法签名，避免 ObjC++ 在 `-Wall -Wextra -Werror` 下对 `id` 产生 selector 歧义。
@protocol MSIMEVoiceContextTextClient <NSObject>
@optional
- (nullable NSString *)bundleIdentifier;
- (NSRange)selectedRange;
- (nullable NSAttributedString *)attributedSubstringFromRange:(NSRange)range;
@end

namespace {

/// 单次同步采集允许消耗的总 AX 时间预算（25 毫秒）与单次 AX 调用超时（10 毫秒）。
constexpr NSTimeInterval kCaptureTotalAXBudgetSec = 0.025;
constexpr float kPerCallAXTimeoutSec = 0.010f;
constexpr int kMaxWebAreaParentDepth = 12;
constexpr int kVisibleTextMaxDepth = 14;
constexpr NSUInteger kVisibleTextMaxNodes = 320;
constexpr NSUInteger kVisibleTextMaxChildrenPerNode = 48;

using ScopedAXElement = std::unique_ptr<const __AXUIElement, decltype(&CFRelease)>;

inline bool IsIgnoredPlaceholderChar(unichar ch) {
    // 剥离 QQ/富文本对象占位符 U+FFFC、BOM 及零宽连接字符。
    return ch == 0xFFFC || ch == 0xFEFF || ch == 0x200B || ch == 0x200C || ch == 0x200D || ch == 0x2060;
}

/// 剥离单行文本中的控制字符并去除首尾空白，避免窗体标题或应用名带入换行与控制码。
NSString *SanitizeSingleLine(NSString * _Nullable text) {
    if (![text isKindOfClass:NSString.class] || !text.length) return @"";
    const NSUInteger len = text.length;
    std::vector<unichar> out;
    out.reserve(len);
    for (NSUInteger i = 0; i < len; ++i) {
        const unichar ch = [text characterAtIndex:i];
        if (IsIgnoredPlaceholderChar(ch)) continue;
        if (ch < 0x20 || ch == 0x7F) {
            if (ch == '\t' || ch == '\n' || ch == '\r') out.push_back(' ');
            continue;
        }
        out.push_back(ch);
    }
    if (out.empty()) return @"";
    NSString *clean = [NSString stringWithCharacters:out.data() length:out.size()];
    NSString *trimmed = [clean stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    return msime::mac::TrimSurrogateEdges(trimmed) ?: @"";
}

/// 清洗多行文本中的非法控制字符与对象占位符，保留换行与制表符以维持代码/Diff 缩进结构。
NSString *StripPlaceholderAndDisallowedControls(NSString * _Nullable text) {
    if (![text isKindOfClass:NSString.class] || !text.length) return @"";
    const NSUInteger len = text.length;
    std::vector<unichar> out;
    out.reserve(len);
    for (NSUInteger i = 0; i < len; ++i) {
        const unichar ch = [text characterAtIndex:i];
        if (IsIgnoredPlaceholderChar(ch)) continue;
        // 允许常规换行与制表符，剔除 NUL 与其余 C0/DEL 控制字符。
        if ((ch < 0x20 && ch != '\n' && ch != '\r' && ch != '\t') || ch == 0x7F) continue;
        out.push_back(ch);
    }
    if (out.empty()) return @"";
    return [NSString stringWithCharacters:out.data() length:out.size()];
}

/// 从尾部截取指定 UTF-16 单元数（用于保留最靠近光标的前文），并保证不截断代理对。
NSString *ClampTailUTF16(NSString * _Nullable text, NSUInteger maxUTF16Units) {
    if (![text isKindOfClass:NSString.class] || !text.length || !maxUTF16Units) return @"";
    NSString *slice = text.length > maxUTF16Units
        ? [text substringFromIndex:text.length - maxUTF16Units]
        : text;
    return msime::mac::TrimSurrogateEdges(slice) ?: @"";
}

/// 读取 AX 元素的字符串属性，类型不匹配或超时返回 nil。
NSString *CopyAXStringAttribute(AXUIElementRef element, CFStringRef attribute) {
    if (!element) return nil;
    CFTypeRef raw = nullptr;
    if (AXUIElementCopyAttributeValue(element, attribute, &raw) != kAXErrorSuccess || !raw) {
        if (raw) CFRelease(raw);
        return nil;
    }
    id obj = CFBridgingRelease(raw);
    if ([obj isKindOfClass:NSString.class]) return (NSString *)obj;
    if ([obj isKindOfClass:NSURL.class]) return [(NSURL *)obj absoluteString];
    return nil;
}

/// 读取 AX 元素的子元素属性，校验 CFTypeID 并为返回元素施加短超时。
id CopyAXElementAttribute(AXUIElementRef element, CFStringRef attribute) {
    if (!element) return nil;
    CFTypeRef raw = nullptr;
    if (AXUIElementCopyAttributeValue(element, attribute, &raw) != kAXErrorSuccess || !raw) {
        if (raw) CFRelease(raw);
        return nil;
    }
    id obj = CFBridgingRelease(raw);
    if (CFGetTypeID((__bridge CFTypeRef)obj) != AXUIElementGetTypeID()) return nil;
    AXUIElementRef child = (__bridge AXUIElementRef)obj;
    if (AXUIElementSetMessagingTimeout(child, kPerCallAXTimeoutSec) != kAXErrorSuccess) return nil;
    return obj;
}

/// 沿浏览器的焦点元素向上有限层回溯，查找 `AXWebArea` 的 `AXURL` 或焦点窗口的 `AXDocument`。
NSString *BestEffortBrowserURL(id focusObj, id focusedWindowObj, NSTimeInterval deadline) {
    id currentObj = focusObj;
    for (int depth = 0; depth < kMaxWebAreaParentDepth && currentObj; ++depth) {
        if (NSProcessInfo.processInfo.systemUptime >= deadline) break;
        AXUIElementRef node = (__bridge AXUIElementRef)currentObj;
        NSString *role = CopyAXStringAttribute(node, kAXRoleAttribute);
        if ([role isEqualToString:@"AXWebArea"]) {
            NSString *url = CopyAXStringAttribute(node, CFSTR("AXURL"));
            if (url.length) return url;
            break;
        }
        currentObj = CopyAXElementAttribute(node, kAXParentAttribute);
    }
    if (focusedWindowObj && NSProcessInfo.processInfo.systemUptime < deadline) {
        NSString *doc = CopyAXStringAttribute((__bridge AXUIElementRef)focusedWindowObj, kAXDocumentAttribute);
        if (doc.length) return doc;
    }
    return @"";
}

/// 通过 IMKTextInput 客户端兜底读取光标前文（原生 AppKit 编辑器在不依赖 AX 时亦可用）。
NSString *FallbackIMKPrecedingText(id<MSIMEVoiceContextTextClient> client, NSUInteger maxUTF16Units) {
    if (!client || !maxUTF16Units) return nil;
    if (![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)]) {
        return nil;
    }
    @try {
        const NSRange selection = [client selectedRange];
        if (selection.location == NSNotFound || selection.location == 0) return @"";
        const NSUInteger start = selection.location > maxUTF16Units ? selection.location - maxUTF16Units : 0;
        const NSRange slice = NSMakeRange(start, selection.location - start);
        NSAttributedString *attributed = [client attributedSubstringFromRange:slice];
        return attributed.string;
    } @catch (NSException *) {
        return nil;
    }
}

/// 通过 IMKTextInput 客户端兜底读取当前选区文本（兼容不暴露 AXSelectedText 的原生 AppKit 控件）。
NSString *FallbackIMKSelectedText(id<MSIMEVoiceContextTextClient> client, NSUInteger maxUTF16Units) {
    if (!client || !maxUTF16Units) return nil;
    if (![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)]) {
        return nil;
    }
    @try {
        const NSRange selection = [client selectedRange];
        if (selection.location == NSNotFound || selection.length == 0) return @"";
        // 多读少量余量供后续清洗零宽字符后再统一裁剪。
        const NSUInteger boundedLength = MIN(selection.length, maxUTF16Units + 64);
        NSAttributedString *attributed =
            [client attributedSubstringFromRange:NSMakeRange(selection.location, boundedLength)];
        return attributed.string;
    } @catch (NSException *) {
        return nil;
    }
}

/// 为已知浏览器与 Electron/Chromium 桌面应用安全启用 `AXEnhancedUserInterface`，使无障碍树暴露完整网页/视图文本。
void EnableEnhancedUIForAppIfSupported(AXUIElementRef app, NSString * _Nullable bundleID) {
    if (!app || !bundleID.length || MSIMEIsAXBlacklistedBundleIdentifier(bundleID)) return;
    NSString *lower = bundleID.lowercaseString;
    const BOOL supported = MSIMEIsBrowserBundleIdentifier(lower) ||
                           [lower containsString:@"electron"] ||
                           [lower isEqualToString:@"com.microsoft.vscode"] ||
                           [lower isEqualToString:@"com.microsoft.vscodeinsiders"] ||
                           [lower isEqualToString:@"com.todesktop.230313mzl4w4u92"] ||
                           [lower isEqualToString:@"com.exafunction.windsurf"] ||
                           [lower isEqualToString:@"com.tinyspeck.slackmacgap"] ||
                           [lower isEqualToString:@"com.hnc.discord"] ||
                           [lower isEqualToString:@"com.anthropic.claudefordesktop"] ||
                           [lower isEqualToString:@"com.openai.chat"] ||
                           [lower isEqualToString:@"com.openai.codex"] ||
                           [lower isEqualToString:@"notion.id"] ||
                           [lower isEqualToString:@"md.obsidian"] ||
                           [lower isEqualToString:@"com.linear"] ||
                           [lower isEqualToString:@"com.bytedance.feishu"] ||
                           [lower isEqualToString:@"com.larksuite.larkapp"] ||
                           [lower isEqualToString:@"com.kangfenmao.cherrystudio"];
    if (!supported) return;
    (void)AXUIElementSetAttributeValue(app, CFSTR("AXEnhancedUserInterface"), kCFBooleanTrue);
}

/// 通过 IMKTextInput 客户端兜底读取光标后文（原生 AppKit 编辑器在不依赖 AX 时亦可用）。
NSString *FallbackIMKFollowingText(id<MSIMEVoiceContextTextClient> client, NSUInteger maxUTF16Units) {
    if (!client || !maxUTF16Units) return nil;
    if (![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)]) {
        return nil;
    }
    @try {
        const NSRange selection = [client selectedRange];
        if (selection.location == NSNotFound) return @"";
        const NSUInteger afterStart = selection.location + selection.length;
        NSAttributedString *attributed =
            [client attributedSubstringFromRange:NSMakeRange(afterStart, maxUTF16Units)];
        return attributed.string;
    } @catch (NSException *) {
        return nil;
    }
}

/// 清洗并规整屏幕可见文本（剥离占位符/非法控制符、折叠冗余空行并按头截断）。
NSString *SanitizeVisibleScreenText(NSString * _Nullable rawText, NSUInteger maxUTF16Units) {
    if (![rawText isKindOfClass:NSString.class] || !rawText.length || !maxUTF16Units) return @"";
    NSString *stripped = StripPlaceholderAndDisallowedControls(rawText);
    if (!stripped.length) return @"";
    NSString *normalized = [[stripped stringByReplacingOccurrencesOfString:@"\r\n" withString:@"\n"]
        stringByReplacingOccurrencesOfString:@"\r" withString:@"\n"];
    NSArray<NSString *> *lines = [normalized componentsSeparatedByString:@"\n"];
    NSMutableArray<NSString *> *out = [NSMutableArray array];
    NSString *lastLine = nil;
    NSUInteger totalLen = 0;
    for (NSString *line in lines) {
        NSString *trimmed = [line stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        if (!trimmed.length) continue;
        if (lastLine && [lastLine isEqualToString:trimmed]) continue;
        [out addObject:trimmed];
        lastLine = trimmed;
        totalLen += trimmed.length + 1;
        if (totalLen >= maxUTF16Units + 64) break;
    }
    if (!out.count) return @"";
    return MSIMEClampUTF16String([out componentsJoinedByString:@"\n"], maxUTF16Units);
}

/// 跳过对文本上下文无意义的装饰/导航角色，同时强制跳过任何密码框子树。
BOOL ShouldSkipVisibleAXRole(NSString * _Nullable role) {
    if (!role.length) return NO;
    return [role isEqualToString:@"AXMenuBar"] ||
           [role isEqualToString:@"AXMenu"] ||
           [role isEqualToString:@"AXScrollBar"] ||
           [role isEqualToString:@"AXToolbar"] ||
           [role isEqualToString:@"AXImage"] ||
           [role isEqualToString:@"AXSplitter"] ||
           [role isEqualToString:@"AXBusyIndicator"] ||
           [role isEqualToString:@"AXProgressIndicator"];
}

/// 广度优先扫描焦点窗口/网页的可见文本节点（严格时间预算与节点数上限，绝不触碰密码框）。
void CollectVisibleTextsFromAXRoot(
    id rootObj,
    NSUInteger maxUTF16Units,
    NSTimeInterval deadline,
    NSMutableArray<NSString *> *collected) {
    if (!rootObj || !maxUTF16Units) return;
    struct QueueItem {
        __strong id element;
        int depth;
    };
    std::vector<QueueItem> queue;
    queue.reserve(128);
    queue.push_back({rootObj, 0});

    NSUInteger head = 0;
    NSUInteger visited = 0;
    NSUInteger charsCount = 0;
    NSString *lastPiece = nil;

    while (head < queue.size() && visited < kVisibleTextMaxNodes && charsCount < maxUTF16Units) {
        if (NSProcessInfo.processInfo.systemUptime >= deadline) break;
        id currentObj = queue[head].element;
        const int depth = queue[head].depth;
        queue[head].element = nil;
        ++head;
        ++visited;

        AXUIElementRef node = (__bridge AXUIElementRef)currentObj;
        if (msime::mac::IsAXElementSecureTextField(node)) continue;

        NSString *role = CopyAXStringAttribute(node, kAXRoleAttribute);
        if (ShouldSkipVisibleAXRole(role)) continue;

        // 对叶子文本节点读取正文/标题，避免对整窗容器读取巨大 AXValue 引发卡顿。
        if ([role isEqualToString:@"AXStaticText"] ||
            [role isEqualToString:@"AXTextArea"] ||
            [role isEqualToString:@"AXTextField"] ||
            [role isEqualToString:@"AXHeading"] ||
            [role isEqualToString:@"AXLink"] ||
            [role isEqualToString:@"AXCell"]) {
            NSString *piece = CopyAXStringAttribute(node, kAXValueAttribute);
            if (!piece.length) piece = CopyAXStringAttribute(node, kAXTitleAttribute);
            if (!piece.length && [role isEqualToString:@"AXHeading"]) {
                piece = CopyAXStringAttribute(node, kAXDescriptionAttribute);
            }
            NSString *clean = SanitizeSingleLine(piece);
            if (clean.length >= 2 && (!lastPiece || ![lastPiece isEqualToString:clean])) {
                NSString *bounded = MSIMEClampUTF16String(clean, MIN(clean.length, (NSUInteger)300));
                [collected addObject:bounded];
                lastPiece = bounded;
                charsCount += bounded.length + 1;
                if (charsCount >= maxUTF16Units) break;
            }
            if ([role isEqualToString:@"AXStaticText"] || [role isEqualToString:@"AXTextField"]) {
                continue;
            }
        }

        if (depth >= kVisibleTextMaxDepth) continue;
        CFTypeRef rawChildren = nullptr;
        // 优先使用 AXVisibleChildren 仅抓屏幕可见区域，回退到 AXChildren。
        AXError err = AXUIElementCopyAttributeValue(node, CFSTR("AXVisibleChildren"), &rawChildren);
        if (err != kAXErrorSuccess || !rawChildren) {
            if (rawChildren) { CFRelease(rawChildren); rawChildren = nullptr; }
            err = AXUIElementCopyAttributeValue(node, kAXChildrenAttribute, &rawChildren);
        }
        if (err != kAXErrorSuccess || !rawChildren) {
            if (rawChildren) CFRelease(rawChildren);
            continue;
        }
        id childrenObj = CFBridgingRelease(rawChildren);
        if (![childrenObj isKindOfClass:NSArray.class]) continue;
        NSArray *children = (NSArray *)childrenObj;
        const NSUInteger limit = MIN(children.count, kVisibleTextMaxChildrenPerNode);
        for (NSUInteger i = 0; i < limit; ++i) {
            if (queue.size() >= kVisibleTextMaxNodes) break;
            id child = children[i];
            if (!child || CFGetTypeID((__bridge CFTypeRef)child) != AXUIElementGetTypeID()) continue;
            AXUIElementRef childRef = (__bridge AXUIElementRef)child;
            if (AXUIElementSetMessagingTimeout(childRef, kPerCallAXTimeoutSec) != kAXErrorSuccess) continue;
            queue.push_back({child, depth + 1});
        }
    }
}

} // 匿名命名空间

@implementation MSIMEVoiceContextSnapshot

- (instancetype)init {
    return [self initWithAppBundleID:@""
                             appName:@""
                         windowTitle:@""
                           webDomain:@""
                       precedingText:@""
                       followingText:@""
                        selectedText:@""
                   visibleScreenText:@""
                       isSecureField:NO
                       axBlacklisted:NO];
}

- (instancetype)initWithAppBundleID:(nullable NSString *)appBundleID
                            appName:(nullable NSString *)appName
                        windowTitle:(nullable NSString *)windowTitle
                          webDomain:(nullable NSString *)webDomain
                      precedingText:(nullable NSString *)precedingText
                       selectedText:(nullable NSString *)selectedText
                      isSecureField:(BOOL)isSecureField
                      axBlacklisted:(BOOL)axBlacklisted {
    return [self initWithAppBundleID:appBundleID
                             appName:appName
                         windowTitle:windowTitle
                           webDomain:webDomain
                       precedingText:precedingText
                       followingText:@""
                        selectedText:selectedText
                   visibleScreenText:@""
                       isSecureField:isSecureField
                       axBlacklisted:axBlacklisted];
}

- (instancetype)initWithAppBundleID:(nullable NSString *)appBundleID
                            appName:(nullable NSString *)appName
                        windowTitle:(nullable NSString *)windowTitle
                          webDomain:(nullable NSString *)webDomain
                      precedingText:(nullable NSString *)precedingText
                      followingText:(nullable NSString *)followingText
                       selectedText:(nullable NSString *)selectedText
                  visibleScreenText:(nullable NSString *)visibleScreenText
                      isSecureField:(BOOL)isSecureField
                      axBlacklisted:(BOOL)axBlacklisted {
    self = [super init];
    if (!self) return nil;
    if (isSecureField) {
        _appBundleID = @"";
        _appName = @"";
        _windowTitle = @"";
        _webDomain = @"";
        _precedingText = @"";
        _followingText = @"";
        _selectedText = @"";
        _visibleScreenText = @"";
        _isSecureField = YES;
        _axBlacklisted = NO;
    } else {
        _appBundleID = [appBundleID isKindOfClass:NSString.class] ? [appBundleID copy] : @"";
        _appName = [appName isKindOfClass:NSString.class] ? [appName copy] : @"";
        _windowTitle = [windowTitle isKindOfClass:NSString.class] ? [windowTitle copy] : @"";
        _webDomain = [webDomain isKindOfClass:NSString.class] ? [webDomain copy] : @"";
        _precedingText = [precedingText isKindOfClass:NSString.class] ? [precedingText copy] : @"";
        _followingText = [followingText isKindOfClass:NSString.class] ? [followingText copy] : @"";
        _selectedText = [selectedText isKindOfClass:NSString.class] ? [selectedText copy] : @"";
        _visibleScreenText = [visibleScreenText isKindOfClass:NSString.class] ? [visibleScreenText copy] : @"";
        _isSecureField = NO;
        _axBlacklisted = axBlacklisted;
    }
    return self;
}

- (instancetype)snapshotWithVisibleScreenText:(nullable NSString *)visibleScreenText {
    if (self.isSecureField || self.axBlacklisted) return [self copy];
    NSString *cleanVisible = SanitizeVisibleScreenText(visibleScreenText, MSIMEVoiceContextMaxVisibleScreenTextUTF16);
    return [[MSIMEVoiceContextSnapshot alloc] initWithAppBundleID:self.appBundleID
                                                          appName:self.appName
                                                      windowTitle:self.windowTitle
                                                        webDomain:self.webDomain
                                                    precedingText:self.precedingText
                                                    followingText:self.followingText
                                                     selectedText:self.selectedText
                                                visibleScreenText:cleanVisible
                                                    isSecureField:NO
                                                    axBlacklisted:NO];
}

+ (instancetype)emptySnapshotWithSecureField:(BOOL)isSecureField {
    return [[self alloc] initWithAppBundleID:@""
                                     appName:@""
                                 windowTitle:@""
                                   webDomain:@""
                               precedingText:@""
                               followingText:@""
                                selectedText:@""
                           visibleScreenText:@""
                               isSecureField:isSecureField
                               axBlacklisted:NO];
}

- (id)copyWithZone:(nullable NSZone *)zone {
    return [[MSIMEVoiceContextSnapshot allocWithZone:zone] initWithAppBundleID:self.appBundleID
                                                                       appName:self.appName
                                                                   windowTitle:self.windowTitle
                                                                     webDomain:self.webDomain
                                                                 precedingText:self.precedingText
                                                                 followingText:self.followingText
                                                                  selectedText:self.selectedText
                                                             visibleScreenText:self.visibleScreenText
                                                                 isSecureField:self.isSecureField
                                                                 axBlacklisted:self.axBlacklisted];
}

- (NSDictionary<NSString *, id> *)toJSONDictionary {
    return @{
        @"app_bundle_id" : self.appBundleID ?: @"",
        @"app_name" : self.appName ?: @"",
        @"window_title" : self.windowTitle ?: @"",
        @"web_domain" : self.webDomain ?: @"",
        @"preceding_text" : self.precedingText ?: @"",
        @"following_text" : self.followingText ?: @"",
        @"selected_text" : self.selectedText ?: @"",
        @"visible_screen_text" : self.visibleScreenText ?: @"",
        @"is_secure_field" : @(self.isSecureField),
        @"ax_blacklisted" : @(self.axBlacklisted),
    };
}

@end

BOOL MSIMEIsBrowserBundleIdentifier(NSString * _Nullable bundleID) {
    if (![bundleID isKindOfClass:NSString.class] || !bundleID.length) return NO;
    NSString *lower = bundleID.lowercaseString;
    static NSSet<NSString *> *browsers = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        browsers = [NSSet setWithArray:@[
            @"company.thebrowser.browser",
            @"company.thebrowser.dia",
            @"com.google.chrome",
            @"com.google.chrome.beta",
            @"com.google.chrome.canary",
            @"com.apple.safari",
            @"com.apple.safaritechnologypreview",
            @"org.mozilla.firefox",
            @"org.mozilla.firefoxdeveloperedition",
            @"com.microsoft.edgemac",
            @"com.microsoft.edgemac.beta",
            @"com.brave.browser",
            @"com.vivaldi.vivaldi",
            @"com.operasoftware.opera",
            @"com.kagi.kagimacos",
            @"com.openai.atlas",
        ]];
    });
    return [browsers containsObject:lower];
}

BOOL MSIMEIsAXBlacklistedBundleIdentifier(NSString * _Nullable bundleID) {
    if (![bundleID isKindOfClass:NSString.class] || !bundleID.length) return NO;
    NSString *lower = bundleID.lowercaseString;
    // 内建默认 AX 防卡死黑名单：微信与 Sublime Text 4 深层遍历 AX 树易触发宿主冻结。
    return [lower isEqualToString:@"com.tencent.xinwechat"] ||
           [lower isEqualToString:@"com.sublimetext.4"];
}

BOOL MSIMEIsAXBlacklistedURLString(NSString * _Nullable urlString) {
    if (![urlString isKindOfClass:NSString.class] || !urlString.length) return NO;
    NSString *trimmed = [urlString stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (!trimmed.length) return NO;
    NSURLComponents *components = [NSURLComponents componentsWithString:trimmed];
    if (!components) return NO;
    NSString *scheme = components.scheme.lowercaseString;
    if (![scheme isEqualToString:@"http"] && ![scheme isEqualToString:@"https"]) return NO;
    NSString *host = components.host.lowercaseString;
    while ([host hasSuffix:@"."]) host = [host substringToIndex:host.length - 1];
    NSString *path = components.path.lowercaseString;
    if ([host isEqualToString:@"docs.google.com"]) {
        if ([path isEqualToString:@"/document/d"] || [path hasPrefix:@"/document/d/"]) return YES;
        // 兼容 Google 多账号路径 `/document/u/<index>/d/...`。
        if ([path hasPrefix:@"/document/u/"] && [path containsString:@"/d/"]) return YES;
        return NO;
    }
    if ([host isEqualToString:@"docs.qq.com"]) {
        if ([path isEqualToString:@"/doc"] || [path hasPrefix:@"/doc/"] ||
            [path isEqualToString:@"/sheet"] || [path hasPrefix:@"/sheet/"]) {
            return YES;
        }
    }
    return NO;
}

BOOL MSIMEIsAXBlacklistedContext(NSString * _Nullable bundleID, NSString * _Nullable rawURLString) {
    return MSIMEIsAXBlacklistedBundleIdentifier(bundleID) || MSIMEIsAXBlacklistedURLString(rawURLString);
}

NSString *MSIMEExtractWebDomainFromURLString(NSString * _Nullable urlString) {
    if (![urlString isKindOfClass:NSString.class] || !urlString.length) return @"";
    NSString *trimmed = [urlString stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (!trimmed.length) return @"";
    NSURLComponents *components = [NSURLComponents componentsWithString:trimmed];
    if (!components) return @"";
    NSString *scheme = components.scheme.lowercaseString;
    if (![scheme isEqualToString:@"http"] && ![scheme isEqualToString:@"https"]) return @"";
    NSString *host = components.host;
    if (!host.length) return @"";
    NSString *cleanHost = SanitizeSingleLine(host).lowercaseString;
    while ([cleanHost hasSuffix:@"."]) {
        cleanHost = [cleanHost substringToIndex:cleanHost.length - 1];
    }
    if (!cleanHost.length) return @"";
    // 域名不允许含空格或斜杠；按 DNS 长度上限校验。
    if ([cleanHost rangeOfCharacterFromSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].location != NSNotFound ||
        [cleanHost containsString:@"/"] ||
        [cleanHost lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > MSIMEVoiceContextMaxWebDomainBytes) {
        return @"";
    }
    return cleanHost;
}

NSString *MSIMEClampUTF16String(NSString * _Nullable text, NSUInteger maxUTF16Units) {
    if (![text isKindOfClass:NSString.class] || !text.length || !maxUTF16Units) return @"";
    NSString *slice = text.length > maxUTF16Units ? [text substringToIndex:maxUTF16Units] : text;
    return msime::mac::TrimSurrogateEdges(slice) ?: @"";
}

NSString *MSIMEClampUTF8BytesString(NSString * _Nullable text, NSUInteger maxUTF8Bytes) {
    if (![text isKindOfClass:NSString.class] || !text.length || !maxUTF8Bytes) return @"";
    if ([text lengthOfBytesUsingEncoding:NSUTF8StringEncoding] <= maxUTF8Bytes) return text;
    NSUInteger utf8Bytes = 0;
    NSUInteger endIndex = 0;
    while (endIndex < text.length) {
        const NSRange cluster = [text rangeOfComposedCharacterSequenceAtIndex:endIndex];
        NSString *piece = [text substringWithRange:cluster];
        const NSUInteger pieceBytes = [piece lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
        if (utf8Bytes + pieceBytes > maxUTF8Bytes) break;
        utf8Bytes += pieceBytes;
        endIndex = NSMaxRange(cluster);
    }
    if (endIndex == 0) return @"";
    return [text substringToIndex:endIndex];
}

NSString *MSIMESanitizeSelectedText(NSString * _Nullable rawText, NSUInteger maxUTF16Units) {
    if (![rawText isKindOfClass:NSString.class] || !rawText.length || !maxUTF16Units) return @"";
    NSString *stripped = StripPlaceholderAndDisallowedControls(rawText);
    NSCharacterSet *wsAndNl = NSCharacterSet.whitespaceAndNewlineCharacterSet;
    NSString *trimmed = [stripped stringByTrimmingCharactersInSet:wsAndNl];
    if (!trimmed.length) return @"";

    // 多行选区（如代码片段、Git diff）仅剔除首尾纯空行与行尾空白，完整保留首行及各行前导缩进。
    if ([stripped rangeOfCharacterFromSet:NSCharacterSet.newlineCharacterSet].location != NSNotFound) {
        NSString *normalized = [stripped stringByReplacingOccurrencesOfString:@"\r\n" withString:@"\n"];
        normalized = [normalized stringByReplacingOccurrencesOfString:@"\r" withString:@"\n"];
        NSArray<NSString *> *lines = [normalized componentsSeparatedByString:@"\n"];
        NSInteger first = -1;
        NSInteger last = -1;
        for (NSInteger i = 0; i < static_cast<NSInteger>(lines.count); ++i) {
            if ([lines[i] stringByTrimmingCharactersInSet:wsAndNl].length > 0) {
                if (first < 0) first = i;
                last = i;
            }
        }
        if (first < 0 || last < first) return @"";
        // 若剥离首尾空行后仅剩单行普通选区（如带尾换行的短语），同步裁剪首尾水平空白。
        if (first == last) {
            return MSIMEClampUTF16String(trimmed, maxUTF16Units);
        }
        NSCharacterSet *horizontalWS = NSCharacterSet.whitespaceCharacterSet;
        NSMutableArray<NSString *> *kept = [NSMutableArray arrayWithCapacity:static_cast<NSUInteger>(last - first + 1)];
        for (NSInteger i = first; i <= last; ++i) {
            NSString *line = lines[i];
            NSUInteger end = line.length;
            while (end > 0 && [horizontalWS characterIsMember:[line characterAtIndex:end - 1]]) {
                --end;
            }
            [kept addObject:[line substringToIndex:end]];
        }
        return MSIMEClampUTF16String([kept componentsJoinedByString:@"\n"], maxUTF16Units);
    }
    return MSIMEClampUTF16String(trimmed, maxUTF16Units);
}

MSIMEVoiceContextSnapshot *MSIMEBuildVoiceContextSnapshotExtended(
    BOOL isSecureEventInput,
    BOOL isSecureAXField,
    NSString * _Nullable appBundleID,
    NSString * _Nullable appName,
    NSString * _Nullable windowTitle,
    NSString * _Nullable rawURLString,
    NSString * _Nullable precedingText,
    NSString * _Nullable followingText,
    NSString * _Nullable selectedText,
    NSString * _Nullable visibleScreenText) {
    // 最高优先级隐私闸门：只要处于安全输入态或密码框，立即返回全空快照。
    if (isSecureEventInput || isSecureAXField) {
        return [MSIMEVoiceContextSnapshot emptySnapshotWithSecureField:YES];
    }

    NSString *safeBundleID = MSIMEClampUTF8BytesString(SanitizeSingleLine(appBundleID), MSIMEVoiceContextMaxBundleIDBytes);
    NSString *safeAppName = MSIMEClampUTF8BytesString(SanitizeSingleLine(appName), MSIMEVoiceContextMaxAppNameBytes);
    NSString *safeWindowTitle = MSIMEClampUTF16String(SanitizeSingleLine(windowTitle), MSIMEVoiceContextMaxWindowTitleUTF16);

    const BOOL isBrowser = MSIMEIsBrowserBundleIdentifier(safeBundleID);
    NSString *effectiveURL = isBrowser ? rawURLString : nil;
    NSString *safeWebDomain = isBrowser ? MSIMEExtractWebDomainFromURLString(effectiveURL) : @"";
    const BOOL axBlacklisted = MSIMEIsAXBlacklistedContext(safeBundleID, effectiveURL);

    // 命中 AX 黑名单时，保留应用标识、域名与轻量标题供场景分类，强制跳过前文、后文、选区与可见屏。
    if (axBlacklisted) {
        return [[MSIMEVoiceContextSnapshot alloc] initWithAppBundleID:safeBundleID
                                                              appName:safeAppName
                                                          windowTitle:safeWindowTitle
                                                            webDomain:safeWebDomain
                                                        precedingText:@""
                                                        followingText:@""
                                                         selectedText:@""
                                                    visibleScreenText:@""
                                                        isSecureField:NO
                                                        axBlacklisted:YES];
    }

    const BOOL isTerminal = msime::mac::IsTerminalBundleIdentifier(safeBundleID);
    NSString *safePreceding = @"";
    if (!isTerminal && [precedingText isKindOfClass:NSString.class] && precedingText.length) {
        NSString *cleaned = StripPlaceholderAndDisallowedControls(precedingText);
        safePreceding = ClampTailUTF16(cleaned, MSIMEVoiceContextMaxPrecedingTextUTF16);
    }

    NSString *safeFollowing = @"";
    if (!isTerminal && [followingText isKindOfClass:NSString.class] && followingText.length) {
        NSString *cleaned = StripPlaceholderAndDisallowedControls(followingText);
        safeFollowing = MSIMEClampUTF16String(cleaned, MSIMEVoiceContextMaxFollowingTextUTF16);
    }

    NSString *safeSelected = MSIMESanitizeSelectedText(selectedText, MSIMEVoiceContextMaxSelectedTextUTF16);
    NSString *safeVisible = SanitizeVisibleScreenText(visibleScreenText, MSIMEVoiceContextMaxVisibleScreenTextUTF16);

    return [[MSIMEVoiceContextSnapshot alloc] initWithAppBundleID:safeBundleID
                                                          appName:safeAppName
                                                      windowTitle:safeWindowTitle
                                                        webDomain:safeWebDomain
                                                    precedingText:safePreceding
                                                    followingText:safeFollowing
                                                     selectedText:safeSelected
                                                visibleScreenText:safeVisible
                                                    isSecureField:NO
                                                    axBlacklisted:NO];
}

MSIMEVoiceContextSnapshot *MSIMEBuildVoiceContextSnapshot(
    BOOL isSecureEventInput,
    BOOL isSecureAXField,
    NSString * _Nullable appBundleID,
    NSString * _Nullable appName,
    NSString * _Nullable windowTitle,
    NSString * _Nullable rawURLString,
    NSString * _Nullable precedingText,
    NSString * _Nullable selectedText) {
    return MSIMEBuildVoiceContextSnapshotExtended(
        isSecureEventInput,
        isSecureAXField,
        appBundleID,
        appName,
        windowTitle,
        rawURLString,
        precedingText,
        @"",
        selectedText,
        @"");
}

MSIMEVoiceContextSnapshot *MSIMECaptureVoiceContext(id _Nullable client) {
    // 第一道隐私闸门：系统处于安全事件输入态时不做任何 AX/客户端读取。
    if (IsSecureEventInputEnabled()) {
        return [MSIMEVoiceContextSnapshot emptySnapshotWithSecureField:YES];
    }

    id<MSIMEVoiceContextTextClient> textClient = client;
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    NSString *clientBundle = nil;
    if (textClient && [textClient respondsToSelector:@selector(bundleIdentifier)]) {
        @try {
            id rawBundle = [textClient bundleIdentifier];
            if ([rawBundle isKindOfClass:NSString.class]) clientBundle = (NSString *)rawBundle;
        } @catch (NSException *) {
            clientBundle = nil;
        }
    }
    NSString *bundleID = clientBundle.length ? clientBundle : front.bundleIdentifier;
    NSString *appName = @"";
    if (front && (!bundleID.length || !front.bundleIdentifier.length ||
                  [front.bundleIdentifier caseInsensitiveCompare:bundleID] == NSOrderedSame)) {
        appName = front.localizedName ?: @"";
    }

    // Bundle 级 AX 黑名单（微信、Sublime Text 4）：零 AX 调用直接返回，彻底规避卡死风险。
    if (MSIMEIsAXBlacklistedBundleIdentifier(bundleID)) {
        return MSIMEBuildVoiceContextSnapshotExtended(NO, NO, bundleID, appName, @"", @"", @"", @"", @"", @"");
    }

    const NSTimeInterval deadline = NSProcessInfo.processInfo.systemUptime + kCaptureTotalAXBudgetSec;
    const pid_t selfPID = NSProcessInfo.processInfo.processIdentifier;
    const BOOL canQueryAX = AXIsProcessTrusted() &&
                            front != nil &&
                            front.processIdentifier > 0 &&
                            front.processIdentifier != selfPID &&
                            bundleID.length > 0 &&
                            front.bundleIdentifier.length > 0 &&
                            [front.bundleIdentifier caseInsensitiveCompare:bundleID] == NSOrderedSame;

    NSString *windowTitle = @"";
    NSString *rawURL = @"";
    NSString *precedingText = @"";
    NSString *followingText = @"";
    NSString *selectedText = @"";
    const BOOL isBrowser = MSIMEIsBrowserBundleIdentifier(bundleID);
    const BOOL isTerminal = msime::mac::IsTerminalBundleIdentifier(bundleID);

    if (canQueryAX) {
        ScopedAXElement app(AXUIElementCreateApplication(front.processIdentifier), CFRelease);
        if (app && AXUIElementSetMessagingTimeout(app.get(), kPerCallAXTimeoutSec) == kAXErrorSuccess) {
            EnableEnhancedUIForAppIfSupported(app.get(), bundleID);
            id focusObj = CopyAXElementAttribute(app.get(), kAXFocusedUIElementAttribute);
            AXUIElementRef focus = focusObj ? (__bridge AXUIElementRef)focusObj : nullptr;
            if (focus) {
                pid_t focusPID = 0;
                if (AXUIElementGetPid(focus, &focusPID) != kAXErrorSuccess || focusPID != front.processIdentifier) {
                    focus = nullptr;
                    focusObj = nil;
                }
            }

            // 第二道隐私闸门：命中 AX 密码框立即终止采集并返回全空安全快照。
            if (focus && msime::mac::IsAXElementSecureTextField(focus)) {
                return [MSIMEVoiceContextSnapshot emptySnapshotWithSecureField:YES];
            }

            id focusedWindowObj = nil;
            if (NSProcessInfo.processInfo.systemUptime < deadline) {
                focusedWindowObj = CopyAXElementAttribute(app.get(), kAXFocusedWindowAttribute);
                if (focusedWindowObj) {
                    windowTitle = CopyAXStringAttribute((__bridge AXUIElementRef)focusedWindowObj, kAXTitleAttribute) ?: @"";
                }
            }

            if (isBrowser && NSProcessInfo.processInfo.systemUptime < deadline) {
                rawURL = BestEffortBrowserURL(focusObj, focusedWindowObj, deadline) ?: @"";
                // 页面级 AX 黑名单（Google Docs / 腾讯文档）：保留域名与标题，跳过所有深层正文与选区查询。
                if (MSIMEIsAXBlacklistedURLString(rawURL)) {
                    return MSIMEBuildVoiceContextSnapshotExtended(NO, NO, bundleID, appName, windowTitle, rawURL, @"", @"", @"", @"");
                }
            }

            if (focus && NSProcessInfo.processInfo.systemUptime < deadline) {
                selectedText = CopyAXStringAttribute(focus, kAXSelectedTextAttribute) ?: @"";
            }

            // 直接复用已验证且已设 10ms 超时的 focus 读取光标前后文，避免重复跨进程 AX IPC 突破 25ms 预算。
            if (!isTerminal && focus && NSProcessInfo.processInfo.systemUptime < deadline) {
                NSString *axPreceding = msime::mac::ReadAXPrecedingContextFromElement(
                    focus, MSIMEVoiceContextMaxPrecedingTextUTF16);
                if (axPreceding.length) precedingText = axPreceding;
            }
            if (!isTerminal && focus && NSProcessInfo.processInfo.systemUptime < deadline) {
                NSString *axFollowing = msime::mac::ReadAXFollowingContextFromElement(
                    focus, MSIMEVoiceContextMaxFollowingTextUTF16);
                if (axFollowing.length) followingText = axFollowing;
            }
        }
    }

    // 针对不暴露 AX 文本或未授予 AX 的原生 AppKit 编辑器，使用 IMKTextInput 做有界兜底。
    if (!isTerminal && !precedingText.length && textClient) {
        NSString *imkPreceding = FallbackIMKPrecedingText(textClient, MSIMEVoiceContextMaxPrecedingTextUTF16);
        if (imkPreceding.length) precedingText = imkPreceding;
    }
    if (!isTerminal && !followingText.length && textClient) {
        NSString *imkFollowing = FallbackIMKFollowingText(textClient, MSIMEVoiceContextMaxFollowingTextUTF16);
        if (imkFollowing.length) followingText = imkFollowing;
    }
    if (!selectedText.length && textClient) {
        NSString *imkSelected = FallbackIMKSelectedText(textClient, MSIMEVoiceContextMaxSelectedTextUTF16);
        if (imkSelected.length) selectedText = imkSelected;
    }

    return MSIMEBuildVoiceContextSnapshotExtended(
        NO, NO, bundleID, appName, windowTitle, rawURL, precedingText, followingText, selectedText, @"");
}

NSString *MSIMECaptureVisibleScreenTextForPID(
    pid_t processIdentifier,
    NSString * _Nullable bundleID,
    NSUInteger maxUTF16Units,
    NSTimeInterval budgetSeconds) {
    if (IsSecureEventInputEnabled() || !AXIsProcessTrusted()) return @"";
    const pid_t selfPID = NSProcessInfo.processInfo.processIdentifier;
    if (processIdentifier <= 0 || processIdentifier == selfPID || !maxUTF16Units) return @"";
    if (MSIMEIsAXBlacklistedBundleIdentifier(bundleID)) return @"";

    const NSTimeInterval clampedBudget = MIN(MAX(budgetSeconds, 0.02), 0.35);
    const NSTimeInterval deadline = NSProcessInfo.processInfo.systemUptime + clampedBudget;

    ScopedAXElement app(AXUIElementCreateApplication(processIdentifier), CFRelease);
    if (!app || AXUIElementSetMessagingTimeout(app.get(), kPerCallAXTimeoutSec) != kAXErrorSuccess) {
        return @"";
    }
    EnableEnhancedUIForAppIfSupported(app.get(), bundleID);

    id focusObj = CopyAXElementAttribute(app.get(), kAXFocusedUIElementAttribute);
    AXUIElementRef focus = focusObj ? (__bridge AXUIElementRef)focusObj : nullptr;
    if (focus && msime::mac::IsAXElementSecureTextField(focus)) {
        return @"";
    }

    id focusedWindowObj = CopyAXElementAttribute(app.get(), kAXFocusedWindowAttribute);
    if (!focusedWindowObj) return @"";

    if (MSIMEIsBrowserBundleIdentifier(bundleID)) {
        NSString *rawURL = BestEffortBrowserURL(focusObj, focusedWindowObj, deadline);
        if (MSIMEIsAXBlacklistedURLString(rawURL)) return @"";
    }

    // 若焦点位于浏览器/Electron 的 AXWebArea 内，优先以 AXWebArea 为根节点，跳过浏览器书签栏/标签栏噪声。
    id scanRoot = focusedWindowObj;
    if (focusObj) {
        id cursor = focusObj;
        for (int depth = 0; depth < kMaxWebAreaParentDepth && cursor; ++depth) {
            if (NSProcessInfo.processInfo.systemUptime >= deadline) break;
            NSString *role = CopyAXStringAttribute((__bridge AXUIElementRef)cursor, kAXRoleAttribute);
            if ([role isEqualToString:@"AXWebArea"]) {
                scanRoot = cursor;
                break;
            }
            cursor = CopyAXElementAttribute((__bridge AXUIElementRef)cursor, kAXParentAttribute);
        }
    }

    NSMutableArray<NSString *> *collected = [NSMutableArray array];
    CollectVisibleTextsFromAXRoot(scanRoot, maxUTF16Units, deadline, collected);
    if (!collected.count) return @"";
    return SanitizeVisibleScreenText([collected componentsJoinedByString:@"\n"], maxUTF16Units);
}
