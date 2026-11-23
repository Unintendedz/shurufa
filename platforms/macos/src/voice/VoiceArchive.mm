#import "VoiceArchive.h"
#import <Security/Security.h>
#import "../settings/RuntimeOptions.h"
#include "../../../common/HostApiString.h"
#include "msime_client.h"
#include <cmath>

NSString *const MSIMEVoiceArchiveDidChangeNotification = @"MSIMEVoiceArchiveDidChange";

namespace {
// 钥匙串条目：只在本机、首次解锁后可读，不随 iCloud 钥匙串同步。换一台 Mac 是一把新密钥、一份空的历史。
NSString *const KeychainService = @"dev.shurufa.voice-archive";
NSString *const KeychainAccount = @"archive-key-v1";
constexpr NSUInteger KeyBytes = 32;

NSDictionary *KeychainQuery() {
    return @{(__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
             (__bridge id)kSecAttrService: KeychainService,
             (__bridge id)kSecAttrAccount: KeychainAccount};
}

// 读出密钥；还没有时生成并写入。读失败但不是「没有」时返回 nil，绝不覆盖已有的密钥。
NSData *LoadOrCreateKey() {
    NSMutableDictionary *query = [KeychainQuery() mutableCopy];
    query[(__bridge id)kSecReturnData] = @YES;
    CFTypeRef result = nullptr;
    OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
    if (status == errSecSuccess) {
        NSData *key = CFBridgingRelease(result);
        return key.length == KeyBytes ? key : nil;
    }
    if (status != errSecItemNotFound) return nil;
    NSMutableData *key = [NSMutableData dataWithLength:KeyBytes];
    if (SecRandomCopyBytes(kSecRandomDefault, KeyBytes, key.mutableBytes) != errSecSuccess) return nil;
    NSMutableDictionary *item = [KeychainQuery() mutableCopy];
    item[(__bridge id)kSecValueData] = key;
    item[(__bridge id)kSecAttrAccessible] = (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
    item[(__bridge id)kSecAttrLabel] = @"Shurufa 语音存档密钥";
    return SecItemAdd((__bridge CFDictionaryRef)item, nullptr) == errSecSuccess ? key : nil;
}

NSString *ArchiveDirectory() {
    NSString *state = MSIMELoadRuntimeOptions()[@"preferences_directory"];
    if (![state isKindOfClass:NSString.class] || !state.isAbsolutePath)
        state = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
    return state.length ? [state stringByAppendingPathComponent:@"voice-archive"] : nil;
}

// 调用一个返回 JSON 的入口，取出 value；失败返回 nil。
id Call(char *raw) {
    auto owned = msime::host_api::own_string(raw);
    if (!owned) return nil;
    NSData *data = [NSData dataWithBytes:owned.get() length:strlen(owned.get())];
    NSDictionary *response = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![response isKindOfClass:NSDictionary.class] || ![response[@"ok"] isEqual:@YES]) return nil;
    return response[@"value"] ?: NSNull.null;
}

id CallWithRequest(char *(*function)(const uint8_t *, size_t), NSDictionary *request) {
    if (![NSJSONSerialization isValidJSONObject:request]) return nil;
    NSData *json = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    return json ? Call(function(static_cast<const uint8_t *>(json.bytes), json.length)) : nil;
}

uint64_t NowMilliseconds() {
    return static_cast<uint64_t>(NSDate.date.timeIntervalSince1970 * 1000.0);
}

void CollectBytes(const uint8_t *bytes, size_t length, void *context) {
    [(__bridge NSMutableData *)context appendBytes:bytes length:length];
}
}

@implementation MSIMEVoiceArchive {
    dispatch_queue_t _queue;
    BOOL _available;
    NSString *_live;
}

+ (instancetype)sharedArchive {
    static MSIMEVoiceArchive *archive;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ archive = [MSIMEVoiceArchive new]; });
    return archive;
}

- (instancetype)init {
    if (!(self = [super init])) return nil;
    _queue = dispatch_queue_create("dev.shurufa.voice-archive", DISPATCH_QUEUE_SERIAL);
    [self openIfNeeded];
    return self;
}

- (BOOL)isAvailable { return [self openIfNeeded]; }

// 密钥第一次取不到（例如登录后钥匙串还没解锁）时，下次使用再试。
- (BOOL)openIfNeeded {
    @synchronized(self) {
        if (_available) return YES;
        NSString *directory = ArchiveDirectory();
        NSData *key = directory ? LoadOrCreateKey() : nil;
        if (!key) return NO;
        const char *path = directory.fileSystemRepresentation;
        _available = Call(msime_client_voice_archive_open(reinterpret_cast<const uint8_t *>(path), strlen(path),
            static_cast<const uint8_t *>(key.bytes), key.length)) != nil;
        if (_available) dispatch_async(_queue, ^{ [self purgeNow]; });
        return _available;
    }
}

- (NSString *)beginRecordingForApp:(NSString *)app provider:(NSString *)provider {
    if (![self openIfNeeded]) return nil;
    __block NSString *identifier = nil;
    // 与追加、收尾走同一条队列：上一条的收尾一定先于这一条开始。
    dispatch_sync(_queue, ^{
        id value = CallWithRequest(msime_client_voice_archive_begin,
            @{@"now_ms": @(NowMilliseconds()), @"app": app ?: @"", @"provider": provider ?: @""});
        identifier = [value isKindOfClass:NSDictionary.class] ? value[@"id"] : nil;
        self->_live = identifier;
    });
    return [identifier isKindOfClass:NSString.class] ? identifier : nil;
}

- (void)appendPCM:(NSData *)pcm {
    if (!pcm.length || !_available) return;
    dispatch_async(_queue, ^{
        if (!self->_live) return;
        msime_client_voice_archive_append(static_cast<const float *>(pcm.bytes), pcm.length / sizeof(float));
    });
}

- (void)endCaptureDiscarding:(BOOL)discard {
    if (!_available) return;
    dispatch_async(_queue, ^{
        NSString *live = self->_live;
        if (!live) return;
        self->_live = nil;
        Call(msime_client_voice_archive_end_capture(NowMilliseconds()));
        if (discard) {
            NSData *identifier = [live dataUsingEncoding:NSUTF8StringEncoding];
            Call(msime_client_voice_archive_delete(static_cast<const uint8_t *>(identifier.bytes), identifier.length));
        }
        [self purgeNow];
        [self postChange];
    });
}

- (void)note:(NSString *)kind text:(NSString *)text recording:(NSString *)identifier {
    if (!identifier.length || !_available) return;
    NSDictionary<NSString *, id> *request = @{
        @"kind": [kind copy],
        @"text": text.length ? [text copy] : @"",
        @"id": [identifier copy],
    };
    dispatch_async(_queue, ^{
        CallWithRequest(msime_client_voice_archive_note, request);
        [self postChange];
    });
}

- (void)noteText:(NSString *)text forRecording:(NSString *)identifier {
    [self noteText:text metadata:nil forRecording:identifier];
}

- (void)noteText:(NSString *)text
        metadata:(nullable NSDictionary<NSString *, id> *)metadata
    forRecording:(NSString *)identifier {
    if (!identifier.length || !_available) return;
    NSString *copiedText = text.length ? [text copy] : @"";
    NSString *copiedIdentifier = [identifier copy];
    NSMutableDictionary<NSString *, id> *payload = [NSMutableDictionary dictionary];
    payload[@"kind"] = @"text";
    payload[@"text"] = copiedText;
    payload[@"id"] = copiedIdentifier;
    BOOL hasMetadata = NO;
    // 仅白名单透传已知元数据键并做显式类型转型与边界归一化，避免 selector 歧义或非法值导致序列化异常。
    if ([metadata isKindOfClass:NSDictionary.class]) {
        for (NSString *key in @[
            @"raw_text", @"mode", @"app_category", @"polish_provider",
            @"polish_model", @"polish_prompt_id", @"polish_rejected_reason"
        ]) {
            id rawValue = metadata[key];
            if ([rawValue isKindOfClass:NSString.class]) {
                NSString *stringValue = static_cast<NSString *>(rawValue);
                if (stringValue.length) {
                    payload[key] = [stringValue copy];
                    hasMetadata = YES;
                }
            }
        }
        id rawPolishFailed = metadata[@"polish_failed"];
        if ([rawPolishFailed isKindOfClass:NSNumber.class]) {
            NSNumber *flagValue = static_cast<NSNumber *>(rawPolishFailed);
            payload[@"polish_failed"] = @(flagValue.boolValue);
            hasMetadata = YES;
        }
        for (NSString *key in @[ @"asr_ms", @"polish_ms" ]) {
            id rawValue = metadata[key];
            if ([rawValue isKindOfClass:NSNumber.class] &&
                CFGetTypeID((__bridge CFTypeRef)rawValue) != CFBooleanGetTypeID()) {
                NSNumber *numberValue = static_cast<NSNumber *>(rawValue);
                const double numeric = numberValue.doubleValue;
                if (std::isfinite(numeric) && numeric >= 0.0 && numeric < 18446744073709551616.0) {
                    payload[key] = @(numberValue.unsignedLongLongValue);
                    hasMetadata = YES;
                }
            }
        }
    }
    if (!copiedText.length && !hasMetadata) return;
    NSDictionary<NSString *, id> *request = [payload copy];
    dispatch_async(_queue, ^{
        id result = CallWithRequest(msime_client_voice_archive_note, request);
        if (!result && hasMetadata && copiedText.length) {
            // 可选元数据若因超限或非法字符被共享层拒绝，回退写入裸文本，确保用户听写内容绝不静默丢失。
            CallWithRequest(msime_client_voice_archive_note, @{
                @"kind": @"text",
                @"text": copiedText,
                @"id": copiedIdentifier,
            });
        }
        [self postChange];
    });
}

- (void)noteDismissedReason:(NSString *)reason forRecording:(NSString *)identifier {
    if (!identifier.length || !reason.length || !_available) return;
    // 原因长度上限（128 字节）与控制字符检查由 Rust 共享层统一把关。
    [self note:@"dismissed" text:reason recording:identifier];
}

- (void)noteFailure:(NSString *)reason forRecording:(NSString *)identifier { [self note:@"failure" text:reason recording:identifier]; }
- (void)noteCancelledRecording:(NSString *)identifier { [self note:@"cancelled" text:nil recording:identifier]; }

- (BOOL)noteRetranscription:(NSString *)text forRecording:(NSString *)identifier {
    if (![self openIfNeeded]) return NO;
    BOOL saved = CallWithRequest(msime_client_voice_archive_note,
        @{@"kind": @"retranscription", @"text": text ?: @"", @"id": identifier}) != nil;
    if (saved) [self postChange];
    return saved;
}

- (NSArray<NSDictionary *> *)entriesWithUnreadableCount:(NSUInteger *)unreadable {
    if (![self openIfNeeded]) return nil;
    id value = Call(msime_client_voice_archive_list());
    if (![value isKindOfClass:NSDictionary.class]) return nil;
    if (unreadable) *unreadable = [value[@"unreadable"] unsignedIntegerValue];
    NSArray *entries = value[@"entries"];
    return [entries isKindOfClass:NSArray.class] ? entries : nil;
}

- (NSData *)audioForRecording:(NSString *)identifier format:(uint32_t)format {
    if (![self openIfNeeded]) return nil;
    NSData *name = [identifier dataUsingEncoding:NSUTF8StringEncoding];
    NSMutableData *bytes = [NSMutableData data];
    id value = Call(msime_client_voice_archive_audio(static_cast<const uint8_t *>(name.bytes), name.length, format,
        CollectBytes, (__bridge void *)bytes));
    return value ? bytes : nil;
}
- (NSData *)WAVForRecording:(NSString *)identifier { return [self audioForRecording:identifier format:0]; }
- (NSData *)PCMForRecording:(NSString *)identifier { return [self audioForRecording:identifier format:1]; }

- (BOOL)deleteRecording:(NSString *)identifier {
    if (![self openIfNeeded]) return NO;
    NSData *name = [identifier dataUsingEncoding:NSUTF8StringEncoding];
    BOOL deleted = Call(msime_client_voice_archive_delete(static_cast<const uint8_t *>(name.bytes), name.length)) != nil;
    if (deleted) [self postChange];
    return deleted;
}

- (void)purge {
    if (![self openIfNeeded]) return;
    dispatch_async(_queue, ^{ [self purgeNow]; [self postChange]; });
}

- (void)purgeNow {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    const NSInteger hours = [defaults integerForKey:@"MSIMEClientVoiceRecordingRetentionHours"];
    const NSInteger days = [defaults integerForKey:@"MSIMEClientVoiceHistoryRetentionDays"];
    CallWithRequest(msime_client_voice_archive_purge, @{@"now_ms": @(NowMilliseconds()),
        @"recording_retention_hours": @(hours == 72 ? 72 : 24), @"history_retention_days": @(MIN(MAX(days, 0), 36500))});
}

- (void)postChange {
    dispatch_async(dispatch_get_main_queue(), ^{
        [NSNotificationCenter.defaultCenter postNotificationName:MSIMEVoiceArchiveDidChangeNotification object:self];
    });
}
@end
