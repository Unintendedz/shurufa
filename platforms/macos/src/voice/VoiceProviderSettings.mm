#import "VoiceProviderSettings.h"
#import "VoiceProviderSettingsKeys.h"
#import "VoiceCaptureDevice.h"
#import "VoiceSettings.h"
#import "TextStreamSupport.h"
#import "OpenLessImport.h"
#import "../settings/SettingsLayout.h"
#import "../settings/RuntimeOptions.h"
#import "MSIMEClientSession.h"
#import "../core/SharedVoicePreferences.h"
#import "VoiceShortcutPresentation.h"
#import "VoiceShortcutPermissions.h"
#include "../../../common/HostApiString.h"
#include "msime_client.h"
NSNotificationName const MSIMEVoiceProviderSettingsDidChangeNotification = @"MSIMEClientVoiceProviderSettingsDidChange";
#import <Security/Security.h>
#import "../core/WindowPresentation.h"
#import "../core/EditionIdentity.h"
#import "../core/AIEndpointPolicy.h"
#include <cstring>
#include <sys/stat.h>
#include "../../../../shared/voice/VoiceProviderCatalog.h"

namespace
{
bool IsRegularDirectory(NSString *path)
{
    struct stat status = {};
    return lstat(path.fileSystemRepresentation, &status) == 0 && S_ISDIR(status.st_mode);
}

bool IsRegularFile(NSString *path)
{
    struct stat status = {};
    return lstat(path.fileSystemRepresentation, &status) == 0 && S_ISREG(status.st_mode);
}

// 跟随符号链接检查本地 GGUF 模型文件，便于用户软链复用已缓存的大体积模型。
bool IsReadableRegularFileFollowingSymlinks(NSString *path)
{
    if (!path.length || !path.isAbsolutePath)
        return false;
    struct stat status = {};
    return stat(path.fileSystemRepresentation, &status) == 0 && S_ISREG(status.st_mode);
}

NSString *Trimmed(NSString *value)
{
    return value ? [value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet] : @"";
}

NSString *LocalPolishModelDirectory(void)
{
    NSString *configured = NSProcessInfo.processInfo.environment[@"SHURUFA_LOCAL_LLM_DIR"];
    if (configured.length && configured.isAbsolutePath)
        return configured;
    return [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/Shurufa/local-llm"];
}

// 按显式路径优先、其次自动发现 ~/Library/Application Support/Shurufa/local-llm/*.gguf 的顺序查找本机润色模型。
NSString *DiscoveredLocalPolishModelPath(NSUserDefaults *defaults)
{
    NSString *explicitPath = Trimmed([defaults stringForKey:@"MSIMEClientVoicePolishLocalModelPath"]);
    if (explicitPath.length)
        return IsReadableRegularFileFollowingSymlinks(explicitPath) ? explicitPath : nil;
    NSString *directory = LocalPolishModelDirectory();
    NSArray<NSString *> *entries = [[NSFileManager.defaultManager contentsOfDirectoryAtPath:directory error:nil]
        sortedArrayUsingSelector:@selector(localizedStandardCompare:)];
    NSString *fallbackTranslation = nil;
    for (NSString *name in entries)
    {
        NSString *lower = name.lowercaseString;
        if (![lower hasSuffix:@".gguf"] || [lower containsString:@"mmproj"])
            continue;
        NSString *candidate = [directory stringByAppendingPathComponent:name];
        if (!IsReadableRegularFileFollowingSymlinks(candidate))
            continue;
        if ([lower containsString:@"hy-mt"])
        {
            if (!fallbackTranslation)
                fallbackTranslation = candidate;
            continue;
        }
        return candidate;
    }
    return fallbackTranslation;
}

NSArray<NSDictionary<NSString *, NSString *> *> *ProviderSpecs()
{
    static NSArray<NSDictionary<NSString *, NSString *> *> *specs;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        const auto text = [](std::string_view value) {
            return [[NSString alloc] initWithBytes:value.data() length:value.size() encoding:NSUTF8StringEncoding];
        };
        NSMutableDictionary<NSString *, NSDictionary<NSString *, NSString *> *> *remote = [NSMutableDictionary dictionary];
        for (const auto &preset : msime::voice::kAsrProviders) {
            NSString *identifier = text(preset.id);
            remote[identifier] = @{@"id": identifier, @"title": text(preset.title),
                                   @"endpoint": text(preset.endpoint), @"model": text(preset.model)};
        }
        // 只保留 macOS 的历史顺序，不在宿主复制共享服务的元数据。
        NSArray<NSString *> *priority = @[@"doubao", @"openai", @"siliconflow", @"groq",
                                          @"everyapi", @"mistral", @"bailian"];
        NSMutableArray<NSDictionary<NSString *, NSString *> *> *ordered = [NSMutableArray array];
        for (NSString *identifier in priority)
            if (remote[identifier]) [ordered addObject:remote[identifier]];
        for (const auto &preset : msime::voice::kAsrProviders) {
            NSString *identifier = text(preset.id);
            if (![priority containsObject:identifier]) [ordered addObject:remote[identifier]];
        }
        [ordered insertObject:@{@"id" : @"textstream", @"title" : @"本地流式识别（TextStream）", @"endpoint" : @"", @"model" : @""} atIndex:0];
        [ordered addObjectsFromArray:@[
            @{@"id" : @"system", @"title" : @"macOS 系统识别", @"endpoint" : @"", @"model" : @""},
            @{@"id" : @"local", @"title" : @"本地模型", @"endpoint" : @"", @"model" : @""}
        ]];
        specs = [ordered copy];
    });
    return specs;
}

NSArray<NSString *> *ProviderValues(NSString *key)
{
    NSMutableArray<NSString *> *values = [NSMutableArray arrayWithCapacity:ProviderSpecs().count];
    for (NSDictionary<NSString *, NSString *> *spec in ProviderSpecs())
        [values addObject:spec[key]];
    return [values copy];
}

NSString *ProviderValue(NSString *provider, NSString *key)
{
    NSString *identifier = provider.lowercaseString ?: @"";
    for (NSDictionary<NSString *, NSString *> *spec in ProviderSpecs())
        if ([spec[@"id"] isEqual:identifier]) return spec[key];
    return @"";
}
} // namespace

NSArray<NSString *> *MSIMEVoiceASRProviderIDs(void)
{
    return ProviderValues(@"id");
}

NSArray<NSString *> *MSIMEVoiceASRProviderTitles(void)
{
    return ProviderValues(@"title");
}

NSString *MSIMEVoiceASRProviderDefaultEndpoint(NSString *provider)
{
    return ProviderValue(provider, @"endpoint");
}

NSString *MSIMEVoiceASRProviderDefaultModel(NSString *provider)
{
    return ProviderValue(provider, @"model");
}

BOOL MSIMEVoiceASRProviderUsesService(NSString *provider)
{
    NSString *identifier = provider.lowercaseString ?: @"";
    return [MSIMEVoiceASRProviderIDs() containsObject:identifier] &&
           ![@[ @"system", @"local", @"textstream" ] containsObject:identifier];
}

/// Which of the form's two cards a message is about. A message is only useful next to the field it is about, and this form has two groups of fields with two independent sets of failures — an unreachable recognition endpoint and an unconfigured polish service are separate problems and used to be reported by the same red line under both cards, so neither said which one it meant.
static NSErrorUserInfoKey const MSIMEVoiceProviderErrorScopeKey = @"MSIMEVoiceProviderErrorScope";
static NSString *const MSIMEVoiceProviderRecognitionScope = @"recognition";
static NSString *const MSIMEVoiceProviderPolishScope = @"polish";

namespace
{
// 错误域不随版本而变：VoiceFailureMessages 按 app.msime.client.voice 前缀认出语音错误。钥匙串服务名随版本而变，见 MSIMEVoiceProviderKeychainService。
NSString *const errorDomain = @"app.msime.client.voice.providers";
NSError *Error(NSString *message, NSString *scope)
{
    return [NSError errorWithDomain:errorDomain
                               code:1
                           userInfo:@{NSLocalizedDescriptionKey : message, MSIMEVoiceProviderErrorScopeKey : scope}];
}
NSDictionary *Key(NSString *kind, NSString *provider, NSString *endpoint)
{
    return @{
        (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService : MSIMEVoiceProviderKeychainService(),
        (__bridge id)kSecAttrAccount : MSIMEVoiceProviderCredentialAccount(kind, provider, endpoint)
    };
}
NSString *ReadKey(NSDictionary *key)
{
    NSMutableDictionary *query = [key mutableCopy];
    query[(__bridge id)kSecReturnData] = @YES;
    CFTypeRef result = nullptr;
    OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
    if (status != errSecSuccess)
        return @"";
    NSData *data = CFBridgingRelease(result);
    NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    return text ? text : @"";
}
NSString *ReadToken(NSString *kind, NSString *provider, NSString *endpoint)
{
    return ReadKey(Key(kind, provider, endpoint));
}
void DeleteKey(NSDictionary *key)
{
    SecItemDelete((__bridge CFDictionaryRef)key);
}
BOOL WriteToken(NSString *kind, NSString *provider, NSString *endpoint, NSString *token, NSError **error)
{
    NSDictionary *query = Key(kind, provider, endpoint);
    if (token.length == 0)
    {
        const OSStatus status = SecItemDelete((__bridge CFDictionaryRef)query);
        if (status == errSecSuccess || status == errSecItemNotFound)
            return YES;
    }
    else
    {
        NSDictionary *attributes = @{(__bridge id)kSecValueData : [token dataUsingEncoding:NSUTF8StringEncoding]};
        OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)query, (__bridge CFDictionaryRef)attributes);
        if (status == errSecItemNotFound)
        {
            NSMutableDictionary *item = [query mutableCopy];
            [item addEntriesFromDictionary:attributes];
            status = SecItemAdd((__bridge CFDictionaryRef)item, nullptr);
        }
        if (status == errSecSuccess)
            return YES;
    }
    if (error)
        // The kind is the card: an ASR key belongs to the recognition card, a polish key to the text card.
        *error = Error(@"无法保存到系统钥匙串，请解锁钥匙串后重试。",
                       [kind isEqualToString:@"polish"] ? MSIMEVoiceProviderPolishScope
                                                        : MSIMEVoiceProviderRecognitionScope);
    return NO;
}
void DeleteToken(NSString *kind, NSString *provider, NSString *endpoint)
{
    DeleteKey(Key(kind, provider, endpoint));
}
BOOL IsEndpoint(NSString *value)
{
    NSURLComponents *url = [NSURLComponents componentsWithString:value];
    return [url.scheme.lowercaseString isEqualToString:@"https"] && url.host.length > 0 && !url.user && !url.password &&
           !url.fragment;
}
BOOL IsWebSocketEndpoint(NSString *value)
{
    NSURLComponents *url = [NSURLComponents componentsWithString:value];
    return [url.scheme.lowercaseString isEqualToString:@"wss"] && url.host.length > 0 && !url.user && !url.password &&
           !url.fragment;
}
// 仅去掉普通空格判断留空，保留 \n/\r/\t 等控制字符交由 Rust `ai::endpoint::validate` 拒绝。
NSString *TrimAsciiSpaces(NSString *value)
{
    if (!value)
        return @"";
    return [value stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@" "]];
}
// 复用 Rust `ai::endpoint::validate`（经 `MSIMEAIEndpointCheck`）判断润色地址是否指向本机、私网或 Tailscale 主机，不重复实现 IP/网段解析。
BOOL IsLocalNetworkPolishEndpoint(NSString *value)
{
    if (!value.length || TrimAsciiSpaces(value).length == 0)
        return NO;
    if (MSIMEAIEndpointCheck(value) != MSIMEAIEndpointProblemNone)
        return NO;
    NSString *spaceTrimmed = TrimAsciiSpaces(value);
    NSString *lower = spaceTrimmed.lowercaseString;
    if ([lower hasPrefix:@"http://"])
        return YES;
    if ([lower hasPrefix:@"https://"])
    {
        NSString *httpProbe = [@"http://" stringByAppendingString:[spaceTrimmed substringFromIndex:8]];
        return MSIMEAIEndpointCheck(httpProbe) == MSIMEAIEndpointProblemNone;
    }
    return NO;
}
BOOL PolishProviderIsLocal(NSString *provider)
{
    return [Trimmed(provider).lowercaseString isEqualToString:@"local"];
}
BOOL PolishProviderIsOllama(NSString *provider)
{
    return [Trimmed(provider).lowercaseString isEqualToString:@"ollama"];
}
// `local`/`ollama` 以及本机/私网/Tailscale 端点免填 API 密钥，公网云端服务仍必填。
BOOL PolishRequiresToken(NSString *polishProvider, NSString *polishEndpoint)
{
    if (PolishProviderIsLocal(polishProvider) || PolishProviderIsOllama(polishProvider))
        return NO;
    if (IsLocalNetworkPolishEndpoint(polishEndpoint))
        return NO;
    return YES;
}
} // namespace
@implementation MetasequoiaVoiceProviderSettings
// dictionaryForKey: type-checks the container and nothing inside it, and the NSString * properties
// enforce nothing at runtime, so a non-string leaf reached -length or -isEqualToString: and killed
// the input method with an unrecognized selector. The domain is only written here, so this needs an
// out-of-band edit (defaults write, a managed preference, a corrupt plist) — but the whole IME dies
// on the next Control+Option+V, and again on the one after that.
static NSString *StringSetting(NSDictionary *saved, NSString *key, NSString *fallback)
{
    id value = saved[key];
    return [value isKindOfClass:[NSString class]] ? (NSString *)value : fallback;
}

static NSString *SharedSetting(NSString *key, NSString *fallback)
{
    NSString *sharedKey = MSIMEVoiceProviderSharedKeys()[key];
    return MSIMEVoiceProviderSharedSetting(
        sharedKey ? [NSUserDefaults.standardUserDefaults objectForKey:sharedKey] : nil, fallback);
}

+ (instancetype)loadSettings
{
    MetasequoiaVoiceProviderSettings *value = [self new];
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    NSDictionary *saved = [defaults dictionaryForKey:@"voiceInput"];
    if (!saved)
        saved = @{};
    NSString *rawProvider =
        SharedSetting(@"provider", @"textstream").lowercaseString;
    if (![MSIMEVoiceASRProviderIDs() containsObject:rawProvider])
        rawProvider = @"textstream";
    value.provider = rawProvider;
    value.endpoint = SharedSetting(@"endpoint", @"");
    value.model = SharedSetting(@"model", @"");
    if (value.endpoint.length == 0)
        value.endpoint = MSIMEVoiceASRProviderDefaultEndpoint(rawProvider);
    if (value.model.length == 0)
        value.model = MSIMEVoiceASRProviderDefaultModel(rawProvider);
    value.modelPath = SharedSetting(@"modelPath", @"");
    value.polishEnabled = MSIMEVoicePolishEnabledForProvider(defaults, saved, rawProvider);
    NSString *polishProvider = [defaults stringForKey:@"MSIMEClientVoicePolishProvider"] ?: MSIMEVoicePolishDefaultProvider;
    // 本机托管（local）与 Ollama 各自使用对应缺省地址和模型，其余云端服务沿用既有缺省。
    if (PolishProviderIsLocal(polishProvider))
    {
        value.polishEndpoint = SharedSetting(@"polishEndpoint", @"");
        value.polishModel = SharedSetting(@"polishModel", @"local");
    }
    else if (PolishProviderIsOllama(polishProvider))
    {
        value.polishEndpoint = SharedSetting(@"polishEndpoint", @"http://127.0.0.1:11434/v1/chat/completions");
        value.polishModel = SharedSetting(@"polishModel", @"qwen3.5:9b");
    }
    else
    {
        value.polishEndpoint = SharedSetting(@"polishEndpoint", MSIMEVoicePolishDefaultEndpoint);
        value.polishModel = SharedSetting(@"polishModel", MSIMEVoicePolishDefaultModel);
    }
    value.polishPromptID = MSIMEPolishPromptIdentifierOrDefault(SharedSetting(@"polishPromptID", @""));
    value.polishPromptCustom1 = SharedSetting(@"polishPromptCustom1", @"");
    value.polishPromptCustom2 = SharedSetting(@"polishPromptCustom2", @"");
    value.polishPromptCustom3 = SharedSetting(@"polishPromptCustom3", @"");
    // 存着别的平台的录音后端时，宿主录音用系统默认设备（MSIMEVoiceCaptureDeviceUID），窗口照实显示「自动」。
    id sharedCaptureDevice = [defaults objectForKey:@"MSIMEClientVoiceCaptureDevice"];
    if (!MSIMEVoiceCaptureBackendSupported([defaults objectForKey:@"MSIMEClientVoiceCaptureBackend"]))
        value.captureDevice = @"";
    else
        value.captureDevice = [sharedCaptureDevice isKindOfClass:NSString.class]
            ? sharedCaptureDevice : StringSetting(saved, @"captureDevice", @"");
    value.tokenSlots = MSIMEValidVoiceTokenSlots([defaults objectForKey:@"MSIMEClientVoiceASRTokens"]) ?: @{};
    const BOOL serviceProvider = MSIMEVoiceASRProviderUsesService(rawProvider);
    NSString *sharedToken = serviceProvider
        ? MSIMEVoiceTokenForProvider(defaults, @"MSIMEClientVoiceASRTokens", rawProvider,
                                     [defaults stringForKey:@"MSIMEClientVoiceASRToken"])
        : @"";
    value.token = sharedToken.length ? sharedToken
        : (serviceProvider ? ReadToken(@"asr", rawProvider, value.endpoint) : @"");
    if (serviceProvider && value.token.length) {
        NSMutableDictionary *slots = [value.tokenSlots mutableCopy];
        slots[rawProvider] = value.token;
        value.tokenSlots = slots;
    }
    if (PolishProviderIsLocal(polishProvider))
    {
        value.polishToken = @"";
    }
    else
    {
        NSString *sharedPolishToken = MSIMEVoiceTokenForProvider(
            defaults, @"MSIMEClientVoicePolishTokens", polishProvider,
            [defaults stringForKey:@"MSIMEClientVoicePolishToken"]);
        value.polishToken = sharedPolishToken.length
            ? sharedPolishToken : ReadToken(@"polish", polishProvider, value.polishEndpoint);
    }
    return value;
}
- (BOOL)validate:(NSError **)error
{
    NSString *message = nil, *scope = MSIMEVoiceProviderRecognitionScope;
    if ([self.provider isEqualToString:@"local"])
    {
        // An installed model directory, which the model installer marks complete by writing msime-model.json last and which runs in the msime-voice-local helper.
        NSString *manifest = [self.modelPath stringByAppendingPathComponent:@"msime-model.json"];
        if (!IsRegularDirectory(self.modelPath) || !IsRegularFile(manifest))
            message = @"请选择已下载的本地语音模型目录。";
    }
    else if (![MSIMEVoiceASRProviderIDs() containsObject:self.provider])
        message = @"请选择识别方式。";
    else if ([self.provider isEqualToString:@"doubao"] &&
             (!IsWebSocketEndpoint(self.endpoint) || self.token.length == 0))
        message = @"请填写 WSS 识别地址和 API 密钥。";
    else if (![self.provider isEqualToString:@"doubao"] && MSIMEVoiceASRProviderUsesService(self.provider) &&
             (!IsEndpoint(self.endpoint) || self.model.length == 0 || self.token.length == 0))
        message = @"请填写 HTTPS 识别地址、模型名称和 API 密钥。";
    // 润色校验与 Rust `ai::endpoint` 策略对齐：放行 HTTPS 与本机/私网/Tailscale HTTP，拒绝公网明文 HTTP 及控制字符，
    // 并对 `local`/`ollama` 及本机/私网端点免除强制 API 密钥；仅在识别配置无误时检查，避免覆盖识别卡片错误。
    if (!message && self.polishEnabled)
    {
        NSString *polishProvider =
            [NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoicePolishProvider"]
                ?: MSIMEVoicePolishDefaultProvider;
        NSString *polishEndpoint = self.polishEndpoint ?: @"";
        const BOOL polishEndpointBlank = TrimAsciiSpaces(polishEndpoint).length == 0;
        NSString *polishModel = Trimmed(self.polishModel);
        NSString *polishToken = Trimmed(self.polishToken);
        if (PolishProviderIsLocal(polishProvider))
        {
            if (!polishEndpointBlank)
            {
                const MSIMEAIEndpointProblem problem = MSIMEAIEndpointCheck(polishEndpoint);
                if (problem == MSIMEAIEndpointProblemCleartextPublicHost)
                {
                    message = MSIMEAIEndpointCleartextMessage;
                    scope = MSIMEVoiceProviderPolishScope;
                }
                else if (problem != MSIMEAIEndpointProblemNone)
                {
                    message = @"请填写有效的整理服务地址（HTTPS，或本机、局域网的 HTTP）。";
                    scope = MSIMEVoiceProviderPolishScope;
                }
            }
        }
        else
        {
            const MSIMEAIEndpointProblem problem = MSIMEAIEndpointCheck(polishEndpoint);
            const BOOL needsToken = PolishRequiresToken(polishProvider, polishEndpoint);
            const BOOL pendingTextStreamToken =
                [self.provider isEqualToString:@"textstream"] && polishToken.length == 0 && needsToken;
            if (problem == MSIMEAIEndpointProblemCleartextPublicHost)
            {
                message = MSIMEAIEndpointCleartextMessage;
                scope = MSIMEVoiceProviderPolishScope;
            }
            else if (!pendingTextStreamToken &&
                     (problem != MSIMEAIEndpointProblemNone || polishModel.length == 0 ||
                      (needsToken && polishToken.length == 0)))
            {
                message = needsToken
                    ? @"启用文本整理需要 HTTPS（或本机/局域网 HTTP）服务地址、模型名称和 API 密钥。"
                    : @"启用本地文本整理需要有效的本机或局域网服务地址与模型名称（API 密钥可留空）。";
                scope = MSIMEVoiceProviderPolishScope;
            }
        }
    }
    if (message)
    {
        if (error)
            *error = Error(message, scope);
        return NO;
    }
    return YES;
}
- (BOOL)save:(NSError **)error
{
    if (![self validate:error])
        return NO;
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    NSString *previousProvider = [defaults stringForKey:@"MSIMEClientVoiceASRProvider"] ?: @"";
    NSString *previousEndpoint = [defaults stringForKey:@"MSIMEClientVoiceASREndpoint"] ?: @"";
    NSString *polishProvider = [defaults stringForKey:@"MSIMEClientVoicePolishProvider"] ?: MSIMEVoicePolishDefaultProvider;
    NSString *previousPolishEndpoint = [defaults stringForKey:@"MSIMEClientVoicePolishEndpoint"] ?: @"";
    const BOOL serviceProvider = MSIMEVoiceASRProviderUsesService(self.provider);
    const BOOL managedLocalPolish = PolishProviderIsLocal(polishProvider);
    const BOOL changedProvider = previousProvider.length &&
        ![previousProvider.lowercaseString isEqual:self.provider.lowercaseString];
    NSString *previousToken = [self.tokenSlots[previousProvider] isKindOfClass:NSString.class]
        ? self.tokenSlots[previousProvider] : @"";
    const BOOL migratePrevious = changedProvider && MSIMEVoiceASRProviderUsesService(previousProvider) &&
        previousEndpoint.length && previousToken.length;
    NSString *polishTokenToStore = managedLocalPolish ? @"" : (self.polishToken ?: @"");
    const BOOL shouldWritePolishKeychain = !managedLocalPolish || previousPolishEndpoint.length > 0;
    if ((migratePrevious && !WriteToken(@"asr", previousProvider, previousEndpoint, previousToken, error)) ||
        (serviceProvider && !WriteToken(@"asr", self.provider, self.endpoint, self.token, error)) ||
        (shouldWritePolishKeychain && !WriteToken(@"polish", polishProvider, self.polishEndpoint, polishTokenToStore, error)))
        return NO;
    // An endpoint edit for the same provider retires that provider's old
    // credential. Switching providers preserves the provider being left.
    if (MSIMEVoiceProviderShouldDeletePreviousCredential(previousProvider, previousEndpoint,
                                                         self.provider, self.endpoint))
        DeleteToken(@"asr", previousProvider, previousEndpoint);
    if (previousPolishEndpoint.length &&
        ![MSIMEVoiceProviderCredentialAccount(@"polish", polishProvider, previousPolishEndpoint)
            isEqual:MSIMEVoiceProviderCredentialAccount(@"polish", polishProvider, self.polishEndpoint)])
        DeleteToken(@"polish", polishProvider, previousPolishEndpoint);
    [defaults setObject:@{
        @"provider" : self.provider,
        @"endpoint" : self.endpoint,
        @"model" : self.model,
        @"modelPath" : self.modelPath,
        @"polishEnabled" : @(self.polishEnabled),
        @"polishEndpoint" : self.polishEndpoint,
        @"polishModel" : self.polishModel,
        @"captureDevice" : self.captureDevice ?: @""
    }
                                              forKey:@"voiceInput"];
    // What the input method actually reads on the next recording. Driven by the shared key table so a
    // field added to this window cannot be saved into the private dictionary alone.
    NSMutableDictionary *values = [@{
        @"provider" : self.provider,
        @"endpoint" : self.endpoint,
        @"model" : self.model,
        @"modelPath" : self.modelPath,
        @"token" : self.token,
        @"polishEndpoint" : self.polishEndpoint,
        @"polishModel" : self.polishModel,
        @"polishToken" : polishTokenToStore,
        @"polishPromptID" : self.polishPromptID ?: MSIMEPolishPromptIdentifiers().firstObject,
        @"polishPromptCustom1" : self.polishPromptCustom1 ?: @"",
        @"polishPromptCustom2" : self.polishPromptCustom2 ?: @"",
        @"polishPromptCustom3" : self.polishPromptCustom3 ?: @"",
        @"captureDevice" : self.captureDevice ?: @""
    } mutableCopy];
    BOOL writeCaptureDevice = YES;
    NSString *captureBackend = MSIMEVoiceProviderCaptureBackendForSaving(
        [defaults objectForKey:@"MSIMEClientVoiceCaptureBackend"], self.captureDevice, &writeCaptureDevice);
    if (captureBackend)
        [defaults setObject:captureBackend forKey:@"MSIMEClientVoiceCaptureBackend"];
    if (!writeCaptureDevice)
        [values removeObjectForKey:@"captureDevice"];
    NSDictionary<NSString *, NSString *> *sharedKeys = MSIMEVoiceProviderSharedKeys();
    for (NSString *field in values)
        [defaults setObject:values[field] forKey:sharedKeys[field]];
    [defaults setObject:MSIMEVoiceProviderTokenSlotsByUpdating(
                            self.tokenSlots ?: [defaults objectForKey:@"MSIMEClientVoiceASRTokens"], self.provider,
                            self.token, serviceProvider)
                 forKey:@"MSIMEClientVoiceASRTokens"];
    [defaults setObject:MSIMEVoiceProviderTokenSlotsByUpdating(
                            [defaults objectForKey:@"MSIMEClientVoicePolishTokens"], polishProvider,
                            polishTokenToStore, !managedLocalPolish)
                 forKey:@"MSIMEClientVoicePolishTokens"];
    [defaults setBool:self.polishEnabled forKey:@"MSIMEClientVoicePolish"];
    [defaults setBool:self.polishEnabled forKey:@"MSIMEClientVoicePolishText"];
    [[NSNotificationCenter defaultCenter] postNotificationName:MSIMEVoiceProviderSettingsDidChangeNotification object:self];
    return YES;
}
@end

// The form is a view rather than a window's contents: the same controls are the 语音输入 page of
// the settings window and the contents of the standalone window the input method's toolbar opens.
// Upstream laid it out in absolute coordinates inside a fixed 610x580 window, which is why it could
// only ever be a window.
@interface MetasequoiaVoiceProviderSettingsView () <NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate>
@end
@implementation MetasequoiaVoiceProviderSettingsView
{
    NSPopUpButton *_provider;
    NSPopUpButton *_captureDevice;
    NSTextField *_endpoint, *_model, *_modelPath, *_polishEndpoint, *_polishModel, *_polishPrompt;
    NSSecureTextField *_token, *_polishToken;
    NSPopUpButton *_polishPromptID;
    NSSwitch *_polish;
    /// The rows the recognition card shows only for the providers they belong to. A local model has no endpoint, no model name and no key, and a row that is present but grey still takes its height: 本地 Whisper left three dead fields occupying half the card above the one field it does use.
    NSView *_endpointRow, *_modelRow, *_tokenRow, *_modelPathRow, *_textStreamRow;
    NSTextField *_textStreamStatus, *_polishKeyStatus;
    NSButton *_textStreamInstall, *_openLessImport;
    NSButton *_fetchPolishModels;
    NSArray<NSString *> *_availablePolishModels;
    /// One message per card, on the card that owns the field it is about.
    NSTextField *_recognitionNotice, *_polishNotice;
    NSString *_loadedProvider;
    /// The endpoint each key on screen was entered for. A key is stored against the origin it was issued by, so moving the field to another origin leaves it behind, and that is when it has to go.
    NSString *_tokenEndpoint, *_polishTokenEndpoint;
    NSMutableDictionary<NSString *, NSString *> *_tokenDrafts;
    /// The three custom presets' wording, the preset the prompt field's text belongs to, and the override stored against a built-in preset. The field shows one preset at a time and the other two must survive being switched away from, exactly as the per-provider keys do in _tokenDrafts.
    NSMutableDictionary<NSString *, NSString *> *_polishPromptDrafts;
    NSString *_polishPromptSelection;
    /// Set while the controls are being filled from storage, so populating them saves nothing.
    BOOL _loading;
    NSPopUpButton *_hotkeyMode;
    NSTextField *_hotkeyLabel, *_hotkeyNotice, *_builtinHotkeyStatus, *_builtinHotkeyHelp;
    NSStackView *_hotkeyCaps;
    NSButton *_recordHotkey, *_clearHotkey;
    NSArray<NSSwitch *> *_builtinHotkeySwitches;
    id _hotkeyMonitor, _hotkeyResignObserver;
    NSDictionary *_pendingModifier;
    NSTextField *_permissionStatus;
    NSButton *_grantPermissions;
    id _permissionKeyObserver;
    // 个人语音词表管理控件与内存快照（对应 <preferences_directory>/voice-vocabulary.json）。
    NSTableView *_vocabTable;
    NSTextField *_vocabCanonicalField, *_vocabAliasesField, *_vocabNotice;
    NSButton *_vocabAddButton, *_vocabRemoveButton;
    NSArray<NSDictionary<NSString *, id> *> *_vocabTerms;
    id _vocabChangeObserver;
}

/// A message on the card it belongs to, or nothing at all. The window has no separate error style — MSIMEDetailLabel is the one quieter line under a row, and what distinguishes a failure from an aside is its colour.
static void ShowNotice(NSTextField *label, NSString *message, NSColor *color)
{
    label.stringValue = message ?: @"";
    label.textColor = color;
    label.hidden = message.length == 0;
}

- (NSTextField *)fieldLabelled:(NSString *)label secure:(BOOL)secure
{
    NSTextField *field = secure ? [NSSecureTextField new] : [NSTextField new];
    field.accessibilityLabel = label;
    field.delegate = self;
    field.translatesAutoresizingMaskIntoConstraints = NO;
    return field;
}

- (instancetype)initWithFrame:(NSRect)frameRect
{
    self = [super initWithFrame:frameRect];
    if (self == nil)
    {
        return nil;
    }
    self.translatesAutoresizingMaskIntoConstraints = NO;
    _tokenDrafts = [NSMutableDictionary dictionary];

    _hotkeyLabel = [NSTextField labelWithString:@"未设置"];
    _hotkeyLabel.lineBreakMode = NSLineBreakByTruncatingMiddle;
    [_hotkeyLabel setContentCompressionResistancePriority:NSLayoutPriorityDefaultHigh forOrientation:NSLayoutConstraintOrientationHorizontal];
    _hotkeyLabel.accessibilityLabel = @"当前自定义语音快捷键";
    _recordHotkey = [NSButton buttonWithTitle:@"录制快捷键…" target:self action:@selector(recordHotkey:)];
    _clearHotkey = [NSButton buttonWithTitle:@"清除" target:self action:@selector(clearHotkey:)];
    _hotkeyCaps = [NSStackView stackViewWithViews:@[]];
    NSStackView *hotkeyRow = [NSStackView stackViewWithViews:MSIMEEditionIsShurufa()
        ? @[_hotkeyCaps, _recordHotkey] : @[_hotkeyLabel, _recordHotkey, _clearHotkey]];
    hotkeyRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    hotkeyRow.spacing = 8;
    _hotkeyMode = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:NO];
    [_hotkeyMode addItemsWithTitles:MSIMEEditionIsShurufa() ? @[@"按住说话", @"切换"] : @[@"原有行为（默认）", @"按住说话", @"切换"]];
    _hotkeyMode.accessibilityLabel = @"语音快捷键触发模式";
    _hotkeyMode.target = self; _hotkeyMode.action = @selector(hotkeyModeChanged:);
    _hotkeyNotice = MSIMEDetailLabel(@"");
    _hotkeyNotice.hidden = YES;
    NSMutableArray *switches = [NSMutableArray array];
    NSMutableArray *shortcutRows = [NSMutableArray arrayWithArray:@[
        MSIMEPreferenceRow(MSIMEEditionIsShurufa() ? @"快捷键" : @"自定义快捷键", hotkeyRow),
        MSIMEPreferenceRow(MSIMEEditionIsShurufa() ? @"触发方式" : @"触发模式", _hotkeyMode)]];
    for (NSString *title in @[@"Control+F9", @"右 Option", @"Control+Command", @"右 Control+右 Option"]) {
        NSSwitch *control = MSIMESettingSwitch(self, @selector(builtinHotkeyChanged:), title);
        control.tag = switches.count;
        [switches addObject:control];
        if (!MSIMEEditionIsShurufa()) [shortcutRows addObject:MSIMESwitchRow(title, control, @"")];
    }
    _builtinHotkeySwitches = switches;
    _builtinHotkeyStatus = MSIMEDetailLabel(@"已由自定义快捷键替代；清除自定义后恢复内置选择。");
    _builtinHotkeyHelp = MSIMEDetailLabel(@"自定义快捷键会替代内置快捷键，清除后恢复内置选择。");
    if (!MSIMEEditionIsShurufa()) {
    [shortcutRows addObject:_builtinHotkeyStatus];
    [shortcutRows addObject:_builtinHotkeyHelp];
    [shortcutRows addObject:MSIMEDetailLabel([NSString stringWithFormat:@"快捷键在 %@ 是当前输入法、光标在文本框里时生效。", MSIMEEditionDisplayName()])];
    [shortcutRows addObject:MSIMEDetailLabel(@"原有行为（默认）：右 Option 和修饰键组合按住说话；Control+F9 按一下开始、再按一下结束。自定义单个修饰键按住说话，自定义组合键切换。")];
    [shortcutRows addObject:MSIMEDetailLabel(@"按住说话：所有快捷键按住开始、松开结束。切换：按一下开始、再按一下结束；单个修饰键在松开时触发。录音中按 Esc 取消，切换应用会取消录音。")];
    }
    _permissionStatus = [NSTextField labelWithString:@""];
    _permissionStatus.accessibilityLabel = @"组合键快捷键所需的系统权限";
    _grantPermissions = [NSButton buttonWithTitle:@"去授权…" target:self action:@selector(grantShortcutPermissions:)];
    NSStackView *permissionRow = [NSStackView stackViewWithViews:@[_permissionStatus, _grantPermissions]];
    permissionRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    permissionRow.spacing = 8;
    [shortcutRows addObject:MSIMEPreferenceRow(@"系统权限", permissionRow)];
    [shortcutRows addObject:MSIMEDetailLabel(@"组合键快捷键（如右 Option+←）要在系统层拦住按键，应用才不会先执行它自己的动作（比如按词移动光标），这需要「辅助功能」和「输入监控」两项授权。单个修饰键快捷键不需要。")];
    [shortcutRows addObject:_hotkeyNotice];
    NSBox *shortcutCard = MSIMECardWithViews(shortcutRows, 0);

    _provider = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:NO];
    _provider.accessibilityLabel = @"识别方式";
    [_provider addItemsWithTitles:MSIMEVoiceASRProviderTitles()];
    _provider.target = self;
    _provider.action = @selector(providerChanged:);
    _endpoint = [self fieldLabelled:@"识别服务地址" secure:NO];
    _model = [self fieldLabelled:@"识别模型" secure:NO];
    _token = (NSSecureTextField *)[self fieldLabelled:@"API 密钥" secure:YES];
    _modelPath = [self fieldLabelled:@"本地模型" secure:NO];
    NSButton *browse = [NSButton buttonWithTitle:@"选择…" target:self action:@selector(browse:)];
    NSStackView *modelPathRow = [NSStackView stackViewWithViews:@[ _modelPath, browse ]];
    modelPathRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    modelPathRow.spacing = 8.0;
    _captureDevice = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:NO];
    _captureDevice.accessibilityLabel = @"录音设备";
    _captureDevice.target = self;
    _captureDevice.action = @selector(commit:);
    _polish = MSIMESettingSwitch(self, @selector(polishChanged:), @"识别后整理文本");
    _polishEndpoint = [self fieldLabelled:@"整理服务地址" secure:NO];
    _polishEndpoint.placeholderString = @"https://... 或本机/局域网 http://...";
    _polishModel = [self fieldLabelled:@"整理模型" secure:NO];
    _fetchPolishModels = [NSButton buttonWithTitle:@"获取模型列表" target:self action:@selector(fetchPolishModels:)];
    _fetchPolishModels.accessibilityLabel = @"获取整理模型列表";
    NSStackView *polishModelRow = [NSStackView stackViewWithViews:@[_polishModel, _fetchPolishModels]];
    polishModelRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    polishModelRow.spacing = 8;
    [polishModelRow.widthAnchor constraintGreaterThanOrEqualToConstant:320].active = YES;
    [_polishModel setContentCompressionResistancePriority:NSLayoutPriorityDefaultLow forOrientation:NSLayoutConstraintOrientationHorizontal];
    [_fetchPolishModels setContentCompressionResistancePriority:NSLayoutPriorityRequired forOrientation:NSLayoutConstraintOrientationHorizontal];
    _polishToken = (NSSecureTextField *)[self fieldLabelled:@"整理 API 密钥" secure:YES];
    _polishPromptID = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:NO];
    _polishPromptID.accessibilityLabel = @"整理方案";
    [_polishPromptID addItemsWithTitles:MSIMEPolishPromptTitles()];
    _polishPromptID.target = self;
    _polishPromptID.action = @selector(polishPromptIDChanged:);
    _polishPrompt = [self fieldLabelled:@"整理提示词" secure:NO];

    _textStreamStatus = MSIMEDetailLabel(@"");
    _textStreamStatus.accessibilityLabel = @"语音模型状态";
    _textStreamInstall = [NSButton buttonWithTitle:@"安装语音模型" target:self action:@selector(installTextStream:)];
    NSStackView *textStreamControls = [NSStackView stackViewWithViews:@[ _textStreamStatus, _textStreamInstall ]];
    textStreamControls.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    textStreamControls.spacing = 8.0;
    _polishKeyStatus = MSIMEDetailLabel(@"");
    _polishKeyStatus.accessibilityLabel = @"整理密钥状态";
    _openLessImport = [NSButton buttonWithTitle:@"从 OpenLess 导入" target:self action:@selector(importFromOpenLess:)];
    NSStackView *importControls = [NSStackView stackViewWithViews:@[ _polishKeyStatus, _openLessImport ]];
    importControls.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    importControls.spacing = 8.0;

    _recognitionNotice = MSIMEDetailLabel(@"");
    _recognitionNotice.accessibilityLabel = @"语音设置状态";
    _recognitionNotice.hidden = YES;
    _polishNotice = MSIMEDetailLabel(@"");
    _polishNotice.accessibilityLabel = @"文本整理状态";
    _polishNotice.hidden = YES;

    _endpointRow = MSIMEPreferenceRow(@"服务地址", _endpoint);
    _modelRow = MSIMEPreferenceRow(@"识别模型", _model);
    _tokenRow = MSIMEPreferenceRow(@"API 密钥", _token);
    _modelPathRow = MSIMEPreferenceRow(@"本地模型", modelPathRow);
    _textStreamRow = MSIMEPreferenceRow(@"语音模型", textStreamControls);
    NSBox *recognitionCard = MSIMECardWithViews(@[
        MSIMEPreferenceRow(@"识别方式", _provider),
        _endpointRow,
        _modelRow,
        _tokenRow,
        _modelPathRow,
        _textStreamRow,
        MSIMEPreferenceRow(@"录音设备", _captureDevice),
        _recognitionNotice,
    ], 0.0);
    recognitionCard.accessibilityLabel = @"语音识别卡片";
    NSBox *polishCard = MSIMECardWithViews(@[
        MSIMESwitchRow(@"识别后整理文本", _polish, @"会把本次转写的文本发送到下面的服务"),
        MSIMEPreferenceRow(@"服务地址", _polishEndpoint),
        MSIMEPreferenceRow(@"整理模型", polishModelRow),
        MSIMEPreferenceRow(@"API 密钥", _polishToken),
        MSIMEPreferenceRow(@"整理方案", _polishPromptID),
        MSIMEPreferenceRowWithDetail(@"整理提示词", @"留空使用所选方案自带的提示词；选中自定义方案时，这里写的内容保存在该方案里。",
                                     _polishPrompt),
        MSIMEPreferenceRowWithDetail(@"从 OpenLess 导入", @"读取 OpenLess 的 Ark 接口、模型、密钥和当前整理风格的提示词；会弹出系统钥匙串授权框，点“允许”。",
                                     importControls),
        _polishNotice,
    ], 0.0);
    polishCard.accessibilityLabel = @"文本整理卡片";

    _vocabTerms = @[];
    _vocabTable = [[NSTableView alloc] initWithFrame:NSZeroRect];
    _vocabTable.dataSource = self;
    _vocabTable.delegate = self;
    _vocabTable.usesAlternatingRowBackgroundColors = YES;
    _vocabTable.allowsEmptySelection = YES;
    _vocabTable.allowsMultipleSelection = NO;
    _vocabTable.rowHeight = 22.0;
    _vocabTable.accessibilityLabel = @"个人语音词表列表";
    NSTableColumn *canonicalCol = [[NSTableColumn alloc] initWithIdentifier:@"canonical"];
    canonicalCol.title = @"标准写法";
    canonicalCol.width = 160.0;
    canonicalCol.minWidth = 90.0;
    NSTableColumn *aliasesCol = [[NSTableColumn alloc] initWithIdentifier:@"aliases"];
    aliasesCol.title = @"误识别别名";
    aliasesCol.width = 230.0;
    aliasesCol.minWidth = 100.0;
    NSTableColumn *sourceCol = [[NSTableColumn alloc] initWithIdentifier:@"source"];
    sourceCol.title = @"来源";
    sourceCol.width = 76.0;
    sourceCol.minWidth = 60.0;
    [_vocabTable addTableColumn:canonicalCol];
    [_vocabTable addTableColumn:aliasesCol];
    [_vocabTable addTableColumn:sourceCol];

    NSScrollView *vocabScroll = [[NSScrollView alloc] initWithFrame:NSZeroRect];
    vocabScroll.documentView = _vocabTable;
    vocabScroll.hasVerticalScroller = YES;
    vocabScroll.autohidesScrollers = YES;
    vocabScroll.borderType = NSBezelBorder;
    vocabScroll.translatesAutoresizingMaskIntoConstraints = NO;
    [vocabScroll.heightAnchor constraintEqualToConstant:136.0].active = YES;

    _vocabCanonicalField = [NSTextField textFieldWithString:@""];
    _vocabCanonicalField.placeholderString = @"标准写法，如 T3 Code";
    _vocabCanonicalField.accessibilityLabel = @"新增词条标准写法";
    _vocabCanonicalField.translatesAutoresizingMaskIntoConstraints = NO;
    _vocabAliasesField = [NSTextField textFieldWithString:@""];
    _vocabAliasesField.placeholderString = @"可选误识别别名（逗号分隔）";
    _vocabAliasesField.accessibilityLabel = @"新增词条误识别别名";
    _vocabAliasesField.translatesAutoresizingMaskIntoConstraints = NO;
    _vocabAddButton = [NSButton buttonWithTitle:@"添加" target:self action:@selector(addVocabularyTerm:)];
    _vocabRemoveButton = [NSButton buttonWithTitle:@"删除" target:self action:@selector(removeSelectedVocabularyTerm:)];
    _vocabRemoveButton.enabled = NO;
    [_vocabAddButton setContentCompressionResistancePriority:NSLayoutPriorityRequired forOrientation:NSLayoutConstraintOrientationHorizontal];
    [_vocabRemoveButton setContentCompressionResistancePriority:NSLayoutPriorityRequired forOrientation:NSLayoutConstraintOrientationHorizontal];

    NSStackView *vocabEditorRow = [NSStackView stackViewWithViews:@[
        _vocabCanonicalField, _vocabAliasesField, _vocabAddButton, _vocabRemoveButton
    ]];
    vocabEditorRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    vocabEditorRow.spacing = 8.0;
    vocabEditorRow.translatesAutoresizingMaskIntoConstraints = NO;
    [_vocabCanonicalField.widthAnchor constraintGreaterThanOrEqualToConstant:140.0].active = YES;
    [_vocabAliasesField.widthAnchor constraintGreaterThanOrEqualToConstant:160.0].active = YES;

    _vocabNotice = MSIMEDetailLabel(@"");
    _vocabNotice.hidden = YES;
    NSBox *vocabularyCard = MSIMECardWithViews(@[
        vocabScroll,
        vocabEditorRow,
        MSIMEDetailLabel(@"语音上屏后 15 秒内手动改错并发送，会自动把纠错词学进这里；同时也自动融合你的输入法用户词库。"),
        _vocabNotice,
    ], 0.0);
    vocabularyCard.accessibilityLabel = @"个人语音词表卡片";

    NSTextField *hint = [NSTextField
        wrappingLabelWithString:@"云端识别会发送本次录音；本地识别使用所选模型。密钥保存在系统钥匙串中。"
                                @"本地流式识别在本机运行，不上传录音；整理支持云端 HTTPS 以及本机/局域网 HTTP 服务，失败或超时时直接输入原文。"];
    hint.font = [NSFont systemFontOfSize:11.0];
    hint.textColor = NSColor.secondaryLabelColor;

    NSStackView *stack = [NSStackView stackViewWithViews:@[
        MSIMESectionLabel(@"语音快捷键"), shortcutCard,
        MSIMESectionLabel(@"语音识别"), recognitionCard,
        MSIMESectionLabel(@"文本整理"), polishCard,
        MSIMESectionLabel(@"个人语音词表"), vocabularyCard,
        hint,
    ]];
    stack.orientation = NSUserInterfaceLayoutOrientationVertical;
    stack.alignment = NSLayoutAttributeLeading;
    stack.distribution = NSStackViewDistributionFill;
    stack.spacing = 10.0;
    stack.translatesAutoresizingMaskIntoConstraints = NO;
    [self addSubview:stack];
    for (NSView *view in stack.arrangedSubviews)
    {
        [view.widthAnchor constraintEqualToAnchor:stack.widthAnchor].active = YES;
    }
    [NSLayoutConstraint activateConstraints:@[
        [stack.leadingAnchor constraintEqualToAnchor:self.leadingAnchor],
        [stack.trailingAnchor constraintEqualToAnchor:self.trailingAnchor],
        [stack.topAnchor constraintEqualToAnchor:self.topAnchor],
        [stack.bottomAnchor constraintEqualToAnchor:self.bottomAnchor],
    ]];
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    _vocabChangeObserver = [NSNotificationCenter.defaultCenter
        addObserverForName:@"MSIMEVoiceVocabularyDidChangeNotification"
                    object:nil
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification * _Nonnull note) {
                    (void)note;
                    [weakSelf reloadVocabularyTerms];
                }];
    [self reloadSettings];
    return self;
}

- (void)reloadSettings
{
    _loading = YES;
    [self reloadHotkeyControls];
    MetasequoiaVoiceProviderSettings *value = [MetasequoiaVoiceProviderSettings loadSettings];
    NSArray *providerIDs = MSIMEVoiceASRProviderIDs();
    NSUInteger providerIndex = [providerIDs indexOfObject:value.provider];
    [_provider selectItemAtIndex:providerIndex == NSNotFound ? 0 : providerIndex];
    _endpoint.stringValue = value.endpoint;
    _model.stringValue = value.model;
    _token.stringValue = value.token;
    _loadedProvider = value.provider;
    _tokenEndpoint = [value.endpoint copy];
    _polishTokenEndpoint = [value.polishEndpoint copy];
    _tokenDrafts = [value.tokenSlots mutableCopy] ?: [NSMutableDictionary dictionary];
    if (MSIMEVoiceASRProviderUsesService(_loadedProvider))
    {
        _tokenDrafts[_loadedProvider] = value.token ?: @"";
    }
    _modelPath.stringValue = value.modelPath;
    [_captureDevice removeAllItems];
    NSMenuItem *automatic = [[NSMenuItem alloc] initWithTitle:@"系统默认" action:nil keyEquivalent:@""];
    automatic.representedObject = @"";
    [_captureDevice.menu addItem:automatic];
    BOOL foundCaptureDevice = value.captureDevice.length == 0;
    for (NSDictionary *device in MSIMEListVoiceCaptureDevices())
    {
        NSString *uid = device[@"uid"], *name = device[@"name"];
        NSString *title =
            [device[@"default"] boolValue] ? [NSString stringWithFormat:@"%@（当前系统默认）", name] : name;
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:nil keyEquivalent:@""];
        item.representedObject = uid;
        [_captureDevice.menu addItem:item];
        if ([uid isEqual:value.captureDevice])
        {
            [_captureDevice selectItem:item];
            foundCaptureDevice = YES;
        }
    }
    if (!foundCaptureDevice)
    {
        NSMenuItem *unavailable =
            [[NSMenuItem alloc] initWithTitle:@"已保存的录音设备（当前不可用）" action:nil keyEquivalent:@""];
        unavailable.representedObject = value.captureDevice;
        [_captureDevice.menu addItem:unavailable];
        [_captureDevice selectItem:unavailable];
    }
    else if (value.captureDevice.length == 0)
    {
        [_captureDevice selectItem:automatic];
    }
    _polish.state = value.polishEnabled ? NSControlStateValueOn : NSControlStateValueOff;
    _polishEndpoint.stringValue = value.polishEndpoint;
    _polishModel.stringValue = value.polishModel;
    _polishToken.stringValue = value.polishToken;
    [_polishPromptID selectItemAtIndex:(NSInteger)[MSIMEPolishPromptIdentifiers() indexOfObject:value.polishPromptID]];
    _polishPromptDrafts = [@{
        @"custom_1" : value.polishPromptCustom1 ?: @"",
        @"custom_2" : value.polishPromptCustom2 ?: @"",
        @"custom_3" : value.polishPromptCustom3 ?: @""
    } mutableCopy];
    _polishPromptSelection = [value.polishPromptID copy];
    _polishPrompt.stringValue = [self polishPromptForPreset:value.polishPromptID];
    [self updateEnabled:nil];
    [self reloadVocabularyTerms];
    ShowNotice(_recognitionNotice, nil, NSColor.systemRedColor);
    ShowNotice(_polishNotice, nil, NSColor.systemRedColor);
    _loading = NO;
}

- (NSArray<NSString *> *)builtinHotkeyKeys {
    return @[@"hotkey_ctrl_f9", @"hotkey_ralt", @"hotkey_ctrl_win", @"hotkey_rctrl_ralt"];
}
- (void)reloadHotkeyControls {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    NSDictionary *custom = MSIMEVoiceCustomShortcut(defaults);
    _hotkeyLabel.stringValue = MSIMEVoiceShortcutTitle(custom);
    _hotkeyLabel.toolTip = _hotkeyLabel.stringValue;
    for (NSView *view in _hotkeyCaps.arrangedSubviews.copy) { [_hotkeyCaps removeArrangedSubview:view]; [view removeFromSuperview]; }
    [_hotkeyCaps addArrangedSubview:MSIMEVoiceShortcutKeycaps(custom)];
    _clearHotkey.enabled = custom != nil;
    _builtinHotkeyStatus.hidden = custom == nil;
    _builtinHotkeyHelp.hidden = custom != nil;
    NSArray *modes = MSIMEEditionIsShurufa() ? @[@"hold",@"toggle"] : @[@"legacy",@"hold",@"toggle"];
    [_hotkeyMode selectItemAtIndex:[modes indexOfObject:MSIMEVoiceHotkeyMode(defaults)]];
    NSArray *keys = @[@"MSIMEClientVoiceHotkeyCtrlF9", @"MSIMEClientVoiceHotkeyRightAlt",
        @"MSIMEClientVoiceHotkeyCtrlCommand", @"MSIMEClientVoiceHotkeyCtrlOption"];
    for (NSUInteger index = 0; index < keys.count; ++index) {
        id value = [defaults objectForKey:keys[index]];
        _builtinHotkeySwitches[index].state = value ? [value boolValue] : index < 2;
        _builtinHotkeySwitches[index].enabled = custom == nil;
    }
    [self reloadPermissionControls];
}
- (void)reloadPermissionControls {
    const auto permissions = MSIMECurrentVoiceShortcutPermissions();
    _permissionStatus.stringValue = [NSString stringWithFormat:@"辅助功能%@ · 输入监控%@",
        permissions.accessibility ? @"已授权" : @"未授权", permissions.inputMonitoring ? @"已授权" : @"未授权"];
    // 只有主键不是修饰键的绑定才需要系统层吞键，缺授权时才标红。
    NSDictionary *custom = MSIMEVoiceCustomShortcut(NSUserDefaults.standardUserDefaults);
    const BOOL needed = custom && !MSIMEVoiceModifierKey([custom[@"key_code"] unsignedShortValue]);
    _permissionStatus.textColor = needed && !permissions.granted() ? NSColor.systemRedColor : NSColor.secondaryLabelColor;
    _grantPermissions.hidden = permissions.granted();
}
- (void)grantShortcutPermissions:(id)sender {
    (void)sender;
    const auto permissions = MSIMECurrentVoiceShortcutPermissions();
    MSIMERequestVoiceShortcutPermissions(permissions);
    if (NSURL *pane = MSIMEVoiceShortcutPermissionsPane(permissions)) [NSWorkspace.sharedWorkspace openURL:pane];
}
// 用户去系统设置打开授权后回到这个窗口时重读状态。
- (void)viewDidMoveToWindow {
    [super viewDidMoveToWindow];
    if (_permissionKeyObserver) [NSNotificationCenter.defaultCenter removeObserver:_permissionKeyObserver];
    _permissionKeyObserver = nil;
    if (!self.window) return;
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    _permissionKeyObserver = [NSNotificationCenter.defaultCenter addObserverForName:NSWindowDidBecomeKeyNotification
        object:self.window queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *) { [weakSelf reloadPermissionControls]; }];
    [self reloadPermissionControls];
}
- (BOOL)saveHotkeyPatch:(NSDictionary *)patch {
    NSString *directory = MSIMELoadRuntimeOptions()[@"preferences_directory"];
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) {
        ShowNotice(_hotkeyNotice, @"无法找到输入法偏好目录，请重新打开已安装的 Shurufa。", NSColor.systemRedColor);
        return NO;
    }
    NSDictionary *snapshot = [MSIMEClientSession loadPreferencesInDirectory:directory error:nil];
    NSMutableDictionary *preferences = [snapshot[@"preferences"] mutableCopy];
    NSMutableDictionary *voice = [preferences[@"voice_input"] mutableCopy];
    if (!snapshot || !preferences || !voice) {
        ShowNotice(_hotkeyNotice, @"无法读取偏好，快捷键未更改，请稍后重试。", NSColor.systemRedColor);
        return NO;
    }
    [voice addEntriesFromDictionary:patch];
    preferences[@"voice_input"] = voice;
    NSMutableDictionary *updated = [snapshot mutableCopy];
    updated[@"preferences"] = preferences;
    if (![MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:[snapshot[@"revision"] unsignedLongLongValue]
        snapshot:updated error:nil]) {
        ShowNotice(_hotkeyNotice, @"快捷键未保存，偏好可能已变化，请重新打开设置后重试。", NSColor.systemRedColor);
        [self reloadHotkeyControls];
        return NO;
    }
    MSIMEApplySharedVoicePreferences(voice, NSUserDefaults.standardUserDefaults);
    [NSUserDefaults.standardUserDefaults synchronize];
    [[NSNotificationCenter defaultCenter] postNotificationName:MSIMEVoiceProviderSettingsDidChangeNotification object:self];
    [self reloadHotkeyControls];
    ShowNotice(_hotkeyNotice, @"已保存", NSColor.secondaryLabelColor);
    return YES;
}
- (void)hotkeyModeChanged:(id)sender {
    (void)sender;
    NSArray *modes = MSIMEEditionIsShurufa() ? @[@"hold",@"toggle"] : @[@"legacy",@"hold",@"toggle"];
    if (!_loading) [self saveHotkeyPatch:@{@"hotkey_mode":modes[_hotkeyMode.indexOfSelectedItem]}];
}
- (void)builtinHotkeyChanged:(NSSwitch *)sender {
    if (!_loading) [self saveHotkeyPatch:@{[self builtinHotkeyKeys][sender.tag]:@(sender.state == NSControlStateValueOn)}];
}
- (void)clearHotkey:(id)sender {
    (void)sender;
    [self stopRecordingHotkey];
    [self saveHotkeyPatch:@{@"macos_hotkey":NSNull.null}];
}
- (void)stopRecordingHotkey {
    if (_hotkeyMonitor) [NSEvent removeMonitor:_hotkeyMonitor];
    if (_hotkeyResignObserver) [NSNotificationCenter.defaultCenter removeObserver:_hotkeyResignObserver];
    _hotkeyMonitor = nil; _hotkeyResignObserver = nil; _pendingModifier = nil;
    _recordHotkey.title = @"录制快捷键…";
}
- (void)acceptRecordedHotkey:(NSDictionary *)binding {
    Class preferencesClass = NSClassFromString(@"MSIMEAppearancePreferences");
    NSString *conflict = MSIMEVoiceShortcutConflict(binding, [preferencesClass sharedPreferences]);
    if (conflict) { ShowNotice(_hotkeyNotice, conflict, NSColor.systemRedColor); _pendingModifier = nil; return; }
    [self stopRecordingHotkey];
    [self saveHotkeyPatch:@{@"macos_hotkey":binding}];
}
- (void)recordHotkey:(id)sender {
    (void)sender;
    if (_hotkeyMonitor) { [self stopRecordingHotkey]; return; }
    _recordHotkey.title = @"请按快捷键…";
    ShowNotice(_hotkeyNotice, @"按 Esc 取消录制。单个修饰键请按下后松开。", NSColor.secondaryLabelColor);
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    _hotkeyResignObserver = [NSNotificationCenter.defaultCenter addObserverForName:NSWindowDidResignKeyNotification
        object:self.window queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *) { [weakSelf stopRecordingHotkey]; }];
    _hotkeyMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:(NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged | NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown)
        handler:^NSEvent *(NSEvent *event) {
            MetasequoiaVoiceProviderSettingsView *view = weakSelf;
            if (!view || NSApp.keyWindow != view.window) return event;
            if (event.type == NSEventTypeLeftMouseDown || event.type == NSEventTypeRightMouseDown) {
                [view stopRecordingHotkey];
                return event;
            }
            if (event.keyCode == 53 && event.type == NSEventTypeKeyDown) {
                [view stopRecordingHotkey];
                ShowNotice(view->_hotkeyNotice, @"已取消录制；Esc 留作取消录音。", NSColor.secondaryLabelColor);
            } else if (event.keyCode == 57) {
                ShowNotice(view->_hotkeyNotice, @"Caps Lock 留作中英文切换，不能设为语音快捷键。", NSColor.systemRedColor);
                view->_pendingModifier = nil;
            } else if (event.type == NSEventTypeKeyDown && !event.isARepeat) {
                [view acceptRecordedHotkey:MSIMERecordedVoiceShortcut(event.keyCode, MSIMEVoiceModifierBits(event.modifierFlags), event.modifierFlags)];
            } else if (event.type == NSEventTypeFlagsChanged) {
                unsigned flags = MSIMEVoiceModifierBits(event.modifierFlags);
                unsigned own = MSIMEVoiceModifierBitForKey(event.keyCode);
                if (own && (flags & own)) view->_pendingModifier = MSIMERecordedVoiceShortcut(event.keyCode, flags & ~own, event.modifierFlags);
                else if (flags == 0 && view->_pendingModifier) [view acceptRecordedHotkey:view->_pendingModifier];
            }
            return nil;
        }];
}
- (void)focusShortcutSettings {
    [self.window.contentView layoutSubtreeIfNeeded];
    [_recordHotkey scrollRectToVisible:NSInsetRect(_recordHotkey.bounds, -12, -12)];
    [self.window makeFirstResponder:_recordHotkey];
}
- (void)fetchPolishModels:(id)sender {
    (void)sender;
    NSString *endpoint = [_polishEndpoint.stringValue stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    NSString *token = [_polishToken.stringValue stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    NSData *request = [NSJSONSerialization dataWithJSONObject:@{@"endpoint":endpoint, @"token":token, @"detailed_errors":@YES} options:0 error:nil];
    _fetchPolishModels.enabled = NO;
    ShowNotice(_polishNotice, @"正在获取模型列表…", NSColor.secondaryLabelColor);
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        auto raw = msime::host_api::own_string(msime_client_ai_models(static_cast<const uint8_t *>(request.bytes), request.length));
        NSData *data = raw ? [NSData dataWithBytes:raw.get() length:strlen(raw.get())] : nil;
        NSDictionary *reply = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
        dispatch_async(dispatch_get_main_queue(), ^{
            MetasequoiaVoiceProviderSettingsView *view = weakSelf;
            if (!view) return;
            view->_fetchPolishModels.enabled = YES;
            if (![endpoint isEqual:[view->_polishEndpoint.stringValue stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet]] ||
                ![token isEqual:[view->_polishToken.stringValue stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet]]) {
                ShowNotice(view->_polishNotice, @"服务配置已变化，请重新获取模型列表。", NSColor.secondaryLabelColor);
                return;
            }
            if (![reply[@"ok"] isEqual:@YES] || ![reply[@"value"] isKindOfClass:NSArray.class]) {
                NSDictionary *messages = @{
                    @"ai_invalid":@"服务地址或密钥格式不正确，请检查后重试。",
                    @"ai_models_unauthorized":@"密钥无效或没有读取模型列表的权限。",
                    @"ai_models_unsupported":@"该服务不支持获取模型列表，或服务地址路径不正确；请检查地址或手动填写模型。",
                    @"ai_models_timeout":@"请求超时，请检查网络或稍后重试。",
                    @"ai_models_invalid":@"服务返回的模型列表无法识别，请手动填写模型。"
                };
                ShowNotice(view->_polishNotice, messages[reply[@"error"] ?: @""] ?: @"无法连接模型列表接口，请检查服务地址和网络。", NSColor.systemRedColor);
                return;
            }
            view->_availablePolishModels = reply[@"value"];
            NSMenu *models = [[NSMenu alloc] initWithTitle:@"选择整理模型"];
            for (NSString *identifier in view->_availablePolishModels) {
                NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:identifier action:@selector(selectPolishModel:) keyEquivalent:@""];
                item.target = view; item.representedObject = identifier;
                [models addItem:item];
            }
            ShowNotice(view->_polishNotice, [NSString stringWithFormat:@"已获取 %lu 个模型，也可继续手动填写。", (unsigned long)models.numberOfItems], NSColor.secondaryLabelColor);
            if (view.window.isVisible) [models popUpMenuPositioningItem:nil atLocation:NSMakePoint(0, NSHeight(view->_fetchPolishModels.bounds)) inView:view->_fetchPolishModels];
        });
    });
}
- (void)selectPolishModel:(NSMenuItem *)sender {
    if (![sender.representedObject isKindOfClass:NSString.class]) return;
    _polishModel.stringValue = sender.representedObject;
    [self commit:nil];
}
- (void)viewWillMoveToWindow:(NSWindow *)newWindow {
    if (!newWindow) [self stopRecordingHotkey];
    [super viewWillMoveToWindow:newWindow];
}
- (void)dealloc {
    [self stopRecordingHotkey];
    if (_vocabChangeObserver) {
        [NSNotificationCenter.defaultCenter removeObserver:_vocabChangeObserver];
        _vocabChangeObserver = nil;
    }
}

- (NSString *)voiceVocabularyPreferencesDirectory {
    return MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path ?: @"";
}

- (id)callVoiceVocabularyManage:(NSDictionary<NSString *, id> *)payload errorMessage:(NSString * _Nullable * _Nullable)errorOut {
    if (errorOut) *errorOut = nil;
    if (![NSJSONSerialization isValidJSONObject:payload]) {
        if (errorOut) *errorOut = @"词表请求格式无效。";
        return nil;
    }
    NSData *body = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    if (!body.length) return nil;
    auto owned = msime::host_api::own_string(
        msime_client_voice_vocabulary_manage(static_cast<const uint8_t *>(body.bytes), body.length));
    if (!owned) {
        if (errorOut) *errorOut = @"词表服务不可用。";
        return nil;
    }
    NSData *raw = [NSData dataWithBytes:owned.get() length:std::strlen(owned.get())];
    NSDictionary *envelope = [NSJSONSerialization JSONObjectWithData:raw options:0 error:nil];
    if (![envelope isKindOfClass:NSDictionary.class] || ![envelope[@"ok"] isEqual:@YES]) {
        if (errorOut) *errorOut = @"词表读写失败，请检查输入格式。";
        return nil;
    }
    return envelope[@"value"];
}

- (void)reloadVocabularyTerms {
    NSString *dir = [self voiceVocabularyPreferencesDirectory];
    if (!dir.length) return;
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSString *errMsg = nil;
        id value = [weakSelf callVoiceVocabularyManage:@{
            @"action" : @"list",
            @"preferences_directory" : dir,
        } errorMessage:&errMsg];
        NSArray *terms = [value isKindOfClass:NSDictionary.class] && [value[@"terms"] isKindOfClass:NSArray.class]
            ? value[@"terms"] : @[];
        dispatch_async(dispatch_get_main_queue(), ^{
            MetasequoiaVoiceProviderSettingsView *strongSelf = weakSelf;
            if (!strongSelf) return;
            strongSelf->_vocabTerms = [terms copy];
            [strongSelf->_vocabTable reloadData];
            strongSelf->_vocabRemoveButton.enabled = strongSelf->_vocabTable.selectedRow >= 0 &&
                strongSelf->_vocabTable.selectedRow < static_cast<NSInteger>(strongSelf->_vocabTerms.count);
        });
    });
}

- (void)addVocabularyTerm:(id)sender {
    (void)sender;
    NSString *canonical = Trimmed(_vocabCanonicalField.stringValue);
    if (!canonical.length) {
        ShowNotice(_vocabNotice, @"请输入要添加的标准写法。", NSColor.systemRedColor);
        return;
    }
    NSString *rawAliases = _vocabAliasesField.stringValue ?: @"";
    NSCharacterSet *seps = [NSCharacterSet characterSetWithCharactersInString:@",，、;"];
    NSMutableArray<NSString *> *aliases = [NSMutableArray array];
    for (NSString *part in [rawAliases componentsSeparatedByCharactersInSet:seps]) {
        NSString *trimmed = Trimmed(part);
        if (trimmed.length && [trimmed caseInsensitiveCompare:canonical] != NSOrderedSame) {
            [aliases addObject:trimmed];
        }
    }
    NSString *dir = [self voiceVocabularyPreferencesDirectory];
    if (!dir.length) return;
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSString *errMsg = nil;
        id value = [weakSelf callVoiceVocabularyManage:@{
            @"action" : @"upsert",
            @"preferences_directory" : dir,
            @"term" : @{
                @"canonical" : canonical,
                @"aliases" : aliases,
                @"source" : @"manual",
                @"weight" : @1000,
            },
        } errorMessage:&errMsg];
        dispatch_async(dispatch_get_main_queue(), ^{
            MetasequoiaVoiceProviderSettingsView *strongSelf = weakSelf;
            if (!strongSelf) return;
            if (!value) {
                ShowNotice(strongSelf->_vocabNotice, errMsg ?: @"添加失败。", NSColor.systemRedColor);
                return;
            }
            strongSelf->_vocabCanonicalField.stringValue = @"";
            strongSelf->_vocabAliasesField.stringValue = @"";
            ShowNotice(strongSelf->_vocabNotice,
                       [NSString stringWithFormat:@"已保存词条「%@」", canonical],
                       NSColor.secondaryLabelColor);
            [strongSelf reloadVocabularyTerms];
            [NSNotificationCenter.defaultCenter postNotificationName:@"MSIMEVoiceVocabularyDidChangeNotification"
                                                              object:strongSelf];
        });
    });
}

- (void)removeSelectedVocabularyTerm:(id)sender {
    (void)sender;
    const NSInteger row = _vocabTable.selectedRow;
    if (row < 0 || row >= static_cast<NSInteger>(_vocabTerms.count)) return;
    NSDictionary *term = _vocabTerms[static_cast<NSUInteger>(row)];
    NSString *canonical = [term[@"canonical"] isKindOfClass:NSString.class] ? term[@"canonical"] : @"";
    if (!canonical.length) return;
    NSString *dir = [self voiceVocabularyPreferencesDirectory];
    if (!dir.length) return;
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSString *errMsg = nil;
        id value = [weakSelf callVoiceVocabularyManage:@{
            @"action" : @"remove",
            @"preferences_directory" : dir,
            @"canonical" : canonical,
        } errorMessage:&errMsg];
        dispatch_async(dispatch_get_main_queue(), ^{
            MetasequoiaVoiceProviderSettingsView *strongSelf = weakSelf;
            if (!strongSelf) return;
            if (!value) {
                ShowNotice(strongSelf->_vocabNotice, errMsg ?: @"删除失败。", NSColor.systemRedColor);
                return;
            }
            ShowNotice(strongSelf->_vocabNotice,
                       [NSString stringWithFormat:@"已删除词条「%@」", canonical],
                       NSColor.secondaryLabelColor);
            [strongSelf reloadVocabularyTerms];
            [NSNotificationCenter.defaultCenter postNotificationName:@"MSIMEVoiceVocabularyDidChangeNotification"
                                                              object:strongSelf];
        });
    });
}

- (NSInteger)numberOfRowsInTableView:(NSTableView *)tableView {
    (void)tableView;
    return static_cast<NSInteger>(_vocabTerms.count);
}

- (NSView *)tableView:(NSTableView *)tableView viewForTableColumn:(NSTableColumn *)tableColumn row:(NSInteger)row {
    if (row < 0 || row >= static_cast<NSInteger>(_vocabTerms.count)) return nil;
    NSDictionary *term = _vocabTerms[static_cast<NSUInteger>(row)];
    NSString *ident = tableColumn.identifier ?: @"";
    NSTextField *cell = [tableView makeViewWithIdentifier:ident owner:self];
    if (!cell) {
        cell = [NSTextField labelWithString:@""];
        cell.identifier = ident;
        cell.font = [NSFont systemFontOfSize:12.0];
        cell.lineBreakMode = NSLineBreakByTruncatingTail;
    }
    if ([ident isEqualToString:@"canonical"]) {
        cell.stringValue = [term[@"canonical"] isKindOfClass:NSString.class] ? term[@"canonical"] : @"";
        cell.textColor = NSColor.labelColor;
    } else if ([ident isEqualToString:@"aliases"]) {
        NSArray *aliases = [term[@"aliases"] isKindOfClass:NSArray.class] ? term[@"aliases"] : @[];
        cell.stringValue = [aliases componentsJoinedByString:@"、"];
        cell.textColor = NSColor.secondaryLabelColor;
    } else {
        NSString *source = [term[@"source"] isKindOfClass:NSString.class] ? term[@"source"] : @"";
        cell.stringValue = [source isEqualToString:@"auto_learned"] ? @"自动学习" : @"手动";
        cell.textColor = [source isEqualToString:@"auto_learned"] ? NSColor.controlAccentColor : NSColor.secondaryLabelColor;
    }
    return cell;
}

- (void)tableViewSelectionDidChange:(NSNotification *)notification {
    (void)notification;
    const NSInteger row = _vocabTable.selectedRow;
    _vocabRemoveButton.enabled = row >= 0 && row < static_cast<NSInteger>(_vocabTerms.count);
}

/// What the prompt field shows for a preset: a custom preset's own slot, and nothing for a built-in one, whose wording is not editable. An empty slot tells the recogniser to use the built-in default.
- (NSString *)polishPromptForPreset:(NSString *)identifier
{
    return _polishPromptDrafts[identifier ?: @""] ?: @"";
}

/// Selecting another preset puts that preset's wording in the field rather than leaving the previous one there under a new name, and keeps what was in the field for the preset being left.
- (void)polishPromptIDChanged:(id)sender
{
    (void)sender;
    [self retainEditedPolishPrompt];
    NSString *identifier = MSIMEPolishPromptIdentifierForIndex(_polishPromptID.indexOfSelectedItem);
    _polishPrompt.stringValue = [self polishPromptForPreset:identifier];
    _polishPromptSelection = [identifier copy];
    [self updateEnabled:nil];
    [self commit:nil];
}

/// Keeps what is in the prompt field against the preset that is currently selected, so that a preset returned to still reads what was written into it.
- (void)retainEditedPolishPrompt
{
    if (_polishPromptDrafts[_polishPromptSelection ?: @""] != nil)
        _polishPromptDrafts[_polishPromptSelection] = _polishPrompt.stringValue ?: @"";
}

- (void)providerChanged:(id)sender
{
    (void)sender;
    if (MSIMEVoiceASRProviderUsesService(_loadedProvider))
    {
        _tokenDrafts[_loadedProvider] = _token.stringValue ?: @"";
    }
    NSArray *providerIDs = MSIMEVoiceASRProviderIDs();
    NSUInteger index = MIN((NSUInteger)_provider.indexOfSelectedItem, providerIDs.count - 1);
    NSString *provider = providerIDs[index];
    _endpoint.stringValue = MSIMEVoiceProviderValueAfterSelection(
        _endpoint.stringValue, MSIMEVoiceASRProviderDefaultEndpoint(provider), ProviderValues(@"endpoint"));
    _model.stringValue = MSIMEVoiceProviderValueAfterSelection(
        _model.stringValue, MSIMEVoiceASRProviderDefaultModel(provider), ProviderValues(@"model"));
    id draft = _tokenDrafts[provider];
    _token.stringValue = MSIMEVoiceASRProviderUsesService(provider)
        ? ([draft isKindOfClass:NSString.class] ? draft : ReadToken(@"asr", provider, _endpoint.stringValue))
        : @"";
    _loadedProvider = provider;
    _tokenEndpoint = [_endpoint.stringValue copy];
    [self updateEnabled:nil];
    [self commit:nil];
}

- (void)polishChanged:(id)sender
{
    [self updateEnabled:sender];
    [self commit:sender];
}

/// The recognition card takes the shape of the provider that is selected: a local model has no service address, no model name and no key, and a cloud provider has no model file. Rows that do not apply are taken out of the card rather than greyed out inside it — 本地 Whisper used to leave three dead fields occupying more of the card than the one field it uses. The same thing is done a page away, where 全拼 / 双拼 / 五笔 each build a card and only the selected scheme's is shown.
///
/// The polish fields are still greyed rather than hidden: they say what the text would be sent to, and that is worth reading with the switch off.
- (void)updateEnabled:(id)sender
{
    (void)sender;
    NSArray *providerIDs = MSIMEVoiceASRProviderIDs();
    NSString *provider = providerIDs[MIN((NSUInteger)_provider.indexOfSelectedItem, providerIDs.count - 1)];
    BOOL service = MSIMEVoiceASRProviderUsesService(provider);
    _endpointRow.hidden = !service;
    // Doubao's model is fixed by the endpoint it is reached through, so there is nothing to name.
    _modelRow.hidden = !service || [provider isEqualToString:@"doubao"];
    _tokenRow.hidden = !service;
    _modelPathRow.hidden = ![provider isEqualToString:@"local"];
    _textStreamRow.hidden = ![provider isEqualToString:@"textstream"];
    const BOOL installing = MSIMETextStreamInstalling();
    _textStreamStatus.stringValue = installing ? @"正在安装…" : (MSIMETextStreamReady() ? @"已就绪" : @"未安装");
    _textStreamInstall.title = MSIMETextStreamReady() ? @"修复语音模型" : @"安装语音模型";
    _textStreamInstall.enabled = !installing;
    NSString *polishProvider =
        [NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoicePolishProvider"]
            ?: MSIMEVoicePolishDefaultProvider;
    const BOOL managedLocalPolish = PolishProviderIsLocal(polishProvider);
    const BOOL tokenOptional = !PolishRequiresToken(polishProvider, _polishEndpoint.stringValue);
    // 本机托管润色显示自动发现的 GGUF 状态，本机/局域网 HTTP 端点提示免密钥，其余保持既有文案。
    if (managedLocalPolish)
    {
        NSString *discovered = DiscoveredLocalPolishModelPath(NSUserDefaults.standardUserDefaults);
        _polishKeyStatus.stringValue = discovered.length
            ? [NSString stringWithFormat:@"本机模型已就绪：%@（免密钥）", discovered.lastPathComponent]
            : @"本机模型未就绪（请将 .gguf 放入 ~/Library/Application Support/Shurufa/local-llm/）";
        _polishEndpoint.placeholderString = @"留空由 Shurufa 自动托管本机模型";
    }
    else if (tokenOptional)
    {
        _polishKeyStatus.stringValue = _polishToken.stringValue.length
            ? @"整理密钥：已设置（本机/局域网可选）"
            : @"整理密钥：本机/局域网免填";
        _polishEndpoint.placeholderString = @"https://... 或本机/局域网 http://...";
    }
    else
    {
        _polishKeyStatus.stringValue = _polishToken.stringValue.length ? @"整理密钥：已设置" : @"整理密钥：未设置";
        _polishEndpoint.placeholderString = @"https://... 或本机/局域网 http://...";
    }
    _polishToken.placeholderString = tokenOptional ? @"本机/局域网服务可留空" : @"";
    BOOL polish = _polish.state == NSControlStateValueOn;
    _polishEndpoint.enabled = polish;
    _polishModel.enabled = polish;
    _polishToken.enabled = polish;
    _polishPromptID.enabled = polish;
    // Only a custom preset has wording of its own to edit.
    _polishPrompt.enabled = polish && _polishPromptDrafts[_polishPromptSelection ?: @""] != nil;
}

/// The warning that has to come before the key is cleared, not after it: a key belongs to the service that issued it, so an address that moves to another host leaves it behind. This used to happen from -controlTextDidChange:, which cleared the key on the first keystroke of an edit that had not been made yet and never said that it had.
- (void)controlTextDidChange:(NSNotification *)notification
{
    if (notification.object == _endpoint && _token.stringValue.length > 0)
        ShowNotice(_recognitionNotice, @"改完服务地址后，为原地址保存的 API 密钥会被清除，需要重新填写。",
                   NSColor.secondaryLabelColor);
    if (notification.object == _polishEndpoint && _polishToken.stringValue.length > 0)
        ShowNotice(_polishNotice, @"改完服务地址后，为原地址保存的 API 密钥会被清除，需要重新填写。",
                   NSColor.secondaryLabelColor);
}

// Everything is stored as soon as it is set: a popup or a switch the moment it changes, a field
// when it loses focus — which is also when a key reaches the keychain, rather than on every
// keystroke. Upstream collected the whole form behind a 保存 button, so a change made and not
// confirmed was discarded without a word when the window went away.
- (void)controlTextDidEndEditing:(NSNotification *)notification
{
    if (notification.object == _polishPrompt) [self retainEditedPolishPrompt];
    NSString *cleared = nil;
    if (notification.object == _endpoint && [self clearTokenForMovedEndpoint]) cleared = @"recognition";
    if (notification.object == _polishEndpoint && [self clearPolishTokenForMovedEndpoint]) cleared = @"polish";
    [self commit:nil];
    [self updateEnabled:nil];
    // After the save, because -commit: is what puts a validation failure on these labels and a cleared key is the more useful of the two things to read.
    if ([cleared isEqual:@"recognition"])
        ShowNotice(_recognitionNotice, @"服务地址已更改，原 API 密钥已清除，请重新填写。", NSColor.secondaryLabelColor);
    else if ([cleared isEqual:@"polish"])
        ShowNotice(_polishNotice, @"服务地址已更改，原 API 密钥已清除，请重新填写。", NSColor.secondaryLabelColor);
}

/// Clears the recognition key when the address it was entered for no longer names the same service, and says whether it did. A path is not part of a credential's origin — see MSIMEVoiceProviderCredentialAccount — so moving between two paths of one host keeps the key.
- (BOOL)clearTokenForMovedEndpoint
{
    NSArray *providerIDs = MSIMEVoiceASRProviderIDs();
    NSString *provider = providerIDs[MIN((NSUInteger)_provider.indexOfSelectedItem, providerIDs.count - 1)];
    const BOOL moved = ![MSIMEVoiceProviderCredentialAccount(@"asr", provider, _tokenEndpoint ?: @"")
        isEqual:MSIMEVoiceProviderCredentialAccount(@"asr", provider, _endpoint.stringValue)];
    _tokenEndpoint = [_endpoint.stringValue copy];
    if (!moved || _token.stringValue.length == 0) return NO;
    _token.stringValue = @"";
    _tokenDrafts[provider] = @"";
    return YES;
}

/// The same for the polish key, which is stored against the polish provider and its own address.
- (BOOL)clearPolishTokenForMovedEndpoint
{
    NSString *provider =
        [NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoicePolishProvider"] ?: MSIMEVoicePolishDefaultProvider;
    const BOOL moved = ![MSIMEVoiceProviderCredentialAccount(@"polish", provider, _polishTokenEndpoint ?: @"")
        isEqual:MSIMEVoiceProviderCredentialAccount(@"polish", provider, _polishEndpoint.stringValue)];
    _polishTokenEndpoint = [_polishEndpoint.stringValue copy];
    if (!moved || _polishToken.stringValue.length == 0) return NO;
    _polishToken.stringValue = @"";
    return YES;
}

- (void)commit:(id)sender
{
    (void)sender;
    if (_loading)
    {
        return;
    }
    MetasequoiaVoiceProviderSettings *value = [MetasequoiaVoiceProviderSettings new];
    NSArray *providerIDs = MSIMEVoiceASRProviderIDs();
    value.provider = providerIDs[MIN((NSUInteger)_provider.indexOfSelectedItem, providerIDs.count - 1)];
    value.endpoint = _endpoint.stringValue;
    value.model = _model.stringValue;
    value.token = _token.stringValue;
    if (MSIMEVoiceASRProviderUsesService(value.provider))
    {
        _tokenDrafts[value.provider] = value.token ?: @"";
    }
    else
    {
        [_tokenDrafts removeObjectForKey:value.provider];
    }
    value.tokenSlots = [_tokenDrafts copy];
    value.modelPath = _modelPath.stringValue;
    value.polishEnabled = _polish.state == NSControlStateValueOn;
    value.polishEndpoint = _polishEndpoint.stringValue;
    value.polishModel = _polishModel.stringValue;
    value.polishToken = _polishToken.stringValue;
    [self retainEditedPolishPrompt];
    value.polishPromptID = MSIMEPolishPromptIdentifierForIndex(_polishPromptID.indexOfSelectedItem);
    value.polishPromptCustom1 = _polishPromptDrafts[@"custom_1"] ?: @"";
    value.polishPromptCustom2 = _polishPromptDrafts[@"custom_2"] ?: @"";
    value.polishPromptCustom3 = _polishPromptDrafts[@"custom_3"] ?: @"";
    id captureDevice = _captureDevice.selectedItem.representedObject;
    value.captureDevice = [captureDevice isKindOfClass:NSString.class] ? captureDevice : @"";
    NSError *error = nil;
    const BOOL saved = [value save:&error];
    NSString *message = saved ? nil : (error.localizedDescription ?: @"设置未能保存。");
    NSString *scope = error.userInfo[MSIMEVoiceProviderErrorScopeKey] ?: MSIMEVoiceProviderRecognitionScope;
    ShowNotice(_recognitionNotice, [scope isEqual:MSIMEVoiceProviderRecognitionScope] ? message : nil,
               NSColor.systemRedColor);
    ShowNotice(_polishNotice, [scope isEqual:MSIMEVoiceProviderPolishScope] ? message : nil, NSColor.systemRedColor);
}

- (void)installTextStream:(id)sender
{
    (void)sender;
    ShowNotice(_recognitionNotice, @"正在安装语音模型：首次需要下载运行环境和约 4.7 GB 的模型，可能要几分钟。", NSColor.secondaryLabelColor);
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    MSIMETextStreamInstall(^(BOOL success, NSString *summary) {
        MetasequoiaVoiceProviderSettingsView *strongSelf = weakSelf;
        if (!strongSelf) return;
        ShowNotice(strongSelf->_recognitionNotice, summary, success ? NSColor.secondaryLabelColor : NSColor.systemRedColor);
        [strongSelf updateEnabled:nil];
    });
    [self updateEnabled:nil];
}

- (void)importFromOpenLess:(id)sender
{
    (void)sender;
    _openLessImport.enabled = NO;
    ShowNotice(_polishNotice, @"正在读取 OpenLess 的配置；如果弹出钥匙串授权框，请点“允许”。", NSColor.secondaryLabelColor);
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSDictionary *plan = nil;
        NSData *payload = [MSIMEOpenLessImport readKeychainPayloadWithService:MSIMEOpenLessKeychainService
                                                                      account:MSIMEOpenLessKeychainAccount error:&error];
        if (payload) plan = [MSIMEOpenLessImport planFromCredentialPayload:payload directory:MSIMEOpenLessSupportDirectory() error:&error];
        dispatch_async(dispatch_get_main_queue(), ^{
            MetasequoiaVoiceProviderSettingsView *strongSelf = weakSelf;
            if (!strongSelf) return;
            strongSelf->_openLessImport.enabled = YES;
            NSError *failure = error;
            if (!plan || ![MSIMEOpenLessImport applyPlan:plan toDefaults:NSUserDefaults.standardUserDefaults error:&failure]) {
                ShowNotice(strongSelf->_polishNotice, failure.localizedDescription ?: @"导入失败。", NSColor.systemRedColor);
                return;
            }
            [strongSelf reloadSettings];
            ShowNotice(strongSelf->_polishNotice, [MSIMEOpenLessImport summaryForPlan:plan], NSColor.secondaryLabelColor);
            [[NSNotificationCenter defaultCenter] postNotificationName:MSIMEVoiceProviderSettingsDidChangeNotification object:strongSelf];
        });
    });
}

- (void)browse:(id)sender
{
    (void)sender;
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.canChooseDirectories = YES;
    panel.canChooseFiles = NO;
    panel.allowsMultipleSelection = NO;
    __weak MetasequoiaVoiceProviderSettingsView *weakSelf = self;
    [panel beginSheetModalForWindow:self.window
                  completionHandler:^(NSModalResponse response) {
                    if (response != NSModalResponseOK) return;
                    MetasequoiaVoiceProviderSettingsView *strongSelf = weakSelf;
                    strongSelf->_modelPath.stringValue = panel.URL.path;
                    [strongSelf commit:nil];
                  }];
}
@end

@implementation MetasequoiaVoiceProviderSettingsWindow
{
    MetasequoiaVoiceProviderSettingsView *_form;
}
+ (instancetype)sharedController
{
    static MetasequoiaVoiceProviderSettingsWindow *window;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      window = [self new];
    });
    return window;
}
- (instancetype)init
{
    NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 620, 720)
                                                   styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                                             NSWindowStyleMaskResizable
                                                     backing:NSBackingStoreBuffered
                                                       defer:NO];
    self = [super initWithWindow:window];
    if (self)
    {
        window.title = [MSIMEEditionDisplayName() stringByAppendingString:@" · 语音输入设置"];
        window.releasedWhenClosed = NO;
        _form = [[MetasequoiaVoiceProviderSettingsView alloc] initWithFrame:NSZeroRect];
        NSView *document = [[NSView alloc] initWithFrame:NSZeroRect];
        document.translatesAutoresizingMaskIntoConstraints = NO;
        [document addSubview:_form];
        NSScrollView *scrollView = [[NSScrollView alloc] initWithFrame:window.contentView.bounds];
        scrollView.hasVerticalScroller = YES;
        scrollView.autohidesScrollers = YES;
        scrollView.drawsBackground = NO;
        scrollView.documentView = document;
        scrollView.translatesAutoresizingMaskIntoConstraints = NO;
        [window.contentView addSubview:scrollView];
        [NSLayoutConstraint activateConstraints:@[
            [scrollView.leadingAnchor constraintEqualToAnchor:window.contentView.leadingAnchor],
            [scrollView.trailingAnchor constraintEqualToAnchor:window.contentView.trailingAnchor],
            [scrollView.topAnchor constraintEqualToAnchor:window.contentView.topAnchor],
            [scrollView.bottomAnchor constraintEqualToAnchor:window.contentView.bottomAnchor],
            [document.widthAnchor constraintEqualToAnchor:scrollView.contentView.widthAnchor],
            [_form.leadingAnchor constraintEqualToAnchor:document.leadingAnchor constant:20.0],
            [_form.trailingAnchor constraintEqualToAnchor:document.trailingAnchor constant:-20.0],
            [_form.topAnchor constraintEqualToAnchor:document.topAnchor constant:20.0],
            [_form.bottomAnchor constraintEqualToAnchor:document.bottomAnchor constant:-20.0],
        ]];
        [window center];
    }
    return self;
}
- (void)showAndActivate
{
    [_form reloadSettings];
    [self showWindow:nil];
    MSIMEPresentWindow(self.window);
}
@end
