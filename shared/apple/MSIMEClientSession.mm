#import "MSIMEClientSession.h"
#import "ClipboardPreferences.h"
#include "msime_client.h"
#include <cstring>

static NSString *const MSIMEClientErrorDomain = @"app.msime.client.host";
static __weak MSIMEClientSession *gActiveSession;

static void setError(NSError **error, NSString *message) {
    if (error) *error = [NSError errorWithDomain:MSIMEClientErrorDomain code:1 userInfo:@{NSLocalizedDescriptionKey: message}];
}
static void setSnapshotError(char *rawError, NSError **error) {
    NSString *message = rawError ? @(rawError) : @"输入运行时调用失败";
    if (rawError) msime_client_string_free(rawError);
    setError(error, message);
}
static BOOL parseUInt64(id value, NSString *field, uint64_t *result, NSError **error) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) {
        setError(error, [NSString stringWithFormat:@"%@ 必须是非负整数", field]);
        return NO;
    }
    NSNumber *number = (NSNumber *)value;
    if ([number compare:@0] == NSOrderedAscending) {
        setError(error, [NSString stringWithFormat:@"%@ 必须是非负整数", field]);
        return NO;
    }
    uint64_t parsed = number.unsignedLongLongValue;
    if ([number compare:@(parsed)] != NSOrderedSame) {
        setError(error, [NSString stringWithFormat:@"%@ 超出范围", field]);
        return NO;
    }
    if (result) *result = parsed;
    return YES;
}
struct VoiceStreamContext { MSIMEVoiceProviderUpdate update; MSIMEVoiceProviderPhase phase; };
static void VoiceUpdate(const uint8_t *text, size_t length, bool final, void *opaque) { auto *c=(VoiceStreamContext *)opaque; if(!c||!c->update||!text||length>4096)return; NSString *value=[[NSString alloc] initWithBytes:text length:length encoding:NSUTF8StringEncoding]; if(value)c->update(value,final); }
static void VoicePhase(uint8_t phase, void *opaque) { auto *c=(VoiceStreamContext *)opaque; if(c&&c->phase)c->phase(phase); }

static NSMapTable<NSNumber *, MSIMEClientSession *> *gNeuralSessions = nil;
@interface MSIMEClientSession ()
- (void)handleNeuralWakeupForGeneration:(uint64_t)generation;
@end
static void MSIMENeuralWakeupCallback(void *opaque, uint64_t session, uint64_t generation) {
    (void)opaque;
    dispatch_async(dispatch_get_main_queue(), ^{
        MSIMEClientSession *target = [gNeuralSessions objectForKey:@(session)];
        [target handleNeuralWakeupForGeneration:generation];
    });
}

static id decodeValue(char *response, NSError **error) {
    if (!response) { setError(error, @"输入运行时未返回响应"); return nil; }
    NSData *data = [NSData dataWithBytes:response length:std::strlen(response)];
    msime_client_string_free(response);
    id object = [NSJSONSerialization JSONObjectWithData:data options:0 error:error];
    if (![object isKindOfClass:[NSDictionary class]]) { setError(error, @"输入运行时响应格式错误"); return nil; }
    NSDictionary *envelope = object;
    if (![envelope[@"ok"] isEqual:@YES]) {
        setError(error, [envelope[@"error"] isKindOfClass:[NSString class]] ? envelope[@"error"] : @"输入运行时调用失败");
        return nil;
    }
    return envelope[@"value"] ?: NSNull.null;
}

static NSDictionary *decode(char *response, NSError **error) {
    id value = decodeValue(response, error);
    return !value ? nil : ([value isKindOfClass:NSDictionary.class] ? value : @{});
}

@implementation MSIMEClientSession {
    uint64_t _handle;
    NSDictionary *_hostOptions;
    BOOL _dedicatedEnglishEnabled;
    NSNumber *_punctuationOverride;
    NSNumber *_pairedPunctuationOverride;
    NSNumber *_punctuationLockOverride;
    NSNumber *_characterWidthOverride;
    BOOL _capsLock;
    NSDictionary *_neuralOptions;
    void (^_neuralUpdateHandler)(uint64_t generation);
    BOOL _neuralV2Enabled;
    // 生命周期核心的选择加入和最后报告的字段属性。它们只活在会话里、不落盘，快照替换会话后在 restoreLiveModes: 里恢复。
    uint32_t _lifecycleFlags;
    uint32_t _fieldTraits;
}
- (NSDictionary *)voiceProviderRequest:(NSDictionary *)query socket:(NSString *)socket error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:query] || ![socket isKindOfClass:NSString.class] || !socket.length) { setError(error, @"语音服务请求格式错误"); return nil; }
    NSData *q = [NSJSONSerialization dataWithJSONObject:query options:0 error:error]; NSData *s = [socket dataUsingEncoding:NSUTF8StringEncoding];
    if (!q || q.length > 65536 || !s || s.length > 4096) { setError(error, @"语音服务请求过大"); return nil; }
    return decode(msime_client_voice_provider_request((const uint8_t *)q.bytes, q.length, (const uint8_t *)s.bytes, s.length), error);
}
- (BOOL)voiceProviderStream:(NSDictionary *)query socket:(NSString *)socket update:(MSIMEVoiceProviderUpdate)update phase:(MSIMEVoiceProviderPhase)phase error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:query] || ![socket isKindOfClass:NSString.class] || !socket.length || !update) { setError(error,@"语音流请求格式错误"); return NO; }
    NSData *q=[NSJSONSerialization dataWithJSONObject:query options:0 error:error], *s=[socket dataUsingEncoding:NSUTF8StringEncoding]; if(!q||q.length>65536||!s||s.length>4096){setError(error,@"语音流请求过大");return NO;}
    VoiceStreamContext context={ [update copy], [phase copy] }; char *response=msime_client_voice_provider_stream_events((const uint8_t *)q.bytes,q.length,(const uint8_t *)s.bytes,s.length,VoiceUpdate,VoicePhase,&context); BOOL ok=decode(response,error)!=nil; return ok;
}
- (BOOL)voiceProviderCancelSocket:(NSString *)socket generation:(uint64_t)generation error:(NSError **)error {
    if (![socket isKindOfClass:NSString.class] || !socket.isAbsolutePath || !socket.length) { setError(error, @"语音取消路径无效"); return NO; }
    NSData *path=[socket dataUsingEncoding:NSUTF8StringEncoding]; if(!path.length || path.length>4096){setError(error,@"语音取消路径无效");return NO;}
    return decode(msime_client_voice_provider_cancel((const uint8_t *)path.bytes,path.length,generation),error)!=nil;
}
- (BOOL)voiceProviderStopSocket:(NSString *)socket generation:(uint64_t)generation error:(NSError **)error {
    if (![socket isKindOfClass:NSString.class] || !socket.isAbsolutePath || !socket.length) { setError(error, @"语音停止路径无效"); return NO; }
    NSData *path=[socket dataUsingEncoding:NSUTF8StringEncoding]; if(!path.length || path.length>4096){setError(error,@"语音停止路径无效");return NO;}
    return decode(msime_client_voice_provider_stop((const uint8_t *)path.bytes,path.length,generation),error)!=nil;
}
+ (NSDictionary<NSString *, id> *)doubaoDecodeFrame:(NSData *)frame error:(NSError **)error {
    if (![frame isKindOfClass:NSData.class] || frame.length == 0 || frame.length > 1048576) {
        setError(error, @"Doubao 响应帧无效");
        return nil;
    }
    return decode(msime_client_doubao_decode_frame((const uint8_t *)frame.bytes, frame.length), error);
}
+ (NSData *)doubaoStartFrameEnableITN:(BOOL)enableITN punctuation:(BOOL)enablePunctuation DDC:(BOOL)enableDDC boostingTable:(NSString *)boostingTable error:(NSError **)error {
    if (![boostingTable isKindOfClass:NSString.class] || boostingTable.length > 4096) { setError(error, @"Doubao boosting table 无效"); return nil; }
    NSData *table = [boostingTable dataUsingEncoding:NSUTF8StringEncoding];
    NSMutableData *output = [NSMutableData dataWithLength:1048576]; size_t written = 0;
    BOOL ok = msime_client_doubao_start_frame(enableITN, enablePunctuation, enableDDC, (const uint8_t *)table.bytes, table.length, (uint8_t *)output.mutableBytes, output.length, &written);
    if (!ok) { setError(error, @"Doubao 开始帧生成失败"); return nil; }
    output.length = written; return output;
}
+ (NSData *)doubaoAudioFrameWithSequence:(int32_t)sequence PCM:(NSData *)pcm final:(BOOL)finalChunk error:(NSError **)error {
    if (![pcm isKindOfClass:NSData.class] || pcm.length > 1048576) { setError(error, @"Doubao 音频帧无效"); return nil; }
    NSMutableData *output = [NSMutableData dataWithLength:pcm.length + 65536]; size_t written = 0;
    BOOL ok = msime_client_doubao_audio_frame(sequence, (const uint8_t *)pcm.bytes, pcm.length, finalChunk, (uint8_t *)output.mutableBytes, output.length, &written);
    if (!ok) { setError(error, @"Doubao 音频帧生成失败"); return nil; }
    output.length = written; return output;
}
- (BOOL)restoreLiveModes:(NSError **)error {
    BOOL restored = YES;
    if (_punctuationOverride) restored = decode(msime_client_set_chinese_punctuation(_handle, _punctuationOverride.boolValue), error) != nil;
    if (restored && _pairedPunctuationOverride) restored = decode(msime_client_set_paired_punctuation(_handle, _pairedPunctuationOverride.boolValue), error) != nil;
    if (restored && _punctuationLockOverride) restored = decode(msime_client_set_punctuation_lock(_handle, _punctuationLockOverride.unsignedCharValue), error) != nil;
    if (restored && _characterWidthOverride) restored = decode(msime_client_set_character_width(_handle, _characterWidthOverride.boolValue), error) != nil;
    if (restored && _dedicatedEnglishEnabled) restored = decode(msime_client_set_english_mode(_handle, true), error) != nil;
    if (restored && _capsLock) restored = decodeValue(msime_client_set_caps_lock(_handle, true), error) != nil;
    if (restored && _lifecycleFlags) {
        _lifecycleFlags = msime_client_set_lifecycle(_handle, _lifecycleFlags);
        if (_fieldTraits) _fieldTraits = msime_client_set_field_traits(_handle, _fieldTraits);
    }
    if (restored && (_neuralOptions || _neuralUpdateHandler)) {
        restored = [self configureNeuralServiceWithOptions:_neuralOptions updateHandler:_neuralUpdateHandler error:error] != nil;
    }
    if (restored) return YES;
    // Do not expose a replacement session silently running in the wrong mode.
    [gNeuralSessions removeObjectForKey:@(_handle)];
    msime_client_string_free(msime_client_destroy(_handle));
    _handle = 0;
    _neuralV2Enabled = NO;
    return NO;
}
- (NSDictionary *)hostOptions { return _hostOptions; }
- (BOOL)neuralV2Enabled { return _neuralV2Enabled; }
- (void)handleNeuralWakeupForGeneration:(uint64_t)generation {
    if (_handle && _neuralUpdateHandler) _neuralUpdateHandler(generation);
}
- (nullable NSDictionary *)configureNeuralServiceWithOptions:(nullable NSDictionary<NSString *, id> *)options
                                               updateHandler:(nullable void (^)(uint64_t generation))updateHandler
                                                       error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    if (!gNeuralSessions) gNeuralSessions = [NSMapTable strongToWeakObjectsMapTable];
    _neuralOptions = [options isKindOfClass:NSDictionary.class] ? [options copy] : nil;
    _neuralUpdateHandler = [updateHandler copy];
    if (_neuralUpdateHandler) [gNeuralSessions setObject:self forKey:@(_handle)];
    else [gNeuralSessions removeObjectForKey:@(_handle)];
    NSData *data = nil;
    if (_neuralOptions) {
        if (![NSJSONSerialization isValidJSONObject:_neuralOptions]) {
            setError(error, @"神经服务配置必须是 JSON 对象");
            return nil;
        }
        data = [NSJSONSerialization dataWithJSONObject:_neuralOptions options:0 error:error];
        if (!data) return nil;
    }
    NSDictionary *result = decode(
        msime_client_configure_neural_service(
            _handle,
            data ? static_cast<const uint8_t *>(data.bytes) : nullptr,
            data ? data.length : 0,
            _neuralUpdateHandler ? MSIMENeuralWakeupCallback : nullptr,
            nullptr),
        error);
    _neuralV2Enabled = result && [result[@"enabled"] isEqual:@YES];
    return result;
}
- (nullable NSDictionary *)pollNeuralWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_poll_neural(_handle), error);
}
- (nullable NSDictionary *)flushProvisionalNeuralWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_flush_provisional_neural(_handle), error);
}
- (BOOL)setNeuralV2Enabled:(BOOL)enabled error:(NSError **)error {
    return decodeValue(msime_client_set_neural_v2_enabled(_handle, enabled), error) != nil;
}
- (NSDictionary *)takeNeuralRequestWithError:(NSError **)error {
    id value = decodeValue(msime_client_take_neural_request(_handle), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
- (NSString *)neuralPrefillContextWithError:(NSError **)error {
    id value = decodeValue(msime_client_neural_prefill_context(_handle), error);
    if (![value isKindOfClass:NSDictionary.class]) return nil;
    id ctx = value[@"context"];
    return [ctx isKindOfClass:NSString.class] ? ctx : @"";
}
- (NSDictionary *)applyNeuralResponseData:(NSData *)responseData
                                lockSlot0:(BOOL)lockSlot0
                                    error:(NSError **)error {
    if (![responseData isKindOfClass:NSData.class] || responseData.length == 0 || responseData.length > 262144) {
        setError(error, @"神经解码响应数据无效");
        return nil;
    }
    id value = decodeValue(
        msime_client_apply_neural_response(
            _handle,
            static_cast<const uint8_t *>(responseData.bytes),
            responseData.length,
            lockSlot0),
        error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
- (NSDictionary *)onlineQueryWithError:(NSError **)error {
    id value = decodeValue(msime_client_online_query(_handle), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
- (NSDictionary *)aiRequestForQuery:(NSDictionary *)query error:(NSError **)error {
    if (![query isKindOfClass:NSDictionary.class] || ![NSJSONSerialization isValidJSONObject:query]) {
        setError(error, @"AI 查询格式错误"); return nil;
    }
    NSData *data = [NSJSONSerialization dataWithJSONObject:query options:0 error:error];
    if (!data || data.length > MSIME_ONLINE_QUERY_MAX_BYTES) { setError(error, @"AI 查询过大"); return nil; }
    id value = decodeValue(msime_client_ai_request_for_query(
        _handle, static_cast<const uint8_t *>(data.bytes), data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
- (NSDictionary *)translationQueryWithError:(NSError **)error {
    id value = decodeValue(msime_client_translation_query(_handle), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
- (NSDictionary *)applyOnlineCandidates:(NSArray<NSString *> *)candidates source:(NSUInteger)source
                                  query:(NSDictionary *)query error:(NSError **)error {
    if (source > 1 || ![candidates isKindOfClass:NSArray.class] || candidates.count > 10 ||
        ![query isKindOfClass:NSDictionary.class] || ![NSJSONSerialization isValidJSONObject:query]) {
        setError(error, @"在线候选格式错误"); return nil;
    }
    for (id candidate in candidates) {
        if (![candidate isKindOfClass:NSString.class] || ![candidate length] ||
            [candidate lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > 4096) {
            setError(error, @"在线候选格式错误"); return nil;
        }
    }
    NSData *queryData = [NSJSONSerialization dataWithJSONObject:query options:0 error:error];
    NSData *candidateData = [NSJSONSerialization dataWithJSONObject:candidates options:0 error:error];
    if (!queryData || !candidateData || queryData.length > MSIME_ONLINE_QUERY_MAX_BYTES || candidateData.length > 16384) {
        setError(error, @"在线候选请求过大"); return nil;
    }
    return decode(msime_client_apply_online_candidates(_handle, (const uint8_t *)queryData.bytes, queryData.length,
        (const uint8_t *)candidateData.bytes, candidateData.length, (uint8_t)source), error);
}
- (NSDictionary *)resolveOnlineCandidatesForSource:(NSUInteger)source
                                            query:(NSDictionary *)query error:(NSError **)error {
    if (source > 1 || ![query isKindOfClass:NSDictionary.class] || ![NSJSONSerialization isValidJSONObject:query]) {
        setError(error, @"在线候选格式错误"); return nil;
    }
    NSData *queryData = [NSJSONSerialization dataWithJSONObject:query options:0 error:error];
    if (!queryData || queryData.length > MSIME_ONLINE_QUERY_MAX_BYTES) { setError(error, @"在线候选请求过大"); return nil; }
    return decode(msime_client_resolve_online_candidates(_handle, (const uint8_t *)queryData.bytes, queryData.length,
        (uint8_t)source), error);
}
+ (NSDictionary *)crossCommitSpacingRequest:(NSDictionary *)request {
    if (![NSJSONSerialization isValidJSONObject:request]) return nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    if (!data || data.length > 65536) return nil;
    return decode(msime_client_cross_commit_spacing((const uint8_t *)data.bytes, data.length), nil);
}
+ (NSDictionary *)customTranslationHTTPRequest:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"自定义翻译请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 16384) { setError(error, @"自定义翻译请求过大"); return nil; }
    id value = decodeValue(msime_client_custom_translation_http_request((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
+ (NSDictionary *)tencentTranslationHTTPRequest:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"腾讯翻译请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"腾讯翻译请求过大"); return nil; }
    id value = decodeValue(msime_client_tencent_translation_http_request((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
+ (NSDictionary *)niuTransTranslationHTTPRequest:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"小牛翻译请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"小牛翻译请求过大"); return nil; }
    id value = decodeValue(msime_client_niutrans_translation_http_request((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
+ (NSDictionary *)aiTranslationHTTPRequest:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"AI 翻译请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"AI 翻译请求过大"); return nil; }
    id value = decodeValue(msime_client_ai_translation_http_request((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
+ (NSString *)parseNiuTransTranslationResponse:(NSData *)body error:(NSError **)error {
    if (![body isKindOfClass:NSData.class] || body.length > 1048576) {
        setError(error, @"小牛翻译响应格式错误或过大"); return nil;
    }
    if (!body.length) return nil;
    id value = decodeValue(msime_client_parse_niutrans_translation_response((const uint8_t *)body.bytes, body.length), error);
    return [value isKindOfClass:NSString.class] ? value : nil;
}
+ (NSString *)parseAITranslationResponse:(NSData *)body error:(NSError **)error {
    if (![body isKindOfClass:NSData.class] || body.length > 1048576) { setError(error, @"AI 翻译响应格式错误或过大"); return nil; }
    if (!body.length) return nil;
    id value = decodeValue(msime_client_parse_ai_translation_response((const uint8_t *)body.bytes, body.length), error);
    return [value isKindOfClass:NSString.class] ? value : nil;
}
+ (NSString *)formatTranslationGloss:(NSString *)gloss error:(NSError **)error {
    NSData *data = [gloss isKindOfClass:NSString.class] ? [gloss dataUsingEncoding:NSUTF8StringEncoding] : nil;
    if (!data || data.length > 65536) { setError(error, @"翻译释义格式错误或过大"); return nil; }
    if (!data.length) return nil;
    id value = decodeValue(msime_client_format_translation_gloss((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSString.class] ? value : nil;
}
+ (NSDictionary *)aiHTTPRequest:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"AI 请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"AI 请求过大"); return nil; }
    id value = decodeValue(msime_client_ai_http_request((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
+ (NSArray<NSString *> *)parseAIResponse:(NSData *)body limit:(NSUInteger)limit error:(NSError **)error {
    if (![body isKindOfClass:NSData.class] || body.length > 1048576 || limit < 1 || limit > 10) {
        setError(error, @"AI 响应格式错误或过大"); return nil;
    }
    if (!body.length) return nil;
    id value = decodeValue(msime_client_parse_ai_response((const uint8_t *)body.bytes, body.length, (uint8_t)limit), error);
    return [value isKindOfClass:NSArray.class] ? value : nil;
}
+ (NSDictionary *)learnedTranslationRequest:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"用户释义请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"用户释义请求过大"); return nil; }
    id value = decodeValue(msime_client_learned_translation_request((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
+ (NSArray *)parseTencentTranslationResponse:(NSData *)body expectedCount:(NSUInteger)count error:(NSError **)error {
    if (![body isKindOfClass:NSData.class] || body.length > 1048576 || count < 1 || count > 10) {
        setError(error, @"腾讯翻译响应格式错误或过大"); return nil;
    }
    if (!body.length) return nil;
    id value = decodeValue(msime_client_parse_tencent_translation_response((const uint8_t *)body.bytes, body.length, count), error);
    return [value isKindOfClass:NSArray.class] ? value : nil;
}
+ (BOOL)customTranslationReplyFailed:(NSData *)body {
    if (![body isKindOfClass:NSData.class]) return YES;
    return msime_client_custom_translation_reply_failed((const uint8_t *)body.bytes, body.length);
}
+ (BOOL)niuTransTranslationReplyFailed:(NSData *)body {
    if (![body isKindOfClass:NSData.class]) return YES;
    return msime_client_niutrans_translation_reply_failed((const uint8_t *)body.bytes, body.length);
}
+ (NSArray<NSDictionary *> *)customTranslationPlan:(NSDictionary *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"翻译计划格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"翻译计划过大"); return nil; }
    id value = decodeValue(msime_client_custom_translation_plan((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSArray.class] ? value : nil;
}
+ (NSString *)parseCustomTranslationResponse:(NSData *)body error:(NSError **)error {
    if (![body isKindOfClass:NSData.class] || body.length > 1048576) { setError(error, @"自定义翻译响应格式错误或过大"); return nil; }
    // NSData may expose a null bytes pointer for an empty buffer.
    if (!body.length) return nil;
    id value = decodeValue(msime_client_parse_custom_translation_response((const uint8_t *)body.bytes, body.length), error);
    return [value isKindOfClass:NSString.class] ? value : nil;
}
+ (NSDictionary *)candidateGlossRequest:(NSDictionary *)request resources:(NSString *)resources error:(NSError **)error {
    if (![resources isKindOfClass:NSString.class] || !resources.isAbsolutePath ||
        ![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"候选释义请求格式错误"); return nil; }
    NSData *path = [resources dataUsingEncoding:NSUTF8StringEncoding];
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!path || path.length > 4096 || !data || data.length > 262144) { setError(error, @"候选释义请求过大"); return nil; }
    return decode(msime_client_candidate_gloss_request((const uint8_t *)data.bytes, data.length,
        (const uint8_t *)path.bytes, path.length), error);
}
+ (NSDictionary *)pronunciationRequest:(NSDictionary *)request resources:(NSString *)resources error:(NSError **)error {
    if (![resources isKindOfClass:NSString.class] || !resources.isAbsolutePath ||
        ![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"读音请求格式错误"); return nil; }
    NSData *path = [resources dataUsingEncoding:NSUTF8StringEncoding];
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!path || path.length > 4096 || !data || data.length > 262144) { setError(error, @"读音请求过大"); return nil; }
    return decode(msime_client_pronunciation_request((const uint8_t *)data.bytes, data.length,
        (const uint8_t *)path.bytes, path.length), error);
}
+ (NSDictionary *)glossBreakdownRequest:(NSDictionary *)request resources:(NSString *)resources error:(NSError **)error {
    if (![resources isKindOfClass:NSString.class] || !resources.isAbsolutePath ||
        ![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"逐词释义请求格式错误"); return nil; }
    NSData *path = [resources dataUsingEncoding:NSUTF8StringEncoding];
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!path || path.length > 4096 || !data || data.length > 65536) { setError(error, @"逐词释义请求过大"); return nil; }
    return decode(msime_client_gloss_breakdown_request((const uint8_t *)data.bytes, data.length,
        (const uint8_t *)path.bytes, path.length), error);
}
- (NSDictionary *)applyTranslations:(NSArray<NSDictionary *> *)translations generation:(uint64_t)generation error:(NSError **)error {
    if (![translations isKindOfClass:NSArray.class] || translations.count > 4096 ||
        ![NSJSONSerialization isValidJSONObject:translations]) { setError(error, @"候选释义格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:translations options:0 error:error];
    if (!data || data.length > 1048576) { setError(error, @"候选释义过大"); return nil; }
    return decode(msime_client_apply_translations(_handle, generation, (const uint8_t *)data.bytes, data.length), error);
}
+ (NSString *)cloudRequestURLForQuery:(NSDictionary *)query error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:query]) { setError(error, @"在线查询格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:query options:0 error:error];
    if (!data || data.length > MSIME_ONLINE_QUERY_MAX_BYTES) { setError(error, @"在线查询过大"); return nil; }
    id value = decodeValue(msime_client_cloud_request_url((const uint8_t *)data.bytes, data.length), error);
    return [value isKindOfClass:NSString.class] ? value : nil;
}
- (NSDictionary *)applyCloudResponse:(NSData *)body query:(NSDictionary *)query error:(NSError **)error {
    if (![body isKindOfClass:NSData.class] || body.length == 0 || body.length > 262144 ||
        ![NSJSONSerialization isValidJSONObject:query]) { setError(error, @"云候选响应格式错误或过大"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:query options:0 error:error];
    if (!data || data.length > MSIME_ONLINE_QUERY_MAX_BYTES) { setError(error, @"在线查询过大"); return nil; }
    return decode(msime_client_apply_cloud_response(_handle, (const uint8_t *)data.bytes, data.length,
        (const uint8_t *)body.bytes, body.length), error);
}
+ (NSDictionary *)dictionaryRequest:(NSDictionary<NSString *, id> *)request error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"词典请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 65536) { setError(error, @"词典请求过大"); return nil; }
    return decode(msime_client_dictionary(static_cast<const uint8_t *>(data.bytes), data.length), error);
}
static void MSIMEInvokeAnnounce(void *context) {
    ((__bridge void (^)(void))context)();
}
+ (NSDictionary *)settingsDictionaryRequest:(NSDictionary<NSString *, id> *)request
                                   announce:(void (^)(void))announce
                                      error:(NSError **)error {
    if (![NSJSONSerialization isValidJSONObject:request]) { setError(error, @"词典请求格式错误"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data) return nil;
    void (^block)(void) = [announce copy];
    return decode(msime_client_settings_dictionary_announced(static_cast<const uint8_t *>(data.bytes), data.length,
        block ? MSIMEInvokeAnnounce : nullptr, (__bridge void *)block), error);
}
+ (NSDictionary *)scelText:(NSData *)bytes error:(NSError **)error {
    if (![bytes isKindOfClass:NSData.class]) { setError(error, @"dictionary_scel_invalid"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:@{ @"bytes_base64": [bytes base64EncodedStringWithOptions:0] }
                                                   options:0 error:error];
    if (!data) return nil;
    return decode(msime_client_dictionary_scel_text(static_cast<const uint8_t *>(data.bytes), data.length), error);
}
+ (NSDictionary *)captureClipboardHistoryRequest:(NSDictionary<NSString *, id> *)request {
    if (![NSJSONSerialization isValidJSONObject:request]) return @{ @"error": @YES };
    NSError *error = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:&error];
    if (!data || data.length > 131072) return @{ @"error": @YES };
    NSDictionary *result = decode(msime_client_capture_clipboard_history(
        static_cast<const uint8_t *>(data.bytes), data.length), &error);
    return result ?: @{ @"error": @YES };
}
+ (NSDictionary *)removeClipboardHistoryRequest:(NSDictionary<NSString *, id> *)request {
    if (![NSJSONSerialization isValidJSONObject:request]) return @{ @"error": @YES };
    NSError *error = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:&error];
    if (!data || data.length > 131072) return @{ @"error": @YES };
    NSDictionary *result = decode(msime_client_remove_clipboard_history(
        static_cast<const uint8_t *>(data.bytes), data.length), &error);
    return result ?: @{ @"error": @YES };
}
+ (NSDictionary *)clipboardCaptureEnabledRequest:(NSString *)directory {
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) return @{ @"error": @YES };
    NSDictionary *snapshot = [self loadPreferencesInDirectory:directory error:nil];
    id enabled = snapshot[@"preferences"][@"clipboard_history"];
    return [enabled isKindOfClass:NSNumber.class] ? @{ @"enabled": enabled } : @{ @"error": @YES };
}
+ (NSDictionary *)enableClipboardHistoryRequest:(NSString *)directory {
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) return @{ @"error": @YES };
    return MSIMEEnableClipboardHistory(^NSDictionary *{
        return [self loadPreferencesInDirectory:directory error:nil];
    }, ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
        return [self savePreferencesInDirectory:directory expectedRevision:revision snapshot:snapshot error:nil];
    });
}
+ (NSDictionary *)clipboardHistoryRequest:(NSString *)directory {
    NSError *error = nil;
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) {
        return @{ @"error": @YES };
    }
    NSData *path = [directory dataUsingEncoding:NSUTF8StringEncoding];
    if (!path || path.length > 16384) return @{ @"error": @YES };
    NSDictionary *result = decode(msime_client_load_clipboard_history(
        static_cast<const uint8_t *>(path.bytes), path.length), &error);
    return result ?: @{ @"error": @YES };
}
+ (NSDictionary *)emojiCatalogRequest:(NSDictionary<NSString *, id> *)request {
    NSError *error = nil;
    if (![request isKindOfClass:NSDictionary.class]) {
        return @{ @"error": [NSError errorWithDomain:MSIMEClientErrorDomain code:1 userInfo:nil] };
    }
    NSString *resources = request[@"resources"];
    if (![resources isKindOfClass:NSString.class] || !resources.isAbsolutePath ||
        ![NSJSONSerialization isValidJSONObject:request]) {
        return @{ @"error": [NSError errorWithDomain:MSIMEClientErrorDomain code:1 userInfo:nil] };
    }
    NSData *query = [NSJSONSerialization dataWithJSONObject:request options:0 error:&error];
    NSData *path = [resources dataUsingEncoding:NSUTF8StringEncoding];
    if (!query || query.length > 16384 || path.length > 4096) {
        return @{ @"error": [NSError errorWithDomain:MSIMEClientErrorDomain code:1 userInfo:nil] };
    }
    NSDictionary *result = decode(msime_client_emoji_catalog_request(
        static_cast<const uint8_t *>(query.bytes), query.length,
        static_cast<const uint8_t *>(path.bytes), path.length), &error);
    return result ?: @{ @"error": error ?: [NSError errorWithDomain:MSIMEClientErrorDomain code:1 userInfo:nil] };
}
+ (NSDictionary *)prepareHostWithResourcesDirectory:(NSString *)resourcesDirectory stateRoot:(NSString *)stateRoot error:(NSError **)error {
    if (![resourcesDirectory isKindOfClass:NSString.class] || ![stateRoot isKindOfClass:NSString.class] ||
        !resourcesDirectory.isAbsolutePath || !stateRoot.isAbsolutePath || resourcesDirectory.length == 0 || stateRoot.length == 0) {
        setError(error, @"词库准备目录必须是绝对路径"); return nil;
    }
    NSDictionary *request = @{@"resources": resourcesDirectory, @"state_root": stateRoot};
    NSData *data = [NSJSONSerialization dataWithJSONObject:request options:0 error:error];
    if (!data || data.length > 16384) { setError(error, @"词库准备请求过大"); return nil; }
    return decode(msime_client_prepare_host(static_cast<const uint8_t *>(data.bytes), data.length), error);
}
+ (NSDictionary *)savePreferencesInDirectory:(NSString *)directory expectedRevision:(uint64_t)revision snapshot:(NSDictionary *)snapshot error:(NSError **)error {
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath || ![NSJSONSerialization isValidJSONObject:snapshot]) { setError(error, @"偏好保存参数无效"); return nil; }
    NSData *dir = [directory dataUsingEncoding:NSUTF8StringEncoding];
    NSData *data = [NSJSONSerialization dataWithJSONObject:snapshot options:0 error:error];
    if (!data || data.length > 1048576) { setError(error, @"偏好快照过大"); return nil; }
    return decode(msime_client_save_preferences(static_cast<const uint8_t *>(dir.bytes), dir.length, revision, static_cast<const uint8_t *>(data.bytes), data.length), error);
}
+ (NSDictionary *)loadPreferencesInDirectory:(NSString *)directory error:(NSError **)error {
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath || directory.length == 0) { setError(error, @"偏好目录必须是绝对路径"); return nil; }
    NSData *dir = [directory dataUsingEncoding:NSUTF8StringEncoding];
    return decode(msime_client_load_preferences(static_cast<const uint8_t *>(dir.bytes), dir.length), error);
}
+ (NSDictionary *)recoverPreferencesInDirectory:(NSString *)directory error:(NSError **)error {
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath || directory.length == 0) { setError(error, @"偏好目录必须是绝对路径"); return nil; }
    NSData *dir = [directory dataUsingEncoding:NSUTF8StringEncoding];
    return decode(msime_client_recover_preferences(static_cast<const uint8_t *>(dir.bytes), dir.length), error);
}
- (nullable instancetype)initWithOptions:(NSDictionary<NSString *, id> *)options error:(NSError **)error {
    if (![NSThread isMainThread]) { setError(error, @"输入会话必须在主线程创建"); return nil; }
    self = [super init];
    if (!self) return nil;
    if (![NSJSONSerialization isValidJSONObject:options]) { setError(error, @"输入会话配置必须是 JSON 对象"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:options options:0 error:error];
    if (!data) return nil;
    // Retain the exact immutable configuration sent to the host, not mutable
    // nested dictionaries owned by the caller and reused during recovery.
    _hostOptions = [NSJSONSerialization JSONObjectWithData:data options:0 error:error];
    if (!_hostOptions) return nil;
    NSDictionary *view = decode(msime_client_create(static_cast<const uint8_t *>(data.bytes), data.length), error);
    if (!view) return nil;
    if (!parseUInt64(view[@"session"], @"输入会话句柄", &_handle, error) || !_handle) {
        if (!_handle && error && !*error) setError(error, @"输入会话句柄无效");
        return nil;
    }
    if (!gActiveSession) gActiveSession = self;
    return self;
}

- (BOOL)checkThreadAndHandle:(NSError **)error {
    if (![NSThread isMainThread]) { setError(error, @"输入会话必须在主线程调用"); return NO; }
    if (!_handle) { setError(error, @"输入会话已关闭"); return NO; }
    return YES;
}

- (nullable NSDictionary *)setFocused:(BOOL)focused error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    NSDictionary *result = decode(msime_client_focus(_handle, focused), error);
    // Keep the last focused session available while a dictionary/settings window
    // has focus; constructing an unrelated session must not steal its target.
    if (result && focused) gActiveSession = self;
    return result;
}
- (nullable NSDictionary *)setEnglishMode:(BOOL)enabled error:(NSError **)error {
    return [self setDedicatedEnglishEnabled:enabled error:error];
}
- (nullable NSDictionary *)setChinesePunctuationEnabled:(BOOL)enabled error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    NSDictionary *view = decode(msime_client_set_chinese_punctuation(_handle, enabled), error);
    if (view) _punctuationOverride = @(enabled);
    return view;
}
- (nullable NSDictionary *)setPairedPunctuationEnabled:(BOOL)enabled error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    NSDictionary *view = decode(msime_client_set_paired_punctuation(_handle, enabled), error);
    if (view) _pairedPunctuationOverride = @(enabled);
    return view;
}
- (BOOL)balancePairedPunctuationAfterAutoClose:(uint8_t)opening error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return NO;
    return decode(msime_client_balance_paired_punctuation_after_auto_close(_handle, opening), error) != nil;
}
- (nullable NSDictionary *)setPunctuationLock:(NSString *)lock error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    uint8_t value = 0;
    if ([lock isEqualToString:@"chinese"]) value = 1;
    else if ([lock isEqualToString:@"english"]) value = 2;
    else if (![lock isEqualToString:@"follow"]) { setError(error, @"标点锁定值无效"); return nil; }
    NSDictionary *view = decode(msime_client_set_punctuation_lock(_handle, value), error);
    if (view) _punctuationLockOverride = @(value);
    return view;
}
- (BOOL)setCapsLockEnabled:(BOOL)enabled error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return NO;
    if (!decodeValue(msime_client_set_caps_lock(_handle, enabled), error)) return NO;
    _capsLock = enabled;
    return YES;
}
- (BOOL)setSurroundingContext:(NSString *)context error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return NO;
    NSData *utf8 = [(context ?: @"") dataUsingEncoding:NSUTF8StringEncoding];
    if (!utf8) return NO;
    // 超过单次 C 接口字节上限时先按 UTF-16 安全截尾，保留紧邻光标的一段。
    if (utf8.length > 4096) {
        NSUInteger keepUnits = MIN(context.length, (NSUInteger)1024);
        NSUInteger start = context.length - keepUnits;
        if (start > 0 && CFStringIsSurrogateLowCharacter([context characterAtIndex:start])) ++start;
        utf8 = [[context substringFromIndex:start] dataUsingEncoding:NSUTF8StringEncoding];
        if (!utf8 || utf8.length > 4096) return NO;
    }
    id value = decodeValue(msime_client_set_surrounding_context(
        _handle, static_cast<const uint8_t *>(utf8.bytes), utf8.length), error);
    return [value isEqual:@YES];
}
- (BOOL)setFollowingContext:(NSString *)context error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return NO;
    NSString *input = context ?: @"";
    if (input.length > 64) {
        NSUInteger keepUnits = 64;
        if (CFStringIsSurrogateHighCharacter([input characterAtIndex:keepUnits - 1])) --keepUnits;
        input = [input substringToIndex:keepUnits];
    }
    NSData *utf8 = [input dataUsingEncoding:NSUTF8StringEncoding];
    if (!utf8 || utf8.length > 4096) return NO;
    id value = decodeValue(msime_client_set_following_context(
        _handle, static_cast<const uint8_t *>(utf8.bytes), utf8.length), error);
    return [value isEqual:@YES];
}
- (nullable NSDictionary *)setCharacterWidthFull:(BOOL)fullwidth error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    NSDictionary *view = decode(msime_client_set_character_width(_handle, fullwidth), error);
    if (view) _characterWidthOverride = @(fullwidth);
    return view;
}
- (nullable NSDictionary *)typeASCII:(uint8_t)character shift:(BOOL)shift error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_character(_handle, character, shift), error);
}
- (nullable NSDictionary *)punctuationASCII:(uint8_t)ascii error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_punctuation_ascii(_handle, ascii), error);
}
- (nullable NSDictionary *)punctuation:(uint8_t)ascii error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_punctuation(_handle, ascii), error);
}
- (nullable NSDictionary *)punctuation:(uint8_t)ascii preceding:(uint32_t)preceding error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_punctuation_with_context(_handle, ascii, preceding), error);
}
- (nullable NSDictionary *)languageCycleEnabled:(BOOL)enabled
                                           mode:(uint8_t)mode
                                         scheme:(uint8_t)scheme
                              lastChineseScheme:(int8_t)lastChineseScheme
                                 hasComposition:(BOOL)hasComposition {
    // 只从会话线程问，和其它入口一样：它读的那几项由控制器在同一线程上填。
    if (![self checkThreadAndHandle:nil]) return nil;
    // 共享层用 `value: null` 表示「不接管」；decode 不返回候选动作，调用方回到原有切换路径。
    return decode(msime_client_language_cycle(enabled, mode, scheme, lastChineseScheme, hasComposition), nil);
}
- (nullable NSDictionary *)command:(uint32_t)command error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_command(_handle, command), error);
}
- (nullable NSDictionary *)resetCacheWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_reset_cache(_handle), error);
}
- (nullable NSDictionary *)selectGeneration:(uint64_t)generation index:(NSUInteger)index error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_select(_handle, generation, index), error);
}
- (nullable NSDictionary *)selectEdgeGeneration:(uint64_t)generation index:(NSUInteger)index edge:(uint8_t)edge error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_select_edge(_handle, generation, index, edge), error);
}
- (nullable NSDictionary *)viewWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_view(_handle), error);
}
- (nullable const ShurufaViewSnapshot *)viewSnapshotWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaViewSnapshot *snapshot = shurufa_session_view(_handle, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)typeASCIISnapshot:(uint8_t)character shift:(BOOL)shift error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_character(_handle, character, shift, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)commandSnapshot:(uint32_t)command error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_command(_handle, command, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)punctuationSnapshot:(uint8_t)character
                                                        preceding:(uint32_t)preceding
                                                     hasPreceding:(BOOL)hasPreceding
                                                     literalASCII:(BOOL)literalASCII
                                                            error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_punctuation(
        _handle, character, preceding, hasPreceding, literalASCII, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)selectSnapshotGeneration:(uint64_t)generation
                                                                 index:(NSUInteger)index
                                                                 error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_select(_handle, generation, index, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)selectAnyCandidateSnapshotGeneration:(uint64_t)generation
                                                                             index:(NSUInteger)index
                                                                             error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_select_any(_handle, generation, index, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)selectEdgeSnapshotGeneration:(uint64_t)generation
                                                                     index:(NSUInteger)index
                                                                      edge:(uint8_t)edge
                                                                     error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_select_edge(_handle, generation, index, edge, &rawError);
    if (!snapshot) setSnapshotError(rawError, error);
    return snapshot;
}
- (nullable const ShurufaTransitionSnapshot *)setFocusedSnapshot:(BOOL)focused error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    const ShurufaTransitionSnapshot *snapshot = shurufa_session_focus(_handle, focused, &rawError);
    if (!snapshot) {
        setSnapshotError(rawError, error);
        return nullptr;
    }
    if (focused) gActiveSession = self;
    return snapshot;
}
- (nullable const ShurufaViewSnapshot *)pollNeuralSnapshotReady:(BOOL *)ready
                                                          moved:(BOOL *)moved
                                                          error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    bool readyValue = false;
    bool movedValue = false;
    const ShurufaViewSnapshot *snapshot = shurufa_session_poll_neural(
        _handle, ready ? &readyValue : nullptr, moved ? &movedValue : nullptr, &rawError);
    if (!snapshot) {
        setSnapshotError(rawError, error);
        return nullptr;
    }
    if (ready) *ready = readyValue ? YES : NO;
    if (moved) *moved = movedValue ? YES : NO;
    return snapshot;
}
- (nullable const ShurufaViewSnapshot *)flushProvisionalNeuralSnapshotFlushed:(BOOL *)flushed
                                                                       error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nullptr;
    char *rawError = nullptr;
    bool flushedValue = false;
    const ShurufaViewSnapshot *snapshot = shurufa_session_flush_provisional_neural(
        _handle, flushed ? &flushedValue : nullptr, &rawError);
    if (!snapshot) {
        setSnapshotError(rawError, error);
        return nullptr;
    }
    if (flushed) *flushed = flushedValue ? YES : NO;
    return snapshot;
}
- (nullable NSDictionary *)allCandidatesWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_all_candidates(_handle), error);
}
- (nullable NSDictionary *)setDisplayPageGeneration:(uint64_t)generation indices:(NSArray<NSNumber *> *)indices error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    if (indices.count > 256) { setError(error, @"显示页座位过多"); return nil; }
    size_t seats[256];
    for (NSUInteger slot = 0; slot < indices.count; ++slot) {
        if (![indices[slot] isKindOfClass:NSNumber.class]) { setError(error, @"显示页座位格式错误"); return nil; }
        seats[slot] = indices[slot].unsignedLongValue;
    }
    return decode(msime_client_set_display_page(_handle, generation, seats, indices.count), error);
}
- (nullable NSDictionary *)selectAnyCandidateGeneration:(uint64_t)generation index:(NSUInteger)index error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_select_any_candidate(_handle, generation, index), error);
}
- (nullable NSDictionary *)setDedicatedEnglishEnabled:(BOOL)enabled error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    NSDictionary *view = decode(msime_client_set_english_mode(_handle, enabled), error);
    if (view) _dedicatedEnglishEnabled = enabled;
    return view;
}
- (nullable NSDictionary *)pinGeneration:(uint64_t)generation index:(NSUInteger)index error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_pin_candidate(_handle, generation, index), error);
}
- (nullable NSDictionary *)removeGeneration:(uint64_t)generation index:(NSUInteger)index error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_remove_candidate(_handle, generation, index), error);
}
// No NSError and no main-thread check here: these run on every key press, and the host library already answers false for a closed handle or another thread.
- (BOOL)keySound:(uint32_t)keyClass {
    return _handle && msime_client_key_sound(_handle, keyClass);
}
- (BOOL)commitSound {
    return _handle && msime_client_commit_sound(_handle);
}
- (BOOL)setMusicActive:(BOOL)active {
    return _handle && msime_client_music_set_active(_handle, active);
}
- (uint32_t)typingEffect:(uint32_t)event {
    return _handle ? msime_client_typing_effect(_handle, event) : 0;
}
// 同上，按键路径上不做主线程检查：库对已关闭的句柄和别的线程本来就返回 0 或 false。
- (uint32_t)setLifecycle:(uint32_t)flags {
    if (!_handle) return 0;
    _lifecycleFlags = msime_client_set_lifecycle(_handle, flags);
    return _lifecycleFlags;
}
- (uint32_t)lifecycleFlags { return _lifecycleFlags; }
- (uint32_t)lifecycleKey:(uint32_t)key {
    return _handle ? msime_client_key(_handle, key) : 0;
}
- (uint32_t)setFieldTraits:(uint32_t)traits {
    if (!_handle) return 0;
    const uint32_t applied = msime_client_set_field_traits(_handle, traits);
    // 没选择加入时库返回 0，也就没有可恢复的属性。
    _fieldTraits = applied;
    return applied;
}
- (BOOL)setLifecycleActive:(BOOL)active {
    return _handle && msime_client_set_active(_handle, active);
}
- (nullable NSDictionary *)typingEffectSettingsWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_typing_effect_settings(_handle), error);
}
- (nullable NSDictionary *)setCandidatePageSize:(uint8_t)size error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    return decode(msime_client_set_candidate_page_size(_handle, size), error);
}
- (nullable NSDictionary *)updatePreferencesSnapshot:(NSDictionary<NSString *, id> *)snapshot error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    if (![NSJSONSerialization isValidJSONObject:snapshot]) { setError(error, @"偏好快照必须是 JSON 对象"); return nil; }
    NSData *data = [NSJSONSerialization dataWithJSONObject:snapshot options:0 error:error];
    if (!data) return nil;
    NSDictionary *result = decode(msime_client_update_preferences(_handle, static_cast<const uint8_t *>(data.bytes), data.length), error);
    if (result) {
        // Keep the accepted desired configuration for snapshot replacement/recovery.
        // Decode our serialized input to avoid retaining caller-owned mutable data.
        NSDictionary *accepted = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
        NSMutableDictionary *options = [_hostOptions mutableCopy];
        options[@"preferences"] = accepted[@"preferences"];
        _hostOptions = [options copy];
    }
    return result;
}
- (NSDictionary *)startVoiceWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    id generation = decodeValue(msime_client_voice_start(_handle), error);
    if (!generation) return nil;
    uint64_t parsedGeneration = 0;
    if (!parseUInt64(generation, @"语音代次", &parsedGeneration, error) || parsedGeneration == 0) {
        setError(error, @"语音代次响应格式错误");
        return nil;
    }
    return @{@"generation": generation};
}
- (BOOL)cancelVoiceWithError:(NSError **)error { if (![self checkThreadAndHandle:error]) return NO; return decode(msime_client_voice_cancel(_handle), error) != nil; }
- (NSDictionary *)applyVoiceText:(NSString *)text generation:(uint64_t)generation error:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return nil;
    if (![text isKindOfClass:NSString.class]) { setError(error, @"语音文本无效"); return nil; }
    NSData *data = [text dataUsingEncoding:NSUTF8StringEncoding];
    if (!data || data.length > 65536) { setError(error, @"语音文本无效"); return nil; }
    // Snapshot before consuming the final-only token, so a view failure cannot
    // silently discard an accepted commit. Voice does not mutate Engine state.
    NSDictionary *view = [self viewWithError:error];
    if (!view) return nil;
    id value = decodeValue(msime_client_voice_apply(_handle, generation, static_cast<const uint8_t *>(data.bytes), data.length), error);
    if (!value || value == NSNull.null) return nil;
    if (![value isKindOfClass:NSString.class]) {
        setError(error, @"语音文本响应格式错误");
        return nil;
    }
    return @{@"commit": value, @"view": view};
}
- (BOOL)closeWithError:(NSError **)error {
    if (![self checkThreadAndHandle:error]) return NO;
    const uint64_t closing = _handle;
    [gNeuralSessions removeObjectForKey:@(closing)];
    NSDictionary *result = decode(msime_client_destroy(_handle), error);
    if (!result) return NO;
    _handle = 0;
    _neuralV2Enabled = NO;
    _neuralUpdateHandler = nil;
    if (gActiveSession == self) gActiveSession = nil;
    return YES;
}
- (void)reloadPreferencesDirectory:(NSString *)directory completion:(void (^)(NSDictionary *, NSError *))completion {
    NSError *error = nil;
    if (![self checkThreadAndHandle:&error]) { completion(nil, error); return; }
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath || directory.length == 0) {
        setError(&error, @"偏好目录必须是绝对路径"); completion(nil, error); return;
    }
    NSData *path = [directory dataUsingEncoding:NSUTF8StringEncoding];
    __weak MSIMEClientSession *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSError *loadError = nil;
        NSDictionary *snapshot = decode(msime_client_load_preferences(static_cast<const uint8_t *>(path.bytes), path.length), &loadError);
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEClientSession *session = weakSelf;
            NSError *updateError = loadError;
            NSDictionary *result = nil;
            if (!session) setError(&updateError, @"输入会话已释放");
            else if (snapshot) result = [session updatePreferencesSnapshot:snapshot error:&updateError];
            completion(result, updateError);
        });
    });
}
- (void)dealloc {
    if (gActiveSession == self) gActiveSession = nil;
    uint64_t handle = _handle;
    if (!handle) return;
    if ([NSThread isMainThread]) {
        msime_client_string_free(msime_client_destroy(handle));
    } else {
        // Destruction must run on the registry's owning thread; capture only the ID.
        dispatch_async(dispatch_get_main_queue(), ^{ msime_client_string_free(msime_client_destroy(handle)); });
    }
}
@end
