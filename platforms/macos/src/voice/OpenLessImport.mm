#import "OpenLessImport.h"
#import "VoiceProviderSettingsKeys.h"
#import <Security/Security.h>

NSString *const MSIMEOpenLessKeychainService = @"com.openless.app";
NSString *const MSIMEOpenLessKeychainAccount = @"credentials.v2";

namespace {
NSString *const ImportDomain = @"app.msime.client.voice.import";
NSString *const DefaultArkEndpoint = @"https://ark.cn-beijing.volces.com/api/v3/chat/completions";
// 整理请求在发出前会拒绝超过 8192 个字符的提示词，导入时就说清楚，不让它在录音时才静默失效。
constexpr NSUInteger MaximumPromptLength = 8192;
constexpr NSUInteger MaximumFileBytes = 4 * 1024 * 1024;

NSError *Failure(NSString *message) {
    return [NSError errorWithDomain:ImportDomain code:1 userInfo:@{NSLocalizedDescriptionKey : message}];
}

id JSONFile(NSString *directory, NSString *name) {
    NSString *path = [directory stringByAppendingPathComponent:name];
    NSDictionary *attributes = [NSFileManager.defaultManager attributesOfItemAtPath:path error:nil];
    if (!attributes || [attributes[NSFileSize] unsignedLongLongValue] > MaximumFileBytes) return nil;
    NSData *data = [NSData dataWithContentsOfFile:path];
    return data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
}

NSString *NonEmptyString(id value) {
    return [value isKindOfClass:NSString.class] && [value length] ? value : nil;
}

NSString *NormalizedEndpoint(NSString *value) {
    NSString *endpoint = [value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (!endpoint.length) return DefaultArkEndpoint;
    while ([endpoint hasSuffix:@"/"]) endpoint = [endpoint substringToIndex:endpoint.length - 1];
    if (![endpoint hasSuffix:@"/chat/completions"]) endpoint = [endpoint stringByAppendingString:@"/chat/completions"];
    return endpoint;
}

// 用户当前在用的整理风格的系统提示词。顺序是确定的：对内置风格，用户在 customStylePrompts 里写的覆盖值优先；其次是 style-packs.json 里这个 id 的包；最后是 styleSystemPrompts 里该风格的出厂提示词。
NSString *ActivePrompt(NSDictionary *preferences, id packs, NSString **source) {
    NSString *packID = NonEmptyString(preferences[@"activeStylePackId"]);
    NSString *mode = NonEmptyString(preferences[@"defaultMode"]);
    NSString *builtinKey = [packID hasPrefix:@"builtin."] ? [packID substringFromIndex:8] : (packID ? nil : mode);
    NSDictionary *custom = [preferences[@"customStylePrompts"] isKindOfClass:NSDictionary.class] ? preferences[@"customStylePrompts"] : nil;
    NSDictionary *system = [preferences[@"styleSystemPrompts"] isKindOfClass:NSDictionary.class] ? preferences[@"styleSystemPrompts"] : nil;
    NSString *prompt = builtinKey ? NonEmptyString(custom[builtinKey]) : nil;
    if (prompt) { *source = @"custom"; return prompt; }
    if (packID && [packs isKindOfClass:NSArray.class])
        for (id pack in packs)
            if ([pack isKindOfClass:NSDictionary.class] && [pack[@"id"] isEqual:packID] && ![pack[@"enabled"] isEqual:@NO] &&
                NonEmptyString(pack[@"prompt"])) {
                *source = @"style-pack";
                return pack[@"prompt"];
            }
    prompt = builtinKey ? NonEmptyString(system[builtinKey]) : nil;
    if (prompt) *source = @"system";
    return prompt;
}
} // namespace

NSString *MSIMEOpenLessSupportDirectory(void) {
    return [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/OpenLess"];
}

@implementation MSIMEOpenLessImport

+ (NSData *)readKeychainPayloadWithService:(NSString *)service account:(NSString *)account error:(NSError **)error {
    NSDictionary *query = @{
        (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService : service,
        (__bridge id)kSecAttrAccount : account,
        (__bridge id)kSecReturnData : @YES,
        (__bridge id)kSecMatchLimit : (__bridge id)kSecMatchLimitOne,
    };
    CFTypeRef result = nullptr;
    const OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
    if (status == errSecSuccess && result) return (__bridge_transfer NSData *)result;
    if (error) {
        if (status == errSecItemNotFound) *error = Failure(@"钥匙串里没有 OpenLess 的凭据。");
        else if (status == errSecUserCanceled || status == errSecAuthFailed || status == errSecInteractionNotAllowed)
            *error = Failure(@"没有获得读取 OpenLess 凭据的授权。");
        else *error = Failure([NSString stringWithFormat:@"读取钥匙串失败（%d）。", (int)status]);
    }
    return nil;
}

+ (NSDictionary *)planFromCredentialPayload:(NSData *)payload directory:(NSString *)directory error:(NSError **)error {
    NSDictionary *ark = [self arkFieldsFromPayload:payload];
    NSString *key = NonEmptyString(ark[@"ark.api_key"]);
    NSString *model = NonEmptyString(ark[@"ark.model_id"]);
    if (!key || !model) {
        if (error) *error = Failure(@"OpenLess 的凭据里没有完整的 Ark 配置（密钥和模型）。");
        return nil;
    }
    id preferences = JSONFile(directory, @"preferences.json");
    if (![preferences isKindOfClass:NSDictionary.class]) {
        if (error) *error = Failure(@"读不到 OpenLess 的 preferences.json。");
        return nil;
    }
    NSString *source = nil;
    NSString *prompt = ActivePrompt(preferences, JSONFile(directory, @"style-packs.json"), &source);
    if (!prompt) {
        if (error) *error = Failure(@"找不到 OpenLess 当前整理风格的提示词。");
        return nil;
    }
    if (prompt.length > MaximumPromptLength) {
        if (error) *error = Failure([NSString stringWithFormat:@"OpenLess 的整理提示词有 %lu 字，超过上限 %lu 字，无法导入。", (unsigned long)prompt.length, (unsigned long)MaximumPromptLength]);
        return nil;
    }
    NSString *endpoint = NormalizedEndpoint(NonEmptyString(ark[@"ark.endpoint"]) ?: @"");
    NSURLComponents *url = [NSURLComponents componentsWithString:endpoint];
    if (![url.scheme.lowercaseString isEqual:@"https"] || !url.host.length || url.user || url.password) {
        if (error) *error = Failure(@"OpenLess 的 Ark 接口地址不是有效的 HTTPS 地址。");
        return nil;
    }
    return @{
        @"endpoint" : endpoint,
        @"model" : model,
        @"apiKey" : key,
        @"prompt" : prompt,
        @"promptSource" : source,
        @"thinking" : @([preferences[@"llmThinkingEnabled"] boolValue]),
    };
}

// 凭据载荷是 UTF-8 JSON。Ark 的字段名在 OpenLess 里是 ark.api_key、ark.model_id、ark.endpoint 等；这里按键名在对象树里找带这些键的那个对象（深度上限 5），也认 {"ark": {"api_key": …}} 的嵌套写法，两种写法读出的字段名统一成 ark.* 形式。
+ (NSDictionary *)arkFieldsFromPayload:(NSData *)payload {
    id root = payload.length ? [NSJSONSerialization JSONObjectWithData:payload options:0 error:nil] : nil;
    NSMutableArray *queue = [NSMutableArray array];
    if (root) [queue addObject:@[ root, @0 ]];
    NSArray<NSString *> *names = @[ @"api_key", @"model_id", @"endpoint", @"temperature", @"max_tokens" ];
    while (queue.count) {
        id node = queue.firstObject[0];
        const NSInteger depth = [queue.firstObject[1] integerValue];
        [queue removeObjectAtIndex:0];
        NSArray *children = nil;
        if ([node isKindOfClass:NSDictionary.class]) {
            NSDictionary *object = node;
            if (object[@"ark.api_key"]) return object;
            NSDictionary *nested = [object[@"ark"] isKindOfClass:NSDictionary.class] ? object[@"ark"] : nil;
            if (nested[@"api_key"]) {
                NSMutableDictionary *flat = [NSMutableDictionary dictionary];
                for (NSString *name in names)
                    if (nested[name]) flat[[@"ark." stringByAppendingString:name]] = nested[name];
                return flat;
            }
            children = object.allValues;
        } else if ([node isKindOfClass:NSArray.class]) {
            children = node;
        }
        if (depth < 5)
            for (id child in children)
                if ([child isKindOfClass:NSDictionary.class] || [child isKindOfClass:NSArray.class]) [queue addObject:@[ child, @(depth + 1) ]];
    }
    return @{};
}

+ (BOOL)applyPlan:(NSDictionary *)plan toDefaults:(NSUserDefaults *)defaults error:(NSError **)error {
    NSString *endpoint = plan[@"endpoint"], *model = plan[@"model"], *key = plan[@"apiKey"], *prompt = plan[@"prompt"];
    if (!endpoint.length || !model.length || !key.length || !prompt.length) {
        if (error) *error = Failure(@"导入计划不完整。");
        return NO;
    }
    NSString *provider = @"ark";
    NSDictionary<NSString *, NSString *> *keys = MSIMEVoiceProviderSharedKeys();
    [defaults setObject:provider forKey:@"MSIMEClientVoicePolishProvider"];
    [defaults setObject:endpoint forKey:keys[@"polishEndpoint"]];
    [defaults setObject:model forKey:keys[@"polishModel"]];
    [defaults setObject:key forKey:keys[@"polishToken"]];
    [defaults setObject:@"custom_1" forKey:keys[@"polishPromptID"]];
    [defaults setObject:prompt forKey:keys[@"polishPromptCustom1"]];
    [defaults setObject:MSIMEVoiceProviderTokenSlotsByUpdating([defaults objectForKey:@"MSIMEClientVoicePolishTokens"], provider, key, YES)
                 forKey:@"MSIMEClientVoicePolishTokens"];
    [defaults setBool:YES forKey:@"MSIMEClientVoicePolish"];
    [defaults setObject:@"textstream" forKey:keys[@"provider"]];
    NSMutableDictionary *window = [([defaults dictionaryForKey:@"voiceInput"] ?: @{}) mutableCopy];
    window[@"provider"] = @"textstream";
    window[@"polishEnabled"] = @YES;
    window[@"polishEndpoint"] = endpoint;
    window[@"polishModel"] = model;
    [defaults setObject:window forKey:@"voiceInput"];
    return YES;
}

+ (NSString *)summaryForPlan:(NSDictionary *)plan {
    NSString *source = [plan[@"promptSource"] isEqual:@"custom"] ? @"自定义提示词"
        : [plan[@"promptSource"] isEqual:@"style-pack"] ? @"风格包提示词" : @"内置风格提示词";
    return [NSString stringWithFormat:@"已从 OpenLess 导入：整理模型 %@，%@（%lu 字，已选为自定义方案 1），API 密钥已设置。",
            plan[@"model"], source, (unsigned long)[plan[@"prompt"] length]];
}

@end
