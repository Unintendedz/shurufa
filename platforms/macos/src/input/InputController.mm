#import <AppKit/AppKit.h>
#import <InputMethodKit/InputMethodKit.h>
#import <CoreText/CoreText.h>
#import <CommonCrypto/CommonDigest.h>
#import "MSIMEClientSession.h"
#import "../settings/RuntimeOptions.h"
#import "../../../../shared/apple/TextClient.h"
#include "msime_client.h"
#include "../../../common/HostApiString.h"
#import "../candidate/CandidatePlacement.h"
#import "../candidate/CandidateWindowLevel.h"
#import "../candidate/CandidateGlossSenses.h"
#import "InputSourceRegistration.h"
#import "InputControllerActivity.h"
#include "../../../common/DictionaryQuiesceLease.h"
#import "../dictionary/DictionaryWindowController.h"
#import "../core/ClientDictionaryRuntime.h"
#import "../settings/AppearancePreferences.h"
#import "InputMenu.h"
#import "../settings/PreferencesWindowController.h"
#import "../core/SharedVoicePreferences.h"
#import "../voice/VoiceProviderOptions.h"
#import "../voice/VoiceTextCommit.h"
#include "SmartPunctuationRewrite.h"
#import "../voice/VoiceDeactivation.h"
#import "../voice/HTTPVoiceRequest.h"
#import "../voice/VoiceHoldShortcut.h"
#import "../voice/ConfiguredVoiceShortcut.h"
#import "../voice/VoiceRecordingMonitor.h"
#import "../voice/VoiceShortcutTap.h"
#import "../core/WindowPresentationLog.h"
#import "../voice/DoubaoVoiceRequest.h"
#import "../voice/LocalVoiceRequest.h"
#import "../voice/VoiceContextCapture.h"
#import "../voice/LocalPolishServer.h"
#import "../voice/VoicePolishClient.h"
#import "../voice/VoicePipelineHelper.h"
#import "../voice/VoiceEditTracker.h"
#import "../voice/VoiceTargetEditability.h"
#import "../voice/VoiceRecoveryToast.h"
#include <time.h>
#import "../voice/VoiceFailureMessages.h"
#import "../core/SupportWindowController.h"
#include "../core/AXPrecedingContext.h"
#import "../candidate/CandidateChrome.h"
#import "../candidate/CandidatePointerMenu.h"
#import "../candidate/CandidatePager.h"
#import "../candidate/CandidateExpandToggle.h"
#import "../candidate/CandidateMotion.h"
#import "../core/WindowPresentation.h"
#import "../candidate/CandidateTypography.h"
#import "../candidate/CandidateTextMetrics.h"
#include "../candidate/CandidateSkin.h"
#include "../settings/ShuangpinProfileNames.h"
#include "../../../../shared/candidate/CandidateWheelRouting.h"
#include "../candidate/CandidatePageSize.h"
#include "../../../../shared/candidate/CandidateExpandedGrid.h"
#import "ShurufaViewState.h"
#import "../core/ChineseTextConversion.h"
#include "../core/CandidateTranslationDisplay.h"
#include "../core/CandidateBadgePolicy.h"
#include "../core/FullWidthInput.h"
#include "InputControllerPhysicalKeys.h"
#include "InputSchemeTraits.h"
#include "../core/ModifierTap.h"
#include "../core/EditableShiftFocus.h"
#include "../core/VimMode.h"
#import "../settings/ShuangpinKeymapPanel.h"
#import "InputModeHUDPanel.h"
#import "InputModeStatusItem.h"
#import "TypingEffectPanel.h"
#import "InputModeIdentifiers.h"
#import "../voice/VoiceInputService.h"
#import "../voice/VoiceProviderSocket.h"
#import "../voice/VoiceWaveOverlay.h"
#import "../voice/VoiceInputLevel.h"
#include "../../../../shared/voice/CaptureDuration.h"
#include "../../../../shared/voice/VoiceProviders.h"
#import "../voice/VoiceCuePlayer.h"
#import "../voice/VoiceAudioMuter.h"
#import "../voice/VoiceProviderSettings.h"
#import "../voice/VoiceRecognitionQuery.h"
#import "../voice/VoiceSettings.h"
#import "../cloud/CloudCandidateRequest.h"
#import "../core/CustomTranslationBatch.h"
#import "../cloud/TranslationCache.h"
#include "../core/WubiCodeHintPolicy.h"
#include "../core/PairedPunctuation.h"
#include "../core/PairedPunctuation.h"
#include "../core/TypingStatistics.h"
#include "../core/DiagnosticLog.h"
#include "../../../../shared/input/EnglishModeOutput.h"
#include "../../../../shared/input/CapsLockTap.h"
#include "TrilingualCycleRouting.h"
#import <IOKit/hidsystem/IOHIDLib.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#import <IOKit/hidsystem/IOHIDParameter.h>
#import <IOKit/hid/IOHIDKeys.h>
#import <IOKit/hid/IOHIDManager.h>
#include <atomic>
#include <cmath>
#include <memory>
#include <optional>
#include <unordered_map>

static BOOL MSIMEStrictBoolean(id value);

static dispatch_queue_t MSIMETypingStatisticsQueue(void) {
    static dispatch_queue_t queue;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ queue = dispatch_queue_create("app.msime.client.typing-statistics", DISPATCH_QUEUE_SERIAL); });
    return queue;
}

// 分布式通知在整个登录会话里广播，名字随版本而变（MSIMEEditionNotificationName，full 不变），一个版本的设置应用不会改动另一个版本的输入法。与 crates/host-macos/native/dictionary.mm 收发的是同一个名字。
#define MSIMETypingStatisticsEnabledChangedNotification \
    MSIMEEditionNotificationName(@"MetasequoiaTypingStatisticsEnabledChangedNotification")
// 由设置窗口（crates/host-macos/native/dictionary.mm）和原生词库窗口在 quiesce 租约写好之后发出。它只负责叫醒控制器；控制器放手之前检查的是词库锁旁边的租约。
#define MSIMEDictionaryMaintenanceWillBeginNotification \
    MSIMEEditionNotificationName(@"MSIMEDictionaryMaintenanceWillBeginNotification")

// Dictionary maintenance needs the Engine's exclusive lock, and every open session holds it shared. While the settings window's lease (platforms/common/DictionaryQuiesceLease.h) is live on a session's user directory, that session is closed and none is opened on it.
static BOOL MSIMEDictionaryQuiesced(NSDictionary *options) {
    id userData = [options isKindOfClass:NSDictionary.class] ? options[@"user_data"] : nil;
    if (![userData isKindOfClass:NSString.class] || ![userData isAbsolutePath]) return NO;
    return msime::dictionary_lease::dictionary_quiesced(std::string([userData fileSystemRepresentation]));
}
// Privacy-preserving default: until the persisted opt-in is loaded, the capture boundary is shut.
static std::atomic_bool MSIMETypingStatisticsEnabled{false};

static void MSIMEReloadTypingStatisticsEnabled(NSString *directory) {
    MSIMETypingStatisticsEnabled.store(false, std::memory_order_relaxed);
    if (MSIMEEditionIsShurufa()) return;
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) return;
    NSData *bytes = [directory dataUsingEncoding:NSUTF8StringEncoding];
    if (!bytes || bytes.length > 16384) return;
    const int32_t enabled = msime_client_typing_statistics_enabled(
        static_cast<const uint8_t *>(bytes.bytes), bytes.length);
    if (enabled >= 0) MSIMETypingStatisticsEnabled.store(enabled == 1, std::memory_order_relaxed);
    else msime_macos_diagnostic_write("stats: enabled_read_failed");
}

static void MSIMERecordTypingStatistics(NSString *directory, NSString *text, msime::mac::TypingSource source) {
    // Match the Windows capture contract: an opt-out does not inspect or classify committed text,
    // allocate a request, enter the worker queue, or touch the statistics store.
    if (!MSIMETypingStatisticsEnabled.load(std::memory_order_relaxed)) return;
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath ||
        ![text isKindOfClass:NSString.class] || text.length == 0) return;
    NSDateComponents *components = [NSCalendar.currentCalendar components:NSCalendarUnitYear | NSCalendarUnitMonth |
        NSCalendarUnitDay | NSCalendarUnitHour fromDate:NSDate.date];
    NSString *day = [NSString stringWithFormat:@"%04ld-%02ld-%02ld", (long)components.year,
        (long)components.month, (long)components.day];
    const std::string_view sourceID = msime::mac::TypingSourceId(source);
    NSString *sourceString = [[NSString alloc] initWithBytes:sourceID.data() length:sourceID.size()
                                                     encoding:NSUTF8StringEncoding];
    if (!sourceString) {
        msime_macos_diagnostic_write("stats: invalid_source");
        return;
    }
    NSDictionary *request = @{ @"directory": directory, @"action": @{
        @"operation": @"record", @"text": text,
        @"source": sourceString,
        @"day": day,
        // The hour axis has to come from the same calendar as the day beside it.
        @"hour": @((long)components.hour) } };
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    if (!data || data.length > 65536) {
        msime_macos_diagnostic_write(data ? "stats: request_too_large" : "stats: request_encode_failed");
        return;
    }
    dispatch_async(MSIMETypingStatisticsQueue(), ^{
        auto response = msime::host_api::own_string(
            msime_client_typing_statistics(static_cast<const uint8_t *>(data.bytes), data.length));
        // Statistics are best effort and must never affect text commitment, so the response carries no UI state. A failure is logged as a label only, like the source's stats open/persist/retention lines: the error string can name files, and the log never carries it.
        if (!response) {
            msime_macos_diagnostic_write("stats: record_failed reason=no_response");
            return;
        }
        if (msime_macos_diagnostic_enabled()) {
            NSData *body = [NSData dataWithBytesNoCopy:response.get() length:strlen(response.get()) freeWhenDone:NO];
            NSDictionary *envelope = [NSJSONSerialization JSONObjectWithData:body options:0 error:nil];
            if (![envelope isKindOfClass:NSDictionary.class])
                msime_macos_diagnostic_write("stats: record_failed reason=malformed_response");
            else if (!MSIMEStrictBoolean(envelope[@"ok"]))
                msime_macos_diagnostic_write("stats: record_failed reason=store");
        }
    });
}

// The local calendar day of this moment, as the statistics store names days.
static NSString *MSIMETypingStatisticsLocalDay(void) {
    NSDateComponents *components = [NSCalendar.currentCalendar components:NSCalendarUnitYear | NSCalendarUnitMonth |
        NSCalendarUnitDay fromDate:NSDate.date];
    return [NSString stringWithFormat:@"%04ld-%02ld-%02ld", (long)components.year, (long)components.month,
        (long)components.day];
}

// Writes one batch of key heatmap counts. The request is built and sent on the statistics worker, so the key path only hands over the batch it has already collected. A caller about to exit the process waits for the write, and with it every earlier write still queued on the serial worker, because exit does not run queued blocks.
static void MSIMERecordKeyPresses(NSString *directory, msime::mac::KeyPressFlush flush, bool waitUntilWritten) {
    if (!MSIMETypingStatisticsEnabled.load(std::memory_order_relaxed)) return;
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath || flush.keys.empty()) return;
    auto batch = std::make_shared<msime::mac::KeyPressFlush>(std::move(flush));
    NSString *path = [directory copy];
    (waitUntilWritten ? dispatch_sync : dispatch_async)(MSIMETypingStatisticsQueue(), ^{
        NSMutableDictionary<NSString *, NSNumber *> *keys = [NSMutableDictionary dictionaryWithCapacity:batch->keys.size()];
        for (const auto &[key, count] : batch->keys)
            keys[[NSString stringWithUTF8String:key.c_str()]] = @(count);
        NSDictionary *request = @{ @"directory": path, @"action": @{
            @"operation": @"record_keys", @"day": [NSString stringWithUTF8String:batch->day.c_str()], @"keys": keys } };
        NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
        if (!data) {
            msime_macos_diagnostic_write("stats: record_keys_failed reason=encode");
            return;
        }
        auto response = msime::host_api::own_string(
            msime_client_typing_statistics(static_cast<const uint8_t *>(data.bytes), data.length));
        // Like the text path, a failure is logged as a label only and never reaches the key path.
        if (!response) {
            msime_macos_diagnostic_write("stats: record_keys_failed reason=no_response");
            return;
        }
        if (msime_macos_diagnostic_enabled()) {
            NSData *body = [NSData dataWithBytesNoCopy:response.get() length:strlen(response.get()) freeWhenDone:NO];
            NSDictionary *envelope = [NSJSONSerialization JSONObjectWithData:body options:0 error:nil];
            if (![envelope isKindOfClass:NSDictionary.class])
                msime_macos_diagnostic_write("stats: record_keys_failed reason=malformed_response");
            else if (!MSIMEStrictBoolean(envelope[@"ok"]))
                msime_macos_diagnostic_write("stats: record_keys_failed reason=store");
        }
    });
}

static NSString *MSIMETranslationProviderScope(NSString *service, NSDictionary *configuration);

static BOOL MSIMEOnlineHasAIInput(NSDictionary *online) {
    if ([online[@"mode"] isEqual:@"english"])
        return MSIMEStrictBoolean(online[@"ai_eligible"]) && [online[@"query_text"] isKindOfClass:NSString.class] && [online[@"query_text"] length];
    return [online[@"pinyin_segments"] isKindOfClass:NSArray.class] && [online[@"pinyin_segments"] count];
}

static NSString *MSIMEAICacheKey(NSDictionary *online, NSDictionary *descriptor) {
    NSDictionary *config = online[@"ai_assistant"];
    NSArray *segments = [online[@"pinyin_segments"] isKindOfClass:NSArray.class] ? online[@"pinyin_segments"] : @[];
    if (![config isKindOfClass:NSDictionary.class] || !MSIMEStrictBoolean(config[@"enabled"]) ||
        !MSIMEOnlineHasAIInput(online) ||
        ![descriptor isKindOfClass:NSDictionary.class] ||
        ![NSJSONSerialization isValidJSONObject:segments]) return nil;
    NSString *descriptorScope = MSIMETranslationProviderScope(@"ai", descriptor);
    if (!descriptorScope.length) return nil;
    NSString *context = [online[@"ai_context"] isKindOfClass:NSString.class] ? online[@"ai_context"] : @"";
    NSData *contextBytes = [context dataUsingEncoding:NSUTF8StringEncoding];
    if (!contextBytes || contextBytes.length > UINT32_MAX) return nil;
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(contextBytes.bytes, (CC_LONG)contextBytes.length, digest);
    NSString *contextHash = [[NSData dataWithBytes:digest length:sizeof(digest)] base64EncodedStringWithOptions:0];
    NSDictionary *identity = @{ @"provider": [config[@"provider"] isKindOfClass:NSString.class] ? config[@"provider"] : @"",
        @"endpoint": [config[@"endpoint"] isKindOfClass:NSString.class] ? config[@"endpoint"] : @"",
        @"model": [config[@"model"] isKindOfClass:NSString.class] ? config[@"model"] : @"",
        @"candidate_limit": [config[@"candidate_limit"] isKindOfClass:NSNumber.class] ? config[@"candidate_limit"] : @3,
        @"prompt_id": [config[@"prompt_id"] isKindOfClass:NSString.class] ? config[@"prompt_id"] : @"",
        @"prompt_custom_1": [config[@"prompt_custom_1"] isKindOfClass:NSString.class] ? config[@"prompt_custom_1"] : @"",
        @"prompt_custom_2": [config[@"prompt_custom_2"] isKindOfClass:NSString.class] ? config[@"prompt_custom_2"] : @"",
        @"prompt_custom_3": [config[@"prompt_custom_3"] isKindOfClass:NSString.class] ? config[@"prompt_custom_3"] : @"",
        @"descriptor_scope": descriptorScope,
        @"ai_context_sha256": contextHash,
        @"mode": online[@"mode"] ?: @"pinyin",
        @"input": online[@"query_text"] ?: @"",
        @"pinyin_segments": segments ?: @[] };
    NSData *data = [NSJSONSerialization dataWithJSONObject:identity options:0 error:nil];
    return data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : nil;
}

static NSInteger MSIMEStrictInteger(id value, NSInteger fallback) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) return fallback;
    NSNumber *number = (NSNumber *)value;
    NSInteger integer = number.integerValue;
    return [number compare:@(integer)] == NSOrderedSame ? integer : fallback;
}

static BOOL MSIMEStrictUnsignedInteger(id value, uint64_t *result) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) return NO;
    NSNumber *number = (NSNumber *)value;
    if ([number compare:@0] == NSOrderedAscending) return NO;
    uint64_t integer = number.unsignedLongLongValue;
    if ([number compare:@(integer)] != NSOrderedSame) return NO;
    if (result) *result = integer;
    return YES;
}

// JSON booleans arrive as CFBoolean-backed NSNumbers. NSNumber's boolValue also
// treats every nonzero number as true, which would let malformed protocol values
// such as 1 or "true" take the success path.
static BOOL MSIMEStrictBoolean(id value) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) != CFBooleanGetTypeID()) return NO;
    return CFBooleanGetValue((CFBooleanRef)(__bridge CFTypeRef)value);
}

static BOOL MSIMEStrictUnsignedIntegerValue(id value, NSUInteger *result) {
    uint64_t integer = 0;
    if (!MSIMEStrictUnsignedInteger(value, &integer) || integer > NSUIntegerMax) return NO;
    if (result) *result = (NSUInteger)integer;
    return YES;
}

// 视图里的 `source` 是 Engine 的 CandidateSource（AiSuggestion = 3），不是 apply 接口的来源编号（0 云、1 AI）；按 1 判断会把用户词库的词当成 AI 候选，候选里有用户词时就跳过 AI 缓存（#6888）。旧的 AI 占位行（12）也不算 AI 候选。
static BOOL MSIMEViewContainsAICandidate(const shurufa::mac::ViewState &view) {
    for (const auto &candidate : view.candidates)
        if (candidate.source == 3) return YES;
    return NO;
}

// Engine 的在线占位行：AI 来源 12 与云来源 15 都不是可上屏候选，画法留着，旧数据或异步请求期间出现时按占位行画。
static BOOL MSIMECandidateIsAIPlaceholder(NSDictionary *candidate) {
    if (![candidate isKindOfClass:NSDictionary.class]) return NO;
    const NSInteger source = MSIMEStrictInteger(candidate[@"source"], -1);
    return source == 12 || source == 15;
}

static msime::mac::TypingSource MSIMEResolveTypingSource(NSDictionary *context, const shurufa::mac::ViewState &view,
                                                          NSDictionary *hostOptions, BOOL englishMode) {
    NSInteger strictScheme = NSIntegerMin;
    NSString *localMode = @"none";
    if ([context isKindOfClass:NSDictionary.class]) {
        strictScheme = MSIMEStrictInteger(context[@"scheme"], NSIntegerMin);
        if ([context[@"local_mode"] isKindOfClass:NSString.class]) localMode = context[@"local_mode"];
    } else if (view.valid) {
        strictScheme = view.scheme;
        localMode = view.localMode.length ? view.localMode : @"none";
    }
    if (strictScheme == NSIntegerMin) return msime::mac::TypingSource::Unknown;
    const BOOL nineKey = view.nineKey;
    BOOL dedicatedEnglish = view.dedicatedEnglish || englishMode;
    NSDictionary *preferences = [hostOptions[@"preferences"] isKindOfClass:NSDictionary.class] ? hostOptions[@"preferences"] : @{};
    NSString *profile = view.shuangpinProfile.length ? view.shuangpinProfile : preferences[@"shuangpin_profile"];
    if (![profile isKindOfClass:NSString.class]) profile = @"xiaohe";
    return msime::mac::ResolveTypingSource((int)strictScheme, nineKey, dedicatedEnglish,
        localMode.UTF8String ?: "none", profile.UTF8String ?: "xiaohe");
}

static msime::mac::TypingSource MSIMEResolveTypingSourceFromSnapshot(
    const ShurufaOutputContext *context, BOOL hasContext, const shurufa::mac::ViewState &view,
    NSDictionary *hostOptions, BOOL englishMode) {
    NSInteger strictScheme = NSIntegerMin;
    NSString *localMode = @"none";
    if (hasContext && context) {
        strictScheme = context->scheme;
        localMode = shurufa::mac::SliceToNSString(context->local_mode, @"none");
    } else if (view.valid) {
        strictScheme = view.scheme;
        localMode = view.localMode.length ? view.localMode : @"none";
    }
    if (strictScheme == NSIntegerMin) return msime::mac::TypingSource::Unknown;
    const BOOL nineKey = view.nineKey;
    BOOL dedicatedEnglish = view.dedicatedEnglish || englishMode;
    NSDictionary *preferences = [hostOptions[@"preferences"] isKindOfClass:NSDictionary.class] ? hostOptions[@"preferences"] : @{};
    NSString *profile = view.shuangpinProfile.length ? view.shuangpinProfile : preferences[@"shuangpin_profile"];
    if (![profile isKindOfClass:NSString.class]) profile = @"xiaohe";
    return msime::mac::ResolveTypingSource((int)strictScheme, nineKey, dedicatedEnglish,
        localMode.UTF8String ?: "none", profile.UTF8String ?: "xiaohe");
}

static NSDictionary *MSIMEStatisticsHostOptions(MSIMEClientSession *session) {
    return [session respondsToSelector:@selector(hostOptions)] ? session.hostOptions : @{};
}

// A view or commit context says itself whether Simplified-to-Traditional conversion applies to its text (`script_conversion`): the runtime decides it from the scheme and the local mode the text was made in. The runtime always writes the field and this host builds no view or commit context of its own, so a dictionary without it did not come from the runtime and its text is left as it is.
static BOOL MSIMEScriptConversionApplies(id value) {
    return [value isKindOfClass:NSDictionary.class] && MSIMEStrictBoolean(value[@"script_conversion"]);
}
static BOOL MSIMEScriptConversionAppliesState(const shurufa::mac::ViewState &view) {
    return view.valid && view.scriptConversion;
}

// commit_context.typing_statistics is false for text the expression, command and mention modes produced: a result the Engine worked out, not something the user typed. A context without the field predates it and counts.
static BOOL MSIMECommitCountsAsTyping(id context) {
    if (![context isKindOfClass:NSDictionary.class]) return YES;
    id counts = context[@"typing_statistics"];
    return ![counts isKindOfClass:NSNumber.class] || MSIMEStrictBoolean(counts);
}

static BOOL MSIMESpellingSymbol(const shurufa::mac::ViewState &view, unichar character) {
    return view.spellingSymbol(character);
}
static BOOL MSIMESpellingSymbolString(const shurufa::mac::ViewState &view, NSString *characters) {
    return view.spellingSymbolString(characters);
}

// 候选字后面的注释（辅助码、五笔编码提示、emoji 说明、地名所属省份……）一律包在半角括号里、紧贴候选字，画成「你好(wqvb)」；已经带括号的原样保留，空串不显示。
static NSString *MSIMEParenthesizedAnnotation(id value) {
    if (![value isKindOfClass:NSString.class]) return @"";
    NSString *annotation = [(NSString *)value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (annotation.length == 0) return @"";
    const unichar first = [annotation characterAtIndex:0], last = [annotation characterAtIndex:annotation.length - 1];
    if ((first == '(' && last == ')') || (first == 0xFF08 && last == 0xFF09)) return annotation;
    return [NSString stringWithFormat:@"(%@)", annotation];
}

// 固定排位候选的判定：0 表示不固定，255 是视图哨兵值。按钮着色与角标共用这一份。
static BOOL MSIMECandidateIsFixedPosition(id fixed) {
    return MSIMEStrictUnsignedInteger(fixed, nullptr) && [fixed compare:@0] == NSOrderedDescending &&
           [fixed compare:@255] != NSOrderedDescending;
}

// 纯逻辑徽标函数返回的 UTF-8 串转成 NSString；空串返回 @""。
static NSString *MSIMECandidateBadgeString(const std::string &badge) {
    if (badge.empty()) return @"";
    return [[NSString alloc] initWithBytes:badge.data() length:badge.size() encoding:NSUTF8StringEncoding] ?: @"";
}

// 装饰词与普通候选共用来源徽标；固定排位仍由普通候选另外追加。
static NSString *CandidateSourceBadge(NSDictionary *candidate, NSString *aiBadge) {
    id source = candidate[@"source"];
    if (![source isKindOfClass:NSNumber.class] || CFGetTypeID((__bridge CFTypeRef)source) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)source)) return @"";
    if ([source isEqual:@2]) return @" ☁️";
    if ([source isEqual:@3]) return aiBadge ?: MSIMECandidateBadgeString(msime::mac::candidate_ai_badge("robot"));
    return @"";
}

// 纠错候选已在字面末尾标出 `*`；未开辅助码时引擎在 `annotation` 回传折叠后的整串原始按键（`corrected_from`），
// 与上方预编辑串重复且在长句中会把候选窗撑到整串拼音宽度，显示层省略该回显，保留带括号的辅助码与五笔反查等真实注释。
static BOOL MSIMEIsLowercaseASCIIToken(NSString *value) {
    if (![value isKindOfClass:NSString.class] || value.length == 0) return NO;
    for (NSUInteger i = 0; i < value.length; ++i) {
        const unichar ch = [value characterAtIndex:i];
        if (ch < 'a' || ch > 'z') return NO;
    }
    return YES;
}

static NSString *MSIMEVisibleCandidateAnnotation(NSDictionary *candidate) {
    if (![candidate isKindOfClass:NSDictionary.class]) return @"";
    id raw = candidate[@"annotation"];
    if (![raw isKindOfClass:NSString.class]) return @"";
    NSString *trimmed = [(NSString *)raw stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (trimmed.length == 0) return @"";
    if (!MSIMEStrictBoolean(candidate[@"corrected"])) return (NSString *)raw;
    if (MSIMEIsLowercaseASCIIToken(trimmed)) return @"";
    NSRange space = [trimmed rangeOfString:@" "];
    if (space.location != NSNotFound) {
        NSString *head = [trimmed substringToIndex:space.location];
        NSString *tail = [[trimmed substringFromIndex:NSMaxRange(space)] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        if (MSIMEIsLowercaseASCIIToken(head) && tail.length > 0) return tail;
    }
    return (NSString *)raw;
}

// 画候选文字（含来源与固定排位的角标）。徽标串由调用方按偏好算好：AI 来源用它替换写死的 🤖，
// 固定排位追加在来源角标之后，与 Windows candidate_view_model.h 的 text+annotation+badge 顺序一致。
static NSString *CandidateDisplayWithBadges(NSDictionary *candidate, BOOL traditional, NSString *aiBadge, NSString *fixedBadge) {
    // 未提供缓存时回退出厂徽标；显式空串保留关闭状态，不把 nil 传给字符串拼接。
    aiBadge = aiBadge ?: MSIMECandidateBadgeString(msime::mac::candidate_ai_badge("robot"));
    fixedBadge = fixedBadge ?: MSIMECandidateBadgeString(msime::mac::candidate_fixed_badge(true, "paperclip"));
    NSString *annotation = MSIMEParenthesizedAnnotation(MSIMEVisibleCandidateAnnotation(candidate));
    NSString *text = candidate[@"text"] ?: @"";
    id corrected = candidate[@"corrected"];
    if (MSIMEStrictBoolean(corrected)) text = [text stringByAppendingString:@"*"];
    if (annotation.length) text = [text stringByAppendingString:annotation];
    id source = candidate[@"source"];
    const BOOL integerSource = [source isKindOfClass:NSNumber.class] &&
                               CFGetTypeID((__bridge CFTypeRef)source) != CFBooleanGetTypeID() &&
                               !CFNumberIsFloatType((__bridge CFNumberRef)source);
    // 来源 6 是 Emoji/符号，来源 13 是日文，来源 18 已由运行时逐段转换；都不应再次整行简转繁。
    if (!(integerSource && ([source isEqual:@6] || [source isEqual:@13] || [source isEqual:@18]))) text = MSIMEChineseOutputString(text, traditional);
    // 来源与固定排位角标只参与显示，不改变候选身份或上屏内容。
    NSString *badge = CandidateSourceBadge(candidate, aiBadge);
    if (fixedBadge.length && MSIMECandidateIsFixedPosition(candidate[@"fixed_position"]))
        badge = [badge stringByAppendingString:fixedBadge];
    return [text stringByAppendingString:badge];
}

// 候选窗首行拼音后面的输入状态：方案名，然后中文组字时是「中」、英文候选（专用英文或临时英文）时是「英」，简繁转换适用于这一页时（`view.script_conversion`）加「简」或「繁」，全角打开时加「全角」，用「 · 」隔开。方案名由调用方按 `view.scheme` 算好，空时不写。
static NSString *MSIMECandidateStatusText(NSString *schemeTitle, const shurufa::mac::ViewState &view, BOOL traditional, BOOL fullWidth) {
    NSMutableArray<NSString *> *parts = [NSMutableArray array];
    if ([schemeTitle isKindOfClass:NSString.class] && schemeTitle.length) [parts addObject:schemeTitle];
    NSString *localMode = view.localMode.length ? view.localMode : @"none";
    const BOOL english = view.dedicatedEnglish || [localMode isEqual:@"temporary_english"];
    if (!MSIMEEditionIsShurufa()) [parts addObject:english ? @"英" : @"中"];
    if (!english && view.scriptConversion) [parts addObject:traditional ? @"繁" : @"简"];
    if (fullWidth) [parts addObject:@"全角"];
    return [parts componentsJoinedByString:@" · "];
}

[[maybe_unused]] static NSString *MSIMECandidateStatusText(NSString *schemeTitle, id view, BOOL traditional, BOOL fullWidth) {
    return MSIMECandidateStatusText(schemeTitle, shurufa::mac::ViewState::fromDictionary((NSDictionary *)view), traditional, fullWidth);
}

// 光标后编辑（偏好 `caret_edit_suffix`）生效时画在候选窗首行右侧的提示，只是标明状态，不能点。
static NSString *const MSIMECaretEditSuffixHint = @"候选编辑：光标后";

// 视图的 `caret_edit_suffix`：只在光标后编辑生效时出现。
static BOOL MSIMECaretEditSuffixActive(const shurufa::mac::ViewState &view) {
    return view.valid && view.caretEditSuffix;
}
[[maybe_unused]] static BOOL MSIMECaretEditSuffixActive(id view) {
    return MSIMECaretEditSuffixActive(shurufa::mac::ViewState::fromDictionary((NSDictionary *)view));
}

// 视图的 `caret_edit_prefix`：光标后编辑里首选开头固定的那段。
static NSString *MSIMECaretEditPrefix(const shurufa::mac::ViewState &view) {
    if (!MSIMECaretEditSuffixActive(view)) return @"";
    return view.caretEditPrefix ?: @"";
}
[[maybe_unused]] static NSString *MSIMECaretEditPrefix(id view) {
    return MSIMECaretEditPrefix(shurufa::mac::ViewState::fromDictionary((NSDictionary *)view));
}

// `offset` 处这个 Unicode 标量占几个 UTF-16 码元：完整的代理对是 2，其余（含落单的代理项）是 1。
static NSUInteger MSIMEScalarUTF16Width(NSString *text, NSUInteger offset) {
    return offset + 1 < text.length && CFStringIsSurrogateHighCharacter([text characterAtIndex:offset]) &&
        CFStringIsSurrogateLowCharacter([text characterAtIndex:offset + 1]) ? 2 : 1;
}

// 候选格画出来的文字 `text` 开头要画淡的 UTF-16 长度。
static NSUInteger MSIMECaretEditDimmedLength(const shurufa::mac::ViewState &view, NSDictionary *candidate, NSString *text) {
    NSString *prefix = MSIMECaretEditPrefix(view);
    if (!prefix.length || ![candidate isKindOfClass:NSDictionary.class] || ![text isKindOfClass:NSString.class]) return 0;
    NSDictionary *identifier = candidate[@"id"];
    NSUInteger index = 0;
    if (![identifier isKindOfClass:NSDictionary.class] || !MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &index) || index != 0)
        return 0;
    NSUInteger scalars = 0;
    for (NSUInteger offset = 0; offset < prefix.length; ++scalars) offset += MSIMEScalarUTF16Width(prefix, offset);
    NSUInteger length = 0;
    for (NSUInteger counted = 0; counted < scalars && length < text.length; ++counted) length += MSIMEScalarUTF16Width(text, length);
    return length;
}
[[maybe_unused]] static NSUInteger MSIMECaretEditDimmedLength(id view, NSDictionary *candidate, NSString *text) {
    return MSIMECaretEditDimmedLength(shurufa::mac::ViewState::fromDictionary((NSDictionary *)view), candidate, text);
}

// 这一页里第一条云候选（来源 2），没有时取 AI 候选（来源 3），首行右侧预览它；AI 占位行和空文本不算。两种都没有时为 nil。
static NSDictionary *MSIMECandidateOnlinePreview(NSArray *candidates) {
    NSDictionary *ai = nil;
    for (NSDictionary *candidate in candidates) {
        if (![candidate isKindOfClass:NSDictionary.class] || MSIMECandidateIsAIPlaceholder(candidate)) continue;
        NSString *text = candidate[@"text"];
        if (![text isKindOfClass:NSString.class] || !text.length || ![candidate[@"id"] isKindOfClass:NSDictionary.class]) continue;
        const NSInteger source = MSIMEStrictInteger(candidate[@"source"], -1);
        if (source == 2) return candidate;
        if (source == 3 && !ai) ai = candidate;
    }
    return ai;
}

static NSString *MSIMEWubiCodeHint(NSDictionary *candidate, const shurufa::mac::ViewState &view, BOOL enabled) {
    if (![candidate isKindOfClass:NSDictionary.class] || !view.valid) return @"";
    NSString *code = candidate[@"code"];
    NSString *typed = view.effectivePreedit();
    NSString *localMode = view.localMode.length ? view.localMode : @"none";
    if (![code isKindOfClass:NSString.class] || ![typed isKindOfClass:NSString.class]) return @"";
    const std::string codeUTF8 = code.UTF8String ? code.UTF8String : "";
    const std::string typedUTF8 = typed.UTF8String ? typed.UTF8String : "";
    const std::string hint = msime::mac::WubiCodeHint(codeUTF8, typedUTF8, enabled,
                                                       (int)view.scheme,
                                                       localMode.UTF8String ?: "none",
                                                       false);
    return hint.empty() ? @"" : [[NSString alloc] initWithBytes:hint.data() length:hint.size() encoding:NSUTF8StringEncoding];
}

static NSString *CandidateDisplayWithWubiHint(NSDictionary *candidate, BOOL traditional, NSString *hint, NSString *aiBadge, NSString *fixedBadge) {
    if (![hint isKindOfClass:NSString.class] || hint.length == 0) return CandidateDisplayWithBadges(candidate, traditional, aiBadge, fixedBadge);
    NSMutableDictionary *annotated = [candidate mutableCopy];
    annotated[@"annotation"] = [NSString stringWithFormat:@"(%@)", hint];
    return CandidateDisplayWithBadges(annotated, traditional, aiBadge, fixedBadge);
}

// The panel draws the 辅助码 (or the engine's annotation) as a run of its own after the candidate text, so it can move under a text that leaves it no room, as the Windows presenter does. The tooltip and accessibility label keep the combined CandidateDisplayWithWubiHint form.
static NSString *CandidateTextRun(NSDictionary *candidate, BOOL traditional, NSString *aiBadge, NSString *fixedBadge) {
    if (candidate[@"annotation"] == nil) return CandidateDisplayWithBadges(candidate, traditional, aiBadge, fixedBadge);
    NSMutableDictionary *plain = [candidate mutableCopy];
    [plain removeObjectForKey:@"annotation"];
    return CandidateDisplayWithBadges(plain, traditional, aiBadge, fixedBadge);
}

static NSString *CandidateAnnotationRun(NSDictionary *candidate, BOOL traditional, NSString *hint) {
    NSString *annotation = [hint isKindOfClass:NSString.class] && hint.length ? [NSString stringWithFormat:@"(%@)", hint]
                                                                              : MSIMEVisibleCandidateAnnotation(candidate);
    if (![annotation isKindOfClass:NSString.class] || annotation.length == 0) return @"";
    return MSIMEChineseOutputString(annotation, traditional);
}

// One page of candidate rows laid out at the card width they get: what the panel draws, what its buttons answer clicks in, and what the keymap panel keeps clear of.
struct MSIMECandidatePageGeometry {
    std::vector<msime::candidate::CandidateRowLayout> rows;
    std::vector<double> naturalColumns;
    std::vector<double> unnumberedColumns;
    std::vector<double> numberWidths;
    CGFloat minimumColumnWidth = 0;
    CGFloat columnGap = 0;
    NSArray<NSString *> *texts = @[];
    NSArray<NSString *> *annotations = @[];
    NSArray<NSString *> *displays = @[];
    // 每个候选按偏好整理后的释义（不含读音与插件注释），按钮绘制取它；测量仍用循环里拼好的完整释义行。
    NSArray<NSString *> *translations = @[];
    // The reading each row draws on its gloss run above any translation (a Korean Hanja's 훈음, empty elsewhere), and the tooltip that shows the row's text with everything under it.
    NSArray<NSString *> *readings = @[];
    // 每一行脚本插件的注释（MSIMECandidateScriptAnnotationRun 收拾过的），没有时为空串；它算进释义行的量高，由按钮画在释义行的最后一行。
    NSArray<NSString *> *scriptAnnotations = @[];
    NSArray<NSString *> *tooltips = @[];
    // Card width, the width the rows share inside it, the height they stack to, and the x of the candidate text inside a row.
    CGFloat width = 0;
    CGFloat lineWidth = 0;
    CGFloat rowsHeight = 0;
    CGFloat contentLeft = 0;
    BOOL hasSkinLayout = NO;
    BOOL hidesNumber = NO;
    CGFloat textLeft = 0;
    CGFloat textRight = 0;
    CGFloat selectedBarWidth = 3;
    CGFloat selectedBarHeight = 0;
    BOOL selectedBarInside = NO;
};

// 展开网格的逐行几何，连同算它用到的输入。移动光标只换高亮和视口，输入都对得上时整份沿用：
// 单字母输入能有几千个候选，每移一步都把它们重测一遍要上百毫秒。
struct MSIMEExpandedGridGeometry {
    NSArray *entries = nil;
    NSFont *font = nil;
    NSFont *glossFont = nil;
    std::string skin;
    BOOL dark = NO;
    CGFloat scale = 0;
    CGFloat inset = 0;
    CGFloat toggleColumn = 0;
    CGFloat rowVisibleWidth = 0;
    NSUInteger columns = 0;
    std::shared_ptr<std::vector<MSIMECandidatePageGeometry>> layouts;
    std::vector<CGFloat> rowOffsets;
    CGFloat rowsWidth = 0;

    bool sameInputs(const MSIMEExpandedGridGeometry &other) const {
        return entries == other.entries && [font isEqual:other.font] && [glossFont isEqual:other.glossFont] &&
            skin == other.skin && dark == other.dark && scale == other.scale &&
            inset == other.inset && toggleColumn == other.toggleColumn && rowVisibleWidth == other.rowVisibleWidth &&
            columns == other.columns;
    }
};

static CGFloat MSIMESkinLengthValue(const std::optional<msime::mac::SkinLength> &length, CGFloat fontSize,
                                    CGFloat scale, CGFloat fallback = 0.0) {
    if (!length) return fallback;
    const double value = length->em ? std::round(length->value * fontSize) : length->value;
    return std::isfinite(value) && value >= 0.0 ? static_cast<CGFloat>(value * (length->em ? 1.0 : scale)) : fallback;
}

// 预编辑外边距只在有文字时参与；canvas 显式行距替换下外边距，稍后由画布统一加到行间。
static msime::mac::SkinInsets MSIMECandidatePreeditMargins(const msime::mac::ResolvedSkin &skin, BOOL visible, CGFloat scale) {
    msime::mac::SkinInsets margin;
    if (!visible || !skin.layout || !skin.layout->preeditMarginDip) return margin;
    margin = *skin.layout->preeditMarginDip;
    margin.top *= scale;
    margin.right *= scale;
    margin.bottom *= scale;
    margin.left *= scale;
    if (skin.canvas && skin.canvas->lineGap) margin.bottom = 0;
    return margin;
}

// 来源 body 的基础行距为 2 DIP；canvas 显式行距替换它，收紧或贴角只消除隐藏预编辑的行距。
static CGFloat MSIMECandidateBodyRowGap(const msime::mac::ResolvedSkin &skin, BOOL visible, CGFloat scale) {
    if (!skin.layout) return 0;
    if (visible) {
        if (skin.canvas && skin.canvas->lineGap.has_value()) return 0;
    } else if (skin.layout->trimHiddenPreeditGap.value_or(false) ||
               skin.layout->itemCornersFollowCard.value_or(false)) return 0;
    return 2.0 * scale;
}

// 释义每个目标语言占一行，宿主内部用 "\n" 分行；共享会话每个候选只存一条不含控制字符的释义，msime_client_apply_translations 遇到 "\n" 会把整批释义拒收，设了第二释义语言后候选下一条释义都不显示（#5598）。所以交给会话时把行分隔换成 U+2028 LINE SEPARATOR（它不是控制字符），读回时在 CandidateTranslation 这唯一的读取处换回 "\n"。
static NSString *const MSIMEGlossLineSeparator = @"\u2028";

static NSString *CandidateTranslation(NSDictionary *candidate) {
    id text = candidate[@"translation"];
    return [text isKindOfClass:NSString.class] ? [text stringByReplacingOccurrencesOfString:MSIMEGlossLineSeparator withString:@"\n"] : @"";
}

// 绘制用的释义：按 `candidate_translation_show_pos` / `candidate_translation_single_sense` 整理词性标记与义项。
// 上屏的义项仍取未整理的 CandidateTranslation（Ctrl+Enter 的释义页、Ctrl+数字的释义列都不受这里影响）。
static NSString *CandidateDisplayTranslation(NSDictionary *candidate, BOOL showPos, BOOL singleSense) {
    NSString *gloss = CandidateTranslation(candidate);
    if (gloss.length == 0) return @"";
    const char *utf8 = gloss.UTF8String;
    const std::string formatted = msime::mac::format_translation_for_display(utf8 ? utf8 : "", showPos, singleSense);
    NSString *display = [[NSString alloc] initWithBytes:formatted.data() length:formatted.size() encoding:NSUTF8StringEncoding];
    return display ?: gloss;
}

// 交给会话的释义。来源自带的 U+2028、U+2029 先折成空格：词典、账号和自定义接口都可能原样带着它，不折掉的话读回时会多出一行，第二语言的释义被挤到第三行，按列上屏也会取错列。
static NSArray<NSDictionary *> *MSIMESessionTranslations(NSArray<NSDictionary *> *results) {
    NSMutableArray<NSDictionary *> *session = [NSMutableArray arrayWithCapacity:results.count];
    for (NSDictionary *entry in results) {
        NSString *translation = [entry isKindOfClass:NSDictionary.class] ? entry[@"translation"] : nil;
        if (![translation isKindOfClass:NSString.class]) { [session addObject:entry]; continue; }
        NSString *flat = [[translation stringByReplacingOccurrencesOfString:MSIMEGlossLineSeparator withString:@" "]
            stringByReplacingOccurrencesOfString:@"\u2029" withString:@" "];
        NSMutableDictionary *encoded = [entry mutableCopy];
        encoded[@"translation"] = [flat stringByReplacingOccurrencesOfString:@"\n" withString:MSIMEGlossLineSeparator];
        [session addObject:encoded];
    }
    return session;
}

static NSString *MSIMECandidateTranslationColumn(NSDictionary *candidate, NSInteger column) {
    if (column <= 0) return @"";
    NSArray<NSString *> *parts = [CandidateTranslation(candidate) componentsSeparatedByString:@"\n"];
    NSUInteger index = (NSUInteger)(column - 1);
    return index < parts.count && [parts[index] isKindOfClass:NSString.class] ? parts[index] : @"";
}

static NSSize MSIMETranslationTextSize(NSString *text, NSFont *font) {
    if (!text.length) return NSZeroSize;
    NSRect bounds = [text boundingRectWithSize:NSMakeSize(CGFLOAT_MAX, CGFLOAT_MAX)
        options:NSStringDrawingUsesLineFragmentOrigin
        attributes:@{NSFontAttributeName:font}];
    return NSMakeSize(ceil(bounds.size.width), ceil(bounds.size.height));
}

static NSArray<NSString *> *MSIMETranslationTargets(NSDictionary *query) {
    NSArray *supported = @[@"en", @"fr", @"ja", @"es", @"ru", @"de", @"ko"];
    NSMutableArray<NSString *> *targets = [NSMutableArray arrayWithCapacity:2];
    NSArray *raw = [query[@"target_languages"] isKindOfClass:NSArray.class] ? query[@"target_languages"] : @[];
    for (id value in raw) {
        if (![value isKindOfClass:NSString.class] || ![(NSString *)value length] || ![supported containsObject:value] || [targets containsObject:value]) continue;
        [targets addObject:(NSString *)value];
    }
    NSString *primary = [query[@"target_language"] isKindOfClass:NSString.class] ? query[@"target_language"] : nil;
    if ([supported containsObject:primary]) {
        [targets removeObject:primary];
        [targets insertObject:primary atIndex:0];
    }
    return targets.count ? [targets copy] : @[];
}

// 翻译查询列出的候选文字。候选词翻译范围是「仅当前高亮」时共享查询的 candidates 只有高亮的那一个，翻译服务和离线词典都只为它工作，所以按视图组请求时只留这些文字。查询没列 candidates（替身会话）时为 nil，不限制。
static NSSet<NSString *> *MSIMEQueryCandidateTexts(NSDictionary *query) {
    id raw = [query isKindOfClass:NSDictionary.class] ? query[@"candidates"] : nil;
    if (![raw isKindOfClass:NSArray.class]) return nil;
    NSMutableSet<NSString *> *texts = [NSMutableSet set];
    for (NSDictionary *candidate in (NSArray *)raw)
        if ([candidate isKindOfClass:NSDictionary.class] && [candidate[@"text"] isKindOfClass:NSString.class]) [texts addObject:candidate[@"text"]];
    return texts;
}

// 「仅当前高亮」时查询另带的 gloss_query：覆盖整页、不带任何服务的离线释义查询（msime_client_translation_query），和主查询同一代次。它只走离线词典，回答每条标 "gloss": true。
static NSDictionary *MSIMEGlossQuery(NSDictionary *query) {
    id gloss = [query isKindOfClass:NSDictionary.class] ? query[@"gloss_query"] : nil;
    return [gloss isKindOfClass:NSDictionary.class] && [gloss[@"generation"] isEqual:query[@"generation"]] ? gloss : nil;
}

static NSArray<NSString *> *MSIMETranslationTargetsFromPreferences(NSDictionary *preferences, NSString *fallback) {
    NSArray *supported = @[@"en", @"fr", @"ja", @"es", @"ru", @"de", @"ko"];
    NSString *primary = [preferences[@"translation_target_language"] isKindOfClass:NSString.class]
        ? preferences[@"translation_target_language"] : fallback;
    NSMutableArray<NSString *> *targets = [NSMutableArray array];
    if ([supported containsObject:primary]) [targets addObject:primary];
    id secondary = preferences[@"translation_secondary_language"];
    if ([supported containsObject:secondary] && ![targets containsObject:secondary])
        [targets addObject:secondary];
    return [targets copy];
}

static NSString *MSIMETranslationWorkKey(NSString *target, NSString *text) {
    return [NSString stringWithFormat:@"%@\u001f%@", target ?: @"", text ?: @""];
}

// The cache outlives input controllers. Include the selected provider's full
// configuration so a credential changed while no controller existed cannot
// reuse an earlier provider's results, without retaining the credential itself.
static NSString *MSIMETranslationProviderScope(NSString *service, NSDictionary *configuration) {
    NSData *encoded = [NSJSONSerialization dataWithJSONObject:configuration options:NSJSONWritingSortedKeys error:nil];
    if (!encoded || encoded.length > UINT32_MAX) return nil;
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(encoded.bytes, (CC_LONG)encoded.length, digest);
    NSString *fingerprint = [[NSData dataWithBytes:digest length:sizeof(digest)] base64EncodedStringWithOptions:0];
    return [NSString stringWithFormat:@"%@:%@", service, fingerprint];
}

static NSString *MSIMEJoinedTranslations(NSDictionary<NSString *, NSString *> *values,
                                          NSArray<NSString *> *targets) {
    NSMutableArray<NSString *> *ordered = [NSMutableArray array];
    for (NSString *target in targets) [ordered addObject:values[target] ?: @""];
    while (ordered.count && ![ordered.lastObject length]) [ordered removeLastObject];
    BOOL hasValue = NO;
    for (NSString *value in ordered) if (value.length) { hasValue = YES; break; }
    return hasValue ? [ordered componentsJoinedByString:@"\n"] : @"";
}

static NSColor *SkinColor(msime::mac::Rgba color) {
    return [NSColor colorWithSRGBRed:color.r green:color.g blue:color.b alpha:color.a];
}

// 只利用取光标矩形时已经返回的颜色，不读取文档或截屏；无有效颜色时继续跟随系统。
static NSAppearance *MSIMECandidateHostAppearance(NSDictionary *attributes) {
    if (![attributes isKindOfClass:NSDictionary.class]) return nil;
    for (NSString *key in @[NSBackgroundColorAttributeName, NSBackgroundColorDocumentAttribute, NSForegroundColorAttributeName]) {
        id value = attributes[key];
        if (![value isKindOfClass:NSColor.class]) continue;
        NSColor *color = [value colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
        if (!color || color.alphaComponent < 0.95) continue;
        CGFloat r = color.redComponent, g = color.greenComponent, b = color.blueComponent;
        if (!std::isfinite(r) || !std::isfinite(g) || !std::isfinite(b)) continue;
        const BOOL foreground = [key isEqual:NSForegroundColorAttributeName];
        // 彩色语法高亮不能代表整个输入区的背景，只有中性文字能作后备依据。
        if (foreground && MAX(r, MAX(g, b)) - MIN(r, MIN(g, b)) > 0.12) continue;
        auto linear = [](CGFloat v) { return v <= 0.04045 ? v / 12.92 : std::pow((v + 0.055) / 1.055, 2.4); };
        const double luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
        const BOOL dark = foreground ? luminance > 0.179 : luminance <= 0.179;
        return [NSAppearance appearanceNamed:dark ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua];
    }
    return nil;
}

// A theme colour for a floating panel that resolves in whichever appearance the panel is drawn in: the light palette's value in Aqua, the dark palette's in Dark Aqua.
static NSColor *MSIMEThemedSkinColor(NSString *name, msime::mac::Rgba lightColor, msime::mac::Rgba darkColor) {
    NSColor *lightValue = SkinColor(lightColor), *darkValue = SkinColor(darkColor);
    return [NSColor colorWithName:name dynamicProvider:^NSColor *(NSAppearance *appearance) {
        return [appearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]] == NSAppearanceNameDarkAqua ? darkValue : lightValue;
    }];
}

// candidate_scale_percent as a factor: the candidate window is laid out at 100% and every font and length of it is multiplied by this, so 150% is the same window half as large again rather than a larger font in the old frame.
static CGFloat MSIMECandidateScale(MSIMEAppearancePreferences *appearance) {
    return appearance.candidateScalePercent / 100.0;
}

// 装饰图有待机动效时四周留出的余量，让它动起来不碰到窗口边缘被裁掉；没有动效时照原样摆放。
static double MSIMEDecorationMotionMargin(const msime::mac::ResolvedSkin &skin) {
    return skin.decorationMotion != msime::mac::DecorationMotion::none ? msime::mac::kDecorationMotionMargin : 0.0;
}

// The reading in the candidate window's top row is set semibold (dc.html L1325) at the size the user picked for it. A resolved family is named by its face (Menlo-Regular), which pins the weight, so the face is swapped for the family and the fallback cascade is kept; a family whose nearest heavier face is bold draws bold, and one with no heavier face keeps its regular one.
static NSFont *MSIMECandidatePreeditFont(MSIMEAppearancePreferences *appearance) {
    const CGFloat size = appearance.preeditFontSize * MSIMECandidateScale(appearance);
    NSFont *regular = [appearance candidateFontOfSize:size englishFirst:YES];
    NSMutableDictionary *attributes = [regular.fontDescriptor.fontAttributes mutableCopy];
    if (attributes[NSFontNameAttribute] && regular.familyName) {
        [attributes removeObjectForKey:NSFontNameAttribute];
        attributes[NSFontFamilyAttribute] = regular.familyName;
    }
    attributes[NSFontTraitsAttribute] = @{NSFontWeightTrait: @(NSFontWeightSemibold)};
    return [NSFont fontWithDescriptor:[NSFontDescriptor fontDescriptorWithFontAttributes:attributes] size:size] ?: regular;
}

static NSUInteger MSIMECandidateDeletionSlot(NSEvent *event) {
    const NSEventModifierFlags required = NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagShift;
    if ((event.modifierFlags & (required | NSEventModifierFlagCommand)) != required) return NSNotFound;
    // Main-row physical digits only; Option/Shift characters vary by layout.
    const unsigned short codes[] = {18, 19, 20, 21, 23, 22, 26, 28};
    for (NSUInteger slot = 0; slot < 8; ++slot) if (event.keyCode == codes[slot]) return slot;
    return NSNotFound;
}
// 这次按键在当前键盘布局上对应的快捷键字母，规则见 `msime::mac::ShortcutLetter`。
static char MSIMEShortcutLetter(NSEvent *event) {
    return msime::mac::ShortcutLetter(event.keyCode, msime::mac::ToggleHotkeyCharacter(event));
}
// 事件是否命中一个可重绑切换热键：可打印主键按无修饰布局字符认，
// 修饰键四旗精确相等，与原来各分支的掩码语义一致。
static BOOL MSIMEToggleChordMatches(msime::mac::ToggleChord chord, NSEvent *event) {
    const NSEventModifierFlags flags = event.modifierFlags;
    return msime::mac::ToggleChordMatches(chord, event.keyCode, msime::mac::ToggleHotkeyCharacter(event),
        (flags & NSEventModifierFlagControl) != 0, (flags & NSEventModifierFlagOption) != 0,
        (flags & NSEventModifierFlagShift) != 0, (flags & NSEventModifierFlagCommand) != 0);
}
// The marks this host closes for the user, opening first. The reference keeps the same list in its
// TIP (`GetPairedPunctuationClosing`).
static NSArray<NSArray<NSString *> *> *MSIMEPunctuationPairs(void) {
    static NSArray<NSArray<NSString *> *> *pairs;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        // 『』〖〗是引号样式、方头括号样式（quote_style、bracket_style）下 `"` 和 `[` 的另一组字。
        pairs = @[@[@"（", @"）"], @[@"【", @"】"], @[@"《", @"》"], @[@"“", @"”"], @[@"‘", @"’"],
                  @[@"〈", @"〉"], @[@"「", @"」"], @[@"『", @"』"], @[@"〖", @"〗"]];
    });
    return pairs;
}

static BOOL MSIMEPairedPunctuationExcludedBundleIdentifier(NSString *identifier) {
    if (![identifier isKindOfClass:NSString.class]) return NO;
    return [identifier caseInsensitiveCompare:@"com.microsoft.Excel"] == NSOrderedSame;
}
static BOOL MSIMEPairedPunctuationExcludedHost(void) {
    NSRunningApplication *app = NSWorkspace.sharedWorkspace.frontmostApplication;
    return MSIMEPairedPunctuationExcludedBundleIdentifier(app.bundleIdentifier);
}
static BOOL MSIMECurrentCandidateIdentity(id identifier, const shurufa::mac::ViewState &view) {
    return view.matchesCandidateIdentity(identifier);
}

// 后台读取会强引用控制器。它的最后一次释放不能落在后台队列上，否则 -dealloc 会在主线程之外收起 AppKit 对象。这里接过调用方的引用并把它清空，再到主线程释放。
// 主线程上用 CFRelease 当场释放，不用 CFBridgingRelease：后者的返回值在未优化的构建里会进主线程 run loop 的自动释放池，控制器要活到这一轮回调结束，排在后面、按弱引用取控制器的完成块就会取到一个 IMK 早已放掉的控制器。
static void MSIMEReleaseControllerOnMain(__strong id *controller) {
    if (!*controller) return;
    CFTypeRef owner = CFBridgingRetain(*controller);
    *controller = nil;
    dispatch_async(dispatch_get_main_queue(), ^{
        CFRelease(owner);
    });
}

// Numeric and space selection must use the candidate identities captured by the
// panel that is actually on screen. AppKit can deliver another key event before
// the previous content view has painted, while _view already points at the next
// Engine generation. Falling back to _view in that window can select a different
// word than the one the user sees.
static NSDictionary *MSIMERenderedCandidateIdentity(NSPanel *panel, NSInteger slot) {
    if (!panel || ![panel.contentView isKindOfClass:NSView.class]) return nil;
    for (NSView *subview in MSIMECandidateControlSubviews(panel.contentView)) {
        if (![subview isKindOfClass:MSIMECandidateButton.class] || subview.tag != slot) continue;
        NSDictionary *identity = ((MSIMECandidateButton *)subview).candidateID;
        return [identity isKindOfClass:NSDictionary.class] ? identity : nil;
    }
    return nil;
}

static NSDictionary *MSIMERenderedHighlightedCandidateIdentity(NSPanel *panel) {
    if (!panel || ![panel.contentView isKindOfClass:NSView.class]) return nil;
    for (NSView *subview in MSIMECandidateControlSubviews(panel.contentView)) {
        if (![subview isKindOfClass:MSIMECandidateButton.class] || subview.tag < 0) continue;
        MSIMECandidateButton *button = (MSIMECandidateButton *)subview;
        if (!button.candidateHighlighted) continue;
        NSDictionary *identity = button.candidateID;
        return [identity isKindOfClass:NSDictionary.class] ? identity : nil;
    }
    return nil;
}

// Translation replies may replace the view while leaving every actionable field unchanged.
// Comparing the rest of the view also protects preedit, paging and candidate menu actions.
static BOOL MSIMEOnlyCandidateTranslationsChanged(const shurufa::mac::ViewState &before,
                                                  const shurufa::mac::ViewState &after) {
    return shurufa::mac::ViewState::onlyTranslationsChanged(before, after);
}

static BOOL MSIMESmartPunctuationKey(unichar character) {
    return character == ',' || character == '.' || character == ':';
}
static BOOL MSIMEASCIIAlphanumeric(unichar character) {
    return (character >= '0' && character <= '9') || (character >= 'A' && character <= 'Z') ||
           (character >= 'a' && character <= 'z');
}
// The scheme's own rules hold: dedicated English and the local modes keep their own rules inside any scheme, so a scheme trait only applies outside them.
static BOOL MSIMESchemeRulesApply(const shurufa::mac::ViewState &view) {
    return view.schemeRulesApply();
}
// The view's scheme number; a view without one reads as quanpin, the Engine's default scheme.
static int MSIMEViewScheme(const shurufa::mac::ViewState &view) {
    return view.valid ? (int)view.scheme : msime::mac::scheme::Quanpin;
}
[[maybe_unused]] static int MSIMEViewScheme(id view) {
    return MSIMEViewScheme(shurufa::mac::ViewState::fromDictionary((NSDictionary *)view));
}
// A scheme trait that holds while the view's scheme rules apply.
static BOOL MSIMESchemeTrait(const shurufa::mac::ViewState &view, bool (*trait)(int)) {
    return MSIMESchemeRulesApply(view) && trait(MSIMEViewScheme(view));
}
[[maybe_unused]] static BOOL MSIMESchemeTrait(id view, bool (*trait)(int)) {
    return MSIMESchemeTrait(shurufa::mac::ViewState::fromDictionary((NSDictionary *)view), trait);
}
// The scheme's openable candidate list (the Korean Hanja list) is showing, as the view reports it (msime_client.h, MSIME_OPEN_CANDIDATE_LIST).
static BOOL MSIMECandidateListOpen(const shurufa::mac::ViewState &view) {
    return view.valid && view.candidateListOpen;
}
[[maybe_unused]] static BOOL MSIMECandidateListOpen(id view) {
    return MSIMECandidateListOpen(shurufa::mac::ViewState::fromDictionary((NSDictionary *)view));
}
// The opened list is the Korean Hanja list, whose rows carry each Hanja's 훈음 as their annotation; a Zhuyin list carries none.
static BOOL MSIMEKoreanHanjaListOpen(const shurufa::mac::ViewState &view) {
    return MSIMECandidateListOpen(view) && MSIMEViewScheme(view) == msime::mac::scheme::Korean;
}
// Candidates may carry translation glosses (kept for dictionary-backed async snapshot checks too).
static BOOL MSIMEViewShowsGlosses(NSDictionary *view) {
    const int scheme = [view isKindOfClass:NSDictionary.class]
        ? (int)MSIMEStrictInteger(view[@"scheme"], msime::mac::scheme::Quanpin)
        : msime::mac::scheme::Quanpin;
    return msime::mac::scheme::ShowsGlosses(scheme);
}
static BOOL MSIMECapsLockFreshUppercaseBypass(NSEvent *event, const shurufa::mac::ViewState &view) {
    if (MSIMEEditionIsShurufa()) return NO;
    if (!event || event.type != NSEventTypeKeyDown) return NO;
    const NSEventModifierFlags competing = NSEventModifierFlagShift | NSEventModifierFlagControl |
                                           NSEventModifierFlagOption | NSEventModifierFlagCommand;
    if (!(event.modifierFlags & NSEventModifierFlagCapsLock) || (event.modifierFlags & competing)) return NO;
    if (event.characters.length != 1) return NO;
    const unichar character = [event.characters characterAtIndex:0];
    if (character < 'A' || character > 'Z') return NO;
    return view.editingText.length == 0 && view.candidates.empty();
}
[[maybe_unused]] static BOOL MSIMECapsLockFreshUppercaseBypass(NSEvent *event, id view) {
    return MSIMECapsLockFreshUppercaseBypass(event, shurufa::mac::ViewState::fromDictionary((NSDictionary *)view));
}
// ---- 单按 Caps Lock 切换中英文 ----
// 把大写锁定撤回关闭。IOHIDSetModifierLockState 走 IOHIDSystem 的参数连接；在 macOS 27 上实测，一个既没有「输入监控」也没有「辅助功能」授权的进程照样能改动锁定状态，IOHIDCheckAccess 前后都停在「未询问」，也不弹授权框，所以输入法不需要为它申请权限。
static bool MSIMEReleaseSystemCapsLock() {
    io_service_t service = IOServiceGetMatchingService(MACH_PORT_NULL, IOServiceMatching(kIOHIDSystemClass));
    if (!service) return false;
    io_connect_t connect = 0;
    const kern_return_t opened = IOServiceOpen(service, mach_task_self(), kIOHIDParamConnectType, &connect);
    IOObjectRelease(service);
    if (opened != KERN_SUCCESS) return false;
    const bool released = IOHIDSetModifierLockState(connect, kIOHIDCapsLockState, false) == KERN_SUCCESS;
    IOServiceClose(connect);
    return released;
}
// 在原生查询边界替换合成状态，回归必须经过生产的实体按住判定。
static CGEventFlags (*MSIMEPhysicalModifierFlagsState)(CGEventSourceStateID) = &CGEventSourceFlagsState;
// 外接键盘有时不把实体按住状态送进 Quartz 的 stateless 位；先从 HID 输入值读取，失败再退回系统位。
static std::unordered_map<IOHIDDeviceRef, bool> MSIMECapsLockHIDDevices;
static std::atomic<bool> MSIMECapsLockHIDStateKnown{false};
static std::atomic<bool> MSIMECapsLockHIDStateHeld{false};

static bool MSIMECapsLockHIDUsage(uint32_t usage) {
    return usage == kHIDUsage_KeyboardCapsLock || usage == kHIDUsage_KeyboardLockingCapsLock;
}

static void MSIMERecomputeCapsLockHIDState() {
    bool held = false;
    for (const auto &entry : MSIMECapsLockHIDDevices) held = held || entry.second;
    MSIMECapsLockHIDStateKnown.store(!MSIMECapsLockHIDDevices.empty(), std::memory_order_release);
    MSIMECapsLockHIDStateHeld.store(held, std::memory_order_release);
}

static void MSIMERecordCapsLockHIDValue(IOHIDValueRef value) {
    if (!value) return;
    IOHIDElementRef element = IOHIDValueGetElement(value);
    if (!element || IOHIDElementGetUsagePage(element) != kHIDPage_KeyboardOrKeypad ||
        !MSIMECapsLockHIDUsage(IOHIDElementGetUsage(element))) return;
    IOHIDDeviceRef device = IOHIDElementGetDevice(element);
    if (!device) return;
    MSIMECapsLockHIDDevices[device] = IOHIDValueGetIntegerValue(value) != 0;
    MSIMERecomputeCapsLockHIDState();
}

static void MSIMECapsLockHIDInputValue(void *, IOReturn result, void *, IOHIDValueRef value) {
    if (result == kIOReturnSuccess) MSIMERecordCapsLockHIDValue(value);
}

static void MSIMECapsLockHIDDeviceRemoved(void *, IOReturn, void *, IOHIDDeviceRef device) {
    if (!device) return;
    MSIMECapsLockHIDDevices.erase(device);
    MSIMERecomputeCapsLockHIDState();
}

static void MSIMECapsLockHIDDeviceMatched(void *, IOReturn result, void *, IOHIDDeviceRef device) {
    if (result != kIOReturnSuccess || !device) return;
    CFArrayRef elements = IOHIDDeviceCopyMatchingElements(device, nullptr, kIOHIDOptionsTypeNone);
    if (!elements) return;
    bool found = false;
    bool held = false;
    for (CFIndex index = 0; index < CFArrayGetCount(elements); ++index) {
        IOHIDElementRef element = static_cast<IOHIDElementRef>(const_cast<void *>(CFArrayGetValueAtIndex(elements, index)));
        if (!element || IOHIDElementGetUsagePage(element) != kHIDPage_KeyboardOrKeypad ||
            !MSIMECapsLockHIDUsage(IOHIDElementGetUsage(element))) continue;
        IOHIDValueRef value = nullptr;
        if (IOHIDDeviceGetValue(device, element, &value) == kIOReturnSuccess && value) {
            found = true;
            held = held || IOHIDValueGetIntegerValue(value) != 0;
        }
    }
    CFRelease(elements);
    if (found) {
        MSIMECapsLockHIDDevices[device] = held;
        MSIMERecomputeCapsLockHIDState();
    }
}

static CFMutableDictionaryRef MSIMECapsLockHIDMatching(uint32_t usage) {
    CFMutableDictionaryRef matching = CFDictionaryCreateMutable(kCFAllocatorDefault, 2,
                                                                  &kCFTypeDictionaryKeyCallBacks,
                                                                  &kCFTypeDictionaryValueCallBacks);
    int32_t pageValue = kHIDPage_KeyboardOrKeypad;
    int32_t usageValue = static_cast<int32_t>(usage);
    CFNumberRef page = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &pageValue);
    CFNumberRef usageNumber = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &usageValue);
    CFDictionarySetValue(matching, CFSTR(kIOHIDElementUsagePageKey), page);
    CFDictionarySetValue(matching, CFSTR(kIOHIDElementUsageKey), usageNumber);
    CFRelease(page);
    CFRelease(usageNumber);
    return matching;
}

// 只在按下 Caps Lock 时设备没报实体按住位、HID 也还没有结果时调用（observeCapsLockTap），报这个位的内置键盘不会走到这里，也就不会被要求「输入监控」授权。打开键盘的 HID 管理器需要这项授权：未询问时请系统询问一次，授权之前不打开，免得在用户没准备时弹框；没打开就留到下一次这样的按键再试，用户之后在系统设置里允许也不必重启输入法。只在主线程调用。
static void MSIMEStartCapsLockHIDTracking() {
    static IOHIDManagerRef tracking = nullptr;
    if (tracking) return;
    if (!MSIMEEnsureInputMonitoringAccess()) return;
    IOHIDManagerRef manager = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDOptionsTypeNone);
    if (!manager) return;
    CFMutableDictionaryRef capsLock = MSIMECapsLockHIDMatching(kHIDUsage_KeyboardCapsLock);
    CFMutableDictionaryRef lockingCapsLock = MSIMECapsLockHIDMatching(kHIDUsage_KeyboardLockingCapsLock);
    const void *matches[] = {capsLock, lockingCapsLock};
    CFArrayRef matching = CFArrayCreate(kCFAllocatorDefault, matches, 2, &kCFTypeArrayCallBacks);
    IOHIDManagerSetInputValueMatchingMultiple(manager, matching);
    CFRelease(matching);
    CFRelease(capsLock);
    CFRelease(lockingCapsLock);
    IOHIDManagerRegisterDeviceMatchingCallback(manager, &MSIMECapsLockHIDDeviceMatched, nullptr);
    IOHIDManagerRegisterDeviceRemovalCallback(manager, &MSIMECapsLockHIDDeviceRemoved, nullptr);
    IOHIDManagerRegisterInputValueCallback(manager, &MSIMECapsLockHIDInputValue, nullptr);
    IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), kCFRunLoopDefaultMode);
    IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), kCFRunLoopCommonModes);
    if (IOHIDManagerOpen(manager, kIOHIDOptionsTypeNone) != kIOReturnSuccess) {
        IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), kCFRunLoopDefaultMode);
        IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), kCFRunLoopCommonModes);
        CFRelease(manager);
        return;
    }
    tracking = manager;
}
// 测试换成计数的：单测不能打开跑测试那台机器的真实键盘，也不能替它申请授权。
static void (*MSIMECapsLockHIDStart)() = &MSIMEStartCapsLockHIDTracking;

static bool MSIMEReadCapsLockHIDState(bool *known) {
    const bool stateKnown = MSIMECapsLockHIDStateKnown.load(std::memory_order_acquire);
    if (known) *known = stateKnown;
    return MSIMECapsLockHIDStateHeld.load(std::memory_order_acquire);
}
static bool (*MSIMECapsLockHIDState)(bool *known) = &MSIMEReadCapsLockHIDState;

// 键码 0x39 的查询跟随大写锁定，松手后仍可能为真；只读 stateless 位区分实体按住与逻辑锁定。
static bool MSIMECapsLockPhysicallyHeld() {
    bool hidStateKnown = false;
    if (MSIMECapsLockHIDState && MSIMECapsLockHIDState(&hidStateKnown) && hidStateKnown) return true;
    if (hidStateKnown) return false;
    return (MSIMEPhysicalModifierFlagsState(kCGEventSourceStateHIDSystemState) & NX_ALPHASHIFT_STATELESS_MASK) != 0;
}
// 和 NSEvent.timestamp 同一个开机后时钟，单位毫秒。
static std::int64_t MSIMEUptimeMilliseconds() {
    return static_cast<std::int64_t>(llround(NSProcessInfo.processInfo.systemUptime * 1000.0));
}
// 测试换成合成的：单测不能改动跑测试那台机器的真实大写锁定，也没有人真的按着键。
static bool (*MSIMECapsLockRelease)() = &MSIMEReleaseSystemCapsLock;
static bool (*MSIMECapsLockKeyHeld)() = &MSIMECapsLockPhysicallyHeld;
static std::int64_t (*MSIMECapsLockNow)() = &MSIMEUptimeMilliseconds;
static NSString *MSIMEChinesePunctuationForSmart(unichar character) {
    switch (character) {
    case ',': return @"，";
    case '.': return @"。";
    case ':': return @"：";
    default: return nil;
    }
}
static unichar MSIMEASCIIForSmartChinesePunctuation(unichar character,
                                                    const msime::input::PunctuationMarks &marks) {
    switch (character) {
    case 0x3002: return '.'; // 。
    case 0xFF0C: return ','; // ，
    case 0xFF01: return '!'; // ！
    case 0xFF1F: return '?'; // ？
    case 0xFF1B: return ';'; // ；
    case 0xFF1A: return ':'; // ：
    // 顿号按 View 的顿号开关（enumerationComma 段）取开着的那个键：`\` 优先、`/` 其次；两个都关按归一的 `\`。
    case 0x3001: return marks.enumeration_comma_backslash ? '\\' : (marks.enumeration_comma_slash ? '/' : '\\');
    case 0x201C: case 0x201D: return '"'; // “ ”
    case 0x2018: case 0x2019: return '\''; // ‘ ’
    case 0x300E: case 0x300F: return '"'; // 『 』
    case 0x300C: case 0x300D: return '\''; // 「 」
    case 0x3010: case 0x3016: return '['; // 【 〖
    case 0x3011: case 0x3017: return ']'; // 】 〗
    case 0x300A: return '<'; // 《
    case 0x300B: return '>'; // 》
    case 0xFF08: return '('; // （
    case 0xFF09: return ')'; // ）
    default: return 0;
    }
}
// 直角引号两组字：「」是 0x300C/0x300D，『』是 0x300E/0x300F。
static BOOL MSIMEIsCornerQuoteMark(id mark) {
    if (![mark isKindOfClass:NSString.class] || [mark length] != 1) return NO;
    const unichar c = [mark characterAtIndex:0];
    return c >= 0x300C && c <= 0x300F;
}
// 这个字所属的一组由哪个键出：corner 样式下 `'` 出「」、`"` 出『』；契约第 3 版起引擎里 `{` `}` 也分别出这两组。普通中文方案的 `{` `}` 在到达引擎前被上面的宿主分支拦成「/」，仍按契约分组判：`{` 算出「」的键，`}` 不算——把它算进「」组会让 `}` 上的屏的」被改回「。注音的大千键位 `{` `}` 由引擎 `SHIFT_PUNCTUATION` 固定出『』，也不算出这两组的键。
static BOOL MSIMEKeyProducesCornerQuote(unichar key, unichar mark, int scheme) {
    const BOOL zhuyin = scheme == msime::mac::scheme::Zhuyin;
    switch (mark) {
    case 0x300C: case 0x300D: return key == '\'' || (!zhuyin && key == '{'); // 「 」
    case 0x300E: case 0x300F: return key == '"' || (!zhuyin && key == '}');  // 『 』
    default: return NO;
    }
}
// View 报告的引号与方头括号样式（host-api 已解析好 corner_when_traditional）；英文模式下的中文标点按它出字。
static msime::input::PunctuationMarks MSIMEViewPunctuationMarks(const shurufa::mac::ViewState &view) {
    return view.inputPunctuationMarks();
}
// `、` 的 ASCII 孪生按出的键算：`/` 出的顿号转回 `/`，`\` 出的转回 `\`——哪个键开着才算数，与直角引号 `MSIMEKeyProducesCornerQuote` 同一条「按出的键算」规则。
static BOOL MSIMEKeyProducesEnumerationComma(unichar key, unichar mark,
                                             const msime::input::PunctuationMarks &marks) {
    if (mark != 0x3001) return NO;
    return (key == '\\' && marks.enumeration_comma_backslash) ||
           (key == '/' && marks.enumeration_comma_slash);
}
// 文档里读回的中文标点与武装的 ASCII 键是否一对：顿号的合法 ASCII 是开着的顿号键集合，别的标点按单值反查比。
static BOOL MSIMESmartPunctuationAsciiMatches(unichar chinese, unichar ascii,
                                              const msime::input::PunctuationMarks &marks) {
    if (chinese == 0x3001)
        return MSIMEKeyProducesEnumerationComma(ascii, chinese, marks);
    return MSIMEASCIIForSmartChinesePunctuation(chinese, marks) == ascii;
}

static BOOL MSIMEASCIIPunctuation(unichar character) {
    return character > 0x20 && character < 0x7F && !MSIMEASCIIAlphanumeric(character);
}

static BOOL MSIMESmartPunctuationSpaceKey(unichar character) {
    switch (character) {
    case '.': case ',': case '!': case '?': case ';': case ':': case '/': case '\\':
    case '"': case '\'': case '[': case ']': case '<': case '>': case '(': case ')':
        return YES;
    default:
        return NO;
    }
}
static NSString *MSIMEFullWidthSmartMark(unichar character, BOOL fullWidth) {
    if (!fullWidth) return [NSString stringWithCharacters:&character length:1];
    const unichar converted = msime::mac::FullWidthCharacter(character);
    return [NSString stringWithCharacters:&converted length:1];
}

@interface MSIMECandidatePanel : NSPanel
@property(nonatomic) BOOL mouseWheelEnabled;
@property(nonatomic) BOOL hasPreviousPage;
@property(nonatomic) BOOL hasNextPage;
@property(nonatomic, copy) void (^pageHandler)(BOOL previous);
@property(nonatomic, copy) void (^scrollHandler)(NSEvent *event);
// True while a wheel page is being applied, so the re-render it causes keeps the scroll remainder.
@property(nonatomic, readonly) BOOL wheelPaging;
- (void)resetWheelAccumulator;
@end
@implementation MSIMECandidatePanel {
    double _wheelAccumulator;
}
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
// Control 点按在 AppKit 里不一定走到视图的 mouseDown:，右键也可能先被窗口处理，所以在窗口这一层就把候选行的菜单点按截下，交给不抢焦点的菜单。
- (void)sendEvent:(NSEvent *)event {
    const BOOL contextClick = event.type == NSEventTypeRightMouseDown ||
        (event.type == NSEventTypeLeftMouseDown && (event.modifierFlags & NSEventModifierFlagControl));
    NSView *frameView = self.contentView.superview;
    if (contextClick && frameView) {
        NSView *hit = [self.contentView hitTest:[frameView convertPoint:event.locationInWindow fromView:nil]];
        while (hit && ![hit isKindOfClass:MSIMECandidateButton.class]) hit = hit.superview;
        if (hit && [(MSIMECandidateButton *)hit presentCandidateMenuForEvent:event]) return;
    }
    [super sendEvent:event];
}
- (void)resetWheelAccumulator { _wheelAccumulator = 0.0; }
- (void)orderOut:(id)sender {
    _wheelAccumulator = 0.0;
    [super orderOut:sender];
}
- (void)scrollWheel:(NSEvent *)event {
    if (self.scrollHandler) {
        self.scrollHandler(event);
        return;
    }
    const auto action = msime::candidate::CandidateWheelPageAction(
        event.scrollingDeltaY, self.mouseWheelEnabled, self.hasPreviousPage, self.hasNextPage);
    if (action == msime::candidate::CandidateWheelAction::None || !self.pageHandler) {
        _wheelAccumulator = 0.0;
        [super scrollWheel:event];
        return;
    }
    const int steps = msime::candidate::ConsumeCandidateWheelDelta(_wheelAccumulator, event.scrollingDeltaY,
        event.hasPreciseScrollingDeltas, (event.phase & (NSEventPhaseBegan | NSEventPhaseMayBegin)) != 0,
        event.momentumPhase != NSEventPhaseNone);
    const BOOL previous = steps > 0;
    _wheelPaging = YES;
    for (int remaining = previous ? steps : -steps; remaining > 0 && self.pageHandler && (previous ? self.hasPreviousPage : self.hasNextPage); --remaining)
        self.pageHandler(previous);
    _wheelPaging = NO;
}
@end

// Match the pinned Windows TextBlock preedit marker spacing in logical points.
static constexpr CGFloat MSIMEPreeditCaretWidth = msime::candidate::kCandidatePreeditCaretWidth;
static constexpr CGFloat MSIMEPreeditCaretSideAir = msime::candidate::kCandidatePreeditCaretSideAir;
static constexpr CGFloat MSIMEPreeditCaretEndAir = msime::candidate::kCandidatePreeditCaretEndAir;
static constexpr CGFloat MSIMEPreeditCaretGap = msime::candidate::kCandidatePreeditCaretGap;
struct MSIMEPreeditSlotMetrics { CGFloat ascent; CGFloat descent; };
static void MSIMEPreeditSlotRelease(void *context) { delete static_cast<MSIMEPreeditSlotMetrics *>(context); }
static CGFloat MSIMEPreeditSlotAscent(void *context) { return static_cast<MSIMEPreeditSlotMetrics *>(context)->ascent; }
static CGFloat MSIMEPreeditSlotDescent(void *context) { return static_cast<MSIMEPreeditSlotMetrics *>(context)->descent; }
static CGFloat MSIMEPreeditSlotWidth(void *) { return MSIMEPreeditCaretGap; }

@interface MSIMECandidatePreeditField : NSTextField
@property(nonatomic) NSUInteger caretIndex;
@property(nonatomic) BOOL showsCaret;
@property(nonatomic, strong) NSColor *caretColor;
@property(nonatomic, readonly) NSRect caretRect;
@end

@implementation MSIMECandidatePreeditField
- (void)setCaretIndex:(NSUInteger)value { _caretIndex = value; self.needsDisplay = YES; }
- (void)setShowsCaret:(BOOL)value { _showsCaret = value; self.needsDisplay = YES; }
- (void)setCaretColor:(NSColor *)value { _caretColor = value; self.needsDisplay = YES; }
- (CTLineRef)newPreeditLine CF_RETURNS_RETAINED {
    NSFont *font = self.font ?: [NSFont systemFontOfSize:16];
    NSMutableAttributedString *text = [[NSMutableAttributedString alloc] initWithString:self.stringValue attributes:@{
        NSFontAttributeName:font,
        NSForegroundColorAttributeName:self.textColor ?: NSColor.labelColor
    }];
    if (self.showsCaret && self.caretIndex < self.stringValue.length) {
        CTRunDelegateCallbacks callbacks = {kCTRunDelegateVersion1, MSIMEPreeditSlotRelease,
            MSIMEPreeditSlotAscent, MSIMEPreeditSlotDescent, MSIMEPreeditSlotWidth};
        CTRunDelegateRef slot = CTRunDelegateCreate(&callbacks, new MSIMEPreeditSlotMetrics{font.ascender, -font.descender});
        NSAttributedString *gap = [[NSAttributedString alloc] initWithString:@"\uFFFC" attributes:@{
            (__bridge NSString *)kCTRunDelegateAttributeName:(__bridge id)slot, NSFontAttributeName:font
        }];
        [text insertAttributedString:gap atIndex:self.caretIndex];
        CFRelease(slot);
    }
    return CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)text);
}
- (NSRect)caretRectForLine:(CTLineRef)line origin:(CGFloat *)origin baseline:(CGFloat *)baseline {
    CGFloat ascent = 0, descent = 0;
    CTLineGetTypographicBounds(line, &ascent, &descent, nullptr);
    CGFloat offset = CTLineGetOffsetForStringIndex(line, MIN(self.caretIndex, self.stringValue.length), nullptr);
    if (self.showsCaret) offset += self.caretIndex < self.stringValue.length ? MSIMEPreeditCaretSideAir : MSIMEPreeditCaretEndAir;
    CGFloat available = MAX(0.0, NSWidth(self.bounds) - 4.0 - MSIMEPreeditCaretWidth);
    // Scroll just enough to keep the insertion point inside the clipped row.
    *origin = 2.0 - MAX(0.0, offset - available);
    CGFloat top = MAX(0.0, (NSHeight(self.bounds) - ascent - descent) / 2.0);
    *baseline = top + ascent;
    return NSMakeRect(*origin + offset, top, MSIMEPreeditCaretWidth, MIN(ascent + descent, NSHeight(self.bounds)));
}
- (NSRect)caretRect {
    if (!self.showsCaret || !self.stringValue.length) return NSZeroRect;
    CTLineRef line = [self newPreeditLine];
    CGFloat origin, baseline;
    NSRect caret = [self caretRectForLine:line origin:&origin baseline:&baseline];
    CFRelease(line);
    return caret;
}
- (BOOL)isFlipped { return YES; }
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    CTLineRef line = [self newPreeditLine];
    CGFloat origin, baseline;
    NSRect caret = [self caretRectForLine:line origin:&origin baseline:&baseline];
    CGContextRef context = NSGraphicsContext.currentContext.CGContext;
    CGContextSaveGState(context);
    CGContextClipToRect(context, NSRectToCGRect(self.bounds));
    CGContextTranslateCTM(context, origin, baseline);
    CGContextScaleCTM(context, 1, -1);
    CGContextSetTextMatrix(context, CGAffineTransformIdentity);
    CGContextSetTextPosition(context, 0, 0);
    CTLineDraw(line, context);
    CGContextRestoreGState(context);
    CFRelease(line);
    if (self.showsCaret && self.stringValue.length) {
        [self.caretColor ?: NSColor.controlAccentColor setFill];
        NSRectFill(NSIntersectionRect(caret, self.bounds));
    }
}
@end

// 首行里的细线：拼音和输入状态之间的竖线、首行和候选行之间的横线。整块填色，颜色在 refreshCandidateSkin 里按主题给；不接收点击。
@interface MSIMECandidateHeaderLineView : NSView
@property(nonatomic, copy) NSColor *lineColor;
@end

@implementation MSIMECandidateHeaderLineView
- (void)setLineColor:(NSColor *)value { _lineColor = [value copy]; self.needsDisplay = YES; }
- (NSView *)hitTest:(NSPoint)point { (void)point; return nil; }
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    [self.lineColor ?: NSColor.separatorColor setFill];
    NSRectFillUsingOperation(self.bounds, NSCompositingOperationSourceOver);
}
@end

// 首行右端的小装饰：一枝水杉的三片小叶，从左下往右上由小到大、由淡到浓，用主题的强调色画；不接收点击。
@interface MSIMECandidateHeaderOrnamentView : NSView
@property(nonatomic, copy) NSColor *tintColor;
@end

@implementation MSIMECandidateHeaderOrnamentView
- (void)setTintColor:(NSColor *)value { _tintColor = [value copy]; self.needsDisplay = YES; }
- (NSView *)hitTest:(NSPoint)point { (void)point; return nil; }
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    if (MSIMEEditionIsShurufa()) return;
    NSColor *tint = self.tintColor ?: NSColor.controlAccentColor;
    const NSRect bounds = NSInsetRect(self.bounds, 1, 1);
    // 一根斜枝从左下到右上，三片叶沿着它排开，越往上越大越浓。
    NSBezierPath *stem = [NSBezierPath bezierPath];
    [stem moveToPoint:NSMakePoint(NSMinX(bounds), NSMinY(bounds))];
    [stem lineToPoint:NSMakePoint(NSMaxX(bounds), NSMaxY(bounds))];
    stem.lineWidth = MAX(0.75, NSWidth(bounds) / 18.0);
    [[tint colorWithAlphaComponent:tint.alphaComponent * 0.4] setStroke];
    [stem stroke];
    const CGFloat unit = NSWidth(bounds) / 3.0;
    for (NSUInteger index = 0; index < 3; ++index) {
        const CGFloat length = unit * (1.0 + 0.3 * index);
        const CGFloat along = (0.25 + 0.3 * index);
        const NSPoint centre = NSMakePoint(NSMinX(bounds) + NSWidth(bounds) * along, NSMinY(bounds) + NSHeight(bounds) * along);
        NSAffineTransform *transform = [NSAffineTransform transform];
        [transform translateXBy:centre.x yBy:centre.y];
        [transform rotateByDegrees:index % 2 ? 90 : 0];
        NSBezierPath *leaf = [NSBezierPath bezierPathWithOvalInRect:NSMakeRect(-length / 2.0, -length * 0.22, length, length * 0.44)];
        [leaf transformUsingAffineTransform:transform];
        [[tint colorWithAlphaComponent:tint.alphaComponent * (0.45 + 0.275 * index)] setFill];
        [leaf fill];
    }
}
@end

// Button target for the non-modal cloud consent prompt; NSAlert's own buttons only end a modal session.
@interface MSIMECloudConsentTarget : NSObject
@property(nonatomic, copy) void (^handler)(BOOL enabled);
@end
@implementation MSIMECloudConsentTarget
- (void)enable:(id)sender { (void)sender; if (self.handler) self.handler(YES); }
- (void)disable:(id)sender { (void)sender; if (self.handler) self.handler(NO); }
@end

@protocol MSIMEInputSessionRecoveryDelegate <NSObject>
- (void)retryUnavailableSession:(id)sender;
- (void)restartUnavailableInputMethod:(id)sender;
- (void)dismissSessionRecoveryNotice:(id)sender;
@end

// 故障提示不能进入模态循环或取得键盘焦点，否则会阻塞其他客户端输入。
@interface MSIMEInputSessionRecoveryPanel : NSPanel
@property(nonatomic, weak) id<MSIMEInputSessionRecoveryDelegate> recoveryDelegate;
@property(nonatomic, readonly) NSTextField *message;
@property(nonatomic, readonly) NSButton *retryButton;
@property(nonatomic, readonly) NSButton *restartButton;
@property(nonatomic, readonly) NSButton *laterButton;
+ (instancetype)sharedPanel;
+ (void)dismissForDelegate:(id<MSIMEInputSessionRecoveryDelegate>)delegate;
- (void)showForDelegate:(id<MSIMEInputSessionRecoveryDelegate>)delegate;
- (void)setRestartPending:(BOOL)pending;
@end

static MSIMEInputSessionRecoveryPanel *MSIMESessionRecoveryPanel;
static BOOL MSIMESessionRecoveryNoticeScheduled = NO;
static BOOL MSIMESessionRecoveryRestartPending = NO;
static uint64_t MSIMESessionRecoveryLastNotice = 0;
static constexpr uint64_t MSIMESessionRecoveryNoticeInterval = 60 * NSEC_PER_SEC;

@implementation MSIMEInputSessionRecoveryPanel
+ (instancetype)sharedPanel {
    if (!MSIMESessionRecoveryPanel) MSIMESessionRecoveryPanel = [[self alloc] init];
    return MSIMESessionRecoveryPanel;
}
+ (void)dismissForDelegate:(id<MSIMEInputSessionRecoveryDelegate>)delegate {
    if (delegate && MSIMESessionRecoveryPanel.recoveryDelegate && MSIMESessionRecoveryPanel.recoveryDelegate != delegate) return;
    [MSIMESessionRecoveryPanel orderOut:nil];
    MSIMESessionRecoveryPanel.recoveryDelegate = nil;
}
- (instancetype)init {
    self = [super initWithContentRect:NSMakeRect(0, 0, 430, 210)
        styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:YES];
    if (!self) return nil;
    self.title = MSIMEEditionDisplayName();
    self.identifier = @"msime-session-recovery";
    self.level = MSIMECandidateWindowLevel();
    self.hidesOnDeactivate = NO;
    self.becomesKeyOnlyIfNeeded = YES;
    self.releasedWhenClosed = NO;
    self.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary |
        NSWindowCollectionBehaviorTransient | NSWindowCollectionBehaviorIgnoresCycle;
    NSTextField *title = [NSTextField labelWithString:@"中文输入暂不可用"];
    title.font = [NSFont boldSystemFontOfSize:17];
    title.frame = NSMakeRect(18, 166, 394, 28);
    [self.contentView addSubview:title];
    _message = [NSTextField wrappingLabelWithString:@""];
    _message.frame = NSMakeRect(18, 64, 394, 94);
    [self.contentView addSubview:_message];
    _laterButton = [NSButton buttonWithTitle:@"稍后" target:self action:@selector(later:)];
    _retryButton = [NSButton buttonWithTitle:@"重试" target:self action:@selector(retry:)];
    _restartButton = [NSButton buttonWithTitle:@"重启输入法" target:self action:@selector(restart:)];
    _laterButton.frame = NSMakeRect(18, 18, 68, 30);
    _retryButton.frame = NSMakeRect(212, 18, 84, 30);
    _restartButton.frame = NSMakeRect(304, 18, 108, 30);
    for (NSButton *button in @[_laterButton, _retryButton, _restartButton]) {
        button.keyEquivalent = @"";
        [self.contentView addSubview:button];
    }
    return self;
}
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
- (void)showForDelegate:(id<MSIMEInputSessionRecoveryDelegate>)delegate {
    self.recoveryDelegate = delegate;
    _message.stringValue = @"按键暂时会直接输入字母。请先重试；仍无法恢复时，可重启输入法，或切换系统中文输入法继续输入。\n\n重启会结束其他窗口中未完成的输入。";
    [self setRestartPending:MSIMESessionRecoveryRestartPending];
    [self center];
    [self orderFrontRegardless];
    NSAccessibilityPostNotificationWithUserInfo(self, NSAccessibilityAnnouncementRequestedNotification,
        @{NSAccessibilityAnnouncementKey:@"中文输入暂不可用，可以重试或重启输入法。"});
}
- (void)setRestartPending:(BOOL)pending {
    _retryButton.enabled = !pending;
    _restartButton.enabled = !pending;
    if (pending) _message.stringValue = @"正在重新启动输入法…";
}
- (void)retry:(id)sender { [self.recoveryDelegate retryUnavailableSession:sender]; }
- (void)restart:(id)sender { [self.recoveryDelegate restartUnavailableInputMethod:sender]; }
- (void)later:(id)sender { [self.recoveryDelegate dismissSessionRecoveryNotice:sender]; }
@end

@interface MSIMEInputController : IMKInputController <MSIMEInputSessionRecoveryDelegate, NSAnimationDelegate>
- (MSIMECustomTranslationBatch *)aiBatchForItems:(NSArray<NSDictionary *> *)items
                                       completion:(void (^)(NSArray<NSDictionary *> *))completion;
- (NSDictionary *)recoverPreferencesInDirectory:(NSString *)directory error:(NSError **)error;
- (NSDictionary *)serviceSnapshotQuery;
- (NSDictionary *)serviceSnapshotView;
- (void)invalidateServiceSnapshots;
- (void)showCandidatePanel;
- (void)observeGlobalVoiceShortcutEvent:(NSEvent *)event;
@end

// 当前持有焦点客户端的控制器。由 `MSIMEAppearancePreferences` 统一落盘写入 `config.json`，控制器在收到持久化通知或聚焦刷新时只读重新加载。
static __weak MSIMEInputController *MSIMEFocusedController;
// 录音跨焦点持有原控制器，异步识别仍属于同一代次；最终投递另取当前焦点。
static MSIMEInputController *MSIMEVoiceRecordingOwner;
static MSIMEVoiceRecordingMonitor MSIMERecordingMonitor;
static MSIMEVoiceShortcutTap MSIMEShortcutTap;
// IMK、AppKit 全局监听与录音 tap 共用回执；结束录音重置手势时不清回执。
struct MSIMEVoiceEventReceipt { NSTimeInterval timestamp = 0; BOOL consumed = NO; };
static std::unordered_map<uint32_t, MSIMEVoiceEventReceipt> MSIMEVoiceEventReceipts;

// 只在主线程调用。水杉不是当前输入源、也没在录音时不吞任何键。
static void MSIMERefreshVoiceShortcutTap() {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    NSDictionary *custom = MSIMEVoiceInputEnabled(defaults) ? MSIMEVoiceCustomShortcut(defaults) : nil;
    std::optional<MSIMEVoiceShortcutBinding> binding;
    if (MSIMEFocusedController || MSIMEVoiceRecordingOwner) binding = MSIMEVoiceSwallowBinding(custom);
    MSIMEShortcutTap.update(binding, ^(NSEvent *event) {
        MSIMEInputController *target = MSIMEVoiceRecordingOwner ?: MSIMEFocusedController;
        [target observeGlobalVoiceShortcutEvent:event];
    });
}


// The full-colour brand mark the floating toolbar and the mode HUD lead with, loaded once. Nil when the bundle has no icon, and the top row then carries no mark.
static NSImage *MSIMECandidateLogoImage() {
    static NSImage *logo;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSString *path = [[NSBundle bundleForClass:MSIMEInputController.class] pathForResource:@"MSIMEClientInputMethod" ofType:@"icns"];
        logo = path ? [[NSImage alloc] initWithContentsOfFile:path] : nil;
    });
    return logo;
}

@implementation MSIMEInputController {
    MSIMEClientSession *_session;
    uint64_t _dictionarySessionLastUse;
    // Stands in for TISEnableInputSource when set, so a test can see which opt-in mode a scheme change enables.
    OSStatus (^_optInInputModeEnabler)(NSString *identifier);
    // 设置后代替菜单栏状态项，测试由此看到状态项被摆成哪个模式（nil 表示隐藏），不在测试进程里真的建 NSStatusItem。
    void (^_inputModeStatusItemPresenter)(NSString *modeID);
    NSUInteger _inputModeActivationGeneration;
    MSIMEVoiceInputService *_voiceService;
    MSIMEVoiceWaveOverlay *_voiceOverlay;
    MSIMEVoiceCuePlayer *_voiceCuePlayer;
    BOOL _voiceCueRecording;
    MSIMEVoiceAudioMuter *_voiceAudioMuter;
    MSIMEHTTPVoiceRequest *_httpVoiceRequest;
    MSIMEClientSession *_httpVoiceSession;
    id _httpVoiceClient;
    uint64_t _httpVoiceGeneration;
    BOOL _httpVoiceProcessing;
    MSIMEVoiceCommitRoute _httpVoiceCommit;
    MSIMEVoiceCommitRoute _doubaoVoiceCommit;
    MSIMEVoiceCommitRoute _liveVoiceCommit;
    // Doubao's websocket or the on-device helper: the two streaming providers share this path.
    id<MSIMEStreamingVoiceRequest> _doubaoVoiceRequest;
    MSIMEHTTPVoiceRequest *_doubaoPolishRequest;
    BOOL _doubaoPolishCreationFailed;
    BOOL _doubaoFinalReceived;
    MSIMEClientSession *_doubaoVoiceSession;
    id _doubaoVoiceClient;
    uint64_t _doubaoVoiceGeneration;
    BOOL _doubaoVoiceProcessing;
    BOOL _doubaoVoiceInline;
    BOOL _doubaoVoiceMarked;
    id _liveVoiceToken;
    MSIMEHTTPVoiceRequest *_livePolishRequest;
    BOOL _livePolishCreationFailed;
    BOOL _liveVoiceFinalReceived;
    MSIMEClientSession *_liveVoiceSession;
    id _liveVoiceClient;
    NSString *_liveVoiceSocket;
    uint64_t _liveVoiceGeneration;
    BOOL _liveVoiceInline;
    BOOL _liveVoiceMarked;
    BOOL _liveVoiceProcessing;
    NSString *_voicePreviewText;
    NSNumber *_voiceCopiedResult;
    NSString *_voicePolishFallbackNotice;
    NSString *_voiceSalvageNotice;
    NSString *_lastUndeliveredVoiceText;
    // 失败浮窗一键原地重试状态（独立于常规实时录音，可被新录音或 Esc 随时取消）。
    BOOL _voiceRetryInFlight;
    uint64_t _voiceRetryGeneration;
    id<MSIMEStreamingVoiceRequest> _voiceRetryStreamRequest;
    MSIMEHTTPVoiceRequest *_voiceRetryHTTPRequest;
    MSIMEVoicePolishTask *_voiceRetryPolishTask;
    // Wave 2 本地语音后处理与润色管线状态（后台词表队列、缓存、上下文快照与在途润色任务）。
    dispatch_queue_t _voicePipelineQueue;
    MSIMEVoicePolishClient *_voicePolishClient;
    MSIMEVoicePolishTask *_voicePolishTask;
    MSIMEVoiceEditTracker *_voiceEditTracker;
    BOOL _voicePolishPending;
    MSIMEVoiceContextSnapshot *_voiceContextSnapshot;
    NSDictionary *_voiceOptionsSnapshot;
    NSArray<NSDictionary<NSString *, id> *> *_voiceVocabularyTerms;
    uint64_t _voiceVocabularyGeneration;
    uint64_t _voiceAsrStartedAtNs;
    BOOL _useVoicePolishClientForDoubao;
    BOOL _useVoicePolishClientForLive;
    id _globalVoiceHotkeyMonitor;
    id _voicePermissionToken;
    uint64_t _voiceGeneration;
    id _activeClient;
    NSAppearance *_candidateHostAppearance;
    __weak id _candidateChromeClient;
    shurufa::mac::ViewState _viewState;
    shurufa::mac::ViewState _renderedCandidateState;
    // 上一次请求读屏朗读的高亮候选（它的 accessibilityLabel），同一段话不重复读；候选窗收起后再出现时清掉。
    NSString *_announcedCandidate;
    // Bumped by every apply:. Writing marked text is a synchronous call into the client, and IMK services the next key inside it, so an apply: can finish after a newer one that ran nested in it.
    uint64_t _applySequence;
    // 每个输入事件与焦点生命周期推进代次，只让可选的上屏格式化过期。
    uint64_t _spacingInputSequence;
    // Bumped when a gloss arrival replaces the view, which can also happen inside an apply:'s marked-text write.
    uint64_t _glossViewSequence;
    // Whether the client may still hold marked text this controller's compositions wrote; see MSIMEApplyTransitionTrackingMarkedText. Wrongly YES costs one redundant clear, wrongly NO leaves a composition stranded in the document, so anything that might mark text sets it.
    BOOL _clientHasMarkedText;
    NSObject *_candidateMenuToken;
    // 候选右键菜单；它不抢客户端的键盘焦点，见 CandidatePointerMenu.h。
    MSIMEPointerMenu *_candidatePointerMenu;
    NSPanel *_panel;
    NSRect _candidateAnchorCaret;
    BOOL _candidateAnchorValid;
    BOOL _candidateFollowCursorMode;
    BOOL _candidateFollowCursorModeKnown;
    MSIMEShuangpinKeymapPanel *_keymapPanel;
    NSString *_preferencesDirectory;
    NSTimer *_preferencesTimer;
    uint64_t _preferenceLoadGeneration;
    BOOL _preferenceLoadInFlight;
    uint64_t _appliedPreferenceRevision;
    MSIMEAppearancePreferences *_appearance;
    BOOL _wubiCodeHintEnabled;
    msime::input::EnglishPunctuationState _englishPunctuation;
    BOOL _capsLock;
    // A physical Backspace hold that began while this controller owned a
    // composition stays ours after one repeat deletes the last preedit byte.
    // Otherwise the next repeat falls through to the client and starts
    // deleting document text even though the user never released the key.
    BOOL _backspaceHoldArmed;
    NSUInteger _requestedPageSize;
    // 上一次展开网格的逐行几何；只有移动光标的重画（`_expandedGridMovesCursor`）拿它比对复用。
    std::optional<MSIMEExpandedGridGeometry> _expandedGridGeometry;
    BOOL _expandedGridMovesCursor;
    NSViewAnimation *_candidateFrameAnimation;
    NSDictionary<NSDictionary *, NSValue *> *_candidateTransitionFrames;
    NSSize _candidateLayoutSize;
    BOOL _candidatePreviousExpanded;
    BOOL _candidatePreviousVisible;
    BOOL _skinShowsSelectedBar;
    std::optional<msime::mac::SkinLayoutGeometry> _candidateLayoutGeometry;
    BOOL _candidateLayoutFitContent;
    // 上一次排版时这种明暗画的皮肤包，空串表示没有画包；外观变化后画的包不同了就要重新排版。
    std::string _renderedCandidateSkin;
    // 上一次排版时皮肤包写了的动效；收起时按它决定退场。
    msime::mac::SkinAnimation _renderedCandidateAnimation;
    // The rendered skin's accent, which the glow selection preset shines in.
    NSColor *_renderedCandidateAccent;
    // The highlighted row's slot plus one when the panel was last shown, 0 for none. Compared by slot rather than candidate identity so typing, which brings new candidates under the same highlight, does not replay the selection preset.
    NSUInteger _animatedHighlightSlot;
    // 记下 _animatedHighlightSlot 时的输入串和页码；换了的话高亮是 Engine 放回去的，不是用户移过去的。
    NSString *_animatedHighlightContext;
    // 候选窗的进场、退场和选中动效，第一次显示候选窗时创建。
    MSIMECandidateMotion *_candidateMotion;
    CFTimeInterval _canvasAnimationAnchor;
    // 上一次排版时皮肤包装饰图的待机动效和周期，显示候选窗时交给 _candidateMotion。
    msime::mac::DecorationMotion _renderedDecorationMotion;
    double _renderedDecorationPeriodMs;
    // APNG 装饰图解码好的各帧，跟着 _appearance 读好的那张装饰图走：它换了（换皮肤、换明暗槽位）才重新解码，打字时不再读文件。
    NSImage *_decorationFramesSource;
    MSIMEDecorationFrames *_decorationFrames;
    BOOL _rerenderingForCandidateSkin;
    BOOL _refreshingCandidateSkin;
    BOOL _candidateSkinRefreshPending;
    CGFloat _tallestVerticalCandidateHeight;
    NSInteger _armedGlossColumn;
    // Ctrl+Enter turns the highlighted candidate's gloss into a page of its senses. The composition
    // is untouched while that page is up - nothing was typed - so leaving it only needs the view
    // that was on screen put back, which is what `_glossSenseSavedState` holds.
    // Japanese conversion in progress: which candidate Space has stepped to, and the reading it
    // belongs to. A reading that has changed is a different conversion, so the pair travels
    // together - the mobile hosts keep exactly this pair for the same reason.
    NSNumber *_japaneseConversionIndex;
    NSString *_japaneseConversionReading;
    NSArray<NSString *> *_glossSenses;
    shurufa::mac::ViewState _glossSenseSavedState;
    NSUInteger _glossSenseCursor;
    // 展开的候选网格（共享偏好 `expand_candidates`，#7307）。
    NSArray<NSDictionary *> *_expandedCandidates;
    // 普通英文只有显式授权后才借用引擎的英文组字；与菜单的专用英文状态分开。
    BOOL _englishOnlineCandidatesEnabled;
    BOOL _englishOnlinePreferenceLoaded;
    BOOL _managedEnglishComposition;
    uint64_t _expandedSession;
    uint64_t _expandedGeneration;
    shurufa::mac::ViewState _expandedSourceState;
    NSUInteger _expandedCursor;
    NSUInteger _expandedFirstRow;
    NSUInteger _expandedVisibleRows;
    CGFloat _expandedScrollOffset;
    CGFloat _expandedViewportHeight;
    BOOL _expandedScrollsViewport;
    MSIMEExpandedCandidateScrollView *_expandedScrollView;
    BOOL _focusPending;
    // Dedicated English lives in the Engine session, so a session released for dictionary maintenance takes it along; the reopen puts it back.
    BOOL _resumeDedicatedEnglish;
    // The last reason prepareSession could not open a session, so the report is made once per reason rather than per focus.
    NSString *_sessionUnavailableReason;
    unichar _lastSmartPunctuation;
    NSTimeInterval _lastSmartPunctuationTime;
    __weak id _smartPunctuationClient;
    unichar _rejectedSmartPunctuation;
    BOOL _smartPunctuationRejected;
    // The last character known to have reached the application (Windows _smartPunctuationShadowChar). Terminal-like hosts never hand typed text back through attributedSubstringFromRange: - keys they pass straight to a pty leave no trace - so passthrough keys and commits are tracked here, and the document is read only while this is invalid. `_smartPunctuationShadowWritten` says a commit or rewrite recorded the shadow during the key event being handled, so the key itself does not overwrite it.
    unichar _smartPunctuationShadow;
    BOOL _smartPunctuationShadowValid;
    BOOL _smartPunctuationShadowWritten;
    // The ASCII form of the Chinese mark a punctuation commit has just left in the document (from the mark map, not the key pressed), and the client it landed in. A space arriving next rewrites that mark as ASCII; anything else disarms.
    unichar _spaceConvertMark;
    __weak id _spaceConvertClient;
    // After a space has turned a Chinese mark into ASCII, the same key within the repeat window writes the Chinese mark back (Windows Kind::AsciiConverted). The key, the mark that was actually replaced, when, and in which client; anything else disarms.
    unichar _spaceRevertKey;
    unichar _spaceRevertChinese;
    NSTimeInterval _spaceRevertTime;
    __weak id _spaceRevertClient;
    __weak id _readableDocumentClient;
    msime::mac::PairedPunctuationTracker _pairedPunctuation;
    // Key heatmap counts not yet written, and the timer that writes them if typing stops before a batch fills.
    msime::mac::KeyPressBatch _keyPressBatch;
    NSTimer *_keyPressFlushTimer;
    // The closing mark this host owes the document while a pair is open. It rides in the marked
    // text after the caret, because IMK gives an input method no way to move a client's insertion
    // point; see MSIMEApplyTransitionWithPendingClosing.
    NSString *_pendingPairedClosing;
    // The closing mark for a pair this host opened itself from the `{` key, which the Engine commits as ASCII and so never names as an opening mark. Consumed by the next apply:.
    NSString *_hostOpenedClosing;
    NSNumber *_typingSourceOverride;
    // 上一个键或激活时安全输入是否打开。打开时不画特效；声音和音乐由生命周期核心按 SECURE 属性关掉，见 secureEventInputActive 和 syncFieldTraits。
    BOOL _secureEventInput;
    // 最后报告给会话的字段属性（MSIME_TRAIT_*），0 是还没报告过。会话新建时清零，之后每个键前比较一次，变了才报告。
    uint32_t _fieldTraits;
    // 打字特效：前台应用是否占着全屏，以及还没画出来的特效答复（按键来自 msime_client_key，上屏来自派发回复的 feedback）。全屏状态要遍历窗口列表，所以放到主队列的下一轮去读，从不在按键上读：激活后、偏好更新后、当前空间切换时（应用进出原生全屏），以及有按键时最多每秒一次（无边框的全屏窗口不切换空间）。按键只在这里记下答复；绘制等到主队列的下一轮，按键从不等它，其间到达的几个键只按最新的计数画一次。
    BOOL _typingEffectFullscreen;
    NSTimeInterval _typingEffectFullscreenCheckedAt;
    BOOL _typingEffectScheduled;
    BOOL _typingEffectCommit;
    uint32_t _typingEffectPacked;
    // The ASCII punctuation key behind the transition about to be applied, or 0. Set only on the punctuation-key routes and consumed by the next apply:, so pairing can read the last mark of a commit that finished a composition (`nihao(` gives `你好（`) without ever rewriting a candidate that merely ends in a mark.
    unichar _punctuationKeyInFlight;
    MSIMEModifierTap _modifierTap;
    msime::mac::EditableShiftGate _editableShiftGate;
    id _editableShiftLocalMouseMonitor;
    id _editableShiftGlobalMouseMonitor;
    NSTimer *_editableShiftMouseTimer;
    // 单按 Caps Lock 切换中英文：大写锁定打开的那一刻布防，轮询到松开时认定是轻点还是长按。
    msime::input::CapsLockTap _capsLockTap;
    NSTimer *_capsLockTapTimer;
    // IOKit 撤回锁定后，窗口服务器可能还会把旧的 Caps Lock 修饰位带进一两个事件；
    // 在看到下一次真实按下前，不要让这份旧状态把工具栏和英文输入重新变成大写。
    BOOL _capsLockTapReleased;
    MSIMEVoiceHoldShortcut _voiceHoldShortcut;
    MSIMEConfiguredVoiceShortcut _configuredVoiceShortcut;
    uint64_t _voiceHoldGeneration;
    BOOL _voiceHoldStarting;
    NSDictionary *_voiceThemePreferences;
    NSDictionary *_menuThemePreferences;
    MSIMEDictionaryWindowController *_dictionaryWindow;
    NSTimer *_cloudTimer;
    MSIMECloudCandidateRequest *_cloudRequest;
    NSDictionary *_cloudQuery;
    uint64_t _cloudEpoch;
    NSOperationQueue *_glossQueue;
    NSDictionary *_glossRequest;
    uint64_t _glossEpoch;
    NSNumber *_glossEnabled;
    NSString *_glossTargetLanguage;
    NSArray<NSString *> *_glossTargetLanguages;
    // 候选释义的显示开关，来自共享偏好；缺省对齐 Windows v0.11.1 出厂配置（显示词性、显示全部释义）。
    BOOL _glossShowPos;
    BOOL _glossSingleSense;
    // 候选徽标：AI 来源角标与固定排位角标的完整串（含前导空格），以及算出它们的原始偏好。
    NSString *_aiBadgeString;
    NSString *_fixedBadgeString;
    BOOL _fixedBadgeEnabled;
    NSString *_fixedBadgeStyle;
    NSString *_aiBadgeStyle;
    NSArray<NSDictionary *> *_glossResults;
    NSOperationQueue *_targetGlossQueue;
    NSDictionary *_targetGlossRequest;
    uint64_t _targetGlossEpoch;
    NSDictionary<NSString *, NSDictionary<NSString *, NSString *> *> *_targetGlossResults;
    // Readings drawn after the gloss lines. The targets the lines follow are nil while the switch is off. English answers
    // come from the shared table and are cached by the text asked for, an empty string meaning it has none; they do not
    // depend on the composition, so the cache outlives it. Japanese is read on demand from the system tokenizer.
    NSArray<NSString *> *_pronunciationTargets;
    NSOperationQueue *_pronunciationQueue;
    NSMutableDictionary<NSString *, NSString *> *_englishPronunciations;
    NSMutableSet<NSString *> *_pronunciationPending;
    NSMutableDictionary<NSString *, NSString *> *_romajiCache;
    MSIMECustomTranslationBatch *_customBatch;
    NSMutableArray<MSIMECustomTranslationBatch *> *_customBatches;
    // Batches whose page went away while a paid request was in flight. They are held here until that request lands, so its answer still reaches the cache and the glossary (Windows' cloud worker caches before its staleness check, cloud_translation.cpp); a batch deallocated early would cancel it.
    NSMutableSet<MSIMECustomTranslationBatch *> *_detachedCustomBatches;
    // Bumped only when custom work is cancelled outright (the provider or its switch changed, or the controller went away); a reply from before that must not reach the cache, since the cache identity carries no credentials.
    uint64_t _customHardEpoch;
    NSTimer *_customTimer;
    NSDictionary *_customQuery;
    NSDictionary *_customTranslationConfig;
    NSDictionary *_tencentTranslationConfig;
    NSDictionary *_niuTransConfig;
    NSDictionary *_aiTranslationConfig;
    NSArray<NSDictionary *> *_customResults;
    // The view generation the held translation results were last applied to. The session drops its translations whenever the generation moves, and a page whose words did not change does not change any request either, so this is what tells synchronizeCandidateServices to put them back.
    NSNumber *_translationAppliedGeneration;
    // Candidate services all inspect the same Engine state during one render pass. Cache each
    // snapshot lazily for that pass; applyTranslations invalidates it before changing the state.
    BOOL _serviceSnapshotActive;
    BOOL _serviceSnapshotQueryLoaded;
    BOOL _serviceSnapshotViewLoaded;
    NSDictionary *_serviceSnapshotQuery;
    NSDictionary *_serviceSnapshotView;
    uint64_t _customEpoch;
    MSIMECustomTranslationBatch *_aiBatch;
    NSTimer *_aiTimer;
    NSDictionary *_aiQuery;
    uint64_t _aiEpoch;
    NSMutableDictionary<NSString *, NSArray<NSString *> *> *_aiCandidateCache;
}

- (NSDictionary *)view {
    return _viewState.toDictionary();
}

- (void)setView:(NSDictionary *)view {
    _viewState = shurufa::mac::ViewState::fromDictionary(view);
}

- (NSDictionary *)renderedCandidateView {
    return _renderedCandidateState.toDictionary();
}

- (void)setRenderedCandidateView:(NSDictionary *)view {
    _renderedCandidateState = shurufa::mac::ViewState::fromDictionary(view);
}

- (void)resetSmartPunctuationState {
    _lastSmartPunctuation = 0;
    _lastSmartPunctuationTime = 0;
    _smartPunctuationClient = nil;
    _rejectedSmartPunctuation = 0;
    _smartPunctuationRejected = NO;
}

- (void)clearSmartPunctuationSpaceConversion {
    _spaceConvertMark = 0;
    _spaceConvertClient = nil;
}

- (void)clearSmartPunctuationSpaceRevert {
    _spaceRevertKey = 0;
    _spaceRevertChinese = 0;
    _spaceRevertTime = 0;
    _spaceRevertClient = nil;
}

- (void)invalidateSmartPunctuationShadow {
    _smartPunctuationShadow = 0;
    _smartPunctuationShadowValid = NO;
}

// Records the character that now sits left of the caret because this host put it there.
- (void)noteSmartPunctuationShadow:(unichar)character {
    if (!character) {
        [self invalidateSmartPunctuationShadow];
        return;
    }
    _smartPunctuationShadow = character;
    _smartPunctuationShadowValid = YES;
    _smartPunctuationShadowWritten = YES;
}

// Port of Windows _UpdateSmartPunctuationShadow, run once per key down after the key has been handled. Editing and caret keys - Delete, Forward Delete, Return, Enter, Tab, Escape, the arrows, Home/End, Page Up/Down - and any Control, Option or Command chord leave the caret somewhere the shadow cannot follow, so they clear it. An eaten key keeps what a commit or rewrite recorded while it was handled and otherwise clears the shadow: it fed the composition, and the document answers until something is committed. A printable key that passed through to the application is the new shadow. The reference skips its smart punctuation keys because its resolver records them itself; here a smart key that is eaten is recorded by that same commit, and one that passes through (English mode, local modes) is what the application typed, so it is recorded like any other passthrough key.
- (void)noteKeyForSmartPunctuationShadow:(NSEvent *)event eaten:(BOOL)eaten {
    if (MSIMEEditionIsShurufa()) return;
    const BOOL written = _smartPunctuationShadowWritten;
    _smartPunctuationShadowWritten = NO;
    if (eaten) {
        if (!written) [self invalidateSmartPunctuationShadow];
        return;
    }
    switch (event.keyCode) {
    case 51: case 117: case 36: case 76: case 48: case 53:
    case 123: case 124: case 125: case 126:
    case 115: case 119: case 116: case 121:
        [self invalidateSmartPunctuationShadow];
        return;
    default:
        break;
    }
    NSString *characters = event.characters;
    const unichar character = characters.length == 1 ? [characters characterAtIndex:0] : 0;
    if ((event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand)) ||
        character < 0x20 || character == 0x7F || (character >= 0xF700 && character <= 0xF8FF)) {
        [self invalidateSmartPunctuationShadow];
        return;
    }
    _smartPunctuationShadow = character;
    _smartPunctuationShadowValid = YES;
}

// The preceding character the direct-output decision works from (Windows _GetPrecedingCharForSmartPunctuation): the shadow first, the document second.
- (uint32_t)precedingForSmartPunctuation:(id<MSIMETextClient>)client {
    return _smartPunctuationShadowValid ? _smartPunctuationShadow : MSIMETextClientPrecedingUnicodeScalar(client);
}

// The posted fallback for a host whose document read returned nothing, so the shadow is the only evidence of what is on screen (the reference's _SmartPunctuationFingerprintMatches trusts the state when it reads nothing back). Overridden in tests.
- (BOOL)postSmartPunctuationRewrite:(unichar)replacement client:(id)client {
    const NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
    return MSIMECaptureSmartPunctuationRewrite(client, now).deliver(replacement);
}

// Whether the host truly exposes no text, rather than a preceding-character read that came back empty for another reason. MSIMETextClientPrecedingUnicodeScalar also returns 0 for a real selection (a mouse drag the shadow never saw) and for a caret at the start of the document; a posted Delete there would erase the selection or insert the mark at the start, so both count as readable. Terminal-like hosts hand back nothing even for the first character, and only they keep the posted route, as the reference only sends input when a collapsed caret's document read comes back empty.
- (BOOL)textClientExposesNoText:(id<MSIMETextClient>)client {
    if (![client respondsToSelector:@selector(selectedRange)]) return YES;
    const NSRange selected = [client selectedRange];
    if (selected.location == NSNotFound) return YES;
    if (selected.length != 0) return NO;
    if (selected.location == 0 && [client respondsToSelector:@selector(attributedSubstringFromRange:)] &&
        [client attributedSubstringFromRange:NSMakeRange(0, 1)].string.length)
        return NO;
    return YES;
}

- (NSString *)readAXPrecedingContextForBundle:(NSString *)bundle maxUTF16Units:(NSUInteger)maxUnits {
    return msime::mac::ReadAXPrecedingContext(bundle, maxUnits);
}

// 仅在空闲会话按下首枚字母键开始中文组字前，读取一次光标左侧现成文本同步给运行时。
// 组字中途、空闲未敲字母、安全密码框、终端或非中文组字方案均不读取，既不产生额外 IPC 也不在空闲时弹出候选。
- (void)syncSurroundingContextForCharacter:(unichar)typed client:(id<MSIMETextClient>)client {
    if (!client || !_session || ![_session respondsToSelector:@selector(setSurroundingContext:error:)]) return;
    if (!((typed >= 'a' && typed <= 'z') || (typed >= 'A' && typed <= 'Z'))) return;
    if (_appearance.englishMode || _managedEnglishComposition || [self secureEventInputActive]) return;
    if (_viewState.hasComposition() || _viewState.dedicatedEnglish)
        return;
    if (!MSIMESchemeRulesApply(_viewState)) return;
    const auto scheme = MSIMEViewScheme(_viewState);
    if (scheme != msime::mac::scheme::Quanpin &&
        scheme != msime::mac::scheme::Shuangpin &&
        scheme != msime::mac::scheme::Wubi)
        return;
    NSString *bundle = [(id)client respondsToSelector:@selector(bundleIdentifier)]
        ? [(id)client bundleIdentifier] : nil;
    if (msime::mac::IsTerminalBundleIdentifier(bundle)) return;
    const BOOL neuralEnabled = _session && [_session respondsToSelector:@selector(neuralV2Enabled)] && _session.neuralV2Enabled;
    const NSUInteger maxContextUnits = neuralEnabled ? 512 : 96;
    const BOOL wantFollowing = neuralEnabled && [_session respondsToSelector:@selector(setFollowingContext:error:)];
    const NSUInteger maxFollowingUnits = wantFollowing ? 32 : 0;
    BOOL provedReadable = NO;
    NSString *following = nil;
    NSString *preceding = MSIMETextClientPrecedingContext(
        client, maxContextUnits, _readableDocumentClient == client, &provedReadable);
    if (provedReadable) _readableDocumentClient = client;
    if (preceding) {
        if (wantFollowing) {
            following = MSIMETextClientFollowingContext(client, maxFollowingUnits);
        }
    } else if (wantFollowing) {
        preceding = msime::mac::ReadAXSurroundingContext(bundle, maxContextUnits, maxFollowingUnits, &following);
        if (!preceding) {
            preceding = [self readAXPrecedingContextForBundle:bundle maxUTF16Units:maxContextUnits];
        }
    } else {
        preceding = [self readAXPrecedingContextForBundle:bundle maxUTF16Units:maxContextUnits];
    }
    if (preceding) {
        [_session setSurroundingContext:preceding error:nil];
        if (wantFollowing && following.length > 0) {
            [_session setFollowingContext:following error:nil];
        }
    }
}

// Replaces the character left of the caret when the host exposes no text there and the shadow says it is `expected`. Returns NO, having posted nothing, when the document is readable (including a selection or a caret at its start), the shadow disagrees or is unknown, or the posted rewrite is not available.
- (BOOL)rewriteUnreadablePreceding:(unichar)expected with:(unichar)replacement client:(id<MSIMETextClient>)client {
    if (!_smartPunctuationShadowValid || _smartPunctuationShadow != expected) return NO;
    if (![self textClientExposesNoText:client]) return NO;
    if (![self postSmartPunctuationRewrite:replacement client:client]) return NO;
    [self noteSmartPunctuationShadow:replacement];
    return YES;
}

// The configured scheme, not the temporary J mode, decides whether the two reversible smart punctuation gestures (space-to-ASCII and repeat-to-Chinese) may run (`host_smart_punctuation`), as Windows KeyEventSink only claims them outside Global::JapaneseInputModeEnabled. Korean and Vietnamese write half-width ASCII punctuation, so neither gesture has a Chinese mark to convert from or back to.
- (BOOL)schemeAllowsSmartPunctuationGestures {
    return msime::mac::scheme::HostSmartPunctuation(MSIMEViewScheme(_viewState));
}

// The scheme's punctuation goes through the Chinese table (`uses_chinese_punctuation`). Where it does not (Korean, Vietnamese) the marks are ASCII already, and the Engine refuses to arm the repeat gesture that would turn them Chinese.
- (BOOL)schemeUsesChinesePunctuation {
    return msime::mac::scheme::UsesChinesePunctuation(MSIMEViewScheme(_viewState));
}

- (void)noteCommittedChinesePunctuationString:(NSString *)commit key:(unichar)key client:(id)client {
    unichar ascii = 0;
    if ([commit isKindOfClass:NSString.class] && commit.length) {
        const unichar mark = [commit characterAtIndex:commit.length - 1];
        const auto marks = MSIMEViewPunctuationMarks(_viewState);
        ascii = MSIMEASCIIForSmartChinesePunctuation(mark, marks);
        // 直角引号的 ASCII 孪生按出的键算：`{` 出的「转回 `{`，不是样式键 `'`。
        if (ascii && MSIMEKeyProducesCornerQuote(key, mark, MSIMEViewScheme(_viewState))) ascii = key;
        // 顿号同理：`/` 出的、`、` 出的各转回自己那个键，关着的键不算顿号来源。
        if (ascii && MSIMEKeyProducesEnumerationComma(key, mark, marks)) ascii = key;
    }
    if (!_appearance.smartPunctuation || !_appearance.smartPunctuationSpaceConvert || !ascii ||
        _pendingPairedClosing || ![self schemeAllowsSmartPunctuationGestures]) {
        [self clearSmartPunctuationSpaceConversion];
        return;
    }
    _spaceConvertMark = ascii;
    _spaceConvertClient = client;
}

- (void)noteCommittedChinesePunctuation:(NSDictionary *)transition key:(unichar)key client:(id)client {
    [self noteCommittedChinesePunctuationString:transition[@"commit"] key:key client:client];
}

// A space right after a Chinese mark the user did not want takes the mark back to ASCII. It is the mirror of repeat-to-Chinese and shares its caution: the preceding character is read back and has to still be the mark that was committed, in the same client, with nothing composing - otherwise a character the user already saw land would be rewritten out from under them. A host that reads back nothing at all (a terminal) is taken on the shadow's word instead and rewritten with posted events, as the reference does when its document read returns nothing.
- (BOOL)convertSmartPunctuationSpace:(NSEvent *)event client:(id<MSIMETextClient>)client {
    if (!_spaceConvertMark) return NO;
    if (event.characters.length != 1 || [event.characters characterAtIndex:0] != ' ' ||
        (event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption |
                                NSEventModifierFlagCommand))) {
        [self clearSmartPunctuationSpaceConversion];
        return NO;
    }
    const unichar mark = _spaceConvertMark;
    id armed = _spaceConvertClient;
    [self clearSmartPunctuationSpaceConversion];
    if (!_appearance.smartPunctuation || !_appearance.smartPunctuationSpaceConvert ||
        _appearance.runtimeFullWidthInput || armed != client || _pendingPairedClosing.length ||
        ![self schemeAllowsSmartPunctuationGestures])
        return NO;
    if (_viewState.hasEditableText() || !_viewState.candidates.empty())
        return NO;
    uint32_t preceding = MSIMETextClientPrecedingUnicodeScalar(client);
    if (!preceding) {
        // Terminals and proxy stores expose no document text; the shadow is the only evidence of what is on screen, and the rewrite has to be posted.
        const unichar shadow = _smartPunctuationShadowValid ? _smartPunctuationShadow : 0;
        const auto marks = MSIMEViewPunctuationMarks(_viewState);
        if (!shadow || !MSIMESmartPunctuationAsciiMatches(shadow, mark, marks) ||
            ![self rewriteUnreadablePreceding:shadow with:mark client:client])
            return NO;
        preceding = shadow;
    } else {
        const auto marks = MSIMEViewPunctuationMarks(_viewState);
        if (!MSIMESmartPunctuationAsciiMatches((unichar)preceding, mark, marks)) return NO;
        const NSRange selected =
            [client respondsToSelector:@selector(selectedRange)] ? [client selectedRange] : NSMakeRange(NSNotFound, 0);
        if (selected.location == NSNotFound || selected.location < 1) return NO;
        NSString *ascii = [NSString stringWithCharacters:&mark length:1];
        [client insertText:ascii replacementRange:NSMakeRange(selected.location - 1, 1)];
        [self noteSmartPunctuationShadow:mark];
    }
    // Windows records chineseLeft as the mark actually read back, so a right quote comes back as a right quote.
    if (_appearance.smartPunctuationRepeatToChinese) {
        _spaceRevertKey = mark;
        _spaceRevertChinese = (unichar)preceding;
        _spaceRevertTime = NSProcessInfo.processInfo.systemUptime;
        _spaceRevertClient = client;
    }
    // This space is the conversion gesture, not document content. Matching the
    // reference also avoids leaving a surprising trailing blank after the user
    // has just corrected the punctuation form.
    return YES;
}

// The same key again right after a space conversion takes the ASCII character back to the Chinese mark it replaced (Windows _CanInterceptSmartPunctuationRevert). It is one-shot: the state is cleared before any check. Every check that fails hands the key to the Engine, which then just types the Chinese mark, as the reference falls back to inserting it.
- (BOOL)revertSmartPunctuationSpace:(NSEvent *)event client:(id<MSIMETextClient>)client {
    if (!_spaceRevertKey) return NO;
    const unichar key = _spaceRevertKey;
    const unichar chinese = _spaceRevertChinese;
    const NSTimeInterval armedAt = _spaceRevertTime;
    id armed = _spaceRevertClient;
    [self clearSmartPunctuationSpaceRevert];
    if (event.characters.length != 1 || [event.characters characterAtIndex:0] != key ||
        (event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand)))
        return NO;
    if (!_appearance.smartPunctuation || !_appearance.smartPunctuationRepeatToChinese ||
        _appearance.runtimeFullWidthInput || armed != client || _pendingPairedClosing.length ||
        ![self schemeAllowsSmartPunctuationGestures] || NSProcessInfo.processInfo.systemUptime - armedAt > 2.0)
        return NO;
    if (_viewState.hasEditableText() || !_viewState.candidates.empty())
        return NO;
    const uint32_t preceding = MSIMETextClientPrecedingUnicodeScalar(client);
    // A host that reads back nothing is rewritten with posted events on the shadow's word, as the conversion was.
    if (!preceding) return [self rewriteUnreadablePreceding:key with:chinese client:client];
    if (preceding != key) return NO;
    const NSRange selected =
        [client respondsToSelector:@selector(selectedRange)] ? [client selectedRange] : NSMakeRange(NSNotFound, 0);
    if (selected.location == NSNotFound || selected.location < 1) return NO;
    [client insertText:[NSString stringWithCharacters:&chinese length:1]
      replacementRange:NSMakeRange(selected.location - 1, 1)];
    [self noteSmartPunctuationShadow:chinese];
    return YES;
}

// 智能标点每次判断记一行（诊断日志开着时），下一份「智能标点没生效」的报告才看得出卡在哪一步。只写是否在组字、影子和前一个字符的来源与类别（数字、字母、其他 ASCII、非 ASCII、没有）以及判断结果，按的是哪个键和字符本身都不写：诊断日志不记按键和输入内容（DiagnosticLog.h）。
static const char *MSIMESmartPunctuationScalarClass(uint32_t scalar) {
    if (!scalar) return "none";
    if (scalar >= '0' && scalar <= '9') return "digit";
    if (scalar < 0x80 && MSIMEASCIIAlphanumeric((unichar)scalar)) return "letter";
    return scalar < 0x80 ? "ascii" : "non_ascii";
}

- (void)logSmartPunctuationComposing:(BOOL)composing source:(const char *)source preceding:(uint32_t)preceding
                            decision:(const char *)decision {
    if (!msime_macos_diagnostic_enabled()) return;
    msime_macos_diagnostic_writef(
        "[smart-punctuation] composing=%d shadow=%s preceding=%s:%s decision=%s", composing ? 1 : 0,
        _smartPunctuationShadowValid ? MSIMESmartPunctuationScalarClass(_smartPunctuationShadow) : "invalid", source,
        MSIMESmartPunctuationScalarClass(preceding), decision);
}

- (BOOL)handleSmartPunctuation:(NSEvent *)event client:(id<MSIMETextClient>)client {
    if (event.characters.length != 1 ||
        (event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand)))
        return NO;
    const unichar character = [event.characters characterAtIndex:0];
    if (!MSIMESmartPunctuationSpaceKey(character)) return NO;
    const BOOL hasComposition = _viewState.hasEditableText() || !_viewState.candidates.empty();
    // Korean and Vietnamese marks are ASCII already, and the repeat gesture that would turn them Chinese never arms there: the key goes to the Engine, which writes the mark after the open composition or leaves it to the application. Zhuyin's marks are bopomofo keys or its Shift overlay, which the Engine writes in any state, so the contextual ASCII route below never takes them either.
    if (![self schemeUsesChinesePunctuation] || MSIMEViewScheme(_viewState) == msime::mac::scheme::Zhuyin) {
        [self resetSmartPunctuationState];
        if (MSIMESmartPunctuationKey(character))
            [self logSmartPunctuationComposing:hasComposition source:"unread" preceding:0 decision:"scheme"];
        return NO;
    }
    if (!_appearance.smartPunctuation) {
        [self resetSmartPunctuationState];
        [self clearSmartPunctuationSpaceConversion];
        [self clearSmartPunctuationSpaceRevert];
        if (MSIMESmartPunctuationKey(character))
            [self logSmartPunctuationComposing:hasComposition source:"unread" preceding:0 decision:"off"];
        return NO;
    }
    const BOOL gestures = [self schemeAllowsSmartPunctuationGestures];
    if (!MSIMESmartPunctuationKey(character)) return NO;
    // Expression mode's decimal point is part of the number being typed, not a mark to convert.
    if (MSIMESpellingSymbol(_viewState, character)) {
        [self logSmartPunctuationComposing:hasComposition source:"unread" preceding:0 decision:"spelling"];
        return NO;
    }
    if (!_appearance.smartPunctuationRepeatToChinese) {
        _lastSmartPunctuation = 0;
        _smartPunctuationRejected = NO;
    }
    const BOOL repeat = _lastSmartPunctuation == character && !_smartPunctuationRejected &&
        _smartPunctuationClient == client && NSProcessInfo.processInfo.systemUptime - _lastSmartPunctuationTime <= 2.0;
    if (repeat && gestures && _appearance.smartPunctuationRepeatToChinese &&
        !_viewState.hasEditableText() && _viewState.candidates.empty()) {
        const uint32_t preceding = MSIMETextClientPrecedingUnicodeScalar(client);
        NSString *expected = MSIMEFullWidthSmartMark(character, _appearance.runtimeFullWidthInput);
        NSString *chinese = MSIMEChinesePunctuationForSmart(character);
        if (preceding && expected.length == 1 && [expected characterAtIndex:0] == (unichar)preceding) {
            const NSRange selected = [client respondsToSelector:@selector(selectedRange)] ? [client selectedRange] : NSMakeRange(NSNotFound, 0);
            const NSUInteger length = expected.length;
            if (selected.location != NSNotFound && selected.location >= length) {
                [self logSmartPunctuationComposing:NO source:"document" preceding:preceding decision:"repeat_to_chinese"];
                [client insertText:chinese replacementRange:NSMakeRange(selected.location - length, length)];
                [self noteSmartPunctuationShadow:[chinese characterAtIndex:chinese.length - 1]];
                [self resetSmartPunctuationState];
                return YES;
            }
        } else if (!preceding && expected.length == 1 && chinese.length == 1 &&
                   [self rewriteUnreadablePreceding:[expected characterAtIndex:0] with:[chinese characterAtIndex:0] client:client]) {
            // Nothing to read back: the shadow vouches for the ASCII mark just committed, and posted events replace it. Without the posting permission this falls through and the Engine types the Chinese mark after it, as the reference did before its SendInput rewrite.
            [self resetSmartPunctuationState];
            [self logSmartPunctuationComposing:NO source:"shadow" preceding:[expected characterAtIndex:0] decision:"repeat_to_chinese_posted"];
            return YES;
        }
    }
    if (_lastSmartPunctuation && _lastSmartPunctuation != character) [self resetSmartPunctuationState];
    const BOOL rejected = _smartPunctuationRejected && _rejectedSmartPunctuation == character;
    uint32_t preceding = 0;
    const char *source = hasComposition ? "candidate" : rejected ? "unread" : _smartPunctuationShadowValid ? "shadow" : "document";
    if (hasComposition) {
        if (const auto *hl = _viewState.highlightedCandidate()) {
            NSString *text = hl->text;
            if ([text isKindOfClass:NSString.class] && text.length && MSIMEASCIIAlphanumeric([text characterAtIndex:text.length - 1]))
                preceding = [text characterAtIndex:text.length - 1];
        }
    } else if (!rejected) {
        preceding = [self precedingForSmartPunctuation:client];
    }
    if (!rejected && preceding && (preceding < 0x80) && MSIMEASCIIAlphanumeric((unichar)preceding)) {
        _punctuationKeyInFlight = character;
        BOOL arm = YES;
        if ([_session respondsToSelector:@selector(punctuationSnapshot:preceding:hasPreceding:literalASCII:error:)]) {
            const ShurufaTransitionSnapshot *snapshot = [_session punctuationSnapshot:(uint8_t)character
                                                                            preceding:preceding
                                                                         hasPreceding:!hasComposition
                                                                         literalASCII:hasComposition
                                                                                error:nil];
            if (!snapshot) {
                _punctuationKeyInFlight = 0;
                [self logSmartPunctuationComposing:hasComposition source:source preceding:preceding decision:"session_error"];
                return NO;
            }
            if (hasComposition || snapshot->handled) {
                NSString *commit = snapshot->has_commit ? shurufa::mac::SliceToNSString(snapshot->commit) : nil;
                const BOOL endsWithASCII = [commit isKindOfClass:NSString.class] && commit.length &&
                    [commit characterAtIndex:commit.length - 1] == character;
                [self logSmartPunctuationComposing:hasComposition source:source preceding:preceding
                                          decision:hasComposition || endsWithASCII ? "ascii" : "engine_commit"];
                NSString *overrideCommit = nil;
                if (_appearance.runtimeFullWidthInput && endsWithASCII) {
                    overrideCommit = [[commit substringToIndex:commit.length - 1]
                        stringByAppendingString:MSIMEFullWidthSmartMark(character, YES)];
                }
                [self applyTransitionSnapshot:snapshot overrideCommit:overrideCommit];
            } else {
                arm = [_appearance.punctuationLock isEqual:@"follow"] && _appearance.runtimeChinesePunctuation;
                [self logSmartPunctuationComposing:NO source:source preceding:preceding
                                          decision:arm ? "ascii_literal" : "ascii_literal_unarmed"];
                [self applyTransitionSnapshot:snapshot
                               overrideCommit:MSIMEFullWidthSmartMark(character, _appearance.runtimeFullWidthInput)];
            }
        } else {
            NSDictionary *transition = hasComposition
                ? [_session punctuationASCII:(uint8_t)character error:nil]
                : [_session punctuation:(uint8_t)character preceding:preceding error:nil];
            if (!transition) {
                _punctuationKeyInFlight = 0;
                [self logSmartPunctuationComposing:hasComposition source:source preceding:preceding decision:"session_error"];
                return NO;
            }
            if (hasComposition || MSIMEStrictBoolean(transition[@"handled"])) {
                NSString *commit = transition[@"commit"];
                const BOOL endsWithASCII = [commit isKindOfClass:NSString.class] && commit.length &&
                    [commit characterAtIndex:commit.length - 1] == character;
                [self logSmartPunctuationComposing:hasComposition source:source preceding:preceding
                                          decision:hasComposition || endsWithASCII ? "ascii" : "engine_commit"];
                if (_appearance.runtimeFullWidthInput && endsWithASCII) {
                    NSMutableDictionary *converted = [transition mutableCopy];
                    converted[@"commit"] = [[commit substringToIndex:commit.length - 1]
                        stringByAppendingString:MSIMEFullWidthSmartMark(character, YES)];
                    transition = converted;
                }
                [self apply:transition];
            } else {
                arm = [_appearance.punctuationLock isEqual:@"follow"] && _appearance.runtimeChinesePunctuation;
                [self logSmartPunctuationComposing:NO source:source preceding:preceding
                                          decision:arm ? "ascii_literal" : "ascii_literal_unarmed"];
                NSMutableDictionary *literal = [transition mutableCopy];
                literal[@"handled"] = @YES;
                literal[@"commit"] = MSIMEFullWidthSmartMark(character, _appearance.runtimeFullWidthInput);
                [self apply:literal];
            }
        }
        if (!arm) {
            [self resetSmartPunctuationState];
            return YES;
        }
        _lastSmartPunctuation = character;
        _lastSmartPunctuationTime = NSProcessInfo.processInfo.systemUptime;
        _smartPunctuationClient = client;
        _smartPunctuationRejected = NO;
        _rejectedSmartPunctuation = 0;
        return YES;
    }
    if (rejected) [self resetSmartPunctuationState];
    [self logSmartPunctuationComposing:hasComposition source:source preceding:preceding decision:rejected ? "rejected" : "engine"];
    // Falling through means the Engine takes the key and commits the Chinese mark; the typeASCII route in handleEvent: notes that commit so a space arriving next can take it back, and the conversion re-reads the document before touching anything.
    return NO;
}

- (void)cancelAITranslations {
    ++_aiEpoch;
    [_aiTimer invalidate]; _aiTimer = nil;
    [_aiBatch cancel]; _aiBatch = nil;
    _aiQuery = nil;
}

- (void)cancelCustomTranslations {
    ++_customEpoch;
    ++_customHardEpoch;
    [_customTimer invalidate];
    _customTimer = nil;
    [_customBatch cancel];
    for (MSIMECustomTranslationBatch *batch in [_customBatches copy])
        if (batch != _customBatch) [batch cancel];
    [_customBatches removeAllObjects];
    for (MSIMECustomTranslationBatch *batch in [_detachedCustomBatches copy]) [batch cancel];
    [_detachedCustomBatches removeAllObjects];
    _customBatch = nil;
    _customQuery = nil;
    _customResults = nil;
}
// The page moved on: nothing more is sent for it, but a request already paid for is left to land in the cache instead of being thrown away.
- (void)detachCustomTranslations {
    ++_customEpoch;
    [_customTimer invalidate];
    _customTimer = nil;
    if (!_detachedCustomBatches) _detachedCustomBatches = [NSMutableSet set];
    __weak MSIMEInputController *weakSelf = self;
    for (MSIMECustomTranslationBatch *batch in [_customBatches copy]) {
        __weak MSIMECustomTranslationBatch *weakBatch = batch;
        BOOL running = [batch detachWithCompletion:^{
            MSIMEInputController *owner = weakSelf;
            MSIMECustomTranslationBatch *ended = weakBatch;
            if (owner && ended) [owner->_detachedCustomBatches removeObject:ended];
        }];
        if (running) [_detachedCustomBatches addObject:batch];
    }
    [_customBatches removeAllObjects];
    _customBatch = nil;
    _customQuery = nil;
    _customResults = nil;
}
- (void)cancelCandidateTranslations {
    [self cancelCandidateGloss];
    [self cancelTargetGloss];
    [self cancelCustomTranslations];
    [self cancelAITranslations];
    // A later session can reuse the same generation number, and nothing is held now to re-apply anyway.
    _translationAppliedGeneration = nil;
}

- (NSDictionary *)highlightedCandidateForGloss {
    if (const auto *hl = _viewState.highlightedCandidate()) return hl->rowDictionary();
    return nil;
}

// Ctrl+Enter offers the highlighted candidate's gloss as a page of its senses.
- (BOOL)glossSensePageActive { return _glossSenses.count > 0; }

- (NSArray<NSString *> *)sensesForHighlightedCandidate {
    NSDictionary *candidate = [self highlightedCandidateForGloss];
    NSString *gloss = CandidateTranslation(candidate);
    if (gloss.length == 0 || gloss.length > 4096) return @[];
    const std::string utf8 = gloss.UTF8String ? gloss.UTF8String : "";
    NSMutableArray<NSString *> *senses = [NSMutableArray array];
    for (const auto &sense : msime::mac::candidate_gloss_senses(utf8)) {
        NSString *text = [[NSString alloc] initWithBytes:sense.data() length:sense.size()
                                               encoding:NSUTF8StringEncoding];
        if (text.length) [senses addObject:text];
    }
    return senses;
}

// The page the panel draws while the senses are up: the same shape as an Engine view, so the
// renderer, the placement and the skin all work unchanged.
- (shurufa::mac::ViewState)glossSenseViewState {
    shurufa::mac::ViewState state = _glossSenseSavedState;
    state.valid = true;
    state.pagingValid = true;
    state.cachedDictionary = nil;
    state.cachedPageIdentity = nil;
    const NSUInteger pageSize = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    const NSUInteger page = _glossSenseCursor / pageSize;
    const NSUInteger start = page * pageSize;
    state.candidates.clear();
    for (NSUInteger index = start; index < MIN(start + pageSize, _glossSenses.count); ++index) {
        shurufa::mac::CandidateRow row;
        row.text = _glossSenses[index];
        row.highlighted = (index == _glossSenseCursor);
        state.candidates.push_back(row);
    }
    state.page = page;
    state.pageCount = (_glossSenses.count + pageSize - 1) / pageSize;
    return state;
}

- (void)showGlossSensePage:(NSArray<NSString *> *)senses {
    _glossSenses = senses;
    _glossSenseCursor = 0;
    _glossSenseSavedState = _viewState;
    _armedGlossColumn = 0;
    _viewState = [self glossSenseViewState];
    [self renderCandidates];
}

// 见 InputControllerActivity.h。普通英文的托管组字也写在 editing_text 里；语音从开始录到识别结果交回期间，三种请求总有一个不为空，录音时语音服务本身也是 active。
- (BOOL)hasUnfinishedInput {
    return _viewState.hasEditableText() || _liveVoiceToken || _doubaoVoiceRequest || _httpVoiceRequest || _voiceService.active;
}

BOOL MSIMEInputControllerHasUnfinishedInput(void) {
    MSIMEInputController *focused = MSIMEFocusedController;
    return focused != nil && [focused hasUnfinishedInput];
}

- (void)discardGlossSensePage {
    _glossSenses = nil;
    _glossSenseSavedState = {};
    _glossSenseCursor = 0;
}

- (void)leaveGlossSensePage {
    if (!_glossSenses.count) return;
    _glossSenses = nil;
    _glossSenseCursor = 0;
    if (_glossSenseSavedState.valid) _viewState = _glossSenseSavedState;
    _glossSenseSavedState = {};
    [self renderCandidates];
}

- (BOOL)commitGlossSenseAtIndex:(NSUInteger)index client:(id)sender {
    if (index >= _glossSenses.count || ![sender respondsToSelector:@selector(insertText:replacementRange:)])
        return NO;
    NSString *sense = _glossSenses[index];
    if (_appearance.traditionalOutput && MSIMEScriptConversionAppliesState(_viewState)) sense = MSIMEChineseOutputString(sense, YES);
    [self discardGlossSensePage];
    [(id<MSIMETextClient>)sender insertText:sense replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    if (![self dispatchCommand:MSIME_CANCEL_AFTER_HOST_COMMIT]) [self renderCandidates];
    return YES;
}

// Every key while the page is up. Anything this does not claim closes the page and is then handled
// as usual, so no key is swallowed by a mode the user has forgotten about.
- (BOOL)handleGlossSenseEvent:(NSEvent *)event client:(id)sender {
    if (!_glossSenses.count || event.type != NSEventTypeKeyDown) return NO;
    const NSEventModifierFlags modifiers = event.modifierFlags &
        (NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand);
    const NSUInteger pageSize = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    const NSUInteger count = _glossSenses.count;
    if (modifiers == 0) {
        const int slot = msime::mac::CandidateDigitSlotOnPage(msime::mac::PhysicalCandidateDigitSlot(event.keyCode), pageSize);
        if (slot >= 0) {
            const NSUInteger index = (_glossSenseCursor / pageSize) * pageSize + (NSUInteger)slot;
            if (index < count && (NSUInteger)slot < pageSize) return [self commitGlossSenseAtIndex:index client:sender];
            return YES; // A slot this page does not have stays inside the page rather than typing.
        }
        switch (event.keyCode) {
            case 49: case 36: case 76: // Space and both Returns take the highlighted sense.
                return [self commitGlossSenseAtIndex:_glossSenseCursor client:sender];
            case 53: [self leaveGlossSensePage]; return YES;
            case 125: case 124: // Down and right move to the next sense.
                if (_glossSenseCursor + 1 < count) ++_glossSenseCursor;
                _viewState = [self glossSenseViewState];
                [self renderCandidates];
                return YES;
            case 126: case 123:
                if (_glossSenseCursor > 0) --_glossSenseCursor;
                _viewState = [self glossSenseViewState];
                [self renderCandidates];
                return YES;
            case 121: // Page down.
                _glossSenseCursor = MIN(count - 1, _glossSenseCursor + pageSize);
                _viewState = [self glossSenseViewState];
                [self renderCandidates];
                return YES;
            case 116:
                _glossSenseCursor = _glossSenseCursor > pageSize ? _glossSenseCursor - pageSize : 0;
                _viewState = [self glossSenseViewState];
                [self renderCandidates];
                return YES;
            default: break;
        }
    }
    [self leaveGlossSensePage];
    return NO;
}

// ---- 展开的候选网格（#7307） ----
//
// 「展开候选」打开时，横排候选窗按 ↓ 或点候选窗上的展开按钮，把这一代的全部候选排成多行：一行是当前每页候选数，可见几行（kExpandedGridVisibleRows），上下滚动，高亮所在的行始终可见。网格开着时它接管键盘：←/→ 在行内移动，↑/↓ 在行之间移动，翻页键一次滚过可见的几行，空格和回车选高亮的那一个，数字键选高亮那一行里的第几个，Esc 或第一行再按 ↑ 收起网格而组字保留；它不认的键先收起网格，再照常处理，免得被一个用户忘了的模式吞掉。网格外的方向键规则不变（见 handleKeyEvent:client: 里方向键那一段）。
//
// 网格不经 View 的分页：打开时经 msime_client_all_candidates 复制一次全部候选，选择时用 msime_client_select_any_candidate 带着复制时的一代，Engine 拒绝过期的一代。每次重画前对照一次当前视图：组字没变而候选换了一代（云候选、AI 回答、整句重排到达）就按新的一代重载，组字变了或这一面不该再有网格（偏好关掉、改成竖排、切到英文、候选列表打开、失焦）就收起，和 Android、iOS 的展开面板一样。

// 网格能否继续显示：偏好打开、横排、没有打开候选列表（韩文汉字、注音）或释义页；新展开入口另外要求多页。
- (BOOL)expandedCandidatesAllowed {
    if (!_session || !_activeClient || !_appearance.expandCandidates || _appearance.vertical || _appearance.englishMode) return NO;
    if ([self glossSensePageActive] || MSIMECandidateListOpen(_viewState) || !_viewState.valid || !_viewState.focused) return NO;
    NSAppearance *current = [_appearance candidateAppearanceOverride] ?: _panel.effectiveAppearance ?: NSApp.effectiveAppearance;
    NSString *match = [current bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
    const auto skin = [_appearance candidateWindowSkinForDark:[match isEqual:NSAppearanceNameDarkAqua]];
    if (skin.expandPanel.has_value() && !*skin.expandPanel) return NO;
    return YES;
}

- (BOOL)expandedCandidatesOffered {
    if (![self expandedCandidatesAllowed]) return NO;
    return _viewState.pagingValid && _viewState.pageCount > 1;
}

// msime_client_all_candidates 的回答里网格要画的候选：只认当前视图这个会话、这一代、身份合法的条目，按 Engine 给的顺序，和普通候选窗画那一页时一样。
- (NSArray<NSDictionary *> *)expandedCandidatesFromSnapshot:(NSDictionary *)snapshot {
    if (![snapshot isKindOfClass:NSDictionary.class] || ![snapshot[@"candidates"] isKindOfClass:NSArray.class]) return nil;
    uint64_t snapshotSession = 0, snapshotGeneration = 0;
    if (!_viewState.valid || !_viewState.identityValid ||
        !MSIMEStrictUnsignedInteger(snapshot[@"session"], &snapshotSession) ||
        !MSIMEStrictUnsignedInteger(snapshot[@"generation"], &snapshotGeneration) ||
        snapshotSession != _viewState.session || snapshotGeneration != _viewState.generation) return nil;
    NSMutableArray<NSDictionary *> *valid = [NSMutableArray array];
    for (id entry in (NSArray *)snapshot[@"candidates"])
        if ([entry isKindOfClass:NSDictionary.class] && !MSIMECandidateIsAIPlaceholder(entry) && MSIMECurrentCandidateIdentity(entry[@"id"], _viewState)) [valid addObject:entry];
    return valid;
}

- (void)discardExpandedCandidates {
    _expandedCandidates = nil;
    _expandedGridGeometry.reset();
    _expandedSession = 0;
    _expandedGeneration = 0;
    _expandedSourceState = {};
    _expandedCursor = 0;
    _expandedFirstRow = 0;
    _expandedScrollOffset = 0;
    _expandedViewportHeight = 0;
    _expandedScrollsViewport = NO;
    _expandedScrollView = nil;
}

// 收起网格时把高亮交回普通候选窗；只导航，不选词或提交组字。
- (void)closeExpandedCandidates {
    if (!_expandedCandidates) return;
    NSDictionary *identifier = _expandedCursor < _expandedCandidates.count ? _expandedCandidates[_expandedCursor][@"id"] : nil;
    NSUInteger target = 0, current = NSNotFound;
    uint64_t identifierGeneration = 0;
    const BOOL valid = MSIMECurrentCandidateIdentity(identifier, _viewState) &&
        MSIMEStrictUnsignedInteger(identifier[@"generation"], &identifierGeneration) &&
        identifierGeneration == _expandedGeneration &&
        MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &target);
    if (const auto *highlighted = _viewState.highlightedCandidate(); highlighted && highlighted->hasIdentity) {
        current = highlighted->index;
    }
    [self discardExpandedCandidates];
    const NSUInteger columns = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    if ([_session respondsToSelector:@selector(commandSnapshot:error:)]) {
        const ShurufaTransitionSnapshot *lastSnapshot = nullptr;
        while (valid && current != NSNotFound && current != target) {
            const uint32_t command = current / columns < target / columns ? MSIME_NEXT_PAGE
                                   : current / columns > target / columns ? MSIME_PREVIOUS_PAGE
                                   : current < target ? MSIME_NEXT_CANDIDATE : MSIME_PREVIOUS_CANDIDATE;
            const ShurufaTransitionSnapshot *next = [_session commandSnapshot:command error:nil];
            if (!next) {
                msime_macos_diagnostic_write("expanded_candidates_close_navigation_failed");
                break;
            }
            lastSnapshot = next;
            if (command == MSIME_NEXT_PAGE) current = (current / columns + 1) * columns;
            else if (command == MSIME_PREVIOUS_PAGE) current = (current / columns - 1) * columns;
            else if (command == MSIME_NEXT_CANDIDATE) ++current;
            else --current;
        }
        if (lastSnapshot) [self applyTransitionSnapshot:lastSnapshot overrideCommit:nil];
        else [self renderCandidates];
    } else {
        NSDictionary *transition = nil;
        while (valid && current != NSNotFound && current != target) {
            const uint32_t command = current / columns < target / columns ? MSIME_NEXT_PAGE
                                   : current / columns > target / columns ? MSIME_PREVIOUS_PAGE
                                   : current < target ? MSIME_NEXT_CANDIDATE : MSIME_PREVIOUS_CANDIDATE;
            NSDictionary *next = [_session command:command error:nil];
            if (!next) {
                msime_macos_diagnostic_write("expanded_candidates_close_navigation_failed");
                break;
            }
            transition = next;
            if (command == MSIME_NEXT_PAGE) current = (current / columns + 1) * columns;
            else if (command == MSIME_PREVIOUS_PAGE) current = (current / columns - 1) * columns;
            else if (command == MSIME_NEXT_CANDIDATE) ++current;
            else --current;
        }
        if (transition) [self apply:transition];
        else [self renderCandidates];
    }
}

// 打开网格，高亮停在普通候选窗正高亮的那一个上。没有更多候选、或者网格画不出来（光标位置无效）时返回 NO，调用方照旧处理这个键。
- (BOOL)openExpandedCandidates {
    return [self openExpandedCandidatesAfterSelection:NO];
}

// 部分选词沿用已展开的展示方式，剩一页也不要求用户重新展开。
- (BOOL)openExpandedCandidatesAfterSelection:(BOOL)afterSelection {
    if (_expandedCandidates || !(afterSelection ? [self expandedCandidatesAllowed] : [self expandedCandidatesOffered])) return NO;
    NSArray<NSDictionary *> *entries = [self expandedCandidatesFromSnapshot:[_session allCandidatesWithError:nil]];
    NSArray *page = _viewState.visibleCandidatesArray();
    if (!entries.count || (!afterSelection && entries.count <= page.count)) return NO;
    const NSUInteger columns = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    // 没有高亮时停在当前这一页的第一个。
    NSUInteger cursor = MIN(_viewState.page * columns, entries.count - 1);
    if (const auto *highlighted = _viewState.highlightedCandidate()) {
        if (NSDictionary *highlightedId = highlighted->idDictionary()) {
            for (NSUInteger index = 0; index < entries.count; ++index)
                if ([entries[index][@"id"] isEqual:highlightedId]) { cursor = index; break; }
        }
    }
    _expandedCandidates = entries;
    _expandedSession = _viewState.session;
    _expandedGeneration = _viewState.generation;
    _expandedSourceState = _viewState;
    _expandedCursor = cursor;
    _expandedScrollOffset = 0;
    _expandedViewportHeight = 0;
    _expandedScrollsViewport = NO;
    _expandedScrollView = nil;
    // 当前高亮所在页是网格的第一行，末页减少显示行数，不向前补行。
    _expandedFirstRow = cursor / columns;
    _expandedVisibleRows = msime::candidate::kExpandedGridVisibleRows;
    [self renderCandidates];
    return _expandedCandidates != nil;
}

// 重画之前对照当前视图，见本节开头。返回 NO 表示网格该收起。
- (BOOL)expandedCandidatesStillApply {
    if (!_expandedCandidates.count || !_session || !_activeClient || !_appearance.expandCandidates || _appearance.vertical ||
        _appearance.englishMode || [self glossSensePageActive] || MSIMECandidateListOpen(_viewState) ||
        !_viewState.valid || !_viewState.focused)
        return NO;
    if (_viewState == _expandedSourceState) return YES;
    if (_viewState.session != _expandedSession || !_viewState.sameExpandedComposition(_expandedSourceState)) return NO;
    NSArray<NSDictionary *> *entries = [self expandedCandidatesFromSnapshot:[_session allCandidatesWithError:nil]];
    if (!entries.count) return NO;
    // 光标留在原来那个词上；它不在新的一代里时夹在范围内。
    NSString *text = _expandedCursor < _expandedCandidates.count ? _expandedCandidates[_expandedCursor][@"text"] : nil;
    NSUInteger cursor = MIN(_expandedCursor, entries.count - 1);
    if ([text isKindOfClass:NSString.class])
        for (NSUInteger index = 0; index < entries.count; ++index)
            if ([entries[index][@"text"] isEqual:text]) { cursor = index; break; }
    _expandedCandidates = entries;
    _expandedGeneration = _viewState.generation;
    _expandedSourceState = _viewState;
    _expandedCursor = cursor;
    _expandedScrollOffset = 0;
    _expandedViewportHeight = 0;
    _expandedScrollsViewport = NO;
    _expandedScrollView = nil;
    return YES;
}

// 选网格里的第 index 个。复制时的一代已经不是当前这一代时什么也不选，只收起网格；`pointer` 是鼠标点选，先开一轮（见 msime_client.h 的顺序契约）。
- (void)selectExpandedCandidateAtIndex:(NSUInteger)index pointer:(BOOL)pointer {
    if (!_expandedCandidates || !_session) return;
    NSDictionary *identifier = index < _expandedCandidates.count ? _expandedCandidates[index][@"id"] : nil;
    uint64_t generation = 0; NSUInteger engineIndex = 0;
    const BOOL current = MSIMECurrentCandidateIdentity(identifier, _viewState) &&
        MSIMEStrictUnsignedInteger(identifier[@"generation"], &generation) &&
        generation == _expandedGeneration &&
        MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &engineIndex);
    const uint64_t sessionID = _viewState.session;
    [self discardExpandedCandidates];
    if (!current) { [self renderCandidates]; return; }
    if (pointer) [self reportPointerToLifecycle];
    id client = _activeClient;
    id session = _session;
    const uint64_t sequence = _applySequence;
    BOOL canReopen = NO;
    if ([_session respondsToSelector:@selector(selectAnyCandidateSnapshotGeneration:index:error:)]) {
        const ShurufaTransitionSnapshot *selected = [_session selectAnyCandidateSnapshotGeneration:generation index:engineIndex error:nil];
        if (!selected) { [self renderCandidates]; return; }
        const BOOL handled = selected->handled ? YES : NO;
        const BOOL hasCommit = selected->has_commit && selected->commit.len > 0;
        const shurufa::mac::ViewState nextState = shurufa::mac::ViewState::fromSnapshot(selected->view);
        [self applyTransitionSnapshot:selected overrideCommit:nil];
        canReopen = handled && !hasCommit && nextState.valid && nextState.session == sessionID &&
            _viewState.valid && _viewState.session == sessionID && nextState.hasEditableText() &&
            _viewState.sameExpandedComposition(nextState);
    } else {
        NSDictionary *selected = [_session selectAnyCandidateGeneration:generation index:engineIndex error:nil];
        if (!selected) { [self renderCandidates]; return; }
        [self apply:selected];
        NSDictionary *nextView = selected[@"view"];
        const shurufa::mac::ViewState nextState = shurufa::mac::ViewState::fromDictionary(nextView);
        canReopen = MSIMEStrictBoolean(selected[@"handled"]) && ![selected[@"commit"] length] &&
            nextState.valid && nextState.session == sessionID && _viewState.valid &&
            _viewState.session == sessionID && nextState.hasEditableText() &&
            _viewState.sameExpandedComposition(nextState);
    }
    // 上屏回调可能重入输入或切换客户端，旧选择不能重新打开已经结束的网格。
    if (_applySequence == sequence + 1 && _activeClient == client && _session == session && canReopen)
        [self openExpandedCandidatesAfterSelection:YES];
}

// 普通滚轮保留按行定位；触控板与惯性由 NSScrollView 处理。
- (void)scrollExpandedCandidatesBackwards:(BOOL)backwards {
    const NSUInteger count = _expandedCandidates.count;
    const NSUInteger columns = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    if (!count || ![_expandedScrollView scrollByRowBackwards:backwards]) return;
    const NSRange visible = _expandedScrollView.visibleRowRange;
    _expandedScrollsViewport = YES;
    const NSUInteger row = _expandedCursor / columns;
    const NSUInteger column = _expandedCursor % columns;
    const NSUInteger target = std::clamp(row, visible.location, NSMaxRange(visible) - 1);
    _expandedCursor = MIN(target * columns + column, count - 1);
    [self renderCandidatesMovingExpandedCursor];
}

// 网格开着时的每一个键，见本节开头。返回 NO 时网格已经收起，这个键照常往下走。
- (BOOL)handleExpandedCandidateEvent:(NSEvent *)event client:(id)sender {
    (void)sender;
    if (!_expandedCandidates || event.type != NSEventTypeKeyDown) return NO;
    if (![self expandedCandidatesStillApply]) {
        [self closeExpandedCandidates];
        return NO;
    }
    const NSEventModifierFlags modifiers = event.modifierFlags &
        (NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand);
    const NSUInteger count = _expandedCandidates.count;
    const NSUInteger columns = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    std::optional<msime::candidate::ExpandedGridMove> move;
    if (modifiers == 0) {
        // 数字键选高亮那一行里的第几个，和普通候选窗选当前页一样；数字是正在拼写的输入（Unicode、表达式模式）或九键组字时不是选词键。
        const int slot = msime::mac::CandidateDigitSlotOnPage(msime::mac::PhysicalCandidateDigitSlot(event.keyCode), columns);
        if (slot >= 0 && !_viewState.nineKey &&
            !MSIMESpellingSymbol(_viewState, (unichar)msime::mac::PhysicalCandidateDigitCharacter(slot))) {
            const NSUInteger row = _expandedCursor / columns;
            const NSUInteger index = row * columns + (NSUInteger)slot;
            if (index < count) [self selectExpandedCandidateAtIndex:index pointer:NO];
            return YES;
        }
        switch (event.keyCode) {
            case 123: move = msime::candidate::ExpandedGridMove::Left; break;
            case 124: move = msime::candidate::ExpandedGridMove::Right; break;
            case 125: move = msime::candidate::ExpandedGridMove::Down; break;
            case 126: move = msime::candidate::ExpandedGridMove::Up; break;
            case 116: move = msime::candidate::ExpandedGridMove::PageUp; break;
            case 121: move = msime::candidate::ExpandedGridMove::PageDown; break;
            case 48:
                if ([_appearance navigationEnabled:@"tab"]) move = msime::candidate::ExpandedGridMove::PageDown;
                break;
            case 49:
                if (MSIMESpellingSymbol(_viewState, ' ')) break;
                [self selectExpandedCandidateAtIndex:_expandedCursor pointer:NO];
                return YES;
            case 36: case 76:
                [self selectExpandedCandidateAtIndex:_expandedCursor pointer:NO];
                return YES;
            case 53:
                [self closeExpandedCandidates];
                return YES;
            default: {
                const int direction = msime::mac::PhysicalCandidatePageDirection(event.keyCode);
                NSString *binding = event.keyCode == 27 || event.keyCode == 24 ? @"minus_equal"
                                  : event.keyCode == 33 || event.keyCode == 30 ? @"brackets"
                                  : event.keyCode == 43 || event.keyCode == 47 ? @"comma_period" : nil;
                const BOOL japaneseMinusEqual = msime::mac::IsJapaneseMinusEqualKey(
                    MSIMEViewScheme(_viewState), [_viewState.localMode isEqual:@"temporary_japanese"], event.keyCode, 0);
                if (direction != 0 && binding && [_appearance navigationEnabled:binding] && !japaneseMinusEqual &&
                    !MSIMESpellingSymbolString(_viewState, event.charactersIgnoringModifiers) && ![self wordCharacterClaimsEvent:event])
                    move = direction < 0 ? msime::candidate::ExpandedGridMove::PageUp : msime::candidate::ExpandedGridMove::PageDown;
                break;
            }
        }
    } else if (modifiers == NSEventModifierFlagShift && event.keyCode == 48 && [_appearance navigationEnabled:@"tab"]) {
        move = msime::candidate::ExpandedGridMove::PageUp;
    }
    if (!move) {
        [self closeExpandedCandidates];
        return NO;
    }
    const auto step = msime::candidate::StepExpandedGrid(_expandedCursor, count, columns, MAX((NSUInteger)1, _expandedVisibleRows), *move);
    if (step.close) {
        [self closeExpandedCandidates];
        return YES;
    }
    _expandedScrollOffset = 0;
    _expandedScrollsViewport = NO;
    _expandedCursor = step.cursor;
    [self renderCandidatesMovingExpandedCursor];
    return YES;
}

// 只有光标在网格里移动的这一次重画可以沿用逐行几何；标记只在这次调用里有效。
- (void)renderCandidatesMovingExpandedCursor {
    _expandedGridMovesCursor = YES;
    [self renderCandidates];
    _expandedGridMovesCursor = NO;
}

- (BOOL)commitCandidateGlossColumn:(NSInteger)column candidate:(NSDictionary *)candidate client:(id)sender {
    if (!_session || ![sender respondsToSelector:@selector(insertText:replacementRange:)] ||
        ![candidate isKindOfClass:NSDictionary.class] || column <= 0) return NO;
    NSString *gloss = MSIMECandidateTranslationColumn(candidate, column);
    if (!gloss.length) return NO;
    if (_appearance.traditionalOutput && MSIMEScriptConversionAppliesState(_viewState)) gloss = MSIMEChineseOutputString(gloss, YES);
    [(id<MSIMETextClient>)sender insertText:gloss replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    _armedGlossColumn = 0;
    [self dispatchCommand:MSIME_CANCEL_AFTER_HOST_COMMIT];
    return YES;
}

- (BOOL)commitHighlightedGlossColumn:(NSInteger)column client:(id)sender {
    return [self commitCandidateGlossColumn:column candidate:[self highlightedCandidateForGloss] client:sender];
}

- (BOOL)cycleArmedGlossColumnBackwards:(BOOL)backwards {
    if (!_session || !_viewState.hasEditableText()) return NO;
    NSDictionary *candidate = [self highlightedCandidateForGloss];
    if (!candidate) return NO;
    BOOL hasPrimary = MSIMECandidateTranslationColumn(candidate, 1).length > 0;
    BOOL hasSecondary = MSIMECandidateTranslationColumn(candidate, 2).length > 0;
    if (!hasPrimary && !hasSecondary) return NO;
    NSMutableArray<NSNumber *> *available = [NSMutableArray arrayWithObject:@0];
    if (hasPrimary) [available addObject:@1];
    if (hasSecondary) [available addObject:@2];
    NSUInteger current = [available indexOfObject:@(_armedGlossColumn)];
    if (current == NSNotFound) current = 0;
    NSInteger step = backwards ? -1 : 1;
    NSInteger next = (NSInteger)current + step;
    if (next < 0) next = (NSInteger)available.count - 1;
    if (next >= (NSInteger)available.count) next = 0;
    _armedGlossColumn = available[(NSUInteger)next].integerValue;
    return YES;
}

- (void)synchronizeAITranslations {
    // AI suggestions are gated by the AI assistant switch alone, like the source's ai_eligible / UpdateAiInput. The gloss switch (candidate_translations) only governs glosses and translations, so it is deliberately not checked here.
    if (!_activeClient || !_session || _focusPending || (_appearance.englishMode && !_managedEnglishComposition) || [self secureEventInputActive]) { [self cancelAITranslations]; return; }
    NSDictionary *online = [_session onlineQueryWithError:nil];
    NSDictionary *config = online[@"ai_assistant"];
    if (![config isKindOfClass:NSDictionary.class] || !MSIMEStrictBoolean(config[@"enabled"]) ||
        !MSIMEOnlineHasAIInput(online)) { [self cancelAITranslations]; return; }
    if (!_aiCandidateCache) _aiCandidateCache = [NSMutableDictionary dictionary];
    NSDictionary *query = @{ @"online": online, @"config": config };
    if ([_aiQuery isEqual:query]) return;
    [self cancelAITranslations]; _aiQuery = query;
    NSDictionary *descriptor = [_session aiRequestForQuery:online error:nil];
    if (!descriptor) {
        // A malformed or temporarily unavailable provider descriptor must not
        // poison this query identity. A later render may observe corrected
        // settings and should be allowed to construct a fresh request.
        _aiQuery = nil;
        // 这次发不出请求：告诉 Host API 没有回答（#6888）。共享内核已经不放 AI 占位行，它答 applied:NO、不改候选列表。
        NSDictionary *resolved = [_session resolveOnlineCandidatesForSource:1 query:online error:nil];
        if (MSIMEStrictBoolean(resolved[@"applied"])) [self apply:resolved];
        return;
    }
    NSString *cacheKey = MSIMEAICacheKey(online, descriptor);
    NSArray<NSString *> *cachedCandidates = (!_viewState.valid || !MSIMEViewContainsAICandidate(_viewState)) && cacheKey
        ? _aiCandidateCache[cacheKey] : nil;
    if (cachedCandidates.count) {
        NSDictionary *transition = [_session applyOnlineCandidates:cachedCandidates source:1 query:online error:nil];
        if (MSIMEStrictBoolean(transition[@"applied"])) {
            // The same shape the provider path records, not the bare online query: this value is compared
            // against a freshly built @{online, config} on the next render, and a bare one never matches,
            // so the render that follows this apply asked the session for another descriptor.
            NSDictionary *postOnline = [_session onlineQueryWithError:nil];
            NSDictionary *postConfig = postOnline[@"ai_assistant"];
            _aiQuery = [postOnline isKindOfClass:NSDictionary.class] && [postConfig isKindOfClass:NSDictionary.class]
                ? @{ @"online": [postOnline copy], @"config": [postConfig copy] } : nil;
            [self apply:transition];
            return;
        }
    }
    NSArray *items = @[ @{ @"text": @"ai", @"request": descriptor,
        @"candidate_limit": config[@"candidate_limit"] ?: @3 } ];
    uint64_t epoch = _aiEpoch; MSIMEClientSession *session = _session; id client = _activeClient;
    __weak MSIMEInputController *weakSelf = self;
    _aiTimer = [NSTimer timerWithTimeInterval:0.65 repeats:NO block:^(NSTimer *timer) {
        MSIMEInputController *owner = weakSelf;
        if (!owner || owner->_aiEpoch != epoch || owner->_aiTimer != timer || owner->_session != session || owner->_activeClient != client ||
            owner->_focusPending || [owner secureEventInputActive] ||
            (owner->_appearance.englishMode && !owner->_managedEnglishComposition) ||
            ![[session onlineQueryWithError:nil] isEqual:online]) return;
        owner->_aiTimer = nil;
        owner->_aiBatch = [owner aiBatchForItems:items completion:^(NSArray *results) {
            MSIMEInputController *current = weakSelf;
            if (!current || current->_aiEpoch != epoch || current->_session != session || current->_activeClient != client || ![current->_aiQuery isEqual:query] ||
                current->_focusPending || [current secureEventInputActive] ||
                (current->_appearance.englishMode && !current->_managedEnglishComposition)) return;
            current->_aiBatch = nil;
            NSMutableArray *texts = [NSMutableArray array];
            for (NSDictionary *result in results) if ([result[@"translation"] isKindOfClass:NSString.class]) [texts addObject:result[@"translation"]];
            NSDictionary *transition = [session applyOnlineCandidates:texts source:1 query:query[@"online"] error:nil];
            if (MSIMEStrictBoolean(transition[@"applied"])) {
                // Cache what the session took, not what the provider said. Caching before the attempt
                // meant a refused suggestion was kept and served straight back on the next render, so the
                // retry re-applied the text the session had just declined instead of asking again.
                if (texts.count && cacheKey) {
                    if (current->_aiCandidateCache.count >= 4096) [current->_aiCandidateCache removeAllObjects];
                    current->_aiCandidateCache[cacheKey] = [texts copy];
                }
                // apply_online_candidates advances the shared generation. Keep
                // the post-apply identity before applying the view so the
                // render pass does not enqueue the same AI request again.
                NSDictionary *postOnline = [session onlineQueryWithError:nil];
                NSDictionary *postConfig = postOnline[@"ai_assistant"];
                current->_aiQuery = [postOnline isKindOfClass:NSDictionary.class] &&
                    [postConfig isKindOfClass:NSDictionary.class]
                    ? @{ @"online": [postOnline copy], @"config": [postConfig copy] } : nil;
                [current apply:transition];
            } else {
                // 回答为空、失败或被拒：告诉 Host API 这次没有回答（#6888）。共享内核已经不放 AI 占位行，它答 applied:NO、不改候选列表。
                NSDictionary *resolved = [session resolveOnlineCandidatesForSource:1 query:query[@"online"] error:nil];
                if (MSIMEStrictBoolean(resolved[@"applied"])) {
                    [current apply:resolved];
                } else if (texts.count) {
                    // 有回答却被拒：等本地候选重建后的下一次渲染再试。
                    current->_aiQuery = nil;
                }
                // 回答为空或失败：同一组字不再重发，查询身份留着，组字一变它就不相等了。
            }
        }];
        [owner->_aiBatch start];
    }];
    // 输入法进入事件跟踪模式时仍启动请求，避免 AI 回答一直等一个未发出的请求。
    [NSRunLoop.mainRunLoop addTimer:_aiTimer forMode:NSRunLoopCommonModes];
}
- (MSIMECustomTranslationBatch *)aiBatchForItems:(NSArray<NSDictionary *> *)items
                                       completion:(void (^)(NSArray<NSDictionary *> *))completion {
    return [[MSIMECustomTranslationBatch alloc]
        initWithAIItems:items
           configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration
              completion:completion];
}
- (NSDictionary *)serviceSnapshotQuery {
    if (!_serviceSnapshotActive) return [_session translationQueryWithError:nil];
    if (!_serviceSnapshotQueryLoaded) {
        _serviceSnapshotQueryLoaded = YES;
        _serviceSnapshotQuery = [_session translationQueryWithError:nil];
    }
    return _serviceSnapshotQuery;
}
- (NSDictionary *)serviceSnapshotView {
    if (!_serviceSnapshotActive) {
        return [_session respondsToSelector:@selector(viewSnapshotWithError:)] && _viewState.valid
            ? _viewState.toDictionary()
            : [_session viewWithError:nil];
    }
    if (!_serviceSnapshotViewLoaded) {
        _serviceSnapshotViewLoaded = YES;
        _serviceSnapshotView = [_session respondsToSelector:@selector(viewSnapshotWithError:)] && _viewState.valid
            ? _viewState.toDictionary()
            : [_session viewWithError:nil];
    }
    return _serviceSnapshotView;
}
- (void)invalidateServiceSnapshots {
    _serviceSnapshotActive = NO;
    _serviceSnapshotQueryLoaded = NO;
    _serviceSnapshotViewLoaded = NO;
    _serviceSnapshotQuery = nil;
    _serviceSnapshotView = nil;
}
- (NSDictionary *)currentCustomTranslationRequest {
    if (!_activeClient || !_session || _focusPending || _appearance.englishMode) return nil;
    // A `/fy` request (command mode) is the user's explicit ask, so the shared query carries it whatever the candidate translation switches say: one English sentence for the selected service of the user's own, into the query's own target language, with no offline dictionary in front of it. None of the gloss gates below apply to it, so the query is read before them, which costs one call per pass while translations are off.
    const BOOL candidateTranslations = !(_appearance && !_appearance.candidateTranslations) && !(_glossEnabled && !_glossEnabled.boolValue);
    NSDictionary *query = [self serviceSnapshotQuery];
    const BOOL command = MSIMEStrictBoolean(query[@"sentence"]);
    if (!candidateTranslations && !command) return nil;
    NSDictionary *config = query[@"niutrans"];
    BOOL niuTrans = [config isKindOfClass:NSDictionary.class] && MSIMEStrictBoolean(config[@"enabled"]);
    if (!niuTrans) config = query[@"custom_translation"];
    BOOL custom = !niuTrans && [config isKindOfClass:NSDictionary.class] && MSIMEStrictBoolean(config[@"enabled"]);
    BOOL tencent = NO;
    if (!niuTrans && !custom) {
        config = query[@"tencent_tmt"];
        tencent = [config isKindOfClass:NSDictionary.class] && MSIMEStrictBoolean(config[@"enabled"]);
    }
    BOOL ai = NO;
    if (!niuTrans && !custom && !tencent) {
        config = query[@"ai_assistant"];
        ai = [config isKindOfClass:NSDictionary.class] && MSIMEStrictBoolean(config[@"enabled"]);
    }
    if (![config isKindOfClass:NSDictionary.class] || !MSIMEStrictBoolean(config[@"enabled"])) return nil;
    // A preference snapshot can be pending in Engine while the composition is active.
    if ((niuTrans && _niuTransConfig && ![_niuTransConfig isEqual:config]) ||
        (!niuTrans && MSIMEStrictBoolean(_niuTransConfig[@"enabled"])) ||
        (custom && _customTranslationConfig && ![_customTranslationConfig isEqual:config]) ||
        (!niuTrans && !custom && !ai && (MSIMEStrictBoolean(_customTranslationConfig[@"enabled"]) ||
            (_tencentTranslationConfig && ![_tencentTranslationConfig isEqual:config]))) ||
        (ai && _aiTranslationConfig && ![_aiTranslationConfig isEqual:config]) ||
        (!command && _glossTargetLanguages && ![_glossTargetLanguages isEqual:MSIMETranslationTargets(query)])) return nil;
    NSDictionary *view = [self serviceSnapshotView];
    if (command) {
        NSArray *sentences = [query[@"candidates"] isKindOfClass:NSArray.class] ? query[@"candidates"] : @[];
        NSDictionary *sentence = sentences.count == 1 && [sentences.firstObject isKindOfClass:NSDictionary.class] ? sentences.firstObject : nil;
        NSString *text = [sentence[@"text"] isKindOfClass:NSString.class] ? sentence[@"text"] : nil;
        NSString *target = [query[@"target_language"] isKindOfClass:NSString.class] ? query[@"target_language"] : nil;
        if (!text.length || !target.length || ![view[@"generation"] isEqual:query[@"generation"]]) return nil;
        return @{@"command":@YES, @"target_language":target, @"target_languages":@[target],
            (niuTrans ? @"niutrans" : custom ? @"custom_translation" : tencent ? @"tencent_tmt" : @"ai_assistant"):config, @"candidates":@[@{@"text":text}]};
    }
    // Korean Hanja rows are translated like Chinese ones; their 훈음 is drawn whatever comes back. Japanese, including temporary Japanese composition, is still left out.
    if (!MSIMEViewShowsGlosses(view) || ![view[@"generation"] isEqual:query[@"generation"]]) return nil;
    NSDictionary *gloss = [self currentGlossRequest];
    // Resolve the local dictionary first; never transmit an already-resolved key.
    if (gloss && (![_glossRequest isEqual:gloss] || !_glossResults)) return nil;
    // 只问查询列出的候选：「仅当前高亮」时就是高亮的那一个。
    NSSet<NSString *> *requested = MSIMEQueryCandidateTexts(query);
    NSMutableArray *candidates = [NSMutableArray array];
    for (NSDictionary *candidate in view[@"candidates"]) {
        if (![candidate[@"text"] isKindOfClass:NSString.class] || ![candidate[@"source"] isKindOfClass:NSNumber.class]) continue;
        if (requested && ![requested containsObject:candidate[@"text"]]) continue;
        [candidates addObject:@{@"text":candidate[@"text"], @"source":candidate[@"source"]}];
    }
    if (!candidates.count) return nil;
    return @{@"target_language":query[@"target_language"], @"target_languages":MSIMETranslationTargets(query),
        (niuTrans ? @"niutrans" : custom ? @"custom_translation" : tencent ? @"tencent_tmt" : @"ai_assistant"):config, @"candidates":[candidates copy],
        @"directory":_preferencesDirectory ?: @""};
}
// 目标语言离线词典分别回答单一语言，合并时保留英文释义并按目标语言顺序排列。
- (NSArray<NSDictionary *> *)offlineGlossResults:(NSDictionary *)targetGloss english:(NSArray<NSDictionary *> *)english {
    NSMutableDictionary<NSString *, NSMutableDictionary<NSString *, NSString *> *> *values = [NSMutableDictionary dictionary];
    void (^fill)(id, NSString *, id) = ^(id text, NSString *target, id value) {
        if (![text isKindOfClass:NSString.class] || ![value isKindOfClass:NSString.class] || ![(NSString *)value length]) return;
        NSMutableDictionary *byTarget = values[text];
        if (!byTarget) { byTarget = [NSMutableDictionary dictionary]; values[text] = byTarget; }
        if (!byTarget[target]) byTarget[target] = value;
    };
    for (NSDictionary *entry in english) fill(entry[@"text"], @"en", entry[@"translation"]);
    [targetGloss enumerateKeysAndObjectsUsingBlock:^(NSString *text, NSDictionary *byTarget, BOOL *stop) {
        (void)stop;
        for (NSString *target in byTarget) fill(text, target, byTarget[target]);
    }];
    NSMutableArray *texts = [NSMutableArray array];
    for (NSDictionary *candidate in targetGloss ? _targetGlossRequest[@"candidates"] : @[]) [texts addObject:candidate[@"text"]];
    for (NSDictionary *entry in english) if (![texts containsObject:entry[@"text"]]) [texts addObject:entry[@"text"]];
    NSArray *targets = (targetGloss ? _targetGlossRequest : nil)[@"target_languages"];
    NSMutableArray *results = [NSMutableArray array];
    for (NSString *text in texts) {
        NSString *translation = MSIMEJoinedTranslations(values[text], targets);
        if (translation.length) [results addObject:@{@"text":text, @"translation":translation}];
    }
    return results;
}
// gloss_query 的回答：不带服务的那份查询照普通查询处理时只有离线词典答得上（英文词典连同学到的释义、其他语言的离线词典），和上面的合并方式相同；系统翻译、账号和自己的服务都不参与。每条标 gloss，会话把它画在每一行。
- (NSArray<NSDictionary *> *)pageGlossResults:(NSDictionary *)glossQuery english:(NSArray<NSDictionary *> *)english
                                  targetGloss:(NSDictionary *)targetGloss {
    if (!MSIMEStrictBoolean(glossQuery[@"english_gloss"])) english = nil;
    NSArray *languages = [glossQuery[@"offline_gloss_languages"] isKindOfClass:NSArray.class] ? glossQuery[@"offline_gloss_languages"] : @[];
    if (!languages.count) targetGloss = nil;
    NSArray *targets = MSIMETranslationTargets(glossQuery);
    NSArray *candidates = [glossQuery[@"candidates"] isKindOfClass:NSArray.class] ? glossQuery[@"candidates"] : @[];
    NSMutableArray *results = [NSMutableArray array];
    for (NSDictionary *candidate in candidates) {
        NSString *text = [candidate isKindOfClass:NSDictionary.class] ? candidate[@"text"] : nil;
        if (![text isKindOfClass:NSString.class]) continue;
        NSMutableDictionary<NSString *, NSString *> *values = [NSMutableDictionary dictionary];
        for (NSDictionary *entry in english)
            if ([entry[@"text"] isEqual:text] && [entry[@"translation"] isKindOfClass:NSString.class] && [entry[@"translation"] length]) {
                values[@"en"] = entry[@"translation"];
                break;
            }
        for (NSString *language in languages) {
            NSString *value = targetGloss[text][language];
            if ([value isKindOfClass:NSString.class] && value.length) values[language] = value;
        }
        // 只有英文词典时照旧原样交英文释义，有其他语言的词典时按目标语言的顺序逐行合并，和主查询那一份一样。
        NSString *translation = targetGloss ? MSIMEJoinedTranslations(values, targets) : values[@"en"];
        if (translation.length) [results addObject:@{@"text":text, @"translation":translation, @"gloss":@YES}];
    }
    return results;
}
// Returns whether the session took the results and the services were synchronized behind them.
- (BOOL)applyCandidateTranslationResults {
    const BOOL ownSnapshot = !_serviceSnapshotActive;
    if (ownSnapshot) {
        _serviceSnapshotActive = YES;
        _serviceSnapshotQueryLoaded = NO;
        _serviceSnapshotViewLoaded = NO;
        _serviceSnapshotQuery = nil;
        _serviceSnapshotView = nil;
    }
    NSMutableArray *results = [NSMutableArray array];
    BOOL customCurrent = _customResults && [_customQuery isEqual:[self currentCustomTranslationRequest]];
    BOOL glossCurrent = _glossResults && [_glossRequest isEqual:[self currentGlossRequest]];
    NSDictionary *targetGloss = _targetGlossResults.count && [_targetGlossRequest isEqual:[self currentTargetGlossRequest]]
        ? _targetGlossResults : nil;
    if (customCurrent && _customResults.count) [results addObjectsFromArray:_customResults];
    else if (targetGloss) [results addObjectsFromArray:[self offlineGlossResults:targetGloss english:glossCurrent ? _glossResults : nil]];
    else if (glossCurrent) [results addObjectsFromArray:_glossResults];
    // 上面这些是候选词翻译，只交查询列出的候选（「仅当前高亮」时离线词典为 gloss_query 查了整页）。gloss_query 的离线释义整页都有，每条标 gloss：会话把它画在每一行，高亮行还没有候选词翻译时也画它。
    NSDictionary *query = [self serviceSnapshotQuery];
    NSSet<NSString *> *requested = MSIMEQueryCandidateTexts(query);
    if (requested)
        [results filterUsingPredicate:[NSPredicate predicateWithBlock:^BOOL(NSDictionary *entry, NSDictionary *bindings) {
            (void)bindings;
            return [requested containsObject:entry[@"text"]];
        }]];
    NSDictionary *glossQuery = MSIMEGlossQuery(query);
    if (glossQuery)
        [results addObjectsFromArray:[self pageGlossResults:glossQuery english:glossCurrent ? _glossResults : nil targetGloss:targetGloss]];
    NSDictionary *view = [self serviceSnapshotView];
    if (!view) {
        if (ownSnapshot) [self invalidateServiceSnapshots];
        return NO;
    }
    // An asynchronous provider can leave the cached service snapshot one generation ahead of
    // the page currently on screen. If that provider has no glosses to apply, sending an empty
    // payload with the newer generation would advance the Engine behind the user's back and
    // make the next synchronization repeat the same empty pass. Record the displayed generation
    // as attempted, refresh the service snapshots, and let the normal fallback chain continue.
    uint64_t serviceGeneration = 0;
    if (!results.count && _viewState.valid &&
        MSIMEStrictUnsignedInteger(view[@"generation"], &serviceGeneration) &&
        _viewState.generation != serviceGeneration) {
        _translationAppliedGeneration = @(_viewState.generation);
        [self invalidateServiceSnapshots];
        [self synchronizeCandidateServices];
        return YES;
    }
    // Applying translations advances the Engine snapshot. Any enclosing service pass must
    // fetch the new query/view before it asks another provider to synchronize.
    [self invalidateServiceSnapshots];
    uint64_t generation = 0;
    if (!MSIMEStrictUnsignedInteger(view[@"generation"], &generation)) return NO;
    NSDictionary *applied = [_session applyTranslations:MSIMESessionTranslations(results) generation:generation error:nil];
    if (!MSIMEStrictBoolean(applied[@"applied"])) return NO;
    NSDictionary *next = applied[@"view"];
    if (![next isKindOfClass:NSDictionary.class]) return NO;
    shurufa::mac::ViewState nextState = shurufa::mac::ViewState::fromDictionary(next);
    _translationAppliedGeneration = @(nextState.generation);
    if (!(nextState == _viewState)) {
        [self discardGlossSensePage];
        _viewState = nextState;
        ++_glossViewSequence;
        [self renderCandidates];
    }
    // What one source answered decides what the next asks for, even when it answered nothing for this page: the online fallback waits for the offline lookup.
    [self synchronizeCandidateServices];
    return YES;
}

- (MSIMECustomTranslationBatch *)customBatchForItems:(NSArray<NSDictionary *> *)items completion:(void (^)(NSArray<NSDictionary *> *))completion {
    return [[MSIMECustomTranslationBatch alloc] initWithItems:items configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration completion:completion];
}
- (MSIMECustomTranslationBatch *)tencentBatchForItems:(NSArray<NSDictionary *> *)items config:(NSDictionary *)config
                                         completion:(void (^)(NSArray<NSDictionary *> *))completion {
    return [[MSIMECustomTranslationBatch alloc] initWithTencentItems:items config:config
        configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration completion:completion];
}
- (MSIMECustomTranslationBatch *)niuTransBatchForItems:(NSArray<NSDictionary *> *)items config:(NSDictionary *)config
                                           completion:(void (^)(NSArray<NSDictionary *> *))completion {
    return [[MSIMECustomTranslationBatch alloc] initWithNiuTransItems:items config:config
        configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration completion:completion];
}
- (NSTimer *)customTranslationTimerWithBlock:(void (^)(NSTimer *))block {
    NSTimer *timer = [NSTimer timerWithTimeInterval:0.5 repeats:NO block:block];
    [NSRunLoop.mainRunLoop addTimer:timer forMode:NSRunLoopCommonModes];
    return timer;
}
- (void)synchronizeCustomTranslations {
    NSDictionary *query = [self currentCustomTranslationRequest];
    if (!query) {
        [self detachCustomTranslations];
        return;
    }
    if ([_customQuery isEqual:query]) return;
    [self detachCustomTranslations];
    // `/fy` translates one English sentence into the query's own target, Chinese, which the candidate target list does not name and the candidate plan refuses; it is its own plan item and is neither read from nor written to the gloss cache.
    const BOOL command = MSIMEStrictBoolean(query[@"command"]);
    NSArray<NSString *> *targets = command ? query[@"target_languages"] : MSIMETranslationTargets(query);
    NSMutableArray<NSDictionary *> *chunks = [NSMutableArray array];
    NSMutableDictionary<NSString *, NSMutableDictionary<NSString *, NSString *> *> *values = [NSMutableDictionary dictionary];
    MSIMETranslationCache *cache = [MSIMETranslationCache sharedCache];
    BOOL tencent = query[@"tencent_tmt"] != nil;
    BOOL ai = query[@"ai_assistant"] != nil;
    BOOL niuTrans = query[@"niutrans"] != nil;
    NSString *scope = MSIMETranslationProviderScope(niuTrans ? @"niutrans" : tencent ? @"tencent" : ai ? @"ai" : @"custom",
        niuTrans ? query[@"niutrans"] : tencent ? query[@"tencent_tmt"] : ai ? query[@"ai_assistant"] : query[@"custom_translation"]);
    if (!scope) return;
    _customQuery = query;
    NSSet *glossTexts = [NSSet setWithArray:[_glossResults valueForKey:@"text"] ?: @[]];
    if ([_glossRequest isEqual:[self currentGlossRequest]]) {
        for (NSDictionary *result in _glossResults) {
            NSString *text = result[@"text"];
            NSString *translation = result[@"translation"];
            if (![text isKindOfClass:NSString.class] || ![translation isKindOfClass:NSString.class] || !translation.length) continue;
            NSMutableDictionary *byTarget = values[text];
            if (!byTarget) { byTarget = [NSMutableDictionary dictionary]; values[text] = byTarget; }
            byTarget[@"en"] = translation;
        }
    }
    for (NSString *target in targets) {
        NSMutableArray *targetCandidates = [NSMutableArray array];
        for (NSDictionary *candidate in query[@"candidates"]) {
            // A packaged/user English gloss already satisfies the English row. Keep
            // other target rows eligible when English is only the secondary language.
            if ([target isEqual:@"en"] && [glossTexts containsObject:candidate[@"text"]]) continue;
            [targetCandidates addObject:candidate];
        }
        if (!targetCandidates.count) continue;
        NSArray *plan = command ? @[@{@"text":targetCandidates.firstObject[@"text"], @"key":targetCandidates.firstObject[@"text"],
            @"source_language":@"en", @"target_language":target}]
            : [MSIMEClientSession customTranslationPlan:@{@"target_language":target, @"candidates":targetCandidates} error:nil];
        NSMutableArray *pending = [NSMutableArray array];
        NSMutableDictionary *identities = [NSMutableDictionary dictionary];
        for (NSDictionary *item in plan) {
            NSArray *identity = @[scope, target, item[@"source_language"], item[@"target_language"], item[@"key"]];
            NSString *workKey = MSIMETranslationWorkKey(target, item[@"text"]);
            id value = command ? nil : [cache valueForIdentity:identity];
            if (value) {
                if ([value isKindOfClass:NSString.class]) {
                    NSMutableDictionary *byTarget = values[item[@"text"]];
                    if (!byTarget) { byTarget = [NSMutableDictionary dictionary]; values[item[@"text"]] = byTarget; }
                    byTarget[target] = value;
                }
                continue;
            }
            if (tencent || niuTrans) {
                [pending addObject:item];
                if (!command) identities[workKey] = identity;
                continue;
            }
            if (ai) {
                NSDictionary *request = @{ @"config":query[@"ai_assistant"], @"input":@{
                    @"text":item[@"key"], @"source_language":item[@"source_language"],
                    @"target_language":item[@"target_language"] }};
                NSDictionary *descriptor = [MSIMEClientSession aiTranslationHTTPRequest:request error:nil];
                if (descriptor) {
                    [pending addObject:@{@"text":item[@"text"], @"request":descriptor}];
                    if (!command) identities[workKey] = identity;
                } else if (!command) [cache rememberTranslation:nil identity:identity];
                continue;
            }
            NSDictionary *descriptor = [MSIMEClientSession customTranslationHTTPRequest:@{@"config":query[@"custom_translation"],
                @"text":item[@"key"], @"source_language":item[@"source_language"], @"target_language":item[@"target_language"]} error:nil];
            if (descriptor) {
                [pending addObject:@{@"text":item[@"text"], @"request":descriptor}];
                if (!command) identities[workKey] = identity;
            } else if (!command) {
                [cache rememberTranslation:nil identity:identity];
            }
        }
        for (NSUInteger offset = 0; offset < pending.count; offset += 9) {
            NSUInteger length = MIN((NSUInteger)9, pending.count - offset);
            NSArray *items = [pending subarrayWithRange:NSMakeRange(offset, length)];
            NSMutableDictionary *chunk = [@{@"target":target, @"items":items} mutableCopy];
            NSMutableDictionary *chunkIdentities = [NSMutableDictionary dictionary];
            for (NSDictionary *item in items) {
                NSString *key = MSIMETranslationWorkKey(target, item[@"text"]);
                NSArray *identity = identities[key];
                if (identity) chunkIdentities[key] = identity;
            }
            chunk[@"identities"] = chunkIdentities;
            [chunks addObject:chunk];
        }
    }
    NSMutableArray *(^combinedResults)(void) = ^NSMutableArray *{
        NSMutableArray *result = [NSMutableArray array];
        for (NSDictionary *candidate in query[@"candidates"]) {
            NSString *text = candidate[@"text"];
            NSString *translation = MSIMEJoinedTranslations(values[text], targets);
            if (translation.length) [result addObject:@{@"text":text, @"translation":translation}];
        }
        return result;
    };
    _customResults = [combinedResults() copy];
    if (_customResults.count) [self applyCandidateTranslationResults];
    if (!chunks.count) return;
    if (![[self currentCustomTranslationRequest] isEqual:query]) return;
    uint64_t epoch = _customEpoch;
    uint64_t hardEpoch = _customHardEpoch;
    MSIMEClientSession *session = _session;
    id client = _activeClient;
    __weak MSIMEInputController *weakSelf = self;
    _customTimer = [self customTranslationTimerWithBlock:^(NSTimer *timer) {
        MSIMEInputController *owner = weakSelf;
        if (!owner || owner->_customEpoch != epoch || owner->_customTimer != timer) return;
        [timer invalidate]; owner->_customTimer = nil;
        if (owner->_session != session || owner->_activeClient != client ||
            ![[owner currentCustomTranslationRequest] isEqual:query]) return;
        owner->_customBatches = [NSMutableArray array];
        for (NSDictionary *chunk in chunks) {
            NSString *target = chunk[@"target"];
            NSArray *items = chunk[@"items"];
            NSDictionary *identities = chunk[@"identities"];
            // Each response is cached and saved as it lands, whether or not its page is still on screen, as Windows caches and runs PersistGloss before its staleness check (cloud_translation.cpp). Only the items a response answered are cached: one the deadline kept from being sent is left free to be asked again, where caching the whole chunk used to hide it for eight minutes.
            void (^onReply)(NSArray<NSDictionary *> *, NSArray<NSString *> *) = ^(NSArray<NSDictionary *> *results, NSArray<NSString *> *answeredTexts) {
                MSIMEInputController *latest = weakSelf;
                if (!latest || latest->_customHardEpoch != hardEpoch) return;
                for (NSString *text in answeredTexts) {
                    NSString *translation = nil;
                    for (NSDictionary *result in results)
                        if ([result[@"text"] isEqual:text]) { translation = result[@"translation"]; break; }
                    NSArray *identity = identities[MSIMETranslationWorkKey(target, text)];
                    if (identity) [cache rememberTranslation:translation identity:identity];
                }
                // Windows saves every successful English fetch to the user gloss store (cloud_translation.cpp PersistGloss), not just the committed candidate.
                if ([target isEqual:@"en"]) [latest persistFetchedTranslations:results forQuery:query];
            };
            void (^completion)(NSArray<NSDictionary *> *) = ^(NSArray<NSDictionary *> *results) {
                MSIMEInputController *latest = weakSelf;
                if (!latest || latest->_customEpoch != epoch || latest->_session != session || latest->_activeClient != client ||
                    ![[latest currentCustomTranslationRequest] isEqual:query]) return;
                for (NSDictionary *item in items) {
                    NSString *text = item[@"text"];
                    NSString *translation = nil;
                    for (NSDictionary *result in results)
                        if ([result[@"text"] isEqual:text]) { translation = result[@"translation"]; break; }
                    if (translation.length) {
                        NSMutableDictionary *byTarget = values[text];
                        if (!byTarget) { byTarget = [NSMutableDictionary dictionary]; values[text] = byTarget; }
                        byTarget[target] = translation;
                    }
                }
                latest->_customResults = [combinedResults() copy];
                [latest applyCandidateTranslationResults];
                latest->_customBatch = nil;
            };
            MSIMECustomTranslationBatch *batch = niuTrans ? [owner niuTransBatchForItems:items config:query[@"niutrans"] completion:completion]
                : tencent ? [owner tencentBatchForItems:items config:query[@"tencent_tmt"] completion:completion]
                : ai ? [[MSIMECustomTranslationBatch alloc] initWithAIItems:items configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration completion:completion]
                : [owner customBatchForItems:items completion:completion];
            batch.onReply = onReply;
            owner->_customBatch = batch;
            [owner->_customBatches addObject:batch];
            [batch start];
        }
    }];
}
- (void)cancelCandidateGloss {
    ++_glossEpoch;
    [_glossQueue cancelAllOperations];
    _glossRequest = nil;
    _glossResults = nil;
}
- (NSDictionary *)currentGlossRequest {
    if (!_activeClient || !_session || _focusPending || _appearance.englishMode ||
        (_appearance && !_appearance.candidateTranslations && !_appearance.candidateEnglishGloss) ||
        (_glossEnabled && !_glossEnabled.boolValue && !_appearance.candidateEnglishGloss)) return nil;
    NSDictionary *query = [self serviceSnapshotQuery];
    NSArray *targets = MSIMETranslationTargets(query);
    if (!query || ![targets containsObject:@"en"]) return nil;
    if (_glossTargetLanguages && ![_glossTargetLanguages isEqual:targets]) return nil;
    NSDictionary *view = [self serviceSnapshotView];
    // Windows suppresses candidate translations in Japanese, including a temporary Japanese composition whose view retains its original scheme. Korean Hanja rows are glossed like Chinese ones: the gloss request also looks a Traditional Hanja up under its Simplified characters.
    if (!MSIMEViewShowsGlosses(view)) return nil;
    if (![view[@"generation"] isEqual:query[@"generation"]]) return nil;
    // 查询列出的候选（「仅当前高亮」时只有高亮的那一个），加上 gloss_query 要英文离线释义的整页：一次读完，高亮在这一页里移动时请求不变，不用再读。
    NSMutableSet<NSString *> *requested = [MSIMEQueryCandidateTexts(query) mutableCopy];
    NSDictionary *glossQuery = MSIMEGlossQuery(query);
    if (requested && MSIMEStrictBoolean(glossQuery[@"english_gloss"])) [requested unionSet:MSIMEQueryCandidateTexts(glossQuery) ?: [NSSet set]];
    NSMutableArray *candidates = [NSMutableArray array];
    for (NSDictionary *candidate in view[@"candidates"])
        if ([candidate[@"text"] isKindOfClass:NSString.class] && [candidate[@"source"] isKindOfClass:NSNumber.class] &&
            (!requested || [requested containsObject:candidate[@"text"]]))
            [candidates addObject:@{@"text":candidate[@"text"], @"source":candidate[@"source"]}];
    return candidates.count ? @{@"target_languages":targets, @"candidates":[candidates copy],
        @"directory":_preferencesDirectory ?: @""} : nil;
}
- (NSDictionary *)readCandidateGloss:(NSDictionary *)request resources:(NSString *)resources {
    NSMutableDictionary *query = [@{@"generation":request[@"generation"],
        @"candidates":request[@"candidates"]} mutableCopy];
    // 旧安装可能把 EngineResources 留在 Application Support，而随包补充词库在输入法 bundle
    // 里。把 bundle 作为只读回退交给 Host API，不要求重新准备词库代次。
    NSString *supplementary = [NSBundle.mainBundle.resourcePath copy];
    if (supplementary.isAbsolutePath) query[@"supplementary_resources"] = supplementary;
    return [MSIMEClientSession candidateGlossRequest:query resources:resources error:nil];
}
+ (dispatch_queue_t)learnedTranslationQueue {
    static dispatch_queue_t queue;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ queue = dispatch_queue_create("msime.learned-translations", DISPATCH_QUEUE_SERIAL); });
    return queue;
}
- (NSArray *)learnedTranslationItems:(NSArray *)candidates results:(NSArray *)results {
    NSArray *plan = [MSIMEClientSession customTranslationPlan:@{@"target_language":@"en", @"candidates":candidates} error:nil];
    NSMutableArray *items = [NSMutableArray array];
    for (NSDictionary *item in plan) {
        NSMutableDictionary *entry = [@{@"text":item[@"text"], @"direction":[item[@"source_language"] isEqual:@"en"]
            ? @"english_to_chinese" : @"chinese_to_english"} mutableCopy];
        if (results) {
            for (NSDictionary *result in results)
                if ([result[@"text"] isEqual:item[@"text"]]) { entry[@"translation"] = result[@"translation"]; break; }
            if (!entry[@"translation"]) continue;
        }
        [items addObject:entry];
    }
    return items;
}
- (void)persistFetchedTranslations:(NSArray<NSDictionary *> *)results forQuery:(NSDictionary *)query {
    NSString *directory = query[@"directory"] ?: _preferencesDirectory;
    if (![MSIMETranslationTargets(query) containsObject:@"en"] || ![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) return;
    // Reuse the query's candidates so each keeps the Engine source the plan requires; a bare {text:} would be rejected.
    NSMutableArray *candidates = [NSMutableArray array];
    for (NSDictionary *candidate in query[@"candidates"])
        for (NSDictionary *result in results)
            if ([result[@"text"] isEqual:candidate[@"text"]] && [result[@"translation"] isKindOfClass:NSString.class] && [result[@"translation"] length]) {
                [candidates addObject:candidate];
                break;
            }
    if (!candidates.count) return;
    NSArray *items = [self learnedTranslationItems:candidates results:results];
    if (!items.count) return;
    NSDictionary *request = @{ @"directory":[directory copy], @"generation":query[@"generation"] ?: @0,
        @"target_language":@"en", @"action":@"remember", @"items":items};
    dispatch_async([MSIMEInputController learnedTranslationQueue], ^{
        [MSIMEClientSession learnedTranslationRequest:request error:nil];
    });
}
- (void)synchronizeCandidateGloss {
    NSDictionary *request = [self currentGlossRequest];
    if (!request) { [self cancelCandidateGloss]; return; }
    if ([_glossRequest isEqual:request]) return;
    [self cancelCandidateGloss];
    _glossRequest = request;
    NSString *resources = [_session.hostOptions[@"resources"] copy];
    BOOL hasResources = [resources isKindOfClass:NSString.class] && resources.isAbsolutePath;
    NSString *directory = request[@"directory"];
    if (!hasResources && !directory.isAbsolutePath) { _glossResults = @[]; return; }
    NSArray *learnedItems = [self learnedTranslationItems:request[@"candidates"] results:nil];
    // The request leaves the generation out so a page whose content did not change compares equal; the reader still needs the one it was read for, captured here from the same query the request was built from, so a synchronization pass asks the session once.
    NSNumber *generation = [self serviceSnapshotQuery][@"generation"] ?: @0;
    NSMutableDictionary *read = [request mutableCopy];
    read[@"generation"] = generation;
    if (!_glossQueue) { _glossQueue = [NSOperationQueue new]; _glossQueue.maxConcurrentOperationCount = 1; _glossQueue.qualityOfService = NSQualityOfServiceUtility; }
    const uint64_t epoch = _glossEpoch;
    MSIMEClientSession *session = _session;
    id client = _activeClient;
    __weak MSIMEInputController *weakSelf = self;
    [_glossQueue addOperationWithBlock:^{
        id reader = hasResources ? weakSelf : nil; // id: handed to MSIMEReleaseControllerOnMain
        NSDictionary *result = reader ? [reader readCandidateGloss:read resources:resources] : nil;
        MSIMEReleaseControllerOnMain(&reader);
        if (result && ![result[@"generation"] isEqual:generation]) return;
        NSMutableArray *translations = [result[@"translations"] mutableCopy] ?: [NSMutableArray array];
        if (directory.isAbsolutePath && learnedItems.count) {
            __block NSDictionary *learned;
            dispatch_sync([MSIMEInputController learnedTranslationQueue], ^{
                learned = [MSIMEClientSession learnedTranslationRequest:@{@"directory":directory,
                    @"generation":generation, @"target_language":@"en", @"action":@"lookup", @"items":learnedItems} error:nil];
            });
            for (NSDictionary *entry in learned[@"translations"]) {
                NSUInteger index = [translations indexOfObjectPassingTest:^BOOL(NSDictionary *existing, NSUInteger position, BOOL *stop) {
                    (void)position; (void)stop;
                    return [entry[@"text"] isEqual:existing[@"text"]];
                }];
                // Learned records override packaged glosses, matching Engine's
                // user glossary overlay and Windows INSERT OR REPLACE semantics.
                if (index == NSNotFound) [translations addObject:entry];
                else translations[index] = entry;
            }
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEInputController *current = weakSelf;
            if (!current || current->_glossEpoch != epoch || current->_session != session || current->_activeClient != client ||
                ![[current currentGlossRequest] isEqual:request] || (result && ![result[@"generation"] isEqual:generation])) return;
            current->_glossResults = [translations copy];
            [current applyCandidateTranslationResults];
        });
    }];
}
- (void)cancelTargetGloss {
    ++_targetGlossEpoch;
    [_targetGlossQueue cancelAllOperations];
    _targetGlossRequest = nil;
    _targetGlossResults = nil;
}
// The non-English targets with an installed offline dictionary. English keeps its own path above, which also overlays the user's learned glossary.
- (NSDictionary *)currentTargetGlossRequest {
    if (!_activeClient || !_session || _focusPending || _appearance.englishMode ||
        (_appearance && !_appearance.candidateTranslations && !_appearance.candidateEnglishGloss) ||
        (_glossEnabled && !_glossEnabled.boolValue && !_appearance.candidateEnglishGloss)) return nil;
    NSDictionary *query = [self serviceSnapshotQuery];
    NSArray *targets = MSIMETranslationTargets(query);
    if (_glossTargetLanguages && ![_glossTargetLanguages isEqual:targets]) return nil;
    // 查询列出的候选，加上 gloss_query 要这些词典释义的整页（见 currentGlossRequest）。
    NSMutableArray *installed = [([query[@"offline_gloss_languages"] isKindOfClass:NSArray.class] ? query[@"offline_gloss_languages"] : @[]) mutableCopy];
    NSMutableSet<NSString *> *requested = [MSIMEQueryCandidateTexts(query) mutableCopy];
    NSDictionary *glossQuery = MSIMEGlossQuery(query);
    NSArray *glossInstalled = [glossQuery[@"offline_gloss_languages"] isKindOfClass:NSArray.class] ? glossQuery[@"offline_gloss_languages"] : @[];
    if (glossInstalled.count) {
        [installed addObjectsFromArray:glossInstalled];
        if (requested) [requested unionSet:MSIMEQueryCandidateTexts(glossQuery) ?: [NSSet set]];
    }
    NSMutableArray<NSString *> *languages = [NSMutableArray array];
    for (NSString *target in targets)
        if (![target isEqual:@"en"] && [installed containsObject:target]) [languages addObject:target];
    if (!languages.count) return nil;
    NSDictionary *view = [self serviceSnapshotView];
    if (!MSIMEViewShowsGlosses(view)) return nil;
    if (![view[@"generation"] isEqual:query[@"generation"]]) return nil;
    NSMutableArray *candidates = [NSMutableArray array];
    for (NSDictionary *candidate in view[@"candidates"])
        if ([candidate[@"text"] isKindOfClass:NSString.class] && [candidate[@"source"] isKindOfClass:NSNumber.class] &&
            (!requested || [requested containsObject:candidate[@"text"]]))
            [candidates addObject:@{@"text":candidate[@"text"], @"source":candidate[@"source"]}];
    return candidates.count ? @{@"target_languages":targets, @"offline_languages":[languages copy],
        @"candidates":[candidates copy]} : nil;
}
- (NSDictionary *)readTargetGloss:(NSDictionary *)request language:(NSString *)language resources:(NSString *)resources {
    return [MSIMEClientSession candidateGlossRequest:@{@"generation":request[@"generation"],
        @"target_language":language, @"candidates":request[@"candidates"]} resources:resources error:nil];
}
- (void)synchronizeTargetGloss {
    NSDictionary *request = [self currentTargetGlossRequest];
    if (!request) { [self cancelTargetGloss]; return; }
    if ([_targetGlossRequest isEqual:request]) return;
    [self cancelTargetGloss];
    _targetGlossRequest = request;
    NSString *resources = [_session.hostOptions[@"resources"] copy];
    if (![resources isKindOfClass:NSString.class] || !resources.isAbsolutePath) { _targetGlossResults = @{}; return; }
    // A queue of its own: cancelCandidateGloss drains the English queue whenever the English request changes, which would otherwise drop this read and leave the request without results.
    // As for the English read, the generation is captured here rather than carried in the request, from the query the request was built from.
    NSNumber *generation = [self serviceSnapshotQuery][@"generation"] ?: @0;
    NSMutableDictionary *read = [request mutableCopy];
    read[@"generation"] = generation;
    if (!_targetGlossQueue) { _targetGlossQueue = [NSOperationQueue new]; _targetGlossQueue.maxConcurrentOperationCount = 1; _targetGlossQueue.qualityOfService = NSQualityOfServiceUtility; }
    const uint64_t epoch = _targetGlossEpoch;
    MSIMEClientSession *session = _session;
    id client = _activeClient;
    __weak MSIMEInputController *weakSelf = self;
    [_targetGlossQueue addOperationWithBlock:^{
        NSMutableDictionary<NSString *, NSMutableDictionary<NSString *, NSString *> *> *values = [NSMutableDictionary dictionary];
        id reader = weakSelf;
        for (NSString *language in request[@"offline_languages"]) {
            NSDictionary *result = [reader readTargetGloss:read language:language resources:resources];
            if (![result[@"generation"] isEqual:generation]) continue;
            for (NSDictionary *entry in result[@"translations"]) {
                NSString *text = entry[@"text"];
                NSString *translation = entry[@"translation"];
                if (![text isKindOfClass:NSString.class] || ![translation isKindOfClass:NSString.class] || !translation.length) continue;
                NSMutableDictionary *byTarget = values[text];
                if (!byTarget) { byTarget = [NSMutableDictionary dictionary]; values[text] = byTarget; }
                byTarget[language] = translation;
            }
        }
        MSIMEReleaseControllerOnMain(&reader);
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEInputController *current = weakSelf;
            if (!current || current->_targetGlossEpoch != epoch || current->_session != session || current->_activeClient != client ||
                ![[current currentTargetGlossRequest] isEqual:request]) return;
            current->_targetGlossResults = [values copy];
            [current applyCandidateTranslationResults];
        });
    }];
}

- (void)cancelCloudCandidates {
    ++_cloudEpoch;
    [_cloudTimer invalidate];
    _cloudTimer = nil;
    [_cloudRequest cancel];
    _cloudRequest = nil;
    _cloudQuery = nil;
}

- (MSIMECloudCandidateRequest *)cloudRequestForURL:(NSURL *)url completion:(void (^)(NSData *))completion {
    return [[MSIMECloudCandidateRequest alloc] initWithURL:url configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration completion:completion];
}

- (MSIMECloudCandidateRequest *)cloudRequestForDescriptor:(NSDictionary *)descriptor completion:(void (^)(NSData *))completion {
    return [[MSIMECloudCandidateRequest alloc] initWithCloudDescriptor:descriptor
        configuration:NSURLSessionConfiguration.ephemeralSessionConfiguration completion:completion];
}

/// Shurufa v2 Tier 1 非阻塞端侧神经解码状态判定（安全密码框、英文模式与缺失模型时严格关闭）。
/// 根据当前版本与宿主会话配置纯 Rust 端侧神经服务；不可用时零副作用回退纯 Tier 0。
- (void)configureNeuralForSession {
    if (!_session || ![_session respondsToSelector:@selector(configureNeuralServiceWithOptions:updateHandler:error:)]) return;
    if (!MSIMEEditionIsShurufa()) {
        [_session configureNeuralServiceWithOptions:@{@"enabled": @NO} updateHandler:nil error:nil];
        return;
    }
    NSString *bundlePath = NSBundle.mainBundle.bundlePath;
    NSString *helperPath = bundlePath.length
        ? [bundlePath stringByAppendingPathComponent:@"Contents/Helpers/ShurufaNeuralService"]
        : nil;
    NSMutableDictionary *config = [NSMutableDictionary dictionaryWithObject:@YES forKey:@"enabled"];
    if (helperPath.length) config[@"helper_path"] = helperPath;
    __weak MSIMEInputController *weakSelf = self;
    __weak MSIMEClientSession *weakSession = _session;
    [_session configureNeuralServiceWithOptions:config
                                  updateHandler:^(uint64_t generation) {
        (void)generation;
        MSIMEInputController *controller = weakSelf;
        MSIMEClientSession *session = weakSession;
        if (!controller || !session || controller->_session != session || !controller->_activeClient) return;
        if ([session respondsToSelector:@selector(pollNeuralSnapshotReady:moved:error:)]) {
            BOOL ready = NO;
            const ShurufaViewSnapshot *polled = [session pollNeuralSnapshotReady:&ready moved:nullptr error:nil];
            if (!polled || !ready) return;
            controller->_viewState = shurufa::mac::ViewState::fromSnapshot(*polled);
            [controller renderCandidates];
            [controller synchronizeCandidateServices];
            return;
        }
        NSDictionary *polled = [session pollNeuralWithError:nil];
        if (!polled) return;
        if ([polled[@"view"] isKindOfClass:NSDictionary.class]) {
            controller->_viewState = shurufa::mac::ViewState::fromDictionary(polled[@"view"]);
        }
        if (MSIMEStrictBoolean(polled[@"ready"])) {
            [controller renderCandidates];
            [controller synchronizeCandidateServices];
        }
    }
                                          error:nil];
}

- (void)synchronizeCloudCandidates {
    NSDictionary *query = _activeClient && _session && !_focusPending && (!_appearance.englishMode || _managedEnglishComposition) && ![self secureEventInputActive] &&
        (!_appearance || _appearance.cloudCandidatesEnabled) ? [_session onlineQueryWithError:nil] : nil;
    if (!MSIMEStrictBoolean(query[@"cloud_candidates"]) || !MSIMEStrictBoolean(query[@"cloud_eligible"])) {
        [self cancelCloudCandidates];
        return;
    }
    NSString *url = query ? [MSIMEClientSession cloudRequestURLForQuery:query error:nil] : nil;
    NSDictionary *descriptor = query[@"cloud_request"];
    if (!url && ![descriptor isKindOfClass:NSDictionary.class]) { [self cancelCloudCandidates]; return; }
    if ([_cloudQuery isEqual:query]) return;
    [self cancelCloudCandidates];
    _cloudQuery = [query copy];
    const uint64_t epoch = _cloudEpoch;
    MSIMEClientSession *session = _session;
    id client = _activeClient;
    __weak MSIMEInputController *weakSelf = self;
    _cloudTimer = [NSTimer timerWithTimeInterval:0.5 repeats:NO block:^(NSTimer *timer) {
        (void)timer;
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_cloudEpoch != epoch || controller->_session != session ||
            controller->_activeClient != client || controller->_focusPending || [controller secureEventInputActive] ||
            (controller->_appearance.englishMode && !controller->_managedEnglishComposition) ||
            (controller->_appearance && !controller->_appearance.cloudCandidatesEnabled) ||
            ![[session onlineQueryWithError:nil] isEqual:query]) return;
        controller->_cloudTimer = nil;
        void (^completion)(NSData *) = ^(NSData *body) {
            MSIMEInputController *current = weakSelf;
            if (!current || current->_cloudEpoch != epoch || current->_session != session ||
                current->_activeClient != client || current->_focusPending || [current secureEventInputActive] ||
                (current->_appearance.englishMode && !current->_managedEnglishComposition) ||
                (current->_appearance && !current->_appearance.cloudCandidatesEnabled) ||
                ![[session onlineQueryWithError:nil] isEqual:query]) return;
            current->_cloudRequest = nil;
            if (!body) {
                current->_cloudQuery = nil;
                return;
            }
            NSDictionary *result = [session applyCloudResponse:body query:query error:nil];
            const BOOL applied = MSIMEStrictBoolean(result[@"applied"]);
            if (applied) {
                // Remember the post-apply identity so rendering does not re-request this result.
                current->_cloudQuery = [[session onlineQueryWithError:nil] copy];
                [current apply:result];
            } else {
                // A valid response can still be rejected when the local candidate page has
                // disappeared (or the provider returned no usable candidate). Leave the
                // query eligible for a later render after local candidates are restored.
                current->_cloudQuery = nil;
            }
        };
        if ([descriptor isKindOfClass:NSDictionary.class]) {
            controller->_cloudRequest = [controller cloudRequestForDescriptor:descriptor completion:completion];
        } else {
            controller->_cloudRequest = [controller cloudRequestForURL:[NSURL URLWithString:url] completion:completion];
        }
        [controller->_cloudRequest start];
    }];
    [NSRunLoop.mainRunLoop addTimer:_cloudTimer forMode:NSRunLoopCommonModes];
}

- (void)ensureAppearance {
    if (_appearance) return;
    _wubiCodeHintEnabled = YES;
    _glossShowPos = YES;
    _glossSingleSense = NO;
    _fixedBadgeEnabled = YES;
    _fixedBadgeStyle = @"paperclip";
    _aiBadgeStyle = @"robot";
    _aiBadgeString = MSIMECandidateBadgeString(msime::mac::candidate_ai_badge(_aiBadgeStyle.UTF8String ?: ""));
    _fixedBadgeString = MSIMECandidateBadgeString(msime::mac::candidate_fixed_badge(_fixedBadgeEnabled, _fixedBadgeStyle.UTF8String ?: ""));
    _appearance = [MSIMEAppearancePreferences sharedPreferences];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(appearanceChanged:) name:MSIMEAppearanceDidChangeNotification object:_appearance];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(appearanceDidPersist:) name:MSIMEAppearanceDidPersistNotification object:_appearance];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(translationPreferencesSaved:) name:MSIMETranslationPreferencesDidSaveNotification object:_appearance];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(appearanceChanged:) name:MSIMEVoiceSettingsDidChangeNotification object:nil];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(voiceProviderSettingsChanged:) name:MSIMEVoiceProviderSettingsDidChangeNotification object:nil];
    [NSDistributedNotificationCenter.defaultCenter addObserver:self
        selector:@selector(typingStatisticsEnabledChanged:)
        name:MSIMETypingStatisticsEnabledChangedNotification object:nil
        suspensionBehavior:NSNotificationSuspensionBehaviorDeliverImmediately];
    _globalVoiceHotkeyMonitor = [NSEvent addGlobalMonitorForEventsMatchingMask:(NSEventMaskKeyDown | NSEventMaskKeyUp) handler:[self globalVoiceHotkeyHandler]];
    static dispatch_once_t defaultsObserverOnce;
    dispatch_once(&defaultsObserverOnce, ^{
        [[NSNotificationCenter defaultCenter] addObserverForName:NSUserDefaultsDidChangeNotification object:nil queue:NSOperationQueue.mainQueue
            usingBlock:^(NSNotification *note) { (void)note; MSIMERefreshVoiceShortcutTap(); }];
    });
}
- (void)typingStatisticsEnabledChanged:(NSNotification *)notification {
    NSNumber *enabled = [notification.userInfo[@"enabled"] isKindOfClass:NSNumber.class]
        ? notification.userInfo[@"enabled"] : nil;
    if (enabled) MSIMETypingStatisticsEnabled.store(enabled.boolValue, std::memory_order_relaxed);
    else MSIMEReloadTypingStatisticsEnabled(_preferencesDirectory);
    // Switching statistics off drops what was collected rather than writing it.
    if (!MSIMETypingStatisticsEnabled.load(std::memory_order_relaxed)) {
        _keyPressBatch.clear();
        [_keyPressFlushTimer invalidate];
        _keyPressFlushTimer = nil;
    }
}
- (void (^)(NSEvent *))globalVoiceHotkeyHandler {
    __weak MSIMEInputController *weakSelf = self;
    return ^(NSEvent *event) { [weakSelf observeGlobalVoiceShortcutEvent:event]; };
}
- (void)observeGlobalVoiceShortcutEvent:(NSEvent *)event {
    if (MSIMEEditionIsShurufa() && MSIMEVoiceRecordingOwner && MSIMEVoiceRecordingOwner != self) return;
    if ((!_activeClient && ![self ownsPersistentVoiceRecording]) || (event.type == NSEventTypeKeyDown && event.isARepeat) ||
        !MSIMEVoiceInputEnabled(NSUserDefaults.standardUserDefaults)) return;
    if ([self usesConfiguredVoiceShortcut]) {
        [self handleConfiguredVoiceShortcut:event];
        return;
    }
    if (event.type != NSEventTypeKeyDown || ![self matchesVoiceToggleHotkey:event]) return;
    // AppKit delivers event-monitor handlers on the main thread. Handle
    // this gesture now: another main-queue hop could toggle a new client
    // or recording after focus or voice state has changed.
    [self toggleVoiceInput:nil];
}
- (BOOL)matchesVoiceToggleHotkey:(NSEvent *)event {
    if (MSIMEToggleChordMatches(_appearance.voiceInputToggleChord, event)) return YES;
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    const BOOL fixedEnabled = [defaults objectForKey:@"MSIMEClientVoiceHotkeyCtrlF9"] == nil ||
        [defaults boolForKey:@"MSIMEClientVoiceHotkeyCtrlF9"];
    return fixedEnabled && event.keyCode == 101 &&
        (event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagShift |
            NSEventModifierFlagOption | NSEventModifierFlagCommand)) == NSEventModifierFlagControl;
}
- (BOOL)usesConfiguredVoiceShortcut {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    return MSIMEVoiceCustomShortcut(defaults) != nil || ![MSIMEVoiceHotkeyMode(defaults) isEqual:@"legacy"];
}
- (BOOL)handleConfiguredVoiceShortcut:(NSEvent *)event {
    std::optional<uint32_t> eventSlot;
    const NSInteger kind = event.type == NSEventTypeKeyDown ? 0 : event.type == NSEventTypeKeyUp ? 1 : event.type == NSEventTypeFlagsChanged ? 2 : -1;
    if (MSIMEEditionIsShurufa() && kind >= 0 && event.keyCode < 128 && event.timestamp > 0) {
        // 使用事件自带的顺序标识，不设时间窗口，也不记录字符。
        eventSlot = (uint32_t)(kind * 128 * 16 + event.keyCode * 16 + MSIMEVoiceModifierBits(event.modifierFlags));
        auto previous = MSIMEVoiceEventReceipts.find(*eventSlot);
        if (previous != MSIMEVoiceEventReceipts.end() && event.timestamp <= previous->second.timestamp)
            return previous->second.consumed;
    }
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    NSDictionary *custom = MSIMEVoiceCustomShortcut(defaults);
    NSString *mode = MSIMEVoiceHotkeyMode(defaults);
    const auto result = _configuredVoiceShortcut.observe(event, {
        custom, mode,
        [defaults objectForKey:@"MSIMEClientVoiceHotkeyRightAlt"] == nil || [defaults boolForKey:@"MSIMEClientVoiceHotkeyRightAlt"],
        [defaults boolForKey:@"MSIMEClientVoiceHotkeyCtrlCommand"] != NO,
        [defaults boolForKey:@"MSIMEClientVoiceHotkeyCtrlOption"] != NO,
        event.type == NSEventTypeKeyDown && [self matchesVoiceToggleHotkey:event] != NO
    }, _voiceService.active,
        (event.type == NSEventTypeFlagsChanged || event.type == NSEventTypeKeyDown)
            ? MSIMEPhysicalModifierFlagsState(kCGEventSourceStateHIDSystemState) : 0);
    if (eventSlot) MSIMEVoiceEventReceipts[*eventSlot] = {event.timestamp, result.consumed};
    if (_configuredVoiceShortcut.began()) _voiceHoldGeneration = _voiceGeneration;
    if (result.consumed || result.action != MSIMEVoiceHoldShortcut::Action::None) [self resetModifierTap];
    if (_configuredVoiceShortcut.locked() && _voiceService.active) [_voiceOverlay setRecordingLocked:YES];
    if (result.action == MSIMEVoiceHoldShortcut::Action::Toggle) {
        const BOOL hold = [mode isEqual:@"hold"] || ([mode isEqual:@"legacy"] && custom &&
            MSIMEVoiceModifierKey([custom[@"key_code"] unsignedShortValue]) && [custom[@"modifiers"] unsignedIntValue] == 0);
        _voiceHoldStarting = hold && !result.onRelease && !_voiceService.active;
        if (!result.onRelease || _voiceHoldGeneration == _voiceGeneration) [self toggleVoiceInput:nil];
        _voiceHoldStarting = NO;
        if (!result.onRelease) _voiceHoldGeneration = _voiceGeneration;
    } else if (result.action == MSIMEVoiceHoldShortcut::Action::Cancel) {
        _voicePermissionToken = nil;
        [self noteVoiceArchiveCancelled];
        [self cancelHTTPVoiceInput]; [self cancelDoubaoVoiceInput]; [self cancelLiveVoiceInput];
        if (_voiceService.active) [_voiceService cancelWithError:nil];
    }
    return result.consumed;
}
- (void)resetCandidateAnchor {
    _candidateAnchorCaret = NSZeroRect;
    _candidateAnchorValid = NO;
}
- (NSRect)candidateCaretForRendering:(NSRect)reported {
    const BOOL followCursor = _appearance == nil || _appearance.candidateFollowCursor;
    if (!_candidateFollowCursorModeKnown || _candidateFollowCursorMode != followCursor) {
        [self resetCandidateAnchor];
        _candidateFollowCursorMode = followCursor;
        _candidateFollowCursorModeKnown = YES;
    }
    if (followCursor) return reported;
    if (!_candidateAnchorValid && MSIMEValidCaret(reported)) {
        _candidateAnchorCaret = reported;
        _candidateAnchorValid = YES;
    }
    return _candidateAnchorValid ? _candidateAnchorCaret : reported;
}
- (void)voiceProviderSettingsChanged:(NSNotification *)notification { (void)notification; [self resetPreferenceLoadState]; _voicePermissionToken = nil; _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset(); [self cancelLiveVoiceInput]; [self cancelDoubaoVoiceInput]; [self cancelHTTPVoiceInput]; if (_voiceService.active) [_voiceService cancelWithError:nil]; [_voiceOverlay dismissFailure]; MSIMERefreshVoiceShortcutTap(); }
- (void)translationPreferencesSaved:(NSNotification *)notification {
    [self resetPreferenceLoadState];
    [self applySharedToolbarPreferences:notification.userInfo];
    [self syncCurrentViewFromSession];
    if (_activeClient) { [self renderCandidates]; [self reloadPreferences]; }
}
// 视图里正在跑的方案的标识，按引擎顺序编号：quanpin、shuangpin、wubi、japanese、korean、cantonese、zhuyin、vietnamese、tibetan、stroke。
- (NSString *)viewSchemeName {
    NSArray<NSString *> *schemes = MSIMEInputSchemeNames();
    const NSInteger index = MSIMEViewScheme(_viewState);
    return index >= 0 && index < (NSInteger)schemes.count ? schemes[index] : MSIMEEditionDefaultScheme();
}
// 候选窗首行等界面显示的方案名，双拼带上它的方案（如「小鹤双拼」）。
- (NSString *)schemeTitleForScheme:(NSString *)scheme {
    NSString *profile = _viewState.shuangpinProfile.length ? _viewState.shuangpinProfile : _appearance.shuangpinProfile;
    NSDictionary<NSString *, NSString *> *schemeTitles = @{
        @"quanpin": @"全拼",
        @"shuangpin": @(msime::mac::ShuangpinSchemaTitle(profile.UTF8String ?: "")),
        @"wubi": [_appearance.wubiProfile isEqual:@"wubi98"] ? @"五笔 98" : @"五笔 86",
        @"japanese": @"日语",
        @"korean": @"韩语",
        @"cantonese": @"粤拼",
        @"zhuyin": @"注音",
        @"vietnamese": @"越南语",
        @"tibetan": @"藏文",
        @"stroke": @"笔画",
    };
    return schemeTitles[scheme];
}
- (void)appearanceChanged:(NSNotification *)notification {
    (void)notification;
    _expandedViewportHeight = 0;
    _expandedGridGeometry.reset();
    [self resetPreferenceLoadState]; // 本机改动使更早之前的磁盘读取失效。
    if (!_appearance.cloudCandidatesEnabled) [self cancelCloudCandidates];
    if (_appearance) _glossEnabled = @(_appearance.candidateTranslations);
    if (_appearance && !_appearance.candidateTranslations) {
        [self cancelCustomTranslations];
        if (!_appearance.candidateEnglishGloss) { [self cancelCandidateGloss]; [self cancelTargetGloss]; }
    }
    if (_appearance && !_appearance.candidateTranslations && !_appearance.candidateEnglishGloss &&
        [self syncCurrentViewFromSession] && _viewState.generationValid) {
        NSDictionary *cleared = [_session applyTranslations:@[] generation:_viewState.generation error:nil];
        if (cleared[@"view"]) _viewState = shurufa::mac::ViewState::fromDictionary(cleared[@"view"]);
    }
    if (_appearance.englishMode && !_managedEnglishComposition && _activeClient &&
        (_viewState.hasEditableText() || !_viewState.candidates.empty())) {
        [self dispatchCommand:MSIME_FINISH_COMPOSITION];
    }
    [self syncPageSize];
    [self syncPunctuation];
    [self syncCharacterWidth];
    if (_activeClient) [self renderCandidates];
    if (_activeClient) [self syncSystemInputModeForClient:_activeClient];
    else if (_appearance && !_appearance.inputModeStatusItem) [self presentInputModeStatusItem:nil];
}
- (void)appearanceDidPersist:(NSNotification *)notification {
    (void)notification;
    if (_activeClient && _preferencesDirectory) [self reloadPreferences];
}
- (void)resetPreferenceLoadState {
    ++_preferenceLoadGeneration;
    _preferenceLoadInFlight = NO;
    _appliedPreferenceRevision = 0;
}
// 方案只能经由共享偏好文档到达 Engine：切换时在主线程经单一写入方 `MSIMEAppearancePreferences` 落盘 `config.json`，并把写入后的快照原样交给持有焦点的会话。
- (void)applyPreferencesToSessionNow {
    if (!_session || !_activeClient || !_preferencesDirectory || !_appearance) return;
    NSDictionary *saved = [_appearance persistPreferencesToDirectory:_preferencesDirectory error:nil];
    if (![saved isKindOfClass:NSDictionary.class]) { msime_macos_diagnostic_write("preferences_save_failed"); return; }
    [self resetPreferenceLoadState];
    _preferenceLoadInFlight = YES;
    [self completePreferenceLoad:saved error:nil generation:_preferenceLoadGeneration session:_session client:_activeClient];
}
- (void)persistAppearancePreferences {
    if (!_preferencesDirectory || !_appearance) return;
    [_appearance setTranslationPreferencesDirectory:_preferencesDirectory];
    [_appearance schedulePersistPreferences];
}
- (void)syncPageSize {
    if (!_session) return;
    [self ensureAppearance];
    NSUInteger size = _appearance.pageSize;
    if (_requestedPageSize == size) return;
    NSDictionary *result = [_session setCandidatePageSize:(uint8_t)size error:nil];
    if (result) {
        _requestedPageSize = size;
        _viewState = shurufa::mac::ViewState::fromDictionary(result[@"view"]);
        if (_panel.isVisible && !_viewState.provisional) {
            [self renderCandidates];
        }
    }
}

- (BOOL)dispatchCommand:(uint32_t)command {
    return [self dispatchCommand:command handled:nullptr];
}

- (BOOL)dispatchCommand:(uint32_t)command handled:(BOOL *)handledOut {
    if (!_session) {
        if (handledOut) *handledOut = NO;
        return NO;
    }
    if ([_session respondsToSelector:@selector(commandSnapshot:error:)]) {
        const ShurufaTransitionSnapshot *snapshot = [_session commandSnapshot:command error:nil];
        if (!snapshot) {
            if (handledOut) *handledOut = NO;
            return NO;
        }
        if (handledOut) *handledOut = snapshot->handled ? YES : NO;
        [self applyTransitionSnapshot:snapshot overrideCommit:nil];
        return YES;
    }
    NSDictionary *transition = [_session command:command error:nil];
    if (!transition) {
        if (handledOut) *handledOut = NO;
        return NO;
    }
    if (handledOut) *handledOut = MSIMEStrictBoolean(transition[@"handled"]);
    [self apply:transition];
    return YES;
}

- (BOOL)dispatchFocus:(BOOL)focused {
    if (!_session) return NO;
    if ([_session respondsToSelector:@selector(setFocusedSnapshot:error:)]) {
        const ShurufaTransitionSnapshot *snapshot = [_session setFocusedSnapshot:focused error:nil];
        if (!snapshot) return NO;
        [self applyTransitionSnapshot:snapshot overrideCommit:nil];
        return YES;
    }
    NSDictionary *transition = [_session setFocused:focused error:nil];
    if (!transition) return NO;
    [self apply:transition];
    return YES;
}

- (BOOL)dispatchSelectGeneration:(uint64_t)generation index:(NSUInteger)index {
    if (!_session) return NO;
    if ([_session respondsToSelector:@selector(selectSnapshotGeneration:index:error:)]) {
        const ShurufaTransitionSnapshot *snapshot = [_session selectSnapshotGeneration:generation index:index error:nil];
        if (!snapshot) return NO;
        [self applyTransitionSnapshot:snapshot overrideCommit:nil];
        return YES;
    }
    NSDictionary *selected = [_session selectGeneration:generation index:index error:nil];
    if (!selected) return NO;
    [self apply:selected];
    return YES;
}

- (BOOL)syncCurrentViewFromSession {
    if (!_session) return NO;
    if ([_session respondsToSelector:@selector(viewSnapshotWithError:)]) {
        if (const ShurufaViewSnapshot *snap = [_session viewSnapshotWithError:nil]) {
            _viewState = shurufa::mac::ViewState::fromSnapshot(*snap);
            return YES;
        }
    }
    if (NSDictionary *view = [_session viewWithError:nil]) {
        _viewState = shurufa::mac::ViewState::fromDictionary(view);
        return YES;
    }
    return NO;
}
- (void)syncPunctuation {
    if (!_session) return;
    NSDictionary *view = [_session setChinesePunctuationEnabled:_appearance.runtimeChinesePunctuation error:nil];
    if (view) [self apply:@{@"view":view}];
    view = [_session setPairedPunctuationEnabled:_appearance.pairedPunctuation && !MSIMEPairedPunctuationExcludedHost() error:nil];
    if (view) [self apply:@{@"view":view}];
    view = [_session setPunctuationLock:_appearance.punctuationLock error:nil];
    if (view) [self apply:@{@"view":view}];
}
// 把大写锁定状态交给会话：「大写锁定时使用英文标点」（`caps_lock_ascii_punctuation`）由共享层按它决定标点去向，宿主只负责报告。
- (void)syncCapsLock {
    if (MSIMEEditionIsShurufa()) return;
    if (_session) [_session setCapsLockEnabled:_capsLock error:nil];
}
- (void)adoptCapsLockState:(BOOL)capsLock {
    if (MSIMEEditionIsShurufa()) return;
    if (_capsLock == capsLock) return;
    _capsLock = capsLock;
    [self syncCapsLock];
}
- (void)cancelCapsLockTap {
    _capsLockTap.reset();
    [_capsLockTapTimer invalidate];
    _capsLockTapTimer = nil;
}
// 失焦、换会话等外部重置打断了待定的轻点：大写锁定按实际状态报告，不再切换。
- (void)abandonCapsLockTap {
    if (MSIMEEditionIsShurufa()) return;
    const bool pending = _capsLockTap.armed();
    [self cancelCapsLockTap];
    if (pending) [self adoptCapsLockState:([NSEvent modifierFlags] & NSEventModifierFlagCapsLock) != 0];
}
// 返回 YES 表示这次大写锁定打开可能是一次轻点：先不报告给会话和工具栏，等 pollCapsLockTap 认定。
- (BOOL)observeCapsLockTap:(NSEvent *)event capsLock:(BOOL)capsLock sender:(id)sender {
    if (MSIMEEditionIsShurufa()) return NO;
    const NSEventModifierFlags others = NSEventModifierFlagShift | NSEventModifierFlagControl |
        NSEventModifierFlagOption | NSEventModifierFlagCommand | NSEventModifierFlagFunction;
    if (_capsLockTap.armed() && event.type == NSEventTypeFlagsChanged && capsLock &&
        !(event.modifierFlags & others) && !MSIMECapsLockKeyHeld()) {
        // 实体松手的状态事件先到时，用原来的按下时刻完成判定，避免把它当作别的键或重新布防。
        [self pollCapsLockTap];
        return YES;
    }
    if (event.type == NSEventTypeFlagsChanged && event.keyCode == 0x39) {
        [self cancelCapsLockTap];
        // 只有大写锁定从关变开才可能是轻点；开着时按它是关掉大写锁定，照常处理，不切换。
        if (!capsLock || _capsLock || !sender || !_appearance.capsLockTapShortcut) return NO;
        // 设备没有提供实体按住位时保留系统大写锁定，不能把缺失状态当作已经松手；同时开始（或重试）HID 监听，拿到结果之后的轻点才能判定。
        if (!(event.modifierFlags & NX_ALPHASHIFT_STATELESS_MASK) && !MSIMECapsLockKeyHeld()) {
            MSIMECapsLockHIDStart();
            return NO;
        }
        _capsLockTap.press(MSIMECapsLockNow(), true, false, (event.modifierFlags & others) != 0);
        if (!_capsLockTap.armed()) return NO;
        __weak MSIMEInputController *weakSelf = self;
        _capsLockTapTimer = [NSTimer timerWithTimeInterval:0.01 repeats:YES block:^(NSTimer *timer) {
            (void)timer;
            [weakSelf pollCapsLockTap];
        }];
        // 事件跟踪与模态面板期间也要轮询，否则轻点会等默认运行模式恢复后才被当作长按。
        [NSRunLoop.mainRunLoop addTimer:_capsLockTapTimer forMode:NSRunLoopCommonModes];
        return YES;
    }
    // 按着 Caps Lock 又按了别的键，是在用大写锁定打字：不再算轻点，大写锁定由调用方照常报告。
    if (_capsLockTap.armed() && (event.type == NSEventTypeKeyDown || event.type == NSEventTypeFlagsChanged))
        [self cancelCapsLockTap];
    return NO;
}
// 定时器调用，测试也直接调用它来推进合成的时钟和按键状态。
- (void)pollCapsLockTap {
    if (MSIMEEditionIsShurufa()) return;
    if (!_capsLockTap.armed()) {
        [self cancelCapsLockTap];
        return;
    }
    const std::int64_t now = MSIMECapsLockNow();
    if (MSIMECapsLockKeyHeld()) {
        if (!_capsLockTap.heldTooLong(now)) return;
        // 按住超过分界：用户要的是大写锁定，补报给会话和工具栏。
        [self cancelCapsLockTap];
        [self adoptCapsLockState:YES];
        return;
    }
    const bool tap = _capsLockTap.release(now);
    [self cancelCapsLockTap];
    // 撤不回大写锁定就不切换：切到英文却留着大写锁定，比什么都不做更糟。
    if (!tap || !_activeClient || !MSIMECapsLockRelease()) {
        [self adoptCapsLockState:YES];
        return;
    }
    _capsLockTapReleased = YES;
    [self adoptCapsLockState:NO];
    [self toggleInputModeFromModifierTap];
}
// 单按 Shift、Control 或 Caps Lock 切换中英文。
//
// A tap during a composition sends out the letters that were typed, not the highlighted candidate. Reaching for Shift mid-word is how a word the dictionary does not carry - a name, a command, an acronym - gets out without losing what was already typed; finishing the composition instead commits the Chinese candidate, which is the opposite of what was asked for.
//
// Commit first, then switch: switching rebuilds the input session, and the other order loses the letters still being composed. setEnglishInputMode: finishes any composition of its own, which is a no-op once this has run.
- (void)toggleInputModeFromModifierTap {
    if (MSIMEEditionIsShurufa()) { return; }
    if ([self applyTrilingualCycleIfEnabled]) return;
    if (_viewState.hasEditableText() && _session && _activeClient) {
        // 单按修饰键是一次物理按下，却不经过 handleEvent: 的按下报告（它只看 keyDown）。派发前按指针开一轮：不出按键音，与以前一样；原样上屏的提交音照常由核心出。
        [self reportPointerToLifecycle];
        [self dispatchCommand:MSIME_COMMIT_RAW];
    }
    [self setEnglishInputMode:!_appearance.englishMode];
}

// 组合键在循环关闭时保留原来的取消组字规则；轻点修饰键单独保留原文提交规则。
- (void)toggleChineseEnglishMode {
    if (MSIMEEditionIsShurufa()) { return; }
    if (![self applyTrilingualCycleIfEnabled])
        [self setEnglishInputMode:!_appearance.englishMode];
}

// 中日英循环切换（共享偏好 `keybindings.trilingual_cycle`，默认关）。
//
// 顺序和「有组字时先上屏原始字母」都在共享层：`msime_client_language_cycle` →
// `crates/client-core/src/language_cycle.rs`，桌面宿主将来共用同一份。这里只做两件事：
// 把控制器此刻的状态交给它，按它的答复执行。返回 NO 表示这一键不循环（开关关着），调用方
// 照旧走二态切换，行为与加这个开关之前完全一样。
//
// 上的屏是原始拼音串，不是高亮的候选：旧版在改名、缩写、命令这类打不出来的词上按住切换键，
// 要的是已经打出来的字母，而不是替用户选一个他没要的字。
- (BOOL)applyTrilingualCycleIfEnabled {
    if (!_appearance.trilingualCycle) return NO;
    if (!_session || !_activeClient) return NO;
    NSDictionary *hostOptions = [self inputSchemeHostOptions];
    NSArray<NSString *> *names = MSIMEInputSchemeNames();
    NSString *chineseFallback = nil;
    for (NSString *name in names) {
        if (msime::mac::cycle::LastChineseSchemeCode(name.UTF8String) >= 0 && MSIMEInputSchemeAvailable(name, hostOptions)) {
            chineseFallback = name;
            break;
        }
    }
    // 从其他版本同步来的开关也要受本版本能力约束。
    if (!chineseFallback || !MSIMEInputSchemeAvailable(@"japanese", hostOptions)) return NO;
    NSString *scheme = MSIMEEffectiveInputScheme(_appearance.inputScheme, _appearance.lastChineseScheme, hostOptions);
    const NSUInteger schemeIndex = scheme ? [names indexOfObject:scheme] : NSNotFound;
    if (schemeIndex == NSNotFound || schemeIndex > 9) return NO;
    const BOOL english = _appearance.englishMode;
    const int mode = msime::mac::cycle::ModeCode(english, schemeIndex == 3);
    NSDictionary *reply = [_session languageCycleEnabled:YES
                                                   mode:(uint8_t)mode
                                                 scheme:(uint8_t)schemeIndex
                                      lastChineseScheme:msime::mac::cycle::LastChineseSchemeCode(_appearance.lastChineseScheme.UTF8String)
                                         hasComposition:_viewState.hasEditableText() ||
                                                        !_viewState.candidates.empty()];
    const auto step = msime::mac::cycle::ParseReply(reply);
    if (!step.claimed) return NO;
    // 先把正在组的东西原样上屏，再换模式：换方案会重建会话，留下的组字会跟着一起丢。
    if (step.commitRaw && _viewState.hasEditableText()) {
        [self reportPointerToLifecycle];
        // 命令失败时消费切换键并保留现有组字，不能重建会话后丢掉它。
        if (![self dispatchCommand:MSIME_COMMIT_RAW]) return YES;
    }
    if (step.mode == static_cast<int>(msime::mac::cycle::Mode::English)) {
        [self setEnglishInputMode:YES];
        return YES;
    }
    // 从英文回中文时先退出英文模式：`setEnglishInputMode:NO` 会把标点、工具栏和菜单栏都摆回中文，
    // 之后换方案的那一次写入才不会被仍在英文的状态盖掉。
    if (_appearance.englishMode) [self setEnglishInputMode:NO showHUD:NO];
    // 换方案：走和输入菜单同一条路（`selectInputScheme:` 的写法），记下离开时的中文方案。
    if (step.scheme >= 0 && step.scheme < (int)names.count) {
        NSString *target = names[(NSUInteger)step.scheme];
        if (step.mode == static_cast<int>(msime::mac::cycle::Mode::Chinese) &&
            !MSIMEInputSchemeAvailable(target, hostOptions)) target = chineseFallback;
        if (![target isEqualToString:_appearance.inputScheme] && MSIMEInputSchemeAvailable(target, hostOptions)) {
            _appearance.inputScheme = target;
            [self applyPreferencesToSessionNow];
        }
    }
    if (english != _appearance.englishMode || ![scheme isEqual:_appearance.inputScheme])
        [self showCurrentInputModeHUD];
    return YES;
}
- (void)syncCharacterWidth {
    if (!_session) return;
    NSDictionary *view = [_session setCharacterWidthFull:_appearance.runtimeFullWidthInput error:nil];
    if (view) [self apply:@{@"view":view}];
}
// 菜单和工具栏一样取全局主题的明暗（THEME_CONTRACT §6）：固定明暗的主题盖过颜色模式，`system` 和以它为底的自定义主题仍由颜色模式决定。NSMenu 没有可以重新着色的调色板，主题能影响的只有明暗。macOS 的菜单没有单独的明暗设置，所以钉住的是交给 ApplyMetasequoiaMenuTheme 的 `theme`。
- (NSDictionary *)resolvedMenuThemePreferences {
    NSDictionary *preferences = _menuThemePreferences ?: @{};
    if (!_appearance) return preferences;
    const auto fixed = [_appearance fixedThemeMode];
    if (!fixed) return preferences;
    NSMutableDictionary *pinned = [preferences mutableCopy];
    pinned[@"theme"] = *fixed ? @"dark" : @"light";
    return pinned;
}
// 「输入方案」子菜单：输入法菜单和悬浮工具栏的切换输入方案按钮用的是同一份。`currentTitle` 带回正在用的方案的名字，没有打勾的项时为 nil。
- (NSMenu *)inputSchemeMenuWithCurrentTitle:(NSString **)currentTitle {
    NSString *profile = [NSString stringWithUTF8String:msime::mac::ShuangpinSchemaTitle(_appearance.shuangpinProfile.UTF8String ?: "")];
    if ([profile hasSuffix:@"双拼"] && profile.length > 2) profile = [profile substringToIndex:profile.length - 2];
    NSArray<NSString *> *schemes = MSIMEInputSchemeNames();
    NSArray<NSString *> *schemeTitles = @[@"全拼", [NSString stringWithFormat:@"双拼（%@）", profile], [_appearance.wubiProfile isEqual:@"wubi98"] ? @"五笔 98" : @"五笔 86", @"日语", @"韩语", @"粤拼", @"注音", @"越南语", @"藏文", @"笔画"];
    NSMenu *schemeMenu = [[NSMenu alloc] initWithTitle:@"输入方案"];
    schemeMenu.autoenablesItems = NO;
    // A scheme that cannot run here (Cantonese, Zhuyin or Stroke without its dictionary) is not offered, and the check is on the scheme actually running, so a preference naming one shows the scheme it fell back to.
    NSDictionary *hostOptions = [self inputSchemeHostOptions];
    NSString *effectiveScheme = MSIMEEffectiveInputScheme(_appearance.inputScheme, _appearance.lastChineseScheme, hostOptions);
    for (NSUInteger index = 0; index < schemes.count; ++index) {
        if (!MSIMEInputSchemeAvailable(schemes[index], hostOptions)) continue;
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:schemeTitles[index] action:@selector(selectInputScheme:) keyEquivalent:@""];
        item.target = self;
        item.representedObject = schemes[index];
        item.state = [effectiveScheme isEqual:schemes[index]] ? NSControlStateValueOn : NSControlStateValueOff;
        if (item.state == NSControlStateValueOn && currentTitle) *currentTitle = schemeTitles[index];
        [schemeMenu addItem:item];
    }
    return schemeMenu;
}
- (NSMenu *)menu {
    [self ensureAppearance];
    NSMenu *menu = [[NSMenu alloc] initWithTitle:MSIMEEditionDisplayName()];
    menu.autoenablesItems = NO;
    ApplyMetasequoiaMenuTheme(menu, [self resolvedMenuThemePreferences]);
    if (_sessionUnavailableReason && !_session) {
        NSMenuItem *retry = [[NSMenuItem alloc] initWithTitle:@"重试中文输入" action:@selector(retryUnavailableSession:) keyEquivalent:@""];
        retry.target = self;
        [menu addItem:retry];
        NSMenuItem *restart = [[NSMenuItem alloc] initWithTitle:@"重启输入法…" action:@selector(restartUnavailableInputMethod:) keyEquivalent:@""];
        restart.target = self;
        [menu addItem:restart];
        [menu addItem:NSMenuItem.separatorItem];
    }
    for (NSUInteger mode = 0; mode < 2; ++mode) {
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:mode ? @"英文输入" : @"中文输入" action:mode ? @selector(selectEnglishMode:) : @selector(selectChineseMode:) keyEquivalent:@""];
        item.target = self;
        item.state = (_appearance.englishMode == (mode == 1) && (mode == 1 || !_viewState.dedicatedEnglish)) ? NSControlStateValueOn : NSControlStateValueOff;
        [menu addItem:item];
    }
    NSMenuItem *englishCandidates = [[NSMenuItem alloc] initWithTitle:@"英文候选模式" action:@selector(toggleDedicatedEnglishMode:) keyEquivalent:@"e"];
    englishCandidates.target = self;
    englishCandidates.keyEquivalentModifierMask = NSEventModifierFlagControl | NSEventModifierFlagShift;
    englishCandidates.state = !_appearance.englishMode && _viewState.dedicatedEnglish ? NSControlStateValueOn : NSControlStateValueOff;
    [menu addItem:englishCandidates];
    [menu addItem:NSMenuItem.separatorItem];
    // Simplified output is the off state of this one toggle rather than a second row: a radio pair spent a row on the default.
    NSMenuItem *traditional = [[NSMenuItem alloc] initWithTitle:@"繁体输出" action:@selector(toggleTraditionalOutput:) keyEquivalent:@""];
    traditional.target = self;
    traditional.state = _appearance.traditionalOutput ? NSControlStateValueOn : NSControlStateValueOff;
    [menu addItem:traditional];
    // The three typing toggles the floating toolbar also carries, so they stay reachable with the toolbar hidden. The key equivalents are only labels for the chords handleEvent already claims (Ctrl+Shift+Space and Ctrl+.), not a second binding.
    NSMenuItem *fullWidth = [[NSMenuItem alloc] initWithTitle:@"全角字符" action:@selector(toggleFullWidthInput:) keyEquivalent:@" "];
    fullWidth.target = self;
    fullWidth.keyEquivalentModifierMask = NSEventModifierFlagControl | NSEventModifierFlagShift;
    fullWidth.state = _appearance.runtimeFullWidthInput ? NSControlStateValueOn : NSControlStateValueOff;
    [menu addItem:fullWidth];
    NSMenuItem *punctuation = [[NSMenuItem alloc] initWithTitle:@"中文标点" action:@selector(toggleChinesePunctuation:) keyEquivalent:@"."];
    punctuation.target = self;
    punctuation.keyEquivalentModifierMask = NSEventModifierFlagControl;
    punctuation.state = _appearance.runtimeChinesePunctuation ? NSControlStateValueOn : NSControlStateValueOff;
    // A punctuation lock pins the runtime state, so the toggle would do nothing; say so instead of offering it.
    punctuation.enabled = !([_appearance.punctuationLock isEqual:@"chinese"] || [_appearance.punctuationLock isEqual:@"english"]);
    [menu addItem:punctuation];
    NSMenuItem *translations = [[NSMenuItem alloc] initWithTitle:@"显示译文" action:@selector(toggleCandidateTranslations:) keyEquivalent:@""];
    translations.target = self;
    translations.state = _appearance.candidateTranslations ? NSControlStateValueOn : NSControlStateValueOff;
    [menu addItem:translations];
    [menu addItem:NSMenuItem.separatorItem];
    // The scheme and the theme are each one choice out of several, so each is a submenu whose row names the current one, the way the system lists an input source's modes. As a radio list under a header the scheme alone took six rows.
    NSString *currentSchemeTitle = nil;
    NSMenu *schemeMenu = [self inputSchemeMenuWithCurrentTitle:&currentSchemeTitle];
    NSMenuItem *scheme = [[NSMenuItem alloc] initWithTitle:currentSchemeTitle ? [NSString stringWithFormat:@"输入方案（%@）", currentSchemeTitle] : @"输入方案" action:nil keyEquivalent:@""];
    scheme.submenu = schemeMenu;
    [menu addItem:scheme];
    NSString *currentTheme = _appearance.globalTheme ?: @"system";
    NSMenu *themes = [[NSMenu alloc] initWithTitle:@"主题"];
    themes.autoenablesItems = NO;
    NSString *currentThemeTitle = nil;
    for (const auto &entry : msime::mac::ThemeCatalog()) {
        NSString *identifier = [NSString stringWithUTF8String:entry.id.c_str()];
        NSString *title = [NSString stringWithUTF8String:entry.title.c_str()];
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:@selector(selectGlobalTheme:) keyEquivalent:@""];
        item.target = self;
        item.representedObject = identifier;
        item.state = [currentTheme isEqual:identifier] ? NSControlStateValueOn : NSControlStateValueOff;
        if (item.state == NSControlStateValueOn) currentThemeTitle = title;
        [themes addItem:item];
    }
    NSMenuItem *theme = [[NSMenuItem alloc] initWithTitle:currentThemeTitle ? [NSString stringWithFormat:@"主题（%@）", currentThemeTitle] : @"主题" action:nil keyEquivalent:@""];
    theme.submenu = themes;
    [menu addItem:theme];
    [menu addItem:NSMenuItem.separatorItem];

    // 保留直接从输入法菜单触发的常用输入工具入口。
    NSMenuItem *emoji = [[NSMenuItem alloc] initWithTitle:@"水杉表情面板…" action:@selector(showEmoji:) keyEquivalent:@""];
    emoji.target = self;
    [menu addItem:emoji];
    // 手写模型只认汉字，不提供手写的版本（日文、越南文和藏文版）菜单里没有这一项。
    if (MSIMEEditionOffersHandwriting()) {
        NSMenuItem *handwriting = [[NSMenuItem alloc] initWithTitle:@"手写输入…" action:@selector(showHandwriting:) keyEquivalent:@""];
        handwriting.target = self;
        [menu addItem:handwriting];
    }
    NSMenuItem *voice = [[NSMenuItem alloc] initWithTitle:@"开始/结束语音输入" action:@selector(showVoicePanel) keyEquivalent:@""];
    voice.target = self;
    [menu addItem:voice];

    [menu addItem:NSMenuItem.separatorItem];
    NSMenuItem *settings = [[NSMenuItem alloc] initWithTitle:[MSIMEEditionDisplayName() stringByAppendingString:@"设置…"] action:@selector(showAppearance:) keyEquivalent:@""];
    settings.target = self;
    [menu addItem:settings];
    // The reference tray menu ends with 关于, which opens the settings window on its about page. One row does not make the menu too tall, and without it the version and licence notices are only reachable by knowing to open settings and scroll to the last page.
    NSMenuItem *about = [[NSMenuItem alloc] initWithTitle:[NSString stringWithFormat:@"关于%@…", MSIMEEditionDisplayName()] action:@selector(showAbout:) keyEquivalent:@""];
    about.target = self;
    [menu addItem:about];
    if (MSIMEEditionIsShurufa()) {
        // 复用现有菜单项和动作，只保留本版的输入入口。
        NSSet<NSString *> *actions = [NSSet setWithArray:@[
            @"retryUnavailableSession:", @"restartUnavailableInputMethod:",
            @"toggleTraditionalOutput:", @"toggleFullWidthInput:", @"toggleChinesePunctuation:",
            @"showVoicePanel", @"showVoiceSettings:", @"showAppearance:", @"showAbout:"]];
        for (NSMenuItem *item in menu.itemArray.copy) {
            if (item.separatorItem) continue;
            if (item == scheme || [actions containsObject:NSStringFromSelector(item.action) ?: @""]) continue;
            [menu removeItem:item];
        }
        BOOL separator = YES;
        for (NSMenuItem *item in menu.itemArray.copy) {
            if (item.separatorItem && separator) [menu removeItem:item];
            separator = item.separatorItem;
        }
        for (NSMenuItem *item in menu.itemArray) item.keyEquivalent = @"";
        settings.title = @"设置…";
    }
    return menu;
}
- (void)showAccount:(id)sender {
    (void)sender;
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"账号窗口暂不可用";
    alert.informativeText = @"请重新启动输入法；若仍无法打开，请检查安装是否完整。";
    [alert runModal];
}
- (void)showHandwriting:(id)sender {
    (void)sender;
    if (!MSIMEEditionOffersHandwriting()) return;
    [self showAccount:nil];
}
- (void)showEmoji:(id)sender {
    if (MSIMEEditionIsShurufa()) return;
    (void)sender;
    [self showSystemCharacterPalette];
}
- (void)showVoicePanel {
    [self toggleVoiceInput:nil];
}
// 普通英文在未选词就离开输入框或撤销授权时保留原始字母，不自动采用补全。
- (BOOL)commitManagedEnglishRaw {
    if (!_managedEnglishComposition || !_viewState.hasEditableText()) return YES;
    return [self dispatchCommand:MSIME_COMMIT_RAW];
}

// 撤回普通英文的托管组字时先上屏原始字母，不能选中补全词或隐藏 marked text。
- (BOOL)setManagedEnglishEnabled:(BOOL)enabled {
    if (MSIMEEditionIsShurufa()) { _managedEnglishComposition = NO; return YES; }
    if (!_session) { _managedEnglishComposition = NO; return YES; }
    if (_managedEnglishComposition == enabled) {
        if (!enabled) return YES;
        BOOL dedicatedEnglish = NO;
        if ([_session respondsToSelector:@selector(viewSnapshotWithError:)]) {
            const ShurufaViewSnapshot *snap = [_session viewSnapshotWithError:nil];
            dedicatedEnglish = snap && snap->dedicated_english;
        } else {
            dedicatedEnglish = MSIMEStrictBoolean([_session viewWithError:nil][@"dedicated_english"]);
        }
        if (dedicatedEnglish) return YES;
    }
    if (!enabled && ![self commitManagedEnglishRaw]) return NO;
    NSDictionary *view = [_session setDedicatedEnglishEnabled:enabled error:nil];
    if (!view) return NO;
    _managedEnglishComposition = enabled;
    [self apply:@{@"view": view}];
    return YES;
}

- (BOOL)synchronizeManagedEnglishMode {
    return [self setManagedEnglishEnabled:_appearance.englishMode && _englishOnlineCandidatesEnabled];
}

- (void)setEnglishInputMode:(BOOL)enabled {
    [self setEnglishInputMode:enabled showHUD:YES];
}
- (void)setEnglishInputMode:(BOOL)enabled showHUD:(BOOL)showHUD {
    if (MSIMEEditionIsShurufa()) return;
    [self ensureAppearance];
    if (enabled) [MSIMEInputSessionRecoveryPanel dismissForDelegate:self];
    const BOOL changed = _appearance.englishMode != enabled;
    if (changed && _managedEnglishComposition && ![self setManagedEnglishEnabled:NO]) return;
    // Read before the cancel below, whose reply need not repeat the flag.
    const BOOL leaveEnglishCandidates = enabled && _viewState.dedicatedEnglish;
    if (enabled && !_appearance.englishMode && _session && _activeClient) {
        // Switching into English drops what was being composed rather than committing it. The
        // reference has two different rules here and this host had copied the wrong one onto both
        // entry points: its Shift toggle is FUNCTION_TOGGLE_IME_MODE, whose handler commits the raw
        // keystroke buffer, while its English-mode switch (Ctrl+Shift+E) is FUNCTION_CANCEL, whose
        // handler terminates the composition and sends nothing. Committing instead put a Chinese
        // candidate nobody chose into the document - the user reached for English precisely because
        // the candidates on screen were not what they wanted.
        //
        // The Shift tap keeps its own rule: it commits the raw letters before calling this, which
        // leaves nothing here to cancel.
        //
        // A scheme that commits on blur (`commits_on_blur`: a Korean syllable, a Zhuyin or Vietnamese composition) is the exception: its composition is text the user already wrote, not candidates they are rejecting, so it is committed, as the scheme switches and the other hosts do.
        const BOOL writtenText = MSIMESchemeTrait(_viewState, msime::mac::scheme::CommitsOnBlur) && _viewState.hasEditableText();
        if (![self dispatchCommand:writtenText ? MSIME_FINISH_COMPOSITION : MSIME_CANCEL]) return; // Do not hide an unsettled composition after an Engine failure.
    }
    // Every switch to English also leaves the English candidate mode, as the reference's status task calls SetEnglishInputMode(false) and ClearState() whenever the mode turns English (event_listener.cpp), so switching back always gives pinyin. An Engine failure leaves the mode as it was.
    if (leaveEnglishCandidates && _session) {
        NSError *error = nil;
        NSDictionary *view = [_session setDedicatedEnglishEnabled:NO error:&error];
        if (!view) { if (error) NSBeep(); return; }
        [self apply:@{@"view":view}];
    }
    // A Chinese/English switch puts punctuation back in step with the mode, as the reference's SyncPunctuationWithImeMode does: English mode gets English punctuation unless punctuation_lock pins Chinese. Returning to Chinese goes back to the saved starting value, the macOS adaptation for the shared chinese_punctuation setting. Set before the mode is saved so the resulting sync and toolbar refresh already see it.
    if (changed) {
        _englishPunctuation = {};
        if (enabled) _appearance.runtimeChinesePunctuation = [_appearance.punctuationLock isEqual:@"chinese"];
        else [_appearance resetRuntimePunctuationForActiveApplication];
    }
    _appearance.englishMode = enabled;
    if (enabled && _englishOnlineCandidatesEnabled && !_session && _activeClient) [self prepareSession];
    [self synchronizeManagedEnglishMode];
    [self resetCandidateAnchor];
    [self hideCandidatePanel:"mode_switch"];
    [_keymapPanel orderOut:nil];
    [self syncSystemInputModeForClient:_activeClient ?: self.client];
    if (changed && showHUD) [self showCurrentInputModeHUD];
}

- (void)showCurrentInputModeHUD {
    if (_appearance.englishMode) {
        [self showInputStateHUDText:@"英" announcement:@"英文输入"];
    } else if (_viewState.dedicatedEnglish) {
        [self showInputStateHUDText:@"英文候选" announcement:@"英文候选模式"];
    } else {
        NSString *scheme = MSIMEEffectiveInputScheme(_appearance.inputScheme, _appearance.lastChineseScheme, [self inputSchemeHostOptions]);
        NSArray<NSString *> *mode = @{@"japanese": @[@"日", @"日语输入"], @"korean": @[@"韩", @"韩语输入"],
            @"vietnamese": @[@"越", @"越南语输入"], @"tibetan": @[@"藏", @"藏语输入"]}[scheme];
        if (mode) [self showInputStateHUDText:mode[0] announcement:mode[1]];
        else [self showInputStateHUDText:@"中" announcement:@"中文输入"];
    }
}
- (void)showInputSchemeHUD {
    NSString *title = nil;
    [self inputSchemeMenuWithCurrentTitle:&title];
    if (title) [self showInputStateHUDText:title announcement:[NSString stringWithFormat:@"输入方案：%@", title]];
}

- (void)showInputStateHUDText:(NSString *)text announcement:(NSString *)announcement {
    if (_appearance.inputModeHUD && _activeClient) {
        NSRect caret = NSZeroRect;
        [(id<IMKTextInput>)_activeClient attributesForCharacterIndex:0 lineHeightRectangle:&caret];
        // 切换提示与悬浮工具栏共用明暗调色板，跟随用户选择的主题和皮肤。
        const auto light = [_appearance toolbarSkinForDark:NO];
        const auto dark = [_appearance toolbarSkinForDark:YES];
        MSIMEInputModeHUDPanel *hud = [MSIMEInputModeHUDPanel sharedPanel];
        [hud setSurfaceColor:MSIMEThemedSkinColor(@"MSIMEInputModeHUDSurface", light.surface, dark.surface)
                 borderColor:MSIMEThemedSkinColor(@"MSIMEInputModeHUDBorder", light.border, dark.border)
                   textColor:MSIMEThemedSkinColor(@"MSIMEInputModeHUDText", light.text, dark.text)];
        [hud applySizingPreferences:@{@"show_app_logo": @(_appearance.showsAppLogo)}];
        [hud showText:text announcement:announcement nearCaretRect:caret];
    }
}
// The HostOptions document the scheme checks read: the session's, or the one on disk before a session exists. Test sessions stand in without hostOptions, as MSIMEStatisticsHostOptions allows for.
- (NSDictionary *)inputSchemeHostOptions {
    return [_session respondsToSelector:@selector(hostOptions)] ? _session.hostOptions : MSIMELoadRuntimeOptions();
}
// 共享偏好文档是方案的权威来源：设置页、`msime config set`、云端同步和本输入法自己都写它，NSUserDefaults 里的 `MSIMEClientInputScheme` 只有本输入法自己写。外观设置在载入文档之前退回那份本地值，而文档平时在激活之后才于后台载入，所以输入法进程重新启动后（重新登录、重启、更新），控制器会先按本输入法上次写下的旧方案把菜单栏选到它的模式、把它记为已同步，期间的任何一次保存还会把它写回文档。用户在设置页选了全拼，下次再用又回到双拼，就是这样来的（#4288）。因此在本进程第一次按外观设置行事之前——客户端获得焦点、或者系统先报告了模式——同步载入一次文档；之后的改动照旧由每秒一次的轮询带来。读不到文档时什么也不做，交给那次后台载入。
- (void)applySharedInputPreferencesBeforeFirstUse {
    if (!_appearance || (_appearance.sharedInputPreferencesApplied && _englishOnlinePreferenceLoaded)) return;
    NSString *directory = _preferencesDirectory;
    if (!directory) {
        id configured = [self runtimeOptions][@"preferences_directory"];
        if ([configured isKindOfClass:NSString.class] && [configured isAbsolutePath]) directory = configured;
    }
    if (!directory) {
        _englishOnlineCandidatesEnabled = MSIMEStrictBoolean([self runtimeOptions][@"preferences"][@"english_online_candidates"]);
        _englishOnlinePreferenceLoaded = YES;
        return;
    }
    // 与 applyPreferencesToSessionNow 一样在主线程直接读：只在本进程第一次用到时读这一次，不经过后台轮询的读取入口。
    NSDictionary *snapshot = [MSIMEClientSession loadPreferencesInDirectory:directory error:nil];
    NSDictionary *preferences = [snapshot isKindOfClass:NSDictionary.class] ? snapshot[@"preferences"] : nil;
    if ([preferences isKindOfClass:NSDictionary.class]) {
        _englishOnlineCandidatesEnabled = MSIMEStrictBoolean(preferences[@"english_online_candidates"]);
        _englishOnlinePreferenceLoaded = YES;
        if (!_appearance.sharedInputPreferencesApplied) [_appearance applySharedInputPreferences:preferences];
    }
}
// 在系统输入菜单里打开一个按需模式。只有 Engine 会话会碰到 TIS；测试的替身会话只走测试设的 enabler，没有就什么也不做，所以测试不会改开发者的输入菜单。返回 NO 表示模式没有打开——没有发出请求，或者系统拒绝了——这次切换不记为已同步，下一次同步再试；以前不论 TIS 返回什么都算成功，启用失败一次，这个模式就再也不会被打开。
- (BOOL)enableOptInInputMode:(NSString *)identifier {
    if (_optInInputModeEnabler) return _optInInputModeEnabler(identifier) == noErr;
    if (![_session isKindOfClass:MSIMEClientSession.class]) return NO;
    return MSIMEEnableInputMode(identifier, TISCreateInputSourceList, TISEnableInputSource) == noErr;
}
// 让输入菜单里选中的模式（中、双、五、粤、注、英、日、한、越、藏或笔）与中英文状态和实际运行的方案保持一致。系统报告过的切换已经记为当前显示的模式，所以这里不会把它回声回去。
//
// 所有方案切换都会走到这里——不论来自输入菜单、两个设置窗口、Tauri 设置页还是用户选中的模式——所以按需模式也在这里打开：实际运行的方案从上次同步的方案（lastSyncedInputScheme，跨启动保留）变成粤拼、注音、越南文、藏文或笔画时，先启用对应模式再选中它。这里看的是实际生效的方案，所以词库装好之前就选了的方案，在词库到位时才算选中。方案和上次同步的一样时不启用任何模式，用户从输入菜单移除的模式不会在每次启动时被加回来。第一次同步（还没有 lastSyncedInputScheme）落在这五个方案上同样启用：这个持久记录是后来才加的，加上它之前就在用粤拼的人，否则要先切走再切回来才能看到「粤」。模式没能打开时不记录这次切换，下一次同步再试。macOS 27 不允许进程启用键盘输入模式，`TISEnableInputSource` 在那里返回 noErr 而状态不变，所以这里在那个版本上打不开任何模式，只是照常记录；用户要在系统设置里自己添加，设置页的「菜单栏入口」会说明去哪里加。
- (void)syncSystemInputModeForClient:(id)client {
    if (MSIMEEditionIsShurufa()) {
        // 系统输入源是唯一权威；旧偏好和模式通知都不能主动选源。
        NSString *observed = [self selectedSystemInputModeID] ?: @"";
        os_log(MSIMEUILog(), "mode_sync observed=%{public}@ emitted=0 policy=system_authority", observed);
        return;
    }
    // 共享偏好会通知多个 IMK 控制器，已失去焦点的客户端不能改当前系统输入源。
    if (MSIMEFocusedController && MSIMEFocusedController != self) return;
    // 显式同步优先于尚未执行的激活校准，不能再用滞后的 TIS 状态覆盖刚发出的选择。
    ++_inputModeActivationGeneration;
    NSString *scheme = MSIMEEffectiveInputScheme(_appearance.inputScheme, _appearance.lastChineseScheme, [self inputSchemeHostOptions]);
    NSString *synced = _appearance.lastSyncedInputScheme;
    if (![scheme isEqualToString:synced]) {
        NSString *optIn = MSIMEOptInInputModeToEnable(synced, scheme, YES);
        if (!optIn || [self enableOptInInputMode:optIn]) _appearance.lastSyncedInputScheme = scheme;
    }
    NSString *mode = MSIMEInputModeID(MSIMEInputModeFor(_appearance.englishMode, scheme));
    NSString *observed = [self selectedSystemInputModeID] ?: @"";
    MSIMESelectSystemInputMode(MSIMESharedSystemInputModeState(), mode, client,
        MSIMEInputSourceIsEnabled, observed);
    // 菜单栏状态项显示控制器算出的模式，而不是 MSIMESelectSystemInputMode 回退后的那个：「双」「五」等模式没有加成输入源时系统图标停在「中」，状态项仍显示真实方案。
    [self presentInputModeStatusItem:_appearance.inputModeStatusItem ? mode : nil];
}
// 单独读取入口让原生测试注入合成输入源，不切换开发者的真实输入源。
- (NSString *)selectedSystemInputModeID {
    TISInputSourceRef source = TISCopyCurrentKeyboardInputSource();
    if (!source) return nil;
    CFTypeRef value = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
    NSString *identifier = value && CFGetTypeID(value) == CFStringGetTypeID() ? [(__bridge NSString *)value copy] : nil;
    CFRelease(source);
    return identifier;
}

- (void)scheduleSystemInputModeSyncForClient:(id)client {
    if (MSIMEEditionIsShurufa()) { [self syncSystemInputModeForClient:client]; return; }
    [self presentInputModeStatusItem:_appearance.inputModeStatusItem ? [self currentInputModeID] : nil];
    const NSUInteger generation = ++_inputModeActivationGeneration;
    __weak MSIMEInputController *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_inputModeActivationGeneration != generation ||
            controller->_activeClient != client || MSIMEFocusedController != controller) return;
        if (!MSIMERecordRestoredSystemInputMode(MSIMESharedSystemInputModeState(), [controller selectedSystemInputModeID])) return;
        [controller syncSystemInputModeForClient:client];
    });
}

// 控制器此刻的输入模式：英文模式是「英」，其余按实际运行的方案（MSIMEEffectiveInputScheme）。
- (NSString *)currentInputModeID {
    NSString *scheme = MSIMEEffectiveInputScheme(_appearance.inputScheme, _appearance.lastChineseScheme, [self inputSchemeHostOptions]);
    return MSIMEInputModeID(MSIMEInputModeFor(_appearance.englishMode, scheme));
}
// 把菜单栏状态项摆成 `mode`，nil 表示隐藏。只有输入法进程（Info.plist 声明了 IMK 连接名）真的往菜单栏放东西；测试进程不碰菜单栏，测试经 inputModeStatusItemPresenter 换上替身。
- (void)presentInputModeStatusItem:(NSString *)mode {
    if (_inputModeStatusItemPresenter) { _inputModeStatusItemPresenter(mode); return; }
    if (!NSBundle.mainBundle.infoDictionary[@"InputMethodConnectionName"]) return;
    MSIMEInputModeStatusItem *item = [MSIMEInputModeStatusItem sharedItem];
    if (mode) [item showMode:mode];
    else [item hide];
}
// The system reports the mode the user picked from the input menu or reached with Ctrl+Space; the controller's Chinese/English state and, for every mode but 英, its scheme follow it. A report that only repeats the mode already shown, or one delivered from inside this controller's own selectInputMode:, leaves the state alone.
- (void)setValue:(id)value forTag:(long)tag client:(id)sender {
    if (tag == kTextServiceInputModePropertyTag) [self systemDidReportInputMode:value client:sender];
    [super setValue:value forTag:tag client:sender];
}
- (void)systemDidReportInputMode:(id)value client:(id)sender {
    MSIMEInputController *focused = MSIMEFocusedController;
    if (MSIMEEditionIsShurufa()) {
        // 旧客户端的模式报告不能改变当前会话；英文由系统 ABC 承担。
        if ((focused && focused != self) || (_activeClient && sender != _activeClient) ||
            MSIMEInputModeForID(value) == MSIMEInputMode::English) return;
    }
    // 其他版本继续把模式报告交给持有焦点的控制器。
    if (focused && focused != self) { [focused systemDidReportInputMode:value client:sender]; return; }
    if (!MSIMEAdoptReportedInputMode(MSIMESharedSystemInputModeState(), value)) return;
    [self ensureAppearance];
    [self applySharedInputPreferencesBeforeFirstUse];
    // The report can arrive before activateServer: or handleEvent: has named the client, and the mode is remembered per application.
    [_appearance activateInputModeForApplication:[sender respondsToSelector:@selector(bundleIdentifier)] ? [sender bundleIdentifier] : nil];
    // Moving from 英 to 日 changes two things, and each change syncs the menu bar on its own: between them it would select 中 or 英 again and the system would report that back as a new choice. Holding `selecting` keeps both quiet, and the sync below selects the one mode they add up to. 英 leaves the scheme alone, so returning to any other mode afterwards finds it where it was.
    MSIMESystemInputModeState &state = MSIMESharedSystemInputModeState();
    state.selecting = true;
    const MSIMEInputMode mode = MSIMEInputModeForID(value);
    NSString *scheme = _appearance.inputScheme;
    NSString *target =
        MSIMESchemeForReportedInputMode(mode, scheme, _appearance.lastChineseScheme, MSIMEInputSourceIsEnabled);
    // A mode whose scheme cannot run here (粤, 注 or 笔 without its dictionary) leaves the scheme alone. Adopting it only made the Engine fall back to the Chinese scheme used before it, so the menu bar said 粤 while the keys typed that scheme - shuangpin, for one user who had never picked Cantonese. The mode can be selectable without the dictionary: on macOS 27.0.1 the system turned 粤 on by itself after --register-input-source enabled the bundle, and it came back each time the user removed it.
    if (target && !MSIMEInputSchemeAvailable(target, [self inputSchemeHostOptions])) target = nil;
    if (target && ![target isEqualToString:scheme]) {
        // The composition was typed under the old scheme and a scheme switch discards it, so commit it first, as the scheme menu does. A Korean syllable is text the user already wrote.
        if (_session && _activeClient && _viewState.hasEditableText()) {
            [self dispatchCommand:MSIME_FINISH_COMPOSITION];
        }
        _appearance.inputScheme = target;
        [self applyPreferencesToSessionNow];
    }
    const BOOL english = _appearance.englishMode;
    [self setEnglishInputMode:mode == MSIMEInputMode::English showHUD:NO];
    state.selecting = false;
    // setEnglishInputMode: can refuse the switch (an Engine cancel failure while composing) after the report was already recorded as shown; select the mode the controller actually has so the menu bar does not keep the refused one. A switch that went through makes this a no-op.
    [self syncSystemInputModeForClient:sender];
    if (english != _appearance.englishMode) [self showCurrentInputModeHUD];
    else if (![scheme isEqual:_appearance.inputScheme]) [self showInputSchemeHUD];
}
- (void)selectChineseMode:(id)sender {
    (void)sender;
    if (_managedEnglishComposition) [self setEnglishInputMode:NO];
    else if (_viewState.dedicatedEnglish) [self setDedicatedEnglishInputMode:NO];
    else [self setEnglishInputMode:NO];
}
- (void)setDedicatedEnglishInputMode:(BOOL)enabled {
    if (MSIMEEditionIsShurufa()) return;
    if (!_activeClient) return;
    [self ensureAppearance];
    if (!_session || _focusPending) [self prepareSession];
    if (!_session) return;
    const BOOL changed = _appearance.englishMode || _viewState.dedicatedEnglish != enabled;
    if (_viewState.hasEditableText()) {
        // Ctrl+Shift+E is the reference's FUNCTION_CANCEL in both directions: _HandleCancel terminates the composition and commits nothing, so neither the highlighted Chinese candidate nor the English word being spelled reaches the document. A scheme that commits on blur (a Korean syllable, a Zhuyin or Vietnamese composition) is already text the user wrote, so it is committed instead. An Engine failure leaves the mode as it was.
        if (![self dispatchCommand:MSIMESchemeTrait(_viewState, msime::mac::scheme::CommitsOnBlur) ? MSIME_FINISH_COMPOSITION : MSIME_CANCEL]) return;
    }
    NSError *error = nil;
    NSDictionary *view = [_session setDedicatedEnglishEnabled:enabled error:&error];
    if (!view) { if (error) NSBeep(); return; }
    _managedEnglishComposition = NO;
    _appearance.englishMode = NO;
    [self syncSystemInputModeForClient:_activeClient];
    [self apply:@{@"view":view}];
    if (changed) [self showCurrentInputModeHUD];
}
- (void)toggleDedicatedEnglishMode:(id)sender {
    (void)sender;
    [self setDedicatedEnglishInputMode:_appearance.englishMode || !_viewState.dedicatedEnglish];
}
- (void)setTraditionalOutput:(BOOL)enabled {
    [self ensureAppearance];
    const BOOL changed = _appearance.traditionalOutput != enabled;
    _appearance.traditionalOutput = enabled;
    if (changed) [self showInputStateHUDText:enabled ? @"繁" : @"简"
        announcement:enabled ? @"繁体输出" : @"简体输出"];
}
- (void)selectSimplifiedOutput:(id)sender { (void)sender; [self setTraditionalOutput:NO]; }
- (void)selectTraditionalOutput:(id)sender { (void)sender; [self setTraditionalOutput:YES]; }
- (void)toggleTraditionalOutput:(id)sender { (void)sender; [self ensureAppearance]; [self setTraditionalOutput:!_appearance.traditionalOutput]; }
- (void)selectEnglishMode:(id)sender { (void)sender; [self setEnglishInputMode:YES]; }
- (void)showSystemCharacterPalette { [NSApp orderFrontCharacterPalette:nil]; }
- (void)checkForUpdates:(id)sender { (void)sender; }
- (void)showVoiceSettings:(id)sender {
    (void)sender;
    [[MetasequoiaVoiceProviderSettingsWindow sharedController] showAndActivate];
}
- (BOOL)usesLocalModelVoice {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    NSString *provider = [defaults stringForKey:@"MSIMEClientVoiceASRProvider"] ?: @"";
    return MSIMEVoiceUsesLocalModelHelper(provider, MSIMEVoiceProviderSocket() != nil,
                                          MSIMELocalVoiceModelDirectory([defaults stringForKey:@"MSIMEClientVoiceASRModelPath"]));
}
- (BOOL)usesTextStreamVoice {
    return MSIMEVoiceUsesTextStream([NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoiceASRProvider"], MSIMEVoiceProviderSocket() != nil);
}
- (BOOL)usesNativeHTTPVoice {
    NSString *provider = [NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoiceASRProvider"] ?: @"";
    // An installed model directory streams through the helper instead; "local" naming anything else never records (MSIMEVoiceLocalModelMissing).
    if ([self usesLocalModelVoice]) return NO;
    return MSIMEVoiceUsesNativeHTTPProvider(provider, MSIMEVoiceProviderSocket() != nil);
}
- (BOOL)usesNativeDoubaoVoice {
    NSString *provider = [NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoiceASRProvider"] ?: @"local";
    return (!MSIMEVoiceProviderSocket() && [provider.lowercaseString isEqual:@"doubao"]) || [self usesLocalModelVoice] || [self usesTextStreamVoice];
}
- (void)dealloc {
    [self stopCandidateFrameAnimation];
    [MSIMEInputSessionRecoveryPanel dismissForDelegate:self];
    [self stopEditableShiftMouseMonitoring];
    [_capsLockTapTimer invalidate];
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    [NSDistributedNotificationCenter.defaultCenter removeObserver:self];
    [NSWorkspace.sharedWorkspace.notificationCenter removeObserver:self];
    if (_globalVoiceHotkeyMonitor) [NSEvent removeMonitor:_globalVoiceHotkeyMonitor];
    [_httpVoiceRequest cancel]; [_doubaoVoiceRequest cancel];
    [_doubaoPolishRequest cancel]; [_livePolishRequest cancel];
    [_voicePolishTask cancel]; _voicePolishTask = nil; _voicePolishPending = NO;
    [_voiceEditTracker cancel]; _voiceEditTracker = nil;
    [_voiceRetryStreamRequest cancel]; _voiceRetryStreamRequest = nil;
    [_voiceRetryHTTPRequest cancel]; _voiceRetryHTTPRequest = nil;
    [_voiceRetryPolishTask cancel]; _voiceRetryPolishTask = nil;
    _voiceRetryInFlight = NO;
    // A client that dies without deactivateServer: leaves the repeating timer on the run loop, and its candidate window on screen.
    [_preferencesTimer invalidate];
    [self flushKeyPresses];
    [_panel orderOut:nil];
}
- (dispatch_queue_t)voicePipelineQueue {
    if (!_voicePipelineQueue) {
        _voicePipelineQueue = dispatch_queue_create("dev.shurufa.voice-pipeline", DISPATCH_QUEUE_SERIAL);
    }
    return _voicePipelineQueue;
}
- (MSIMEVoicePolishClient *)sharedVoicePolishClient {
    if (!_voicePolishClient) _voicePolishClient = [[MSIMEVoicePolishClient alloc] init];
    return _voicePolishClient;
}
- (MSIMEVoiceEditTracker *)sharedVoiceEditTracker {
    if (!_voiceEditTracker) _voiceEditTracker = [[MSIMEVoiceEditTracker alloc] init];
    return _voiceEditTracker;
}
- (void)beginVoiceEditTrackingForText:(NSString *)text preferencesDirectory:(NSString *)prefsDir {
    if (![text isKindOfClass:NSString.class] || !text.length) return;
    if (![prefsDir isKindOfClass:NSString.class] || !prefsDir.length) return;
    MSIMEVoiceContextSnapshot *snapshot = _voiceContextSnapshot;
    if (!MSIMEVoiceOptionsContextAware(_voiceOptionsSnapshot)) return;
    if (snapshot.isSecureField || snapshot.axBlacklisted) return;
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    const pid_t targetPID = front ? front.processIdentifier : 0;
    NSString *bundleID = snapshot.appBundleID.length ? snapshot.appBundleID : front.bundleIdentifier;
    [[self sharedVoiceEditTracker] beginTrackingInsertedText:text
                                        preferencesDirectory:prefsDir
                                                   targetPID:targetPID
                                                    bundleID:bundleID
                                               isSecureField:snapshot.isSecureField
                                               axBlacklisted:snapshot.axBlacklisted];
}
// 仅采纳测试子类覆写返回的非 MSIMEHTTPVoiceRequest 夹具；生产路径统一交由 MSIMEVoicePolishClient 处理。
- (id)injectedLegacyPolisherForSelector:(SEL)selector options:(NSDictionary *)options {
    if ([self methodForSelector:selector] == [MSIMEInputController instanceMethodForSelector:selector]) {
        return nil;
    }
    id candidate = selector == @selector(makeDoubaoPolishRequest:)
        ? [self makeDoubaoPolishRequest:options]
        : [self makeLiveVoicePolishRequest:options];
    if (!candidate) return nil;
    if ([candidate isKindOfClass:MSIMEHTTPVoiceRequest.class]) {
        [(MSIMEHTTPVoiceRequest *)candidate cancel];
        return nil;
    }
    return candidate;
}
- (void)prepareVoicePipelineForOptions:(NSDictionary *)options {
    [self cancelArchivedVoiceRetry];
    [self bindVoiceServiceResilienceHandlers];
    if ([self shouldShowVoiceRecoveryToast]) [[MSIMEVoiceRecoveryToast sharedToast] dismiss];
    _voiceSalvageNotice = nil;
    [_voiceEditTracker cancel];
    [_voicePolishTask cancel];
    _voicePolishTask = nil;
    _voicePolishPending = NO;
    _voiceOptionsSnapshot = [options copy];
    _voiceAsrStartedAtNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
    _voiceContextSnapshot = MSIMECaptureVoiceContext(_activeClient);
    _voiceVocabularyTerms = nil;
    const uint64_t vocabGen = _voiceGeneration;
    _voiceVocabularyGeneration = vocabGen;
    NSDictionary *hostOptions = [_session respondsToSelector:@selector(hostOptions)]
        ? [_session.hostOptions copy] : nil;
    const BOOL captureVisible = MSIMEVoiceOptionsContextAware(options) &&
                                !_voiceContextSnapshot.isSecureField &&
                                !_voiceContextSnapshot.axBlacklisted;
    NSRunningApplication *front = captureVisible ? NSWorkspace.sharedWorkspace.frontmostApplication : nil;
    const pid_t frontPID = front ? front.processIdentifier : 0;
    NSString *frontBundleID = _voiceContextSnapshot.appBundleID.length
        ? [_voiceContextSnapshot.appBundleID copy]
        : [front.bundleIdentifier copy];
    if (hostOptions.count || (captureVisible && frontPID > 0)) {
        __weak MSIMEInputController *weakSelf = self;
        dispatch_async([self voicePipelineQueue], ^{
            MSIMEVoiceVocabularyBundle *bundle = hostOptions.count
                ? MSIMEVoiceLoadVocabulary(hostOptions, nil, nil) : nil;
            NSString *visibleScreen = (captureVisible && frontPID > 0)
                ? MSIMECaptureVisibleScreenTextForPID(
                      frontPID, frontBundleID, MSIMEVoiceContextMaxVisibleScreenTextUTF16, 0.22)
                : @"";
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEInputController *controller = weakSelf;
                if (!controller || controller->_voiceVocabularyGeneration != vocabGen) return;
                if (bundle) controller->_voiceVocabularyTerms = bundle.terms;
                if (visibleScreen.length && controller->_voiceContextSnapshot) {
                    controller->_voiceContextSnapshot =
                        [controller->_voiceContextSnapshot snapshotWithVisibleScreenText:visibleScreen];
                }
            });
        });
    }
    NSString *prewarmPath = nil;
    if (MSIMEVoiceResolveLocalPolishPrewarmPath(options, &prewarmPath)) {
        [MSIMELocalPolishServer.sharedServer prewarmWithModelPath:prewarmPath];
    }
}
- (void)clearVoicePipelineState {
    [_voicePolishTask cancel];
    _voicePolishTask = nil;
    _voicePolishPending = NO;
    _voiceContextSnapshot = nil;
    _voiceOptionsSnapshot = nil;
    _voiceVocabularyTerms = nil;
    _voiceVocabularyGeneration = 0;
    _voiceAsrStartedAtNs = 0;
    _useVoicePolishClientForDoubao = NO;
    _useVoicePolishClientForLive = NO;
}
- (NSArray<NSDictionary<NSString *, id> *> *)cachedVoiceVocabularyTerms {
    return _voiceVocabularyGeneration == _voiceGeneration ? _voiceVocabularyTerms : nil;
}
- (void)noteVoiceArchiveDismissed:(NSString *)reason {
    if (_voiceService.archiveRecordingID &&
        [MSIMEVoiceInputService.archive respondsToSelector:@selector(noteDismissedReason:forRecording:)]) {
        [MSIMEVoiceInputService.archive noteDismissedReason:reason ?: @"dismissed"
                                               forRecording:_voiceService.archiveRecordingID];
    }
}
- (BOOL)voiceServiceReportsAccidentalSilence {
    return [_voiceService respondsToSelector:@selector(isLikelyAccidentalSilence)] &&
        _voiceService.isLikelyAccidentalSilence;
}
- (uint64_t)currentVoiceAsrMillisecondsForRequest:(id)request {
    if ([request respondsToSelector:@selector(lastFinalMetrics)]) {
        NSDictionary *metrics = [(MSIMELocalVoiceRequest *)request lastFinalMetrics];
        if ([metrics[@"asr_ms"] isKindOfClass:NSNumber.class]) {
            const long long value = [metrics[@"asr_ms"] longLongValue];
            if (value > 0) return static_cast<uint64_t>(value);
        }
    }
    if (_voiceAsrStartedAtNs > 0) {
        const uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        if (now > _voiceAsrStartedAtNs) return (now - _voiceAsrStartedAtNs) / 1000000ULL;
    }
    return 0;
}
- (NSNumber *)currentVoiceAudioMillisecondsForRequest:(id)request {
    if ([request respondsToSelector:@selector(lastFinalMetrics)]) {
        NSDictionary *metrics = [(MSIMELocalVoiceRequest *)request lastFinalMetrics];
        if ([metrics[@"audio_ms"] isKindOfClass:NSNumber.class]) return metrics[@"audio_ms"];
    }
    const NSTimeInterval duration = _voiceService.recordedDuration;
    if (duration > 0.0) return @(static_cast<uint64_t>(duration * 1000.0 + 0.5));
    return nil;
}
- (MSIMEHTTPVoiceRequest *)makeDoubaoPolishRequest:(NSDictionary *)options {
    if (!([options[@"polish_enabled"] boolValue] || [options[@"polish_text"] boolValue]) || ![options[@"polish_token"] length]) return nil;
    return [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
}
- (void)applyDoubaoFinalText:(NSString *)text request:(id<MSIMEStreamingVoiceRequest>)request polishFailed:(BOOL)polishFailed {
    [self applyDoubaoFinalText:text request:request polishFailed:polishFailed replaceSelection:NO archiveMetadata:nil];
}
- (void)applyDoubaoFinalText:(NSString *)text
                     request:(id<MSIMEStreamingVoiceRequest>)request
                polishFailed:(BOOL)polishFailed
            replaceSelection:(BOOL)replaceSelection
             archiveMetadata:(NSDictionary<NSString *, id> *)archiveMetadata {
    if (_doubaoVoiceRequest != request) return;
    if ([self ownsDoubaoVoiceFocus]) {
        if (!text.length) { [self reportVoiceFailure:MSIMEVoiceFailureNoSpeech]; return; }
        NSDictionary *result = text.length ? [_doubaoVoiceSession applyVoiceText:text generation:_doubaoVoiceGeneration error:nil] : nil;
        if (!result) { [self reportVoiceFailure:MSIMEVoiceFailureSession]; return; }
        if (result) {
            if (![self ownsPersistentVoiceRecording]) _doubaoVoiceMarked = NO;
            [self applyVoiceResult:result
                             route:_doubaoVoiceCommit
                      polishFailed:polishFailed
                  replaceSelection:replaceSelection
                   archiveMetadata:archiveMetadata];
        }
    }
    [self cancelDoubaoVoiceInput];
}
- (MSIMEDoubaoVoiceRequest *)makeDoubaoVoiceRequest:(NSDictionary *)options error:(NSError **)error {
    return [[MSIMEDoubaoVoiceRequest alloc] initWithOptions:options error:error];
}
- (id<MSIMEStreamingVoiceRequest>)makeLocalVoiceRequest:(NSDictionary *)options error:(NSError **)error {
    return [[MSIMELocalVoiceRequest alloc] initWithOptions:options hostOptions:_session.hostOptions error:error];
}
- (id<MSIMEStreamingVoiceRequest>)makeTextStreamVoiceRequest:(NSDictionary *)options error:(NSError **)error {
    return [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:options hostOptions:_session.hostOptions error:error];
}
- (id<MSIMEStreamingVoiceRequest>)makeStreamingVoiceRequest:(NSDictionary *)options error:(NSError **)error {
    id provider = options[@"asr_provider"];
    if ([provider isKindOfClass:NSString.class] && [[provider lowercaseString] isEqual:@"textstream"])
        return [self makeTextStreamVoiceRequest:options error:error];
    if ([provider isKindOfClass:NSString.class] && [[provider lowercaseString] isEqual:@"local"])
        return [self makeLocalVoiceRequest:options error:error];
    return [self makeDoubaoVoiceRequest:options error:error];
}
- (BOOL)ownsDoubaoVoiceFocus {
    return _doubaoVoiceRequest && [self voiceFocusMatchesClient:_doubaoVoiceClient session:_doubaoVoiceSession generation:_doubaoVoiceGeneration];
}
- (void)voiceCaptureDidStart {
    if (_voiceCueRecording) return;
    _voiceCueRecording = YES;
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    // Windows plays the start cue and only then mutes other audio. The macOS mute is device-wide and would swallow the cue, so it waits for the cue to finish; any restore before then cancels it.
    void (^mute)(void) = nil;
    if (MSIMEVoiceMuteSystemAudioEnabled(defaults)) mute = [_voiceAudioMuter deferredMute];
    if (MSIMEVoiceCuesEnabled(defaults)) [_voiceCuePlayer playStartCueThen:mute];
    else if (mute) mute();
}
- (void)voiceCaptureDidEnd {
    if (!_voiceCueRecording) return;
    _voiceCueRecording = NO;
    if (MSIMEVoiceCuesEnabled(NSUserDefaults.standardUserDefaults)) [_voiceCuePlayer playStopCue];
}
- (NSScreen *)voiceInputScreen {
    if (!_activeClient) return nil;
    NSRect caret = NSZeroRect;
    [(id<IMKTextInput>)_activeClient attributesForCharacterIndex:0 lineHeightRectangle:&caret];
    if (!MSIMEValidCaret(caret)) return nil;
    return MSIMEScreenForPoint(NSMakePoint(NSMidX(caret), NSMidY(caret)));
}
- (void)refreshVoiceOverlayScreen {
    if (_voiceOverlay) _voiceOverlay.preferredScreen = [self voiceInputScreen];
}
// 只有开麦之后才会发生的失败；权限、凭据、模型缺失在开麦之前就拦下了。
static BOOL MSIMEVoiceFailureFollowsCapture(MSIMEVoiceFailure failure) {
    switch (failure) {
        case MSIMEVoiceFailureCapture: case MSIMEVoiceFailureProvider: case MSIMEVoiceFailureNoSpeech:
        case MSIMEVoiceFailureTimeout: case MSIMEVoiceFailureSession: return YES;
        default: return NO;
    }
}
// 存档里记下的失败原因：一句话说明加上识别服务给出的细节。
static NSString *MSIMEVoiceArchiveFailureReason(MSIMEVoiceFailure failure, NSString *detail) {
    NSString *reason = failure == MSIMEVoiceFailureCapture ? @"录音失败" : failure == MSIMEVoiceFailureProvider ? @"识别失败" :
        failure == MSIMEVoiceFailureNoSpeech ? @"未识别到语音" : failure == MSIMEVoiceFailureTimeout ? @"处理超时" : @"语音会话不可用";
    return detail.length ? [NSString stringWithFormat:@"%@：%@", reason, detail] : reason;
}
- (void)noteVoiceArchiveCancelled {
    if (_voiceService.archiveRecordingID) [MSIMEVoiceInputService.archive noteCancelledRecording:_voiceService.archiveRecordingID];
}
- (BOOL)shouldShowVoiceRecoveryToast {
    return MSIMEEditionIsShurufa() && (_voiceOverlay == nil || [_voiceOverlay isKindOfClass:MSIMEVoiceWaveOverlay.class]);
}
- (MSIMEVoiceTargetEditableState)voiceTargetEditabilityForClient:(id)client {
    const MSIMEVoiceTargetEditabilityProbeResult probe = MSIMEProbeVoiceTargetEditabilityDetailed(client);
    os_log(MSIMEUILog(),
           "voice_target_editability state=%ld ax_trusted=%d front_match=%d focus_err=%d role=%{public}@",
           (long)probe.state, probe.axTrusted ? 1 : 0, probe.frontmostMatchesClient ? 1 : 0,
           (int)probe.focusError, probe.axRole ?: @"none");
    return probe.state;
}
- (void)showVoiceHistoryFromRecoveryToast {
    [[MSIMEPreferencesWindowController sharedController] showAndActivateWithPageIdentifier:@"voice-history"];
}
- (void)cancelArchivedVoiceRetry {
    if (!_voiceRetryInFlight && !_voiceRetryStreamRequest && !_voiceRetryHTTPRequest && !_voiceRetryPolishTask) return;
    ++_voiceRetryGeneration;
    _voiceRetryInFlight = NO;
    [_voiceRetryStreamRequest cancel];
    _voiceRetryStreamRequest = nil;
    [_voiceRetryHTTPRequest cancel];
    _voiceRetryHTTPRequest = nil;
    [_voiceRetryPolishTask cancel];
    _voiceRetryPolishTask = nil;
    if ([_voiceOverlay isKindOfClass:MSIMEVoiceWaveOverlay.class] && !_voiceService.active) {
        [_voiceOverlay setListening:NO];
    }
}
- (void)bindVoiceServiceResilienceHandlers {
    if (!_voiceService || ![_voiceService respondsToSelector:@selector(setCaptureRouteDidChangeHandler:)]) return;
    __weak MSIMEInputController *weakSelf = self;
    _voiceService.captureRouteDidChangeHandler = ^(NSString *deviceName, BOOL recovered) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || ![controller shouldShowVoiceRecoveryToast]) return;
        NSString *safeName = deviceName.length ? deviceName : @"系统默认麦克风";
        NSString *title = recovered ? @"麦克风已自动切换" : @"原麦克风不可用，已切换";
        NSString *detail = [NSString stringWithFormat:@"当前使用「%@」，录音未中断。", safeName];
        [[MSIMEVoiceRecoveryToast sharedToast] showRouteNoticeTitle:title detail:detail];
    };
    _voiceService.captureDeviceLostHandler = ^(NSError *error) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || !controller->_voiceService.active) return;
        const BOOL hasSalvageableSpeech = controller->_voiceService.recordedDuration >= 0.4 &&
                                          controller->_voiceService.hasLikelySpeech;
        if (hasSalvageableSpeech) {
            controller->_voiceSalvageNotice = @"麦克风已断开，已保留并转写已录内容";
            if (controller->_doubaoVoiceRequest && !controller->_doubaoVoiceProcessing) {
                [controller finishDoubaoVoiceInput];
                return;
            }
            if (controller->_httpVoiceRequest && !controller->_httpVoiceProcessing) {
                [controller finishHTTPVoiceInput];
                return;
            }
            if (controller->_liveVoiceToken && !controller->_liveVoiceProcessing) {
                [controller finishLiveVoiceInput];
                return;
            }
        }
        [controller reportVoiceFailure:MSIMEVoiceFailureCapture detail:error.localizedDescription];
    };
}
- (void)deliverRecoveredVoiceText:(NSString *)text
                      recordingID:(NSString *)recordingID
                     polishFailed:(BOOL)polishFailed {
    if (!text.length) return;
    if (recordingID.length && [MSIMEVoiceInputService.archive respondsToSelector:@selector(noteRetranscription:forRecording:)]) {
        [MSIMEVoiceInputService.archive noteRetranscription:text forRecording:recordingID];
    }
    MSIMEInputController *destination = [self currentVoiceDestination];
    BOOL targetWasNonEditable = NO;
    if (destination && [self voiceTargetEditabilityForClient:destination->_activeClient] == MSIMEVoiceTargetEditableStateNotEditable) {
        destination = nil;
        targetWasNonEditable = YES;
    }
    NSString *output = text;
    MSIMEAppearancePreferences *outputAppearance = destination ? destination->_appearance : _appearance;
    if (outputAppearance.traditionalOutput) {
        output = MSIMEChineseOutputString(output, YES);
    }
    if (destination) {
        if (destination->_session) [destination apply:[destination->_session command:MSIME_FINISH_COMPOSITION error:nil]];
        [destination noteSmartPunctuationShadow:[output characterAtIndex:output.length - 1]];
        [(id<MSIMETextClient>)destination->_activeClient insertText:output replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
        destination->_clientHasMarkedText = NO;
        NSString *prefsDir = destination->_preferencesDirectory ?: MSIMEStatisticsHostOptions(destination->_session)[@"preferences_directory"];
        MSIMERecordTypingStatistics(prefsDir, output, msime::mac::TypingSource::Voice);
        [destination beginVoiceEditTrackingForText:output preferencesDirectory:prefsDir];
        if (polishFailed) [_voiceOverlay showCompletionNotice:@"重试完成（整理失败，已输入原文）"];
        return;
    }
    const BOOL copied = [self copyVoiceText:output];
    _lastUndeliveredVoiceText = [output copy];
    if ([self shouldShowVoiceRecoveryToast]) {
        NSString *title = targetWasNonEditable
            ? (copied ? @"重试完成 · 当前位置不可输入，已复制" : @"重试完成 · 请点击复制")
            : (copied ? @"重试完成 · 焦点已离开，已复制" : @"重试完成 · 请点击复制");
        __weak MSIMEInputController *weakSelf = self;
        [[MSIMEVoiceRecoveryToast sharedToast] showUndeliveredText:output
                                                             title:title
                                                            copied:copied
                                                       copyHandler:^BOOL(NSString *copyTarget) {
            MSIMEInputController *inner = weakSelf;
            return inner ? [inner copyVoiceText:copyTarget] : MSIMEWriteVoiceClipboard(copyTarget, NSPasteboard.generalPasteboard);
        }];
    }
}
- (void)retryArchivedVoiceRecording:(NSString *)recordingID {
    if (!recordingID.length || _voiceService.active || _voiceRetryInFlight) return;
    id<MSIMEVoiceArchiving> archive = MSIMEVoiceInputService.archive;
    if (!archive || ![archive respondsToSelector:@selector(PCMForRecording:)]) return;
    if (MSIMEVoiceProviderSocket()) return;

    _voiceRetryInFlight = YES;
    const uint64_t retryGen = ++_voiceRetryGeneration;
    if (!_voiceOverlay) {
        _voiceOverlay = [[MSIMEVoiceWaveOverlay alloc] init];
        [_voiceOverlay setPersistentRecording:MSIMEEditionIsShurufa()];
        [_voiceOverlay applyThemePreferences:_voiceThemePreferences ?: @{}];
    }
    [self refreshVoiceOverlayScreen];
    [_voiceOverlay setProcessing:NO];
    if ([_voiceOverlay respondsToSelector:@selector(setProcessingHint:)]) {
        [_voiceOverlay setProcessingHint:@"正在重试识别已保存录音…"];
    }

    NSDictionary *query = MSIMEVoiceRecognitionQuery(NSUserDefaults.standardUserDefaults, 0, NO);
    NSDictionary *hostOptions = [_session respondsToSelector:@selector(hostOptions)]
        ? [_session.hostOptions copy] : MSIMELoadRuntimeOptions();
    MSIMEVoiceContextSnapshot *snapshot = MSIMECaptureVoiceContext([self currentVoiceDestination] ? [self currentVoiceDestination]->_activeClient : _activeClient);
    const BOOL contextAware = MSIMEVoiceOptionsContextAware(query);
    NSString *appCategory = contextAware ? MSIMEVoiceInferAppCategory(snapshot) : @"general";

    __weak MSIMEInputController *weakSelf = self;
    void (^failRetry)(NSString *) = ^(NSString *reason) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_voiceRetryGeneration != retryGen) return;
        [controller cancelArchivedVoiceRetry];
        if ([controller shouldShowVoiceRecoveryToast]) {
            [[MSIMEVoiceRecoveryToast sharedToast]
                showRetryableFailureTitle:@"重试识别失败 · 录音仍已保留"
                                   detail:reason.length ? reason : @"请检查本地语音模型或网络设置后重试"
                             retryHandler:^{ [weakSelf retryArchivedVoiceRecording:recordingID]; }
                           historyHandler:^{ [weakSelf showVoiceHistoryFromRecoveryToast]; }];
        }
    };

    void (^finalizeRetryText)(NSString *, NSNumber *) = ^(NSString *rawText, NSNumber *audioMs) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_voiceRetryGeneration != retryGen) return;
        if (!rawText.length) { failRetry(@"未识别到语音内容"); return; }
        NSString *modelKind = [query[@"asr_provider"] isKindOfClass:NSString.class] ? query[@"asr_provider"] : @"";
        dispatch_async([controller voicePipelineQueue], ^{
            MSIMEVoiceVocabularyBundle *vocab = hostOptions.count
                ? MSIMEVoiceLoadVocabulary(hostOptions, rawText, appCategory) : nil;
            MSIMEVoicePostprocessResult *post = MSIMEVoiceRunPostprocess(
                rawText, modelKind, audioMs, @"dictation", appCategory, vocab.terms);
            NSString *processed = post.text.length ? post.text : rawText;
            NSArray<NSString *> *hints = vocab.terms.count
                ? MSIMEVoiceSelectPromptHints(vocab.terms, processed, appCategory)
                : (vocab.promptHints ?: @[]);
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEInputController *ready = weakSelf;
                if (!ready || ready->_voiceRetryGeneration != retryGen) return;
                if (post.dismissed) { failRetry(@"录音仅含静音或语气词"); return; }
                BOOL polishCreationFailed = NO;
                if (MSIMEVoiceShouldUsePolishClient(query, &polishCreationFailed)) {
                    [ready->_voiceOverlay setProcessing:YES];
                    if ([ready->_voiceOverlay respondsToSelector:@selector(setProcessingHint:)]) {
                        [ready->_voiceOverlay setProcessingHint:@"重试完成，正在润色…"];
                    }
                    MSIMEVoicePolishRequest *polishReq = MSIMEVoiceBuildPolishRequest(
                        query, processed, processed, snapshot, hints);
                    ready->_voiceRetryPolishTask = [[ready sharedVoicePolishClient]
                        polishRequest:polishReq
                             slowHint:nil
                              partial:nil
                           completion:^(MSIMEVoicePolishResult *polishResult) {
                               MSIMEInputController *inner = weakSelf;
                               if (!inner || inner->_voiceRetryGeneration != retryGen) return;
                               NSString *finalText = polishResult.text.length ? polishResult.text : processed;
                               [inner cancelArchivedVoiceRetry];
                               [inner deliverRecoveredVoiceText:finalText
                                                    recordingID:recordingID
                                                   polishFailed:polishResult.polishFailed];
                           }];
                    return;
                }
                [ready cancelArchivedVoiceRetry];
                [ready deliverRecoveredVoiceText:processed
                                     recordingID:recordingID
                                    polishFailed:polishCreationFailed];
            });
        });
    };

    dispatch_async([self voicePipelineQueue], ^{
        NSData *pcm = [archive PCMForRecording:recordingID];
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEInputController *controller = weakSelf;
            if (!controller || controller->_voiceRetryGeneration != retryGen) return;
            if (!pcm.length) { failRetry(@"未找到已保存的录音音频"); return; }
            NSNumber *audioMs = @(static_cast<uint64_t>((pcm.length / sizeof(float)) * 1000ULL / 16000ULL));
            NSString *provider = [query[@"asr_provider"] isKindOfClass:NSString.class]
                ? [query[@"asr_provider"] lowercaseString] : @"";
            NSError *reqError = nil;
            if (MSIMEVoiceUsesNativeHTTPProvider(provider, NO)) {
                MSIMEHTTPVoiceRequest *httpReq = [[MSIMEHTTPVoiceRequest alloc] initWithOptions:query error:&reqError];
                if (!httpReq) { failRetry(MSIMEVoiceFailureDetail(reqError)); return; }
                controller->_voiceRetryHTTPRequest = httpReq;
                if (![httpReq recognizePCM:pcm completion:^(NSString *text, NSError *failure) {
                    if (failure) { failRetry(MSIMEVoiceFailureDetail(failure)); return; }
                    finalizeRetryText(text ?: @"", audioMs);
                } error:&reqError]) {
                    failRetry(MSIMEVoiceFailureDetail(reqError));
                }
                return;
            }
            id<MSIMEStreamingVoiceRequest> streamReq = nil;
            if ([provider isEqualToString:@"textstream"]) {
                streamReq = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:query hostOptions:hostOptions error:&reqError];
            } else if ([provider isEqualToString:@"local"]) {
                streamReq = [[MSIMELocalVoiceRequest alloc] initWithOptions:query hostOptions:hostOptions error:&reqError];
            } else if ([provider isEqualToString:@"doubao"]) {
                streamReq = [[MSIMEDoubaoVoiceRequest alloc] initWithOptions:query error:&reqError];
            } else {
                failRetry(@"当前识别方式不支持一键重试，请在听写历史中切换识别服务后重听");
                return;
            }
            if (!streamReq) { failRetry(MSIMEVoiceFailureDetail(reqError)); return; }
            controller->_voiceRetryStreamRequest = streamReq;
            __block BOOL streamFinished = NO;
            if (![streamReq startWithResult:^(NSString *text, BOOL final, NSError *failure) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    MSIMEInputController *inner = weakSelf;
                    if (!inner || inner->_voiceRetryGeneration != retryGen || streamFinished) return;
                    if (failure) { streamFinished = YES; failRetry(MSIMEVoiceFailureDetail(failure)); return; }
                    if (!final) return;
                    streamFinished = YES;
                    finalizeRetryText(text ?: @"", audioMs);
                });
            } error:&reqError]) {
                failRetry(MSIMEVoiceFailureDetail(reqError));
                return;
            }
            // 按 0.1 秒分块把存档 16 kHz PCM 喂回流式识别器。
            dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                constexpr NSUInteger kChunkBytes = 1600 * sizeof(float);
                NSError *feedError = nil;
                BOOL ok = YES;
                for (NSUInteger offset = 0; ok && offset < pcm.length; offset += kChunkBytes) {
                    NSData *chunk = [pcm subdataWithRange:NSMakeRange(offset, MIN(kChunkBytes, pcm.length - offset))];
                    ok = [streamReq appendPCM:chunk error:&feedError];
                }
                if (ok) ok = [streamReq finishWithError:&feedError];
                if (!ok) {
                    dispatch_async(dispatch_get_main_queue(), ^{
                        if (!streamFinished) {
                            streamFinished = YES;
                            failRetry(MSIMEVoiceFailureDetail(feedError));
                        }
                    });
                }
            });
        });
    });
}
- (void)reportVoiceFailure:(MSIMEVoiceFailure)failure { [self reportVoiceFailure:failure detail:nil]; }
- (void)reportVoiceFailure:(MSIMEVoiceFailure)failure detail:(NSString *)detail {
    const BOOL persistent = MSIMEEditionIsShurufa() && MSIMEVoiceRecordingOwner == self;
    NSString *failedRecordingID = [_voiceService.archiveRecordingID copy];
    const NSTimeInterval failedDuration = _voiceService.recordedDuration;
    // 开麦之前的失败（缺权限、缺凭据、缺模型）没有录音，archiveRecordingID 还是上一次激活的，不记。
    if (failedRecordingID.length && MSIMEVoiceFailureFollowsCapture(failure))
        [MSIMEVoiceInputService.archive noteFailure:MSIMEVoiceArchiveFailureReason(failure, detail)
                                          forRecording:failedRecordingID];
    _voicePermissionToken = nil; _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset();
    [self cancelHTTPVoiceInput]; [self cancelDoubaoVoiceInput]; [self cancelLiveVoiceInput];
    if (_voiceService.active) [_voiceService cancelWithError:nil];
    [_voiceAudioMuter restore]; [self voiceCaptureDidEnd];
    const BOOL canOfferRetry = [self shouldShowVoiceRecoveryToast] &&
                               failedRecordingID.length &&
                               failedDuration >= 0.35 &&
                               MSIMEVoiceFailureFollowsCapture(failure) &&
                               [MSIMEVoiceInputService.archive respondsToSelector:@selector(PCMForRecording:)];
    if (canOfferRetry) {
        NSString *title = failure == MSIMEVoiceFailureTimeout
            ? @"语音处理超时 · 录音已保留"
            : failure == MSIMEVoiceFailureCapture
                ? @"录音中断 · 已录音频已保留"
                : @"语音识别失败 · 录音已保留";
        __weak MSIMEInputController *weakSelf = self;
        [[MSIMEVoiceRecoveryToast sharedToast]
            showRetryableFailureTitle:title
                               detail:detail
                         retryHandler:^{ [weakSelf retryArchivedVoiceRecording:failedRecordingID]; }
                       historyHandler:^{ [weakSelf showVoiceHistoryFromRecoveryToast]; }];
        // 恢复卡片已在屏幕底部居中完整承接失败提示与重试按钮，不再在同一位置叠一层纯文本失败条。
        return;
    }
    if (!_activeClient && !persistent) return;
    if (!_voiceOverlay) {
        _voiceOverlay = [MSIMEVoiceWaveOverlay new];
        [_voiceOverlay setPersistentRecording:MSIMEEditionIsShurufa()];
        [_voiceOverlay applyThemePreferences:_voiceThemePreferences ?: @{}];
    }
    [self refreshVoiceOverlayScreen];
    [_voiceOverlay showFailure:failure detail:detail];
}
- (void)cancelDoubaoVoiceInput {
    if (!_doubaoVoiceRequest) return;
    if (_doubaoVoiceMarked && [self ownsDoubaoVoiceFocus])
        [(id<MSIMETextClient>)_doubaoVoiceClient setMarkedText:@"" selectionRange:NSMakeRange(0, 0) replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    [_doubaoVoiceRequest cancel];
    [_doubaoPolishRequest cancel];
    _doubaoPolishRequest = nil;
    _doubaoPolishCreationFailed = NO;
    [self clearVoicePipelineState];
    _doubaoFinalReceived = NO;
    _doubaoVoiceRequest = nil;
    _doubaoVoiceSession = nil;
    _doubaoVoiceClient = nil;
    _doubaoVoiceMarked = NO;
    _doubaoVoiceProcessing = NO;
    [_voiceService cancelWithError:nil];
    [_voiceAudioMuter restore];
    [_voiceOverlay setListening:NO];
    [self voiceCaptureDidEnd];
    [self releaseVoiceRecordingOwnership];
}
- (BOOL)startDoubaoVoiceInputWithOptions:(NSDictionary *)options {
    NSError *error = nil;
    id<MSIMEStreamingVoiceRequest> request = [self makeStreamingVoiceRequest:options error:&error];
    NSDictionary *finished = request && _activeClient && _session ? [_session command:MSIME_FINISH_COMPOSITION error:&error] : nil;
    if (!finished) {
        [request cancel]; [_voiceService cancelWithError:nil]; [_voiceAudioMuter restore]; [_voiceOverlay setListening:NO];
        [self reportVoiceFailure:request ? MSIMEVoiceFailureSession : MSIMEVoiceFailureProvider detail:MSIMEVoiceFailureDetail(error)];
        return NO;
    }
    [self apply:finished];
    [self prepareVoicePipelineForOptions:options];
    _doubaoVoiceRequest = request;
    _doubaoPolishRequest = [self injectedLegacyPolisherForSelector:@selector(makeDoubaoPolishRequest:) options:options];
    BOOL doubaoPolishCreationFailed = NO;
    _useVoicePolishClientForDoubao = !_doubaoPolishRequest &&
        MSIMEVoiceShouldUsePolishClient(options, &doubaoPolishCreationFailed);
    _doubaoPolishCreationFailed = !_doubaoPolishRequest && doubaoPolishCreationFailed;
    _doubaoFinalReceived = NO;
    _doubaoVoiceSession = _session;
    _doubaoVoiceClient = _activeClient;
    _doubaoVoiceGeneration = _voiceGeneration;
    _doubaoVoiceProcessing = NO;
    _doubaoVoiceMarked = NO;
    _doubaoVoiceCommit = MSIMECaptureVoiceCommit(options[@"commit_mode"], _activeClient);
    _doubaoVoiceInline = [options[@"stream"] boolValue] && [_doubaoVoiceCommit.mode isEqual:@"tsf"];
    [self bindVoiceOverlayActions];
    __weak MSIMEInputController *weakSelf = self;
    __weak id<MSIMEStreamingVoiceRequest> weakRequest = request;
    BOOL wiredDualPartialState = NO;
    if ([request respondsToSelector:@selector(setPartialStateHandler:)]) {
        wiredDualPartialState = YES;
        [(MSIMELocalVoiceRequest *)request setPartialStateHandler:^(NSString *finalized, NSString *draft) {
            MSIMEInputController *controller = weakSelf;
            id<MSIMEStreamingVoiceRequest> liveRequest = weakRequest;
            if (!controller || !liveRequest || controller->_doubaoVoiceRequest != liveRequest) return;
            if (![controller ownsDoubaoVoiceFocus] || controller->_doubaoFinalReceived) return;
            if (!controller->_doubaoVoiceInline &&
                [controller->_voiceOverlay respondsToSelector:@selector(setFinalizedTranscript:draft:)]) {
                [controller->_voiceOverlay setFinalizedTranscript:finalized ?: @"" draft:draft ?: @""];
            }
        }];
    }
    if (![request startWithResult:^(NSString *text, BOOL final, NSError *failure) {
        MSIMEInputController *controller = weakSelf;
        id<MSIMEStreamingVoiceRequest> liveRequest = weakRequest;
        if (!controller || !liveRequest || controller->_doubaoVoiceRequest != liveRequest) return;
        if (![controller ownsDoubaoVoiceFocus]) { [controller cancelDoubaoVoiceInput]; return; }
        if (controller->_doubaoFinalReceived) return;
        if (failure) { [controller reportVoiceFailure:MSIMEVoiceFailureProvider detail:MSIMEVoiceFailureDetail(failure)]; return; }
        if (!final) {
            controller->_voicePreviewText = [text copy];
            if (controller->_doubaoVoiceInline && text) {
                [(id<MSIMETextClient>)controller->_doubaoVoiceClient setMarkedText:text selectionRange:NSMakeRange(text.length, 0) replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
                controller->_doubaoVoiceMarked = YES;
                controller->_clientHasMarkedText = YES;
            }
            const BOOL dualRendered = wiredDualPartialState &&
                [controller->_voiceOverlay respondsToSelector:@selector(setFinalizedTranscript:draft:)];
            if (!controller->_doubaoVoiceInline && !dualRendered && text.length <= 65536) {
                [controller->_voiceOverlay setTranscript:text ?: @""];
            }
            return; // Partial text must not consume the runtime's final-only token.
        }
        controller->_doubaoFinalReceived = YES;
        if (!controller->_doubaoVoiceProcessing) {
            controller->_doubaoVoiceProcessing = YES;
            // Final already arrived: stop capture without sending a final packet.
            [controller->_voiceService finishPCMStreamingWithError:nil];
            [controller->_voiceAudioMuter restore];
            [controller voiceCaptureDidEnd];
        }
        if (msime::voice::short_capture(controller->_voiceService.recordedDuration)) {
            [controller cancelDoubaoVoiceInput]; return;
        }
        if ([controller voiceServiceReportsAccidentalSilence]) {
            [controller noteVoiceArchiveDismissed:@"accidental_silence"];
            [controller cancelDoubaoVoiceInput]; return;
        }
        NSDictionary *sessionOptions = controller->_voiceOptionsSnapshot ?: @{};
        MSIMEVoiceContextSnapshot *snapshot = controller->_voiceContextSnapshot;
        const BOOL contextAware = MSIMEVoiceOptionsContextAware(sessionOptions);
        NSString *appCategory = contextAware ? MSIMEVoiceInferAppCategory(snapshot) : @"general";
        const uint64_t asrMs = [controller currentVoiceAsrMillisecondsForRequest:liveRequest];
        NSNumber *audioMs = [controller currentVoiceAudioMillisecondsForRequest:liveRequest];
        NSString *modelKind = [sessionOptions[@"asr_provider"] isKindOfClass:NSString.class] ? sessionOptions[@"asr_provider"] : @"";
        NSString *rawText = text.length ? [text copy] : @"";
        // 主线程仅消费录音开始时后台预加载的内存词表缓存，绝不在 IMK 主线程执行磁盘或 SQLite 扫描。
        NSArray<NSDictionary<NSString *, id> *> *cachedTerms = [controller cachedVoiceVocabularyTerms];
        NSString *processedText = rawText;
        if (rawText.length) {
            MSIMEVoicePostprocessResult *post = MSIMEVoiceRunPostprocess(
                rawText, modelKind, audioMs, @"dictation", appCategory, cachedTerms);
            if (post.dismissed) {
                [controller noteVoiceArchiveDismissed:post.dismissedReason ?: @"dismissed"];
                [controller cancelDoubaoVoiceInput];
                return;
            }
            if (post.text.length) processedText = post.text;
        }
        MSIMEHTTPVoiceRequest *polisher = controller->_doubaoPolishRequest;
        if (processedText.length && polisher) {
            [controller->_voiceOverlay setProcessing:YES];
            if (!controller->_doubaoVoiceInline) [controller->_voiceOverlay setTranscript:processedText];
            if ([polisher polishText:processedText completion:^(NSString *polished, NSError *polishError) {
                const BOOL failed = polisher.polishFailed || polishError != nil || !polished.length;
                NSString *finalText = !polishError && polished.length ? polished : processedText;
                NSDictionary *meta = MSIMEVoiceBuildArchiveMetadata(rawText, asrMs, nil, sessionOptions, appCategory, failed);
                [weakSelf applyDoubaoFinalText:finalText
                                       request:liveRequest
                                  polishFailed:failed
                              replaceSelection:NO
                               archiveMetadata:meta];
            } error:nil]) return;
            NSDictionary *meta = MSIMEVoiceBuildArchiveMetadata(rawText, asrMs, nil, sessionOptions, appCategory, YES);
            [controller applyDoubaoFinalText:processedText request:liveRequest polishFailed:YES replaceSelection:NO archiveMetadata:meta];
            return;
        }
        if (processedText.length && controller->_useVoicePolishClientForDoubao) {
            [controller->_voiceOverlay setProcessing:YES];
            if (!controller->_doubaoVoiceInline) [controller->_voiceOverlay setTranscript:processedText];
            controller->_voicePolishPending = YES;
            NSDictionary *hostOptions = [controller->_doubaoVoiceSession respondsToSelector:@selector(hostOptions)]
                ? [controller->_doubaoVoiceSession.hostOptions copy] : nil;
            // 在后台串行队列按本句文本筛选 Top-K 词表提示，随后切回主线程发起异步润色任务。
            dispatch_async([controller voicePipelineQueue], ^{
                NSArray<NSString *> *hints = cachedTerms.count
                    ? MSIMEVoiceSelectPromptHints(cachedTerms, processedText, appCategory)
                    : (hostOptions.count ? MSIMEVoiceLoadVocabulary(hostOptions, processedText, appCategory).promptHints : @[]);
                dispatch_async(dispatch_get_main_queue(), ^{
                    MSIMEInputController *ready = weakSelf;
                    if (!ready || ready->_doubaoVoiceRequest != liveRequest) return;
                    // 送入 Pre-LLM 确定性清洗后的 processedText，存档仍保留原始 ASR rawText。
                    MSIMEVoicePolishRequest *polishReq = MSIMEVoiceBuildPolishRequest(
                        sessionOptions, processedText, processedText, snapshot, hints);
                    [ready->_voicePolishTask cancel];
                    ready->_voicePolishTask = [[ready sharedVoicePolishClient]
                        polishRequest:polishReq
                             slowHint:^(NSString *message) {
                                 MSIMEInputController *inner = weakSelf;
                                 if (!inner || inner->_doubaoVoiceRequest != liveRequest) return;
                                 if ([inner->_voiceOverlay respondsToSelector:@selector(setProcessingHint:)]) {
                                     [inner->_voiceOverlay setProcessingHint:message];
                                 }
                             }
                              partial:nil
                           completion:^(MSIMEVoicePolishResult *polishResult) {
                               MSIMEInputController *inner = weakSelf;
                               if (!inner || inner->_doubaoVoiceRequest != liveRequest) return;
                               inner->_voicePolishTask = nil;
                               inner->_voicePolishPending = NO;
                               NSString *finalText = polishResult.text.length ? polishResult.text : processedText;
                               const BOOL replaceSel = [polishResult.resolvedAction isEqualToString:@"replace_selection"];
                               NSDictionary *meta = MSIMEVoiceBuildArchiveMetadata(
                                   rawText, asrMs, polishResult, sessionOptions, appCategory, polishResult.polishFailed);
                               [inner applyDoubaoFinalText:finalText
                                                   request:liveRequest
                                              polishFailed:polishResult.polishFailed
                                          replaceSelection:replaceSel
                                           archiveMetadata:meta];
                           }];
                });
            });
            return;
        }
        NSDictionary *meta = rawText.length
            ? MSIMEVoiceBuildArchiveMetadata(rawText, asrMs, nil, sessionOptions, appCategory, controller->_doubaoPolishCreationFailed)
            : nil;
        [controller applyDoubaoFinalText:processedText
                                 request:liveRequest
                            polishFailed:polisher != nil || controller->_doubaoPolishCreationFailed
                        replaceSelection:NO
                         archiveMetadata:meta];
    } error:&error]) { [self reportVoiceFailure:MSIMEVoiceFailureProvider detail:MSIMEVoiceFailureDetail(error)]; return NO; }
    NSString *device = MSIMEVoiceCaptureDeviceUID(NSUserDefaults.standardUserDefaults);
    if (![_voiceService startPCMStreaming:^(NSData *pcm, NSError *failure) {
        id<MSIMEStreamingVoiceRequest> liveRequest = weakRequest;
        if (!liveRequest) return;
        NSError *sendError = failure;
        BOOL sent = !failure && pcm && [liveRequest appendPCM:pcm error:&sendError];
        const float *samples = static_cast<const float *>(pcm.bytes);
        const float level = sent ? msime::voice::input_level(samples, pcm.length / sizeof(float)) : 0;
        // Never stop the capture engine while holding its stream admission lock.
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEInputController *controller = weakSelf;
            if (!controller || controller->_doubaoVoiceRequest != liveRequest) return;
            if (controller->_doubaoFinalReceived) return;
            if (![controller ownsDoubaoVoiceFocus]) { [controller cancelDoubaoVoiceInput]; return; }
            if (!sent) [controller reportVoiceFailure:MSIMEVoiceFailureCapture];
            else if (!controller->_doubaoVoiceProcessing) [controller->_voiceOverlay setInputLevel:level];
        });
    } deviceUID:device error:&error]) { [self reportVoiceFailure:MSIMEVoiceFailureCapture]; return NO; }
    [self voiceCaptureDidStart];
    return YES;
}
- (void)finishDoubaoVoiceInput {
    if (!_doubaoVoiceRequest) return;
    if (_doubaoVoiceProcessing) { [self cancelDoubaoVoiceInput]; return; }
    _doubaoVoiceProcessing = YES;
    NSError *error = nil;
    NSData *tail = [_voiceService finishPCMStreamingWithError:&error];
    if (tail && !error && msime::voice::short_capture(_voiceService.recordedDuration)) { [self cancelDoubaoVoiceInput]; return; }
    if (tail && !error && [self voiceServiceReportsAccidentalSilence]) {
        [self noteVoiceArchiveDismissed:@"accidental_silence"];
        [self cancelDoubaoVoiceInput];
        return;
    }
    [_voiceAudioMuter restore];
    [_voiceOverlay setProcessing:NO];
    [self voiceCaptureDidEnd];
    if (!tail || error || (tail.length && ![_doubaoVoiceRequest appendPCM:tail error:&error]) ||
        ![_doubaoVoiceRequest finishWithError:&error]) [self reportVoiceFailure:MSIMEVoiceFailureProvider detail:MSIMEVoiceFailureDetail(error)];
}
- (MSIMEHTTPVoiceRequest *)makeHTTPVoiceRequest:(NSDictionary *)options error:(NSError **)error {
    return [[MSIMEHTTPVoiceRequest alloc] initWithOptions:options error:error];
}
- (void)cancelHTTPVoiceInput {
    if (!_httpVoiceRequest) return;
    [_httpVoiceRequest cancel];
    _httpVoiceRequest = nil;
    _httpVoiceSession = nil;
    _httpVoiceClient = nil;
    _httpVoiceProcessing = NO;
    [self clearVoicePipelineState];
    [_voiceService cancelWithError:nil];
    [_voiceAudioMuter restore];
    [_voiceOverlay setListening:NO];
    [self voiceCaptureDidEnd];
    [self releaseVoiceRecordingOwnership];
}
- (BOOL)startHTTPVoiceInputWithOptions:(NSDictionary *)options {
    NSError *error = nil;
    MSIMEHTTPVoiceRequest *request = [self makeHTTPVoiceRequest:options error:&error];
    if (!request || !_activeClient || !_session) {
        [_voiceService cancelWithError:nil]; [_voiceAudioMuter restore]; [_voiceOverlay setListening:NO];
        [self reportVoiceFailure:request ? MSIMEVoiceFailureSession : MSIMEVoiceFailureProvider detail:MSIMEVoiceFailureDetail(error)];
        return NO;
    }
    [self prepareVoicePipelineForOptions:options];
    _httpVoiceRequest = request;
    _httpVoiceSession = _session;
    _httpVoiceClient = _activeClient;
    _httpVoiceCommit = MSIMECaptureVoiceCommit(options[@"commit_mode"], _activeClient);
    _httpVoiceGeneration = _voiceGeneration;
    _httpVoiceProcessing = NO;
    __weak MSIMEInputController *weakSelf = self;
    [self bindVoiceOverlayActions];
    __weak MSIMEHTTPVoiceRequest *weakRequest = request;
    request.polishingHandler = ^{
        MSIMEInputController *controller = weakSelf;
        if (!controller || !weakRequest || controller->_httpVoiceRequest != weakRequest || !controller->_httpVoiceProcessing ||
            ![controller voiceFocusMatchesClient:controller->_httpVoiceClient session:controller->_httpVoiceSession generation:controller->_httpVoiceGeneration]) return;
        [controller->_voiceOverlay setProcessing:YES];
    };
    NSString *device = MSIMEVoiceCaptureDeviceUID(NSUserDefaults.standardUserDefaults);
    const NSUInteger sampleLimit = request.sampleLimit;
    if (![_voiceService startPCMRecording:^(AVAudioPCMBuffer *buffer) {
        const float level = MSIMEVoiceInputLevel(buffer);
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEInputController *controller = weakSelf;
            if (!controller || controller->_httpVoiceRequest != request || controller->_httpVoiceProcessing ||
                ![controller voiceFocusMatchesClient:controller->_httpVoiceClient session:controller->_httpVoiceSession generation:controller->_httpVoiceGeneration]) return;
            [controller->_voiceOverlay setInputLevel:level];
            // MSIME-Windows keeps every sample and tells the user when the upload is too large; this host keeps only what the provider takes. Once it has that much, end the recording the way a release does, so the overlay turns to 识别中... and the end cue plays instead of the wave animating over audio that is no longer kept. The physical release that follows must not cancel the recognition.
            if (controller->_voiceService.recordedDuration * 16000.0 >= sampleLimit) {
                controller->_voiceHoldShortcut.reset(); controller->_configuredVoiceShortcut.reset();
                [controller finishHTTPVoiceInput];
            }
        });
    } deviceUID:device failure:^(NSError *failure) {
        (void)failure;
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_httpVoiceRequest != request) return;
        // 采集服务先停机再回报错误，active 为 NO 不代表这次请求已经过期。
        if (![controller voiceIdentityMatchesClient:controller->_httpVoiceClient session:controller->_httpVoiceSession generation:controller->_httpVoiceGeneration]) { [controller cancelHTTPVoiceInput]; return; }
        [controller reportVoiceFailure:MSIMEVoiceFailureCapture];
    } error:&error]) { [self reportVoiceFailure:MSIMEVoiceFailureCapture]; return NO; }
    [self voiceCaptureDidStart];
    return YES;
}
- (void)finishHTTPVoiceInput {
    MSIMEHTTPVoiceRequest *request = _httpVoiceRequest;
    if (!request) return;
    if (_httpVoiceProcessing) { [self cancelHTTPVoiceInput]; return; }
    _httpVoiceProcessing = YES;
    NSError *error = nil;
    NSData *pcm = [_voiceService finishPCMRecordingWithError:&error];
    if (pcm && !error && msime::voice::short_capture(_voiceService.recordedDuration)) { [self cancelHTTPVoiceInput]; return; }
    if (pcm && !error && [self voiceServiceReportsAccidentalSilence]) {
        [self noteVoiceArchiveDismissed:@"accidental_silence"];
        [self cancelHTTPVoiceInput];
        return;
    }
    [_voiceAudioMuter restore];
    [_voiceOverlay setProcessing:NO];
    [self voiceCaptureDidEnd];
    if (!pcm.length || error) { [self reportVoiceFailure:MSIMEVoiceFailureCapture]; return; }
    MSIMEClientSession *session = _httpVoiceSession;
    id client = _httpVoiceClient;
    const uint64_t generation = _httpVoiceGeneration;
    __weak MSIMEInputController *weakSelf = self;
    if (![request recognizePCM:pcm completion:^(NSString *text, NSError *failure) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_httpVoiceRequest != request) return;
        if (![controller voiceFocusMatchesClient:client session:session generation:generation]) { [controller cancelHTTPVoiceInput]; return; }
        if (failure || !text.length) { [controller reportVoiceFailure:failure ? MSIMEVoiceFailureProvider : MSIMEVoiceFailureNoSpeech detail:MSIMEVoiceFailureDetail(failure)]; return; }
        if (!failure && text.length && [controller voiceFocusMatchesClient:client session:session generation:generation]) {
            NSDictionary *sessionOptions = controller->_voiceOptionsSnapshot ?: @{};
            MSIMEVoiceContextSnapshot *snapshot = controller->_voiceContextSnapshot;
            const BOOL contextAware = MSIMEVoiceOptionsContextAware(sessionOptions);
            NSString *appCategory = contextAware ? MSIMEVoiceInferAppCategory(snapshot) : @"general";
            const uint64_t asrMs = [controller currentVoiceAsrMillisecondsForRequest:nil];
            NSNumber *audioMs = [controller currentVoiceAudioMillisecondsForRequest:nil];
            NSArray<NSDictionary<NSString *, id> *> *cachedTerms = [controller cachedVoiceVocabularyTerms];
            MSIMEVoicePostprocessResult *post = MSIMEVoiceRunPostprocess(
                text, @"", audioMs, @"dictation", appCategory, cachedTerms);
            if (post.dismissed) {
                [controller noteVoiceArchiveDismissed:post.dismissedReason ?: @"dismissed"];
                [controller cancelHTTPVoiceInput];
                return;
            }
            NSString *finalText = post.text.length ? post.text : text;
            // Native host session methods are main-thread-only. Recheck the
            // departing focus identity before the runtime's own generation check.
            NSDictionary *result = [session applyVoiceText:finalText generation:generation error:nil];
            if (!result) { [controller reportVoiceFailure:MSIMEVoiceFailureSession]; return; }
            NSDictionary *meta = MSIMEVoiceBuildArchiveMetadata(
                text, asrMs, nil, sessionOptions, appCategory, request.polishFailed);
            [controller applyVoiceResult:result
                                   route:controller->_httpVoiceCommit
                            polishFailed:request.polishFailed
                        replaceSelection:NO
                         archiveMetadata:meta];
        }
        [controller cancelHTTPVoiceInput];
    } error:&error]) [self reportVoiceFailure:MSIMEVoiceFailureProvider detail:MSIMEVoiceFailureDetail(error)];
}
- (BOOL)ownsPersistentVoiceRecording {
    return MSIMEEditionIsShurufa() && MSIMEVoiceRecordingOwner == self && _voiceService.active;
}
- (BOOL)voiceIdentityMatchesClient:(id)client session:(MSIMEClientSession *)session generation:(uint64_t)generation {
    return ((MSIMEEditionIsShurufa() && MSIMEVoiceRecordingOwner == self) || (_activeClient == client && _session == session)) &&
        _voiceGeneration == generation;
}
- (BOOL)voiceFocusMatchesClient:(id)client session:(MSIMEClientSession *)session generation:(uint64_t)generation {
    return [self voiceIdentityMatchesClient:client session:session generation:generation] && _voiceService.active;
}
- (void)suspendVoiceForFocusChange {
    _voicePermissionToken = nil;
    if ([self ownsPersistentVoiceRecording]) {
        [self detachVoiceMarkedText];
        return;
    }
    _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset();
    [self cancelLiveVoiceInput]; [self cancelDoubaoVoiceInput]; [self cancelHTTPVoiceInput];
}
- (BOOL)startVoiceRecordingMonitor:(void (^)(NSEvent *))handler {
    return MSIMERecordingMonitor.start(handler);
}
- (void)detachVoiceMarkedText {
    if (![self ownsPersistentVoiceRecording]) return;
    for (id client in @[_liveVoiceClient ?: NSNull.null, _doubaoVoiceClient ?: NSNull.null]) {
        if (client == NSNull.null) continue;
        const BOOL marked = client == _liveVoiceClient ? _liveVoiceMarked : _doubaoVoiceMarked;
        if (!marked) continue;
        [(id<MSIMETextClient>)client setMarkedText:@"" selectionRange:NSMakeRange(0,0) replacementRange:NSMakeRange(NSNotFound,NSNotFound)];
        if ([client respondsToSelector:@selector(unmarkText)]) [client unmarkText];
    }
    _liveVoiceMarked = NO; _doubaoVoiceMarked = NO; _clientHasMarkedText = NO;
    _liveVoiceInline = NO; _doubaoVoiceInline = NO;
    [_voiceOverlay setTranscript:_voicePreviewText ?: @""];
}
- (void)releaseVoiceRecordingOwnership {
    if (MSIMEVoiceRecordingOwner != self || _voiceService.active) return;
    MSIMERecordingMonitor.stop();
    // 最终投递或取消后丢弃旧手势，不能依赖可能晚到的 KeyUp 清理下一轮状态。
    _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset();
    if (_voicePolishFallbackNotice) [_voiceOverlay showCompletionNotice:_voicePolishFallbackNotice];
    else if (_voiceCopiedResult) [_voiceOverlay showCopyResult:_voiceCopiedResult.boolValue];
    _voiceCopiedResult = nil; _voicePreviewText = nil; _voicePolishFallbackNotice = nil;
    MSIMEVoiceRecordingOwner = nil;
    MSIMERefreshVoiceShortcutTap();
}
- (BOOL)ownsLiveVoiceToken:(id)token {
    return token && token == _liveVoiceToken && ([self ownsPersistentVoiceRecording] ||
        (_activeClient == _liveVoiceClient && _session == _liveVoiceSession)) &&
        _voiceGeneration == _liveVoiceGeneration && _voiceService.active;
}
- (void)cancelLiveVoiceInput {
    if (!_liveVoiceToken) return;
    [_livePolishRequest cancel]; _livePolishRequest = nil;
    _livePolishCreationFailed = NO;
    [self clearVoicePipelineState];
    if (_liveVoiceMarked && [self ownsLiveVoiceToken:_liveVoiceToken])
        [(id<MSIMETextClient>)_liveVoiceClient setMarkedText:@"" selectionRange:NSMakeRange(0, 0) replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    MSIMEDeactivateVoice(_voiceService, _liveVoiceSession, _voiceAudioMuter, _voiceOverlay, _liveVoiceSocket, _liveVoiceGeneration);
    [self voiceCaptureDidEnd];
    _liveVoiceToken = nil; _liveVoiceSession = nil; _liveVoiceClient = nil; _liveVoiceSocket = nil;
    _liveVoiceMarked = NO; _liveVoiceProcessing = NO; _liveVoiceFinalReceived = NO;
    [self releaseVoiceRecordingOwnership];
}

- (MSIMEHTTPVoiceRequest *)makeLiveVoicePolishRequest:(NSDictionary *)options {
    if (!([options[@"polish_enabled"] boolValue] || [options[@"polish_text"] boolValue]) || ![options[@"polish_token"] length]) return nil;
    return [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
}
- (void)expireLiveVoice:(id)token {
    if ([self ownsLiveVoiceToken:token]) {
        // 最终转写已经到达且正在润色时，由润色请求自己的超时预算负责原文回退。
        if (_liveVoiceFinalReceived && (_livePolishRequest || _voicePolishTask || _voicePolishPending)) return;
        [self reportVoiceFailure:MSIMEVoiceFailureTimeout];
    } else if (token && _liveVoiceToken == token) [self cancelLiveVoiceInput];
}
- (id)beginLiveVoiceWithOptions:(NSDictionary *)options socket:(NSString *)socket {
    NSDictionary *finished = _session && _activeClient ? [_session command:MSIME_FINISH_COMPOSITION error:nil] : nil;
    if (!finished) {
        MSIMEDeactivateVoice(_voiceService, _session, _voiceAudioMuter, _voiceOverlay, socket, _voiceGeneration);
        [self reportVoiceFailure:MSIMEVoiceFailureSession];
        return nil;
    }
    [self apply:finished];
    if (!socket.length) [self prepareVoicePipelineForOptions:options];
    _liveVoiceToken = [NSObject new]; _liveVoiceSession = _session; _liveVoiceClient = _activeClient;
    _liveVoiceGeneration = _voiceGeneration; _liveVoiceSocket = [socket copy];
    // 外部 socket 提供商自行持有润色阶段；系统语音路径优先走测试注入夹具或新版独立润色客户端。
    _livePolishRequest = socket.length ? nil : [self injectedLegacyPolisherForSelector:@selector(makeLiveVoicePolishRequest:) options:options];
    BOOL livePolishCreationFailed = NO;
    _useVoicePolishClientForLive = !socket.length && !_livePolishRequest &&
        MSIMEVoiceShouldUsePolishClient(options, &livePolishCreationFailed);
    _livePolishCreationFailed = !socket.length && !_livePolishRequest && livePolishCreationFailed;
    _liveVoiceFinalReceived = NO;
    _liveVoiceCommit = MSIMECaptureVoiceCommit(options[@"commit_mode"], _activeClient);
    _liveVoiceInline = [options[@"stream"] boolValue] && [_liveVoiceCommit.mode isEqual:@"tsf"];
    _liveVoiceMarked = NO; _liveVoiceProcessing = NO;
    [self bindVoiceOverlayActions];
    id token = _liveVoiceToken;
    __weak MSIMEInputController *weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [weakSelf expireLiveVoice:token];
    });
    return token;
}
- (void)bindVoiceOverlayActions {
    if (MSIMEEditionIsShurufa()) { MSIMEVoiceRecordingOwner = self; MSIMERefreshVoiceShortcutTap(); _voiceCopiedResult = nil; _voicePreviewText = nil; _voicePolishFallbackNotice = nil; }
    __weak MSIMEInputController *weakSelf = self;
    id client = _activeClient;
    MSIMEClientSession *session = _session;
    const uint64_t generation = _voiceGeneration;
    id http = _httpVoiceRequest, doubao = _doubaoVoiceRequest, live = _liveVoiceToken;
    _voiceOverlay.actionHandler = ^(BOOL cancel) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || (![controller ownsPersistentVoiceRecording] && (controller->_activeClient != client || controller->_session != session)) ||
            controller->_voiceGeneration != generation || !controller->_voiceService.active ||
            controller->_httpVoiceRequest != http || controller->_doubaoVoiceRequest != doubao ||
            controller->_liveVoiceToken != live || !(http || doubao || live)) return;
        // A subsequent physical hold release must not toggle pending recognition.
        controller->_voiceHoldShortcut.reset(); controller->_configuredVoiceShortcut.reset();
        if (cancel) {
            [controller noteVoiceArchiveCancelled];
            [controller cancelHTTPVoiceInput]; [controller cancelDoubaoVoiceInput]; [controller cancelLiveVoiceInput];
        } else if (controller->_httpVoiceProcessing || controller->_doubaoVoiceProcessing || controller->_liveVoiceProcessing) {
            if (!MSIMEEditionIsShurufa()) [controller->_voiceOverlay dismissProcessing];
        } else if (http) [controller finishHTTPVoiceInput];
        else if (doubao) [controller finishDoubaoVoiceInput];
        else [controller finishLiveVoiceInput];
    };
    if (MSIMEEditionIsShurufa()) {
        [_voiceOverlay setPersistentRecording:YES];
        BOOL available = [self startVoiceRecordingMonitor:^(NSEvent *event) {
            MSIMEInputController *owner = weakSelf;
            if (![owner ownsPersistentVoiceRecording] || owner->_voiceGeneration != generation) return;
            [owner handleConfiguredVoiceShortcut:event];
        }];
        [_voiceOverlay setShortcutMonitoringAvailable:available];
    }
}
- (void)applyLiveVoiceFinalText:(NSString *)text token:(id)token polishFailed:(BOOL)polishFailed {
    [self applyLiveVoiceFinalText:text token:token polishFailed:polishFailed replaceSelection:NO archiveMetadata:nil];
}
- (void)applyLiveVoiceFinalText:(NSString *)text
                          token:(id)token
                   polishFailed:(BOOL)polishFailed
               replaceSelection:(BOOL)replaceSelection
                archiveMetadata:(NSDictionary<NSString *, id> *)archiveMetadata {
    if (!token || token != _liveVoiceToken) return;
    if ([self ownsLiveVoiceToken:token]) {
        if (!text.length) { [self reportVoiceFailure:MSIMEVoiceFailureNoSpeech]; return; }
        NSDictionary *result = text.length ? [_liveVoiceSession applyVoiceText:text generation:_liveVoiceGeneration error:nil] : nil;
        if (!result) { [self reportVoiceFailure:MSIMEVoiceFailureSession]; return; }
        if (result) {
            if (![self ownsPersistentVoiceRecording]) _liveVoiceMarked = NO;
            [self applyVoiceResult:result
                             route:_liveVoiceCommit
                      polishFailed:polishFailed
                  replaceSelection:replaceSelection
                   archiveMetadata:archiveMetadata];
        }
    }
    [self cancelLiveVoiceInput];
}
- (void)applyLiveVoiceText:(NSString *)text final:(BOOL)final token:(id)token {
    if (!token || token != _liveVoiceToken) return;
    if (![self ownsLiveVoiceToken:token]) { [self cancelLiveVoiceInput]; return; }
    if (_liveVoiceFinalReceived) return;
    if (!final) {
        _voicePreviewText = [text copy];
        if (_liveVoiceInline && text.length <= 65536) {
            [(id<MSIMETextClient>)_liveVoiceClient setMarkedText:text ?: @"" selectionRange:NSMakeRange(text.length, 0) replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
            _liveVoiceMarked = YES;
            _clientHasMarkedText = YES;
        }
        if (!_liveVoiceInline && text.length <= 65536) [_voiceOverlay setTranscript:text ?: @""];
        return;
    }
    _liveVoiceFinalReceived = YES;
    if (!_liveVoiceSocket.length && !_liveVoiceProcessing) {
        [self finishLiveVoiceInput];
        if (![self ownsLiveVoiceToken:token]) return;
    }
    if (!_liveVoiceSocket.length && [self voiceServiceReportsAccidentalSilence]) {
        [self noteVoiceArchiveDismissed:@"accidental_silence"];
        [self cancelLiveVoiceInput];
        return;
    }
    NSDictionary *sessionOptions = _voiceOptionsSnapshot ?: @{};
    MSIMEVoiceContextSnapshot *snapshot = _voiceContextSnapshot;
    const BOOL contextAware = MSIMEVoiceOptionsContextAware(sessionOptions);
    NSString *appCategory = contextAware ? MSIMEVoiceInferAppCategory(snapshot) : @"general";
    const uint64_t asrMs = [self currentVoiceAsrMillisecondsForRequest:nil];
    NSNumber *audioMs = [self currentVoiceAudioMillisecondsForRequest:nil];
    NSString *rawText = text.length ? [text copy] : @"";
    NSString *processedText = rawText;
    NSArray<NSDictionary<NSString *, id> *> *cachedTerms = [self cachedVoiceVocabularyTerms];
    if (!_liveVoiceSocket.length && rawText.length) {
        MSIMEVoicePostprocessResult *post = MSIMEVoiceRunPostprocess(
            rawText, @"system", audioMs, @"dictation", appCategory, cachedTerms);
        if (post.dismissed) {
            [self noteVoiceArchiveDismissed:post.dismissedReason ?: @"dismissed"];
            [self cancelLiveVoiceInput];
            return;
        }
        if (post.text.length) processedText = post.text;
    }
    if (processedText.length && _livePolishRequest) {
        if (!_liveVoiceProcessing) [self finishLiveVoiceInput];
        [_voiceService stopTranscription];
        [_voiceOverlay setProcessing:YES];
        if (!_liveVoiceInline) [_voiceOverlay setTranscript:processedText];
        NSString *original = [processedText copy];
        __weak MSIMEInputController *weakSelf = self;
        MSIMEHTTPVoiceRequest *polisher = _livePolishRequest;
        if ([polisher polishText:original completion:^(NSString *polished, NSError *error) {
            const BOOL failed = polisher.polishFailed || error != nil || !polished.length;
            NSString *finalText = !error && polished.length ? polished : original;
            NSDictionary *meta = MSIMEVoiceBuildArchiveMetadata(rawText, asrMs, nil, sessionOptions, appCategory, failed);
            [weakSelf applyLiveVoiceFinalText:finalText
                                        token:token
                                 polishFailed:failed
                             replaceSelection:NO
                              archiveMetadata:meta];
        } error:nil]) return;
    }
    if (processedText.length && _useVoicePolishClientForLive) {
        if (!_liveVoiceProcessing) [self finishLiveVoiceInput];
        [_voiceService stopTranscription];
        [_voiceOverlay setProcessing:YES];
        if (!_liveVoiceInline) [_voiceOverlay setTranscript:processedText];
        _voicePolishPending = YES;
        NSDictionary *hostOptions = [_liveVoiceSession respondsToSelector:@selector(hostOptions)]
            ? [_liveVoiceSession.hostOptions copy] : nil;
        __weak MSIMEInputController *weakSelf = self;
        dispatch_async([self voicePipelineQueue], ^{
            NSArray<NSString *> *hints = cachedTerms.count
                ? MSIMEVoiceSelectPromptHints(cachedTerms, processedText, appCategory)
                : (hostOptions.count ? MSIMEVoiceLoadVocabulary(hostOptions, processedText, appCategory).promptHints : @[]);
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEInputController *ready = weakSelf;
                if (!ready || ready->_liveVoiceToken != token) return;
                MSIMEVoicePolishRequest *polishReq = MSIMEVoiceBuildPolishRequest(
                    sessionOptions, processedText, processedText, snapshot, hints);
                [ready->_voicePolishTask cancel];
                ready->_voicePolishTask = [[ready sharedVoicePolishClient]
                    polishRequest:polishReq
                         slowHint:^(NSString *message) {
                             MSIMEInputController *inner = weakSelf;
                             if (!inner || inner->_liveVoiceToken != token) return;
                             if ([inner->_voiceOverlay respondsToSelector:@selector(setProcessingHint:)]) {
                                 [inner->_voiceOverlay setProcessingHint:message];
                             }
                         }
                          partial:nil
                       completion:^(MSIMEVoicePolishResult *polishResult) {
                           MSIMEInputController *inner = weakSelf;
                           if (!inner || inner->_liveVoiceToken != token) return;
                           inner->_voicePolishTask = nil;
                           inner->_voicePolishPending = NO;
                           NSString *finalText = polishResult.text.length ? polishResult.text : processedText;
                           const BOOL replaceSel = [polishResult.resolvedAction isEqualToString:@"replace_selection"];
                           NSDictionary *meta = MSIMEVoiceBuildArchiveMetadata(
                               rawText, asrMs, polishResult, sessionOptions, appCategory, polishResult.polishFailed);
                           [inner applyLiveVoiceFinalText:finalText
                                                    token:token
                                             polishFailed:polishResult.polishFailed
                                         replaceSelection:replaceSel
                                          archiveMetadata:meta];
                       }];
            });
        });
        return;
    }
    NSDictionary *meta = (!_liveVoiceSocket.length && rawText.length)
        ? MSIMEVoiceBuildArchiveMetadata(rawText, asrMs, nil, sessionOptions, appCategory, _livePolishCreationFailed)
        : nil;
    [self applyLiveVoiceFinalText:processedText
                            token:token
                     polishFailed:_livePolishRequest != nil || _livePolishCreationFailed
                 replaceSelection:NO
                  archiveMetadata:meta];
}
- (void)applyLiveVoicePhase:(NSUInteger)phase token:(id)token {
    if (![self ownsLiveVoiceToken:token]) return;
    if (phase == 0 && !_liveVoiceProcessing) [self voiceCaptureDidStart];
    else if (phase == 1 || phase == 2) {
        _liveVoiceProcessing = YES;
        [_voiceAudioMuter restore]; [_voiceOverlay setProcessing:phase == 2];
        [self voiceCaptureDidEnd];
    }
}
- (void)finishLiveVoiceInput {
    if (!_liveVoiceToken) return;
    if (_liveVoiceProcessing) { [self cancelLiveVoiceInput]; return; }
    _liveVoiceProcessing = YES;
    // endAudio is sent by stopMicrophoneCapture; leave Speech alive for its final.
    [_voiceService stopMicrophoneCapture];
    if (!_liveVoiceSocket.length && msime::voice::short_capture(_voiceService.recordedDuration)) { [self cancelLiveVoiceInput]; return; }
    if (!_liveVoiceSocket.length && [self voiceServiceReportsAccidentalSilence]) {
        [self noteVoiceArchiveDismissed:@"accidental_silence"];
        [self cancelLiveVoiceInput];
        return;
    }
    [_voiceAudioMuter restore]; [_voiceOverlay setProcessing:NO];
    [self voiceCaptureDidEnd];
    if (_liveVoiceSocket.length) {
        NSString *socket = _liveVoiceSocket; MSIMEClientSession *session = _liveVoiceSession;
        uint64_t generation = _liveVoiceGeneration;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{ [session voiceProviderStopSocket:socket generation:generation error:nil]; });
    }
    id token = _liveVoiceToken;
    __weak MSIMEInputController *weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [weakSelf expireLiveVoice:token];
    });
}
- (void)finishVoiceInputForDisable {
    _voicePermissionToken = nil;
    // Windows RefreshKeyboardHook stops capture on disable without discarding
    // its final result. Repeated preference loads must not act as a second stop.
    _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset();
    if (_httpVoiceRequest && !_httpVoiceProcessing) [self finishHTTPVoiceInput];
    if (_doubaoVoiceRequest && !_doubaoVoiceProcessing) [self finishDoubaoVoiceInput];
    if (_liveVoiceToken && !_liveVoiceProcessing) [self finishLiveVoiceInput];
}
- (void)requestVoicePermissionForSpeech:(BOOL)speech resume:(BOOL)resume {
    id token = [NSObject new], client = _activeClient;
    MSIMEClientSession *session = _session;
    __weak MSIMEVoiceInputService *service = _voiceService;
    const uint64_t generation = _voiceGeneration;
    NSString *provider = [NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoiceASRProvider"] ?: @"";
    _voicePermissionToken = token;
    __weak MSIMEInputController *weakSelf = self;
    void (^completion)(BOOL) = ^(BOOL granted) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_voicePermissionToken != token) return;
        // Consume before resuming: Speech may chain a microphone request, and
        // duplicate callbacks must not consume that request or toggle capture.
        controller->_voicePermissionToken = nil;
        if (controller->_activeClient != client || controller->_session != session ||
            controller->_voiceService != service || controller->_voiceGeneration != generation ||
            ![provider isEqual:([NSUserDefaults.standardUserDefaults stringForKey:@"MSIMEClientVoiceASRProvider"] ?: @"")]) return;
        if (!granted) { [controller reportVoiceFailure:speech ? MSIMEVoiceFailureSpeechPermission : MSIMEVoiceFailureMicrophonePermission]; return; }
        if (!resume) return;
        [controller toggleVoiceInput:nil];
    };
    if (speech) [_voiceService requestSpeechPermission:completion];
    else [_voiceService requestMicrophonePermission:completion];
}
- (void)toggleVoiceInput:(id)sender {
    (void)sender;
    if (MSIMEEditionIsShurufa() && MSIMEVoiceRecordingOwner && MSIMEVoiceRecordingOwner != self) {
        [MSIMEVoiceRecordingOwner toggleVoiceInput:nil]; return;
    }
    if ([self ownsPersistentVoiceRecording]) {
        if (_httpVoiceRequest && !_httpVoiceProcessing) [self finishHTTPVoiceInput];
        else if (_doubaoVoiceRequest && !_doubaoVoiceProcessing) [self finishDoubaoVoiceInput];
        else if (_liveVoiceToken && !_liveVoiceProcessing) [self finishLiveVoiceInput];
        else if ([_voiceOverlay respondsToSelector:@selector(setProcessingHint:)]) {
            [_voiceOverlay setProcessingHint:@"上一条仍在处理中（按 Esc 可取消）"];
        }
        return;
    }
    if (_voiceRetryInFlight) [self cancelArchivedVoiceRetry];
    if (_voicePermissionToken) { _voicePermissionToken = nil; return; }
    // All menu, toolbar, local/global shortcut and permission callbacks converge
    // here. Recheck after asynchronous permission delivery, before any capture.
    if (!MSIMEVoiceInputEnabled(NSUserDefaults.standardUserDefaults)) {
        [self finishVoiceInputForDisable];
        return;
    }
    if (!_activeClient) return;
    if (!_session) [self prepareSession];
    if (!_session) { [self reportVoiceFailure:MSIMEVoiceFailureSession]; return; }
    if (!_voiceService) _voiceService = [[MSIMEVoiceInputService alloc] init];
    [self bindVoiceServiceResilienceHandlers];
    if (!_voiceCuePlayer) _voiceCuePlayer = [[MSIMEVoiceCuePlayer alloc] init];
    if (!_voiceAudioMuter) _voiceAudioMuter = [[MSIMEVoiceAudioMuter alloc] init];
    if (!_voiceOverlay) {
        _voiceOverlay = [[MSIMEVoiceWaveOverlay alloc] init];
        [_voiceOverlay setPersistentRecording:MSIMEEditionIsShurufa()];
        [_voiceOverlay applyThemePreferences:_voiceThemePreferences ?: @{}];
    }
    [self refreshVoiceOverlayScreen];
    if (_httpVoiceRequest) { [self finishHTTPVoiceInput]; return; }
    if (_doubaoVoiceRequest) { [self finishDoubaoVoiceInput]; return; }
    if (_liveVoiceToken) { [self finishLiveVoiceInput]; return; }
    if (_voiceService.active) { [_voiceService cancelWithError:nil]; [_voiceAudioMuter restore]; [_voiceOverlay setListening:NO]; return; }
    // Checked before any permission prompt or capture, where MSIME-Windows StartRecording checks it: without a token the recording could only fail after the user had spoken.
    NSUserDefaults *voiceDefaults = NSUserDefaults.standardUserDefaults;
    if (MSIMEVoiceASRTokenMissing([voiceDefaults stringForKey:@"MSIMEClientVoiceASRProvider"],
                                  [voiceDefaults stringForKey:@"MSIMEClientVoiceASRToken"], MSIMEVoiceProviderSocket() != nil)) {
        [self reportVoiceFailure:MSIMEVoiceFailureMissingToken];
        return;
    }
    if (MSIMEVoiceTextStreamMissing([voiceDefaults stringForKey:@"MSIMEClientVoiceASRProvider"], MSIMEVoiceProviderSocket() != nil, MSIMETextStreamReady())) {
        [self reportVoiceFailure:MSIMEVoiceFailureMissingLocalModel];
        return;
    }
    if (MSIMEVoiceLocalModelMissing(MSIMEVoiceEffectiveASRProvider([voiceDefaults stringForKey:@"MSIMEClientVoiceASRProvider"]), MSIMEVoiceProviderSocket() != nil,
                                    MSIMELocalVoiceModelDirectory([voiceDefaults stringForKey:@"MSIMEClientVoiceASRModelPath"]))) {
        [self reportVoiceFailure:MSIMEVoiceFailureMissingLocalModel];
        return;
    }
    __weak MSIMEInputController *weakSelf = self;
    void (^start)(void) = ^{
        MSIMEInputController *controller = weakSelf;
        if (!controller || !controller->_session) return;
        NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
        // 存着别的平台的录音后端时不再拒绝录音：MSIMEVoiceCaptureDeviceUID 把它当作系统默认设备。
        NSError *error = nil;
        id client = controller->_activeClient;
        controller->_voiceService.archiveApp = [client respondsToSelector:@selector(bundleIdentifier)] ? [client bundleIdentifier] : nil;
        controller->_voiceService.archiveProvider = MSIMEVoiceEffectiveASRProvider([defaults stringForKey:@"MSIMEClientVoiceASRProvider"]);
        if (![controller->_voiceService startWithSession:controller->_session generation:&controller->_voiceGeneration error:&error]) { [controller reportVoiceFailure:MSIMEVoiceFailureSession]; return; }
        [controller->_voiceOverlay setListening:YES];
        NSString *language = [[NSUserDefaults standardUserDefaults] stringForKey:@"MSIMEClientVoiceLanguage"] ?: @"zh-CN";
        NSString *socket = MSIMEVoiceProviderSocket();
        NSDictionary *query = MSIMEVoiceRecognitionQuery(defaults, controller->_voiceGeneration, socket.length > 0);
        if (socket.length) {
            id token = [controller beginLiveVoiceWithOptions:query socket:socket];
            if (!token) return;
            MSIMEClientSession *session = controller->_liveVoiceSession;
            dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                NSError *providerError = nil;
                [session voiceProviderStream:query socket:socket update:^(NSString *text, BOOL final) {
                    dispatch_async(dispatch_get_main_queue(), ^{
                        [weakSelf applyLiveVoiceText:text final:final token:token];
                    });
                } phase:^(NSUInteger phase) {
                    dispatch_async(dispatch_get_main_queue(), ^{ [weakSelf applyLiveVoicePhase:phase token:token]; });
                } error:&providerError];
                dispatch_async(dispatch_get_main_queue(), ^{
                    MSIMEInputController *live = weakSelf;
                    if ([live ownsLiveVoiceToken:token]) [live reportVoiceFailure:MSIMEVoiceFailureProvider];
                    else if (live && live->_liveVoiceToken == token) [live cancelLiveVoiceInput];
                });
            });
            return;
        }
        if ([controller usesNativeHTTPVoice]) { [controller startHTTPVoiceInputWithOptions:query]; return; }
        if ([controller usesNativeDoubaoVoice]) { [controller startDoubaoVoiceInputWithOptions:query]; return; }
        id token = [controller beginLiveVoiceWithOptions:query socket:nil];
        if (!token) return;
        if (![controller->_voiceService startTranscriptionWithLanguage:language textHandler:^(NSString *text, BOOL final) {
            [weakSelf applyLiveVoiceText:text final:final token:token];
        } error:&error]) { [controller reportVoiceFailure:MSIMEVoiceFailureProvider]; return; }
        NSString *deviceUID = MSIMEVoiceCaptureDeviceUID(NSUserDefaults.standardUserDefaults);
        if (![controller->_voiceService startMicrophoneCapture:^(AVAudioPCMBuffer *buffer) {
            const float level = MSIMEVoiceInputLevel(buffer);
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEInputController *liveController = weakSelf;
                if ([liveController ownsLiveVoiceToken:token] && !liveController->_liveVoiceProcessing)
                    [liveController->_voiceOverlay setInputLevel:level];
            });
        } deviceUID:deviceUID error:&error]) { [controller reportVoiceFailure:MSIMEVoiceFailureCapture]; return; }
        [controller voiceCaptureDidStart];
    };
    // A permission sheet can outlive the physical hold. Require a fresh hold
    // after authorization instead of starting capture after the key was released.
    const BOOL resumeAfterPermission = !_voiceHoldStarting;
    if (![self usesNativeHTTPVoice] && ![self usesNativeDoubaoVoice] && !MSIMEVoiceProviderSocket() &&
        _voiceService.speechAuthorizationStatus != SFSpeechRecognizerAuthorizationStatusAuthorized) {
        [self requestVoicePermissionForSpeech:YES resume:resumeAfterPermission];
        return;
    }
    if (_voiceService.microphoneAuthorizationStatus != AVAuthorizationStatusAuthorized) {
        [self requestVoicePermissionForSpeech:NO resume:resumeAfterPermission];
        return;
    }
    start();
}
- (void)openWebsite:(id)sender { (void)sender; if (MSIMEEditionOffersUpstreamLinks()) [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://msime.app/"]]; }
- (void)showHelp:(id)sender { (void)sender; [[MSIMESupportWindowController sharedController] showPage:MSIMESupportPageHelp]; }
- (void)showAbout:(id)sender {
    (void)sender;
    [[MSIMEPreferencesWindowController sharedController] showAndActivateWithPageIdentifier:@"about"];
}
- (void)showFeedback:(id)sender { (void)sender; [[MSIMESupportWindowController sharedController] showPage:MSIMESupportPageFeedback]; }
- (void)openCharacterPalette:(id)sender {
    (void)sender;
    if (_session && _activeClient) {
        NSDictionary *finished = [_session command:MSIME_FINISH_COMPOSITION error:nil];
        if (!finished) return;
        [self apply:finished];
    }
    [self showSystemCharacterPalette];
}
- (void)showAppearance:(id)sender {
    (void)sender;
    [[MSIMEPreferencesWindowController sharedController] showAndActivateWithPageIdentifier:MSIMEEditionIsShurufa() ? @"input" : @"appearance"];
}
- (void)showDictionary:(id)sender {
    (void)sender;
    if (!_session) [self prepareSession];
    if (!_session) return;
    _dictionaryWindow = [[MSIMEDictionaryWindowController alloc] initWithOptions:_session.hostOptions];
    [_dictionaryWindow showWindow:nil];
    MSIMEPresentWindow(_dictionaryWindow.window);
}
- (void)prepareDictionary:(id)sender {
    (void)sender;
    if (_session && _activeClient) {
        if (![self commitManagedEnglishRaw]) return;
        // 会话随后就关掉，和词库维护一样按失焦上屏处理，不出声不画。
        if (![self dispatchCommand:MSIME_FINISH_ON_FOCUS_LOSS]) return;
        if ([_session respondsToSelector:@selector(setFocusedSnapshot:error:)]) [_session setFocusedSnapshot:NO error:nil];
        else [_session setFocused:NO error:nil];
        [_session closeWithError:nil];
        _session = nil;
        [_preferencesTimer invalidate];
        _preferencesTimer = nil;
    }
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.canChooseFiles = NO; panel.canChooseDirectories = YES; panel.allowsMultipleSelection = NO;
    [panel beginWithCompletionHandler:^(NSModalResponse response) {
        if (response != NSModalResponseOK || !panel.URL) return;
        NSURL *state = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager);
        [MSIMEDictionaryRuntime prepareResourcesDirectory:panel.URL.path stateRoot:state.path completion:^(NSDictionary *options, NSError *error) {
            if (!options) { NSAlert *alert = [NSAlert new]; alert.messageText = @"词库准备失败"; alert.informativeText = error.localizedDescription ?: @"无法准备词库"; [alert runModal]; return; }
            NSData *data = [NSJSONSerialization dataWithJSONObject:options options:0 error:nil];
            NSURL *target = [state URLByAppendingPathComponent:@"runtime-options.json"];
            [[NSFileManager defaultManager] createDirectoryAtURL:state withIntermediateDirectories:YES attributes:nil error:nil];
            [data writeToURL:target options:NSDataWritingAtomic error:nil];
        }];
    }];
}

// Every controller that has a session open, whether or not it is the one typing: IMK keeps a controller per text input client, and each holds the dictionary lock through its own session.
+ (NSHashTable<MSIMEInputController *> *)dictionarySessionHolders {
    static NSHashTable<MSIMEInputController *> *holders;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        holders = [NSHashTable weakObjectsHashTable];
        // Registered once, on the class, so one notification releases every controller exactly once. The distributed center holds notifications for a background process unless told to deliver them at once, and IMK never becomes the active application.
        [NSDistributedNotificationCenter.defaultCenter addObserver:self
            selector:@selector(dictionaryMaintenanceWillBegin:)
            name:MSIMEDictionaryMaintenanceWillBeginNotification object:nil
            suspensionBehavior:NSNotificationSuspensionBehaviorDeliverImmediately];
        // The native dictionary window runs in this process and posts locally.
        [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(dictionaryMaintenanceWillBegin:)
            name:MSIMEDictionaryMaintenanceWillBeginNotification object:nil];
    });
    return holders;
}
+ (void)holdDictionarySession:(MSIMEInputController *)controller {
    controller->_dictionarySessionLastUse = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
    [[self dictionarySessionHolders] addObject:controller];
}
// IMK 会保留多个客户端控制器；闲置会话继续持有词库文件和目录句柄，不能随客户端数量无限增长。
static constexpr NSUInteger MSIMEIdleDictionarySessionLimit = 4;
+ (NSUInteger)releaseIdleDictionarySessionsKeeping:(NSUInteger)limit {
    NSUInteger kept = 0;
    NSUInteger released = 0;
    NSArray<MSIMEInputController *> *holders = [[self dictionarySessionHolders].allObjects
        sortedArrayUsingComparator:^NSComparisonResult(MSIMEInputController *left, MSIMEInputController *right) {
            if (left->_dictionarySessionLastUse == right->_dictionarySessionLastUse) return NSOrderedSame;
            return left->_dictionarySessionLastUse > right->_dictionarySessionLastUse ? NSOrderedAscending : NSOrderedDescending;
        }];
    for (MSIMEInputController *controller in holders) {
        if (controller->_activeClient || !controller->_session || [controller ownsPersistentVoiceRecording]) continue;
        if (kept++ < limit) continue;
        [controller releaseDictionarySession];
        ++released;
    }
    return released;
}
+ (void)dictionaryMaintenanceWillBegin:(NSNotification *)notification {
    (void)notification;
    [self releaseQuiescedDictionarySessions];
}
+ (void)releaseQuiescedDictionarySessions {
    for (MSIMEInputController *controller in [self dictionarySessionHolders].allObjects)
        [controller releaseDictionarySessionIfQuiesced];
}
// 设置窗口持有维护租约时关闭会话并释放词库锁；活动组字先上屏，下一次按键通过 prepareSession 重建。
- (void)releaseDictionarySessionIfQuiesced {
    if (!_session || !MSIMEDictionaryQuiesced(_session.hostOptions)) return;
    [self releaseDictionarySession];
}
- (void)releaseDictionarySession {
    if (!_session) return;
    [self cancelLiveVoiceInput];
    [self cancelDoubaoVoiceInput];
    [self cancelHTTPVoiceInput];
    MSIMEDeactivateVoice(_voiceService, _session, _voiceAudioMuter, _voiceOverlay,
        MSIMEVoiceProviderSocket(), _voiceGeneration);
    [self cancelCandidateTranslations];
    [self cancelCloudCandidates];
    [self resetModifierTap];
    [self abandonCapsLockTap];
    [self resetPreferenceLoadState];
    if (_activeClient) {
        if (![self commitManagedEnglishRaw]) return;
        // 关闭活动会话时按失焦上屏处理，不出声不画。
        if (![self dispatchCommand:MSIME_FINISH_ON_FOCUS_LOSS]) {
            MSIMEApplyTransition(@{@"view": @{@"editing_text": @"", @"preedit": @"", @"caret_position": @0}}, (id<MSIMETextClient>)_activeClient);
        }
    }
    [self discardGlossSensePage];
    [self discardExpandedCandidates];
    // 普通英文借用的状态由最新授权决定，不能作为用户主动选中的专用英文恢复。
    _resumeDedicatedEnglish = !_managedEnglishComposition && _viewState.dedicatedEnglish;
    _managedEnglishComposition = NO;
    if ([_session respondsToSelector:@selector(setFocusedSnapshot:error:)]) [_session setFocusedSnapshot:NO error:nil];
    else [_session setFocused:NO error:nil];
    [_session closeWithError:nil];
    _session = nil;
    _focusPending = YES;
    [_candidateMotion stop];
    [_panel orderOut:nil];
    [[MSIMEInputController dictionarySessionHolders] removeObject:self];
}

- (void)activateServer:(id)sender {
    ++_spacingInputSequence;
    if (_activeClient && _activeClient != sender && ![self commitManagedEnglishRaw]) return;
    [MSIMEInputSessionRecoveryPanel dismissForDelegate:nil];
    // A newly activated IME session may target a different document/client.
    // Never carry host-owned closings across that boundary - including one this host still owed
    // the previous document, which cannot be written into this one.
    _pendingPairedClosing = nil;
    _pairedPunctuation.clear();
    _englishPunctuation = {};
    [self clearSmartPunctuationSpaceConversion];
    [self clearSmartPunctuationSpaceRevert];
    [_voiceOverlay dismissFailure];
    if (_activeClient && _activeClient != sender) [self suspendVoiceForFocusChange];
    else if (![self ownsPersistentVoiceRecording]) { _voicePermissionToken = nil; _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset(); }
    [self cancelCandidateTranslations];
    [self cancelCloudCandidates];
    [self resetCandidateAnchor];
    [self claimCandidatePanel];
    [self resetModifierTap];
    [self abandonCapsLockTap];
    [super activateServer:sender];
    [self ensureAppearance];
    msime_macos_diagnostic_write("focus_in");
    if (_activeClient && _activeClient != sender) [self dispatchFocus:NO];
    [self applySharedInputPreferencesBeforeFirstUse];
    [_appearance activateInputModeForApplication:[sender respondsToSelector:@selector(bundleIdentifier)] ? [sender bundleIdentifier] : nil];
    // 系统可能仍在从强制 ASCII 的应用恢复输入源；离开激活调用栈后再核对实际模式，避免重复选择把它退回 U.S.（#8993）。
    [self scheduleSystemInputModeSyncForClient:sender];
    _capsLock = !MSIMEEditionIsShurufa() && ([NSEvent modifierFlags] & NSEventModifierFlagCapsLock) != 0;
    [self syncCapsLock];
    _activeClient = sender;
    MSIMEFocusedController = self;
    [self resetPreferenceLoadState];
    [self ensureAppearance];
    _focusPending = _appearance.englishMode;
    if (!_appearance.englishMode || _englishOnlineCandidatesEnabled) [self prepareSession];
    else [self startPreferencesMonitoring];
    [self claimBackgroundMusic];
    NSNotificationCenter *workspaceCenter = NSWorkspace.sharedWorkspace.notificationCenter;
    [workspaceCenter removeObserver:self name:NSWorkspaceActiveSpaceDidChangeNotification object:nil];
    [workspaceCenter addObserver:self selector:@selector(typingEffectSpaceChanged:) name:NSWorkspaceActiveSpaceDidChangeNotification object:nil];
    [self refreshTypingEffectFullscreen];
    [self requestCloudCandidatesConsentIfNeeded];
    MSIMERefreshVoiceShortcutTap();
}

- (void)resolveCloudCandidatesConsentWithOptions:(NSDictionary *)options {
    if (![options isKindOfClass:NSDictionary.class]) return;
    id directory = options[@"preferences_directory"];
    id userData = options[@"user_data"];
    if (![directory isKindOfClass:NSString.class] || ![directory isAbsolutePath]) return;
    [_appearance resolveCloudCandidatesConsentWithPreferencesDirectory:directory
        userDataDirectory:[userData isKindOfClass:NSString.class] && [userData isAbsolutePath] ? userData : nil];
}

// One consent prompt per process at a time, however many controllers IMK creates.
static BOOL MSIMECloudConsentPrompting = NO;

/// Ask once, on a fresh profile, before the first cloud candidate query is sent.
///
/// The Windows installer asks this on its 联网功能 page. macOS has no installer step that every user passes through, and the IME types without the settings app ever being opened, so the IME process asks itself. Until an answer arrives nothing is sent; a prompt closed without an answer is asked again on the next activation.
- (void)requestCloudCandidatesConsentIfNeeded {
    if (MSIMEEditionIsShurufa()) return;
    if (!_appearance || _appearance.cloudCandidatesAnswered || MSIMECloudConsentPrompting) return;
    MSIMECloudConsentPrompting = YES;
    MSIMEAppearancePreferences *appearance = _appearance;
    __weak MSIMEInputController *weakSelf = self;
    // Off the activation path, so the client's first keystrokes are not held behind the prompt.
    dispatch_async(dispatch_get_main_queue(), ^{
        MSIMEInputController *controller = weakSelf;
        if (!controller || appearance.cloudCandidatesAnswered) { MSIMECloudConsentPrompting = NO; return; }
        [controller presentCloudConsent:^(NSNumber *enabled) {
            MSIMECloudConsentPrompting = NO;
            if (enabled) [appearance answerCloudCandidates:enabled.boolValue];
        }];
    });
}

static NSString *const MSIMECloudConsentTitle = @"联网功能";
static NSString *const MSIMECloudConsentMessage =
    @"拼音切分、候选排序和词频学习全部在本机完成，不联网。\n\n"
    @"启用云候选：输入过程中把正在输入的拼写发送给 Google 的 input-tools 服务（inputtools.google.com），换回一条额外候选。已上屏的文本、词库内容和学习到的词频都不会发送。\n\n"
    @"不启用时，输入内容不会离开这台 Mac：AI 联想和语音输入要你自己填入 API token，候选翻译要你在设置里选择翻译服务，之后才会发出请求。之后可在「设置 → 输入」的「云候选」里更改。";

/// Show the consent prompt and report the choice: @YES, @NO, or nil when it closed without one. Overridden by tests.
///
/// Non-modal on purpose: a modal loop would stop this process from serving IMK while the prompt is up, which would freeze typing in every other app until it is answered. The bundle is LSBackgroundOnly, so it has to activate itself for the window to take focus.
- (void)presentCloudConsent:(void (^)(NSNumber *enabled))completion {
    static NSAlert *alert;
    static void (^pending)(NSNumber *);
    pending = [completion copy];
    alert = [NSAlert new];
    alert.messageText = MSIMECloudConsentTitle;
    NSDictionary *preferences = [_appearance sharedPreferencesByMerging:_session.hostOptions[@"preferences"] ?: @{}];
    NSDictionary *provider = preferences[@"cloud_candidate_provider"];
    NSString *recipient = @"Google input-tools（inputtools.google.com）";
    if ([provider[@"source"] isEqual:@"custom"] && [provider[@"endpoint"] isKindOfClass:NSString.class]) {
        NSString *endpoint = [provider[@"endpoint"] stringByReplacingOccurrencesOfString:@"{query}" withString:@"query"];
        recipient = [NSURL URLWithString:endpoint].host ?: @"所选自定义服务";
    }
    alert.informativeText = [MSIMECloudConsentMessage stringByReplacingOccurrencesOfString:
        @"Google 的 input-tools 服务（inputtools.google.com）" withString:recipient];
    // 「不启用」排第一，是回车对应的默认按钮：对话框在用户打字时弹出，顺手的一个回车不能替用户打开联网。
    NSButton *disable = [alert addButtonWithTitle:@"不启用"];
    NSButton *enable = [alert addButtonWithTitle:@"启用云候选"];
    static MSIMECloudConsentTarget *target;
    target = [MSIMECloudConsentTarget new];
    target.handler = ^(BOOL enabled) {
        [alert.window orderOut:nil];
        void (^finish)(NSNumber *) = pending;
        pending = nil;
        if (finish) finish(@(enabled));
        // Released after the button action returns; the target and the window are still on the stack here.
        dispatch_async(dispatch_get_main_queue(), ^{ alert = nil; target = nil; });
    };
    enable.target = target;
    enable.action = @selector(enable:);
    disable.target = target;
    disable.action = @selector(disable:);
    [alert layout];
    alert.window.level = NSFloatingWindowLevel;
    [alert.window center];
    MSIMEPresentWindow(alert.window);
}

// 背景音乐在本进程某个控制器是当前输入法时播放。IMK 不保证上一个客户端的 deactivateServer: 先于下一个的 activateServer:，所以只有最后让音乐响起的控制器能停它。生命周期核心按会话记着真正的所有者，这里只决定哪个控制器在偏好更新、会话新建或重建之后重新报告激活。声明在 prepareSession 之前，它们也读它。
static __weak MSIMEInputController *MSIMEMusicOwner;

- (NSDictionary *)runtimeOptions { return MSIMELoadRuntimeOptions(); }

// What this host asks a session for, on top of what the options file carries.
//
// The file is shared with hosts that render a view differently, so a behaviour this host draws is
// requested here rather than written into it. Its own function because the session it produces is
// built inside prepareSession, where a test would have to stand up a whole Engine to see it.
//
// soundPacks is the bundle's built-in sound-pack directory, or nil when the bundle has none. The host library otherwise looks for it beside `resources`, and here that is EngineResources in Application Support, not in the bundle.
static NSDictionary *MSIMESessionOptions(NSDictionary *runtimeOptions, NSString *soundPacks) {
    if (![runtimeOptions isKindOfClass:NSDictionary.class]) return nil;
    NSMutableDictionary *requested = [runtimeOptions mutableCopy];
    // This host draws view.phrase_prefix, so a phrase being assembled out of several selections
    // stays in the composition instead of arriving in the document one piece at a time.
    requested[@"phrase_preedit"] = @YES;
    // macOS 让快捷短语编码参与普通候选；其他宿主仍保留显式 K 模式入口。这是临时宿主能力，不写入偏好。
    requested[@"quick_phrase_candidates"] = @YES;
    // An options file that names a directory itself keeps it.
    if (soundPacks.length && !requested[@"sound_packs"]) requested[@"sound_packs"] = soundPacks;
    return requested;
}

// The current preferences are available before the first HostSession is
// constructed. Configure the log from them so failures emitted while that
// constructor loads plugin tables are not lost before the asynchronous
// preferences reload catches up.
static BOOL MSIMEConfigureDiagnosticLog(NSString *directory,
                                        NSDictionary *preferences) {
    NSDictionary *diagnostic = [preferences isKindOfClass:NSDictionary.class]
        ? preferences[@"diagnostic_log"] : nil;
    const BOOL enabled = [diagnostic isKindOfClass:NSDictionary.class] &&
        [diagnostic[@"server"] isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)diagnostic[@"server"]) == CFBooleanGetTypeID() &&
        [diagnostic[@"server"] boolValue];
    const std::string path = [directory isKindOfClass:NSString.class] && directory.isAbsolutePath
        ? std::string(directory.UTF8String ?: "") : std::string();
    msime_macos_diagnostic_configure(path, enabled);
    return enabled;
}

static void MSIMEConfigureDiagnosticLogFromRuntimeOptions(NSDictionary *options) {
    if (![options isKindOfClass:NSDictionary.class]) return;
    MSIMEConfigureDiagnosticLog(options[@"preferences_directory"], options[@"preferences"]);
}

static void MSIMEConfigureDiagnosticLogBeforeSession(NSDictionary *options) {
    if (![options isKindOfClass:NSDictionary.class]) return;
    NSString *directory = [options[@"preferences_directory"] isKindOfClass:NSString.class] &&
        [options[@"preferences_directory"] isAbsolutePath] ? options[@"preferences_directory"] : nil;
    if (directory) {
        NSDictionary *snapshot = [MSIMEClientSession loadPreferencesInDirectory:directory error:nil];
        NSDictionary *preferences = [snapshot isKindOfClass:NSDictionary.class] ? snapshot[@"preferences"] : nil;
        if ([preferences isKindOfClass:NSDictionary.class]) {
            MSIMEConfigureDiagnosticLog(directory, preferences);
            return;
        }
    }
    MSIMEConfigureDiagnosticLogFromRuntimeOptions(options);
}

// Resources/sound-packs of this bundle, where CMakeLists.txt stages the built-in packs; nil when it is not there.
static NSString *MSIMEBundleSoundPacks(NSBundle *bundle) {
    NSString *directory = [bundle.resourcePath stringByAppendingPathComponent:@"sound-packs"];
    BOOL isDirectory = NO;
    return directory.isAbsolutePath && [NSFileManager.defaultManager fileExistsAtPath:directory isDirectory:&isDirectory] && isDirectory
        ? directory : nil;
}

// 区分运行时配置不可用与引擎拒绝创建，并用可独立测试的固定错误类别说明失败原因。
static NSString *MSIMESessionUnavailableReason(NSDictionary *options, NSError *error = nil) {
    if (!options) return @"no usable runtime options";
    // 只允许固定类别进入日志；引擎的其他错误可能包含私人路径或配置值。
    NSString *message = error.localizedDescription;
    for (NSString *category in @[@"dictionary access unavailable", @"dictionary maintenance busy",
                                 @"invalid options document", @"unsupported host API version"])
        if ([message isEqualToString:category]) return category;
    return @"session refused the runtime options";
}

// Why there is no session, said once per reason.
//
// A controller without a session passes every key straight to the application, which is indistinguishable from the user having chosen English - and until now it left nothing behind at all: the error from initWithOptions: was discarded and the path where the options themselves are missing said nothing. That silence is what a dictionary that was never prepared looks like, and it is what #912 was diagnosed from: the report was "the input method hands English keys to the application in Chinese mode", the cause was guessed at as a hardcoded resource path, and the fix landed in src/dictionary/DictionaryRuntime.mm, which no target compiles. The two reasons below are the ones that need telling apart - nothing prepared the dictionary, or something prepared it and the Engine would not take it - and each has a different next step.
//
// A category rather than the underlying error, because the Host API's errors can name private directories (see the note on MSIMERefreshRuntimeOptionsWith) and this goes to the unified log, which leaves the machine in a sysdiagnose. NSLog rather than the diagnostic log for the same reason the diagnostic log cannot carry it: it is configured from the preferences directory, which arrives in the very runtime options that are missing here.
- (void)reportSessionUnavailable:(NSString *)reason {
    if (![_sessionUnavailableReason isEqualToString:reason]) {
        _sessionUnavailableReason = [reason copy];
        NSLog(@"MSIME has no input session (%@); keys pass through to the application until one opens", reason);
        msime_macos_diagnostic_writef("session_unavailable reason=%s", reason.UTF8String);
    }
    [self requestSessionRecoveryNotice];
}

- (BOOL)canPresentSessionRecovery {
    return !_session && _sessionUnavailableReason && _activeClient && MSIMEFocusedController == self &&
        !_appearance.englishMode && !_resumeDedicatedEnglish && !_viewState.dedicatedEnglish &&
        ![self secureEventInputActive];
}

- (void)requestSessionRecoveryNotice {
    if (![self canPresentSessionRecovery] || MSIMESessionRecoveryRestartPending || MSIMESessionRecoveryNoticeScheduled || MSIMESessionRecoveryPanel.isVisible) return;
    const uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
    if (MSIMESessionRecoveryLastNotice && now - MSIMESessionRecoveryLastNotice < MSIMESessionRecoveryNoticeInterval) return;
    MSIMESessionRecoveryNoticeScheduled = YES;
    __weak MSIMEInputController *weakSelf = self;
    // 离开激活和按键栈后再画界面；队列送达前可能已经恢复、失焦或进入安全输入。
    dispatch_async(dispatch_get_main_queue(), ^{
        MSIMESessionRecoveryNoticeScheduled = NO;
        MSIMEInputController *owner = weakSelf;
        if (![owner canPresentSessionRecovery] || MSIMESessionRecoveryRestartPending || MSIMESessionRecoveryPanel.isVisible) return;
        const uint64_t delivered = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        if (MSIMESessionRecoveryLastNotice && delivered - MSIMESessionRecoveryLastNotice < MSIMESessionRecoveryNoticeInterval) return;
        MSIMESessionRecoveryLastNotice = delivered;
        [[MSIMEInputSessionRecoveryPanel sharedPanel] showForDelegate:owner];
    });
}

- (void)dismissSessionRecoveryNotice:(id)sender {
    (void)sender;
    [MSIMEInputSessionRecoveryPanel dismissForDelegate:self];
}

- (void)retryUnavailableSession:(id)sender {
    (void)sender;
    if (![self canPresentSessionRecovery] || MSIMESessionRecoveryRestartPending) return;
    [MSIMEInputController releaseIdleDictionarySessionsKeeping:0];
    [self prepareSession];
    if ([self canPresentSessionRecovery]) {
        MSIMESessionRecoveryLastNotice = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        [[MSIMEInputSessionRecoveryPanel sharedPanel] showForDelegate:self];
        MSIMESessionRecoveryPanel.message.stringValue = @"重试后仍无法恢复中文输入。可以重启输入法，或切换系统中文输入法继续输入。\n\n重启会结束其他窗口中未完成的输入。";
    }
}

- (void)restartUnavailableInputMethod:(id)sender {
    if (![self canPresentSessionRecovery] || MSIMESessionRecoveryRestartPending) return;
    // 菜单入口也先展示影响说明，第二次明确点击提示中的按钮才重启。
    MSIMEInputSessionRecoveryPanel *panel = [MSIMEInputSessionRecoveryPanel sharedPanel];
    if (sender != panel.restartButton || !panel.isVisible || panel.recoveryDelegate != self) {
        MSIMESessionRecoveryLastNotice = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        [panel showForDelegate:self];
        return;
    }
    [panel setRestartPending:YES];
    [self restartCurrentInputMethod];
}

- (void)prepareSession {
    BOOL reopened = NO;
    BOOL created = NO;
    if (!_session) {
        [MSIMEInputController releaseIdleDictionarySessionsKeeping:MSIMEIdleDictionarySessionLimit];
        NSDictionary *options = nil;
        NSError *sessionError = nil;
        for (NSUInteger attempt = 0; attempt < 2; ++attempt) {
            options = MSIMESessionOptions([self runtimeOptions], MSIMEBundleSoundPacks(NSBundle.mainBundle));
            // 词库维护持有租约时保持等待，不能用回收重试绕过它。
            if (options && MSIMEDictionaryQuiesced(options)) {
                if (!_preferencesTimer) [self startPreferencesMonitoring];
                return;
            }
            [self resolveCloudCandidatesConsentWithOptions:options];
            sessionError = nil;
            if (options) {
                MSIMEConfigureDiagnosticLogBeforeSession(options);
                _session = [self createSessionWithOptions:options error:&sessionError];
            }
            if (_session || attempt != 0 || [MSIMEInputController releaseIdleDictionarySessionsKeeping:0] == 0) break;
            // 文件句柄耗尽也会让配置读取失败；释放闲置会话后重新读取，最多重试一次。
            msime_macos_diagnostic_write("session_retry_after_idle_release");
        }
        if (options) {
            _requestedPageSize = 0;
            id directory = options[@"preferences_directory"];
            if ([directory isKindOfClass:NSString.class] && [directory isAbsolutePath]) _preferencesDirectory = [directory copy];
            if (_session) {
                [MSIMEInputController holdDictionarySession:self];
                created = YES;
                // 生命周期核心接管按键音、上屏音、打字特效和背景音乐。先于下面的 setFocused:，让核心看到这次聚焦；字段属性在聚焦之前由 confirmFieldTraitsBeforeFocus 报告，音乐所有权等到 claimBackgroundMusic。
                [_session setLifecycle:MSIME_LIFECYCLE_FEEDBACK];
                reopened = _resumeDedicatedEnglish;
                _resumeDedicatedEnglish = NO;
            }
        }
        if (_session) {
            _sessionUnavailableReason = nil;
            [MSIMEInputSessionRecoveryPanel dismissForDelegate:self];
        } else [self reportSessionUnavailable:MSIMESessionUnavailableReason(options, sessionError)];
    }
    [self syncPageSize];
    if (_session) {
        _englishOnlineCandidatesEnabled = MSIMEStrictBoolean(MSIMEStatisticsHostOptions(_session)[@"preferences"][@"english_online_candidates"]);
        [self synchronizeManagedEnglishMode];
        [self syncPunctuation];
        [self syncCharacterWidth];
        [self syncCapsLock];
        if (reopened && !MSIMEEditionIsShurufa()) {
            NSDictionary *view = [_session setDedicatedEnglishEnabled:YES error:nil];
            if (view) _viewState = shurufa::mac::ViewState::fromDictionary(view);
        }
        [self confirmFieldTraitsBeforeFocus];
        [self dispatchFocus:YES];
        _focusPending = NO;
        [self configureNeuralForSession];
        [self refreshTypingEffectSettings];
        // 激活时若处于英文模式就还没有会话，那次 claimBackgroundMusic 发给的是 nil，播放器从没被告知输入法处于活动状态；会话在之后切回中文、按键或菜单操作时才建好，这里补上，否则背景音乐要等下一次偏好变化才响。
        if (created && MSIMEMusicOwner == self) [self claimBackgroundMusic];
    }
    [self startPreferencesMonitoring];
}

- (MSIMEClientSession *)createSessionWithOptions:(NSDictionary *)options error:(NSError **)error {
    return [[MSIMEClientSession alloc] initWithOptions:options error:error];
}

- (void)startPreferencesMonitoring {
    if (!_preferencesDirectory) {
        NSDictionary *options = [self runtimeOptions];
        id directory = options[@"preferences_directory"];
        if ([directory isKindOfClass:NSString.class] && [directory isAbsolutePath]) _preferencesDirectory = [directory copy];
        // English-mode activation reaches here without prepareSession; a directory set earlier was already resolved where it was set.
        [self resolveCloudCandidatesConsentWithOptions:options];
    }
    if (_activeClient && _preferencesDirectory) {
        // Activation may happen after the setting changed while the IMK process was not running.
        // Load once here; subsequent changes arrive through the distributed notification above.
        MSIMEReloadTypingStatisticsEnabled(_preferencesDirectory);
        [_appearance setTranslationPreferencesDirectory:_preferencesDirectory];
        [_preferencesTimer invalidate];
        __weak MSIMEInputController *weakSelf = self;
        _preferencesTimer = [NSTimer scheduledTimerWithTimeInterval:1.0 repeats:YES block:^(NSTimer *timer) {
            // The run loop, not the controller, keeps this timer; it stops once its owner is gone.
            MSIMEInputController *owner = weakSelf;
            if (!owner) { [timer invalidate]; return; }
            // Catches a maintenance notification that never arrived.
            [MSIMEInputController releaseQuiescedDictionarySessions];
            [owner reloadPreferences];
            // 正在显示的候选窗不会经过下面 -renderCandidates 里「出现前」的那次检查，替换要在这里赶上。
            if ([owner applyReplacedSkinPackages] && owner->_activeClient && owner->_panel.isVisible) [owner renderCandidates];
        }];
        [self reloadPreferences];
    }
}

- (NSDictionary *)readPreferencesSnapshotInDirectory:(NSString *)directory error:(NSError **)error {
    return [MSIMEClientSession loadPreferencesInDirectory:directory error:error];
}

- (NSDictionary *)recoverPreferencesInDirectory:(NSString *)directory error:(NSError **)error {
    return [MSIMEClientSession recoverPreferencesInDirectory:directory error:error];
}

// Windows 源码在输入法启动时修复无法解析的 config.toml（InitImeConfig 位于 LoadImeConfig 之前）。这里的文档由每个控制器从后台队列轮询，因此在锁下按目录记录一次恢复尝试；目录表有上限，长期运行的输入法不会因用户切换配置目录而持续增长。
static BOOL MSIMEClaimPreferenceRecovery(NSString *directory) {
    static const NSUInteger kMaxClaimedPreferenceDirectories = 64;
    static NSMutableOrderedSet<NSString *> *claimed;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ claimed = [NSMutableOrderedSet orderedSet]; });
    @synchronized(claimed) {
        if ([claimed containsObject:directory]) return NO;
        [claimed addObject:directory];
        if (claimed.count > kMaxClaimedPreferenceDirectories) [claimed removeObjectAtIndex:0];
        return YES;
    }
}

- (void)completePreferenceLoad:(NSDictionary *)snapshot error:(NSError *)error generation:(uint64_t)generation
                       session:(MSIMEClientSession *)session client:(id)client {
    if (generation != _preferenceLoadGeneration || !_preferenceLoadInFlight) return;
    _preferenceLoadInFlight = NO;
    if (!snapshot || error) {
        msime_macos_diagnostic_write("preferences_load_failed");
        return;
    }
    if (!_activeClient || _activeClient != client || _session != session) return;
    // The poll reads this document once a second. Applying an unchanged one costs a full pass over
    // every preference, another trip into the Engine and a diagnostic line, a second at a time, for
    // nothing - and it buried the log this was found in. A revision of zero predates the field and
    // is always applied.
    uint64_t revision = 0;
    if (snapshot[@"revision"] && !MSIMEStrictUnsignedInteger(snapshot[@"revision"], &revision)) { msime_macos_diagnostic_write("preferences_load_failed"); return; }
    if (revision != 0 && revision == _appliedPreferenceRevision) return;
    NSDictionary *preferences = snapshot[@"preferences"];
    NSMutableDictionary *inputPreferences = [preferences isKindOfClass:NSDictionary.class] ? [preferences mutableCopy] : nil;
    id inlinePreedit = inputPreferences[@"tsf_preedit_style"];
    if (![inlinePreedit isKindOfClass:NSString.class] ||
        ![@[@"raw", @"pinyin", @"empty"] containsObject:inlinePreedit])
        inputPreferences[@"tsf_preedit_style"] = @"raw";
    if (!session) {
        if (inputPreferences) [_appearance applySharedInputPreferences:inputPreferences];
        [self applySharedToolbarPreferences:snapshot[@"preferences"]];
        return;
    }
    NSError *updateError = nil;
    NSDictionary *result = [session updatePreferencesSnapshot:snapshot error:&updateError];
    // Failed loads/updates retain the existing window appearance and runtime.
    if (result && !updateError) {
        _appliedPreferenceRevision = revision;
        if (inputPreferences) [_appearance applySharedInputPreferences:inputPreferences];
        [self applySharedToolbarPreferences:snapshot[@"preferences"]];
        // After the preferences, which the session's resolved effect refines with the selected pack.
        [self refreshTypingEffectSettings];
        [self syncCurrentViewFromSession];
        if (!_viewState.valid && result[@"view"]) {
            _viewState = shurufa::mac::ViewState::fromDictionary(result[@"view"]);
        }
        if (_viewState.valid) {
            MSIMEApplyTransitionTrackingMarkedText(@{@"view": _viewState.toDictionary()}, (id<MSIMETextClient>)_activeClient,
                                                   _appearance.inlinePreeditStyle, nil, &_clientHasMarkedText);
        }
        if (MSIMEMusicOwner == self) [self claimBackgroundMusic];
        // Another surface - the shared settings page, an account push - can have changed the scheme.
        [self syncSystemInputModeForClient:_activeClient];
        [self renderCandidates];
        [self synchronizeCandidateServices];
        // 升级前存在本机的应用例外还没进文档：保存一次把它们发布出去，不等用户碰巧改一项别的外观设置。保存后的重新载入看到文档已经包含它们，这个标记就落下，不会反复保存。
        if (_appearance.applicationInputModeRulesAwaitPublication) [self persistAppearancePreferences];
    } else {
        msime_macos_diagnostic_write("preferences_apply_failed");
    }
}

// 选中的皮肤包被同名替换时，清掉展开页高度缓存，候选窗会在下一次绘制时重新读取外观与装饰图。
- (BOOL)applyReplacedSkinPackages {
    if (!_appearance || ![_appearance reloadReplacedSkinPackages]) return NO;
    _expandedViewportHeight = 0;
    return YES;
}

- (void)reloadPreferences {
    if (!_activeClient || !_preferencesDirectory || _preferenceLoadInFlight) return;
    _preferenceLoadInFlight = YES;
    const uint64_t generation = _preferenceLoadGeneration;
    MSIMEClientSession *session = _session;
    id client = _activeClient;
    NSString *directory = [_preferencesDirectory copy];
    __weak MSIMEInputController *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        id current = weakSelf; // 用 id：要交给 MSIMEReleaseControllerOnMain
        if (!current) return;
        NSError *error = nil;
        NSDictionary *snapshot = [current readPreferencesSnapshotInDirectory:directory error:&error];
        // A document that is not JSON at all is backed up and repaired here, so the IME comes back on the salvaged values instead of running on whatever it last applied. The host refuses a well-formed document it cannot read - most likely a newer build's - and leaves that to the settings page's explicit repair.
        NSString *backupName = nil;
        if (!snapshot && error && MSIMEClaimPreferenceRecovery(directory)) {
            NSError *recoveryError = nil;
            NSDictionary *recovery = [current recoverPreferencesInDirectory:directory error:&recoveryError];
            NSDictionary *recoveredSnapshot = [recovery[@"snapshot"] isKindOfClass:NSDictionary.class] ? recovery[@"snapshot"] : nil;
            if (recoveredSnapshot && !recoveryError) {
                snapshot = recoveredSnapshot;
                error = nil;
                if ([recovery[@"recovered"] isEqual:@YES])
                    backupName = [recovery[@"backup_name"] isKindOfClass:NSString.class] ? recovery[@"backup_name"] : @"";
            } else if (recoveryError) {
                error = recoveryError;
            }
        }
        // 读取期间 IMK 可能已放掉控制器，这里的强引用就成了最后一个；交回主线程释放，dealloc 收起浮动工具栏时才不会在本线程触碰 AppKit。释放先于下面的完成块入队，完成块仍按弱引用取到控制器，时机与此前相同。
        MSIMEReleaseControllerOnMain(&current);
        dispatch_async(dispatch_get_main_queue(), ^{
            [weakSelf completePreferenceLoad:snapshot error:error generation:generation session:session client:client];
            // After the completion, which is what configures the diagnostic log from the repaired document.
            if (backupName)
                msime_macos_diagnostic_write(std::string("preferences_recovered backup=") + (backupName.UTF8String ?: ""));
        });
    });
}

- (void)applySharedToolbarPreferences:(NSDictionary *)preferences {
    if (MSIMEConfigureDiagnosticLog(_preferencesDirectory, preferences))
        msime_macos_diagnostic_write("preferences_applied");
    if ([preferences isKindOfClass:NSDictionary.class]) {
        _englishOnlinePreferenceLoaded = YES;
        const BOOL englishOnlineChanged = _englishOnlineCandidatesEnabled != MSIMEStrictBoolean(preferences[@"english_online_candidates"]);
        _englishOnlineCandidatesEnabled = MSIMEStrictBoolean(preferences[@"english_online_candidates"]);
        if (englishOnlineChanged) {
            [self cancelAITranslations];
            [self cancelCloudCandidates];
        }
        [self synchronizeManagedEnglishMode];
        id wubiCodeHint = preferences[@"wubi_code_hint"];
        if ([wubiCodeHint isKindOfClass:NSNumber.class] &&
            CFGetTypeID((__bridge CFTypeRef)wubiCodeHint) == CFBooleanGetTypeID())
            _wubiCodeHintEnabled = [wubiCodeHint boolValue];
        _voiceThemePreferences = [preferences copy];
        _menuThemePreferences = [preferences copy];
        if (_voiceOverlay) [_voiceOverlay applyThemePreferences:preferences];
    }
    if (MSIMEApplySharedVoicePreferences(preferences[@"voice_input"], NSUserDefaults.standardUserDefaults))
        _voicePermissionToken = nil;
    if (!MSIMEVoiceInputEnabled(NSUserDefaults.standardUserDefaults))
        [self finishVoiceInputForDisable];
    BOOL translationChanged = NO;
    BOOL candidateTranslationsEnabled = _appearance.candidateTranslations;
    BOOL candidateEnglishGlossEnabled = _appearance.candidateEnglishGloss;
    id glossEnabled = preferences[@"candidate_translations"];
    if ([glossEnabled isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)glossEnabled) == CFBooleanGetTypeID()) {
        translationChanged = ![_glossEnabled isEqual:glossEnabled];
        _glossEnabled = glossEnabled;
        candidateTranslationsEnabled = [glossEnabled boolValue];
    }
    id englishGlossEnabled = preferences[@"candidate_english_gloss"];
    if ([englishGlossEnabled isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)englishGlossEnabled) == CFBooleanGetTypeID()) {
        translationChanged |= candidateEnglishGlossEnabled != [englishGlossEnabled boolValue];
        candidateEnglishGlossEnabled = [englishGlossEnabled boolValue];
    }
    id target = preferences[@"translation_target_language"];
    if ([@[@"en", @"fr", @"ja", @"es", @"ru", @"de", @"ko"] containsObject:target]) {
        translationChanged |= ![_glossTargetLanguage isEqual:target];
        _glossTargetLanguage = [target copy];
    }
    if (target || preferences[@"translation_secondary_language"] ||
        [preferences.allKeys containsObject:@"translation_secondary_language"]) {
        NSArray *targets = MSIMETranslationTargetsFromPreferences(preferences, _glossTargetLanguage ?: @"en");
        translationChanged |= ![_glossTargetLanguages isEqual:targets];
        _glossTargetLanguages = [targets copy];
    }
    // 释义显示只改画法，不换请求：只记下开关，下一次画候选时按它整理释义。缺键时保留出厂默认。
    id showPos = preferences[@"candidate_translation_show_pos"];
    if ([showPos isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)showPos) == CFBooleanGetTypeID())
        _glossShowPos = [showPos boolValue];
    id singleSense = preferences[@"candidate_translation_single_sense"];
    if ([singleSense isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)singleSense) == CFBooleanGetTypeID())
        _glossSingleSense = [singleSense boolValue];
    // 候选角标同样只改画法：记下原始偏好，变了才重算完整徽标串。缺键时保留出厂默认。
    BOOL badgeChanged = NO;
    id fixedBadge = preferences[@"candidate_fixed_badge"];
    if ([fixedBadge isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)fixedBadge) == CFBooleanGetTypeID() &&
        _fixedBadgeEnabled != [fixedBadge boolValue]) {
        _fixedBadgeEnabled = [fixedBadge boolValue];
        badgeChanged = YES;
    }
    id fixedBadgeStyle = preferences[@"candidate_fixed_badge_style"];
    if ([fixedBadgeStyle isKindOfClass:NSString.class] && [(NSString *)fixedBadgeStyle length] &&
        ![_fixedBadgeStyle isEqual:fixedBadgeStyle]) {
        _fixedBadgeStyle = [(NSString *)fixedBadgeStyle copy];
        badgeChanged = YES;
    }
    id aiBadgeStyle = preferences[@"candidate_ai_badge_style"];
    if ([aiBadgeStyle isKindOfClass:NSString.class] && [(NSString *)aiBadgeStyle length] &&
        ![_aiBadgeStyle isEqual:aiBadgeStyle]) {
        _aiBadgeStyle = [(NSString *)aiBadgeStyle copy];
        badgeChanged = YES;
    }
    if (badgeChanged) {
        _aiBadgeString = MSIMECandidateBadgeString(msime::mac::candidate_ai_badge(_aiBadgeStyle.UTF8String ?: ""));
        _fixedBadgeString = MSIMECandidateBadgeString(msime::mac::candidate_fixed_badge(_fixedBadgeEnabled, _fixedBadgeStyle.UTF8String ?: ""));
    }
    NSDictionary *custom = preferences[@"custom_translation"];
    if ([custom isKindOfClass:NSDictionary.class]) {
        if (_customTranslationConfig && ![_customTranslationConfig isEqual:custom]) [[MSIMETranslationCache sharedCache] clear];
        translationChanged |= ![_customTranslationConfig isEqual:custom];
        _customTranslationConfig = [custom copy];
    }
    NSDictionary *tencent = preferences[@"tencent_tmt"];
    if ([tencent isKindOfClass:NSDictionary.class]) {
        if (_tencentTranslationConfig && ![_tencentTranslationConfig isEqual:tencent]) [[MSIMETranslationCache sharedCache] clear];
        translationChanged |= ![_tencentTranslationConfig isEqual:tencent];
        _tencentTranslationConfig = [tencent copy];
    }
    NSDictionary *niuTrans = preferences[@"niutrans"];
    if ([niuTrans isKindOfClass:NSDictionary.class]) {
        if (_niuTransConfig && ![_niuTransConfig isEqual:niuTrans]) [[MSIMETranslationCache sharedCache] clear];
        translationChanged |= ![_niuTransConfig isEqual:niuTrans];
        _niuTransConfig = [niuTrans copy];
    }
    NSDictionary *aiTranslation = preferences[@"ai_assistant"];
    if ([aiTranslation isKindOfClass:NSDictionary.class]) {
        if (_aiTranslationConfig && ![_aiTranslationConfig isEqual:aiTranslation]) [[MSIMETranslationCache sharedCache] clear];
        translationChanged |= ![_aiTranslationConfig isEqual:aiTranslation];
        _aiTranslationConfig = [aiTranslation copy];
    }
    if (translationChanged || (_glossEnabled && !_glossEnabled.boolValue)) {
        // None of these settings feed the AI request, so a pending AI suggestion survives them; AI config changes are caught by the _aiQuery identity check on the next render.
        [self cancelCustomTranslations];
        if (!candidateEnglishGlossEnabled) { [self cancelCandidateGloss]; [self cancelTargetGloss]; }
        NSDictionary *view = [_session viewWithError:nil];
        if (!candidateTranslationsEnabled && !candidateEnglishGlossEnabled && view) {
            uint64_t generation = 0;
            if (MSIMEStrictUnsignedInteger(view[@"generation"], &generation)) [_session applyTranslations:@[] generation:generation error:nil];
        }
    }
    id pageSize = preferences[@"candidate_page_size"];
    NSUInteger strictPageSize = 0;
    if (MSIMEStrictUnsignedIntegerValue(pageSize, &strictPageSize) && strictPageSize >= msime::mac::kMinimumCandidatePageSize && strictPageSize <= msime::mac::kMaximumCandidatePageSize && strictPageSize != _requestedPageSize) _requestedPageSize = 0;
    [_appearance applySharedInputPreferences:preferences];
    [_appearance applySharedCandidatePreferences:preferences];
    if (!_appearance.inputModeHUD) [[MSIMEInputModeHUDPanel sharedPanel] orderOut:nil];
    // 共享文档里的开关在载入它的这一刻生效，不等下一次切换中英文：组字中推迟应用文档的分支只走到这里，不会调 sync。关上时总是隐藏；打开时只有持有客户端的控制器按当前模式显示，输入法不在前台时收到的通知不把它放回菜单栏。文档只在有控制器持有客户端时才被载入（每秒一次的轮询和保存后的重新载入都要求 _activeClient），所以在设置应用里拨开关时要等回到输入框、输入法重新获得焦点才生效。
    if (!_appearance.inputModeStatusItem) [self presentInputModeStatusItem:nil];
    else if (_activeClient) [self presentInputModeStatusItem:[self currentInputModeID]];
    [_appearance applySharedAssistancePreferences:preferences];
    [_appearance applySharedToolbarPreferences:preferences];
    [_appearance applySharedLocalModes:preferences[@"local_modes"]];
    // 全局主题随文档其余部分一起到达，中英文提示与工具栏调色板沿用同一明暗策略；自带明暗的主题钉住明暗。
    NSDictionary *toolbarThemePreferences = preferences;
    if (const auto fixed = [_appearance fixedThemeMode]) {
        NSMutableDictionary *pinned = [preferences mutableCopy];
        pinned[@"theme"] = *fixed ? @"dark" : @"light";
        toolbarThemePreferences = pinned;
    }
    const BOOL systemGlass = [_appearance.globalTheme isEqual:@"system"];
    [MSIMEInputModeHUDPanel sharedPanel].liquidGlassEnabled = systemGlass;
    // The mode badge wears the toolbar's palette, so it is drawn in the toolbar's mode too; it is updated whether or not the toolbar is shown.
    [[MSIMEInputModeHUDPanel sharedPanel] applyThemePreferences:toolbarThemePreferences];
    // 提示尺寸独立于工具栏，并遵循显示图标设置。
    [[MSIMEInputModeHUDPanel sharedPanel] applySizingPreferences:preferences];
    // Typing effects follow preferences.plugins; a partial document without it leaves them as they are.
    [[MSIMETypingEffectPanel sharedPanel] applyPreferences:preferences];
    // Effects switched on mid-session were never checked against the foreground application at activation.
    if (_activeClient) [self refreshTypingEffectFullscreen];
    NSDictionary *toolbar = preferences[@"floating_toolbar"];
    id enabled = [toolbar isKindOfClass:NSDictionary.class] ? toolbar[@"enabled"] : nil;
    if ([enabled isKindOfClass:NSNumber.class]) {
        [_appearance applySharedToolbarVisibility:[enabled boolValue]];
    }
}

- (void)deactivateServer:(id)sender {
    ++_spacingInputSequence;
    [[MSIMEInputModeHUDPanel sharedPanel] orderOut:nil];
    [[MSIMETypingEffectPanel sharedPanel] settle];
    // Every focus loss writes the key heatmap counts, including a late one for a previous client: they are this controller's presses either way.
    [self flushKeyPresses];
    // 上一个客户端迟到的回调不能拆掉当前客户端的组字、面板、监视和尚未松开的修饰键。成对标点也一样：activateServer: 已丢掉上一个客户端欠着的闭合符，此刻待补的闭合符和跳过记录都属于当前客户端，替它补上会把这一对提前合上、把组字中的 marked text 整段替换掉，所以迟到判断必须先于补闭合符。
    if (!sender || sender != _activeClient) return;
    [MSIMEInputSessionRecoveryPanel dismissForDelegate:self];
    if (![self commitManagedEnglishRaw]) return;
    [self flushPendingPairedClosing];
    _pairedPunctuation.clear();
    _backspaceHoldArmed = NO;
    [self clearSmartPunctuationSpaceConversion];
    [self clearSmartPunctuationSpaceRevert];
    [self invalidateSmartPunctuationShadow];
    msime_macos_diagnostic_write("focus_out");
    [_voiceEditTracker noteClientDeactivated];
    [self suspendVoiceForFocusChange];
    if (![self ownsPersistentVoiceRecording])
        MSIMEDeactivateVoice(_voiceService, _session, _voiceAudioMuter, _voiceOverlay,
            MSIMEVoiceProviderSocket(), _voiceGeneration);
    [self cancelCandidateTranslations];
    [self cancelCloudCandidates];
    [self discardGlossSensePage];
    [self discardExpandedCandidates];
    [self resetModifierTap];
    [self abandonCapsLockTap];
    [self resetPreferenceLoadState];
    // A focus-out is the reference's ClientSuspended, which never changes floating-toolbar visibility: the toolbar keeps this controller as its owner and stays on screen until the next activateServer: hands it on or the user selects another input source.
    [_keymapPanel orderOut:nil];
    [_preferencesTimer invalidate];
    _preferencesTimer = nil;
    [self releaseBackgroundMusic];
    if (MSIMEEditionIsShurufa()) [self commitShurufaRawComposition];
    if (_session) [self dispatchFocus:NO];
    [self resetCandidateAnchor];
    [self hideCandidatePanel:"focus_out"];
    _activeClient = nil;
    if (MSIMEFocusedController == self) MSIMEFocusedController = nil;
    MSIMERefreshVoiceShortcutTap();
    _dictionarySessionLastUse = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
    [MSIMEInputController releaseIdleDictionarySessionsKeeping:MSIMEIdleDictionarySessionLimit];
    [super deactivateServer:sender];
}

// 菜单与快捷键复用同一套全半角/标点切换路径，保持运行态一致。
- (void)toggleFullWidthInput:(id)sender { (void)sender; [self toggleRuntimeFullWidthInput]; }
- (void)toggleChinesePunctuation:(id)sender {
    (void)sender;
    [self ensureAppearance];
    if (_session && _activeClient && _viewState.hasEditableText()) {
        if (![self dispatchCommand:MSIME_FINISH_COMPOSITION]) return;
    }
    if ([_appearance.punctuationLock isEqual:@"chinese"] || [_appearance.punctuationLock isEqual:@"english"]) {
        _appearance.runtimeChinesePunctuation = [_appearance.punctuationLock isEqual:@"chinese"];
        [self syncPunctuation];
        return;
    }
    // 标点切换只改变当前应用的运行态，保存值仍是初始状态。
    _appearance.runtimeChinesePunctuation = !_appearance.runtimeChinesePunctuation;
    [self syncPunctuation];
    [self showInputStateHUDText:_appearance.runtimeChinesePunctuation ? @"中文标点" : @"英文标点"
        announcement:_appearance.runtimeChinesePunctuation ? @"中文标点" : @"英文标点"];
}
- (void)toggleCandidateTranslations:(id)sender {
    (void)sender;
    [self ensureAppearance];
    _appearance.candidateTranslations = !_appearance.candidateTranslations;
    [self showInputStateHUDText:_appearance.candidateTranslations ? @"译文开" : @"译文关"
        announcement:_appearance.candidateTranslations ? @"显示译文" : @"隐藏译文"];
}
- (void)selectInputScheme:(id)sender {
    MSIMEInputController *focused = MSIMEFocusedController;
    if (focused && focused != self) { [focused selectInputScheme:sender]; return; }
    [self ensureAppearance];
    NSString *scheme = [sender respondsToSelector:@selector(representedObject)] ? [sender representedObject] : nil;
    if (![MSIMEInputSchemeNames() containsObject:scheme] || [_appearance.inputScheme isEqual:scheme]) return;
    // The composition was typed under the old scheme; commit it rather than reinterpret its keys.
    if (_session && _activeClient && _viewState.hasEditableText()) {
        if (![self dispatchCommand:MSIME_FINISH_COMPOSITION]) return;
    }
    _appearance.inputScheme = scheme;
    [self applyPreferencesToSessionNow];
    [self showInputSchemeHUD];
}
- (void)selectGlobalTheme:(id)sender {
    [self ensureAppearance];
    NSString *theme = [sender respondsToSelector:@selector(representedObject)] ? [sender representedObject] : nil;
    if ([theme isKindOfClass:NSString.class]) _appearance.globalTheme = theme;
}
- (void)toggleRuntimeFullWidthInput {
    [self ensureAppearance];
    _appearance.runtimeFullWidthInput = !_appearance.runtimeFullWidthInput;
    [self syncCharacterWidth];
    [self showInputStateHUDText:_appearance.runtimeFullWidthInput ? @"全角" : @"半角"
        announcement:_appearance.runtimeFullWidthInput ? @"全角字符" : @"半角字符"];
}



- (NSUInteger)recognizedEvents:(id)sender {
    (void)sender;
    return NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged;
}

- (void)restartCurrentInputMethod {
    if (MSIMESessionRecoveryRestartPending) return;
    MSIMESessionRecoveryRestartPending = YES;
    // 完成块也被 NSWorkspace 并发队列上的回调持有，那边可能最后才释放它：只捕获弱引用，理由同 showCloudClipboard:。
    __weak MSIMEInputController *weakSelf = self;
    [self launchInputMethodRestart:^(BOOL launched) {
        MSIMEInputController *owner = weakSelf;
        MSIMESessionRecoveryRestartPending = NO;
        if (!launched) {
            if (owner && MSIMESessionRecoveryPanel.recoveryDelegate == owner) {
                [MSIMESessionRecoveryPanel setRestartPending:NO];
                MSIMESessionRecoveryPanel.message.stringValue = MSIMEEditionText(@"未能启动输入法重启。请先切换系统中文输入法继续输入，并检查水杉输入法是否安装完整。");
            }
            NSBeep();
            return;
        }
        [MSIMEInputSessionRecoveryPanel dismissForDelegate:nil];
        if (owner) [owner terminateCurrentInputMethod];
        else [NSApp terminate:nil];
    }];
}

- (void)launchInputMethodRestart:(void (^)(BOOL launched))completion {
    MSIMELaunchInputSourceReregistration(NSBundle.mainBundle.bundleURL, NSWorkspace.sharedWorkspace, completion);
}

- (void)terminateCurrentInputMethod {
    [self flushKeyPressesWaitingUntilWritten:YES];
    [NSApp terminate:nil];
}

// Whether word-to-character is enabled and bound to the pair this event belongs to. Shared by the paging
// exclusion and the branch that acts on it so the two can never disagree about who owns the key.
- (BOOL)wordCharacterClaimsEvent:(NSEvent *)event {
    NSDictionary *wordCharacter = [_appearance wordCharacterOptions];
    if (![wordCharacter[@"enabled"] boolValue]) return NO;
    // A scheme whose letters build the written text directly (a Korean syllable or Hanja, a Vietnamese word) has no word to take a character from: the pair is punctuation there (see candidateListMark in handleEvent:client:).
    if (MSIMESchemeTrait(_viewState, msime::mac::scheme::LetterComposition)) return NO;
    NSString *characters = event.charactersIgnoringModifiers;
    if (characters.length != 1) return NO;
    const unichar character = [characters characterAtIndex:0];
    // A key the Engine spells with, such as expression mode's '-', is input rather than an edge pick.
    if (MSIMESpellingSymbol(_viewState, character)) return NO;
    const BOOL brackets = [wordCharacter[@"keys"] isEqual:@"brackets"];
    return msime::mac::IsPhysicalWordCharacterKey(event.keyCode, brackets, static_cast<char>(character)) &&
        (character == (brackets ? '[' : '-') || character == (brackets ? ']' : '='));
}

// The height the horizontal panel keeps under every candidate for its glosses, whether or not this page
// has any yet. Sizing by content instead means the first composition reserves nothing - the gloss request
// is debounced and answered seconds later, so no candidate carries one - and the panel grows the moment
// the answer lands, which is the jump the reservation exists to prevent. Reserved per configured target
// language, so a single-language setup does not pay for a row it will never fill.
//
// Horizontal only. Vertical draws the gloss on the candidate's own row, where it costs width rather than
// height, and width still follows the content: reserving it would widen the panel for nothing.
//
// A Korean Hanja list always keeps a line for the 훈음, which every Hanja row draws on its gloss line whatever the translation switches say, with the translation lines, when they are on, under it.
- (CGFloat)reservedGlossHeightForFont:(NSFont *)glossFont {
    if (_appearance.vertical) return 0;
    return [self glossHeightReservedForFont:glossFont];
}

// 预留的释义行：横排每一行都留，竖排只给视图标了 translation_reserved 的那一行留（候选词翻译范围是「仅当前高亮」时的高亮行），行数相同。
- (CGFloat)glossHeightReservedForFont:(NSFont *)glossFont {
    const NSUInteger readingLines = MSIMEKoreanHanjaListOpen(_viewState) ? 1 : 0;
    const BOOL translations = (_appearance.candidateTranslations || _appearance.candidateEnglishGloss) &&
        !(_glossEnabled && !_glossEnabled.boolValue && !_appearance.candidateEnglishGloss);
    const NSUInteger lines = readingLines + (translations ? MIN(MAX(_glossTargetLanguages.count, (NSUInteger)1), (NSUInteger)2) : 0);
    if (lines == 0) return 0;
    NSMutableArray<NSString *> *placeholder = [NSMutableArray arrayWithCapacity:lines];
    for (NSUInteger line = 0; line < lines; ++line) [placeholder addObject:@"X"];
    return MSIMETranslationTextSize([placeholder componentsJoinedByString:@"\n"], glossFont).height + MSIMECandidateGlossPadding * MSIMECandidateScale(_appearance);
}

// Secure event input is on while a password field, or a terminal's secure keyboard entry, has the keyboard. It is window-server state shared by every process, so an application that leaves it on also silences this one; that errs the right way, because a click per keystroke tells anyone listening how long a password is.
- (BOOL)secureEventInputActive { return IsSecureEventInputEnabled(); }

// 成为音乐跟随的控制器：经 msime_client_set_active 告诉生命周期核心这个会话处于激活，核心记它为音乐所有者，并按字段属性决定放不放（安全输入时不放）。偏好更新后也调用一次：没有别的声音时打开的音乐，要等播放器听到「输入法处于激活」才响。
- (void)claimBackgroundMusic {
    MSIMEInputController *previous = MSIMEMusicOwner;
    MSIMEMusicOwner = self;
    if (!_session) {
        // 英文模式下激活的控制器还没有会话，核心里的所有者仍是上一个控制器的会话。所有权在宿主这边已经转走，上一个控制器迟到的 deactivateServer: 不会再报告停用，所以这里替它报告，免得音乐在离开输入法后一直放着。会话建好后 prepareSession 会再认领一次。
        if (previous && previous != self) [previous->_session setLifecycleActive:NO];
        return;
    }
    // 先报字段属性，核心据此判断安全输入，免得在密码框里先响一下。
    [self syncFieldTraits];
    [_session setLifecycleActive:YES];
}

- (void)releaseBackgroundMusic {
    if (MSIMEMusicOwner != self) return;
    MSIMEMusicOwner = nil;
    [_session setLifecycleActive:NO];
}

- (NSDictionary *)directEnglishSpacingContext {
    const int scheme = msime::mac::cycle::LastChineseSchemeCode(_appearance.inputScheme.UTF8String);
    return @{@"scheme":@(scheme < 0 ? 255 : scheme), @"local_mode":@"direct_english"};
}

// 先让原文落地再读上下文，避免同步查询中的下一键越过尚未上屏的文字。
- (void)formatCrossCommitText:(NSString *)text context:(NSDictionary *)context
                       client:(id<MSIMETextClient>)client isCurrent:(BOOL (^)(void))isCurrent {
    NSDictionary *config = _appearance.crossCommitSpacing;
    if (!text.length || text.length > 4096 || ![config[@"enabled"] isEqual:@YES] ||
        _appearance.runtimeFullWidthInput || ![context isKindOfClass:NSDictionary.class]) return;
    BOOL (^canFormat)(void) = ^BOOL {
        return isCurrent() && ![self secureEventInputActive] && !self->_appearance.runtimeFullWidthInput &&
            [config isEqual:self->_appearance.crossCommitSpacing];
    };
    if (!canFormat()) return;
    @try {
        NSString *application = [client respondsToSelector:@selector(bundleIdentifier)] ? [(id)client bundleIdentifier] : nil;
        if (!canFormat()) return;
        NSMutableDictionary *request = [@{@"config":config, @"application":application ?: @"", @"context":context} mutableCopy];
        if (![[MSIMEClientSession crossCommitSpacingRequest:request][@"enabled"] isEqual:@YES]) return;
        uint32_t preceding = 0;
        NSRange committed = MSIMETextClientCommittedRange(client, text, &preceding, canFormat);
        if (!canFormat() || committed.location == NSNotFound || !preceding) return;
        request[@"preceding"] = @(preceding);
        request[@"text"] = text;
        NSString *spaced = [MSIMEClientSession crossCommitSpacingRequest:request][@"text"];
        if (canFormat() && [spaced isKindOfClass:NSString.class] && ![spaced isEqualToString:text])
            [client insertText:spaced replacementRange:committed];
    } @catch (NSException *exception) {
        // 客户端可拒绝文档范围；原文已上屏，格式化失败不重试、不记录输入内容。
        (void)exception;
    }
}

- (void)writeDirectEnglishText:(NSString *)text client:(id<MSIMETextClient>)client {
    if ((id)client != _activeClient) return;
    const uint64_t sequence = _applySequence;
    const uint64_t inputSequence = _spacingInputSequence;
    BOOL (^isCurrent)(void) = ^BOOL {
        return (id)client == self->_activeClient && sequence == self->_applySequence &&
            inputSequence == self->_spacingInputSequence;
    };
    [client insertText:text replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    if (isCurrent())
        [self formatCrossCommitText:text context:[self directEnglishSpacingContext] client:client isCurrent:isCurrent];
    // 格式化替换不重复计数，统计只记本次原始上屏。
    MSIMERecordTypingStatistics(_preferencesDirectory ?: [self runtimeOptions][@"preferences_directory"], text,
                                msime::mac::TypingSource::English);
}

// 把字段属性报告给生命周期核心：安全输入是 SECURE，英文模式是 HOST_ENGLISH，macOS 两项都读得到，所以带 KNOWN。只在变了时调用，按契约先于这个键的 msime_client_key。核心按 SECURE 关掉声音、特效和音乐，所以安全输入的开关不必再由这里去开关音乐。
- (void)syncFieldTraits {
    _secureEventInput = [self secureEventInputActive];
    if (_secureEventInput) [MSIMEInputSessionRecoveryPanel dismissForDelegate:self];
    const uint32_t traits = MSIME_TRAIT_KNOWN | (_secureEventInput ? MSIME_TRAIT_SECURE : 0) |
        (_appearance.englishMode ? MSIME_TRAIT_HOST_ENGLISH : 0);
    if (!_session || traits == _fieldTraits) return;
    _fieldTraits = traits;
    [_session setFieldTraits:traits];
}

// 每次 setFocused:YES 之前都报告一次字段属性，变没变都报：脚本运行时在每次 msime_client_focus(true) 之后要先确认字段属性才投递事件，聚焦前的报告（同值也算）或聚焦后的第一个键都算确认。只在变了时报告的话，复用的会话、快照替换后的会话和按键换了客户端时的聚焦都等不到确认，focus_in 到不了脚本。新建的会话也走这里，所以建会话时不另报。
- (void)confirmFieldTraitsBeforeFocus {
    _fieldTraits = 0;
    [self syncFieldTraits];
}

// 交给本输入法的每个物理按下都先报告给生命周期核心，不论这个键最后被处理还是交给应用；核心据此出按键音并算出打字特效。例外照旧：自动重复（按住一个键算一次，由调用处排除）、Command 或 Control 快捷键（Linux 和 Windows 上也不出声；Option 在 macOS 上打字，照样出声）和英文模式（各桌面宿主都不出声，Windows 只听得见它组字的键）。这些键不调用 msime_client_key，也就不开新的一轮。安全输入时照样调用：核心看到 SECURE 属性，什么都不出。
- (void)reportKeyToLifecycle:(NSEvent *)event {
    [self syncFieldTraits];
    const BOOL shortcut = (event.modifierFlags & (NSEventModifierFlagCommand | NSEventModifierFlagControl)) != 0;
    // 没有会话（会话不可用，或词库维护期间）时没有人可报告，也就什么都不画。
    if (!_session || shortcut || _appearance.englishMode) return;
    const uint32_t packed = [_session lifecycleKey:msime::mac::PhysicalKeySoundClass(event.keyCode)];
    if (!_secureEventInput) [self drawTypingEffect:packed commit:NO];
}

// 指针点选、翻页和修饰键单按都要先开一轮：和物理按键一样先比较字段属性，再调 msime_client_key(MSIME_KEY_POINTER)。MSIME_KEY_POINTER 不出声也不画，这些操作以前就不出按键音；上屏音和上屏特效照常来自随后的派发。
- (void)reportPointerToLifecycle {
    if (!_session) return;
    [self syncFieldTraits];
    [_session lifecycleKey:MSIME_KEY_POINTER];
}

// 判断前台应用窗口是否占满所在显示器，用于控制全屏时打字特效的显隐。
static BOOL MSIMEWindowCoversDisplay(CGRect windowBounds, CGRect displayBounds) {
    if (!std::isfinite(windowBounds.origin.x) || !std::isfinite(windowBounds.origin.y) ||
        !std::isfinite(windowBounds.size.width) || !std::isfinite(windowBounds.size.height) ||
        !std::isfinite(displayBounds.origin.x) || !std::isfinite(displayBounds.origin.y) ||
        !std::isfinite(displayBounds.size.width) || !std::isfinite(displayBounds.size.height) ||
        CGRectIsEmpty(windowBounds) || CGRectIsEmpty(displayBounds))
        return NO;
    constexpr CGFloat tolerance = 2.0;
    return CGRectGetMinX(windowBounds) <= CGRectGetMinX(displayBounds) + tolerance &&
           CGRectGetMinY(windowBounds) <= CGRectGetMinY(displayBounds) + tolerance &&
           CGRectGetMaxX(windowBounds) >= CGRectGetMaxX(displayBounds) - tolerance &&
           CGRectGetMaxY(windowBounds) >= CGRectGetMaxY(displayBounds) - tolerance;
}

static BOOL MSIMEFrontmostApplicationOwnsFullscreenDisplay(void) {
    NSRunningApplication *frontmost = NSWorkspace.sharedWorkspace.frontmostApplication;
    if (frontmost == nil || frontmost == NSRunningApplication.currentApplication) return NO;
    NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID));
    if (![windows isKindOfClass:NSArray.class]) return NO;
    const pid_t pid = frontmost.processIdentifier;
    for (NSDictionary *window in windows) {
        if (![window isKindOfClass:NSDictionary.class] ||
            [window[(id)kCGWindowOwnerPID] intValue] != pid ||
            [window[(id)kCGWindowLayer] integerValue] != 0)
            continue;
        CGRect bounds = CGRectZero;
        if (!CGRectMakeWithDictionaryRepresentation((CFDictionaryRef)window[(id)kCGWindowBounds], &bounds)) continue;
        for (NSScreen *screen in NSScreen.screens) {
            // CGWindow 与 CGDisplay 共用 Quartz 全局坐标空间，显式比较原点与宽高，避免仅放大窗口误判为全屏。
            NSNumber *number = screen.deviceDescription[@"NSScreenNumber"];
            CGRect display = number != nil ? CGDisplayBounds((CGDirectDisplayID)number.unsignedIntValue)
                                           : NSRectToCGRect(screen.frame);
            if (MSIMEWindowCoversDisplay(bounds, display)) return YES;
        }
    }
    return NO;
}

// 前台应用的全屏状态延后到主队列下一轮读取，避免按键路径上枚举屏幕窗口列表。
- (void)refreshTypingEffectFullscreen {
    if (!MSIMETypingEffectPanel.sharedPanel.configured) {
        _typingEffectFullscreen = NO;
        return;
    }
    _typingEffectFullscreenCheckedAt = NSProcessInfo.processInfo.systemUptime;
    __weak MSIMEInputController *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
        MSIMEInputController *strongSelf = weakSelf;
        if (strongSelf && strongSelf->_activeClient) strongSelf->_typingEffectFullscreen = MSIMEFrontmostApplicationOwnsFullscreenDisplay();
    });
}

// Entering or leaving native full screen moves the foreground application to another space.
// The session's resolved typing effect for the panel: read after a focus-in and after a preference update, never on a key, since it may read the effect pack's manifest.
- (void)refreshTypingEffectSettings {
    [[MSIMETypingEffectPanel sharedPanel] applySettings:_session ? [_session typingEffectSettingsWithError:nil] : nil];
}

- (void)typingEffectSpaceChanged:(NSNotification *)notification {
    (void)notification;
    if (_activeClient) [self refreshTypingEffectFullscreen];
}

// 记下一个键或一次上屏的打字特效答复，留到主队列的下一轮再画，这里什么都不画。答复由生命周期核心算好（msime_client_key 的返回值、派发回复的 feedback），连击已经计过。前台应用占着全屏时不在它上面画。
- (void)drawTypingEffect:(uint32_t)packed commit:(BOOL)commit {
    if (!MSIMETypingEffectPanel.sharedPanel.configured) return;
    if (NSProcessInfo.processInfo.systemUptime - _typingEffectFullscreenCheckedAt >= 1.0) [self refreshTypingEffectFullscreen];
    if (_typingEffectFullscreen) return;
    // A tier reached by an earlier key still undrawn is kept, so its bounce is not lost to the key after it.
    _typingEffectPacked = packed | (_typingEffectScheduled ? (_typingEffectPacked & MSIMETypingEffectTierUp) : 0);
    _typingEffectCommit = commit || (_typingEffectScheduled && _typingEffectCommit);
    if (_typingEffectScheduled) return;
    _typingEffectScheduled = YES;
    __weak MSIMEInputController *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{ [weakSelf presentTypingEffect]; });
}

- (void)presentTypingEffect {
    const uint32_t packed = _typingEffectPacked;
    const BOOL commit = _typingEffectCommit;
    _typingEffectScheduled = NO;
    _typingEffectPacked = 0;
    _typingEffectCommit = NO;
    if (!_activeClient || _secureEventInput) return;
    MSIMETypingEffectPanel *panel = MSIMETypingEffectPanel.sharedPanel;
    const MSIMETypingEffect effect = MSIMETypingEffectDecode(packed);
    // Nothing to draw, as after a backspace with only the counter on: the panel only takes a stale badge down, so the client is not asked for its caret.
    if (effect.style == MSIMETypingEffectStyleOff && MSIMETypingEffectComboText(effect.combo) == nil) {
        [panel presentEffect:packed commit:commit caretRect:NSZeroRect candidateView:nil cardRect:NSZeroRect cornerRadius:0];
        return;
    }
    NSRect caret = NSZeroRect;
    [(id<IMKTextInput>)_activeClient attributesForCharacterIndex:0 lineHeightRectangle:&caret];
    if (!MSIMEValidCaret(caret)) caret = NSZeroRect;
    MSIMECandidateChromeView *card = _panel.isVisible && [_panel.contentView isKindOfClass:MSIMECandidateChromeView.class]
        ? (MSIMECandidateChromeView *)_panel.contentView
        : nil;
    const NSRect cardRect = card ? card.cardBounds : NSZeroRect;
    [panel presentEffect:packed commit:commit caretRect:caret candidateView:card cardRect:cardRect cornerRadius:card.cornerRadius];
}

// 每个按键按下都会从这里离开，因此智能标点影子只在按键处理完、结果确定后看到一次。宿主自己派发的事件（语音 sendinput 路径和智能标点重写）会跳过：派发者已经记录了它们携带的内容。
- (BOOL)handleEvent:(NSEvent *)event client:(id)sender {
    ++_spacingInputSequence;
    const bool timed = msime_macos_diagnostic_enabled();
    const uint64_t started = timed ? clock_gettime_nsec_np(CLOCK_UPTIME_RAW) : 0;
    const NSEventType type = event.type;
    const char *label = type == NSEventTypeKeyDown ? "down" : type == NSEventTypeKeyUp ? "up" : type == NSEventTypeFlagsChanged ? "flags" : "other";
    CGEventRef nativeEvent = event.CGEvent;
    const BOOL selfPosted = nativeEvent && CGEventGetIntegerValueField(nativeEvent, kCGEventSourceUserData) == MSIMEVoiceCommitEventTag;
    if (timed && event.timestamp > 0 && !selfPosted) {
        // NSEvent.timestamp and systemUptime share the uptime clock, so this is how long the key waited before reaching us. It includes WindowServer and IMK delivery as well as time queued behind work on our main thread, such as a redraw, so it is an upper bound on our own queueing, unlike Windows' stage=queue which starts at the server's own enqueue.
        const double queueMs = (NSProcessInfo.processInfo.systemUptime - event.timestamp) * 1000.0;
        if (queueMs >= 8.0) msime_macos_diagnostic_writef("[key-latency] stage=queue type=%s elapsed_ms=%.3f", label, queueMs);
    }
    _smartPunctuationShadowWritten = NO;
    // 候选菜单不拿键盘焦点，按键照常来到这里：任何按键都先收起菜单，Esc 和系统菜单一样只关菜单。
    if (event.type == NSEventTypeKeyDown && _candidatePointerMenu.visible) {
        [_candidatePointerMenu dismiss];
        if (event.keyCode == 53) return YES;
    }
    if (event.type == NSEventTypeKeyDown && sender) {
        [self ensureAppearance];
    }
    if (event.type == NSEventTypeKeyDown && sender && !selfPosted && !event.isARepeat) [self reportKeyToLifecycle:event];
    const BOOL handled = [self handleKeyEvent:event client:sender];
    if (event.type == NSEventTypeKeyDown && sender && !selfPosted) {
        [self noteKeyForSmartPunctuationShadow:event eaten:handled];
    }
    if (!selfPosted) [self recordKeyPress:event client:sender];
    if (!handled) [self recordPassthroughKey:event client:sender];
    const double elapsedMs = timed ? static_cast<double>(clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - started) / 1e6 : 0;
    if (timed && elapsedMs >= 8.0) {
        msime_macos_diagnostic_writef("[key-latency] stage=handle type=%s handled=%d elapsed_ms=%.3f", label, handled ? 1 : 0, elapsedMs);
    }
    return handled;
}

- (void)recordPassthroughKey:(NSEvent *)event client:(id)sender {
    if (!MSIMETypingStatisticsEnabled.load(std::memory_order_relaxed)) return;
    if (event.type != NSEventTypeKeyDown || !sender) return;
    // A password field turns on secure event input; what is typed there is never counted. Asked through secureEventInputActive like the key sounds, so a test can pin the window-server state instead of inheriting whatever the machine has on.
    if ([self secureEventInputActive]) return;
    CGEventRef nativeEvent = event.CGEvent;
    if (nativeEvent && CGEventGetIntegerValueField(nativeEvent, kCGEventSourceUserData) == MSIMEVoiceCommitEventTag) return;
    NSString *characters = event.characters;
    if (characters.length != 1) return;
    if (!msime::mac::ShouldCountPassthroughCharacter([characters characterAtIndex:0], (event.modifierFlags & NSEventModifierFlagControl) != 0, (event.modifierFlags & NSEventModifierFlagCommand) != 0)) return;
    const msime::mac::TypingSource source = _appearance.englishMode ? msime::mac::TypingSource::English : MSIMEResolveTypingSource(nil, _viewState, MSIMEStatisticsHostOptions(_session), NO);
    MSIMERecordTypingStatistics(_preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"], characters, source);
}

// Counts one physical key press for the key heatmap, whether the input method consumes the key or hands it to the application. Only each key's press count per local day is kept, never the order or what was typed. A held key counts once, as does a modifier, which reports both edges as FlagsChanged. Nothing is collected while statistics are off or while a password field holds secure event input.
- (void)recordKeyPress:(NSEvent *)event client:(id)sender {
    if (!MSIMETypingStatisticsEnabled.load(std::memory_order_relaxed)) {
        _keyPressBatch.clear();
        return;
    }
    if (!sender) return;
    const unsigned short keyCode = event.keyCode;
    if (event.type == NSEventTypeKeyDown) {
        if (event.isARepeat) return;
    } else if (event.type != NSEventTypeFlagsChanged || !msime::mac::IsModifierPress(keyCode, event.modifierFlags)) {
        return;
    }
    if ([self secureEventInputActive]) return;
    const std::string_view keyId = msime::mac::KeyIdForVirtualKeyCode(keyCode);
    if (keyId.empty()) return;
    // The day is taken at the press, so counts collected before midnight are written under the day they belong to.
    for (msime::mac::KeyPressFlush &flush : _keyPressBatch.record(MSIMETypingStatisticsLocalDay().UTF8String, keyId))
        MSIMERecordKeyPresses([self keyPressStatisticsDirectory], std::move(flush), false);
    if (_keyPressBatch.empty()) {
        [_keyPressFlushTimer invalidate];
        _keyPressFlushTimer = nil;
    } else if (!_keyPressFlushTimer) {
        __weak MSIMEInputController *weakSelf = self;
        _keyPressFlushTimer = [NSTimer timerWithTimeInterval:30 repeats:NO block:^(NSTimer *) {
            MSIMEInputController *controller = weakSelf;
            if (!controller) return;
            controller->_keyPressFlushTimer = nil;
            [controller flushKeyPresses];
        }];
        [NSRunLoop.mainRunLoop addTimer:_keyPressFlushTimer forMode:NSRunLoopCommonModes];
    }
}

- (NSString *)keyPressStatisticsDirectory {
    return _preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"];
}

- (void)flushKeyPresses {
    [self flushKeyPressesWaitingUntilWritten:NO];
}

// Exit runs no queued blocks and never reaches dealloc, so the paths that terminate the process write the pending counts first and wait for them; blocking a few milliseconds is acceptable there.
- (void)flushKeyPressesWaitingUntilWritten:(BOOL)wait {
    [_keyPressFlushTimer invalidate];
    _keyPressFlushTimer = nil;
    if (std::optional<msime::mac::KeyPressFlush> flush = _keyPressBatch.drain())
        MSIMERecordKeyPresses([self keyPressStatisticsDirectory], std::move(*flush), wait);
}

- (void)stopEditableShiftMouseMonitoring {
    [_editableShiftMouseTimer invalidate];
    _editableShiftMouseTimer = nil;
    if (_editableShiftLocalMouseMonitor) [NSEvent removeMonitor:_editableShiftLocalMouseMonitor];
    if (_editableShiftGlobalMouseMonitor) [NSEvent removeMonitor:_editableShiftGlobalMouseMonitor];
    _editableShiftLocalMouseMonitor = nil;
    _editableShiftGlobalMouseMonitor = nil;
}
- (void)cancelEditableShiftTap {
    _editableShiftGate.cancel();
    [self stopEditableShiftMouseMonitoring];
}
- (void)resetModifierTap {
    _modifierTap.reset();
    _editableShiftGate.reset();
    [self stopEditableShiftMouseMonitoring];
}
- (BOOL)startEditableShiftMouseMonitoring {
    const NSEventMask mask = NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown | NSEventMaskOtherMouseDown |
        NSEventMaskLeftMouseUp | NSEventMaskRightMouseUp | NSEventMaskOtherMouseUp |
        NSEventMaskLeftMouseDragged | NSEventMaskRightMouseDragged | NSEventMaskOtherMouseDragged | NSEventMaskScrollWheel;
    __weak MSIMEInputController *weakSelf = self;
    _editableShiftLocalMouseMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:mask handler:^NSEvent *(NSEvent *event) {
        [weakSelf cancelEditableShiftTap];
        return event;
    }];
    _editableShiftGlobalMouseMonitor = [NSEvent addGlobalMonitorForEventsMatchingMask:mask handler:^(NSEvent *event) {
        (void)event;
        [weakSelf cancelEditableShiftTap];
    }];
    // 轻点上限为半秒；丢失松键通知也不能让监视器一直挂着。
    _editableShiftMouseTimer = [NSTimer timerWithTimeInterval:0.5 repeats:NO block:^(NSTimer *timer) {
        (void)timer;
        [weakSelf cancelEditableShiftTap];
    }];
    [NSRunLoop.mainRunLoop addTimer:_editableShiftMouseTimer forMode:NSRunLoopCommonModes];
    return _editableShiftLocalMouseMonitor != nil && _editableShiftGlobalMouseMonitor != nil;
}
- (msime::mac::EditableShiftFocus)editableShiftFocusForClient:(id)sender {
    if ([self secureEventInputActive]) return {};
    NSString *bundle = [sender respondsToSelector:@selector(bundleIdentifier)] ? [sender bundleIdentifier] : nil;
    return msime::mac::CaptureEditableShiftFocus(bundle);
}
- (BOOL)observeModifierTap:(NSEvent *)event client:(id)sender {
    if (MSIMEEditionIsShurufa()) { [self resetModifierTap]; return NO; }
    const BOOL restricted = _appearance.shiftTapRequiresEditable && _appearance.shiftTapShortcut;
    const bool wasArmed = _editableShiftGate.armed();
    const bool allowed = _editableShiftGate.observe(event, restricted, [self, sender] {
        return [self editableShiftFocusForClient:sender];
    });
    if (_editableShiftGate.armed() && !wasArmed && ![self startEditableShiftMouseMonitoring])
        [self cancelEditableShiftTap];
    if (!_editableShiftGate.armed()) [self stopEditableShiftMouseMonitoring];
    const bool tap = _modifierTap.observe(event, _appearance.shiftTapShortcut, _appearance.controlTapShortcut);
    const bool shift = event.type == NSEventTypeFlagsChanged && (event.keyCode == 56 || event.keyCode == 60);
    // Control 轻点和其他切换快捷键不受可编辑焦点限制。
    return tap && (!shift || allowed);
}

- (BOOL)handleKeyEvent:(NSEvent *)event client:(id)sender {
    CGEventRef nativeEvent = event.CGEvent;
    if (nativeEvent && CGEventGetIntegerValueField(nativeEvent, kCGEventSourceUserData) == MSIMEVoiceCommitEventTag) return NO;
    if (event.type != NSEventTypeKeyDown && event.type != NSEventTypeKeyUp && event.type != NSEventTypeFlagsChanged) return NO;
    // Shurufa 把 Caps Lock 完全交给系统，不读实体状态，也不撤回系统锁定位。
    if (MSIMEEditionIsShurufa() && event.keyCode == 0x39) {
        [self resetModifierTap];
        return NO;
    }
    if (!MSIMEEditionIsShurufa()) {
        BOOL capsLock = (event.modifierFlags & NSEventModifierFlagCapsLock) != 0;
        if (_capsLockTapReleased && !capsLock) _capsLockTapReleased = NO;
        BOOL staleCapsLock = NO;
        if (_capsLockTapReleased && event.type == NSEventTypeFlagsChanged && event.keyCode == 0x39 && capsLock) {
            // 这时如果确实又按下了 Caps Lock，物理状态仍会是按下；否则只是合成撤回后迟到的旧状态。
            if (MSIMECapsLockKeyHeld()) _capsLockTapReleased = NO;
            else { capsLock = NO; staleCapsLock = YES; }
        }
        if (_capsLockTapReleased && event.type == NSEventTypeKeyDown && event.keyCode != 0x39 && capsLock) {
            capsLock = NO;
            staleCapsLock = YES;
        }
        if (!staleCapsLock && ![self observeCapsLockTap:event capsLock:capsLock sender:sender]) [self adoptCapsLockState:capsLock];
    }
    if (!sender) {
        _voicePermissionToken = nil;
        [_voiceOverlay dismissFailure];
        [self resetModifierTap];
        [self abandonCapsLockTap];
        _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset();
        _backspaceHoldArmed = NO;
        [self clearSmartPunctuationSpaceConversion];
        [self clearSmartPunctuationSpaceRevert];
        [self invalidateSmartPunctuationShadow];
        return NO;
    }
    [self ensureAppearance];
    if (sender != _activeClient) {
        if (![self commitManagedEnglishRaw]) return YES;
        [_voiceOverlay dismissFailure];
        [self suspendVoiceForFocusChange];
        [self cancelCandidateTranslations];
        [self cancelCloudCandidates];
        [self resetCandidateAnchor];
        [self resetModifierTap];
        [self resetPreferenceLoadState];
        [self flushPendingPairedClosing];
        _pairedPunctuation.clear();
        [self resetSmartPunctuationState];
        [self clearSmartPunctuationSpaceConversion];
        [self clearSmartPunctuationSpaceRevert];
        _backspaceHoldArmed = NO;
        // Clear the previous client's marked text before accepting the new focus.
        [self dispatchFocus:NO];
        [MSIMEInputSessionRecoveryPanel dismissForDelegate:nil];
        _activeClient = sender;
        MSIMEFocusedController = self;
        // Whatever the previous client was last given says nothing about this one.
        [self invalidateSmartPunctuationShadow];
        [self applySharedInputPreferencesBeforeFirstUse];
        [_appearance activateInputModeForApplication:[sender respondsToSelector:@selector(bundleIdentifier)] ? [sender bundleIdentifier] : nil];
        [self syncSystemInputModeForClient:sender];
        // Punctuation and width are per app, so the new client's values reach the Engine before it types.
        [self syncPunctuation];
        [self syncCharacterWidth];
        _focusPending = _appearance.englishMode;
        if (!_appearance.englishMode || _englishOnlineCandidatesEnabled) {
            [self synchronizeManagedEnglishMode];
            [self confirmFieldTraitsBeforeFocus];
            [self dispatchFocus:YES];
            [self refreshTypingEffectSettings];
        }
        // 按键也能接管失焦后保留的会话，此时没有 activateServer: 帮它恢复偏好轮询。
        [self startPreferencesMonitoring];
    }
    NSUserDefaults *voiceDefaults = NSUserDefaults.standardUserDefaults;
    if (event.type == NSEventTypeKeyDown && event.keyCode == 53) {
        [_voiceOverlay dismissFailure];
        if (_voiceRetryInFlight) {
            [self cancelArchivedVoiceRetry];
            return YES;
        }
    }
    if (_voicePermissionToken && event.type == NSEventTypeKeyDown && event.keyCode == 53) {
        _voicePermissionToken = nil; _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset(); [self resetModifierTap]; return YES;
    }
    const BOOL voiceEnabled = MSIMEVoiceInputEnabled(voiceDefaults);
    if (!voiceEnabled) { _voiceHoldShortcut.reset(); _configuredVoiceShortcut.reset(); }
    const NSEventModifierFlags physicalModifierFlags = voiceEnabled && event.type == NSEventTypeFlagsChanged
        ? MSIMEPhysicalModifierFlagsState(kCGEventSourceStateHIDSystemState) : 0;
    if (MSIMEEditionIsShurufa() && MSIMEVoiceRecordingOwner && MSIMEVoiceRecordingOwner != self &&
        [MSIMEVoiceRecordingOwner handleConfiguredVoiceShortcut:event]) return YES;
    const BOOL configuredVoice = voiceEnabled && [self usesConfiguredVoiceShortcut];
    if (configuredVoice && [self handleConfiguredVoiceShortcut:event]) return YES;
    const auto voiceShortcut = voiceEnabled && !configuredVoice ? _voiceHoldShortcut.observe(event, {
        [voiceDefaults boolForKey:@"MSIMEClientVoiceHotkeyRightAlt"] != NO,
        [voiceDefaults boolForKey:@"MSIMEClientVoiceHotkeyCtrlCommand"] != NO,
        [voiceDefaults boolForKey:@"MSIMEClientVoiceHotkeyCtrlOption"] != NO,
        // 空格锁定固定为开：空格只在按住录音组合键期间才被拦下，平常打字不受影响。设置页不再给这个开关（`voice_space_lock_switch`），存下的 false 也不再读。
        true
    }, _voiceService.active, physicalModifierFlags) : MSIMEVoiceHoldShortcut::Result{};
    if (voiceShortcut.consumed || voiceShortcut.action != MSIMEVoiceHoldShortcut::Action::None) [self resetModifierTap];
    // MSIME-Windows shows the overlay's cancel and confirm buttons once the hold is locked (ControlCommand::Lock).
    if (voiceShortcut.consumed && _voiceHoldShortcut.locked() && _voiceService.active) [_voiceOverlay setRecordingLocked:YES];
    if (voiceShortcut.action == MSIMEVoiceHoldShortcut::Action::Toggle) {
        _voiceHoldStarting = !voiceShortcut.onRelease && !_voiceService.active;
        if (!voiceShortcut.onRelease || _voiceHoldGeneration == _voiceGeneration) [self toggleVoiceInput:nil];
        _voiceHoldStarting = NO;
        if (!voiceShortcut.onRelease) _voiceHoldGeneration = _voiceGeneration;
    }
    else if (voiceShortcut.action == MSIMEVoiceHoldShortcut::Action::Cancel) {
        [self noteVoiceArchiveCancelled];
        [self cancelHTTPVoiceInput]; [self cancelDoubaoVoiceInput]; [self cancelLiveVoiceInput];
    }
    if (voiceShortcut.consumed) return YES;
    if ([self observeModifierTap:event client:sender]) {
        [self toggleInputModeFromModifierTap];
        return YES;
    }
    if (event.type != NSEventTypeKeyDown) return NO;
    if (_viewState.valid && _viewState.provisional && _session && _activeClient) {
        const NSEventModifierFlags mods = event.modifierFlags &
            (NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand);
        const unichar ch = event.characters.length == 1 ? [event.characters characterAtIndex:0] : 0;
        const BOOL keepsProvisional = (mods == 0) && ((ch >= 'a' && ch <= 'z') || ch == ' ' || ch == '\r' || ch == '\n');
        if (!keepsProvisional) {
            if ([_session respondsToSelector:@selector(flushProvisionalNeuralSnapshotFlushed:error:)]) {
                BOOL flushed = NO;
                const ShurufaViewSnapshot *snap = [_session flushProvisionalNeuralSnapshotFlushed:&flushed error:nil];
                if (flushed && snap) {
                    _viewState = shurufa::mac::ViewState::fromSnapshot(*snap);
                    [self renderCandidates];
                    [self synchronizeCandidateServices];
                }
            } else if ([_session respondsToSelector:@selector(flushProvisionalNeuralWithError:)]) {
                NSDictionary *flushed = [_session flushProvisionalNeuralWithError:nil];
                if (MSIMEStrictBoolean(flushed[@"flushed"]) && [flushed[@"view"] isKindOfClass:NSDictionary.class]) {
                    _viewState = shurufa::mac::ViewState::fromDictionary(flushed[@"view"]);
                    [self renderCandidates];
                    [self synchronizeCandidateServices];
                }
            }
        }
    }
    [_voiceEditTracker noteKeyEvent:event];
    if (MSIMEEditionIsShurufa()) {
        const auto flags = event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl |
            NSEventModifierFlagOption | NSEventModifierFlagCommand);
        // 语音绑定优先；已移除的固定切换键不再吞键或提交当前组字。
        if ((event.keyCode == kVK_Space && (flags == NSEventModifierFlagShift ||
                flags == (NSEventModifierFlagControl | NSEventModifierFlagOption))) ||
            (event.keyCode == kVK_ANSI_H && flags == (NSEventModifierFlagOption | NSEventModifierFlagShift))) return NO;
    }
    [_appearance lockActiveInputMode];
    if (event.keyCode == 51) {
        const BOOL compositionActive = _viewState.hasComposition();
        BOOL suppressEscapedRepeat = NO;
        if (!event.isARepeat) {
            _backspaceHoldArmed = compositionActive;
        } else suppressEscapedRepeat = _backspaceHoldArmed && !compositionActive;
        [self clearSmartPunctuationSpaceRevert];
        if (_lastSmartPunctuation) {
            _smartPunctuationRejected = YES;
            _rejectedSmartPunctuation = _lastSmartPunctuation;
            _lastSmartPunctuation = 0;
        }
        if (suppressEscapedRepeat) {
            // The same hold already consumed the composition. Keep consuming
            // its repeats locally until a fresh Backspace press re-evaluates
            // ownership; no Engine request or document edit is needed here.
            return YES;
        }
    } else if (_smartPunctuationRejected && event.characters.length == 1 &&
               [event.characters characterAtIndex:0] != _rejectedSmartPunctuation) {
        _smartPunctuationRejected = NO;
        _rejectedSmartPunctuation = 0;
    } else if (_lastSmartPunctuation && event.characters.length == 1 &&
               [event.characters characterAtIndex:0] != _lastSmartPunctuation) {
        [self resetSmartPunctuationState];
    }
    if (voiceEnabled && !configuredVoice && !event.isARepeat && [self matchesVoiceToggleHotkey:event]) {
        [self toggleVoiceInput:nil];
        return YES;
    }
    const NSEventModifierFlags competing = NSEventModifierFlagCommand | NSEventModifierFlagControl | NSEventModifierFlagOption;
    // Shurufa 的录音不归某次按键所有：方向键、打字和其他组合键照常交给应用，只把实时文字移到浮层，录音只由快捷键和 Esc 结束。
    if ([self ownsPersistentVoiceRecording]) [self detachVoiceMarkedText];
    else {
        if (_doubaoVoiceRequest) [self cancelDoubaoVoiceInput];
        if (_liveVoiceToken) [self cancelLiveVoiceInput];
    }
    // 「切换中英文」这一组快捷键（Ctrl+Option+Space、Shift+Space）和中英文切换的修饰键轻点走同一条路：
    // 中日英循环开着时它们一起参与循环，关着时仍是二态切换。
    if (_appearance.controlOptionSpaceShortcut && event.keyCode == 49 &&
        (event.modifierFlags & (competing | NSEventModifierFlagShift)) == (NSEventModifierFlagControl | NSEventModifierFlagOption)) {
        if (!event.isARepeat) [self toggleChineseEnglishMode];
        return YES;
    }
    if (_appearance.inputModeShortcut && event.keyCode == 49 && (event.modifierFlags & NSEventModifierFlagShift) &&
        !(event.modifierFlags & competing) && !_modifierTap.shiftChordInterrupted()) {
        if (!event.isARepeat) [self toggleChineseEnglishMode];
        return YES;
    }
    if (MSIMEToggleChordMatches(_appearance.punctuationToggleChord, event)) {
        if (!event.isARepeat) [self toggleChinesePunctuation:nil];
        return YES;
    }
    const char shortcutLetter = MSIMEShortcutLetter(event);
    if (_appearance.characterSetShortcut && shortcutLetter == 'f' &&
        (event.modifierFlags & (competing | NSEventModifierFlagShift)) == (NSEventModifierFlagControl | NSEventModifierFlagShift)) {
        // Like the Windows host, reserve the chord but only toggle in Chinese mode.
        if (!event.isARepeat && !_appearance.englishMode) [self toggleTraditionalOutput:nil];
        return YES;
    }
    if (MSIMEToggleChordMatches(_appearance.englishCandidatesToggleChord, event)) {
        if (!event.isARepeat) [self toggleDedicatedEnglishMode:nil];
        return YES;
    }
    // 全半角：绑定的 chord 在中英文模式下都生效；Option+Shift+H 是 macOS 既有快捷键，
    // 由 `toggle_fullwidth_option_shift_h` 开关门控，只在中文模式。
    const auto widthArm = msime::mac::FullWidthInputToggleArm(
        event.keyCode, msime::mac::ToggleHotkeyCharacter(event), event.modifierFlags, _appearance.fullWidthToggleChord);
    if (widthArm != msime::mac::ToggleFullWidthArm::none &&
        (widthArm == msime::mac::ToggleFullWidthArm::chord || _appearance.fullWidthShortcut) &&
        (!_appearance.englishMode || widthArm == msime::mac::ToggleFullWidthArm::chord)) {
        if (!event.isARepeat) [self toggleRuntimeFullWidthInput];
        return YES;
    }
    const auto maintenanceShortcut = msime::mac::MaintenanceShortcut(
        shortcutLetter,
        (event.modifierFlags & NSEventModifierFlagControl) != 0,
        (event.modifierFlags & NSEventModifierFlagShift) != 0,
        (event.modifierFlags & NSEventModifierFlagOption) != 0,
        (event.modifierFlags & NSEventModifierFlagCommand) != 0);
    if (!MSIMEEditionIsShurufa() && maintenanceShortcut != msime::mac::MaintenanceShortcutAction::None) {
        if (event.isARepeat) return YES;
        switch (maintenanceShortcut) {
            case msime::mac::MaintenanceShortcutAction::ClearCache: {
                if (!_session) [self prepareSession];
                NSError *error = nil;
                NSDictionary *transition = [_session resetCacheWithError:&error];
                if (transition) {
                    [self apply:transition];
                    [self showInputStateHUDText:@"缓存已清理" announcement:@"输入缓存已清理"];
                }
                else if (error) NSBeep();
                break;
            }
            case msime::mac::MaintenanceShortcutAction::Restart:
                [self restartCurrentInputMethod];
                break;
            case msime::mac::MaintenanceShortcutAction::Terminate:
                [self terminateCurrentInputMethod];
                break;
            case msime::mac::MaintenanceShortcutAction::None:
                break;
        }
        return YES;
    }
    if (_appearance.englishMode && _englishOnlineCandidatesEnabled && !_session) [self prepareSession];
    if (_appearance.englishMode && ![self synchronizeManagedEnglishMode]) return YES;
    BOOL managedLiteralKey = NO;
    if (_managedEnglishComposition) {
        const NSEventModifierFlags shortcuts = NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand;
        const BOOL shortcut = (event.modifierFlags & shortcuts) != 0;
        const unichar character = event.characters.length == 1 ? [event.characters characterAtIndex:0] : 0;
        const BOOL asciiLetter = (character >= 'a' && character <= 'z') ||
            (character >= 'A' && character <= 'Z');
        // 重复字母按键是 macOS 长按变音符号菜单的起点。先把托管的原始字母上屏，
        // 再把重复事件交回 Cocoa；单次按键和带竞争修饰键的快捷键仍由输入法处理。
        if (event.isARepeat && !(event.modifierFlags & shortcuts) && asciiLetter) {
            if (![self commitManagedEnglishRaw]) return YES;
            return NO;
        }
        const BOOL candidateDigit = _panel.isVisible && character >= '1' && character <= '9' &&
            (NSUInteger)(character - '1') < _viewState.candidates.size();
        const BOOL printableOther = !candidateDigit && character >= 0x20 && character < 0x7f &&
            !((character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z'));
        managedLiteralKey = shortcut || printableOther || event.keyCode == 36 || event.keyCode == 76;
        if (managedLiteralKey && _viewState.hasEditableText()) {
            // 空格明确选择高亮补全，其他结束键保留原始拼写；当前键继续按英文规则交给应用。
            if (![self dispatchCommand:event.keyCode == 49 && !shortcut ? MSIME_COMMIT_CANDIDATE : MSIME_COMMIT_RAW]) return YES;
        }
    }
    // English mode normally leaves keys to the application, but the Chinese
    // punctuation lock and full-width setting still belong to the IME. This
    // mirrors the closed-IME path used by the other native hosts.
    if (_appearance.englishMode && (!_managedEnglishComposition || managedLiteralKey)) {
        const NSEventModifierFlags competing = NSEventModifierFlagControl | NSEventModifierFlagOption |
                                                NSEventModifierFlagCommand;
        if (!(event.modifierFlags & competing) && event.characters.length == 1) {
            const BOOL keypad = (event.modifierFlags & NSEventModifierFlagNumericPad) != 0 || event.keyCode == 65;
            const BOOL chinese = [_appearance.punctuationLock isEqual:@"chinese"] ||
                ([_appearance.punctuationLock isEqual:@"follow"] && _appearance.runtimeChinesePunctuation);
            unichar character = [event.characters characterAtIndex:0];
            if (_capsLockTapReleased && !(event.modifierFlags & NSEventModifierFlagShift) &&
                character >= 'A' && character <= 'Z') character = (unichar)(character + ('a' - 'A'));
            const std::string output = msime::input::english_mode_output(
                character, keypad, chinese, _appearance.runtimeFullWidthInput, _englishPunctuation,
                MSIMEViewPunctuationMarks(_viewState));
            if (!output.empty()) {
                NSString *text = [[NSString alloc] initWithBytes:output.data() length:output.size()
                                                         encoding:NSUTF8StringEncoding];
                [self writeDirectEnglishText:text client:(id<MSIMETextClient>)sender];
                return YES;
            }
            // 使用激活时已有的应用标识判定接管；排除应用仍把原键交回，不在落字前查客户端。
            const BOOL spacingEnabled = !_appearance.runtimeFullWidthInput && ![self secureEventInputActive] &&
                [_appearance.crossCommitSpacing[@"enabled"] isEqual:@YES] &&
                [[MSIMEClientSession crossCommitSpacingRequest:@{
                    @"config":_appearance.crossCommitSpacing,
                    @"application":_appearance.activeInputModeApplication ?: @"",
                    @"context":[self directEnglishSpacingContext]}][@"enabled"] isEqual:@YES];
            const BOOL asciiLetterOrDigit = (character >= 'a' && character <= 'z') ||
                (character >= 'A' && character <= 'Z') || (character >= '0' && character <= '9');
            if ((spacingEnabled && asciiLetterOrDigit) || (_capsLockTapReleased && character >= 'a' && character <= 'z')) {
                [self writeDirectEnglishText:[NSString stringWithCharacters:&character length:1]
                                      client:(id<MSIMETextClient>)sender];
                return YES;
            }
        }
        return NO;
    }
    // Korean letters take their case from Shift alone (KoreanKeyLetter) and Vietnamese composes an uppercase letter as written, so Caps Lock does not hand them to the application either.
    if (!MSIMESchemeTrait(_viewState, msime::mac::scheme::CapsLockBypassExempt) && MSIMECapsLockFreshUppercaseBypass(event, _viewState)) return NO;
    if (!_session) {
        [self prepareSession];
        // 这个键经过 handleEvent: 时还没有会话（词库维护刚结束，或快照恢复没留下会话），那时没有可报告的对象。会话刚建好，按 msime_client.h 的顺序契约先补报这次按下，再为它派发。
        if (_session && event.type == NSEventTypeKeyDown && !event.isARepeat) [self reportKeyToLifecycle:event];
    }
    if (!_session) return NO;
    if (_focusPending) [self prepareSession];
    [self syncPageSize];
    // 展开的候选网格开着时先归它，它不认的键已经收起网格，往下照常处理。
    if (_expandedCandidates && [self handleExpandedCandidateEvent:event client:sender]) return YES;
    NSUInteger deletionSlot = MSIMECandidateDeletionSlot(event);
    if (_panel.isVisible && deletionSlot != NSNotFound) {
        if (event.isARepeat) return YES;
        NSDictionary *identifier = MSIMERenderedCandidateIdentity(_panel, (NSInteger)deletionSlot);
        if (!MSIMECurrentCandidateIdentity(identifier, _viewState)) return YES;
        NSError *error = nil;
        uint64_t generation = 0; NSUInteger index = 0;
        if (!MSIMEStrictUnsignedInteger(identifier[@"generation"], &generation) || !MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &index)) return YES;
        NSDictionary *result = [_session removeGeneration:generation index:index error:&error];
        if (result) [self apply:result];
        else if (error) NSBeep();
        return YES; // Never finish composition or leak a reserved deletion chord.
    }
    // Candidate numbers follow the physical ANSI number row, matching the
    // Windows TSF path even when the active keyboard layout emits different
    // characters.  Let nine-key mode and modified chords reach the Engine.
    const NSEventModifierFlags candidateDigitModifiers = NSEventModifierFlagShift | NSEventModifierFlagControl |
                                                          NSEventModifierFlagOption | NSEventModifierFlagCommand;
    const int physicalDigit = msime::mac::CandidateDigitSlotOnPage(msime::mac::PhysicalCandidateDigitSlot(event.keyCode),
        _appearance.pageSize);
    NSArray *visibleCandidates = _viewState.visibleCandidatesArray();
    const NSEventModifierFlags glossModifiers = event.modifierFlags &
        (NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand);
    if (_panel.isVisible && physicalDigit >= 0 &&
        glossModifiers == NSEventModifierFlagOption) {
        NSDictionary *candidate = ((NSUInteger)physicalDigit < visibleCandidates.count) ? visibleCandidates[(NSUInteger)physicalDigit] : nil;
        if ([self commitCandidateGlossColumn:1 candidate:candidate client:sender]) return YES;
    }
    if (_panel.isVisible && physicalDigit >= 0 &&
        glossModifiers == NSEventModifierFlagControl) {
        NSDictionary *candidate = ((NSUInteger)physicalDigit < visibleCandidates.count) ? visibleCandidates[(NSUInteger)physicalDigit] : nil;
        if ([self commitCandidateGlossColumn:2 candidate:candidate client:sender]) return YES;
    }
    if (_panel.isVisible && physicalDigit >= 0 && glossModifiers == 0 && _armedGlossColumn > 0) {
        NSDictionary *candidate = ((NSUInteger)physicalDigit < visibleCandidates.count) ? visibleCandidates[(NSUInteger)physicalDigit] : nil;
        if ([self commitCandidateGlossColumn:_armedGlossColumn candidate:candidate client:sender]) return YES;
    }
    const BOOL digitIsSpelling = MSIMESpellingSymbol(_viewState, (unichar)msime::mac::PhysicalCandidateDigitCharacter(physicalDigit));
    if (msime::mac::ShouldRoutePhysicalCandidateDigit(
            _panel.isVisible, _viewState.nineKey, digitIsSpelling,
            (event.modifierFlags & candidateDigitModifiers) != 0) ||
        msime::mac::ShouldRouteSpellingShiftCandidateDigit(
            _panel.isVisible, digitIsSpelling,
            (event.modifierFlags & candidateDigitModifiers) == NSEventModifierFlagShift,
            MSIMESpellingSymbolString(_viewState, event.characters))) {
        const int slot = physicalDigit;
        if (slot >= 0) {
            // The panel owns the rendered snapshot. If it is from an older
            // generation, consume the key until the new page is visible instead
            // of letting it fall through to Engine numeric input.
            NSDictionary *identifier = MSIMERenderedCandidateIdentity(_panel, slot);
            if (!MSIMECurrentCandidateIdentity(identifier, _viewState)) return YES;
            uint64_t generation = 0; NSUInteger index = 0;
            if (!MSIMEStrictUnsignedInteger(identifier[@"generation"], &generation) || !MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &index)) return YES;
            [self dispatchSelectGeneration:generation index:index];
            return YES;
        }
    }
    // Match Windows keypad punctuation and Linux's physical keypad route.
    // Decimal always remains ASCII '.', while arithmetic/separator keys use
    // the Engine punctuation policy when idle. With a composition, every
    // keypad mark finishes the highlighted candidate and appends its literal
    // ASCII byte. Physical routing keeps '-' and '=' out of main-row paging.
    // A keypad mark the view lists among its spelling symbols (Zhuyin's bopomofo keys, an expression operator) is input like the keypad digits and goes on as a character.
    const char keypadPunctuation = msime::mac::KeypadPunctuation(event.keyCode);
    if (keypadPunctuation && !MSIMESpellingSymbol(_viewState, (unichar)keypadPunctuation) &&
        !(event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl |
                                 NSEventModifierFlagOption | NSEventModifierFlagCommand))) {
        const BOOL hasComposition = _viewState.hasEditableText() || !_viewState.candidates.empty();
        _punctuationKeyInFlight = (unichar)keypadPunctuation;
        BOOL transitionOK = NO;
        BOOL handled = NO;
        NSString *committed = nil;
        const BOOL literalASCII = hasComposition || keypadPunctuation == '.';
        if ([_session respondsToSelector:@selector(punctuationSnapshot:preceding:hasPreceding:literalASCII:error:)]) {
            const ShurufaTransitionSnapshot *snapshot = [_session punctuationSnapshot:(uint8_t)keypadPunctuation
                                                                            preceding:0
                                                                         hasPreceding:NO
                                                                         literalASCII:literalASCII
                                                                                error:nil];
            if (!snapshot) _punctuationKeyInFlight = 0;
            else {
                transitionOK = YES;
                handled = snapshot->handled ? YES : NO;
                if (!hasComposition && keypadPunctuation != '.' && snapshot->has_commit && snapshot->commit.len > 0) {
                    committed = shurufa::mac::SliceToNSString(snapshot->commit);
                }
                [self applyTransitionSnapshot:snapshot overrideCommit:nil];
            }
        } else {
            NSDictionary *transition = literalASCII
                ? [_session punctuationASCII:(uint8_t)keypadPunctuation error:nil]
                : [_session punctuation:(uint8_t)keypadPunctuation error:nil];
            if (!transition) _punctuationKeyInFlight = 0;
            else {
                transitionOK = YES;
                handled = MSIMEStrictBoolean(transition[@"handled"]);
                if (!hasComposition && keypadPunctuation != '.' && [transition[@"commit"] isKindOfClass:NSString.class]) {
                    committed = transition[@"commit"];
                }
                [self apply:transition];
            }
        }
        if (transitionOK) {
            // Only the idle route commits the Engine's Chinese form; the ASCII route leaves the tail ASCII.
            if (!hasComposition && keypadPunctuation != '.') {
                [self noteCommittedChinesePunctuationString:committed key:(unichar)keypadPunctuation client:sender];
            }
            if (handled) return YES;
        }
        if (keypadPunctuation == '.') {
            NSString *text = @".";
            [(id<MSIMETextClient>)sender insertText:text replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
            MSIMERecordTypingStatistics(_preferencesDirectory ?: [self runtimeOptions][@"preferences_directory"], text,
                                        MSIMEResolveTypingSource(nil, _viewState, MSIMEStatisticsHostOptions(_session), _appearance.englishMode));
            return YES;
        }
        // A normal punctuation transition can be unhandled in an English or
        // local mode. Leave that key to the host rather than reinterpreting it
        // as the main-row '-'/'=' candidate navigation shortcut.
        return handled;
    }
    if ([self revertSmartPunctuationSpace:event client:(id<MSIMETextClient>)sender]) return YES;
    if ([self convertSmartPunctuationSpace:event client:(id<MSIMETextClient>)sender]) return YES;
    if ([self handleSmartPunctuation:event client:(id<MSIMETextClient>)sender]) return YES;
    // 释义页先处理按键；退出释义页的同一枚 Tab 继续沿用原有的列循环或翻页。
    const BOOL glossSensePageWasActive = [self glossSensePageActive];
    if (glossSensePageWasActive && [self handleGlossSenseEvent:event client:sender]) return YES;
    // Ctrl+Enter offers the highlighted candidate's gloss: one sense commits, several open the page
    // above. The reference and both Linux hosts do the same; here the chord used to fall through to
    // the rule below and commit the composition instead.
    if ((event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagShift |
                                NSEventModifierFlagOption | NSEventModifierFlagCommand)) ==
            NSEventModifierFlagControl &&
        (event.keyCode == 36 || event.keyCode == 76) && event.type == NSEventTypeKeyDown &&
        _appearance.candidateTranslations && _panel.isVisible) {
        NSArray<NSString *> *senses = [self sensesForHighlightedCandidate];
        if (senses.count == 1) {
            NSDictionary *highlighted = [self highlightedCandidateForGloss];
            NSMutableDictionary *single = [highlighted mutableCopy];
            single[@"translation"] = senses.firstObject;
            if ([self commitCandidateGlossColumn:1 candidate:single client:sender]) return YES;
        } else if (senses.count > 1) {
            [self showGlossSensePage:senses];
            return YES;
        }
    }
    // Ctrl+Backspace deletes a segmentation unit and Ctrl+Left / Ctrl+Right move the caret by one, which is what the reference's composition editor does (`IsSegmentBackspaceKey` and `IsSegmentCaretKey` in its input_key_policy.h) and what both Linux front ends and the Windows host already route. It has to be decided before the rule below, which hands every Ctrl, Option and Command chord back to the application after finishing the composition - that rule is what left this host without segment editing.
    //
    // Only the bare Ctrl chord, and for Left / Right also the bare Shift chord, is the input method's: with Ctrl and Shift together, with Option or Command also held, or with nothing being composed, the key stays the application's. A scheme that locks the caret (`locks_caret`: Korean, Zhuyin, Vietnamese) has no segments, so there the chord finishes the composition below and the application edits by word as usual.
    //
    // Shift+左右与 Ctrl+左右相同，按音节移动拼音光标：macOS 默认把 Ctrl+左右分给调度中心切换桌面，按键到不了输入法，Shift+左右是这里实际能用的那一个（Rime 也把 Shift+方向键当 Ctrl+方向键）。两者在所有方案里都发分段命令，不再把全拼、双拼换成逐个字母的光标命令：共享 runtime 在横排候选窗里会把高亮不在首选时的字母光标命令换成移动候选，分段命令从不被换，所以停在哪个候选上它们都编辑拼音。Shift+Backspace 不在其中，仍是下面的普通退格。没有组字时两者照旧是应用的选择文字和按词移动。
    const NSEventModifierFlags segmentChord = event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagShift |
                                                                    NSEventModifierFlagOption | NSEventModifierFlagCommand);
    const BOOL segmentCaretKey = (event.keyCode == 123 || event.keyCode == 124) &&
        (segmentChord == NSEventModifierFlagControl || segmentChord == NSEventModifierFlagShift);
    const BOOL segmentBackspaceKey = event.keyCode == 51 && segmentChord == NSEventModifierFlagControl;
    if ((segmentCaretKey || segmentBackspaceKey) &&
        !MSIMESchemeTrait(_viewState, msime::mac::scheme::LocksCaret) && _session && _activeClient && _viewState.hasComposition()) {
        const uint32_t segment = segmentBackspaceKey ? MSIME_BACKSPACE_SEGMENT
            : event.keyCode == 123 ? MSIME_MOVE_LEFT_SEGMENT : MSIME_MOVE_RIGHT_SEGMENT;
        // An Engine failure leaves the composition alone rather than finishing it below: the user asked to edit what is there, not to commit it.
        [self dispatchCommand:segment];
        return YES;
    }
    if (event.modifierFlags & (NSEventModifierFlagCommand | NSEventModifierFlagControl | NSEventModifierFlagOption)) {
        [self dispatchCommand:MSIME_FINISH_COMPOSITION];
        return NO;
    }
    uint32_t command = UINT32_MAX;
    [self ensureAppearance];
    // 引擎只在完整双拼后公布 Tab；显式辅助码优先于普通候选翻页和释义列循环。
    if (!glossSensePageWasActive && event.keyCode == 48 && !(event.modifierFlags & NSEventModifierFlagShift) &&
        MSIMESpellingSymbolString(_viewState, @"\t")) {
        if ([_session respondsToSelector:@selector(typeASCIISnapshot:shift:error:)]) {
            const ShurufaTransitionSnapshot *snapshot = [_session typeASCIISnapshot:'\t' shift:NO error:nil];
            if (snapshot && snapshot->handled) {
                [self applyTransitionSnapshot:snapshot overrideCommit:nil];
                return YES;
            }
        } else {
            NSDictionary *transition = [_session typeASCII:'\t' shift:NO error:nil];
            if (MSIMEStrictBoolean(transition[@"handled"])) {
                [self apply:transition];
                return YES;
            }
        }
    }
    if (_panel.isVisible && event.keyCode == 48 &&
        !(event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand))) {
        if ([self cycleArmedGlossColumnBackwards:(event.modifierFlags & NSEventModifierFlagShift) != 0]) {
            [self renderCandidates];
            return YES;
        }
        // 打开展开候选后，Tab 直接进入多行网格；网格里的 Tab 仍由
        // handleExpandedCandidateEvent: 滚过可见的几行。偏好关闭或只有一页时，Tab 保持翻页。
        if (!(event.modifierFlags & NSEventModifierFlagShift) &&
            _appearance.expandCandidates && [self openExpandedCandidates]) return YES;
        if ([_appearance navigationEnabled:@"tab"]) {
            [self dispatchCommand:(event.modifierFlags & NSEventModifierFlagShift) ? MSIME_PREVIOUS_PAGE : MSIME_NEXT_PAGE];
            return YES;
        }
        // With candidates visible, a disabled Tab paging shortcut is still owned by the IME:
        // do not let the host application move focus away from the composition.
        return YES;
    }
    // Candidate paging is keyed by the physical ANSI key, matching Windows
    // even when the current keyboard layout produces a different glyph (or no
    // text at all). Unicode '+' is an Engine code-sequence character, the
    // Japanese minus/equal keys remain composition input, and so does any key
    // the Engine spells with (expression mode's '-' and '.').
    const int physicalPageDirection = msime::mac::PhysicalCandidatePageDirection(event.keyCode);
    const BOOL japaneseMinusEqual = msime::mac::IsJapaneseMinusEqualKey(
        MSIMEViewScheme(_viewState), [_viewState.localMode isEqual:@"temporary_japanese"],
        event.keyCode, 0);
    const BOOL engineInputKey = ([_viewState.localMode isEqual:@"unicode"] &&
        [event.charactersIgnoringModifiers isEqual:@"+"]) || MSIMESpellingSymbolString(_viewState, event.charactersIgnoringModifiers);
    // Word-to-character owns whichever pair it is bound to, and paging does not get to take it. The two are
    // alternatives, which applyCloudSettingsSnapshot: already says by refusing a snapshot whose paging
    // preset collides - but that only guards the cloud path, so a locally enabled bracket or minus paging
    // shortcut quietly won here and left 以词定字 doing nothing at all, with nothing to say why.
    //
    // The test is the one the word-to-character branch below applies, so the key is excluded from paging
    // exactly when that branch will claim it: whichever way it goes, the keystroke has an owner.
    const BOOL wordCharacterOwnsKey = [self wordCharacterClaimsEvent:event];
    // The marks among the paging keys (- = [ ] , .) are punctuation while an opened candidate list (the Korean Hanja list) shows, as they are with no list: the Engine closes it and writes the composition with the mark, so a mark typed after a syllable never turns a page instead. Page Up and Page Down still page.
    const BOOL candidateListMark = MSIMECandidateListOpen(_viewState) && event.keyCode != 116 && event.keyCode != 121;
    if (_panel.isVisible && !(event.modifierFlags & NSEventModifierFlagShift) &&
        physicalPageDirection != 0 && !japaneseMinusEqual && !engineInputKey && !wordCharacterOwnsKey && !candidateListMark) {
        const BOOL previous = physicalPageDirection < 0 &&
            ((event.keyCode == 27 && [_appearance navigationEnabled:@"minus_equal"]) ||
             (event.keyCode == 33 && [_appearance navigationEnabled:@"brackets"]) ||
             (event.keyCode == 43 && [_appearance navigationEnabled:@"comma_period"]) ||
             (event.keyCode == 116 && [_appearance navigationEnabled:@"page_up_down"]));
        const BOOL next = physicalPageDirection > 0 &&
            ((event.keyCode == 24 && [_appearance navigationEnabled:@"minus_equal"]) ||
             (event.keyCode == 30 && [_appearance navigationEnabled:@"brackets"]) ||
             (event.keyCode == 47 && [_appearance navigationEnabled:@"comma_period"]) ||
             (event.keyCode == 121 && [_appearance navigationEnabled:@"page_up_down"]));
        if (previous || next) {
            [self dispatchCommand:previous ? MSIME_PREVIOUS_PAGE : MSIME_NEXT_PAGE];
            return YES;
        }
    }
    if (_panel.isVisible && !(event.modifierFlags & NSEventModifierFlagShift)) {
        NSString *characters = event.charactersIgnoringModifiers;
        if (characters.length == 1) {
            const unichar character = [characters characterAtIndex:0];
            // In temporary Japanese mode '-' and '=' are composition input (the
            // Windows TSF path gives these keys to the engine as well). Do not
            // consume them as candidate paging shortcuts while the panel is up.
            if (msime::mac::IsJapaneseMinusEqualKey(MSIMEViewScheme(_viewState),
                                                     [_viewState.localMode isEqual:@"temporary_japanese"],
                                                     event.keyCode, static_cast<char>(character))) {
                // Fall through to the normal engine dispatch below.
            } else {
            BOOL brackets = [[_appearance wordCharacterOptions][@"keys"] isEqual:@"brackets"];
            BOOL first = character == (brackets ? '[' : '-');
            // The same claim the paging exclusion above asks about, so the two cannot disagree about who
            // owns the key and leave it doing nothing.
            if ([self wordCharacterClaimsEvent:event]) {
                if (!_viewState.valid || !_viewState.focused || _viewState.candidates.empty()) return YES;
                for (const auto &candidate : _viewState.candidates) {
                    if (!candidate.highlighted) continue;
                    NSDictionary *identifier = candidate.idDictionary();
                    if (!MSIMECurrentCandidateIdentity(identifier, _viewState)) return YES;
                    const uint64_t generation = candidate.generation;
                    const NSUInteger index = candidate.index;
                    NSString *candidateText = candidate.text ?: @"";
                    NSString *phrasePrefix = _viewState.phrasePrefix ?: @"";
                    if ([_session respondsToSelector:@selector(selectEdgeSnapshotGeneration:index:edge:error:)]) {
                        const ShurufaTransitionSnapshot *selected = [_session selectEdgeSnapshotGeneration:generation
                                                                                                     index:index
                                                                                                      edge:first ? MSIME_FIRST_HAN : MSIME_LAST_HAN
                                                                                                     error:nil];
                        if (selected) {
                            [self applyTransitionSnapshot:selected
                                           overrideCommit:nil
                                        wordCandidateText:candidateText
                                             phrasePrefix:phrasePrefix
                                                wordFirst:first
                                              hasWordEdge:YES];
                        }
                    } else {
                        NSDictionary *selected = [_session selectEdgeGeneration:generation index:index edge:first ? MSIME_FIRST_HAN : MSIME_LAST_HAN error:nil];
                        if (selected) {
                            NSMutableDictionary *annotated = [selected mutableCopy];
                            annotated[@"word_character_candidate"] = candidateText;
                            annotated[@"word_character_prefix"] = phrasePrefix;
                            annotated[@"word_character_first"] = @(first);
                            [self apply:annotated];
                        }
                    }
                    return YES; // Unsupported/stale candidates must not turn into punctuation.
                }
                return YES;
            }
            }
        }
    }
    // 「展开候选」打开时横排候选窗的 ↓ 展开多行网格（#7307）；只有一页、竖排、候选列表开着时 openExpandedCandidates 返回 NO，↓ 照下面的规则移高亮。注音组字时 ↓ 打开候选列表（MSIMECandidateListDownKey）在前面，优先于这里。
    if (_panel.isVisible && event.keyCode == 125 && !(event.modifierFlags & NSEventModifierFlagShift) && [self openExpandedCandidates]) return YES;
    if (MSIMEEditionIsShurufa() && _panel.isVisible && !(event.modifierFlags & NSEventModifierFlagShift) &&
        (event.keyCode == kVK_LeftArrow || event.keyCode == kVK_RightArrow ||
         event.keyCode == kVK_UpArrow || event.keyCode == kVK_DownArrow)) {
        const BOOL previous = event.keyCode == kVK_LeftArrow || event.keyCode == kVK_UpArrow;
        [self dispatchCommand:previous ? MSIME_PREVIOUS_CANDIDATE : MSIME_NEXT_CANDIDATE];
        return YES;
    }
    // 拼音普通候选横排时：首选上左键编辑光标，右键先回到拼音末尾，再选择候选；不在首选时左右键只移动候选。首选是全部候选的第一个，翻到后面一页的第一个不算。竖排时左右键与候选方向垂直，只编辑光标，高亮由上下键移动。展开网格和已打开的候选列表先由各自的导航处理。
    // 打开「左 / 右（移动候选项）」时这里不接管：同一条规则由共享 runtime 按同一份偏好判断，五个宿主的口径才一致。
    if (_panel.isVisible && (event.keyCode == 123 || event.keyCode == 124) &&
        !(event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand)) &&
        !MSIMECandidateListOpen(_viewState) && MSIMESchemeTrait(_viewState, msime::mac::scheme::EditsSentence) &&
        [_appearance navigationEnabled:@"arrows"] && ![_appearance navigationEnabled:@"left_right"]) {
        // 双拼的预编辑以音节为编辑单位。左右键在音节内部也按一个音节移动，避免高亮候选变化后把按键误解释成选词，导致用户只能删掉整句重拼。
        if (MSIMEViewScheme(_viewState) == msime::mac::scheme::Shuangpin) {
            const uint32_t segment = event.keyCode == 123 ? MSIME_MOVE_LEFT_SEGMENT : MSIME_MOVE_RIGHT_SEGMENT;
            [self dispatchCommand:segment];
            return YES;
        }
        NSUInteger visibleSlot = 0;
        NSUInteger highlighted = NSNotFound;
        for (const auto &candidate : _viewState.candidates) {
            if (candidate.isPlaceholder()) continue;
            if (candidate.highlighted) { highlighted = visibleSlot; break; }
            ++visibleSlot;
        }
        NSString *editing = _viewState.editingText;
        const NSUInteger caret = _viewState.caretPosition;
        const BOOL caretValid = editing.length && caret <= editing.length;
        const NSUInteger page = _viewState.page;
        if (!_appearance.vertical && highlighted != NSNotFound && caretValid) {
            const BOOL left = event.keyCode == 123;
            const BOOL edit = page == 0 && highlighted == 0 && (left || caret < editing.length);
            const uint32_t command = edit ? (left ? MSIME_MOVE_LEFT : MSIME_MOVE_RIGHT)
                                         : (left ? MSIME_PREVIOUS_CANDIDATE : MSIME_NEXT_CANDIDATE);
            [self dispatchCommand:command];
            return YES;
        }
    }
    // 其它方案仍由上下选择、左右编辑；韩文汉字和注音列表的四个方向键都选择候选。
    if (_panel.isVisible && event.keyCode >= 123 && event.keyCode <= 126) {
        const BOOL horizontal = event.keyCode == 123 || event.keyCode == 124;
        if (MSIMECandidateListOpen(_viewState) || (!horizontal && [_appearance navigationEnabled:@"arrows"])) {
            const BOOL backwards = event.keyCode == 123 || event.keyCode == 126;
            [self dispatchCommand:backwards ? MSIME_PREVIOUS_CANDIDATE : MSIME_NEXT_CANDIDATE];
            return YES;
        }
    }
    // A scheme that spells with Space (Zhuyin's first tone while a syllable is pending) gets it as a character: the key is input, not the selection of a highlighted item.
    const BOOL spaceIsSpelling = event.keyCode == 49 && MSIMESpellingSymbol(_viewState, ' ');
    // Provisional 窗口内面板仍停留在上一代候选（等待 ~28 ms 防闪或神经回包），此时按空格必须走下方
    // MSIME_COMMIT_CANDIDATE（由 Rust 会话层执行热态有界等待并提交当前代次首选），不能按旧面板身份吞键。
    if (_panel.isVisible && !_viewState.provisional && event.keyCode == 49 && !spaceIsSpelling &&
        !(event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl |
                                 NSEventModifierFlagOption | NSEventModifierFlagCommand))) {
        // Space commits the highlighted item shown by the panel. Keep the
        // identity fence symmetric with numeric and mouse selection.
        NSDictionary *identifier = MSIMERenderedHighlightedCandidateIdentity(_panel);
        if (identifier) {
            if (!MSIMECurrentCandidateIdentity(identifier, _viewState)) return YES;
            if (_armedGlossColumn > 0 && [self commitHighlightedGlossColumn:_armedGlossColumn client:sender]) return YES;
            uint64_t generation = 0; NSUInteger index = 0;
            if (!MSIMEStrictUnsignedInteger(identifier[@"generation"], &generation) || !MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &index)) return YES;
            [self dispatchSelectGeneration:generation index:index];
            return YES;
        }
    }
    if (_panel.isVisible && (event.keyCode == 36 || event.keyCode == 76) &&
        !(event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl |
                                 NSEventModifierFlagOption | NSEventModifierFlagCommand)) &&
        _armedGlossColumn > 0 && [self commitHighlightedGlossColumn:_armedGlossColumn client:sender]) return YES;
    // NSTextInputClient 没有设置光标的接口，所以跨过后半个是把光标后已知的那个后半个整个替换掉，光标随之后移，字不会重复。
    //
    // 只在本宿主记着自己补过的后半个时才读光标后的字：什么都没记时读回来既跨不过、也没有可清的，却要在每个键上向应用同步来回两次（`selectedRange` 和 `attributedSubstringFromRange:`）。
    if (_appearance.pairedPunctuation && !_pairedPunctuation.empty() && event.characters.length == 1 &&
        !MSIMESpellingSymbolString(_viewState, event.characters) &&
        !(event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand))) {
        NSString *following = MSIMETextClientFollowingCharacter((id<MSIMETextClient>)sender);
        NSString *typed = event.characters;
        if (!following && !_pairedPunctuation.empty()) _pairedPunctuation.clear();
        if (following.length == 1 && [following isEqualToString:typed] &&
            msime::mac::paired_closing_should_skip(_pairedPunctuation, typed.UTF8String, following.UTF8String, YES,
                                                    event.modifierFlags, NSEventModifierFlagControl,
                                                    NSEventModifierFlagOption, NSEventModifierFlagCommand)) {
            NSRange selected = [(id<MSIMETextClient>)sender selectedRange];
            [(id<MSIMETextClient>)sender insertText:typed replacementRange:NSMakeRange(selected.location, 1)];
            MSIMERecordTypingStatistics(_preferencesDirectory ?: [self runtimeOptions][@"preferences_directory"], typed,
                                        MSIMEResolveTypingSource(nil, _viewState, MSIMEStatisticsHostOptions(_session), _appearance.englishMode));
            return YES;
        }
    }
    // 中文方案把 Shift+[ / Shift+] 的大括号映射成「」，和引号键的样式并存。
    // 注音自己使用这两个键，独立英文和本地模式也保留引擎的原始规则。
    if (([event.characters isEqualToString:@"{"] || [event.characters isEqualToString:@"}"]) &&
        (event.modifierFlags & NSEventModifierFlagShift) && MSIMESchemeRulesApply(_viewState) &&
        msime::mac::scheme::HostSmartPunctuation(MSIMEViewScheme(_viewState)) &&
        _appearance.runtimeChinesePunctuation && ![_appearance.punctuationLock isEqual:@"english"] &&
        !(_capsLock && MSIMEStrictBoolean(MSIMEStatisticsHostOptions(_session)[@"preferences"][@"caps_lock_ascii_punctuation"])) &&
        !MSIMESpellingSymbolString(_viewState, event.characters)) {
        const BOOL opening = [event.characters isEqualToString:@"{"];
        const BOOL composing = _viewState.hasComposition();
        // 只跨过本宿主补出的、并且仍在光标后的右引号，不触碰文档原有的文字。
        if (!opening && !composing && _appearance.pairedPunctuation && _pairedPunctuation.matches("」")) {
            NSString *following = MSIMETextClientFollowingCharacter((id<MSIMETextClient>)sender);
            if (msime::mac::paired_closing_should_skip(_pairedPunctuation, "」", following.UTF8String ?: "",
                    following != nil, event.modifierFlags, NSEventModifierFlagControl,
                    NSEventModifierFlagOption, NSEventModifierFlagCommand)) {
                NSRange selected = [(id<MSIMETextClient>)sender selectedRange];
                [(id<MSIMETextClient>)sender insertText:@"」" replacementRange:NSMakeRange(selected.location, 1)];
                MSIMERecordTypingStatistics(_preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"], @"」",
                    MSIMEResolveTypingSource(nil, _viewState, MSIMEStatisticsHostOptions(_session), _appearance.englishMode));
                return YES;
            }
        }
        const uint8_t ascii = opening ? '{' : '}';
        NSString *mark = opening ? @"「" : @"」";
        // 右引号尚挂在 marked text 时，这次上屏会带上它，不再追加第二个。
        if (!opening && [_pendingPairedClosing isEqualToString:mark]) mark = @"";
        if ([_session respondsToSelector:@selector(punctuationSnapshot:preceding:hasPreceding:literalASCII:error:)]) {
            const ShurufaTransitionSnapshot *engine = [_session punctuationSnapshot:ascii preceding:0 hasPreceding:NO literalASCII:YES error:nil];
            // 会话失败时保留组字，不能把标点直接写进应用而丢掉尚未提交的拼音。
            if (!engine) return YES;
            NSString *overrideCommit = mark;
            if (engine->handled) {
                NSString *commit = engine->has_commit ? shurufa::mac::SliceToNSString(engine->commit) : nil;
                if ([commit isKindOfClass:NSString.class] && [commit hasSuffix:event.characters])
                    overrideCommit = [[commit substringToIndex:commit.length - 1] stringByAppendingString:mark];
                else
                    overrideCommit = nil;
            }
            _punctuationKeyInFlight = ascii;
            if (opening) _hostOpenedClosing = @"」";
            [self applyTransitionSnapshot:engine overrideCommit:overrideCommit];
            _hostOpenedClosing = nil;
            return YES;
        }
        NSDictionary *engine = [_session punctuationASCII:ascii error:nil];
        // 会话失败时保留组字，不能把标点直接写进应用而丢掉尚未提交的拼音。
        if (!engine) return YES;
        NSMutableDictionary *transition = [engine mutableCopy];
        if (MSIMEStrictBoolean(engine[@"handled"])) {
            NSString *commit = engine[@"commit"];
            if ([commit isKindOfClass:NSString.class] && [commit hasSuffix:event.characters])
                transition[@"commit"] = [[commit substringToIndex:commit.length - 1] stringByAppendingString:mark];
        } else {
            transition[@"handled"] = @YES;
            transition[@"commit"] = mark;
        }
        _punctuationKeyInFlight = ascii;
        if (opening) _hostOpenedClosing = @"」";
        [self apply:transition];
        _hostOpenedClosing = nil;
        return YES;
    }
    // The reference sends `{` down its punctuation path and closes it with `}` (`_GetPairedPunctuationClosingFor`), whether or not a composition is live, and the Linux host does the same. The Engine answers `{` on its ASCII route: while composing it commits the candidate followed by `{`, and idle it leaves the key alone, so the host commits the opening mark itself. The mark is not in MSIMEPunctuationPairs because a symbol candidate that is exactly `{` is not paired by the reference.
    // A scheme outside the Chinese punctuation table (`uses_chinese_punctuation`: Korean, Vietnamese) ignores the Chinese punctuation switch, so its `{` is the Engine's plain ASCII mark. Zhuyin's Shift overlay turns `{` into 『 in the Engine whatever the switches say, so its `{` is the Engine's as well.
    if ([event.characters isEqualToString:@"{"] &&
        !(MSIMESchemeRulesApply(_viewState) && (!msime::mac::scheme::UsesChinesePunctuation(MSIMEViewScheme(_viewState)) ||
                                           MSIMEViewScheme(_viewState) == msime::mac::scheme::Zhuyin)) &&
        !(event.modifierFlags & (NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagCommand)) &&
        _appearance.runtimeChinesePunctuation && _appearance.pairedPunctuation && !_pendingPairedClosing &&
        !MSIMEPairedPunctuationExcludedHost()) {
        NSString *opening = _appearance.runtimeFullWidthInput ? @"｛" : @"{";
        NSString *closing = _appearance.runtimeFullWidthInput ? @"｝" : @"}";
        if ([_session respondsToSelector:@selector(punctuationSnapshot:preceding:hasPreceding:literalASCII:error:)]) {
            const ShurufaTransitionSnapshot *engine = [_session punctuationSnapshot:'{' preceding:0 hasPreceding:NO literalASCII:YES error:nil];
            if (engine) {
                NSString *overrideCommit = opening;
                if (engine->handled) {
                    NSString *commit = engine->has_commit ? shurufa::mac::SliceToNSString(engine->commit) : nil;
                    if ([commit isKindOfClass:NSString.class] && [commit hasSuffix:@"{"])
                        overrideCommit = [[commit substringToIndex:commit.length - 1] stringByAppendingString:opening];
                    else
                        overrideCommit = nil;
                }
                _hostOpenedClosing = closing;
                [self applyTransitionSnapshot:engine overrideCommit:overrideCommit];
                _hostOpenedClosing = nil;
                return YES;
            }
        }
        NSDictionary *engine = [_session punctuationASCII:'{' error:nil];
        NSMutableDictionary *transition = nil;
        if (MSIMEStrictBoolean(engine[@"handled"])) {
            transition = [engine mutableCopy];
            NSString *commit = engine[@"commit"];
            if ([commit isKindOfClass:NSString.class] && [commit hasSuffix:@"{"])
                transition[@"commit"] = [[commit substringToIndex:commit.length - 1] stringByAppendingString:opening];
        } else {
            transition = [NSMutableDictionary dictionaryWithObject:opening forKey:@"commit"];
            if ([engine[@"view"] isKindOfClass:NSDictionary.class]) transition[@"view"] = engine[@"view"];
            else if (_viewState.valid) transition[@"view"] = _viewState.toDictionary();
        }
        _hostOpenedClosing = closing;
        [self apply:transition];
        _hostOpenedClosing = nil;
        return YES;
    }
    // With its navigation binding off, a paging or arrow key is the application's and moves the caret, so it ends a composition that commits on blur (a Korean syllable, a Zhuyin or Vietnamese composition) the way Tab does below. A candidate list on screen keeps the key instead, as every candidate list does (the switch below swallows it), so the composition is not written out.
    const BOOL applicationNavigationKey = ((event.keyCode == 116 || event.keyCode == 121) && ![_appearance navigationEnabled:@"page_up_down"]) ||
        ((event.keyCode == 125 || event.keyCode == 126) && ![_appearance navigationEnabled:@"arrows"]);
    if (MSIMESchemeTrait(_viewState, msime::mac::scheme::CommitsOnBlur) && applicationNavigationKey && !_panel.isVisible && _viewState.hasEditableText())
        [self dispatchCommand:MSIME_FINISH_COMPOSITION];
    // Vim 模式（共享偏好 vim_mode_apps）：名单命中的前台应用里，无修饰、非长按、无组字的 Esc
    // 会原样回到应用（MSIME_CANCEL 无事可取消），搭车把水杉切到英文，Vim 用户立刻能按 hjkl。
    // 判定必须在下面派发 MSIME_CANCEL 之前：组字开着时这次 Esc 归引擎取消组字，不能既取消又切。
    const BOOL vimListed = event.keyCode == 53 &&
        [_appearance vimModeEnabledForApplication:[sender respondsToSelector:@selector(bundleIdentifier)]
                                                       ? [sender bundleIdentifier] : nil];
    if (msime::mac::VimModeEscapeSwitchesToEnglish(
            event.keyCode, event.isARepeat,
            (event.modifierFlags & (NSEventModifierFlagShift | NSEventModifierFlagControl |
                                    NSEventModifierFlagOption | NSEventModifierFlagCommand)) != 0,
            _viewState.hasComposition(),
            vimListed, _appearance.englishMode))
        [self setEnglishInputMode:YES];
    switch (event.keyCode) {
        case 48:
            // With no candidate panel on screen (the caret rect was invalid or there is no screen to show it on) Tab and Shift+Tab are the application's, whatever navigation.tab says. A composition still being edited is finished first, for every scheme and not only a Korean syllable, so the key does not move focus away from marked text that would then dangle in the old field. With nothing composed the session is left alone.
            if (_viewState.hasComposition())
                [self dispatchCommand:MSIME_FINISH_COMPOSITION];
            return NO;
        case 51: command = MSIME_BACKSPACE; break;
        // With an opened candidate list (the Korean Hanja list) showing, Return chooses the highlighted Hanja, as Space does; only the session knows the highlight, so the command is the candidate one (msime_client.h). Otherwise Return writes the syllable out and breaks the line.
        case 36: case 76: command = MSIMECandidateListOpen(_viewState) ? MSIME_COMMIT_CANDIDATE : MSIME_COMMIT_RAW; break;
        case 53: [self flushPendingPairedClosing]; _pairedPunctuation.clear(); command = MSIME_CANCEL; break;
        case 49: if (!spaceIsSpelling) command = MSIME_COMMIT_CANDIDATE; break;
        case 123: command = MSIME_MOVE_LEFT; break;
        case 124: command = MSIME_MOVE_RIGHT; break;
        // Home 分两段：候选窗开着时高亮不在首选就回到首选，已在首选才把光标移到开头（MSIME_FIRST_CANDIDATE_OR_HOME，由共享 runtime 判断）。
        case 115: command = _panel.isVisible ? MSIME_FIRST_CANDIDATE_OR_HOME : MSIME_MOVE_HOME; break;
        case 119: command = _panel.isVisible ? MSIME_LAST_CANDIDATE : MSIME_MOVE_END; break;
        case 117: command = MSIME_DELETE_FORWARD; break;
        case 116: if (![_appearance navigationEnabled:@"page_up_down"]) return _panel.isVisible; command = MSIME_PREVIOUS_PAGE; break;
        case 121: if (![_appearance navigationEnabled:@"page_up_down"]) return _panel.isVisible; command = MSIME_NEXT_PAGE; break;
        case 126: if (![_appearance navigationEnabled:@"arrows"]) return _panel.isVisible; command = MSIME_PREVIOUS_CANDIDATE; break;
        case 125: if (![_appearance navigationEnabled:@"arrows"]) return _panel.isVisible; command = MSIME_NEXT_CANDIDATE; break;
    }
    BOOL transitionOK = NO;
    BOOL transitionHandled = NO;
    NSString *punctuationCommit = nil;
    const BOOL isPunctuationASCII = (command == UINT32_MAX && event.characters.length == 1 &&
        MSIMEASCIIPunctuation([event.characters characterAtIndex:0]));
    if (command != UINT32_MAX) {
        transitionOK = [self dispatchCommand:command handled:&transitionHandled];
    } else if (event.characters.length == 1 && [event.characters characterAtIndex:0] <= 127) {
        const BOOL shift = (event.modifierFlags & NSEventModifierFlagShift) != 0;
        unichar typed = [event.characters characterAtIndex:0];
        if (MSIMESchemeTrait(_viewState, msime::mac::scheme::FoldsLetterCase)) typed = (unichar)msime::mac::KoreanKeyLetter((char)typed, shift);
        _punctuationKeyInFlight = MSIMEASCIIPunctuation(typed) ? typed : 0;
        [self syncSurroundingContextForCharacter:typed client:(id<MSIMETextClient>)sender];
        if ([_session respondsToSelector:@selector(typeASCIISnapshot:shift:error:)]) {
            const ShurufaTransitionSnapshot *snapshot = [_session typeASCIISnapshot:(uint8_t)typed shift:shift error:nil];
            if (snapshot) {
                transitionOK = YES;
                transitionHandled = snapshot->handled ? YES : NO;
                if (isPunctuationASCII && snapshot->has_commit && snapshot->commit.len > 0) {
                    punctuationCommit = shurufa::mac::SliceToNSString(snapshot->commit);
                }
                [self applyTransitionSnapshot:snapshot overrideCommit:nil];
            }
        } else {
            NSDictionary *transition = [_session typeASCII:(uint8_t)typed shift:shift error:nil];
            if (transition) {
                transitionOK = YES;
                transitionHandled = MSIMEStrictBoolean(transition[@"handled"]);
                if (isPunctuationASCII && [transition[@"commit"] isKindOfClass:NSString.class]) {
                    punctuationCommit = transition[@"commit"];
                }
                [self apply:transition];
            }
        }
    }
    if (!transitionOK) { _punctuationKeyInFlight = 0; return NO; }
    // A punctuation key is the only route that arms the space conversion, as in the reference, whose punctuation handler is the one caller: a candidate picked with Space or a digit never arms, even when its text ends in a mark.
    if (isPunctuationASCII)
        [self noteCommittedChinesePunctuationString:punctuationCommit key:[event.characters characterAtIndex:0] client:sender];
    if (transitionHandled) return YES;
    // Match Apple: Engine gets first refusal, then finish any composition before fallback.
    if (_viewState.hasEditableText()) {
        // 拼音未结束时按标点要保留原始字母（例如 `Observable<`），不能在应用收到标点前提交首候选。
        const BOOL rawPunctuation = event.characters.length == 1 &&
            MSIMEASCIIPunctuation([event.characters characterAtIndex:0]) &&
            MSIMESchemeTrait(_viewState, msime::mac::scheme::EditsSentence);
        if (![self dispatchCommand:rawPunctuation ? MSIME_COMMIT_RAW : MSIME_FINISH_COMPOSITION]) return NO;
    }
    // A scheme that does not widen (`widens_full_width`: Korean, Vietnamese) writes half-width digits and punctuation whatever the width switch says, as host-api does for the Engine's own commits.
    if (!(MSIMESchemeRulesApply(_viewState) && !msime::mac::scheme::WidensFullWidth(MSIMEViewScheme(_viewState))) && _appearance.runtimeFullWidthInput &&
        !_viewState.hasEditableText() && event.characters.length == 1 &&
        msime::mac::IsFullWidthDirectCharacter([event.characters characterAtIndex:0], event.modifierFlags)) {
        const unichar converted = msime::mac::FullWidthCharacter([event.characters characterAtIndex:0]);
        NSString *fullWidthText = [NSString stringWithCharacters:&converted length:1];
        [(id<MSIMETextClient>)sender insertText:fullWidthText replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
        MSIMERecordTypingStatistics(_preferencesDirectory ?: [self runtimeOptions][@"preferences_directory"], fullWidthText,
                                    MSIMEResolveTypingSource(nil, _viewState, MSIMEStatisticsHostOptions(_session), _appearance.englishMode));
        return YES;
    }
    return NO;
}

// 与系统拼音一致：切换输入源（如轻点 Caps Lock）或系统要求结束组字时，已打的字母原样上屏，不替用户选候选。
- (void)commitShurufaRawComposition {
    if (!_session || !_viewState.hasEditableText()) return;
    [self dispatchCommand:MSIME_COMMIT_RAW];
}

- (void)commitComposition:(id)sender {
    if (sender != _activeClient || !_session) return;
    [self flushPendingPairedClosing];
    _pairedPunctuation.clear();
    [self resetSmartPunctuationState];
    [self clearSmartPunctuationSpaceConversion];
    [self clearSmartPunctuationSpaceRevert];
    // IMK 要求结束组字时，普通英文原样上屏；其他模式沿用失焦选词与反馈规则。
    if (_managedEnglishComposition) [self commitManagedEnglishRaw];
    else if (MSIMEEditionIsShurufa()) [self commitShurufaRawComposition];
    else [self dispatchCommand:MSIME_FINISH_ON_FOCUS_LOSS];
    // 光标即将离开刚上屏的文字。
    [self invalidateSmartPunctuationShadow];
}

- (MSIMEVoiceCommitOutcome)postVoiceText:(NSString *)text route:(const MSIMEVoiceCommitRoute &)route {
    return route.deliver(text);
}

- (MSIMEInputController *)currentVoiceDestination {
    MSIMEInputController *focused = MSIMEFocusedController;
    if (!focused || !focused->_activeClient) return nil;
    NSString *mode = [focused selectedSystemInputModeID];
    return [@[MSIMEChineseInputModeID, MSIMEShuangpinInputModeID, MSIMEEnglishInputModeID] containsObject:mode ?: @""] ? focused : nil;
}
- (BOOL)copyVoiceText:(NSString *)text {
    return MSIMEWriteVoiceClipboard(text, NSPasteboard.generalPasteboard);
}
static NSRange MSIMEVoiceReplacementRangeForClient(id client, BOOL replaceSelection) {
    if (replaceSelection && [client respondsToSelector:@selector(selectedRange)]) {
        @try {
            NSRange selected = [(id<IMKTextInput>)client selectedRange];
            if (selected.location != NSNotFound && selected.length > 0) {
                return selected;
            }
        } @catch (NSException *exception) {
            (void)exception;
        }
    }
    return NSMakeRange(NSNotFound, NSNotFound);
}
- (void)applyVoiceResult:(NSDictionary *)transition route:(const MSIMEVoiceCommitRoute &)route {
    [self applyVoiceResult:transition route:route polishFailed:NO replaceSelection:NO archiveMetadata:nil];
}
- (void)applyVoiceResult:(NSDictionary *)transition route:(const MSIMEVoiceCommitRoute &)route polishFailed:(BOOL)polishFailed {
    [self applyVoiceResult:transition route:route polishFailed:polishFailed replaceSelection:NO archiveMetadata:nil];
}
- (void)applyVoiceResult:(NSDictionary *)transition
                   route:(const MSIMEVoiceCommitRoute &)route
            polishFailed:(BOOL)polishFailed
        replaceSelection:(BOOL)replaceSelection
         archiveMetadata:(NSDictionary<NSString *, id> *)archiveMetadata {
    // 语音不修改引擎，桥接层只附当前 view；它仍携带本方案的简繁转换规则。
    if (!transition[@"commit_context"] && [transition[@"view"] isKindOfClass:NSDictionary.class]) {
        NSMutableDictionary *contextual = [transition mutableCopy];
        contextual[@"commit_context"] = transition[@"view"];
        transition = contextual;
    }
    NSString *text = transition[@"commit"];
    if ([text isKindOfClass:NSString.class] && text.length && _voiceService.archiveRecordingID) {
        if (archiveMetadata.count &&
            [MSIMEVoiceInputService.archive respondsToSelector:@selector(noteText:metadata:forRecording:)]) {
            [MSIMEVoiceInputService.archive noteText:text
                                            metadata:archiveMetadata
                                        forRecording:_voiceService.archiveRecordingID];
        } else {
            [MSIMEVoiceInputService.archive noteText:text forRecording:_voiceService.archiveRecordingID];
        }
    }
    if ([self ownsPersistentVoiceRecording] && [text isKindOfClass:NSString.class] && text.length) {
        [self detachVoiceMarkedText];
        MSIMEInputController *destination = [self currentVoiceDestination];
        BOOL targetWasNonEditable = NO;
        if (destination &&
            [self voiceTargetEditabilityForClient:destination->_activeClient] == MSIMEVoiceTargetEditableStateNotEditable) {
            destination = nil;
            targetWasNonEditable = YES;
        }
        MSIMEAppearancePreferences *outputAppearance = destination ? destination->_appearance : _appearance;
        if (outputAppearance.traditionalOutput && MSIMEScriptConversionApplies(transition[@"commit_context"]))
            text = MSIMEChineseOutputString(text, YES);
        if (destination) {
            if (destination->_session) [destination apply:[destination->_session command:MSIME_FINISH_COMPOSITION error:nil]];
            // 与普通上屏一致，先更新前文，避免同步客户端回调读到录音前的标点状态。
            [destination noteSmartPunctuationShadow:[text characterAtIndex:text.length - 1]];
            NSRange replacement = MSIMEVoiceReplacementRangeForClient(destination->_activeClient, replaceSelection);
            [(id<MSIMETextClient>)destination->_activeClient insertText:text replacementRange:replacement];
            destination->_clientHasMarkedText = NO;
            if (polishFailed) _voicePolishFallbackNotice = @"整理失败，已输入原文";
            else if (_voiceSalvageNotice.length) _voicePolishFallbackNotice = _voiceSalvageNotice;
            NSString *prefsDir = destination->_preferencesDirectory ?: MSIMEStatisticsHostOptions(destination->_session)[@"preferences_directory"];
            MSIMERecordTypingStatistics(prefsDir, text, msime::mac::TypingSource::Voice);
            [destination beginVoiceEditTrackingForText:text preferencesDirectory:prefsDir];
        } else {
            const BOOL copied = [self copyVoiceText:text];
            _lastUndeliveredVoiceText = [text copy];
            if ([self shouldShowVoiceRecoveryToast]) {
                // 底部居中已由毛玻璃预览卡片完整展示结果与“再次复制”，不再同时叠加 3 秒的纯文字“已复制”条。
                _voiceCopiedResult = nil;
                _voicePolishFallbackNotice = nil;
                NSString *toastTitle = nil;
                if (polishFailed) {
                    toastTitle = copied ? @"整理失败且焦点不可输入 · 已复制原文" : @"整理失败且焦点不可输入 · 请点击复制";
                } else if (targetWasNonEditable) {
                    toastTitle = copied ? @"当前位置不可输入 · 结果已复制" : @"当前位置不可输入 · 请点击复制";
                } else {
                    toastTitle = copied ? @"焦点已离开输入框 · 结果已复制" : @"焦点已离开输入框 · 请点击复制";
                }
                __weak MSIMEInputController *weakSelf = self;
                [[MSIMEVoiceRecoveryToast sharedToast] showUndeliveredText:text
                                                                     title:toastTitle
                                                                    copied:copied
                                                               copyHandler:^BOOL(NSString *copyTarget) {
                    MSIMEInputController *inner = weakSelf;
                    return inner ? [inner copyVoiceText:copyTarget] : MSIMEWriteVoiceClipboard(copyTarget, NSPasteboard.generalPasteboard);
                }];
            } else {
                _voiceCopiedResult = @(copied);
                if (polishFailed && copied) _voicePolishFallbackNotice = @"整理失败，已复制原文";
                else if (_voiceSalvageNotice.length && copied) _voicePolishFallbackNotice = @"麦克风断开 · 已保留并复制结果";
            }
        }
        _voiceSalvageNotice = nil;
        return;
    }
    if ([route.mode isEqual:@"tsf"] || ![text isKindOfClass:NSString.class] || !text.length) {
        if (replaceSelection && [text isKindOfClass:NSString.class] && text.length && _activeClient) {
            NSRange selected = MSIMEVoiceReplacementRangeForClient(_activeClient, YES);
            if (selected.location != NSNotFound && selected.length > 0) {
                NSString *replacementText = text;
                if (_appearance.traditionalOutput && MSIMEScriptConversionApplies(transition[@"commit_context"])) {
                    replacementText = MSIMEChineseOutputString(replacementText, YES);
                }
                [self noteSmartPunctuationShadow:[replacementText characterAtIndex:replacementText.length - 1]];
                [(id<MSIMETextClient>)_activeClient insertText:replacementText replacementRange:selected];
                _clientHasMarkedText = NO;
                NSString *prefsDir = _preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"];
                MSIMERecordTypingStatistics(prefsDir, replacementText, msime::mac::TypingSource::Voice);
                [self beginVoiceEditTrackingForText:replacementText preferencesDirectory:prefsDir];
                NSMutableDictionary *remaining = [transition mutableCopy];
                [remaining removeObjectForKey:@"commit"];
                [self apply:remaining];
                return;
            }
        }
        _typingSourceOverride = @(static_cast<NSInteger>(msime::mac::TypingSource::Voice));
        [self apply:transition];
        _typingSourceOverride = nil;
        if ([text isKindOfClass:NSString.class] && text.length) {
            NSString *prefsDir = _preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"];
            [self beginVoiceEditTrackingForText:text preferencesDirectory:prefsDir];
        }
        return;
    }
    if (_appearance.traditionalOutput && MSIMEScriptConversionApplies(transition[@"commit_context"]))
        text = MSIMEChineseOutputString(text, YES);
    // Posted voice text reaches the application without passing the shadow.
    [self invalidateSmartPunctuationShadow];
    if ([self postVoiceText:text route:route] == MSIMEVoiceCommitOutcome::unavailable) {
        _typingSourceOverride = @(static_cast<NSInteger>(msime::mac::TypingSource::Voice));
        [self apply:transition];
        _typingSourceOverride = nil;
        if ([text isKindOfClass:NSString.class] && text.length) {
            NSString *prefsDir = _preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"];
            [self beginVoiceEditTrackingForText:text preferencesDirectory:prefsDir];
        }
        return;
    }
    NSString *postedPrefsDir = _preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"];
    MSIMERecordTypingStatistics(postedPrefsDir, text, msime::mac::TypingSource::Voice);
    [self beginVoiceEditTrackingForText:text preferencesDirectory:postedPrefsDir];
    NSMutableDictionary *remaining = [transition mutableCopy];
    [remaining removeObjectForKey:@"commit"];
    [self apply:remaining];
}

- (void)flushPendingPairedClosing {
    // The closing mark lives in the marked text, so inserting it replaces that range rather than
    // adding a second one. Called wherever the composition is torn down: the pair the user opened
    // is always closed, never dropped along with the composition that was holding it open.
    NSString *closing = _pendingPairedClosing;
    _pendingPairedClosing = nil;
    if (!closing.length || !_activeClient) return;
    [(id<MSIMETextClient>)_activeClient insertText:closing
                                  replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    _pairedPunctuation.push(closing.UTF8String);
}

static NSDictionary *MSIMECommitContextDictionary(const ShurufaOutputContext &context) {
    return @{
        @"scheme": @(context.scheme),
        @"script_conversion": @(context.script_conversion),
        @"typing_statistics": @(context.typing_statistics),
        @"local_mode": shurufa::mac::SliceToNSString(context.local_mode, @"none"),
    };
}

- (NSString *)processCommitForPairedPunctuation:(NSString *)commitForTracking
                                 punctuationKey:(unichar)punctuationKey
                                         scheme:(int)scheme
                                  openedClosing:(NSString * __strong *)openedClosingOut {
    // The Engine commits the opening mark alone; closing the pair is this host's job, the way the
    // Windows TIP appends its closing mark and the Linux host appends its own. What differs is the
    // caret: those two move it back a character and IMK cannot. So the opening goes in as committed
    // text and the closing becomes the tail of the marked text until the composition ends.
    // With pairing on, every press of a quote key starts a fresh pair.
    //
    // The Engine alternates the quote keys - one press gives “, the next ”, because that is what a
    // host without pairing needs. A host that supplies the closing half itself never sends the
    // second press, so the alternation is left pointing at the closing mark and the *next* quote
    // the user types opens with ”. The reference rewrites it at the same point and says the same
    // thing: in paired mode every press starts a pair rather than following the toggle.
    //
    // A punctuation key typed during a composition finishes it, and the Engine commits the candidate and the mark as one string (`nihao(` gives `你好（`). The reference decides pairing from `punctuationStr.back()` inside its punctuation-key path, so the mark read here is the last character of such a commit. Any other commit is read whole, which keeps a candidate or phrase that merely ends in a quote or bracket exactly as it is.
    NSString *mark = commitForTracking;
    BOOL markEndsLongerCommit = NO;
    if (punctuationKey && [commitForTracking isKindOfClass:NSString.class] && commitForTracking.length) {
        const NSRange last = [commitForTracking rangeOfComposedCharacterSequenceAtIndex:commitForTracking.length - 1];
        if (last.location > 0) {
            mark = [commitForTracking substringWithRange:last];
            markEndsLongerCommit = YES;
        }
    }
    const BOOL quoteKey = !markEndsLongerCommit || punctuationKey == '"' || punctuationKey == '\'' ||
        punctuationKey == '{' || punctuationKey == '}';
    // 直角引号同理：」改回「，』改回『，但只在按的正是出这个字的引号键时改——corner 样式的 `'` `"`，以及契约第 3 版起非注音方案的 `{` `}`。注音的大千键位 `]` `}` 自己出」』（引擎的 `SHIFT_PUNCTUATION`），那是用户要打的后半个，改回前半个就成了再开一对。
    static NSDictionary<NSString *, NSString *> *const reopenings = @{@"”": @"“", @"’": @"‘", @"」": @"「", @"』": @"『"};
    const BOOL cornerQuoteMark = MSIMEIsCornerQuoteMark(mark);
    if (_appearance.pairedPunctuation && [mark isKindOfClass:NSString.class] && quoteKey && reopenings[mark] &&
        (!cornerQuoteMark || MSIMEKeyProducesCornerQuote(punctuationKey, [mark characterAtIndex:0], scheme)) &&
        !_pendingPairedClosing && !MSIMEPairedPunctuationExcludedHost()) {
        NSString *opening = reopenings[mark];
        commitForTracking = [[commitForTracking substringToIndex:commitForTracking.length - mark.length] stringByAppendingString:opening];
        mark = opening;
    }
    NSString *openedClosing = nil;
    // 注音 `[` `{` `]` `}` 出的「』」『同样不补后半个：这个宿主只在出这个字的键上配对，`]` `}` 到了引擎照样再出一个，补了就成了「内容」」。没有按键的整段上屏（例如候选）照旧按字补。
    const BOOL cornerQuoteFromOtherKey = cornerQuoteMark && punctuationKey &&
        !MSIMEKeyProducesCornerQuote(punctuationKey, [mark characterAtIndex:0], scheme);
    if ([mark isKindOfClass:NSString.class] && mark.length && !cornerQuoteFromOtherKey &&
        _appearance.pairedPunctuation && !_pendingPairedClosing && !MSIMEPairedPunctuationExcludedHost()) {
        for (NSArray<NSString *> *pair in MSIMEPunctuationPairs())
            if ([mark isEqualToString:pair[0]]) { openedClosing = pair[1]; break; }
    }
    NSString *hostOpenedClosing = _hostOpenedClosing;
    _hostOpenedClosing = nil;
    static NSDictionary<NSString *, NSString *> *const hostOpenings = @{@"}": @"{", @"｝": @"｛", @"」": @"「"};
    NSString *hostOpening = hostOpenedClosing ? hostOpenings[hostOpenedClosing] : nil;
    if (hostOpening && !openedClosing && [commitForTracking isKindOfClass:NSString.class] &&
        [commitForTracking hasSuffix:hostOpening] &&
        _appearance.pairedPunctuation && !_pendingPairedClosing && !MSIMEPairedPunctuationExcludedHost())
        openedClosing = hostOpenedClosing;
    // A pair this host closed is a pair the Engine still counts as open. Book title marks are the
    // ones that notice: 《 inside 《 is 〈, so an unbalanced count turns the next pair the user
    // types into 〈〉. Both halves of that nesting come from the same key, hence one call for both.
    if (openedClosing && ([mark isEqualToString:@"《"] || [mark isEqualToString:@"〈"]))
        [_session balancePairedPunctuationAfterAutoClose:'<' error:nil];
    if ([commitForTracking isKindOfClass:NSString.class] && commitForTracking.length >= 2 && _appearance.pairedPunctuation) {
        for (NSArray<NSString *> *pair in MSIMEPunctuationPairs())
            if ([commitForTracking hasPrefix:pair[0]] && [commitForTracking hasSuffix:pair[1]]) { _pairedPunctuation.push(pair[1].UTF8String); break; }
    }
    if (openedClosingOut) *openedClosingOut = openedClosing;
    return commitForTracking;
}

- (void)finishApplyWithNextState:(const shurufa::mac::ViewState &)nextState
                       nextValid:(BOOL)nextValid
                       isCurrent:(BOOL (^)(void))isCurrent
               glossViewSequence:(uint64_t)glossViewSequence {
    // 新派发或焦点已接管客户端，不能再发布旧视图。
    if (!isCurrent()) return;
    // A gloss that arrived during the write was merged into this same page, and the transition's copy of the page predates it.
    if (glossViewSequence == _glossViewSequence || !_viewState.valid || !_viewState.identityValid ||
        !nextValid || !nextState.identityValid || _viewState.session != nextState.session ||
        _viewState.generation != nextState.generation) {
        _viewState = nextValid ? nextState : shurufa::mac::ViewState{};
    }
    if (!_viewState.valid || _viewState.candidates.empty())
        _armedGlossColumn = 0;
    if (_viewState.valid && _viewState.provisional) {
        // 行内预编辑与提交文本已同步落盘（<2 ms）；候选窗等待 ~28 ms 防闪窗或提前回包的神经结果单次绘制。
        return;
    }
    [self renderCandidates];
    [self synchronizeCandidateServices];
}

- (void)applyTransitionSnapshot:(const ShurufaTransitionSnapshot *)snapshot overrideCommit:(NSString *)overrideCommit {
    [self applyTransitionSnapshot:snapshot
                   overrideCommit:overrideCommit
                wordCandidateText:nil
                     phrasePrefix:nil
                        wordFirst:NO
                      hasWordEdge:NO];
}

- (void)applyTransitionSnapshot:(const ShurufaTransitionSnapshot *)snapshot
                 overrideCommit:(NSString *)overrideCommit
              wordCandidateText:(NSString *)wordCandidate
                   phrasePrefix:(NSString *)phrasePrefix
                      wordFirst:(BOOL)wordFirst
                    hasWordEdge:(BOOL)hasWordEdge {
    ++_applySequence;
    if (!snapshot || !_activeClient) return;
    // An Engine answer replaces the view the sense page was drawn over, so the page goes with it.
    [self discardGlossSensePage];
    const shurufa::mac::ViewState nextState = shurufa::mac::ViewState::fromSnapshot(snapshot->view);
    const BOOL hasRawCommit = snapshot->has_commit && snapshot->commit.ptr != nullptr;
    NSString *rawCommit = overrideCommit ?: (hasRawCommit ? shurufa::mac::SliceToNSString(snapshot->commit) : nil);
    // A commit ends the conversion it belonged to; so does a composition that has gone away.
    if ([rawCommit isKindOfClass:NSString.class] || !nextState.hasEditableText()) {
        _japaneseConversionIndex = nil;
        _japaneseConversionReading = nil;
    }
    const auto sourceOverride = _typingSourceOverride
        ? static_cast<msime::mac::TypingSource>(_typingSourceOverride.integerValue)
        : msime::mac::TypingSource::Unknown;
    _typingSourceOverride = nil;
    const unichar punctuationKey = _punctuationKeyInFlight;
    _punctuationKeyInFlight = 0;
    const shurufa::mac::ViewState previousState = _viewState;
    const BOOL hasCommitContext = snapshot->has_commit_context ? YES : NO;
    const ShurufaOutputContext commitContext = snapshot->commit_context;
    const BOOL contextScriptConversion = hasCommitContext && commitContext.script_conversion;
    const BOOL countsAsTyping = !hasCommitContext || commitContext.typing_statistics;
    const uint64_t effect = snapshot->has_feedback ? snapshot->typing_effect : 0;
    const auto resolvedSource = sourceOverride == msime::mac::TypingSource::Unknown
        ? MSIMEResolveTypingSourceFromSnapshot(&commitContext, hasCommitContext, previousState, MSIMEStatisticsHostOptions(_session), _appearance.englishMode)
        : sourceOverride;
    NSDictionary *spacingContext = (hasCommitContext && [_appearance.crossCommitSpacing[@"enabled"] isEqual:@YES])
        ? MSIMECommitContextDictionary(commitContext) : nil;

    BOOL wordCharacterConverted = NO;
    NSString *commit = rawCommit;
    if (_appearance.traditionalOutput && hasWordEdge && [commit isKindOfClass:NSString.class] &&
        contextScriptConversion && [wordCandidate isKindOfClass:NSString.class] && [phrasePrefix isKindOfClass:NSString.class]) {
        NSString *convertedWord = MSIMEChineseOutputString([phrasePrefix stringByAppendingString:wordCandidate], YES);
        NSString *edge = MSIMEEdgeHanCharacter(convertedWord, wordFirst);
        if (edge.length) {
            commit = [MSIMEChineseOutputString(phrasePrefix, YES) stringByAppendingString:edge];
            wordCharacterConverted = YES;
        }
    }
    if ([commit isKindOfClass:NSString.class] && commit.length)
        _armedGlossColumn = 0;
    NSString *openedClosing = nil;
    NSString *commitForTracking = [self processCommitForPairedPunctuation:commit
                                                           punctuationKey:punctuationKey
                                                                   scheme:MSIMEViewScheme(_viewState)
                                                            openedClosing:&openedClosing];
    NSString *displayCommit = commitForTracking;
    if (_appearance.traditionalOutput && !wordCharacterConverted && contextScriptConversion &&
        [commitForTracking isKindOfClass:NSString.class]) {
        displayCommit = MSIMEChineseOutputString(commitForTracking, YES);
    }
    const uint64_t applySequence = _applySequence;
    const uint64_t inputSequence = _spacingInputSequence;
    const uint64_t glossViewSequence = _glossViewSequence;
    NSString *pendingClosing = _pendingPairedClosing;
    const MSIMEInlinePreeditStyle preeditStyle = MSIMESchemeTrait(nextState, msime::mac::scheme::AlwaysInlinePreedit)
        ? MSIMEInlinePreeditStylePinyin : _appearance.inlinePreeditStyle;
    if (pendingClosing && [displayCommit isKindOfClass:NSString.class]) {
        _pairedPunctuation.push(pendingClosing.UTF8String);
        _pendingPairedClosing = nil;
    }
    if (openedClosing) _pendingPairedClosing = openedClosing;
    if ([displayCommit isKindOfClass:NSString.class]) {
        NSString *landed = pendingClosing.length ? pendingClosing : displayCommit;
        if (landed.length) [self noteSmartPunctuationShadow:[landed characterAtIndex:landed.length - 1]];
    }
    id<MSIMETextClient> textClient = (id<MSIMETextClient>)_activeClient;
    BOOL (^isCurrent)(void) = ^BOOL {
        return applySequence == self->_applySequence &&
            textClient == (id<MSIMETextClient>)self->_activeClient;
    };
    NSString *effectiveOverride = (displayCommit != nil) ? displayCommit : overrideCommit;
    MSIMEApplyTransitionSnapshotWithCommitObserver(snapshot, effectiveOverride, textClient, preeditStyle, pendingClosing,
        &_clientHasMarkedText, isCurrent, ^(NSString *text) {
            BOOL (^canFormat)(void) = ^BOOL {
                return isCurrent() && inputSequence == self->_spacingInputSequence;
            };
            if (sourceOverride == msime::mac::TypingSource::Unknown && spacingContext)
                [self formatCrossCommitText:text context:spacingContext client:textClient isCurrent:canFormat];
        });
    if (isCurrent() && openedClosing) {
        _clientHasMarkedText = YES;
        [textClient setMarkedText:openedClosing selectionRange:NSMakeRange(0, 0)
                replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
    }
    if ([displayCommit isKindOfClass:NSString.class] && displayCommit.length) {
        if (countsAsTyping)
            MSIMERecordTypingStatistics(_preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"],
                                        displayCommit, resolvedSource);
        if (effect != 0 && effect <= UINT32_MAX)
            [self drawTypingEffect:static_cast<uint32_t>(effect) commit:YES];
    }
    [self finishApplyWithNextState:nextState nextValid:YES isCurrent:isCurrent glossViewSequence:glossViewSequence];
}

- (void)apply:(NSDictionary *)transition {
    ++_applySequence;
    if (!transition || !_activeClient) return;
    // An Engine answer replaces the view the sense page was drawn over, so the page goes with it.
    [self discardGlossSensePage];
    // A commit ends the conversion it belonged to; so does a composition that has gone away.
    if ([transition[@"commit"] isKindOfClass:NSString.class] ||
        ![transition[@"view"][@"editing_text"] length]) {
        _japaneseConversionIndex = nil;
        _japaneseConversionReading = nil;
    }
    const auto sourceOverride = _typingSourceOverride
        ? static_cast<msime::mac::TypingSource>(_typingSourceOverride.integerValue)
        : msime::mac::TypingSource::Unknown;
    _typingSourceOverride = nil;
    const unichar punctuationKey = _punctuationKeyInFlight;
    _punctuationKeyInFlight = 0;
    const shurufa::mac::ViewState previousState = _viewState;
    NSDictionary *wordCharacter = transition[@"word_character_candidate"] ? transition : nil;
    NSString *commit = transition[@"commit"];
    if (_appearance.traditionalOutput && wordCharacter && [commit isKindOfClass:NSString.class] &&
        MSIMEScriptConversionApplies(transition[@"commit_context"]) &&
        [wordCharacter[@"word_character_candidate"] isKindOfClass:NSString.class] &&
        [wordCharacter[@"word_character_prefix"] isKindOfClass:NSString.class]) {
        NSString *prefix = wordCharacter[@"word_character_prefix"];
        NSString *candidate = wordCharacter[@"word_character_candidate"];
        NSString *convertedWord = MSIMEChineseOutputString([prefix stringByAppendingString:candidate], YES);
        NSString *edge = MSIMEEdgeHanCharacter(convertedWord, MSIMEStrictBoolean(wordCharacter[@"word_character_first"]));
        if (edge.length) {
            NSMutableDictionary *converted = [transition mutableCopy];
            converted[@"commit"] = [MSIMEChineseOutputString(prefix, YES) stringByAppendingString:edge];
            converted[@"word_character_converted"] = @YES;
            transition = converted;
        }
    }
    NSString *commitForTracking = transition[@"commit"];
    if ([commitForTracking isKindOfClass:NSString.class] && commitForTracking.length)
        _armedGlossColumn = 0;
    NSString *openedClosing = nil;
    NSString *trackedCommit = [self processCommitForPairedPunctuation:commitForTracking
                                                       punctuationKey:punctuationKey
                                                               scheme:MSIMEViewScheme(_viewState)
                                                        openedClosing:&openedClosing];
    if (trackedCommit != commitForTracking) {
        NSMutableDictionary *reopened = [transition mutableCopy];
        reopened[@"commit"] = trackedCommit;
        transition = reopened;
    }
    NSDictionary *displayTransition = transition;
    if (_appearance.traditionalOutput && !MSIMEStrictBoolean(transition[@"word_character_converted"]) &&
        MSIMEScriptConversionApplies(transition[@"commit_context"]) && [transition[@"commit"] isKindOfClass:NSString.class]) {
        NSMutableDictionary *converted = [transition mutableCopy];
        converted[@"commit"] = MSIMEChineseOutputString(transition[@"commit"], YES);
        displayTransition = converted;
    }
    const uint64_t applySequence = _applySequence;
    const uint64_t inputSequence = _spacingInputSequence;
    const uint64_t glossViewSequence = _glossViewSequence;
    NSString *pendingClosing = _pendingPairedClosing;
    const shurufa::mac::ViewState nextState = shurufa::mac::ViewState::fromDictionary(displayTransition[@"view"]);
    // A Korean, Zhuyin or Vietnamese composition is always drawn inline, whatever the preedit display preference says: until Option+Return opens a candidate list there is no candidate window to show it in, and hidden it would be text the user cannot see being written.
    const MSIMEInlinePreeditStyle preeditStyle = MSIMESchemeTrait(nextState, msime::mac::scheme::AlwaysInlinePreedit)
        ? MSIMEInlinePreeditStylePinyin : _appearance.inlinePreeditStyle;
    // 沿用 #9405 的写后保护：同步回调读到新标点状态，外层不再覆盖较新的标记。

    if (pendingClosing && [displayTransition[@"commit"] isKindOfClass:NSString.class]) {
        _pairedPunctuation.push(pendingClosing.UTF8String);
        _pendingPairedClosing = nil;
    }
    if (openedClosing) _pendingPairedClosing = openedClosing;

    if ([displayTransition[@"commit"] isKindOfClass:NSString.class]) {
        NSString *landed = pendingClosing.length ? pendingClosing : displayTransition[@"commit"];
        if (landed.length) [self noteSmartPunctuationShadow:[landed characterAtIndex:landed.length - 1]];
    }
    id<MSIMETextClient> textClient = (id<MSIMETextClient>)_activeClient;
    BOOL (^isCurrent)(void) = ^BOOL {
        return applySequence == self->_applySequence &&
            textClient == (id<MSIMETextClient>)self->_activeClient;
    };
    MSIMEApplyTransitionWithCommitObserver(displayTransition, textClient, preeditStyle, pendingClosing,
        &_clientHasMarkedText, isCurrent, ^(NSString *text) {
            BOOL (^canFormat)(void) = ^BOOL {
                return isCurrent() && inputSequence == self->_spacingInputSequence;
            };
            if (sourceOverride == msime::mac::TypingSource::Unknown)
                [self formatCrossCommitText:text context:transition[@"commit_context"] client:textClient isCurrent:canFormat];
        });
    if (isCurrent() && openedClosing) {
        _clientHasMarkedText = YES;
        [textClient setMarkedText:openedClosing selectionRange:NSMakeRange(0, 0)
                replacementRange:NSMakeRange(NSNotFound, NSNotFound)];

    }
    if ([displayTransition[@"commit"] isKindOfClass:NSString.class] && [displayTransition[@"commit"] length]) {
        const auto source = sourceOverride == msime::mac::TypingSource::Unknown
            ? MSIMEResolveTypingSource(transition[@"commit_context"], previousState, MSIMEStatisticsHostOptions(_session), _appearance.englishMode)
            : sourceOverride;
        if (MSIMECommitCountsAsTyping(transition[@"commit_context"]))
            MSIMERecordTypingStatistics(_preferencesDirectory ?: MSIMEStatisticsHostOptions(_session)[@"preferences_directory"],
                                        displayTransition[@"commit"], source);
        // 上屏音由生命周期核心在派发里出，上屏特效随派发回复的 feedback 回来。只有派发类调用的回复带它：失焦上屏、语音转写和宿主自己合成的上屏都没有，所以既不出声也不画。安全输入时核心答 0。核心答 0 的上屏（失焦上屏、安全输入、特效关闭）不画：画一个 0 会把还在屏幕上的连击徽标撤掉。
        uint64_t effect = 0;
        NSDictionary *feedback = [transition[@"feedback"] isKindOfClass:NSDictionary.class] ? transition[@"feedback"] : nil;
        if (feedback && MSIMEStrictUnsignedInteger(feedback[@"typing_effect"], &effect) && effect != 0 && effect <= UINT32_MAX)
            [self drawTypingEffect:static_cast<uint32_t>(effect) commit:YES];
    }
    [self finishApplyWithNextState:nextState nextValid:nextState.valid isCurrent:isCurrent glossViewSequence:glossViewSequence];
}

// Everything that follows the candidates on screen. Each one returns at once when its request has not changed.
- (void)synchronizeCandidateServices {
    // Every refresh clears the session's translations and moves its generation, even when the page shows the same words, and the requests below no longer change with the generation, so none of them would put the glosses back. Re-apply what is held once per generation instead, as Windows re-applies its cached glosses on every redraw (event_listener.cpp ApplyCandidateTranslations); the successful apply comes back here with the generation recorded.
    NSNumber *generation = (_viewState.valid && _viewState.generationValid) ? @(_viewState.generation) : nil;
    if ([generation isKindOfClass:NSNumber.class] && ![generation isEqual:_translationAppliedGeneration] &&
        (_glossResults.count || _targetGlossResults.count || _customResults.count)) {
        // Recorded before the attempt, so a refused apply falls through to the synchronization below rather than retrying.
        _translationAppliedGeneration = generation;
        if ([self applyCandidateTranslationResults]) return;
    }
    const BOOL nested = _serviceSnapshotActive;
    if (!nested) {
        _serviceSnapshotActive = YES;
        _serviceSnapshotQueryLoaded = NO;
        _serviceSnapshotViewLoaded = NO;
        _serviceSnapshotQuery = nil;
        _serviceSnapshotView = nil;
    }
    [self synchronizeCloudCandidates];
    [self synchronizeCandidateGloss];
    [self synchronizeTargetGloss];
    [self synchronizeCustomTranslations];
    [self synchronizeAITranslations];
    [self synchronizePronunciation];
    if (!nested) [self invalidateServiceSnapshots];
}

// How the gloss lines of `candidate` are read, "\n"-joined parallel to them; empty while the switch is off or nothing is known.
- (NSString *)pronunciationForCandidate:(NSDictionary *)candidate {
    NSString *translation = CandidateTranslation(candidate);
    if (!_pronunciationTargets || !translation.length || ![candidate[@"text"] isKindOfClass:NSString.class]) return @"";
    // Romaji is cached per term: a page is drawn more than once per key and the tokenizer is not free.
    if (!_romajiCache || _romajiCache.count > 4096) _romajiCache = [NSMutableDictionary dictionary];
    NSMutableDictionary<NSString *, NSString *> *cache = _romajiCache;
    return MSIMEGlossPronunciation(candidate[@"text"], translation, _pronunciationTargets, _englishPronunciations,
                                   ^NSString *(NSString *term) {
                                       NSString *reading = cache[term];
                                       if (!reading) cache[term] = reading = MSIMEJapaneseRomaji(term);
                                       return reading;
                                   });
}

// Asks the shared table for the English texts on the current page it has not answered yet, then redraws. Nothing is
// asked while the switch is off; turning it on or off redraws the page with or without readings.
- (void)synchronizePronunciation {
    NSDictionary *query = (_activeClient && _session && !_focusPending) ? [self serviceSnapshotQuery] : nil;
    NSArray<NSString *> *targets = [query[@"candidate_pronunciation"] isEqual:@YES] ? MSIMETranslationTargets(query) : nil;
    if (!(targets == _pronunciationTargets || [targets isEqual:_pronunciationTargets])) {
        _pronunciationTargets = [targets copy];
        if (_activeClient) [self renderCandidates];
    }
    if (!targets) return;
    NSString *resources = [_session.hostOptions[@"resources"] copy];
    if (![resources isKindOfClass:NSString.class] || !resources.isAbsolutePath) return;
    if (!_englishPronunciations || _englishPronunciations.count > 4096) _englishPronunciations = [NSMutableDictionary dictionary];
    if (!_pronunciationPending) _pronunciationPending = [NSMutableSet set];
    NSMutableOrderedSet<NSString *> *missing = [NSMutableOrderedSet orderedSet];
    for (const auto &candidate : _viewState.candidates) {
        if (!candidate.text.length) continue;
        NSDictionary *candidateDict = candidate.rowDictionary();
        for (NSString *text in MSIMEGlossEnglishTexts(candidate.text, CandidateTranslation(candidateDict), targets))
            if (!_englishPronunciations[text] && ![_pronunciationPending containsObject:text]) [missing addObject:text];
    }
    if (!missing.count) return;
    [_pronunciationPending addObjectsFromArray:missing.array];
    if (!_pronunciationQueue) {
        _pronunciationQueue = [NSOperationQueue new];
        _pronunciationQueue.maxConcurrentOperationCount = 1;
        _pronunciationQueue.qualityOfService = NSQualityOfServiceUtility;
    }
    NSMutableArray *items = [NSMutableArray arrayWithCapacity:missing.count];
    for (NSString *text in missing) [items addObject:@{@"text": text, @"language": @"en"}];
    NSMutableDictionary *request = [@{@"generation": _viewState.valid ? @(_viewState.generation) : @0, @"items": items} mutableCopy];
    // 旧配置的活动词库可能位于用户目录，音标表仍随输入法 bundle 分发。
    NSString *supplementary = NSBundle.mainBundle.resourcePath;
    if (supplementary.isAbsolutePath) request[@"supplementary_resources"] = supplementary;
    NSArray<NSString *> *asked = missing.array;
    __weak MSIMEInputController *weakSelf = self;
    [_pronunciationQueue addOperationWithBlock:^{
        NSDictionary *result = [MSIMEClientSession pronunciationRequest:request resources:resources error:nil];
        NSMutableDictionary<NSString *, NSString *> *answered = [NSMutableDictionary dictionary];
        for (NSDictionary *entry in [result[@"pronunciations"] isKindOfClass:NSArray.class] ? result[@"pronunciations"] : @[])
            if ([entry[@"text"] isKindOfClass:NSString.class] && [entry[@"pronunciation"] isKindOfClass:NSString.class])
                answered[entry[@"text"]] = entry[@"pronunciation"];
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEInputController *current = weakSelf;
            if (!current) return;
            // A text the table does not know is remembered as having no reading, so it is not asked again every key.
            for (NSString *text in asked) {
                current->_englishPronunciations[text] = answered[text] ?: @"";
                [current->_pronunciationPending removeObject:text];
            }
            if (current->_activeClient && current->_pronunciationTargets && answered.count) [current renderCandidates];
        });
    }];
}

// The card is at most half the screen's visible width, as the Windows card is at most half its work area, and at least seven times the candidate font size, as the Windows card and the source skins' `min-width: 7em` are. Every row is laid out at the width it then gets: text, 辅助码 and gloss wider than their column wrap inside it and the row takes their height (CandidateItemLayout.h).
- (MSIMECandidatePageGeometry)candidatePageGeometry:(NSArray *)candidates font:(NSFont *)font glossFont:(NSFont *)glossFont
                                     showSelectedBar:(BOOL)showSelectedBar inset:(CGFloat)inset paging:(BOOL)paging
                                             visible:(NSRect)visible preeditWidth:(CGFloat)preeditWidth
                                        minimumWidth:(CGFloat)minimumWidth {
    return [self candidatePageGeometry:candidates font:font glossFont:glossFont showSelectedBar:showSelectedBar inset:inset
                                paging:paging visible:visible preeditWidth:preeditWidth minimumWidth:minimumWidth
                         reservesGloss:YES];
}

// `reservesGloss` 为 NO 时横排的一行不为还没到的释义预留高度，只按已有的内容排：展开的候选网格用它，网格里只有 Engine 当前那一页会请求释义，其余行一直空着那一行会把网格撑得很稀。
- (MSIMECandidatePageGeometry)candidatePageGeometry:(NSArray *)candidates font:(NSFont *)font glossFont:(NSFont *)glossFont
                                     showSelectedBar:(BOOL)showSelectedBar inset:(CGFloat)inset paging:(BOOL)paging
                                             visible:(NSRect)visible preeditWidth:(CGFloat)preeditWidth
                                        minimumWidth:(CGFloat)minimumWidth reservesGloss:(BOOL)reservesGloss {
    return [self candidatePageGeometry:candidates font:font glossFont:glossFont showSelectedBar:showSelectedBar inset:inset
        paging:paging visible:visible preeditWidth:preeditWidth minimumWidth:minimumWidth reservesGloss:reservesGloss pagerWidth:0];
}

// 列表翻页窄列占卡片总宽度，候选的测量与命中只使用扣除它的区域。
- (MSIMECandidatePageGeometry)candidatePageGeometry:(NSArray *)candidates font:(NSFont *)font glossFont:(NSFont *)glossFont
                                     showSelectedBar:(BOOL)showSelectedBar inset:(CGFloat)inset paging:(BOOL)paging
                                             visible:(NSRect)visible preeditWidth:(CGFloat)preeditWidth
                                        minimumWidth:(CGFloat)minimumWidth reservesGloss:(BOOL)reservesGloss pagerWidth:(CGFloat)pagerWidth {
    return [self candidatePageGeometry:candidates font:font glossFont:glossFont showSelectedBar:showSelectedBar inset:inset
        paging:paging visible:visible preeditWidth:preeditWidth minimumWidth:minimumWidth reservesGloss:reservesGloss
        pagerWidth:pagerWidth expandedColumns:0 sharedColumns:nullptr];
}

- (MSIMECandidatePageGeometry)candidatePageGeometry:(NSArray *)candidates font:(NSFont *)font glossFont:(NSFont *)glossFont
                                     showSelectedBar:(BOOL)showSelectedBar inset:(CGFloat)inset paging:(BOOL)paging
                                             visible:(NSRect)visible preeditWidth:(CGFloat)preeditWidth
                                        minimumWidth:(CGFloat)minimumWidth reservesGloss:(BOOL)reservesGloss pagerWidth:(CGFloat)pagerWidth
                                     expandedColumns:(NSUInteger)columnCount
                                       sharedColumns:(const std::vector<double> *)sharedColumns {
    MSIMECandidatePageGeometry geometry;
    const BOOL vertical = _appearance.vertical && columnCount == 0;
    const BOOL traditional = _appearance.traditionalOutput && MSIMEScriptConversionAppliesState(_viewState);
    // The fonts arrive at the window's scale already; the row's own paddings follow them.
    const CGFloat scale = MSIMECandidateScale(_appearance);
    const auto &layout = _candidateLayoutGeometry;
    const auto userStyle = [_appearance candidateWindowStyle];
    const CGFloat numberScale = layout && layout->numberScale.has_value()
        ? static_cast<CGFloat>(std::clamp(*layout->numberScale, 0.0, 2.0))
        : MSIMECandidateNumberScale;
    NSFont *numberFont = MSIMECandidateNumberFont(font, numberScale);
    NSMutableArray<NSString *> *texts = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *annotations = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *displays = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *translations = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *displayTranslations = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *readings = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *scriptAnnotations = [NSMutableArray arrayWithCapacity:candidates.count];
    NSMutableArray<NSString *> *tooltips = [NSMutableArray arrayWithCapacity:candidates.count];
    // A Korean Hanja's 훈음 (나라 이름 한) arrives as the row's annotation. Drawn inline at the candidate size it made every row several times as wide as its one character, so it goes on the small gloss line under the Hanja instead, always, with any translation on the lines after it. It stays out of `translation`, which is what the gloss commit chords write.
    const BOOL hanjaReadings = MSIMEKoreanHanjaListOpen(_viewState);
    const BOOL customRowHeight = layout && layout->itemExtraHeight.has_value();
    const CGFloat itemExtraHeight = customRowHeight ? MSIMESkinLengthValue(layout->itemExtraHeight, font.pointSize, scale) : 0.0;
    const CGFloat rowFloor = customRowHeight && !userStyle.itemPaddingVertical ? font.pointSize * 1.35 + itemExtraHeight : 0.0;
    const CGFloat rowPadding = customRowHeight || userStyle.itemPaddingVertical ? 0.0 : MSIMECandidateRowPadding * scale;
    CGFloat candidateRow = MAX(rowFloor, MSIMECandidateTextHeight(@"", font) + rowPadding);
    geometry.hasSkinLayout = layout.has_value() || userStyle.itemPaddingHorizontal.has_value();
    geometry.hidesNumber = numberScale == 0.0;
    CGFloat numberDigitWidth = 0;
    if (!vertical && !geometry.hidesNumber) {
        // 比例数字也按稳定的位数留栏，不能让 3 项变成 4 项时文字起点跟着最宽字形移动。
        for (NSUInteger digit = 0; digit < 10; ++digit) {
            NSString *label = [NSString stringWithFormat:@"%lu", (unsigned long)digit];
            numberDigitWidth = MAX(numberDigitWidth, [label sizeWithAttributes:@{NSFontAttributeName:numberFont}].width);
        }
    }
    CGFloat numberWidth = 0;
    NSUInteger index = 0;
    for (NSDictionary *candidate in candidates) {
        NSString *hint = MSIMEWubiCodeHint(candidate, _viewState, _wubiCodeHintEnabled);
        NSString *text = CandidateTextRun(candidate, traditional, _aiBadgeString, _fixedBadgeString);
        NSString *annotation = CandidateAnnotationRun(candidate, traditional, hint);
        NSString *reading = @"";
        if (hanjaReadings) {
            reading = annotation;
            annotation = @"";
        } else {
            annotation = MSIMEParenthesizedAnnotation(annotation);
        }
        NSString *translation = CandidateDisplayTranslation(candidate, _glossShowPos, _glossSingleSense);
        NSString *gloss = MSIMECandidateGlossRun(reading, translation);
        // The tooltip and the accessibility label keep the reading apart from the Hanja rather than running the two together.
        NSString *display = hanjaReadings ? (reading.length ? [NSString stringWithFormat:@"%@ %@", text, reading] : text)
                                          : CandidateDisplayWithWubiHint(candidate, traditional, hint, _aiBadgeString, _fixedBadgeString);
        [texts addObject:text];
        [annotations addObject:annotation];
        [displays addObject:display];
        [readings addObject:reading];
        [displayTranslations addObject:translation];
        // 插件注释原样显示，不做繁简转换：那是插件写的文字，不是要上屏的候选。
        NSString *scriptAnnotation = MSIMECandidateScriptAnnotationRun(candidate[@"script_annotation"]);
        [scriptAnnotations addObject:scriptAnnotation];
        // 按画出来的样子量：读音和插件注释都算进释义行，行的宽高才和按钮画的释义行一致。
        NSString *pronunciation = [self pronunciationForCandidate:candidate];
        [translations addObject:MSIMECandidateGlossWithScriptAnnotation(pronunciation.length
            ? MSIMECandidateGlossRun(reading, MSIMECandidateGlossDisplay(translation, pronunciation, nil, nil))
            : gloss, scriptAnnotation)];
        NSString *tip = MSIMECandidateGlossWithScriptAnnotation(gloss, scriptAnnotation);
        [tooltips addObject:tip.length ? [NSString stringWithFormat:@"%@\n%@", hanjaReadings ? text : display, tip] : display];
        NSString *number = [NSString stringWithFormat:@"%lu", (unsigned long)++index];
        const CGFloat labelWidth = geometry.hidesNumber ? 0.0 : vertical
            ? [number sizeWithAttributes:@{NSFontAttributeName:numberFont}].width
            : number.length * numberDigitWidth;
        numberWidth = MAX(numberWidth, labelWidth);
        geometry.numberWidths.push_back(ceil(layout ? MAX(9.0 * scale, numberWidth) : numberWidth));
        // Fallback glyphs can stand taller than the primary font; a one-line row is as tall as its tallest.
        candidateRow = MAX(candidateRow, MSIMECandidateTextHeight([text stringByAppendingString:annotation], font) + rowPadding);
    }
    geometry.texts = texts;
    geometry.annotations = annotations;
    geometry.displays = displays;
    geometry.translations = displayTranslations;
    geometry.readings = readings;
    geometry.scriptAnnotations = scriptAnnotations;
    geometry.tooltips = tooltips;
    if (columnCount && !geometry.hidesNumber) {
        NSString *number = [NSString stringWithFormat:@"%lu", (unsigned long)columnCount];
        numberWidth = MAX(numberWidth, number.length * numberDigitWidth);
    }
    const CGFloat textLeft = userStyle.itemPaddingHorizontal ? *userStyle.itemPaddingHorizontal * scale
        : layout && layout->numberInset ? MAX(5.0 * scale, MSIMESkinLengthValue(layout->numberInset, font.pointSize, scale))
                                       : MSIMECandidateTextLeft(showSelectedBar, scale);
    const CGFloat numberGap = layout ? MSIMESkinLengthValue(layout->numberGap, font.pointSize, scale, MSIMECandidateNumberGap * scale)
                                     : MSIMECandidateNumberGap * scale;
    geometry.textLeft = textLeft;
    geometry.textRight = layout ? MSIMESkinLengthValue(layout->itemPaddingRight, font.pointSize, scale, MSIMECandidateTextRight * scale)
                                : MSIMECandidateTextRight * scale;
    if (layout && _candidateLayoutFitContent) geometry.textRight = textLeft;
    if (userStyle.itemPaddingHorizontal) geometry.textRight = *userStyle.itemPaddingHorizontal * scale;
    // 来源在隐藏序号时仍保留至少 9 DIP 的序号栏，测量和绘制共用同一列起点。
    if (layout) numberWidth = MAX(9.0 * scale, numberWidth);
    geometry.contentLeft = textLeft + ceil(numberWidth) + numberGap;
    msime::candidate::CandidateLayoutMetrics metrics =
        MSIMECandidateLayoutMetrics(font, glossFont, candidateRow, geometry.contentLeft + geometry.textRight, scale);
    if (layout) {
        metrics.itemGap = MSIMESkinLengthValue(layout->itemGap, font.pointSize, scale, 0.0);
        metrics.columnGap = metrics.itemGap + (_candidateLayoutFitContent ? 0.0 : 6.0 * scale);
        metrics.justifyRows = layout->justifyRows.value_or(false);
        metrics.itemMinWidth = vertical ? 0.0 : MSIMESkinLengthValue(layout->itemMinWidth, font.pointSize, scale);
    }
    if (userStyle.itemPaddingVertical) metrics.paddingVertical = *userStyle.itemPaddingVertical * scale;
    if (userStyle.itemSpacing) metrics.itemGap = metrics.columnGap = *userStyle.itemSpacing * scale;
    geometry.columnGap = metrics.columnGap > 0 ? metrics.columnGap : metrics.itemGap;
    geometry.minimumColumnWidth = ceil(metrics.itemMinWidth);
    auto unnumberedMetrics = metrics;
    unnumberedMetrics.chrome -= ceil(numberWidth);
    unnumberedMetrics.itemMinWidth = 0;
    std::vector<msime::candidate::CandidateItemWidths> items;
    items.reserve(candidates.count);
    CGFloat natural = 0;
    for (NSUInteger i = 0; i < texts.count; ++i) {
        items.push_back(MSIMECandidateItemWidths(texts[i], annotations[i], translations[i], font, glossFont, scale));
        const CGFloat itemWidth = ceil(msime::candidate::CandidateItemNaturalWidth(items.back(), metrics, !vertical));
        geometry.naturalColumns.push_back(itemWidth);
        geometry.unnumberedColumns.push_back(ceil(msime::candidate::CandidateItemNaturalWidth(items.back(), unnumberedMetrics, !vertical)));
        natural = vertical ? MAX(natural, itemWidth) : natural + itemWidth;
    }
    if (!vertical && items.size() > 1) natural += metrics.columnGap * static_cast<CGFloat>(items.size() - 1);
    // 列表翻页窄列参与卡片总宽度；首行翻页不占候选区域。
    CGFloat width = MAX(20, natural + 2 * inset + pagerWidth);
    width = MAX(width, preeditWidth);
    // Half the screen caps the card, except that a horizontal page whose candidate lines fit on one screen-wide line grows past it as far as its candidates need with nothing wrapped, glosses included, up to the screen less a margin on each side. Only a page wider than that squeezes its glosses to wrap under their text (SingleLineColumns). A page whose candidate lines alone do not fit breaks onto more lines anyway, so it keeps the half-screen card rather than a screen-wide one.
    CGFloat widthCap = MAX(80, floor(visible.size.width * 0.5));
    if (!vertical) {
        const CGFloat screenCap = MAX(widthCap, floor(visible.size.width - 2 * MSIMECandidateScreenMargin));
        if (ceil(msime::candidate::SingleLineMinimumWidth(items, metrics)) + 2 * inset + pagerWidth <= screenCap)
            widthCap = MAX(widthCap, MIN(screenCap, natural + 2 * inset + pagerWidth));
        if (_appearance.adaptiveCandidateCount)
            widthCap = MIN(_appearance.candidateMaxWidth, MAX(1.0, visible.size.width - 2 * MSIMECandidateScreenMargin));
    }
    width = MIN(width, widthCap);
    if (paging) width = MAX(width, 76 * scale);
    // 共享版式用固定 160 DIP 下限，收紧取消下限；原生主题保留 7em。包与装饰的下限始终参与。
    if (vertical)
        width = MAX(width, msime::candidate::CandidateCardMinimumWidth(font.pointSize, minimumWidth, widthCap,
            layout ? std::optional<bool>(_candidateLayoutFitContent) : std::nullopt, scale));
    geometry.width = width;
    geometry.lineWidth = MAX(1, width - 2 * inset - pagerWidth);
    const msime::candidate::CandidatePageMeasure measure = [texts, annotations, translations, font, glossFont, scale, rowPadding](std::size_t row, msime::candidate::CandidateRun run, double runWidth) {
        return MSIMECandidateRunMeasure(texts[row], annotations[row], translations[row], font, glossFont, scale, rowPadding)(run, runWidth);
    };
    // Horizontal rows keep the height a gloss will need even before it arrives, so the card does not grow under the user seconds after the composition started.
    const CGFloat minimumHeight = vertical || !reservesGloss ? 0 : candidateRow + 2 * metrics.paddingVertical + [self reservedGlossHeightForFont:glossFont];
    if (sharedColumns) {
        NSParameterAssert(!vertical && items.size() <= sharedColumns->size());
        double x = 0, tallest = minimumHeight;
        geometry.lineWidth = 0;
        for (double column : *sharedColumns) geometry.lineWidth += column;
        geometry.lineWidth += geometry.columnGap * (sharedColumns->size() - 1);
        geometry.width = geometry.lineWidth + 2 * inset + pagerWidth;
        for (NSUInteger index = 0; index < items.size(); ++index) {
            msime::candidate::CandidateRowLayout row;
            row.x = x;
            row.width = (*sharedColumns)[index];
            row.item = msime::candidate::LayoutCandidateItem(items[index], row.width - metrics.chrome, metrics, true,
                [&, index](msime::candidate::CandidateRun run, double runWidth) { return measure(index, run, runWidth); });
            tallest = MAX(tallest, row.item.height);
            geometry.rows.push_back(row);
            x += row.width + geometry.columnGap;
        }
        for (auto &row : geometry.rows) row.height = tallest;
    } else {
        geometry.rows = msime::candidate::LayoutCandidatePage(items, geometry.lineWidth, metrics, !vertical, measure, minimumHeight);
    }
    // 竖排的译文画在候选同一行。候选词翻译范围是「仅当前高亮」时只有高亮行有译文，视图给这一行标了 translation_reserved：它一直按译文到了以后的行高留着（行数和横排的预留相同），回答还没到、高亮换了一行，窗口都不变高变矮。
    if (vertical && reservesGloss && geometry.rows.size() == candidates.count) {
        const CGFloat glossHeight = [self glossHeightReservedForFont:glossFont];
        const CGFloat reservedHeight = glossHeight > 0
            ? MAX(metrics.candidateRow, MAX(metrics.translationLine, glossHeight)) + 2.0 * MAX(0.0, metrics.paddingVertical) : 0;
        double shift = 0;
        for (NSUInteger row = 0; row < candidates.count; ++row) {
            auto &layoutRow = geometry.rows[row];
            layoutRow.y += shift;
            NSDictionary *candidate = candidates[row];
            if (![candidate isKindOfClass:NSDictionary.class] || !MSIMEStrictBoolean(candidate[@"translation_reserved"]) ||
                layoutRow.height >= reservedHeight) continue;
            shift += reservedHeight - layoutRow.height;
            layoutRow.height = reservedHeight;
        }
    }
    for (auto &row : geometry.rows) {
        // Whole points keep neighbouring buttons touching and their text on the pixel grid.
        const double left = round(row.x), right = round(row.x + row.width);
        const double top = round(row.y), bottom = round(row.y + row.height);
        row.x = left;
        row.width = right - left;
        row.y = top;
        row.height = bottom - top;
    }
    geometry.rowsHeight = ceil(msime::candidate::CandidatePageHeight(geometry.rows));
    geometry.selectedBarWidth = layout && layout->selectedBarWidth ? MAX(3.0 * scale, MSIMESkinLengthValue(layout->selectedBarWidth, font.pointSize, scale)) : 3.0 * scale;
    geometry.selectedBarHeight = layout && layout->selectedBarHeightEm
        ? MAX(geometry.selectedBarWidth * 2.0, static_cast<CGFloat>(*layout->selectedBarHeightEm * font.pointSize))
        : 0.0;
    geometry.selectedBarInside = !layout || layout->selectedBarInside.value_or(false);
    return geometry;
}

- (msime::mac::CandidatePagerMetrics)candidatePagerForSkin:(const msime::mac::ResolvedSkin &)skin
    font:(NSFont *)font scale:(CGFloat)scale vertical:(BOOL)vertical {
    const CGFloat extra = skin.layout && skin.layout->itemExtraHeight
        ? MSIMESkinLengthValue(skin.layout->itemExtraHeight, font.pointSize, scale) : 2 * scale;
    const BOOL triangle = skin.layout && skin.layout->pageArrowStyle.value_or("chevron") == "triangle";
    return msime::mac::CandidatePager(font.pointSize, scale, font.pointSize * 1.35 + extra, !vertical, triangle);
}

- (msime::mac::CandidateExpandToggleMetrics)candidateExpandToggleForSkin:(const msime::mac::ResolvedSkin &)skin
    font:(NSFont *)font scale:(CGFloat)scale {
    const CGFloat extra = skin.layout && skin.layout->itemExtraHeight
        ? MSIMESkinLengthValue(skin.layout->itemExtraHeight, font.pointSize, scale) : 2 * scale;
    return msime::mac::CandidateExpandToggle(font.pointSize, scale, font.pointSize * 1.35 + extra);
}

- (void)updateKeymapPanel {
    NSDictionary *viewDict = _viewState.toDictionary();
    NSString *editing = MSIMEShuangpinKeymapEditingText(viewDict);
    NSString *profile = _viewState.shuangpinProfile;
    NSString *mode = _viewState.localMode;
    if (!_session || !_activeClient || _appearance.englishMode ||
        MSIMEViewScheme(_viewState) != 1 ||
        ![mode isEqualToString:@"none"] ||
        _viewState.dedicatedEnglish ||
        !profile.length ||
        !MSIMEShouldShowShuangpinKeymap(YES, _appearance.shuangpinKeymap, editing.length > 0)) {
        [_keymapPanel orderOut:nil];
        return;
    }
    NSRect cursor = NSZeroRect;
    [_activeClient attributesForCharacterIndex:0 lineHeightRectangle:&cursor];
    if (!MSIMEValidCaret(cursor)) { [_keymapPanel orderOut:nil]; return; }
    if (!_keymapPanel) _keymapPanel = [[MSIMEShuangpinKeymapPanel alloc] init];
    [_keymapPanel setProfileName:profile];
    // The keymap opens beside the candidate window, so it is drawn in the candidate window's mode and marks the current key in the theme's accent.
    _keymapPanel.appearance = [_appearance candidateAppearanceOverride];
    [_keymapPanel setAccentColor:MSIMEThemedSkinColor(@"MSIMEKeymapAccent", [_appearance resolvedSkinForDark:NO].tokens.accent,
                                                      [_appearance resolvedSkinForDark:YES].tokens.accent)];
    [_keymapPanel updateHighlightedKey:MSIMEShuangpinKeymapHighlightedKey(viewDict)];
    const CGFloat scale = MSIMECandidateScale(_appearance);
    CGFloat clearance = (_appearance.fontSize + 42.0) * scale;
    const NSUInteger pageCount = _viewState.pageCount;
    if (_appearance.vertical) clearance = ((_appearance.fontSize + 10.0) * MIN(_viewState.candidates.size(), _appearance.pageSize) + 24.0) * scale;
    CGFloat preeditExtra = 0;
    CGFloat bodyRowGap = 0;
    BOOL headerPager = _appearance.showsCandidatePageNumber;
    NSString *preedit = _viewState.effectivePreedit();
    NSArray *candidates = _viewState.visibleCandidatesArray();
    if (candidates.count) {
        // The same rows the candidate panel lays out, so a card that wraps a long candidate onto several lines is kept clear of in full.
        NSScreen *screen = MSIMECandidateScreen(cursor);
        NSFont *font = [_appearance candidateFontOfSize:_appearance.fontSize * scale englishFirst:YES];
        NSFont *glossFont = [_appearance candidateFontOfSize:_appearance.translationFontSize * scale englishFirst:YES];
        // 键位图先于候选窗更新，必须自行读取本次皮肤，不能使用上一次重绘留下的几何。
        NSAppearance *current = _keymapPanel.appearance ?: _panel.effectiveAppearance ?: NSApp.effectiveAppearance;
        NSString *theme = [current bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
        auto skin = [_appearance candidateWindowSkinForDark:[theme isEqual:NSAppearanceNameDarkAqua]];
        const BOOL preeditVisible = _appearance.showsCandidatePreedit && preedit.length > 0;
        const auto margin = MSIMECandidatePreeditMargins(skin, preeditVisible, scale);
        preeditExtra = margin.top + margin.bottom + (preeditVisible && skin.canvas ? skin.canvas->lineGap.value_or(0) : 0);
        bodyRowGap = MSIMECandidateBodyRowGap(skin, preeditVisible, scale);
        _candidateLayoutGeometry = skin.layout;
        _candidateLayoutFitContent = !_appearance.vertical || skin.fitContent || _appearance.candidateWindowFitContent;
        _skinShowsSelectedBar = skin.tokens.showSelectedBar;
        if (skin.layout && skin.layout->padding)
            skin.tokens.pad = static_cast<float>(MSIMESkinLengthValue(skin.layout->padding, font.pointSize, scale, skin.tokens.pad));
        const CGFloat inset = msime::mac::CandidateInset(skin.tokens);
        headerPager = _appearance.showsCandidatePageNumber && !skin.pageArrows.has_value();
        const BOOL listPager = pageCount > 1 && _appearance.showsCandidatePageNumber && skin.pageArrows.value_or(false) &&
            !(skin.expandPanel.value_or(false) && [self expandedCandidatesOffered]);
        const auto pager = listPager ? [self candidatePagerForSkin:skin font:font scale:scale vertical:_appearance.vertical]
                                    : msime::mac::CandidatePagerMetrics{};
        const BOOL skinExpand = skin.expandPanel.value_or(false) && _appearance.showsCandidateExpandButton && [self expandedCandidatesOffered];
        const auto toggle = skinExpand ? [self candidateExpandToggleForSkin:skin font:font scale:scale]
                                      : msime::mac::CandidateExpandToggleMetrics{};
        NSRect rowVisible = screen ? screen.visibleFrame : NSMakeRect(0, 0, 1440, 900);
        const BOOL dark = [theme isEqual:NSAppearanceNameDarkAqua];
        const auto shadowInsets = msime::mac::CandidateShadowExtraInsets([self candidateShadowForSkin:skin dark:dark],
            skin.canvas ? skin.canvas->outer : msime::mac::SkinInsets{},
            [_appearance decorationImageForDark:dark] ? skin.decorationTopDip : 0);
        const CGFloat canvasExtra = skin.canvas ? msime::mac::CanvasExtraWidth(*skin.canvas, inset) : 0;
        rowVisible.size.width = MAX(0.0, rowVisible.size.width - canvasExtra - 2 * (shadowInsets.left + shadowInsets.right));
        const MSIMECandidatePageGeometry geometry =
            [self candidatePageGeometry:candidates font:font glossFont:glossFont showSelectedBar:_skinShowsSelectedBar inset:inset
                                 paging:pageCount > 1 && headerPager
                                visible:rowVisible preeditWidth:0 minimumWidth:0 reservesGloss:YES pagerWidth:pager.column + toggle.column()];
        const CGFloat verticalInset = msime::mac::CandidateVerticalInset(skin.tokens);
        const auto padding = skin.canvas ? msime::mac::CanvasContentInsets(*skin.canvas, inset, verticalInset)
                                         : msime::mac::SkinInsets{verticalInset, inset, verticalInset, inset};
        clearance = MAX(clearance, geometry.rowsHeight + padding.top + padding.bottom + pager.row);
    }
    if (!_viewState.candidates.empty()) {
        // The card's top row: the brand mark, the reading when it is shown, and the page indicator whenever there is more than one page and `show_candidate_page_number` is on.
        CGFloat header = (pageCount > 1 && headerPager) || (_appearance.showsAppLogo && MSIMECandidateLogoImage()) ? MSIMECandidateHeaderHeight * scale : 0;
        if (_appearance.showsCandidatePreedit && [preedit length]) {
            NSFont *preeditFont = MSIMECandidatePreeditFont(_appearance);
            header = MAX(header, MAX(22.0 * scale, MSIMECandidateTextHeight(preedit, preeditFont) + 6.0 * scale));
        }
        clearance += header + bodyRowGap + (header > 0 ? preeditExtra : 0);
    }
    [_keymapPanel showNearCaretRect:cursor candidateClearance:clearance];
}

// IMK keeps a controller per text input client and each owns its own candidate window, so a controller that loses its client without a matching deactivateServer: (a mismatched sender, or a client that goes away) would leave its last frame on screen after the next controller commits. Only one composition is ever on screen, so whichever controller shows its window or takes focus hides the previous owner's.
static __weak MSIMEInputController *MSIMECandidatePanelOwner;
- (void)claimCandidatePanel {
    MSIMEInputController *previous = MSIMECandidatePanelOwner;
    if (previous && previous != self) [previous hideCandidatePanel:"superseded"];
    MSIMECandidatePanelOwner = self;
}

static const char *MSIMEMotionPresetName(msime::mac::SkinAnimationPreset preset) {
    switch (preset) {
    case msime::mac::SkinAnimationPreset::none: return "none";
    case msime::mac::SkinAnimationPreset::fade: return "fade";
    case msime::mac::SkinAnimationPreset::fade_scale: return "fade_scale";
    case msime::mac::SkinAnimationPreset::slide_up: return "slide_up";
    case msime::mac::SkinAnimationPreset::spring: return "spring";
    case msime::mac::SkinAnimationPreset::glow: return "glow";
    }
    return "none";
}

// 动效没播时只看画面分不出是皮肤没写、数据没到，还是被规则挡下，所以把这次的决定连同画的包和它的预设一起记下来。
- (void)logCandidateMotion {
    if (!msime_macos_diagnostic_enabled()) return;
    const auto &animation = _renderedCandidateAnimation;
    msime_macos_diagnostic_writef("candidate motion %s skin=%s enter=%s exit=%s selection=%s duration_ms=%.0f",
        _candidateMotion.lastDecision.UTF8String ?: "-", _renderedCandidateSkin.empty() ? "-" : _renderedCandidateSkin.c_str(),
        MSIMEMotionPresetName(animation.enter), MSIMEMotionPresetName(animation.exit), MSIMEMotionPresetName(animation.selection),
        animation.durationMs);
}

// 候选窗同步显隐：_panel.isVisible 立即是新状态，键盘和指针路由从不等动画。皮肤包写了的动效只加在图层上，见 MSIMECandidateMotion。
- (void)hideCandidatePanel:(const char *)reason {
    [_candidatePointerMenu dismiss];
    _candidateTransitionFrames = nil;
    [self stopCandidateFrameAnimation];
    if (msime_macos_diagnostic_enabled() && _panel.isVisible) msime_macos_diagnostic_writef("candidate hide reason=%s", reason);
    _animatedHighlightSlot = 0;
    _animatedHighlightContext = nil;
    // 只有普通收起（上屏、取消、切到英文）可以播退场；失焦、会话替换、光标无效等立即收起，不在屏幕上留旧候选文字。
    const BOOL allowExit = strcmp(reason, "empty") == 0 || strcmp(reason, "english_mode") == 0;
    const BOOL wasVisible = _panel.isVisible;
    if ([_panel.contentView isKindOfClass:MSIMECandidateChromeView.class])
        [(MSIMECandidateChromeView *)_panel.contentView setCanvasPlaybackVisible:NO anchor:0];
    _canvasAnimationAnchor = 0;
    if (_candidateMotion) [_candidateMotion hidePanel:_panel animation:_renderedCandidateAnimation allowExit:allowExit];
    else [_panel orderOut:nil];
    if (wasVisible && _candidateMotion) [self logCandidateMotion];
}

- (void)showCandidatePanel {
    if (!_candidateMotion) _candidateMotion = [MSIMECandidateMotion new];
    NSMutableArray<NSView *> *rows = [NSMutableArray array];
    MSIMECandidateDecorationView *decoration = nil;
    for (NSView *subview in MSIMECandidateControlSubviews(_panel.contentView)) {
        if ([subview isKindOfClass:MSIMECandidateButton.class] && subview.tag >= 0) [rows addObject:subview];
    }
    for (NSView *subview in _panel.contentView.subviews)
        if ([subview isKindOfClass:MSIMECandidateDecorationView.class]) decoration = (MSIMECandidateDecorationView *)subview;
    const BOOL wasVisible = _panel.isVisible;
    [_candidateMotion showPanel:_panel animation:_renderedCandidateAnimation rows:rows];
    // 打字时每个键都重新显示一次，只记从隐藏到显示的那一次。
    if (!wasVisible) [self logCandidateMotion];
    // 每次排版都换了新的装饰视图，动画重新挂上，相位接着上一帧。
    [_candidateMotion animateDecoration:decoration motion:_renderedDecorationMotion periodMs:_renderedDecorationPeriodMs];
    if (_canvasAnimationAnchor <= 0) _canvasAnimationAnchor = CACurrentMediaTime();
    if ([_panel.contentView isKindOfClass:MSIMECandidateChromeView.class])
        [(MSIMECandidateChromeView *)_panel.contentView setCanvasPlaybackVisible:YES anchor:_canvasAnimationAnchor];
}

- (void)setCandidateLayoutSize:(NSSize)size {
    _candidateLayoutSize = size;
    if (!_candidatePreviousVisible || _appearance.vertical) [_panel setContentSize:size];
}

- (NSSize)candidatePanelSize {
    return [_panel frameRectForContentRect:NSMakeRect(0, 0, _candidateLayoutSize.width, _candidateLayoutSize.height)].size;
}

- (void)stopCandidateFrameAnimation {
    NSViewAnimation *animation = _candidateFrameAnimation;
    _candidateFrameAnimation = nil;
    animation.delegate = nil;
    [animation stopAnimation];
    if ([_panel.contentView isKindOfClass:MSIMECandidateChromeView.class]) {
        MSIMECandidateChromeView *content = (id)_panel.contentView;
        content.resizingCandidateContentHeight = 0;
        [content setNeedsLayout:YES];
        [content layoutSubtreeIfNeeded];
    }
}

- (void)animationDidEnd:(NSAnimation *)animation {
    if (animation == _candidateFrameAnimation) [self stopCandidateFrameAnimation];
}

- (void)applyCandidateFrame:(NSRect)frame {
    const BOOL animate = _candidatePreviousVisible && !_appearance.vertical &&
        _candidatePreviousExpanded != (_expandedCandidates != nil) && !_candidateMotion.reduceMotion &&
        !NSEqualRects(_panel.frame, frame);
    MSIMECandidateChromeView *content = (id)_panel.contentView;
    if (!animate) {
        _candidateTransitionFrames = nil;
        [_panel setFrame:frame display:YES];
        [content layoutSubtreeIfNeeded];
        return;
    }
    // NSViewAnimation 按整点更新窗口，目标也对齐整点，避免完成时再跳半点。
    frame.origin.x = floor(frame.origin.x);
    frame.origin.y = floor(frame.origin.y);
    frame.size.width = ceil(frame.size.width);
    frame.size.height = ceil(frame.size.height);
    // 先完成目标布局，但不提前改变窗口。动画直接改变原窗口的 frame，背景、边框和阴影一起收放。
    [content layoutSubtreeIfNeeded];
    content.resizingCandidateContentHeight = NSHeight(content.candidateContentView.bounds);
    content.frame = [_panel contentRectForFrameRect:_panel.frame];
    [content setFrameOrigin:NSZeroPoint];
    [content layoutSubtreeIfNeeded];
    NSMutableArray<NSDictionary *> *animations = [NSMutableArray arrayWithObject:@{
        NSViewAnimationTargetKey:_panel,
        NSViewAnimationStartFrameKey:[NSValue valueWithRect:_panel.frame],
        NSViewAnimationEndFrameKey:[NSValue valueWithRect:frame]}];
    // 同一身份的候选从展开前的位置移到新位置；新出现的候选直接在目标位置。
    for (NSView *view in MSIMECandidateControlSubviews(content)) {
        if (![view isKindOfClass:MSIMECandidateButton.class] || view.tag < 0) continue;
        MSIMECandidateButton *button = (id)view;
        NSValue *previous = button.candidateID ? _candidateTransitionFrames[button.candidateID] : nil;
        if (!previous) continue;
        const NSRect current = [_panel convertRectToScreen:[button convertRect:button.bounds toView:nil]];
        const NSRect old = previous.rectValue;
        // 内容随窗口顶部移动；只补偿格子相对窗口顶部的位置，不能再次叠加整窗移动。
        const CGFloat dx = NSMinX(old) - (NSMinX(current) - NSMinX(_panel.frame));
        const CGFloat dy = NSMinY(old) - (NSMinY(current) - NSMaxY(_panel.frame));
        if (fabs(dx) < 0.5 && fabs(dy) < 0.5) continue;
        const NSPoint origin = [button.superview convertPoint:button.frame.origin toView:nil];
        NSRect start = button.frame;
        start.origin = [button.superview convertPoint:NSMakePoint(origin.x + dx, origin.y + dy) fromView:nil];
        [animations addObject:@{NSViewAnimationTargetKey:button,
            NSViewAnimationStartFrameKey:[NSValue valueWithRect:start],
            NSViewAnimationEndFrameKey:[NSValue valueWithRect:button.frame]}];
    }
    _candidateTransitionFrames = nil;
    _candidateFrameAnimation = [[NSViewAnimation alloc] initWithViewAnimations:animations];
    _candidateFrameAnimation.duration = 0.2;
    _candidateFrameAnimation.animationCurve = NSAnimationEaseOut;
    _candidateFrameAnimation.animationBlockingMode = NSAnimationNonblocking;
    _candidateFrameAnimation.delegate = self;
    [_candidateFrameAnimation startAnimation];
}

- (MSIMEDecorationFrames *)decorationFramesForImage:(NSImage *)image path:(const std::string &)path {
    if (image != _decorationFramesSource) {
        _decorationFramesSource = image;
        _decorationFrames = image && !path.empty() ? [MSIMEDecorationFrames framesWithContentsOfFile:@(path.c_str())] : nil;
    }
    return _decorationFrames;
}

// 高亮在同一个输入串的同一页里换了格位时（方向键、Tab、网格里移动），按皮肤的选中预设装饰新高亮行；高亮本身已经随这一帧同步画好。打字或翻页时 Engine 把高亮放回第一格，那不是选中，不播，与 CandidatePanel 只在 selectCandidateWithIdentifier: 时装饰一致。
- (void)decorateCandidateSelection:(BOOL)panelWasVisible {
    MSIMECandidateButton *highlighted = nil;
    for (NSView *subview in MSIMECandidateControlSubviews(_panel.contentView))
        if ([subview isKindOfClass:MSIMECandidateButton.class] && subview.tag >= 0 && ((MSIMECandidateButton *)subview).candidateHighlighted) {
            highlighted = (MSIMECandidateButton *)subview;
            break;
        }
    const NSUInteger slot = highlighted ? (NSUInteger)highlighted.tag + 1 : 0;
    const NSUInteger previous = _animatedHighlightSlot;
    _animatedHighlightSlot = slot;
    NSString *context = [NSString stringWithFormat:@"%@\n%lu", _viewState.editingText ?: @"", (unsigned long)_viewState.page];
    const BOOL sameContext = [_animatedHighlightContext isEqualToString:context];
    _animatedHighlightContext = context;
    if (!panelWasVisible || !sameContext || previous == 0 || slot == 0 || slot == previous) return;
    if (msime_macos_diagnostic_enabled())
        msime_macos_diagnostic_writef("candidate motion select slot=%lu->%lu selection=%s duration_ms=%.0f", (unsigned long)previous,
            (unsigned long)slot, MSIMEMotionPresetName(_renderedCandidateAnimation.selection), _renderedCandidateAnimation.durationMs);
    [_candidateMotion decorateSelectionOfRow:highlighted fill:highlighted.fillColor cornerRadius:highlighted.cornerRadius
                                      accent:_renderedCandidateAccent animation:_renderedCandidateAnimation];
}

- (BOOL)usesAutomaticCandidateAppearance {
    if (@available(macOS 26.0, *)) {
        return ![_appearance candidateAppearanceOverride] && [_appearance.globalTheme isEqual:@"system"] &&
            [_appearance.themeMode isEqual:@"system"] && [_appearance.candidateTheme isEqual:@"follow"];
    }
    return NO;
}

// 窗口只采用显式主题或客户端颜色，不能把玻璃的自适应结果写回它的父窗口。
- (NSAppearance *)candidateAppearanceForClientAttributes:(NSDictionary *)attributes {
    NSAppearance *override = [_appearance candidateAppearanceOverride];
    NSAppearance *host = [self usesAutomaticCandidateAppearance] ? MSIMECandidateHostAppearance(attributes) : nil;
    const BOOL update = _appearance.candidateAppearanceOverrideConfigured || _candidateHostAppearance != nil || host != nil;
    _candidateHostAppearance = host;
    NSAppearance *resolved = override ?: host;
    if (update && _panel.appearance != resolved && ![_panel.appearance isEqual:resolved])
        _panel.appearance = resolved;
    return resolved;
}

- (MSIMECandidateChromeView *)candidateChromeWithFrame:(NSRect)frame skin:(const msime::mac::ResolvedSkin &)skin {
    if ([self usesAutomaticCandidateAppearance] && skin.id == "system" && skin.candidateSkin.empty() &&
        !skin.canvas && _candidateChromeClient == _activeClient &&
        [_panel.contentView isKindOfClass:MSIMECandidateChromeView.class]) {
        MSIMECandidateChromeView *content = (id)_panel.contentView;
        if (content.liquidGlassActive && !NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency) {
            // 保留玻璃及其自适应外观，只重建候选控件；旧按钮仍必须脱离当前内容容器。
            content.appearanceTarget = nil;
            [_candidateMotion endSelection];
            for (NSView *child in content.candidateContentView.subviews) [child removeFromSuperview];
            content.frame = frame;
            return content;
        }
    }
    _candidateChromeClient = _activeClient;
    return [[MSIMECandidateChromeView alloc] initWithFrame:frame];
}

// The candidate window, created on first use. Its wheel pages the Engine's candidates, or scrolls the expanded grid while that is open.
- (MSIMECandidatePanel *)candidatePanelWithAppearance:(NSAppearance *)candidateAppearance {
    if (!_panel) {
        MSIMECandidatePanel *panel = [[MSIMECandidatePanel alloc] initWithContentRect:NSZeroRect styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:NO];
        __weak MSIMEInputController *weakSelf = self;
        panel.pageHandler = ^(BOOL previous) {
            MSIMEInputController *controller = weakSelf;
            if (!controller || !controller->_activeClient || !controller->_session || !controller->_panel.isVisible) return;
            // 网格开着时滚轮滚的是网格的行，不经 Engine，也就没有派发要报告。
            if (controller->_expandedCandidates) {
                [controller scrollExpandedCandidatesBackwards:previous];
                return;
            }
            // 指针翻页先开一轮，见 msime_client.h 的顺序契约。
            [controller reportPointerToLifecycle];
            [controller dispatchCommand:previous ? MSIME_PREVIOUS_PAGE : MSIME_NEXT_PAGE];
        };
        panel.scrollHandler = nil;
        _panel = panel;
        _panel.level = MSIMECandidateWindowLevel();
        _panel.hasShadow = YES;
        _panel.hidesOnDeactivate = NO;
        _panel.becomesKeyOnlyIfNeeded = YES;
        _panel.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        if (_appearance.candidateAppearanceOverrideConfigured || _candidateHostAppearance) _panel.appearance = candidateAppearance;
    }
    return (MSIMECandidatePanel *)_panel;
}

// 面板现在这张卡片顶上的装饰透明带高度，和 _panel.frame.size 一样从面板上读：refreshCandidateSkin 发现明暗换了包时会整页重排，局部变量里的装饰高度可能已经不是面板上的那一张。
- (CGFloat)candidateCardTopInset {
    if (![_panel.contentView isKindOfClass:MSIMECandidateChromeView.class]) return 0;
    MSIMECandidateChromeView *content = (id)_panel.contentView;
    return content.cardTopInset + content.canvasOuterInsets.top + content.shadowOuterInsets.top;
}

// The reading in the card's top row, with the caret where the composition has it.
- (MSIMECandidatePreeditField *)candidatePreeditLabel:(NSString *)preedit font:(NSFont *)preeditFont {
    MSIMECandidatePreeditField *label = [MSIMECandidatePreeditField labelWithString:preedit];
    label.identifier = @"candidate-preedit";
    label.accessibilityLabel = @"候选窗预编辑";
    label.font = preeditFont;
    NSString *editing = _viewState.editingText ?: @"";
    label.caretIndex = MSIMEPreeditCaretPosition(editing, preedit, @(_viewState.caretPosition));
    // The chosen part of a phrase in progress is held out of the document, so the window has to
    // show it as well; without this the reading in the window would disagree with the marked
    // text in the client, which carries it.
    NSString *phrase = _viewState.phrasePrefix;
    if (phrase.length) {
        label.stringValue = [phrase stringByAppendingString:label.stringValue];
        label.caretIndex += phrase.length;
    }
    label.showsCaret = _viewState.valid && _viewState.focused;
    return label;
}

// 一个候选按钮，按这一页版面里第 slot 格的排版画；普通候选窗和展开的网格共用。`number` 是按钮上的序号，空串时不画序号（网格里只有高亮那一行标序号，数字键选的就是那一行）。帧、标签、菜单和高亮之外的状态由调用方定。
- (MSIMECandidateButton *)candidateButtonFor:(NSDictionary *)candidate geometry:(const MSIMECandidatePageGeometry &)geometry
                                        slot:(NSUInteger)slot number:(NSString *)number font:(NSFont *)font
                                  numberFont:(NSFont *)numberFont glossFont:(NSFont *)glossFont scale:(CGFloat)scale
                                    vertical:(BOOL)vertical action:(SEL)action {
    const msime::candidate::CandidateRowLayout &row = geometry.rows[slot];
    NSString *display = geometry.displays[slot];
    NSString *title = [NSString stringWithFormat:@"%@  %@", number, geometry.texts[slot]];
    MSIMECandidateButton *button = [MSIMECandidateButton buttonWithTitle:title target:self action:action];
    button.candidateID = candidate[@"id"];
    button.accessibilityLabel = number.length ? [NSString stringWithFormat:@"%@  %@", number, display] : display;
    // 读屏先读候选本身，再说明后面那段是插件写的，免得把插件的文字当成输入法的释义或辅助码。
    NSString *scriptAnnotation = geometry.scriptAnnotations[slot];
    if (scriptAnnotation.length)
        button.accessibilityLabel = [NSString stringWithFormat:@"%@，插件注释：%@", button.accessibilityLabel, scriptAnnotation];
    // AI 占位行画的是「…」（等回答）或「—」（没有回答），读屏读它的意思，不读标点；说法和 iOS、Android 的一样。共享内核已经不再产出这种行，这里留着。
    if (MSIMECandidateIsAIPlaceholder(candidate)) {
        NSString *state = [candidate[@"text"] isEqual:@"…"] ? @"AI 候选等待中" : @"AI 候选没有结果";
        button.accessibilityLabel = number.length ? [NSString stringWithFormat:@"%@  %@", number, state] : state;
    }
    button.font = font;
    button.numberFont = numberFont;
    button.chromeScale = scale;
    button.hasSkinLayout = geometry.hasSkinLayout;
    button.hidesNumber = geometry.hidesNumber;
    button.textLeftInset = geometry.textLeft;
    button.textRightInset = geometry.textRight;
    button.selectedBarWidth = geometry.selectedBarWidth;
    button.selectedBarHeight = geometry.selectedBarHeight;
    button.selectedBarInside = geometry.selectedBarInside;
    button.lineBreakMode = NSLineBreakByWordWrapping;
    button.annotation = row.item.annotation.width > 0 ? geometry.annotations[slot] : @"";
    button.translation = slot < geometry.translations.count ? geometry.translations[slot] : CandidateTranslation(candidate);
    button.glossReading = geometry.readings[slot];
    button.scriptAnnotation = scriptAnnotation;
    button.dimmedPrefixLength = MSIMECaretEditDimmedLength(_viewState, candidate, geometry.texts[slot]);
    button.pronunciation = [self pronunciationForCandidate:candidate];
    button.armedGlossColumn = _armedGlossColumn;
    button.translationFont = glossFont;
    button.itemLayout = row.item;
    button.hasItemLayout = YES;
    button.contentLeft = geometry.contentLeft;
    button.translationBelow = MSIMECandidateGlossRun(button.glossReading, button.translation).length || scriptAnnotation.length ? row.item.translation.below : !vertical;
    button.toolTip = geometry.tooltips[slot];
    button.bordered = NO;
    button.candidateHighlighted = MSIMEStrictBoolean(candidate[@"highlighted"]);
    button.candidatePending = MSIMECandidateIsAIPlaceholder(candidate);
    button.candidateFixed = MSIMECandidateIsFixedPosition(candidate[@"fixed_position"]);
    button.alignment = NSTextAlignmentLeft;
    return button;
}

// 原生主题的首行展开／收起按钮，标签 -3，模板符号图沿用既有外观。
- (MSIMECandidateButton *)expandedCandidatesToggleButton:(BOOL)expanded frame:(NSRect)frame scale:(CGFloat)scale {
    NSString *label = expanded ? @"收起候选" : @"展开候选";
    MSIMECandidateButton *button = [MSIMECandidateButton buttonWithTitle:@"" target:self action:@selector(toggleExpandedCandidates:)];
    NSImage *symbol = [NSImage imageWithSystemSymbolName:expanded ? @"chevron.up" : @"chevron.down" accessibilityDescription:label];
    NSImageSymbolConfiguration *configuration = [NSImageSymbolConfiguration configurationWithPointSize:11 * scale weight:NSFontWeightSemibold];
    button.image = [symbol imageWithSymbolConfiguration:configuration] ?: symbol;
    if (!button.image) button.title = expanded ? @"⌃" : @"⌄";
    button.imagePosition = NSImageOnly;
    button.frame = frame;
    button.font = [NSFont systemFontOfSize:NSFont.systemFontSize * scale];
    button.bordered = NO;
    button.tag = -3;
    button.identifier = expanded ? @"candidate-collapse" : @"candidate-expand";
    button.accessibilityLabel = label;
    button.toolTip = expanded ? @"收起候选（Esc）" : @"展开候选（↓）";
    return button;
}

// 共享卷轴使用来源线段和字号尺寸，仍走原来的控制器身份与命令检查。
- (MSIMECandidateButton *)skinExpandedCandidatesToggleButton:(BOOL)expanded frame:(NSRect)frame
    metrics:(msime::mac::CandidateExpandToggleMetrics)metrics {
    MSIMECandidateExpandToggleButton *button = [MSIMECandidateExpandToggleButton buttonWithTitle:@"" target:self action:@selector(toggleExpandedCandidates:)];
    button.toggleMetrics = metrics;
    button.expanded = expanded;
    button.frame = frame;
    button.bordered = NO;
    button.tag = -3;
    button.identifier = expanded ? @"candidate-collapse" : @"candidate-expand";
    button.accessibilityLabel = expanded ? @"收起候选" : @"展开候选";
    button.toolTip = expanded ? @"收起候选（Esc）" : @"展开候选（↓）";
    return button;
}

- (void)configureDecorationWord:(MSIMECandidateDecorationView *)decoration
                           skin:(const msime::mac::ResolvedSkin &)skin font:(NSFont *)font {
    NSString *word = @"";
    if (skin.decorationCandidate) {
        NSArray *page = _viewState.visibleCandidatesArray();
        int highlight = -1;
        for (NSUInteger index = 0; index < page.count; ++index)
            if ([page[index] isKindOfClass:NSDictionary.class] && MSIMEStrictBoolean(page[index][@"highlighted"])) {
                highlight = static_cast<int>(index); break;
            }
        if (const auto seat = skin.decorationCandidate->seat(page.count, highlight)) {
            NSDictionary *candidate = page[*seat];
            if ([candidate isKindOfClass:NSDictionary.class]) {
                NSString *text = [candidate[@"text"] isKindOfClass:NSString.class] ? candidate[@"text"] : @"";
                NSString *badge = skin.decorationCandidate->stripBadge ? @"" : CandidateSourceBadge(candidate, _aiBadgeString);
                word = MSIMEChineseOutputString([text stringByAppendingString:badge],
                    _appearance.traditionalOutput && MSIMEScriptConversionAppliesState(_viewState) &&
                    ![candidate[@"source"] isEqual:@13] && ![candidate[@"source"] isEqual:@18]);
            }
        }
    }
    [decoration setCandidateWord:word style:skin.decorationCandidate
                        boxSize:NSMakeSize(skin.decorationWidthDip, skin.decorationTopDip)
                      alignment:skin.decorationAlign font:font];
}

- (msime::mac::CandidateShadow)candidateShadowForSkin:(const msime::mac::ResolvedSkin &)skin dark:(BOOL)dark {
    return msime::mac::ResolveCandidateShadow(skin.shadow, dark, skin.canvas.has_value(),
        skin.canvas && [_appearance canvasAssetsForDark:dark].panelLoaded, MSIMECandidateScale(_appearance));
}

// 原有行度量保留字体与释义的排版规则，最后补入 canvas 四边边距与卡片外的阴影留白。
- (void)configureCanvasForContent:(MSIMECandidateChromeView *)content skin:(const msime::mac::ResolvedSkin &)skin
                            dark:(BOOL)dark inset:(CGFloat)inset preeditVisible:(BOOL)preeditVisible {
    const msime::mac::SkinCanvas canvas = skin.canvas.value_or(msime::mac::SkinCanvas{});
    MSIMECandidateCanvasAssets *assets = skin.canvas ? [_appearance canvasAssetsForDark:dark] : nil;
    const auto shadow = [self candidateShadowForSkin:skin dark:dark];
    const auto extra = msime::mac::CandidateShadowExtraInsets(shadow, canvas.outer, content.cardTopInset);
    content.candidateShadow = shadow;
    content.shadowOuterInsets = extra;
    const CGFloat verticalInset = msime::mac::CandidateVerticalInset(skin.tokens);
    const auto padding = msime::mac::CanvasContentInsets(canvas, inset, verticalInset);
    const CGFloat gap = preeditVisible ? canvas.lineGap.value_or(0) : 0;
    const CGFloat deltaWidth = padding.left + padding.right - 2 * inset;
    const CGFloat deltaHeight = padding.top + padding.bottom - 2 * verticalInset + gap;
    for (NSView *child in content.candidateContentView.subviews) {
        NSRect frame = child.frame;
        frame.origin.x += padding.left - inset;
        frame.origin.y += padding.bottom - verticalInset;
        // 预编辑行距只移动首行控件，列表翻页与分隔线仍贴着候选区域。
        const BOOL listControl = [child isKindOfClass:MSIMECandidatePagerButton.class] ||
            [child isKindOfClass:MSIMECandidateExpandToggleButton.class] ||
            [child.identifier isEqual:@"candidate-pager-divider"];
        if (!listControl && ![child.identifier isEqual:@"candidate-expanded-viewport"] &&
            (![child isKindOfClass:MSIMECandidateButton.class] || child.tag < 0)) frame.origin.y += gap;
        if ([child isKindOfClass:MSIMEExpandedCandidateScrollView.class])
            frame.size.width = MAX(0.0, frame.size.width + padding.right - inset);
        child.frame = frame;
        if ([child isKindOfClass:MSIMEExpandedCandidateScrollView.class]) {
            NSScrollView *viewport = (NSScrollView *)child;
            [viewport layoutSubtreeIfNeeded];
            [viewport.documentView setFrameSize:NSMakeSize(NSWidth(viewport.contentView.bounds),
                NSHeight(viewport.documentView.frame))];
        }
    }
    // 旧装饰带仍相对卡片顶部；自由挂件则相对新的卡片矩形定位。
    for (NSView *child in content.subviews) if ([child isKindOfClass:MSIMECandidateDecorationView.class]) {
        NSRect frame = child.frame;
        frame.origin.x += canvas.outer.left + extra.left + deltaWidth / 2;
        frame.origin.y += canvas.outer.bottom + extra.bottom + deltaHeight;
        child.frame = frame;
    }
    content.candidateRowsRect = NSOffsetRect(content.candidateRowsRect, padding.left - inset, padding.bottom - verticalInset);
    content.canvasOuterInsets = canvas.outer;
    NSSize size = content.frame.size;
    size.width += deltaWidth + canvas.outer.left + canvas.outer.right + extra.left + extra.right;
    size.height += deltaHeight + canvas.outer.top + canvas.outer.bottom + extra.top + extra.bottom;
    content.frame = NSMakeRect(0, 0, size.width, size.height);
    [self setCandidateLayoutSize:size];
    [content layoutSubtreeIfNeeded];
    if (skin.canvas) [content configureCanvas:canvas assets:assets];
}

// 展开网格共用各列宽度，行内按单元格实宽折行；皮肤、字体和选词契约沿用普通候选窗。
- (BOOL)renderExpandedCandidates {
    const bool timed = msime_macos_diagnostic_enabled();
    const uint64_t buildStarted = timed ? clock_gettime_nsec_np(CLOCK_UPTIME_RAW) : 0;
    NSRect reportedCursor = NSZeroRect;
    NSDictionary *clientAttributes = [(id<IMKTextInput>)_activeClient attributesForCharacterIndex:0 lineHeightRectangle:&reportedCursor];
    NSRect cursor = [self candidateCaretForRendering:reportedCursor];
    if (!MSIMEValidCaret(cursor)) return NO;
    NSScreen *screen = MSIMECandidateScreen(cursor);
    if (!screen) return NO;
    const NSRect visible = screen.visibleFrame;
    [self ensureAppearance];
    NSAppearance *candidateAppearance = [self candidateAppearanceForClientAttributes:clientAttributes];
    MSIMECandidatePanel *candidatePanel = [self candidatePanelWithAppearance:candidateAppearance];
    __weak MSIMEInputController *weakSelf = self;
    candidatePanel.scrollHandler = ^(NSEvent *event) {
        MSIMEInputController *controller = weakSelf;
        MSIMEExpandedCandidateScrollView *scrollView = controller ? controller->_expandedScrollView : nil;
        [scrollView scrollWheel:event];
    };
    NSAppearance *currentAppearance = candidateAppearance ?: _panel.effectiveAppearance ?: NSApp.effectiveAppearance;
    NSString *currentTheme = [currentAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
    const BOOL skinDark = [currentTheme isEqual:NSAppearanceNameDarkAqua];
    auto skin = [_appearance candidateWindowSkinForDark:skinDark];
    _candidateLayoutGeometry = skin.layout;
    _candidateLayoutFitContent = !_appearance.vertical || skin.fitContent || _appearance.candidateWindowFitContent;
    if (_renderedCandidateSkin != skin.candidateSkin) _canvasAnimationAnchor = 0;
    _renderedCandidateSkin = skin.candidateSkin;
    _renderedCandidateAnimation = skin.animation;
    _renderedDecorationMotion = skin.decorationMotion;
    _renderedDecorationPeriodMs = skin.decorationMotionPeriodMs;
    NSImage *decorationImage = [_appearance decorationImageForDark:skinDark];
    const CGFloat scale = MSIMECandidateScale(_appearance);
    NSFont *font = [_appearance candidateFontOfSize:_appearance.fontSize * scale englishFirst:YES];
    if (skin.layout && skin.layout->padding)
        skin.tokens.pad = static_cast<float>(MSIMESkinLengthValue(skin.layout->padding, font.pointSize, scale, skin.tokens.pad));
    const auto tokens = skin.tokens;
    _skinShowsSelectedBar = tokens.showSelectedBar;
    _renderedCandidateAccent = SkinColor(tokens.accent);
    const CGFloat inset = msime::mac::CandidateInset(tokens);
    const CGFloat verticalInset = msime::mac::CandidateVerticalInset(tokens);
    const CGFloat rowGap = tokens.itemSpacing.value_or(0.0);
    const CGFloat numberScale = skin.layout && skin.layout->numberScale.has_value()
        ? static_cast<CGFloat>(std::clamp(*skin.layout->numberScale, 0.0, 2.0))
        : MSIMECandidateNumberScale;
    NSFont *numberFont = MSIMECandidateNumberFont(font, numberScale);
    NSFont *glossFont = [_appearance candidateFontOfSize:_appearance.translationFontSize * scale englishFirst:YES];
    NSString *preeditValue = _viewState.effectivePreedit();
    NSString *preedit = _appearance.showsCandidatePreedit && preeditValue.length ? preeditValue : @"";
    NSFont *preeditFont = MSIMECandidatePreeditFont(_appearance);
    const CGFloat preeditHeight = preedit.length ? MAX(22.0 * scale, MSIMECandidateTextHeight(preedit, preeditFont) + 6.0 * scale) : 0;
    NSImage *logo = _appearance.showsAppLogo ? MSIMECandidateLogoImage() : nil;
    const CGFloat logoSide = MSIMECandidateLogoSide * scale;
    const CGFloat logoWidth = logo ? logoSide + MSIMECandidateLogoGap * scale : 0;
    const CGFloat headerRow = MSIMECandidateHeaderHeight * scale;
    const CGFloat arrowWidth = MSIMECandidatePageArrowWidth * scale;
    const CGFloat indicatorGap = MSIMECandidatePageIndicatorGap * scale;

    const NSUInteger count = _expandedCandidates.count;
    const NSUInteger columns = MAX((NSUInteger)1, (NSUInteger)_appearance.pageSize);
    // 共享卷轴的收起入口留在候选右侧；原生主题保留首行行号及其开关。
    const BOOL skinToggle = skin.expandPanel.value_or(false) && _appearance.showsCandidateExpandButton;
    const auto toggle = skinToggle ? [self candidateExpandToggleForSkin:skin font:font scale:scale]
                                  : msime::mac::CandidateExpandToggleMetrics{};
    const CGFloat scrollerPadding = MSIMEExpandedCandidateScrollerPadding(scale);
    NSRect rowVisible = visible;
    const CGFloat canvasExtra = skin.canvas ? msime::mac::CanvasExtraWidth(*skin.canvas, inset) : 0;
    const auto shadowInsets = msime::mac::CandidateShadowExtraInsets([self candidateShadowForSkin:skin dark:skinDark],
        skin.canvas ? skin.canvas->outer : msime::mac::SkinInsets{}, decorationImage ? skin.decorationTopDip : 0);
    rowVisible.size.width = MAX(0.0, rowVisible.size.width - canvasExtra - 2 * (shadowInsets.left + shadowInsets.right));
    // 逐行几何不依赖光标。移动光标的重画在输入都没变时沿用上一次的，其余重画（释义到达、外观或屏幕变化）照常重排。
    MSIMEExpandedGridGeometry grid;
    grid.entries = _expandedCandidates;
    grid.font = font;
    grid.glossFont = glossFont;
    grid.skin = skin.candidateSkin;
    grid.dark = skinDark;
    grid.scale = scale;
    grid.inset = inset;
    grid.toggleColumn = toggle.column();
    grid.rowVisibleWidth = rowVisible.size.width;
    grid.columns = columns;
    if (_expandedGridMovesCursor && _expandedGridGeometry && _expandedGridGeometry->sameInputs(grid)) {
        grid = *_expandedGridGeometry;
    } else {
        _expandedGridGeometry.reset();
        const NSUInteger gridRows = msime::candidate::ExpandedGridRows(count, columns);
        if (!gridRows) return NO;
        auto geometryForRow = [&](NSUInteger row, const std::vector<double> *sharedColumns = nullptr) {
            const NSUInteger start = row * columns;
            const NSUInteger length = MIN(columns, count - start);
            NSArray<NSDictionary *> *slice = [_expandedCandidates subarrayWithRange:NSMakeRange(start, length)];
            return [self candidatePageGeometry:slice font:font glossFont:glossFont showSelectedBar:tokens.showSelectedBar
                                          inset:inset paging:NO visible:rowVisible preeditWidth:0
                                   minimumWidth:MAX(0.0, MAX(skin.minWidthDip, skin.decorationWidthDip) - canvasExtra)
                                   reservesGloss:NO pagerWidth:toggle.column() expandedColumns:columns sharedColumns:sharedColumns];
        };
        grid.layouts = std::make_shared<std::vector<MSIMECandidatePageGeometry>>();
        auto &layouts = *grid.layouts;
        layouts.reserve(gridRows);
        grid.rowOffsets = {0};
        grid.rowOffsets.reserve(gridRows + 1);
        std::vector<double> sharedColumns(columns, 0);
        for (NSUInteger row = 0; row < gridRows; ++row) {
            layouts.push_back(geometryForRow(row));
            grid.rowsWidth = MAX(grid.rowsWidth, layouts.back().width + scrollerPadding);
            for (NSUInteger slot = 0; slot < layouts.back().naturalColumns.size(); ++slot)
                sharedColumns[slot] = MAX(sharedColumns[slot], layouts.back().naturalColumns[slot]);
        }
        const CGFloat columnGap = layouts.front().columnGap;
        const CGFloat available = MAX((CGFloat)columns, floor(rowVisible.size.width - 2 * MSIMECandidateScreenMargin -
            2 * inset - toggle.column() - scrollerPadding - columnGap * (columns - 1)));
        // 保留短列自然宽度，只收窄超长列；整张网格共用同一组列边界，末页不拉伸。
        auto sorted = sharedColumns;
        std::sort(sorted.begin(), sorted.end());
        CGFloat remaining = available, cap = available;
        for (NSUInteger slot = 0; slot < sorted.size(); ++slot) {
            cap = remaining / (sorted.size() - slot);
            if (sorted[slot] >= cap) break;
            remaining -= sorted[slot];
        }
        for (double &column : sharedColumns) column = MIN(column, cap);
        for (NSUInteger row = 0; row < gridRows; ++row) {
            layouts[row] = geometryForRow(row, &sharedColumns);
            grid.rowsWidth = MAX(grid.rowsWidth, layouts[row].width + scrollerPadding);
            grid.rowOffsets.push_back(grid.rowOffsets.back() + layouts[row].rowsHeight + (row + 1 < gridRows ? rowGap : 0));
        }
    }
    _expandedCursor = MIN(_expandedCursor, count - 1);
    // 来源的 expand_rows 只数首行之后的网格；原生网格把当前页也计入视口。
    const NSUInteger windowLimit = skin.expandRows ? 1 + *skin.expandRows : msime::candidate::kExpandedGridVisibleRows;
    const NSUInteger cursorRow = _expandedCursor / columns;
    if (!_expandedScrollsViewport)
        _expandedFirstRow = msime::candidate::ExpandedGridFirstRow(_expandedFirstRow, cursorRow, grid.layouts->size(),
            MIN(grid.layouts->size(), windowLimit));
    _expandedGridGeometry = grid;
    const auto layouts = grid.layouts;
    const std::vector<CGFloat> rowOffsets = grid.rowOffsets;
    const NSUInteger rows = layouts->size();
    // 双拼键位图按普通候选窗一页的高度让开候选窗（updateKeymapPanel），网格比那高，会压在它上面；网格开着时先收起键位图，收起网格后的下一次重画再放回来。
    [_keymapPanel orderOut:nil];
    const NSUInteger windowRows = MIN(rows, windowLimit);
    _expandedFirstRow = MIN(_expandedFirstRow, rows - 1);

    const BOOL pageControls = !skin.expandPanel.value_or(false) && _appearance.showsCandidatePageNumber;
    NSFont *indicatorFont = [NSFont monospacedDigitSystemFontOfSize:MSIMECandidatePageIndicatorPointSize * scale weight:NSFontWeightRegular];
    const BOOL rowIndicatorShown = pageControls;
    NSString *rowIndicator = [NSString stringWithFormat:@"%lu / %lu", (unsigned long)(cursorRow + 1), (unsigned long)rows];
    const CGFloat indicatorWidth = rowIndicatorShown ? ceil([rowIndicator sizeWithAttributes:@{NSFontAttributeName: indicatorFont}].width) : 0;
    const CGFloat controlsWidth = pageControls ? (rowIndicatorShown ? indicatorWidth + indicatorGap : 0) + arrowWidth : 0;
    const auto preeditMargin = MSIMECandidatePreeditMargins(skin, preedit.length > 0, scale);
    const CGFloat bodyRowGap = MSIMECandidateBodyRowGap(skin, preedit.length > 0, scale);
    const CGFloat headerContentHeight = (preedit.length || logo || pageControls) ? MAX(preeditHeight, headerRow) : 0;
    const CGFloat headerHeight = headerContentHeight + preeditMargin.top + preeditMargin.bottom;
    const CGFloat preeditWidth = preedit.length ? ceil([preedit sizeWithAttributes:@{NSFontAttributeName:preeditFont}].width) + 4 * scale + MSIMEPreeditCaretGap + preeditMargin.left + preeditMargin.right : 0;
    const CGFloat headerWidth = 2 * inset + logoWidth + preeditWidth + (preedit.length && pageControls ? indicatorGap : 0) + controlsWidth;

    const CGFloat width = ceil(MAX(headerWidth, grid.rowsWidth));
    const NSUInteger visibleRows = MIN(windowRows, rows - _expandedFirstRow);
    if (!_expandedScrollsViewport) _expandedVisibleRows = visibleRows;
    if (!_expandedScrollsViewport || _expandedViewportHeight <= 0)
        _expandedViewportHeight = rowOffsets[_expandedFirstRow + visibleRows] - rowOffsets[_expandedFirstRow] -
            (_expandedFirstRow + visibleRows < rows ? rowGap : 0);
    const CGFloat rowsHeight = _expandedViewportHeight;
    const CGFloat initialScrollTop = rowOffsets[_expandedFirstRow] + _expandedScrollOffset;

    if (!candidatePanel.wheelPaging) [candidatePanel resetWheelAccumulator];
    // 面板空白处的滚动也转交同一个原生视口，普通候选窗的滚轮开关不变。
    candidatePanel.mouseWheelEnabled = YES;
    _panel.opaque = NO;
    _panel.backgroundColor = NSColor.clearColor;
    const CGFloat decorationHeight = decorationImage ? MAX(0.0, skin.decorationTopDip) : 0.0;
    const CGFloat height = rowsHeight + 2 * verticalInset + decorationHeight + headerHeight + bodyRowGap;
    const CGFloat headerBottom = height - verticalInset - decorationHeight - headerHeight;
    // 候选窗口只在补齐画布与阴影留白后调整尺寸，避免每次组字先缩小再撑高。
    MSIMECandidateChromeView *content = [self candidateChromeWithFrame:NSMakeRect(0, 0, width, height) skin:skin];
    content.candidateContentView.clipsToBounds = YES;
    content.candidateRowsRect = NSMakeRect(inset, verticalInset,
        MAX(0.0, width - 2 * inset - toggle.column() - scrollerPadding), rowsHeight);
    content.candidateRowsTouchTop = headerHeight == 0 && bodyRowGap == 0;
    MSIMEExpandedCandidateScrollView *viewport = [[MSIMEExpandedCandidateScrollView alloc]
        initWithFrame:NSMakeRect(0, verticalInset, MAX(0.0, width - toggle.column()), rowsHeight)];
    viewport.clipsToBounds = YES;
    [viewport configureRowOffsets:rowOffsets width:NSWidth(viewport.frame)];
    _expandedScrollView = viewport;
    NSArray<NSDictionary *> *entries = _expandedCandidates;
    viewport.makeRow = ^NSArray<NSView *> *(NSUInteger row, CGFloat top, CGFloat documentHeight) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_expandedCandidates != entries || row >= layouts->size()) return @[];
        const MSIMECandidatePageGeometry &geometry = (*layouts)[row];
        const NSUInteger start = row * columns;
        const NSUInteger length = MIN(columns, count - start);
        NSMutableArray<NSView *> *buttons = [NSMutableArray array];
        for (NSUInteger slot = 0; slot < length; ++slot) {
            const NSUInteger index = start + slot;
            const msime::candidate::CandidateRowLayout &cell = geometry.rows[slot];
            NSString *number = row == cursorRow ? [NSString stringWithFormat:@"%lu", (unsigned long)(slot + 1)] : @"";
            MSIMECandidateButton *button = [controller candidateButtonFor:entries[index] geometry:geometry slot:slot number:number
                font:font numberFont:numberFont glossFont:glossFont scale:scale vertical:NO action:@selector(selectExpandedCandidate:)];
            button.tag = (NSInteger)index;
            button.candidateHighlighted = index == controller->_expandedCursor;
            button.frame = NSMakeRect(inset + cell.x, documentHeight - top - cell.y - cell.height, cell.width, cell.height);
            [buttons addObject:button];
        }
        return buttons;
    };
    __weak MSIMEExpandedCandidateScrollView *weakViewport = viewport;
    __weak MSIMECandidatePanel *weakPanel = candidatePanel;
    viewport.canScroll = ^BOOL {
        MSIMEInputController *controller = weakSelf;
        return controller && controller->_expandedScrollView == weakViewport && controller->_expandedCandidates == entries &&
            controller->_activeClient && controller->_session && controller->_panel.isVisible;
    };
    viewport.manualScrollBegan = ^{
        MSIMEInputController *controller = weakSelf;
        if (controller) {
            [controller->_candidateFrameAnimation setCurrentProgress:1];
            [controller stopCandidateFrameAnimation];
            controller->_expandedScrollsViewport = YES;
        }
    };
    viewport.lineScrollHandler = ^(BOOL previous) {
        [weakSelf scrollExpandedCandidatesBackwards:previous];
    };
    viewport.positionChanged = ^(NSUInteger first, CGFloat offset, BOOL previous, BOOL next) {
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_expandedScrollView != weakViewport) return;
        controller->_expandedFirstRow = first;
        controller->_expandedScrollOffset = offset;
        weakPanel.hasPreviousPage = previous;
        weakPanel.hasNextPage = next;
    };
    viewport.visibleRowsChanged = ^{
        MSIMEInputController *controller = weakSelf;
        if (!controller || controller->_expandedScrollView != weakViewport ||
            ![weakViewport isDescendantOf:MSIMECandidateControlsView(controller->_panel.contentView)]) return;
        [controller->_candidateMotion endSelection];
        [controller refreshCandidateSkin];
    };
    viewport.hasMoreRows = NO;
    [content.candidateContentView addSubview:viewport];
    const CGFloat controlsBottom = headerBottom + preeditMargin.bottom + floor((headerContentHeight - headerRow) / 2);
    const CGFloat collapseLeft = width - inset - arrowWidth;
    if (skinToggle) {
        [content.candidateContentView addSubview:[self skinExpandedCandidatesToggleButton:YES
            frame:NSMakeRect(width - inset - toggle.marginRight - toggle.side,
                headerBottom - bodyRowGap - toggle.marginTop - toggle.side, toggle.side, toggle.side) metrics:toggle]];
    }
    if (pageControls) {
        if (rowIndicatorShown) {
            NSTextField *indicator = [NSTextField labelWithString:rowIndicator];
            indicator.identifier = @"candidate-page-indicator";
            indicator.accessibilityLabel = [NSString stringWithFormat:@"第 %lu 行，共 %lu 行", (unsigned long)(cursorRow + 1), (unsigned long)rows];
            indicator.font = indicatorFont;
            indicator.alignment = NSTextAlignmentRight;
            const CGFloat indicatorHeight = ceil([rowIndicator sizeWithAttributes:@{NSFontAttributeName: indicatorFont}].height);
            indicator.frame = NSMakeRect(collapseLeft - indicatorGap - indicatorWidth,
                                         controlsBottom + floor((headerRow - indicatorHeight) / 2), indicatorWidth, indicatorHeight);
            [content.candidateContentView addSubview:indicator];
        }
        [content.candidateContentView addSubview:[self expandedCandidatesToggleButton:YES frame:NSMakeRect(collapseLeft, controlsBottom, arrowWidth, headerRow) scale:scale]];
    }
    if (preedit.length) {
        MSIMECandidatePreeditField *label = [self candidatePreeditLabel:preedit font:preeditFont];
        label.frame = NSMakeRect(inset + logoWidth + preeditMargin.left, headerBottom + preeditMargin.bottom + floor((headerContentHeight - preeditHeight) / 2),
                                 MAX(0.0, width - 2 * inset - logoWidth - preeditMargin.left - preeditMargin.right - controlsWidth - (pageControls ? indicatorGap : 0)), preeditHeight);
        [content.candidateContentView addSubview:label];
    }
    if (logo) {
        NSImageView *mark = [NSImageView imageViewWithImage:logo];
        mark.identifier = @"candidate-logo";
        mark.accessibilityLabel = MSIMEEditionDisplayName();
        mark.imageScaling = NSImageScaleProportionallyUpOrDown;
        mark.frame = NSMakeRect(inset + 2 * scale, headerBottom + preeditMargin.bottom + floor((headerContentHeight - logoSide) / 2), logoSide, logoSide);
        [content.candidateContentView addSubview:mark];
    }
    content.cardTopInset = decorationHeight;
    const NSSize decorationSize = decorationImage.size;
    if (const auto placed = msime::mac::DecorationPlacement(skin.decorationAlign, width, inset, decorationHeight, skin.decorationWidthDip,
                                                            decorationSize.width, decorationSize.height,
                                                            MSIMEDecorationMotionMargin(skin))) {
        MSIMECandidateDecorationView *decoration = [[MSIMECandidateDecorationView alloc] initWithFrame:NSMakeRect(placed->x, height - placed->top - placed->height, placed->width, placed->height)];
        decoration.identifier = @"candidate-decoration";
        decoration.image = decorationImage;
        decoration.frames = [self decorationFramesForImage:decorationImage path:skin.decorationPath];
        [self configureDecorationWord:decoration skin:skin font:font];
        [content addSubview:decoration];
    }
    [self configureCanvasForContent:content skin:skin dark:skinDark inset:inset preeditVisible:preedit.length > 0];
    if (_panel.contentView != content) {
        const NSRect layoutFrame = content.frame;
        _panel.contentView = content;
        content.frame = layoutFrame;
    }
    [viewport scrollToTopOffset:initialScrollTop];
    // 网格不是 Engine 的一页：清掉记下的那一页，收起后第一次重画走完整的重建，而不是在网格的按钮上原地改释义。
    _renderedCandidateState = {};
    content.appearanceTarget = self;
    content.appearanceAction = @selector(refreshCandidateSkin);
    [self refreshCandidateSkin];
    // refreshCandidateSkin 发现明暗换了包时会整个重画一次，那一次已经把面板摆好了。
    if (_panel.contentView != content) return YES;
    const NSSize panelSize = [self candidatePanelSize];
    _tallestVerticalCandidateHeight = MSIMETallestCandidateHeight(_tallestVerticalCandidateHeight, panelSize.height, false, _panel.isVisible);
    const NSPoint origin = MSIMECandidateOrigin(cursor, panelSize, visible, 0, [self candidateCardTopInset],
                                                content.canvasOuterInsets.left + content.shadowOuterInsets.left,
                                                content.canvasOuterInsets.bottom + content.shadowOuterInsets.bottom);
    [self applyCandidateFrame:NSMakeRect(origin.x, origin.y, panelSize.width, panelSize.height)];
    const BOOL wasVisible = _panel.isVisible;
    [self claimCandidatePanel];
    [self showCandidatePanel];
    [self decorateCandidateSelection:wasVisible];
    [self announceHighlightedCandidate:wasVisible];
    if (timed)
        msime_macos_diagnostic_writef("candidate-frame show expanded rows=%lu visible_rows=%lu candidates=%lu size=(%.0f,%.0f) origin=(%.0f,%.0f) flipped=%d build_ms=%.3f",
            (unsigned long)rows, (unsigned long)visibleRows, (unsigned long)count, panelSize.width, panelSize.height, origin.x, origin.y,
            origin.y >= NSMaxY(cursor) ? 1 : 0, static_cast<double>(clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - buildStarted) / 1e6);
    return YES;
}

- (void)renderCandidates {
    const bool timed = msime_macos_diagnostic_enabled();
    const uint64_t buildStarted = timed ? clock_gettime_nsec_np(CLOCK_UPTIME_RAW) : 0;
    _candidatePreviousVisible = _panel.isVisible;
    _candidateLayoutSize = [_panel contentRectForFrameRect:_panel.frame].size;
    _candidatePreviousExpanded = NO;
    for (NSView *subview in MSIMECandidateControlsView(_panel.contentView).subviews)
        if ([subview isKindOfClass:MSIMEExpandedCandidateScrollView.class]) _candidatePreviousExpanded = YES;
    _candidateTransitionFrames = nil;
    if (_candidatePreviousVisible && _candidatePreviousExpanded != (_expandedCandidates != nil) && !_appearance.vertical) {
        NSMutableDictionary<NSDictionary *, NSValue *> *frames = [NSMutableDictionary dictionary];
        for (NSView *view in MSIMECandidateControlSubviews(_panel.contentView)) {
            if (![view isKindOfClass:MSIMECandidateButton.class] || view.tag < 0) continue;
            NSDictionary *identifier = ((MSIMECandidateButton *)view).candidateID;
            if (!identifier) continue;
            NSRect rect = [_panel convertRectToScreen:[view convertRect:view.bounds toView:nil]];
            rect.origin.x -= NSMinX(_panel.frame);
            rect.origin.y -= NSMaxY(_panel.frame);
            frames[identifier] = [NSValue valueWithRect:rect];
        }
        _candidateTransitionFrames = frames;
    }
    // 先记下看到的位置再停动画：停下会取消顶部对齐，窗口还在半途时旧内容会整体上下错开。
    [self stopCandidateFrameAnimation];
    const shurufa::mac::ViewState previousRenderedState = _renderedCandidateState;
    // 换了 token，开着的菜单里每一项都已失效，收起它而不是留着点了没反应的菜单。
    [_candidatePointerMenu dismiss];
    _candidateMenuToken = [NSObject new];
    [self updateKeymapPanel];
    // 展开的候选网格开着时画网格；它对不上当前视图或画不出来时收起，落到下面画普通的一页。
    if (_expandedCandidates) {
        if ([self expandedCandidatesStillApply] && [self renderExpandedCandidates]) return;
        [self discardExpandedCandidates];
    }
    // 普通候选页结束上一轮展开布局，不能把旧字号或皮肤的视口带到下一次展开。
    _expandedViewportHeight = 0;
    if (_appearance.englishMode && !_managedEnglishComposition) { _renderedCandidateState = {}; [self resetCandidateAnchor]; [self hideCandidatePanel:"english_mode"]; return; }
    NSArray *candidates = _viewState.visibleCandidatesArray();
    if (![candidates isKindOfClass:NSArray.class] || candidates.count == 0) {
        _armedGlossColumn = 0;
        _renderedCandidateState = {};
        [self resetCandidateAnchor];
        [self hideCandidatePanel:"empty"];
        return;
    }
    if (_armedGlossColumn > 0) {
        NSDictionary *highlighted = [self highlightedCandidateForGloss];
        if (!MSIMECandidateTranslationColumn(highlighted, _armedGlossColumn).length) _armedGlossColumn = 0;
    }
    NSRect reportedCursor = NSZeroRect;
    NSDictionary *clientAttributes = [(id<IMKTextInput>)_activeClient attributesForCharacterIndex:0 lineHeightRectangle:&reportedCursor];
    NSRect cursor = [self candidateCaretForRendering:reportedCursor];
    if (!MSIMEValidCaret(cursor)) { _renderedCandidateState = {}; [self hideCandidatePanel:"invalid_caret"]; return; }
    NSScreen *screen = MSIMECandidateScreen(cursor);
    if (!screen) { _renderedCandidateState = {}; [self hideCandidatePanel:"no_screen"]; return; }
    NSRect visible = screen.visibleFrame;
    [self ensureAppearance];
    // 只在候选窗从隐藏到出现时查，同一次输入里的后续按键不再 stat；显示期间的替换由每秒的偏好轮询发现。
    if (!_panel.isVisible) [self applyReplacedSkinPackages];
    NSAppearance *candidateAppearance = [self candidateAppearanceForClientAttributes:clientAttributes];
    const BOOL vertical = _appearance.vertical;
    NSAppearance *currentAppearance = candidateAppearance ?: _panel.effectiveAppearance ?: NSApp.effectiveAppearance;
    NSString *currentTheme = [currentAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
    // The styled skin carries the window's scale in its pad, radii, decoration and minimum width; the fonts and the fixed lengths below take it from `scale`.
    const BOOL skinDark = [currentTheme isEqual:NSAppearanceNameDarkAqua];
    auto skin = [_appearance candidateWindowSkinForDark:skinDark];
    _candidateLayoutGeometry = skin.layout;
    _candidateLayoutFitContent = !_appearance.vertical || skin.fitContent || _appearance.candidateWindowFitContent;
    if (_renderedCandidateSkin != skin.candidateSkin) _canvasAnimationAnchor = 0;
    _renderedCandidateSkin = skin.candidateSkin;
    _renderedCandidateAnimation = skin.animation;
    _renderedDecorationMotion = skin.decorationMotion;
    _renderedDecorationPeriodMs = skin.decorationMotionPeriodMs;
    _renderedCandidateAccent = SkinColor(skin.tokens.accent);
    // 装饰图取这种明暗画的那个包的，两个槽位可以是两个包。
    NSImage *decorationImage = [_appearance decorationImageForDark:skinDark];
    const CGFloat scale = MSIMECandidateScale(_appearance);
    NSFont *font = [_appearance candidateFontOfSize:_appearance.fontSize * scale englishFirst:YES];
    if (skin.layout && skin.layout->padding)
        skin.tokens.pad = static_cast<float>(MSIMESkinLengthValue(skin.layout->padding, font.pointSize, scale, skin.tokens.pad));
    const auto geometry = skin.tokens;
    _skinShowsSelectedBar = geometry.showSelectedBar;
    const CGFloat inset = msime::mac::CandidateInset(geometry);
    const CGFloat verticalInset = msime::mac::CandidateVerticalInset(geometry);
    NSFont *glossFont = [_appearance candidateFontOfSize:_appearance.translationFontSize * scale englishFirst:YES];
    NSString *preeditValue = _viewState.effectivePreedit();
    NSString *preedit = _appearance.showsCandidatePreedit && preeditValue.length ? preeditValue : @"";
    NSRect rowVisible = visible;
    const CGFloat canvasExtra = skin.canvas ? msime::mac::CanvasExtraWidth(*skin.canvas, inset) : 0;
    const auto shadowInsets = msime::mac::CandidateShadowExtraInsets([self candidateShadowForSkin:skin dark:skinDark],
        skin.canvas ? skin.canvas->outer : msime::mac::SkinInsets{}, decorationImage ? skin.decorationTopDip : 0);
    rowVisible.size.width = MAX(0.0, rowVisible.size.width - canvasExtra - 2 * (shadowInsets.left + shadowInsets.right));
    NSFont *preeditFont = MSIMECandidatePreeditFont(_appearance);
    CGFloat preeditHeight = preedit.length ? MAX(22.0 * scale, MSIMECandidateTextHeight(preedit, preeditFont) + 6.0 * scale) : 0;
    const NSUInteger page = _viewState.page;
    const NSUInteger pageCount = _viewState.pageCount;
    const BOOL paging = pageCount > 1;
    // 用户开关控制所有鼠标翻页按钮；共享版式把它们放进列表，显式关闭时也不回退到首行。
    const BOOL pageControls = paging && _appearance.showsCandidatePageNumber && !skin.pageArrows.has_value();
    const BOOL listPageControls = paging && _appearance.showsCandidatePageNumber && skin.pageArrows.value_or(false) &&
        !(skin.expandPanel.value_or(false) && [self expandedCandidatesOffered]);
    const auto listPager = listPageControls ? [self candidatePagerForSkin:skin font:font scale:scale vertical:vertical]
                                           : msime::mac::CandidatePagerMetrics{};
    // 首行打头是水杉 logo（悬浮工具栏和中英提示也是），然后是拼音；「1 / 3」和 ‹ › 在右侧。logo 按 `show_app_logo` 显示，隐藏时拼音顶到左边；logo、拼音和翻页控件都不显示时首行不占高度。
    NSImage *logo = _appearance.showsAppLogo ? MSIMECandidateLogoImage() : nil;
    const CGFloat logoSide = MSIMECandidateLogoSide * scale;
    const CGFloat logoWidth = logo ? logoSide + MSIMECandidateLogoGap * scale : 0;
    const CGFloat headerRow = MSIMECandidateHeaderHeight * scale;
    const CGFloat arrowWidth = MSIMECandidatePageArrowWidth * scale;
    const CGFloat indicatorGap = MSIMECandidatePageIndicatorGap * scale;
    // 共享卷轴固定占候选右侧窄列；原生主题的首行与紧凑按钮保持既有规则。
    const BOOL expandable = _appearance.showsCandidateExpandButton && [self expandedCandidatesOffered];
    const BOOL skinExpand = expandable && skin.expandPanel.value_or(false);
    const auto toggle = skinExpand ? [self candidateExpandToggleForSkin:skin font:font scale:scale]
                                  : msime::mac::CandidateExpandToggleMetrics{};
    const BOOL compactExpand = expandable && !skinExpand && !pageControls && !preedit.length && !logo;
    const BOOL pageIndicatorShown = pageControls;
    NSFont *pageIndicatorFont = [NSFont monospacedDigitSystemFontOfSize:MSIMECandidatePageIndicatorPointSize * scale weight:NSFontWeightRegular];
    NSString *pageIndicator = pageIndicatorShown ? [NSString stringWithFormat:@"%lu / %lu", (unsigned long)(page + 1), (unsigned long)pageCount] : @"";
    const CGFloat pageIndicatorWidth = pageIndicatorShown ? ceil([pageIndicator sizeWithAttributes:@{NSFontAttributeName: pageIndicatorFont}].width) : 0;
    const CGFloat pageControlsWidth = (pageControls ? (pageIndicatorShown ? pageIndicatorWidth + indicatorGap : 0) + 2 * arrowWidth : 0) +
        (expandable && !skinExpand && !compactExpand ? arrowWidth : 0);
    const auto preeditMargin = MSIMECandidatePreeditMargins(skin, preedit.length > 0, scale);
    const CGFloat bodyRowGap = MSIMECandidateBodyRowGap(skin, preedit.length > 0, scale);
    // 光标后编辑生效时首行右侧有它的提示，没有别的首行内容时也要留出首行。
    const BOOL caretEditActive = MSIMECaretEditSuffixActive(_viewState);
    const CGFloat headerContentHeight = MAX(preeditHeight, pageControls || logo || (expandable && !skinExpand && !compactExpand) || caretEditActive ? headerRow : 0);
    const CGFloat headerHeight = headerContentHeight + preeditMargin.top + preeditMargin.bottom;
    const CGFloat numberScale = skin.layout && skin.layout->numberScale.has_value()
        ? static_cast<CGFloat>(std::clamp(*skin.layout->numberScale, 0.0, 2.0))
        : MSIMECandidateNumberScale;
    NSFont *numberFont = MSIMECandidateNumberFont(font, numberScale);
    const CGFloat preeditWidth = preedit.length ? ceil([preedit sizeWithAttributes:@{NSFontAttributeName:preeditFont}].width) + 4 * scale + MSIMEPreeditCaretGap + preeditMargin.left + preeditMargin.right : 0;
    // 首行在拼音之后接一段输入状态（竖线隔开），右侧在翻页控件之前放云候选预览和一枝小装饰；三样都只在显示拼音时出现。
    const BOOL statusVisible = _appearance.showsCandidateStatus && preedit.length;
    NSString *statusText = statusVisible ? MSIMECandidateStatusText([self schemeTitleForScheme:[self viewSchemeName]], _viewState,
                                                                     _appearance.traditionalOutput, _appearance.runtimeFullWidthInput) : @"";
    NSFont *statusFont = [NSFont systemFontOfSize:MSIMECandidatePageIndicatorPointSize * scale weight:NSFontWeightRegular];
    const CGFloat statusGap = 8 * scale;
    const CGFloat statusTextWidth = statusText.length ? ceil([statusText sizeWithAttributes:@{NSFontAttributeName: statusFont}].width) : 0;
    const CGFloat statusBlock = statusTextWidth > 0 ? 2 * statusGap + scale + statusTextWidth : 0;
    // 「候选编辑：光标后」和翻页的「1 / 3」同一字号，放在翻页控件左边；它算进卡片的最小宽度，模式生效期间总能画全。
    NSString *caretEditHint = caretEditActive ? MSIMECaretEditSuffixHint : @"";
    const NSSize caretEditHintSize = caretEditHint.length ? [caretEditHint sizeWithAttributes:@{NSFontAttributeName: statusFont}] : NSZeroSize;
    const CGFloat caretEditHintWidth = ceil(caretEditHintSize.width);
    const CGFloat caretEditHintBlock = caretEditHintWidth > 0 ? caretEditHintWidth + indicatorGap : 0;
    BOOL caretEditHintShown = caretEditHintWidth > 0;
    NSDictionary *onlinePreview = preedit.length ? MSIMECandidateOnlinePreview(candidates) : nil;
    NSString *previewText = onlinePreview ? MSIMEChineseOutputString(onlinePreview[@"text"],
        _appearance.traditionalOutput && MSIMEScriptConversionAppliesState(_viewState)) : @"";
    NSFont *previewFont = [_appearance candidateFontOfSize:MSIMECandidatePageIndicatorPointSize * scale englishFirst:NO];
    const CGFloat previewIconSide = ceil(previewFont.pointSize);
    const CGFloat previewGap = 12 * scale;
    // 预览最多占这么宽，再长的整句截断。
    const CGFloat previewNatural = previewText.length
        ? MIN(220 * scale, previewIconSide + 4 * scale + ceil([previewText sizeWithAttributes:@{NSFontAttributeName: previewFont}].width) + 4 * scale) : 0;
    const CGFloat ornamentWidth = statusVisible ? 18 * scale : 0;
    const CGFloat ornamentGap = 8 * scale;
    // 这三样只占卡片已有的宽度，不计入最小宽度：云候选是异步到的，算进去的话回答一到卡片就突然变宽，中英切换也会让卡片跟着伸缩。
    const CGFloat headerWidth = preedit.length || pageControls || logo || caretEditActive
        ? 2 * inset + logoWidth + preeditWidth + (preedit.length && pageControlsWidth ? indicatorGap : 0) + pageControlsWidth + caretEditHintBlock : 0;
    if (compactExpand) rowVisible.size.width = MAX(0.0, rowVisible.size.width - arrowWidth);
    MSIMECandidatePageGeometry pageGeometry = [self candidatePageGeometry:candidates font:font glossFont:glossFont showSelectedBar:geometry.showSelectedBar inset:inset
                                                                   paging:pageControls visible:rowVisible
                                                             preeditWidth:vertical ? headerWidth : (pageControlsWidth + logoWidth + caretEditHintBlock > 0 ? 2 * inset + pageControlsWidth + logoWidth + caretEditHintBlock : 0)
                                                             minimumWidth:MAX(0.0, MAX(skin.minWidthDip, skin.decorationWidthDip) - canvasExtra)
                                                            reservesGloss:YES pagerWidth:listPager.column + toggle.column()];
    // 共享窄列可含半点 DIP，先取整总宽，避免 AppKit 调整内容宽度后按钮离右边多出半点。
    const CGFloat width = skinExpand ? ceil(pageGeometry.width) : pageGeometry.width + (compactExpand ? arrowWidth : 0);
    const CGFloat headerArrowWidth = arrowWidth;
    const BOOL headerLogo = logo != nil;
    const CGFloat headerLogoWidth = headerLogo ? logoWidth : 0;
    const CGFloat headerControlsWidth = pageControlsWidth;
    MSIMECandidatePanel *candidatePanel = [self candidatePanelWithAppearance:candidateAppearance];
    candidatePanel.scrollHandler = nil;
    if (!candidatePanel.wheelPaging) [candidatePanel resetWheelAccumulator];
    candidatePanel.mouseWheelEnabled = [_appearance navigationEnabled:@"mouse_wheel"];
    candidatePanel.hasPreviousPage = page > 0;
    candidatePanel.hasNextPage = page + 1 < pageCount;
    _panel.opaque = NO;
    _panel.backgroundColor = NSColor.clearColor;
    // The band above the card that the decoration stands in, transparent; none without an image to put there, as on Windows.
    const CGFloat decorationHeight = decorationImage ? MAX(0.0, skin.decorationTopDip) : 0.0;
    const CGFloat height = pageGeometry.rowsHeight + listPager.row + 2 * verticalInset + decorationHeight + headerHeight + bodyRowGap;
    const CGFloat headerBottom = height - verticalInset - decorationHeight - headerHeight;
    // 释义回复不改变候选身份与窗口结构；尺寸一致时原地重绘，翻页或布局变化仍完整重建并撤下旧按钮。
    const auto previousShadow = [_panel.contentView isKindOfClass:MSIMECandidateChromeView.class]
        ? ((MSIMECandidateChromeView *)_panel.contentView).shadowOuterInsets : msime::mac::SkinInsets{};
    const NSSize previousContentSize = NSMakeSize(_panel.frame.size.width - previousShadow.left - previousShadow.right,
        _panel.frame.size.height - previousShadow.top - previousShadow.bottom);
    if (!skin.canvas && MSIMEOnlyCandidateTranslationsChanged(previousRenderedState, _viewState) &&
        [_panel.contentView isKindOfClass:MSIMECandidateChromeView.class] &&
        NSEqualSizes(previousContentSize, NSMakeSize(width, height))) {
        MSIMECandidateChromeView *content = (id)_panel.contentView;
        BOOL reusable = YES;
        for (NSUInteger index = 0; index < candidates.count && reusable; ++index) {
            MSIMECandidateButton *button = nil;
            for (NSView *subview in content.candidateContentView.subviews)
                if ([subview isKindOfClass:MSIMECandidateButton.class] && subview.tag == (NSInteger)index) {
                    if (button) { reusable = NO; break; }
                    button = (id)subview;
                }
            if (!button) { reusable = NO; break; }
            NSDictionary *candidate = candidates[index];
            const msime::candidate::CandidateRowLayout &row = pageGeometry.rows[index];
            NSString *title = [NSString stringWithFormat:@"%lu  %@", (unsigned long)(index + 1), pageGeometry.texts[index]];
            if (![button.candidateID isEqual:candidate[@"id"]] || ![button.title isEqual:title] ||
                ![button.annotation isEqual:pageGeometry.annotations[index]] ||
                ![button.scriptAnnotation isEqual:pageGeometry.scriptAnnotations[index]] ||
                button.candidateHighlighted != MSIMEStrictBoolean(candidate[@"highlighted"]) ||
                !NSEqualRects(button.frame, NSMakeRect(inset + row.x, headerBottom - bodyRowGap - row.y - row.height,
                                                       row.width, row.height))) {
                reusable = NO;
                break;
            }
            [self configureCandidateMenu:button candidate:candidate];
            button.frame = NSMakeRect(inset + row.x, headerBottom - bodyRowGap - row.y - row.height,
                                      row.width, row.height);
            button.toolTip = pageGeometry.tooltips[index];
            button.translation = index < pageGeometry.translations.count ? pageGeometry.translations[index] : CandidateTranslation(candidate);
            // 读音跟着新的释义重算，和上面排版量的是同一份；留着旧的会把上一条释义的读音画在新释义下面。
            button.pronunciation = [self pronunciationForCandidate:candidate];
            button.glossReading = pageGeometry.readings[index];
            button.armedGlossColumn = _armedGlossColumn;
            button.translationFont = glossFont;
            button.itemLayout = row.item;
            button.hasItemLayout = YES;
            button.contentLeft = pageGeometry.contentLeft;
            button.hasSkinLayout = pageGeometry.hasSkinLayout;
            button.hidesNumber = pageGeometry.hidesNumber;
            button.textLeftInset = pageGeometry.textLeft;
            button.textRightInset = pageGeometry.textRight;
            button.selectedBarWidth = pageGeometry.selectedBarWidth;
            button.selectedBarHeight = pageGeometry.selectedBarHeight;
            button.selectedBarInside = pageGeometry.selectedBarInside;
            button.translationBelow = MSIMECandidateGlossRun(button.glossReading, button.translation).length || button.scriptAnnotation.length ? row.item.translation.below : !vertical;
            button.needsDisplay = YES;
        }
        if (reusable) {
            for (NSView *subview in content.subviews)
                if ([subview isKindOfClass:MSIMECandidateDecorationView.class])
                    [self configureDecorationWord:(MSIMECandidateDecorationView *)subview skin:skin font:font];
            for (NSView *subview in content.candidateContentView.subviews)
                if ([subview isKindOfClass:MSIMECandidateButton.class] && (subview.tag == -1 || subview.tag == -2)) {
                    MSIMECandidateButton *button = (id)subview;
                    button.candidateID = _viewState.pageIdentityDictionary();
                    button.enabled = button.tag == -1 ? page > 0 : page + 1 < pageCount;
                }
            content.needsDisplay = YES;
            _renderedCandidateState = _viewState;
            [self refreshCandidateSkin];
            const NSSize panelSize = [self candidatePanelSize];
            _tallestVerticalCandidateHeight = MSIMETallestCandidateHeight(_tallestVerticalCandidateHeight, panelSize.height, vertical, _panel.isVisible);
            [_panel setFrameOrigin:MSIMECandidateOrigin(cursor, panelSize, visible, vertical ? _tallestVerticalCandidateHeight : 0,
                [self candidateCardTopInset], content.canvasOuterInsets.left + content.shadowOuterInsets.left,
                content.canvasOuterInsets.bottom + content.shadowOuterInsets.bottom)];
            [self claimCandidatePanel];
            [self showCandidatePanel];
            return;
        }
    }
    // 候选窗口只在补齐画布与阴影留白后调整尺寸，避免每次组字先缩小再撑高。
    MSIMECandidateChromeView *content = [self candidateChromeWithFrame:NSMakeRect(0, 0, width, height) skin:skin];
    content.candidateContentView.clipsToBounds = NO;
    content.candidateRowsRect = NSMakeRect(inset, verticalInset + listPager.row, pageGeometry.lineWidth, pageGeometry.rowsHeight);
    content.candidateRowsTouchTop = headerHeight == 0 && bodyRowGap == 0;
    NSUInteger slot = 0;
    const CGFloat rowsTop = headerBottom - bodyRowGap;
    for (NSDictionary *candidate in candidates) {
        const msime::candidate::CandidateRowLayout &row = pageGeometry.rows[slot];
        MSIMECandidateButton *button = [self candidateButtonFor:candidate geometry:pageGeometry slot:slot
                                                         number:[NSString stringWithFormat:@"%lu", (unsigned long)(slot + 1)]
                                                           font:font numberFont:numberFont glossFont:glossFont scale:scale
                                                       vertical:vertical action:@selector(selectCandidate:)];
        [self configureCandidateMenu:button candidate:candidate];
        button.tag = (NSInteger)slot;
        // The frame is the laid out row, so a wrapped line is inside the area that takes the click.
        button.frame = NSMakeRect(inset + row.x, rowsTop - row.y - row.height, row.width, row.height);
        ++slot;
        [content.candidateContentView addSubview:button];
    }
    if (listPageControls) {
        const CGFloat bottom = vertical ? inset + scale : rowsTop - pageGeometry.rowsHeight + listPager.marginBottom;
        const CGFloat left = vertical ? inset + pageGeometry.textLeft - (listPager.buttonWidth - listPager.glyphWidth()) / 2
                                     : width - inset - listPager.marginRight - 2 * listPager.buttonWidth - listPager.gap;
        for (NSUInteger direction = 0; direction < 2; ++direction) {
            MSIMECandidatePagerButton *button = [MSIMECandidatePagerButton buttonWithTitle:@"" target:self action:@selector(changeCandidatePage:)];
            button.frame = NSMakeRect(left + direction * (listPager.buttonWidth + listPager.gap), bottom,
                listPager.buttonWidth, listPager.buttonHeight);
            button.pagerMetrics = listPager;
            button.bordered = NO;
            button.tag = direction == 0 ? -1 : -2;
            button.enabled = direction == 0 ? page > 0 : page < pageCount - 1;
            button.identifier = direction == 0 ? @"candidate-list-previous" : @"candidate-list-next";
            button.accessibilityLabel = direction == 0 ? @"上一页候选" : @"下一页候选";
            button.candidateID = _viewState.pageIdentityDictionary();
            [content.candidateContentView addSubview:button];
        }
        if (listPager.dividerWidth > 0) {
            NSBox *divider = [[NSBox alloc] initWithFrame:NSMakeRect(left - listPager.dividerGap - listPager.dividerWidth,
                bottom, listPager.dividerWidth, listPager.buttonHeight)];
            divider.identifier = @"candidate-pager-divider";
            divider.boxType = NSBoxCustom;
            divider.borderWidth = 0;
            divider.contentViewMargins = NSZeroSize;
            [content.candidateContentView addSubview:divider];
        }
    }
    if (pageControls) {
        // 首行右对齐；预编辑更高时，按钮仍相对它居中。
        const CGFloat controlsBottom = headerBottom + preeditMargin.bottom + floor((headerContentHeight - headerRow) / 2);
        const CGFloat arrowsLeft = width - inset - (expandable ? 3 : 2) * headerArrowWidth;
        if (pageIndicatorShown) {
            NSTextField *indicator = [NSTextField labelWithString:pageIndicator];
            indicator.identifier = @"candidate-page-indicator";
            indicator.accessibilityLabel = [NSString stringWithFormat:@"第 %lu 页，共 %lu 页", (unsigned long)(page + 1), (unsigned long)pageCount];
            indicator.font = pageIndicatorFont;
            indicator.alignment = NSTextAlignmentRight;
            const CGFloat indicatorHeight = ceil([pageIndicator sizeWithAttributes:@{NSFontAttributeName: pageIndicatorFont}].height);
            indicator.frame = NSMakeRect(arrowsLeft - indicatorGap - pageIndicatorWidth,
                                         controlsBottom + floor((headerRow - indicatorHeight) / 2), pageIndicatorWidth, indicatorHeight);
            [content.candidateContentView addSubview:indicator];
        }
        for (NSUInteger direction = 0; direction < 2; ++direction) {
            MSIMECandidateButton *button = [MSIMECandidateButton buttonWithTitle:direction == 0 ? @"‹" : @"›" target:self action:@selector(changeCandidatePage:)];
            button.frame = NSMakeRect(arrowsLeft + direction * headerArrowWidth, controlsBottom, headerArrowWidth, headerRow);
            button.font = [NSFont systemFontOfSize:NSFont.systemFontSize * scale];
            button.bordered = NO;
            button.tag = direction == 0 ? -1 : -2;
            button.enabled = direction == 0 ? page > 0 : page < pageCount - 1;
            button.accessibilityLabel = direction == 0 ? @"上一页候选" : @"下一页候选";
            // A retained button from an old page cannot navigate a newer view.
            button.candidateID = _viewState.pageIdentityDictionary();
            [content.candidateContentView addSubview:button];
        }
    }
    if (skinExpand) {
        [content.candidateContentView addSubview:[self skinExpandedCandidatesToggleButton:NO
            frame:NSMakeRect(width - inset - toggle.marginRight - toggle.side,
                rowsTop - toggle.marginTop - toggle.side, toggle.side, toggle.side) metrics:toggle]];
    } else if (expandable) {
        const CGFloat buttonHeight = compactExpand ? MIN(headerRow, pageGeometry.rows.front().height) : headerRow;
        const CGFloat buttonBottom = compactExpand ? rowsTop - pageGeometry.rows.front().height + floor((pageGeometry.rows.front().height - buttonHeight) / 2)
                                                  : headerBottom + preeditMargin.bottom + floor((headerContentHeight - headerRow) / 2);
        const CGFloat expandWidth = compactExpand ? arrowWidth : headerArrowWidth;
        [content.candidateContentView addSubview:[self expandedCandidatesToggleButton:NO
            frame:NSMakeRect(width - inset - expandWidth, buttonBottom, expandWidth, buttonHeight) scale:scale]];
    }
    if (caretEditHintShown) {
        // 只标明状态，不接收点击；颜色和「1 / 3」一样取次要色，在 refreshCandidateSkinOnce 里跟着皮肤和明暗换。
        NSTextField *hint = [NSTextField labelWithString:caretEditHint];
        hint.identifier = @"candidate-caret-edit-hint";
        hint.accessibilityLabel = caretEditHint;
        hint.font = statusFont;
        hint.alignment = NSTextAlignmentRight;
        hint.lineBreakMode = NSLineBreakByClipping;
        const CGFloat hintHeight = ceil(caretEditHintSize.height);
        const CGFloat hintRight = width - inset - (headerControlsWidth ? headerControlsWidth + indicatorGap : 0);
        hint.frame = NSMakeRect(hintRight - caretEditHintWidth, headerBottom + preeditMargin.bottom + floor((headerContentHeight - hintHeight) / 2),
                                caretEditHintWidth, hintHeight);
        [content.candidateContentView addSubview:hint];
    }
    if (preedit.length) {
        const CGFloat rowBottom = headerBottom + preeditMargin.bottom;
        const CGFloat left = inset + headerLogoWidth + preeditMargin.left;
        const CGFloat right = width - inset - preeditMargin.right - (headerControlsWidth ? headerControlsWidth + indicatorGap : 0) -
            (caretEditHintShown ? caretEditHintBlock : 0);
        const CGFloat preeditNatural = preeditWidth - preeditMargin.left - preeditMargin.right;
        // 卡片宽度由候选行定，放不下整行时先让出装饰，再让出状态，预览按剩下的宽度截断，太窄就不画；拼音总能拿到余下的全部宽度，光标照旧在框里滚动。
        CGFloat ornament = ornamentWidth, statusRoom = statusBlock, preview = previewNatural;
        auto needed = [&] { return preeditNatural + statusRoom + (preview > 0 ? previewGap + preview : 0) + (ornament > 0 ? ornamentGap + ornament : 0); };
        if (needed() > right - left) ornament = 0;
        if (needed() > right - left) statusRoom = 0;
        if (needed() > right - left) {
            preview = MAX(0.0, right - left - preeditNatural - previewGap);
            if (preview < previewIconSide + 40 * scale) preview = 0;
        }
        const CGFloat rightExtras = (preview > 0 ? previewGap + preview : 0) + (ornament > 0 ? ornamentGap + ornament : 0);
        const CGFloat labelWidth = MAX(0.0, MIN(preeditNatural, right - left - statusRoom - rightExtras));
        MSIMECandidatePreeditField *label = [self candidatePreeditLabel:preedit font:preeditFont];
        label.frame = NSMakeRect(left, rowBottom + floor((headerContentHeight - preeditHeight) / 2),
                                 statusRoom > 0 || rightExtras > 0 ? labelWidth : MAX(0.0, right - left), preeditHeight);
        [content.candidateContentView addSubview:label];
        if (statusRoom > 0) {
            const CGFloat dividerHeight = round(MIN(headerContentHeight, preeditHeight) * 0.5);
            MSIMECandidateHeaderLineView *divider = [[MSIMECandidateHeaderLineView alloc]
                initWithFrame:NSMakeRect(NSMaxX(label.frame) + statusGap, rowBottom + floor((headerContentHeight - dividerHeight) / 2), scale, dividerHeight)];
            divider.identifier = @"candidate-status-divider";
            [content.candidateContentView addSubview:divider];
            NSTextField *statusLabel = [NSTextField labelWithString:statusText];
            statusLabel.identifier = @"candidate-status";
            statusLabel.accessibilityLabel = [NSString stringWithFormat:@"输入状态：%@", statusText];
            statusLabel.font = statusFont;
            statusLabel.lineBreakMode = NSLineBreakByClipping;
            const CGFloat statusHeight = ceil([statusText sizeWithAttributes:@{NSFontAttributeName: statusFont}].height);
            statusLabel.frame = NSMakeRect(NSMaxX(divider.frame) + statusGap, rowBottom + floor((headerContentHeight - statusHeight) / 2),
                                           statusTextWidth, statusHeight);
            [content.candidateContentView addSubview:statusLabel];
        }
        CGFloat trailing = right;
        if (ornament > 0) {
            MSIMECandidateHeaderOrnamentView *mark = [[MSIMECandidateHeaderOrnamentView alloc]
                initWithFrame:NSMakeRect(trailing - ornament, rowBottom + floor((headerContentHeight - ornament) / 2), ornament, ornament)];
            mark.identifier = @"candidate-header-ornament";
            [content.candidateContentView addSubview:mark];
            trailing -= ornament + ornamentGap;
        }
        if (preview > 0) {
            // 和候选行里那一格是同一个候选：点它就选那一格，身份对不上当前视图时 selectCandidate: 不做任何事。
            MSIMECandidateButton *button = [MSIMECandidateButton buttonWithTitle:previewText target:self action:@selector(selectCandidate:)];
            button.identifier = @"candidate-online-preview";
            button.tag = -4;
            button.bordered = NO;
            button.font = previewFont;
            button.candidateID = onlinePreview[@"id"];
            const BOOL cloud = MSIMEStrictInteger(onlinePreview[@"source"], -1) == 2;
            NSImage *icon = [NSImage imageWithSystemSymbolName:cloud ? @"cloud" : @"sparkles" accessibilityDescription:nil];
            NSImageSymbolConfiguration *configuration = [NSImageSymbolConfiguration configurationWithPointSize:previewIconSide weight:NSFontWeightRegular];
            button.image = [icon imageWithSymbolConfiguration:configuration] ?: icon;
            button.imagePosition = NSImageLeading;
            button.alignment = NSTextAlignmentRight;
            button.cell.lineBreakMode = NSLineBreakByTruncatingTail;
            button.toolTip = previewText;
            button.accessibilityLabel = [NSString stringWithFormat:@"%@：%@", cloud ? @"云候选" : @"AI 候选", previewText];
            button.frame = NSMakeRect(trailing - preview, rowBottom + floor((headerContentHeight - headerRow) / 2), preview, headerRow);
            [content.candidateContentView addSubview:button];
        }
    }
    if (headerHeight > 0 && !skin.canvas) {
        // 首行和候选行之间一条细线，左右各让出内边距；有间距时画在间距正中。
        const CGFloat hairline = 1.0 / MAX(1.0, _panel.backingScaleFactor);
        MSIMECandidateHeaderLineView *rule = [[MSIMECandidateHeaderLineView alloc]
            initWithFrame:NSMakeRect(inset, floor(rowsTop + bodyRowGap / 2), MAX(0.0, width - 2 * inset), hairline)];
        rule.identifier = @"candidate-header-rule";
        [content.candidateContentView addSubview:rule];
    }
    if (logo && headerLogo) {
        NSImageView *mark = [NSImageView imageViewWithImage:logo];
        mark.identifier = @"candidate-logo";
        mark.accessibilityLabel = MSIMEEditionDisplayName();
        mark.imageScaling = NSImageScaleProportionallyUpOrDown;
        mark.frame = NSMakeRect(inset + 2 * scale, headerBottom + preeditMargin.bottom + floor((headerContentHeight - logoSide) / 2), logoSide, logoSide);
        [content.candidateContentView addSubview:mark];
    }
    content.cardTopInset = decorationHeight;
    const NSSize decorationSize = decorationImage.size;
    if (const auto placed = msime::mac::DecorationPlacement(skin.decorationAlign, width, inset, decorationHeight, skin.decorationWidthDip,
                                                            decorationSize.width, decorationSize.height,
                                                            MSIMEDecorationMotionMargin(skin))) {
        // 最后加入，装饰图压在卡片顶边和首行上；不接收点击。
        MSIMECandidateDecorationView *decoration = [[MSIMECandidateDecorationView alloc] initWithFrame:NSMakeRect(placed->x, height - placed->top - placed->height, placed->width, placed->height)];
        decoration.identifier = @"candidate-decoration";
        decoration.image = decorationImage;
        decoration.frames = [self decorationFramesForImage:decorationImage path:skin.decorationPath];
        [self configureDecorationWord:decoration skin:skin font:font];
        [content addSubview:decoration];
    }
    [self configureCanvasForContent:content skin:skin dark:skinDark inset:inset preeditVisible:preedit.length > 0];
    if (_panel.contentView != content) {
        const NSRect layoutFrame = content.frame;
        _panel.contentView = content;
        content.frame = layoutFrame;
    }
    _renderedCandidateState = _viewState;
    content.appearanceTarget = self;
    content.appearanceAction = @selector(refreshCandidateSkin);
    [self refreshCandidateSkin];
    const NSSize panelSize = [self candidatePanelSize];
    // Every hide path orders the panel out, so a panel that is not on screen yet starts a fresh flip memory.
    _tallestVerticalCandidateHeight = MSIMETallestCandidateHeight(_tallestVerticalCandidateHeight, panelSize.height, vertical, _panel.isVisible);
    const NSPoint origin = MSIMECandidateOrigin(cursor, panelSize, visible, vertical ? _tallestVerticalCandidateHeight : 0,
        [self candidateCardTopInset], content.canvasOuterInsets.left + content.shadowOuterInsets.left,
        content.canvasOuterInsets.bottom + content.shadowOuterInsets.bottom);
    [self applyCandidateFrame:NSMakeRect(origin.x, origin.y, panelSize.width, panelSize.height)];
    const BOOL wasVisible = _panel.isVisible;
    [self claimCandidatePanel];
    [self showCandidatePanel];
    [self decorateCandidateSelection:wasVisible];
    [self announceHighlightedCandidate:wasVisible];
    // Geometry and counts only, the macOS form of the source's candidate-frame/candidate-position audit: flipped=1 means the window went above the caret for lack of room below.
    if (timed)
        msime_macos_diagnostic_writef("candidate-frame show rows=%lu vertical=%d caret=(%.0f,%.0f,%.0f,%.0f) size=(%.0f,%.0f) origin=(%.0f,%.0f) visible=(%.0f,%.0f,%.0f,%.0f) flipped=%d build_ms=%.3f",
            (unsigned long)candidates.count, vertical ? 1 : 0, NSMinX(cursor), NSMinY(cursor), NSWidth(cursor), NSHeight(cursor),
            panelSize.width, panelSize.height, origin.x, origin.y, NSMinX(visible), NSMinY(visible), NSWidth(visible), NSHeight(visible),
            origin.y >= NSMaxY(cursor) ? 1 : 0, static_cast<double>(clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - buildStarted) / 1e6);
}

// 读屏：请求朗读高亮的候选。候选窗是从不成为 key window 的非激活面板，焦点一直留在客户程序里，VoiceOver 不会自己去读它，所以每画一页就请求朗读高亮的那一行；朗读请求不移动焦点。和上一次读的是同一段话时不再读（比如多打一个字母首选没变），候选窗刚出现时（panelWasVisible 为 NO）照读。
- (void)announceHighlightedCandidate:(BOOL)panelWasVisible {
    if (!panelWasVisible) _announcedCandidate = nil;
    NSString *announcement = nil;
    for (NSView *subview in MSIMECandidateControlSubviews(_panel.contentView))
        if ([subview isKindOfClass:MSIMECandidateButton.class] && subview.isAccessibilitySelected) {
            announcement = subview.accessibilityLabel;
            break;
        }
    if (!announcement.length || [announcement isEqual:_announcedCandidate]) return;
    _announcedCandidate = [announcement copy];
    NSAccessibilityPostNotificationWithUserInfo(_panel, NSAccessibilityAnnouncementRequestedNotification,
                                                @{NSAccessibilityAnnouncementKey: announcement});
}

- (void)refreshCandidateSkin {
    if (_refreshingCandidateSkin) {
        _candidateSkinRefreshPending = YES;
        return;
    }
    _refreshingCandidateSkin = YES;
    @try {
        do {
            _candidateSkinRefreshPending = NO;
            [self refreshCandidateSkinOnce];
        } while (_candidateSkinRefreshPending);
    } @finally {
        _refreshingCandidateSkin = NO;
    }
}

- (void)refreshCandidateSkinOnce {
    // 玻璃布局会同步重入外观回调；本轮结束后重读内容区外观，避免旧颜色盖回新颜色。
    if (![_panel.contentView isKindOfClass:MSIMECandidateChromeView.class]) return;
    MSIMECandidateChromeView *content = (id)_panel.contentView;
    // 玻璃会按背后的应用单独调整内容外观，文字必须跟随内容区域，而不是仍跟随系统的外层卡片。
    NSString *match = [content.candidateContentView.effectiveAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
    const BOOL dark = [match isEqual:NSAppearanceNameDarkAqua];
    const auto skin = [_appearance candidateWindowSkinForDark:dark];
    auto tokens = skin.tokens;
    const auto shadow = [self candidateShadowForSkin:skin dark:dark];
    const auto extra = msime::mac::CandidateShadowExtraInsets(shadow,
        skin.canvas ? skin.canvas->outer : msime::mac::SkinInsets{}, content.cardTopInset);
    const auto previousExtra = content.shadowOuterInsets;
    const BOOL shadowGeometryChanged = extra.top != previousExtra.top || extra.right != previousExtra.right ||
        extra.bottom != previousExtra.bottom || extra.left != previousExtra.left;
    if (tokens.showSelectedBar != _skinShowsSelectedBar) { _expandedViewportHeight = 0; [self renderCandidates]; return; }
    // 明暗切换后画的是另一个槽位的包（或者这种明暗不画包）：装饰、最小宽度和圆角都跟着变，只换颜色不够，整页重排一次。重排结束时还会回到这里，那时包已经对上，不会再重排。面板没在屏幕上时不重排：重排会把它重新显示出来，下一次显示候选时本来就按当时的明暗排版。
    if ((skin.candidateSkin != _renderedCandidateSkin || shadowGeometryChanged) &&
        _panel.isVisible && !_rerenderingForCandidateSkin) {
        _rerenderingForCandidateSkin = YES;
        _expandedViewportHeight = 0;
        [self renderCandidates];
        _rerenderingForCandidateSkin = NO;
        return;
    }
    content.candidateShadow = shadow;
    // 同一个包的明暗模式也可能使用不同贴图；保留布局和候选身份，只替换缓存的绘制层。
    if (skin.canvas && content.canvasAssets != [_appearance canvasAssetsForDark:dark]) {
        [content configureCanvas:*skin.canvas assets:[_appearance canvasAssetsForDark:dark]];
        [content setCanvasPlaybackVisible:_panel.isVisible anchor:_canvasAnimationAnchor];
    }
    // The card's own opacity is already in the surface and border alpha and in backgroundOpacity, rather than in the panel's alphaValue, which would fade the candidates along with the card.
    content.fillColor = SkinColor(tokens.surface);
    content.strokeColor = SkinColor(tokens.border);
    // tokens.radius is the user's radius when one is set, else the package's card radius when it sets one, at the window's scale.
    content.cornerRadius = tokens.radius;
    content.lineWidth = tokens.borderWidth;
    content.clipsCandidateContent = tokens.explicitItemRadius ||
        std::min(msime::mac::CandidateInset(tokens), msime::mac::CandidateVerticalInset(tokens)) < std::max(0.0f, tokens.borderWidth) / 2.0f;
    content.needsLayout = YES;
    // Only in a mode whose resolution draws the package: one that supports a single mode draws no background in the other.
    content.backgroundImage = skin.backgroundPath.empty() ? nil : [_appearance backgroundImageForDark:dark];
    content.backgroundFit = skin.backgroundFit;
    content.backgroundOpacity = skin.backgroundOpacity;
    // 普通候选和展开网格都经这里刷新；只有未覆盖皮肤的系统主题使用原生材质。
    content.liquidGlassEnabled = skin.id == "system" && skin.candidateSkin.empty();
    // 九宫格与共享档位由内容绘制，不再叠系统窗口阴影；原生主题保留原有窗口阴影。
    _panel.hasShadow = content.nativeWindowShadowEnabled;
    if (content.liquidGlassActive) msime::mac::ApplyNativeGlassCandidateContrast(tokens, dark);
    NSArray<MSIMECandidateButton *> *candidateButtons = [MSIMECandidateControlSubviews(content) filteredArrayUsingPredicate:
        [NSPredicate predicateWithBlock:^BOOL(NSView *view, NSDictionary *_) {
            return ([view isKindOfClass:MSIMECandidateButton.class] ||
                    [view isKindOfClass:MSIMECandidatePreeditField.class]) &&
                view.tag >= 0;
        }]];
    // The top row's page controls take the theme's colours too: labelColor would follow the system appearance, not a theme that fixes its own mode. The design draws 「1 / 3」 and ‹ › in the secondary colour (dc.html L1324); an arrow with no page behind it fades further.
    for (NSView *child in content.candidateContentView.subviews) {
        if (([child.identifier isEqual:@"candidate-page-indicator"] || [child.identifier isEqual:@"candidate-caret-edit-hint"]) &&
            [child isKindOfClass:NSTextField.class])
            ((NSTextField *)child).textColor = SkinColor(tokens.number);
        // 首行的输入状态和两条细线用次要色，细线再淡一些；装饰和云候选预览用强调色。
        if ([child.identifier isEqual:@"candidate-status"] && [child isKindOfClass:NSTextField.class])
            ((NSTextField *)child).textColor = [SkinColor(tokens.number) colorWithAlphaComponent:tokens.number.a * 0.8];
        if ([child isKindOfClass:MSIMECandidateHeaderLineView.class])
            ((MSIMECandidateHeaderLineView *)child).lineColor = [SkinColor(tokens.number) colorWithAlphaComponent:tokens.number.a *
                ([child.identifier isEqual:@"candidate-header-rule"] ? 0.18 : 0.35)];
        if ([child isKindOfClass:MSIMECandidateHeaderOrnamentView.class])
            ((MSIMECandidateHeaderOrnamentView *)child).tintColor = SkinColor(tokens.accent);
        if ([child isKindOfClass:MSIMECandidateButton.class] && child.tag == -4) {
            MSIMECandidateButton *preview = (id)child;
            NSMutableParagraphStyle *paragraph = [NSMutableParagraphStyle new];
            paragraph.lineBreakMode = NSLineBreakByTruncatingTail;
            paragraph.alignment = NSTextAlignmentRight;
            preview.attributedTitle = [[NSAttributedString alloc] initWithString:preview.title attributes:@{
                NSFontAttributeName: preview.font ?: [NSFont systemFontOfSize:MSIMECandidatePageIndicatorPointSize],
                NSForegroundColorAttributeName: SkinColor(tokens.accent),
                NSParagraphStyleAttributeName: paragraph,
            }];
            preview.contentTintColor = SkinColor(tokens.accent);
            continue;
        }
        if ([child.identifier isEqual:@"candidate-pager-divider"] && [child isKindOfClass:NSBox.class])
            ((NSBox *)child).fillColor = SkinColor(tokens.pageArrowDivider.value_or(msime::mac::Rgba{0, 0, 0, 0}));
        if (![child isKindOfClass:MSIMECandidateButton.class] || (child.tag != -1 && child.tag != -2 && child.tag != -3)) continue;
        MSIMECandidateButton *arrow = (id)child;
        const BOOL listArrow = [arrow isKindOfClass:MSIMECandidatePagerButton.class];
        const BOOL skinToggle = [arrow isKindOfClass:MSIMECandidateExpandToggleButton.class];
        const BOOL triangle = listArrow && ((MSIMECandidatePagerButton *)arrow).pagerMetrics.triangle;
        const auto glyph = triangle ? tokens.text : tokens.number;
        const CGFloat disabledAlpha = listArrow ? (triangle ? 0.25 : 0.3) : 0.4;
        arrow.titleColor = arrow.enabled ? SkinColor(glyph) : [SkinColor(glyph) colorWithAlphaComponent:glyph.a * disabledAlpha];
        if (listArrow || skinToggle) arrow.fillColor = SkinColor(tokens.selected.a > 0.001 ? tokens.selected : tokens.hover);
        // 原生展开／收起的模板图按 contentTintColor 着色，共享卷轴线段直接使用 titleColor。
        if (arrow.tag == -3) arrow.contentTintColor = SkinColor(tokens.number);
        arrow.hoverColor = SkinColor(tokens.hover);
        arrow.cornerRadius = skinToggle ? MIN(4 * MSIMECandidateScale(_appearance), tokens.candidateRadius)
            : listArrow && !triangle ? MIN(3 * MSIMECandidateScale(_appearance), tokens.candidateRadius) : tokens.candidateRadius;
        arrow.needsDisplay = YES;
    }
    for (MSIMECandidateButton *button in candidateButtons) {
        if ([button.identifier isEqual:@"candidate-preedit"] && [button isKindOfClass:NSTextField.class]) {
            ((NSTextField *)(id)button).textColor = SkinColor(tokens.preeditText.value_or(tokens.accent));
            if ([button isKindOfClass:MSIMECandidatePreeditField.class])
                ((MSIMECandidatePreeditField *)(id)button).caretColor = SkinColor(tokens.preeditCaret.value_or(tokens.accent));
        }
        if (![button isKindOfClass:MSIMECandidateButton.class]) continue;
        button.fillColor = SkinColor(tokens.selected);
        button.hoverColor = SkinColor(tokens.hover);
        button.titleColor = button.candidateHighlighted ? SkinColor(tokens.selectedText) : SkinColor(tokens.text);
        // Windows fixed-position span overrides candidate text, not its number.
        if (button.candidateFixed) button.titleColor = [NSColor colorWithSRGBRed:55.0/255 green:154.0/255 blue:211.0/255 alpha:1];
        // AI 占位行用次要色（序号的颜色）画，和真候选区分开。
        if (button.candidatePending)
            button.titleColor = button.candidateHighlighted ? SkinColor(tokens.selectedNumber) : SkinColor(tokens.number);
        // The translation is the theme's secondary colour, which always equals the number colour: the design draws a plain row's gloss in cSub (dc.html L2150, L2183), the theme's kb.sub or the platform's sub, and on the selected fill in candSelTr (dc.html L1533), the selected number's colour. A fixed-position row keeps the Windows renderer's rule instead, where the translation is a child of the candidate text and takes its fixed-position colour at reduced opacity.
        // A package's own translation colour is its secondary colour, drawn on every row as Windows does.
        // 选中行换成按选中底色推导的释义色（SkinTokens::selectedTranslation），释义和它下面的读音才不会和不透明的选中底色同色。
        if (button.candidateHighlighted && tokens.selectedTranslation)
            button.translationColor = SkinColor(*tokens.selectedTranslation);
        else if (tokens.translation)
            button.translationColor = SkinColor(*tokens.translation);
        else if (button.candidateFixed) button.translationColor = [button.titleColor colorWithAlphaComponent:MSIMECandidateTranslationOpacity];
        else button.translationColor = button.candidateHighlighted ? SkinColor(tokens.selectedNumber) : SkinColor(tokens.number);
        button.numberColor = button.candidateHighlighted ? SkinColor(tokens.selectedNumber) : SkinColor(tokens.number);
        button.barColor = SkinColor(tokens.selectedBar.value_or(tokens.accent));
        button.showSelectedBar = tokens.showSelectedBar;
        button.cornerRadius = msime::mac::CandidateRowRadius(tokens, button.candidateHighlighted);
        button.hasCornerRadii = !tokens.explicitItemRadius && skin.layout && skin.layout->itemCornersFollowCard.value_or(false);
        if (button.hasCornerRadii) {
            const NSRect row = [button.superview convertRect:button.frame toView:content.candidateContentView], area = content.candidateRowsRect;
            button.cornerRadii = msime::mac::CandidateRowCornerRadii(
                {NSMinX(row), -NSMaxY(row), NSWidth(row), NSHeight(row)},
                {NSMinX(area), -NSMaxY(area), NSWidth(area), NSHeight(area)}, content.candidateRowsTouchTop,
                button.cornerRadius, MAX(0.01, tokens.radius), 0.5 * MSIMECandidateScale(_appearance));
        }
        button.selectionLeftInset = MSIMECandidateScale(_appearance);
        button.contentTintColor = SkinColor(tokens.text);
        button.needsDisplay = YES;
    }
    content.needsDisplay = YES;
}

- (void)selectCandidate:(MSIMECandidateButton *)button {
    if (!_activeClient || !_session || !_panel.isVisible || !button.enabled || button.superview != MSIMECandidateControlsView(_panel.contentView)) return;
    NSDictionary *identifier = button.candidateID;
    if (!MSIMECurrentCandidateIdentity(identifier, _viewState)) return;
    uint64_t generation = 0; NSUInteger index = 0;
    if (!MSIMEStrictUnsignedInteger(identifier[@"generation"], &generation) || !MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &index)) return;
    // 指针点选先开一轮；上屏音和上屏特效照常来自这次派发。
    [self reportPointerToLifecycle];
    [self dispatchSelectGeneration:generation index:index];
}

// 鼠标点网格里的候选。按钮可能是上一次重画留下的，所以它记着的身份要和网格此刻同一位置上的候选一致。
- (void)selectExpandedCandidate:(MSIMECandidateButton *)button {
    if (!_activeClient || !_session || !_panel.isVisible || !button.enabled || ![button isDescendantOf:MSIMECandidateControlsView(_panel.contentView)] ||
        !_expandedCandidates || button.tag < 0 || (NSUInteger)button.tag >= _expandedCandidates.count ||
        ![button.candidateID isEqual:_expandedCandidates[(NSUInteger)button.tag][@"id"]]) return;
    [self selectExpandedCandidateAtIndex:(NSUInteger)button.tag pointer:YES];
}

// 候选窗首行的展开按钮和网格首行的收起按钮。只读候选、不经 Engine 派发，所以不报告指针。
- (void)toggleExpandedCandidates:(MSIMECandidateButton *)button {
    if (!_activeClient || !_session || !_panel.isVisible || button.superview != MSIMECandidateControlsView(_panel.contentView)) return;
    if (_expandedCandidates) [self closeExpandedCandidates];
    else [self openExpandedCandidates];
}

// 候选菜单携带本帧身份，仅在右键请求时创建，避免每次打字构建整棵菜单树。
- (void)configureCandidateMenu:(MSIMECandidateButton *)button candidate:(NSDictionary *)candidate {
    button.menu = nil;
    __weak MSIMEInputController *weakSelf = self;
    __weak MSIMECandidateButton *weakButton = button;
    id token = _candidateMenuToken;
    button.candidateMenuProvider = ^NSMenu * {
        MSIMEInputController *controller = weakSelf;
        MSIMECandidateButton *row = weakButton;
        if (!controller || !row || controller->_candidateMenuToken != token ||
            !MSIMECurrentCandidateIdentity(candidate[@"id"], controller->_viewState)) return nil;
        if (!row.menu) row.menu = [controller menuForCandidate:candidate];
        return row.menu;
    };
    button.candidateMenuPresenter = ^(NSMenu *menu, NSEvent *event) {
        [weakSelf presentCandidateMenu:menu event:event];
    };
}

// NSMenu 会把键盘焦点从客户端窗口抢到输入法进程，Gecko 类客户端随即结束组字；这里换成不抢焦点的菜单，开在候选窗之上。
- (void)presentCandidateMenu:(NSMenu *)menu event:(NSEvent *)event {
    if (!_panel.isVisible || !event.window) return;
    if (!_candidatePointerMenu) _candidatePointerMenu = [MSIMEPointerMenu new];
    [_candidatePointerMenu popUpMenu:menu atScreenPoint:[event.window convertPointToScreen:event.locationInWindow] level:_panel.level + 1];
}

- (NSMenu *)menuForCandidate:(NSDictionary *)candidate {
    NSDictionary *identifier = candidate[@"id"];
    if (!MSIMECurrentCandidateIdentity(identifier, _viewState) || !_candidateMenuToken) return nil;
    // 打开的候选列表（编进 Engine 的韩文汉字表）里的行，Engine 不置顶、不固定、不删除，列表也不学习，所以不给菜单。
    if (MSIMECandidateListOpen(_viewState)) return nil;
    NSString *text = [candidate[@"text"] isKindOfClass:NSString.class] ? candidate[@"text"] : @"";
    NSDictionary *context = @{@"id":[identifier copy], @"text":text, @"render":_candidateMenuToken};
    NSMenuItem *(^item)(NSString *, NSInteger) = ^NSMenuItem *(NSString *title, NSInteger tag) {
        NSMenuItem *entry = [[NSMenuItem alloc] initWithTitle:title action:@selector(candidateMenuAction:) keyEquivalent:@""];
        entry.target = self;
        entry.tag = tag;
        entry.representedObject = context;
        return entry;
    };
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@"候选操作"];
    menu.autoenablesItems = NO;
    ApplyMetasequoiaMenuTheme(menu, [self resolvedMenuThemePreferences]);
    // 置顶只记在 Engine 里（`pinned_candidates`），宿主没有自己的一份可查，和 Linux 一样始终是「置顶」。
    [menu addItem:item(@"置顶", 0)];
    NSMenuItem *fixed = [[NSMenuItem alloc] initWithTitle:@"固定排位" action:nil keyEquivalent:@""];
    NSMenu *positions = [[NSMenu alloc] initWithTitle:@"固定排位"];
    positions.autoenablesItems = NO;
    ApplyMetasequoiaMenuTheme(positions, [self resolvedMenuThemePreferences]);
    const NSInteger currentFixed = MSIMEStrictInteger(candidate[@"fixed_position"], 0);
    for (NSInteger position = 1; position <= 5; ++position) {
        NSMenuItem *entry = item([NSString stringWithFormat:@"第 %ld 位", (long)position], 10 + position);
        if (currentFixed == position) entry.state = NSControlStateValueOn;
        [positions addItem:entry];
    }
    [positions addItem:NSMenuItem.separatorItem];
    [positions addItem:item(@"取消固定", 2)];
    fixed.submenu = positions;
    [menu addItem:fixed];
    // Windows hides deletion for one Unicode scalar, including supplementary Han.
    if ([text isKindOfClass:NSString.class] && [text lengthOfBytesUsingEncoding:NSUTF32LittleEndianStringEncoding] / 4 > 1) {
        NSMenuItem *remove = item(@"删除", 1);
        NSArray *candidates = _viewState.visibleCandidatesArray();
        NSUInteger slot = [candidates isKindOfClass:NSArray.class] ? [candidates indexOfObjectIdenticalTo:candidate] : NSNotFound;
        if (slot < 8) {
            remove.keyEquivalent = [NSString stringWithFormat:@"%lu", (unsigned long)slot + 1];
            remove.keyEquivalentModifierMask = NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagShift;
        }
        [menu addItem:remove];
    }
    return menu;
}

- (void)candidateMenuAction:(NSMenuItem *)item {
    if (!_activeClient || !_session || !_panel.isVisible || !item.enabled) return;
    NSDictionary *context = item.representedObject;
    if (![context isKindOfClass:NSDictionary.class] || context[@"render"] != _candidateMenuToken) return;
    NSDictionary *identifier = context[@"id"];
    if (!MSIMECurrentCandidateIdentity(identifier, _viewState)) return;
    uint64_t generation = 0; NSUInteger index = 0;
    if (!MSIMEStrictUnsignedInteger(identifier[@"generation"], &generation) || !MSIMEStrictUnsignedIntegerValue(identifier[@"index"], &index)) return;
    // 菜单项包含置顶、删除、取消固定（0-2）和固定排位（11-15），其余标签不派发，也就不报告。
    if ((item.tag < 0 || item.tag > 2) && (item.tag < 11 || item.tag > 15)) return;
    // 候选右键菜单的这几项都是指针操作引起的派发，和点选候选一样先开一轮，见 msime_client.h 的顺序契约。
    [self reportPointerToLifecycle];
    NSError *error = nil;
    NSDictionary *result = nil;
    switch (item.tag) {
        case 0: result = [_session pinGeneration:generation index:index error:&error]; break;
        case 1: result = [_session removeGeneration:generation index:index error:&error]; break;
        case 2: result = [_session clearPositionGeneration:generation index:index error:&error]; break;
        default:
            result = [_session fixGeneration:generation index:index position:(uint8_t)(item.tag - 10) error:&error];
    }
    if (result) [self apply:result];
    else if (error) NSBeep();
}

- (void)changeCandidatePage:(MSIMECandidateButton *)button {
    if (!_activeClient || !_session || !_panel.isVisible || !button.enabled || button.superview != MSIMECandidateControlsView(_panel.contentView)) return;
    if (!_viewState.valid || !_viewState.focused || !_viewState.identityValid || !_viewState.pagingValid ||
        ![button.candidateID isKindOfClass:NSDictionary.class]) return;
    uint64_t buttonSession = 0, buttonGeneration = 0;
    NSUInteger buttonPage = 0;
    if (!MSIMEStrictUnsignedInteger(button.candidateID[@"session"], &buttonSession) ||
        !MSIMEStrictUnsignedInteger(button.candidateID[@"generation"], &buttonGeneration) ||
        !MSIMEStrictUnsignedIntegerValue(button.candidateID[@"page"], &buttonPage)) return;
    if (buttonSession != _viewState.session || buttonGeneration != _viewState.generation ||
        buttonPage != _viewState.page) return;
    const NSUInteger page = _viewState.page;
    const NSUInteger count = _viewState.pageCount;
    if (count <= 1 || page >= count) return;
    // 先定下要派发的命令，报告和派发共用同一个条件，免得两边各写一份、日后改出分歧。
    uint32_t command = UINT32_MAX;
    if (button.tag == -1 && page > 0) command = MSIME_PREVIOUS_PAGE;
    else if (button.tag == -2 && page < count - 1) command = MSIME_NEXT_PAGE;
    if (command == UINT32_MAX) return;
    // 指针翻页先开一轮。
    [self reportPointerToLifecycle];
    [self dispatchCommand:command];
}
@end
