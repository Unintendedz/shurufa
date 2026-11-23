#import "VoicePipelineHelper.h"
#import "../core/SharedVoicePreferences.h"
#import "VoiceProviderOptions.h"
#include "../../../common/HostApiString.h"
#include "msime_client.h"

#include <cstring>

@interface MSIMEVoicePostprocessResult ()
@property(nonatomic, copy, readwrite) NSString *text;
@property(nonatomic, assign, readwrite) BOOL dismissed;
@property(nonatomic, copy, readwrite, nullable) NSString *dismissedReason;
@property(nonatomic, assign, readwrite) BOOL changed;
@end

@implementation MSIMEVoicePostprocessResult
- (instancetype)initWithText:(NSString *)text
                   dismissed:(BOOL)dismissed
             dismissedReason:(nullable NSString *)dismissedReason
                     changed:(BOOL)changed {
    if (!(self = [super init])) return nil;
    _text = [text copy] ?: @"";
    _dismissed = dismissed;
    _dismissedReason = [dismissedReason copy];
    _changed = changed;
    return self;
}
@end

@interface MSIMEVoiceVocabularyBundle ()
@property(nonatomic, copy, readwrite) NSArray<NSDictionary<NSString *, id> *> *terms;
@property(nonatomic, copy, readwrite) NSArray<NSString *> *promptHints;
@end

@implementation MSIMEVoiceVocabularyBundle
- (instancetype)initWithTerms:(NSArray<NSDictionary<NSString *, id> *> *)terms
                  promptHints:(NSArray<NSString *> *)promptHints {
    if (!(self = [super init])) return nil;
    _terms = [terms copy] ?: @[];
    _promptHints = [promptHints copy] ?: @[];
    return self;
}
@end

namespace {

// 解析标准 Rust C ABI 信封 `{"ok": true, "value": ...}`，失败返回 nil。
id CallHostJSON(char *(*fn)(const uint8_t *, size_t), NSDictionary *payload) {
    if (!fn || ![payload isKindOfClass:NSDictionary.class] || ![NSJSONSerialization isValidJSONObject:payload]) {
        return nil;
    }
    NSData *body = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    if (!body.length) return nil;
    auto owned = msime::host_api::own_string(fn(static_cast<const uint8_t *>(body.bytes), body.length));
    if (!owned) return nil;
    NSData *raw = [NSData dataWithBytes:owned.get() length:std::strlen(owned.get())];
    NSDictionary *envelope = [NSJSONSerialization JSONObjectWithData:raw options:0 error:nil];
    if (![envelope isKindOfClass:NSDictionary.class] || ![envelope[@"ok"] isEqual:@YES]) {
        return nil;
    }
    return envelope[@"value"];
}

// 仅裁剪首尾普通 ASCII 空格，保留 `\r`/`\n`/`\t` 等控制字符交由 Rust 安全校验显式拒绝，避免静默吞错。
NSString *TrimAsciiSpaces(id value) {
    if (![value isKindOfClass:NSString.class]) return @"";
    static NSCharacterSet *asciiSpace = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        asciiSpace = [NSCharacterSet characterSetWithCharactersInString:@" "];
    });
    return [(NSString *)value stringByTrimmingCharactersInSet:asciiSpace];
}

// 仅用于非安全敏感枚举/提示词正文的首尾空白清理。
NSString *TrimWhitespace(id value) {
    if (![value isKindOfClass:NSString.class]) return @"";
    return [(NSString *)value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
}

NSString *DefaultPolishEndpointForProvider(NSString *provider) {
    static NSDictionary<NSString *, NSString *> *defaults = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        defaults = @{
            @"siliconflow": @"https://api.siliconflow.cn/v1/chat/completions",
            @"openai": @"https://api.openai.com/v1/chat/completions",
            @"deepseek": @"https://api.deepseek.com/chat/completions",
            @"groq": @"https://api.groq.com/openai/v1/chat/completions",
            @"minimax": @"https://api.minimax.cn/v1/chat/completions",
            @"kimi": @"https://api.moonshot.cn/v1/chat/completions",
            @"moonshot": @"https://api.moonshot.cn/v1/chat/completions",
            @"zhipu": @"https://open.bigmodel.cn/api/paas/v4/chat/completions",
            @"qwen": @"https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            @"dashscope": @"https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            @"volcengine": @"https://ark.cn-beijing.volces.com/api/v3/chat/completions",
            @"openrouter": @"https://openrouter.ai/api/v1/chat/completions",
            @"everyapi": @"https://api.everyapi.ai/v1/chat/completions",
            @"mistral": @"https://api.mistral.ai/v1/chat/completions",
            @"ollama": @"http://127.0.0.1:11434/v1/chat/completions",
            @"local": @"",
        };
    });
    return defaults[provider] ?: @"";
}

NSString *DefaultPolishModelForProvider(NSString *provider) {
    static NSDictionary<NSString *, NSString *> *defaults = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        defaults = @{
            @"siliconflow": @"Qwen/Qwen3-8B",
            @"openai": @"gpt-4o-mini",
            @"deepseek": @"deepseek-v4-flash",
            @"groq": @"llama-3.3-70b-versatile",
            @"minimax": @"MiniMax-M3.1-Flash",
            @"kimi": @"kimi-k2-0905-preview",
            @"moonshot": @"kimi-k2-0905-preview",
            @"zhipu": @"glm-4-flash",
            @"qwen": @"qwen-plus",
            @"dashscope": @"qwen-plus",
            @"volcengine": @"doubao-seed-1.6-flash",
            @"openrouter": @"openrouter/auto",
            @"everyapi": @"openai/gpt-4o-mini",
            @"mistral": @"mistral-small-latest",
            @"ollama": @"qwen3.5:9b",
            @"local": @"local",
        };
    });
    return defaults[provider] ?: @"";
}

NSString *EffectivePolishProvider(NSDictionary * _Nullable options) {
    NSString *provider = TrimWhitespace(options[@"polish_provider"]).lowercaseString;
    return provider.length ? provider : MSIMEVoicePolishDefaultProvider;
}

BOOL IsValidBooleanNumber(id value) {
    return [value isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID();
}

// 以下分类函数与 `crates/client-providers/src/voice/polish_context.rs::classify_app` 逐条保持同步。
BOOL DomainMatches(NSString *host, NSString *target) {
    if ([host isEqualToString:target]) return YES;
    if (host.length <= target.length) return NO;
    return [host hasSuffix:target] &&
        [host characterAtIndex:(host.length - target.length - 1)] == '.';
}

NSString * _Nullable ClassifyWebDomain(NSString *host) {
    if (!host.length) return nil;
    static NSArray<NSString *> *aiDomains = nil;
    static NSArray<NSString *> *mailDomains = nil;
    static NSArray<NSString *> *imDomains = nil;
    static NSArray<NSString *> *docDomains = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        aiDomains = @[
            @"claude.ai", @"chatgpt.com", @"chat.openai.com", @"chat.deepseek.com",
            @"gemini.google.com", @"aistudio.google.com", @"poe.com", @"perplexity.ai",
            @"grok.com", @"kimi.moonshot.cn", @"kimi.com", @"doubao.com",
            @"yuanbao.tencent.com", @"chat.qwen.ai", @"tongyi.aliyun.com"
        ];
        mailDomains = @[
            @"mail.google.com", @"outlook.live.com", @"outlook.office.com",
            @"outlook.office365.com", @"mail.qq.com", @"mail.163.com",
            @"mail.126.com", @"mail.proton.me", @"app.fastmail.com", @"mail.icloud.com"
        ];
        imDomains = @[
            @"web.whatsapp.com", @"web.telegram.org", @"app.slack.com", @"discord.com",
            @"wx.qq.com", @"teams.microsoft.com", @"teams.live.com"
        ];
        docDomains = @[
            @"docs.google.com", @"sheets.google.com", @"slides.google.com",
            @"notion.so", @"notion.site", @"feishu.cn", @"larksuite.com",
            @"github.com", @"gitlab.com", @"linear.app", @"atlassian.net",
            @"yuque.com", @"wolai.com", @"coda.io", @"overleaf.com",
            @"craft.do", @"figma.com", @"cloudflare.com", @"vercel.com"
        ];
    });
    for (NSString *target in aiDomains) if (DomainMatches(host, target)) return @"ai_chat";
    for (NSString *target in mailDomains) if (DomainMatches(host, target)) return @"mail";
    for (NSString *target in imDomains) if (DomainMatches(host, target)) return @"im";
    for (NSString *target in docDomains) if (DomainMatches(host, target)) return @"browser_doc";
    return nil;
}

NSString *ClassifyBrowserTitle(NSString *title) {
    if (!title.length) return @"browser_doc";
    for (NSString *kw in @[@"claude", @"chatgpt", @"deepseek", @"gemini", @"perplexity", @"poe"]) {
        if ([title containsString:kw]) return @"ai_chat";
    }
    for (NSString *kw in @[@"gmail", @"outlook", @"收件箱", @"写邮件", @"proton mail"]) {
        if ([title containsString:kw]) return @"mail";
    }
    for (NSString *kw in @[@"whatsapp", @"telegram", @"slack", @"discord"]) {
        if ([title containsString:kw]) return @"im";
    }
    return @"browser_doc";
}

NSString * _Nullable ClassifyBundleID(NSString *bundle, NSString *title) {
    if (!bundle.length) return nil;
    static NSSet<NSString *> *imBundles = nil;
    static NSSet<NSString *> *ideBundles = nil;
    static NSSet<NSString *> *aiBundles = nil;
    static NSSet<NSString *> *mailBundles = nil;
    static NSSet<NSString *> *browserBundles = nil;
    static NSSet<NSString *> *docBundles = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        imBundles = [NSSet setWithArray:@[
            @"com.tencent.xinwechat", @"com.tencent.qq", @"com.tencent.mipadqq",
            @"com.tdesktop.telegram", @"ru.keepcoder.telegram", @"com.tinyspeck.slackmacgap",
            @"net.whatsapp.whatsapp", @"com.whatsapp.whatsapp", @"desktop.whatsapp",
            @"com.apple.mobilesms", @"com.electron.lark", @"com.bytedance.feishu",
            @"com.larksuite.larkapp", @"com.hnc.discord", @"com.alibaba.dingtalkmac",
            @"com.tencent.weworkmac", @"com.tencent.meeting",
            @"org.whispersystems.signal-desktop", @"com.microsoft.teams", @"com.microsoft.teams2"
        ]];
        ideBundles = [NSSet setWithArray:@[
            @"com.googlecode.iterm2", @"dev.warp.warp-stable", @"dev.warp.warp-preview",
            @"com.apple.terminal", @"com.mitchellh.ghostty", @"net.kovidgoyal.kitty",
            @"io.alacritty", @"org.alacritty", @"co.zeit.hyper", @"com.github.wez.wezterm",
            @"com.openai.codex", @"com.microsoft.vscode", @"com.microsoft.vscodeinsiders",
            @"com.todesktop.230313mzl4w4u92", @"com.exafunction.windsurf",
            @"ai.opencode.desktop", @"dev.zed.zed", @"dev.zed.zed-preview",
            @"com.apple.dt.xcode", @"com.sublimetext.4", @"com.sublimetext.3"
        ]];
        aiBundles = [NSSet setWithArray:@[
            @"com.anthropic.claudefordesktop", @"com.openai.chat", @"com.openai.atlas",
            @"sh.paseo.desktop", @"com.kangfenmao.cherrystudio", @"ai.perplexity.mac",
            @"com.electron.ollama", @"ai.elementlabs.lmstudio"
        ]];
        mailBundles = [NSSet setWithArray:@[
            @"com.apple.mail", @"com.microsoft.outlook", @"com.readdle.smartemail-mac",
            @"com.freron.mailmate", @"org.mozilla.thunderbird",
            @"com.postbox-inc.postboxapp", @"com.superhuman.electron", @"com.mimestream.mimestream"
        ]];
        browserBundles = [NSSet setWithArray:@[
            @"company.thebrowser.browser", @"company.thebrowser.dia",
            @"com.google.chrome", @"com.google.chrome.beta", @"com.google.chrome.canary",
            @"com.apple.safari", @"com.apple.safaritechnologypreview",
            @"org.mozilla.firefox", @"org.mozilla.firefoxdeveloperedition",
            @"com.microsoft.edgemac", @"com.microsoft.edgemac.beta",
            @"com.brave.browser", @"com.vivaldi.vivaldi",
            @"com.operasoftware.opera", @"com.kagi.kagimacos"
        ]];
        docBundles = [NSSet setWithArray:@[
            @"notion.id", @"md.obsidian", @"com.apple.notes", @"com.apple.iwork.pages",
            @"com.microsoft.word", @"com.typora.typora", @"abnerworks.typora",
            @"net.shinyfrog.bear", @"com.linear"
        ]];
    });
    if ([imBundles containsObject:bundle]) return @"im";
    if ([bundle hasPrefix:@"com.jetbrains."] || [ideBundles containsObject:bundle]) return @"terminal_ide";
    if ([aiBundles containsObject:bundle]) return @"ai_chat";
    if ([mailBundles containsObject:bundle]) return @"mail";
    if ([browserBundles containsObject:bundle]) return ClassifyBrowserTitle(title);
    if ([docBundles containsObject:bundle]) return @"browser_doc";
    return nil;
}

NSString * _Nullable ClassifyAppName(NSString *name, NSString *title) {
    if (!name.length) return nil;
    static NSSet<NSString *> *imNames = nil;
    static NSSet<NSString *> *ideNames = nil;
    static NSSet<NSString *> *aiNames = nil;
    static NSSet<NSString *> *mailNames = nil;
    static NSSet<NSString *> *browserNames = nil;
    static NSSet<NSString *> *docNames = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        imNames = [NSSet setWithArray:@[
            @"wechat", @"微信", @"qq", @"telegram", @"slack", @"whatsapp",
            @"messages", @"信息", @"discord", @"feishu", @"飞书", @"lark",
            @"dingtalk", @"钉钉", @"企业微信", @"wecom", @"腾讯会议",
            @"tencent meeting", @"signal"
        ]];
        ideNames = [NSSet setWithArray:@[
            @"iterm2", @"iterm", @"warp", @"terminal", @"终端", @"ghostty",
            @"kitty", @"alacritty", @"wezterm", @"codex", @"visual studio code",
            @"vs code", @"code", @"cursor", @"windsurf", @"opencode",
            @"zed", @"xcode", @"sublime text"
        ]];
        aiNames = [NSSet setWithArray:@[
            @"claude", @"chatgpt", @"chatgpt atlas", @"paseo",
            @"cherry studio", @"cherrystudio", @"perplexity"
        ]];
        mailNames = [NSSet setWithArray:@[
            @"mail", @"邮件", @"outlook", @"spark", @"thunderbird", @"mailmate", @"superhuman"
        ]];
        browserNames = [NSSet setWithArray:@[
            @"arc", @"dia", @"google chrome", @"chrome", @"safari", @"firefox",
            @"microsoft edge", @"edge", @"brave", @"vivaldi"
        ]];
        docNames = [NSSet setWithArray:@[
            @"notion", @"obsidian", @"notes", @"备忘录", @"pages", @"word", @"typora", @"bear", @"linear"
        ]];
    });
    if ([imNames containsObject:name]) return @"im";
    if ([ideNames containsObject:name] ||
        [name hasPrefix:@"intellij"] || [name hasPrefix:@"pycharm"] ||
        [name hasPrefix:@"rustrover"] || [name hasPrefix:@"webstorm"] ||
        [name hasPrefix:@"goland"] || [name hasPrefix:@"clion"]) {
        return @"terminal_ide";
    }
    if ([aiNames containsObject:name]) return @"ai_chat";
    if ([mailNames containsObject:name]) return @"mail";
    if ([browserNames containsObject:name]) return ClassifyBrowserTitle(title);
    if ([docNames containsObject:name]) return @"browser_doc";
    return nil;
}

NSArray<NSString *> *ParsePromptHints(id value) {
    if (![value isKindOfClass:NSDictionary.class]) return @[];
    NSArray *rawHints = [value[@"prompt_hints"] isKindOfClass:NSArray.class] ? value[@"prompt_hints"] : @[];
    NSMutableArray<NSString *> *hints = [NSMutableArray arrayWithCapacity:MIN(rawHints.count, (NSUInteger)60)];
    for (id item in rawHints) {
        if ([item isKindOfClass:NSString.class] && [(NSString *)item length]) {
            [hints addObject:item];
            if (hints.count >= 60) break;
        }
    }
    return [hints copy];
}

} // namespace

NSString *MSIMEVoiceInferAppCategory(MSIMEVoiceContextSnapshot * _Nullable snapshot) {
    if (!snapshot || snapshot.isSecureField) return @"general";
    NSString *domain = TrimWhitespace(snapshot.webDomain).lowercaseString;
    while ([domain hasSuffix:@"."]) {
        domain = [domain substringToIndex:domain.length - 1];
    }
    NSString *category = ClassifyWebDomain(domain);
    if (category) return category;

    NSString *bundle = TrimWhitespace(snapshot.appBundleID).lowercaseString;
    NSString *title = TrimWhitespace(snapshot.windowTitle).lowercaseString;
    category = ClassifyBundleID(bundle, title);
    if (category) return category;

    NSString *name = TrimWhitespace(snapshot.appName).lowercaseString;
    category = ClassifyAppName(name, title);
    if (category) return category;

    return @"general";
}

MSIMEVoiceVocabularyBundle *MSIMEVoiceLoadVocabulary(
    NSDictionary * _Nullable hostOptions,
    NSString * _Nullable rawText,
    NSString * _Nullable appCategory) {
    if (![hostOptions isKindOfClass:NSDictionary.class] || !hostOptions.count) {
        return [[MSIMEVoiceVocabularyBundle alloc] initWithTerms:@[] promptHints:@[]];
    }
    NSMutableDictionary<NSString *, id> *request = [NSMutableDictionary dictionary];
    request[@"options"] = hostOptions;
    id prefDir = hostOptions[@"preferences_directory"];
    if ([prefDir isKindOfClass:NSString.class] && [(NSString *)prefDir length]) {
        request[@"preferences_directory"] = prefDir;
    }
    request[@"limit"] = @300;
    if (rawText.length) {
        request[@"raw_text"] = rawText;
        request[@"prompt_limit"] = @60;
    }
    if (appCategory.length) {
        request[@"app_category"] = appCategory;
    }
    id value = CallHostJSON(msime_client_voice_vocabulary, request);
    if (![value isKindOfClass:NSDictionary.class]) {
        return [[MSIMEVoiceVocabularyBundle alloc] initWithTerms:@[] promptHints:@[]];
    }
    NSArray *rawTerms = [value[@"terms"] isKindOfClass:NSArray.class] ? value[@"terms"] : @[];
    NSMutableArray<NSDictionary<NSString *, id> *> *terms = [NSMutableArray arrayWithCapacity:rawTerms.count];
    for (id item in rawTerms) {
        if ([item isKindOfClass:NSDictionary.class]) {
            [terms addObject:item];
        }
    }
    return [[MSIMEVoiceVocabularyBundle alloc] initWithTerms:terms promptHints:ParsePromptHints(value)];
}

NSArray<NSString *> *MSIMEVoiceSelectPromptHints(
    NSArray<NSDictionary<NSString *, id> *> * _Nullable cachedTerms,
    NSString * _Nullable rawText,
    NSString * _Nullable appCategory) {
    if (!cachedTerms.count || !rawText.length) return @[];
    NSMutableDictionary<NSString *, id> *request = [NSMutableDictionary dictionary];
    request[@"cached_terms"] = cachedTerms;
    request[@"raw_text"] = rawText;
    request[@"prompt_limit"] = @60;
    if (appCategory.length) {
        request[@"app_category"] = appCategory;
    }
    id value = CallHostJSON(msime_client_voice_vocabulary, request);
    return ParsePromptHints(value);
}

MSIMEVoicePostprocessResult *MSIMEVoiceRunPostprocess(
    NSString *rawText,
    NSString * _Nullable modelKind,
    NSNumber * _Nullable audioMs,
    NSString * _Nullable mode,
    NSString * _Nullable appCategory,
    NSArray<NSDictionary<NSString *, id> *> * _Nullable vocabularyTerms) {
    NSString *safeText = [rawText isKindOfClass:NSString.class] ? rawText : @"";
    NSMutableDictionary<NSString *, id> *request = [NSMutableDictionary dictionary];
    request[@"text"] = safeText;
    if (modelKind.length) request[@"model_kind"] = modelKind;
    if (audioMs != nil) request[@"audio_ms"] = @(audioMs.unsignedLongLongValue);
    if (mode.length) request[@"mode"] = mode;
    if (appCategory.length) request[@"app_category"] = appCategory;
    if (vocabularyTerms.count) request[@"vocabulary"] = vocabularyTerms;

    id value = CallHostJSON(msime_client_voice_postprocess, request);
    if (![value isKindOfClass:NSDictionary.class]) {
        // 确定性后处理调用异常时无损保留原文，避免丢失用户语音。
        return [[MSIMEVoicePostprocessResult alloc] initWithText:safeText
                                                       dismissed:NO
                                                 dismissedReason:nil
                                                         changed:NO];
    }
    NSString *text = [value[@"text"] isKindOfClass:NSString.class] ? value[@"text"] : safeText;
    BOOL dismissed = [value[@"dismissed"] isKindOfClass:NSNumber.class] && [value[@"dismissed"] boolValue];
    NSString *reason = [value[@"dismissed_reason"] isKindOfClass:NSString.class] ? value[@"dismissed_reason"] : nil;
    BOOL changed = [value[@"changed"] isKindOfClass:NSNumber.class] && [value[@"changed"] boolValue];
    return [[MSIMEVoicePostprocessResult alloc] initWithText:text
                                                   dismissed:dismissed
                                             dismissedReason:reason
                                                     changed:changed];
}

MSIMEVoicePolishContext * _Nullable MSIMEVoicePolishContextFromSnapshot(
    MSIMEVoiceContextSnapshot * _Nullable snapshot,
    BOOL contextAware) {
    if (!snapshot) return nil;
    if (snapshot.isSecureField) {
        // 密码框硬门控：无论上下文感知开关是否打开，均显式标记 isSecureField=YES 并清空全部文本。
        MSIMEVoicePolishContext *blocked = [MSIMEVoicePolishContext new];
        blocked.isSecureField = YES;
        blocked.appBundleID = @"";
        blocked.appName = @"";
        blocked.windowTitle = @"";
        blocked.webDomain = @"";
        blocked.precedingText = @"";
        blocked.followingText = @"";
        blocked.selectedText = @"";
        blocked.visibleScreenText = @"";
        blocked.axBlacklisted = NO;
        return blocked;
    }
    if (!contextAware) return nil;
    MSIMEVoicePolishContext *context = [MSIMEVoicePolishContext new];
    context.appBundleID = snapshot.appBundleID;
    context.appName = snapshot.appName;
    context.windowTitle = snapshot.windowTitle;
    context.webDomain = snapshot.webDomain;
    context.precedingText = snapshot.precedingText;
    context.followingText = snapshot.followingText;
    context.selectedText = snapshot.selectedText;
    context.visibleScreenText = snapshot.visibleScreenText;
    context.isSecureField = NO;
    context.axBlacklisted = snapshot.axBlacklisted;
    return context;
}

BOOL MSIMEVoiceOptionsContextAware(NSDictionary * _Nullable options) {
    id flag = options[@"polish_context_aware"];
    if (IsValidBooleanNumber(flag)) return [flag boolValue];
    return MSIMEVoicePolishContextAwareEnabled(NSUserDefaults.standardUserDefaults);
}

BOOL MSIMEVoiceOptionsSelectionRewriteEnabled(NSDictionary * _Nullable options) {
    id flag = options[@"polish_enable_selection_rewrite"];
    if (IsValidBooleanNumber(flag)) return [flag boolValue];
    return MSIMEVoicePolishEnableSelectionRewriteEnabled(NSUserDefaults.standardUserDefaults);
}

BOOL MSIMEVoiceShouldUsePolishClient(
    NSDictionary * _Nullable options,
    BOOL * _Nullable creationFailedOut) {
    if (creationFailedOut) *creationFailedOut = NO;
    if (![options isKindOfClass:NSDictionary.class]) return NO;

    BOOL enabled = NO;
    if ([options[@"polish_enabled"] isKindOfClass:NSNumber.class]) {
        enabled = [options[@"polish_enabled"] boolValue];
    } else if ([options[@"polish_text"] isKindOfClass:NSNumber.class]) {
        enabled = [options[@"polish_text"] boolValue];
    }
    if (!enabled) return NO;

    NSString *provider = EffectivePolishProvider(options);
    // 仅裁剪普通 ASCII 空格，保留控制字符交由后续 Rust 校验显式拒绝。
    NSString *endpoint = TrimAsciiSpaces(options[@"polish_endpoint"]);
    if (!endpoint.length) {
        endpoint = DefaultPolishEndpointForProvider(provider);
    }
    NSString *token = TrimAsciiSpaces(options[@"polish_token"]);
    const BOOL tokenless = MSIMEVoicePolishProviderIsTokenless(provider, endpoint);
    if (!tokenless && !token.length) {
        return NO;
    }

    // 与共享偏好和既有测试保持一致：自定义提示词超过 8192 字符时在发起前标记配置创建失败。
    for (NSString *key in @[@"polish_prompt_custom_1", @"polish_prompt_custom_2", @"polish_prompt_custom_3"]) {
        id custom = options[key];
        if ([custom isKindOfClass:NSString.class] && [(NSString *)custom length] > 8192) {
            if (creationFailedOut) *creationFailedOut = YES;
            return NO;
        }
    }
    return YES;
}

BOOL MSIMEVoiceResolveLocalPolishPrewarmPath(
    NSDictionary * _Nullable options,
    NSString * _Nullable * _Nullable modelPathOut) {
    if (modelPathOut) *modelPathOut = nil;
    BOOL creationFailed = NO;
    if (!MSIMEVoiceShouldUsePolishClient(options, &creationFailed)) return NO;
    if (![EffectivePolishProvider(options) isEqualToString:@"local"]) return NO;

    NSString *promptID = TrimWhitespace(options[@"polish_prompt_id"]);
    NSString *translationPath = TrimAsciiSpaces(options[@"polish_translation_model_path"]);
    NSString *localPath = TrimAsciiSpaces(options[@"polish_local_model_path"]);
    NSString *selectedPath = nil;
    if ([promptID isEqualToString:@"zh2en"] && translationPath.length) {
        selectedPath = translationPath;
    } else if (localPath.length) {
        selectedPath = localPath;
    }
    if (modelPathOut) *modelPathOut = selectedPath;
    return YES;
}

MSIMEVoicePolishRequest *MSIMEVoiceBuildPolishRequest(
    NSDictionary *options,
    NSString *inputText,
    NSString *fallbackText,
    MSIMEVoiceContextSnapshot * _Nullable snapshot,
    NSArray<NSString *> * _Nullable vocabularyHints) {
    NSString *provider = EffectivePolishProvider(options);
    // 保留 endpoint / model / token 中的控制字符交由 Rust `polish.rs` 显式校验，不静默剥离 `\r\n\t`。
    NSString *endpoint = TrimAsciiSpaces(options[@"polish_endpoint"]);
    if (!endpoint.length && ![provider isEqualToString:@"local"]) {
        endpoint = DefaultPolishEndpointForProvider(provider);
    }
    NSString *model = TrimAsciiSpaces(options[@"polish_model"]);
    if (!model.length) {
        model = DefaultPolishModelForProvider(provider);
    }
    NSString *token = TrimAsciiSpaces(options[@"polish_token"]);
    NSString *promptID = TrimWhitespace(options[@"polish_prompt_id"]);
    if (!promptID.length) promptID = @"cleanup";

    NSMutableDictionary<NSString *, NSString *> *customPrompts = [NSMutableDictionary dictionary];
    NSDictionary<NSString *, NSString *> *slotMap = @{
        @"custom_1": @"polish_prompt_custom_1",
        @"custom_2": @"polish_prompt_custom_2",
        @"custom_3": @"polish_prompt_custom_3",
    };
    for (NSString *slot in slotMap) {
        NSString *text = TrimWhitespace(options[slotMap[slot]]);
        if (text.length) customPrompts[slot] = text;
    }

    NSString *localModelPath = nil;
    if ([provider isEqualToString:@"local"]) {
        NSString *translationPath = TrimAsciiSpaces(options[@"polish_translation_model_path"]);
        NSString *generalPath = TrimAsciiSpaces(options[@"polish_local_model_path"]);
        if ([promptID isEqualToString:@"zh2en"] && translationPath.length) {
            localModelPath = translationPath;
        } else if (generalPath.length) {
            localModelPath = generalPath;
        }
    }

    const BOOL contextAware = MSIMEVoiceOptionsContextAware(options);
    const BOOL selectionRewrite = MSIMEVoiceOptionsSelectionRewriteEnabled(options);

    MSIMEVoicePolishRequest *request = [MSIMEVoicePolishRequest new];
    request.provider = provider;
    request.endpoint = [provider isEqualToString:@"local"] ? nil : endpoint;
    request.model = model;
    request.token = ([provider isEqualToString:@"local"] || [provider isEqualToString:@"ollama"]) ? nil : token;
    request.localModelPath = localModelPath;
    request.promptID = promptID;
    request.customPrompts = customPrompts.count ? [customPrompts copy] : nil;
    request.mode = @"auto";
    request.enableSelectionRewrite = selectionRewrite;
    if ([promptID isEqualToString:@"zh2en"]) {
        request.targetLanguage = @"en";
    }
    request.stream = NO;
    request.enablePanguSpacing = YES;
    request.rawText = inputText ?: @"";
    request.fallbackText = fallbackText.length ? fallbackText : inputText;
    request.context = MSIMEVoicePolishContextFromSnapshot(snapshot, contextAware);
    request.vocabularyHints = vocabularyHints.count ? vocabularyHints : nil;
    return request;
}

NSDictionary<NSString *, id> *MSIMEVoiceBuildArchiveMetadata(
    NSString * _Nullable rawText,
    uint64_t asrMs,
    MSIMEVoicePolishResult * _Nullable polishResult,
    NSDictionary * _Nullable options,
    NSString * _Nullable appCategory,
    BOOL polishFailedFallback) {
    NSMutableDictionary<NSString *, id> *metadata = [NSMutableDictionary dictionary];
    if (rawText.length) {
        metadata[@"raw_text"] = [rawText copy];
    }
    if (asrMs > 0) {
        metadata[@"asr_ms"] = @(asrMs);
    }
    if (polishResult) {
        [metadata addEntriesFromDictionary:[polishResult archiveMetadata]];
    } else {
        metadata[@"mode"] = @"dictation";
        metadata[@"app_category"] = appCategory.length ? appCategory : @"general";
        if (polishFailedFallback) {
            metadata[@"polish_failed"] = @YES;
        }
    }
    if (polishResult || polishFailedFallback) {
        NSString *provider = EffectivePolishProvider(options);
        if (provider.length) metadata[@"polish_provider"] = provider;
        NSString *model = TrimAsciiSpaces(options[@"polish_model"]);
        if (!model.length) model = DefaultPolishModelForProvider(provider);
        if (model.length) metadata[@"polish_model"] = model;
        NSString *promptID = TrimWhitespace(options[@"polish_prompt_id"]);
        if (!promptID.length) promptID = @"cleanup";
        metadata[@"polish_prompt_id"] = promptID;
    }
    return [metadata copy];
}
