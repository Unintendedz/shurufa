#import "../../src/voice/OpenLessImport.h"
#import "../../src/voice/VoiceProviderSettingsKeys.h"
#import <Security/Security.h>
#include <cassert>
#include <cstdio>

// 导入只用合成数据：假的 OpenLess 目录、假的凭据载荷、另一个 service 名下的假钥匙串条目，写入的是测试用的偏好套件。不读用户真实的 OpenLess 条目。

namespace {
NSString *const FakeKey = @"synthetic-ark-key-0000";

void WriteJSON(NSString *directory, NSString *name, id object) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:object options:0 error:nil];
    assert([data writeToFile:[directory stringByAppendingPathComponent:name] atomically:YES]);
}

NSString *MakeDirectory(NSDictionary *preferences, id packs) {
    NSString *directory = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
    assert([NSFileManager.defaultManager createDirectoryAtPath:directory withIntermediateDirectories:YES attributes:nil error:nil]);
    WriteJSON(directory, @"preferences.json", preferences);
    if (packs) WriteJSON(directory, @"style-packs.json", packs);
    // 私人内容必须读不到：放一个会让任何读取它的实现出错的假文件。
    assert([@"not json" writeToFile:[directory stringByAppendingPathComponent:@"history.json"] atomically:YES encoding:NSUTF8StringEncoding error:nil]);
    return directory;
}

NSData *Payload(id object) { return [NSJSONSerialization dataWithJSONObject:object options:0 error:nil]; }

NSDictionary *Credentials() {
    return @{@"ark.api_key" : FakeKey, @"ark.model_id" : @"synthetic-model", @"ark.endpoint" : @"https://ark.example.test/api/v3"};
}
} // namespace

int main() {
    @autoreleasepool {
        NSDictionary *preferences = @{
            @"activeLlmProvider" : @"ark", @"defaultMode" : @"structured", @"activeStylePackId" : @"builtin.structured",
            @"llmThinkingEnabled" : @YES,
            @"customStylePrompts" : @{@"structured" : @"", @"formal" : @""},
            @"styleSystemPrompts" : @{@"structured" : @"synthetic structured prompt", @"formal" : @"synthetic formal prompt"},
        };
        NSArray *packs = @[ @{@"id" : @"builtin.structured", @"enabled" : @YES, @"prompt" : @"synthetic pack prompt"},
                            @{@"id" : @"user.pack", @"enabled" : @YES, @"prompt" : @"synthetic user pack prompt"} ];

        // 载荷的两种写法都认；缺字段时报错。
        NSString *directory = MakeDirectory(preferences, packs);
        NSError *error = nil;
        NSDictionary *plan = [MSIMEOpenLessImport planFromCredentialPayload:Payload(Credentials()) directory:directory error:&error];
        assert(plan && !error);
        assert([plan[@"endpoint"] isEqual:@"https://ark.example.test/api/v3/chat/completions"] && [plan[@"model"] isEqual:@"synthetic-model"]);
        assert([plan[@"apiKey"] isEqual:FakeKey] && [plan[@"thinking"] isEqual:@YES]);
        // 风格包里的这个 id 优先于出厂提示词；用户在 customStylePrompts 里写了覆盖值时，覆盖值优先。
        assert([plan[@"prompt"] isEqual:@"synthetic pack prompt"] && [plan[@"promptSource"] isEqual:@"style-pack"]);
        NSString *withoutPacks = MakeDirectory(preferences, nil);
        plan = [MSIMEOpenLessImport planFromCredentialPayload:Payload(@{@"channels" : @[ @{@"ark" : @{@"api_key" : FakeKey, @"model_id" : @"synthetic-model"}} ]}) directory:withoutPacks error:&error];
        assert(plan && [plan[@"prompt"] isEqual:@"synthetic structured prompt"] && [plan[@"promptSource"] isEqual:@"system"]);
        assert([plan[@"endpoint"] hasPrefix:@"https://ark.cn-beijing.volces.com/"]);
        NSMutableDictionary *custom = [preferences mutableCopy];
        custom[@"customStylePrompts"] = @{@"structured" : @"synthetic custom prompt"};
        plan = [MSIMEOpenLessImport planFromCredentialPayload:Payload(Credentials()) directory:MakeDirectory(custom, packs) error:&error];
        assert([plan[@"prompt"] isEqual:@"synthetic custom prompt"] && [plan[@"promptSource"] isEqual:@"custom"]);
        NSMutableDictionary *userPack = [preferences mutableCopy];
        userPack[@"activeStylePackId"] = @"user.pack";
        plan = [MSIMEOpenLessImport planFromCredentialPayload:Payload(Credentials()) directory:MakeDirectory(userPack, packs) error:&error];
        assert([plan[@"prompt"] isEqual:@"synthetic user pack prompt"]);

        assert(![MSIMEOpenLessImport planFromCredentialPayload:Payload(@{@"ark.api_key" : FakeKey}) directory:directory error:&error] && error);
        assert(![MSIMEOpenLessImport planFromCredentialPayload:[@"not json" dataUsingEncoding:NSUTF8StringEncoding] directory:directory error:&error]);
        assert(![MSIMEOpenLessImport planFromCredentialPayload:Payload(Credentials()) directory:@"/nonexistent" error:&error]);
        NSMutableDictionary *insecure = [Credentials() mutableCopy];
        insecure[@"ark.endpoint"] = @"http://ark.example.test/api/v3";
        assert(![MSIMEOpenLessImport planFromCredentialPayload:Payload(insecure) directory:directory error:&error]);
        NSMutableDictionary *tooLong = [preferences mutableCopy];
        tooLong[@"styleSystemPrompts"] = @{@"structured" : [@"x" stringByPaddingToLength:8193 withString:@"x" startingAtIndex:0]};
        assert(![MSIMEOpenLessImport planFromCredentialPayload:Payload(Credentials()) directory:MakeDirectory(tooLong, nil) error:&error]);
        assert([error.localizedDescription containsString:@"8192"]);

        // 写入：整理服务、自定义方案 1 并选中，识别服务设为 textstream；密钥进偏好但不进摘要。
        plan = [MSIMEOpenLessImport planFromCredentialPayload:Payload(Credentials()) directory:directory error:&error];
        NSString *suite = [@"app.msime.test.openless." stringByAppendingString:NSUUID.UUID.UUIDString];
        NSUserDefaults *defaults = [[NSUserDefaults alloc] initWithSuiteName:suite];
        assert([MSIMEOpenLessImport applyPlan:plan toDefaults:defaults error:&error]);
        assert([[defaults stringForKey:@"MSIMEClientVoiceASRProvider"] isEqual:@"textstream"]);
        assert([[defaults stringForKey:@"MSIMEClientVoicePolishProvider"] isEqual:@"ark"] && [defaults boolForKey:@"MSIMEClientVoicePolish"]);
        assert([[defaults stringForKey:@"MSIMEClientVoicePolishEndpoint"] isEqual:@"https://ark.example.test/api/v3/chat/completions"]);
        assert([[defaults stringForKey:@"MSIMEClientVoicePolishModel"] isEqual:@"synthetic-model"]);
        assert([[defaults stringForKey:@"MSIMEClientVoicePolishToken"] isEqual:FakeKey]);
        assert([[defaults stringForKey:@"MSIMEClientVoicePolishPromptID"] isEqual:@"custom_1"]);
        assert([[defaults stringForKey:@"MSIMEClientVoicePolishPromptCustom1"] isEqual:@"synthetic pack prompt"]);
        assert([MSIMEVoiceProviderSharedKeys()[@"polishPromptCustom1"] isEqual:@"MSIMEClientVoicePolishPromptCustom1"]);
        assert([[defaults dictionaryForKey:@"MSIMEClientVoicePolishTokens"][@"ark"] isEqual:FakeKey]);
        NSDictionary *window = [defaults dictionaryForKey:@"voiceInput"];
        assert([window[@"provider"] isEqual:@"textstream"] && [window[@"polishEnabled"] isEqual:@YES]);
        NSString *summary = [MSIMEOpenLessImport summaryForPlan:plan];
        assert(![summary containsString:FakeKey] && [summary containsString:@"synthetic-model"] && [summary containsString:@"密钥已设置"]);
        NSMutableDictionary *incomplete = [plan mutableCopy];
        [incomplete removeObjectForKey:@"apiKey"];
        assert(![MSIMEOpenLessImport applyPlan:incomplete toDefaults:defaults error:&error]);
        [defaults removePersistentDomainForName:suite];

        // 钥匙串读取：用另一个 service 名下的假条目。读不到条目时报错而不是返回空。
        NSString *service = [@"app.msime.test.openless." stringByAppendingString:NSUUID.UUID.UUIDString];
        NSString *account = @"credentials.test";
        assert(![MSIMEOpenLessImport readKeychainPayloadWithService:service account:account error:&error] && error);
        NSDictionary *item = @{(__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword, (__bridge id)kSecAttrService : service,
                               (__bridge id)kSecAttrAccount : account, (__bridge id)kSecValueData : Payload(Credentials())};
        if (SecItemAdd((__bridge CFDictionaryRef)item, nullptr) == errSecSuccess) {
            NSData *read = [MSIMEOpenLessImport readKeychainPayloadWithService:service account:account error:&error];
            assert(read && [read isEqual:Payload(Credentials())]);
            assert([MSIMEOpenLessImport planFromCredentialPayload:read directory:directory error:&error]);
            SecItemDelete((__bridge CFDictionaryRef)@{(__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword, (__bridge id)kSecAttrService : service});
        } else {
            std::printf("keychain unavailable here; skipped the fake item round trip\n");
        }
        assert([MSIMEOpenLessKeychainService isEqual:@"com.openless.app"] && [MSIMEOpenLessKeychainAccount isEqual:@"credentials.v2"]);
        std::printf("openless import ok\n");
    }
    return 0;
}
