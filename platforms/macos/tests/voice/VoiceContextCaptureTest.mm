// 链接依赖：-framework AppKit -framework ApplicationServices -framework Carbon
#import "../../src/voice/VoiceContextCapture.h"
#import "../../src/core/AXPrecedingContext.h"
#include <stdexcept>

namespace {

void require(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}

void TestSecureFieldBlocking() {
    // 密码框或系统安全输入态下必须立即清空全部字段，不泄漏任何标题、域名或文本。
    MSIMEVoiceContextSnapshot *eventSecure = MSIMEBuildVoiceContextSnapshot(
        YES,
        NO,
        @"com.google.Chrome",
        @"Google Chrome",
        @"Synthetic Login Page",
        @"https://auth.example.invalid/login?next=dashboard",
        @"synthetic preceding text",
        @"synthetic selected secret");
    require(eventSecure.isSecureField, "安全事件输入态必须标记 isSecureField");
    require(!eventSecure.axBlacklisted, "安全输入态不应误标 axBlacklisted");
    require(eventSecure.appBundleID.length == 0, "安全输入态必须清空 appBundleID");
    require(eventSecure.appName.length == 0, "安全输入态必须清空 appName");
    require(eventSecure.windowTitle.length == 0, "安全输入态必须清空 windowTitle");
    require(eventSecure.webDomain.length == 0, "安全输入态必须清空 webDomain");
    require(eventSecure.precedingText.length == 0, "安全输入态必须清空 precedingText");
    require(eventSecure.selectedText.length == 0, "安全输入态必须清空 selectedText");

    MSIMEVoiceContextSnapshot *axSecure = MSIMEBuildVoiceContextSnapshot(
        NO,
        YES,
        @"com.apple.Notes",
        @"Notes",
        @"Synthetic Note",
        nil,
        @"前文内容",
        @"选中内容");
    require(axSecure.isSecureField, "AX 密码框必须标记 isSecureField");
    require(axSecure.precedingText.length == 0 && axSecure.selectedText.length == 0 && axSecure.windowTitle.length == 0,
            "AX 密码框必须清空所有文本与标题字段");

    NSDictionary *dict = [axSecure toJSONDictionary];
    require([dict[@"is_secure_field"] isEqual:@YES], "JSON 字典应包含 is_secure_field=true");
    require([dict[@"selected_text"] isEqual:@""], "密码框 JSON 字典不应携带选区文本");

    // 校验从 AXPrecedingContext.h 抽离的安全子角色判定内联函数。
    require(msime::mac::IsSecureTextFieldSubrole((__bridge NSString *)kAXSecureTextFieldSubrole),
            "kAXSecureTextFieldSubrole 必须判为安全密码框");
    require(!msime::mac::IsSecureTextFieldSubrole((__bridge NSString *)kAXTextFieldRole),
            "普通文本框角色不应判为安全密码框");
    require(!msime::mac::IsSecureTextFieldSubrole(@"AXSearchField"),
            "搜索框子角色不应判为安全密码框");
    require(!msime::mac::IsSecureTextFieldSubrole(nil), "nil 子角色必须返回 false");
    require(!msime::mac::IsSecureTextFieldSubrole(@(123)), "非字符串子角色必须返回 false");
    require(!msime::mac::IsAXElementSecureTextField(nullptr), "空 AX 元素必须返回 false");
    require(msime::mac::ReadAXPrecedingContextFromElement(nullptr, 500) == nil,
            "空 AX 元素读取前文必须安全返回 nil");
}

void TestAXBlacklist() {
    require(MSIMEIsAXBlacklistedBundleIdentifier(@"com.tencent.xinWeChat"), "微信必须在默认 AX 黑名单中");
    require(MSIMEIsAXBlacklistedBundleIdentifier(@"COM.TENCENT.XINWECHAT"), "AX 黑名单匹配必须忽略大小写");
    require(MSIMEIsAXBlacklistedBundleIdentifier(@"com.sublimetext.4"), "Sublime Text 4 必须在默认 AX 黑名单中");
    require(!MSIMEIsAXBlacklistedBundleIdentifier(@"com.microsoft.VSCode"), "VS Code 不应在默认 AX 黑名单中");
    require(!MSIMEIsAXBlacklistedBundleIdentifier(nil), "nil bundleID 不应命中黑名单");

    // 微信命中黑名单时，保留 bundleID 与 appName 供 IM 分类，但必须清空前文与选区。
    MSIMEVoiceContextSnapshot *wechat = MSIMEBuildVoiceContextSnapshot(
        NO,
        NO,
        @"com.tencent.xinWeChat",
        @"WeChat",
        @"合成群聊",
        nil,
        @"不该读取的微信前文",
        @"不该读取的微信选区");
    require(wechat.axBlacklisted, "微信快照必须标记 axBlacklisted");
    require(!wechat.isSecureField, "微信黑名单降级不应误标为密码框");
    require([wechat.appBundleID isEqualToString:@"com.tencent.xinWeChat"], "黑名单降级必须保留 appBundleID");
    require([wechat.appName isEqualToString:@"WeChat"], "黑名单降级必须保留 appName");
    require(wechat.precedingText.length == 0, "黑名单降级必须跳过前文");
    require(wechat.selectedText.length == 0, "黑名单降级必须跳过选区");

    // Google Docs 编辑页命中页面级黑名单：保留域名与标题，清空前文与选区。
    NSString *gdocURL = @"https://docs.google.com/document/d/synthetic-doc-id-42/edit?usp=sharing#heading=h.1";
    require(MSIMEIsAXBlacklistedURLString(gdocURL), "docs.google.com/document/d 必须命中页面黑名单");
    require(MSIMEIsAXBlacklistedURLString(@"https://docs.google.com/document/u/1/d/synthetic-doc-id-42/edit"),
            "多账号 Google Docs 路径也必须命中页面黑名单");
    require(!MSIMEIsAXBlacklistedURLString(@"https://example.invalid/redirect?u=https://docs.google.com/document/d/1"),
            "查询参数里的伪造路径不得误触发 Google Docs 黑名单");
    require(!MSIMEIsAXBlacklistedURLString(@"https://docs.google.com/spreadsheets/d/synthetic-sheet-1"),
            "非 /document/d 路径不应误命中 Google Docs 黑名单");

    MSIMEVoiceContextSnapshot *gdoc = MSIMEBuildVoiceContextSnapshot(
        NO,
        NO,
        @"company.thebrowser.Browser",
        @"Arc",
        @"Synthetic Design Doc - Google Docs",
        gdocURL,
        @"不该读取的文档前文",
        @"不该读取的文档选区");
    require(gdoc.axBlacklisted, "Google Docs 页面必须标记 axBlacklisted");
    require([gdoc.webDomain isEqualToString:@"docs.google.com"], "Google Docs 黑名单快照仍须保留剥离后的 host");
    require(gdoc.precedingText.length == 0 && gdoc.selectedText.length == 0,
            "Google Docs 黑名单快照必须跳过前文与选区");
}

void TestBrowserDomainExtraction() {
    require(MSIMEIsBrowserBundleIdentifier(@"company.thebrowser.Browser"), "Arc 应识别为浏览器");
    require(MSIMEIsBrowserBundleIdentifier(@"com.google.Chrome"), "Chrome 应识别为浏览器");
    require(MSIMEIsBrowserBundleIdentifier(@"com.apple.Safari"), "Safari 应识别为浏览器");
    require(MSIMEIsBrowserBundleIdentifier(@"org.mozilla.firefox"), "Firefox 应识别为浏览器");
    require(MSIMEIsBrowserBundleIdentifier(@"com.microsoft.edgemac"), "Edge 应识别为浏览器");
    require(!MSIMEIsBrowserBundleIdentifier(@"com.apple.mail"), "Mail 不应识别为浏览器");

    // 严格剥离凭据、端口、路径、查询参数与片段，仅保留小写 host。
    require([MSIMEExtractWebDomainFromURLString(
                @"https://user:secret@CLAUDE.AI:443/chat/synthetic-id?token=fake#bottom")
                isEqualToString:@"claude.ai"],
            "必须仅保留小写 host 并剥离路径、查询串与凭据");
    require([MSIMEExtractWebDomainFromURLString(@"http://127.0.0.1:18080/v1/chat?foo=bar")
                isEqualToString:@"127.0.0.1"],
            "本地 http 地址应正确提取 host");
    require([MSIMEExtractWebDomainFromURLString(@"https://dash.cloudflare.com./zone/synthetic")
                isEqualToString:@"dash.cloudflare.com"],
            "域名末尾点应规范剥离");
    require([MSIMEExtractWebDomainFromURLString(@"file:///Users/synthetic/private.md") isEqualToString:@""],
            "file:// 本地路径不得暴露为 webDomain");
    require([MSIMEExtractWebDomainFromURLString(@"about:blank") isEqualToString:@""],
            "非 http(s) 协议必须返回空串");
    require([MSIMEExtractWebDomainFromURLString(@"") isEqualToString:@""], "空 URL 必须返回空串");
    require([MSIMEExtractWebDomainFromURLString(nil) isEqualToString:@""], "nil URL 必须返回空串");

    // 非浏览器应用即使传入 URL，也不得填充 webDomain。
    MSIMEVoiceContextSnapshot *ide = MSIMEBuildVoiceContextSnapshot(
        NO,
        NO,
        @"com.microsoft.VSCode",
        @"Visual Studio Code",
        @"polish.rs — shurufa",
        @"https://github.com/synthetic/repo/pull/1",
        @"let x = 1;",
        @"");
    require([ide.webDomain isEqualToString:@""], "非浏览器应用不应写入 webDomain");
}

void TestFakeSelectionSanitization() {
    // QQ 经典假选区：仅含 U+FFFC（Object Replacement Character）或零宽字符时必须归空。
    require([MSIMESanitizeSelectedText(@"\uFFFC", 4000) isEqualToString:@""],
            "单个 U+FFFC 假选区必须清洗为空串");
    require([MSIMESanitizeSelectedText(@" \uFFFC\uFEFF\u200B\u200C\u200D\u2060 \t\n ", 4000) isEqualToString:@""],
            "纯占位符与零宽空白组合必须清洗为空串");
    require([MSIMESanitizeSelectedText(@" \uFFFC\u200B需要改写的合成文本\uFEFF\uFFFC \n", 4000)
                isEqualToString:@"需要改写的合成文本"],
            "有效选区文本中的 U+FFFC 与零宽字符应被剥离并保留正文");

    // 多行代码/Git diff 选区必须保留首行及各行的四空格前导缩进，同时剔除首尾空行与行尾空白。
    NSString *diff = @"  \n    - old_value = 1  \n    + new_value = 2\n  ";
    require([MSIMESanitizeSelectedText(diff, 4000) isEqualToString:@"    - old_value = 1\n    + new_value = 2"],
            "多行 Git diff 选区应完整保留首行四空格缩进与内部换行");

    // 选区包含 QQ U+FFFC 时，装配后的快照 selectedText 必须为空串。
    MSIMEVoiceContextSnapshot *qq = MSIMEBuildVoiceContextSnapshot(
        NO,
        NO,
        @"com.tencent.qq",
        @"QQ",
        @"合成好友",
        nil,
        @"刚才聊到",
        @"\uFFFC");
    require([qq.selectedText isEqualToString:@""], "QQ U+FFFC 假选区在快照中必须为空");
    require([qq.precedingText isEqualToString:@"刚才聊到"], "QQ 非黑名单时应保留前文");
}

void TestTerminalDowngrade() {
    for (NSString *bundle in @[
             @"com.googlecode.iterm2",
             @"dev.warp.Warp-Stable",
             @"com.apple.Terminal",
             @"com.mitchellh.ghostty",
         ]) {
        require(msime::mac::IsTerminalBundleIdentifier(bundle), "常见终端必须命中 IsTerminalBundleIdentifier");
        MSIMEVoiceContextSnapshot *snap = MSIMEBuildVoiceContextSnapshot(
            NO,
            NO,
            bundle,
            @"Terminal",
            @"zsh — synthetic-repo",
            nil,
            @"满屏滚动日志不该作为前文送出",
            @"  error: synthetic compile failure  ");
        require(!snap.isSecureField && !snap.axBlacklisted, "终端不应标记为密码框或黑名单");
        require([snap.appBundleID isEqualToString:bundle], "终端必须保留 appBundleID");
        require([snap.windowTitle isEqualToString:@"zsh — synthetic-repo"], "终端必须保留 windowTitle");
        require([snap.precedingText isEqualToString:@""], "终端必须跳过前文读取");
        require([snap.selectedText isEqualToString:@"error: synthetic compile failure"],
                "终端显式选中的错误文本应正常清洗保留");
    }
}

void TestBoundsAndJSONSerialization() {
    // 1) 超长 windowTitle 截断不得劈开代理对（😀 占 2 个 UTF-16 单元）。
    NSMutableString *longTitle = [NSMutableString string];
    for (int i = 0; i < 199; ++i) [longTitle appendString:@"a"];
    [longTitle appendString:@"😀tail"];
    NSString *clampedTitle = MSIMEClampUTF16String(longTitle, MSIMEVoiceContextMaxWindowTitleUTF16);
    require(clampedTitle.length == 199, "代理对跨边界时必须回退 1 个 UTF-16 单元避免产生孤立高代理");

    // 2) 超长 precedingText 必须保留靠近光标的尾部 500 单元。
    NSMutableString *longPreceding = [NSMutableString string];
    for (int i = 0; i < 600; ++i) [longPreceding appendString:@"x"];
    [longPreceding appendString:@"尾部锚点"];
    MSIMEVoiceContextSnapshot *bounded = MSIMEBuildVoiceContextSnapshot(
        NO,
        NO,
        @"com.anthropic.claudefordesktop",
        @"Claude",
        longTitle,
        nil,
        longPreceding,
        [@"选" stringByPaddingToLength:4500 withString:@"选" startingAtIndex:0]);
    require(bounded.windowTitle.length <= MSIMEVoiceContextMaxWindowTitleUTF16, "windowTitle 超过 200 UTF-16 必须截断");
    require(bounded.precedingText.length == MSIMEVoiceContextMaxPrecedingTextUTF16, "precedingText 应裁剪至 500 单元");
    require([bounded.precedingText hasSuffix:@"尾部锚点"], "precedingText 裁剪必须保留靠近光标的尾部内容");
    require(bounded.selectedText.length == MSIMEVoiceContextMaxSelectedTextUTF16, "selectedText 应裁剪至 4000 单元");

    // 3) 多字节 UTF-8 截断不得破坏汉字边界。
    NSString *longChineseName = [@"汉" stringByPaddingToLength:100 withString:@"汉" startingAtIndex:0];
    NSString *clampedName = MSIMEClampUTF8BytesString(longChineseName, MSIMEVoiceContextMaxAppNameBytes);
    require([clampedName lengthOfBytesUsingEncoding:NSUTF8StringEncoding] <= MSIMEVoiceContextMaxAppNameBytes,
            "appName UTF-8 字节数不得超过 128");
    require(clampedName.length == 42, "128 字节上限应容纳 42 个 3 字节汉字（126 字节）");

    // 4) JSON 字典键名严格对齐蓝图 C.4 且可合法序列化。
    NSDictionary *jsonDict = [bounded toJSONDictionary];
    NSError *err = nil;
    NSData *encoded = [NSJSONSerialization dataWithJSONObject:jsonDict options:0 error:&err];
    require(encoded != nil && err == nil, "toJSONDictionary 必须可合法序列化为 JSON");
    require([jsonDict[@"app_bundle_id"] isEqualToString:@"com.anthropic.claudefordesktop"],
            "app_bundle_id 字段缺失或不匹配");
    require([jsonDict[@"is_secure_field"] isEqual:@NO], "is_secure_field 字段应为 false");
    require([jsonDict[@"ax_blacklisted"] isEqual:@NO], "ax_blacklisted 字段应为 false");

    // 5) 实时采集在空客户端下必须安全返回非空对象，不弹窗、不崩溃。
    MSIMEVoiceContextSnapshot *live = MSIMECaptureVoiceContext(nil);
    require(live != nil, "MSIMECaptureVoiceContext(nil) 必须返回有效快照");
}

} // 匿名命名空间

int main() {
    @autoreleasepool {
        TestSecureFieldBlocking();
        TestAXBlacklist();
        TestBrowserDomainExtraction();
        TestFakeSelectionSanitization();
        TestTerminalDowngrade();
        TestBoundsAndJSONSerialization();
    }
    return 0;
}
