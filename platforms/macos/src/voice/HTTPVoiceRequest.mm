#import "HTTPVoiceRequest.h"
#import "VoiceFailureMessages.h"
#import "../core/EditionIdentity.h"
#import <os/log.h>
#include "../../../../shared/voice/VoiceProviders.h"
#include "../../../../shared/voice/PolishPrompt.h"
#include <algorithm>
#include <cmath>
#include <cstring>

namespace {
NSError *Failure(const std::string &detail = {}) {
    NSMutableDictionary *info = [@{NSLocalizedDescriptionKey: @"语音请求失败，请检查识别服务设置"} mutableCopy];
    // The shared provider layer builds the detail from the answer, never from the token or the upload; a body that is not UTF-8 is simply not shown.
    NSString *text = detail.empty() ? nil
        : [[NSString alloc] initWithBytes:detail.data() length:detail.size() encoding:NSUTF8StringEncoding];
    if (text.length) info[MSIMEVoiceFailureDetailKey] = text;
    return [NSError errorWithDomain:@"app.msime.client.voice" code:6 userInfo:info];
}
std::string String(NSDictionary *options, NSString *key) {
    NSString *value = options[key];
    return value ? std::string(value.UTF8String) : std::string();
}
BOOL Endpoint(const std::string &value) {
    NSURLComponents *url = [NSURLComponents componentsWithString:@(value.c_str())];
    BOOL loopback = [@[@"127.0.0.1", @"localhost", @"::1"] containsObject:url.host.lowercaseString];
    return url.host.length && !url.user && !url.password && !url.fragment &&
        ([url.scheme.lowercaseString isEqual:@"https"] || (loopback && [url.scheme.lowercaseString isEqual:@"http"]));
}
std::string PolishEndpoint(const std::string &value) {
    if (!Endpoint(value)) return {};
    if (!MSIMEEditionIsShurufa()) return value;
    NSURLComponents *url = [NSURLComponents componentsWithString:@(value.c_str())];
    NSString *path = url.percentEncodedPath;
    while ([path hasSuffix:@"/"]) path = [path substringToIndex:path.length - 1];
    NSString *last = path.lastPathComponent;
    const BOOL version = [last hasPrefix:@"v"] && last.length > 1 &&
        [[last substringFromIndex:1] rangeOfCharacterFromSet:
            [NSCharacterSet characterSetWithCharactersInString:@"0123456789"].invertedSet].location == NSNotFound;
    const BOOL prefix = !path.length || version || [path hasSuffix:@"/v1beta/openai"];
    // 只补明确的 API 根路径；自定义完整路由（包括尾斜杠）保持原样。
    if (prefix) path = [path stringByAppendingString:@"/chat/completions"];
    else if (![path hasSuffix:@"/chat/completions"]) return value;
    url.percentEncodedPath = path;
    return std::string(url.string.UTF8String ?: "");
}
std::string Polish(std::string text, NSDictionary *options, const std::shared_ptr<std::atomic_bool> &cancelled,
                   bool &failed, void (^polishing)(void) = nil) {
    failed = false;
    if (text.empty() || cancelled->load() ||
        !([options[@"polish_enabled"] boolValue] || [options[@"polish_text"] boolValue]) || ![options[@"polish_token"] length]) return text;
    const NSTimeInterval started = NSProcessInfo.processInfo.systemUptime;
    const char *status = "failed";
    try {
        auto provider = String(options, @"polish_provider");
        auto endpoint = String(options, @"polish_endpoint");
        auto model = String(options, @"polish_model");
        if (endpoint.empty()) endpoint = msime::voice::default_polish_endpoint(provider);
        endpoint = PolishEndpoint(endpoint);
        if (model.empty()) model = msime::voice::default_polish_model(provider);
        msime::windows::PolishPromptSlots slots;
        slots.id = String(options, @"polish_prompt_id");
        slots.custom_1 = String(options, @"polish_prompt_custom_1");
        slots.custom_2 = String(options, @"polish_prompt_custom_2");
        slots.custom_3 = String(options, @"polish_prompt_custom_3");
        auto prompt = msime::windows::polish_prompt_for(slots);
        if (!endpoint.empty()) {
            // 异步阶段通知保留取消标记的副本，不能捕获参数引用。
            if (polishing) {
                std::shared_ptr<std::atomic_bool> flag = cancelled;
                dispatch_async(dispatch_get_main_queue(), ^{ if (!flag->load()) polishing(); });
            }
            // 整理预算默认 30 秒；测试只能调小，不放宽。失败保留完整识别文字。
            long budget = 30000;
            id requested = options[@"polish_timeout_ms"];
            if ([requested isKindOfClass:NSNumber.class] && [requested longValue] >= 200) budget = std::min(budget, [requested longValue]);
            auto polished = msime::voice::polish_cloud_text(text, provider, endpoint, model,
                String(options, @"polish_token"), prompt, String(options, @"polish_thinking"), cancelled, budget);
            NSString *validText = [[NSString alloc] initWithBytes:polished.data() length:polished.size() encoding:NSUTF8StringEncoding];
            if (validText.length && polished.size() <= 65536) { text = std::move(polished); status = "ok"; }
            else failed = true;
        } else { failed = true; status = "invalid_endpoint"; }
    } catch (const std::exception &) { failed = true; }
    if (cancelled->load()) { failed = false; status = "cancelled"; }
    static os_log_t log;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ log = os_log_create(MSIMEInputMethodBundleIdentifier().UTF8String, "voice"); });
    // 只保留状态与耗时；不记录地址、模型、凭据、请求或回复正文。
    os_log(log, "voice_polish status=%{public}s elapsed_ms=%.0f", status,
           (NSProcessInfo.processInfo.systemUptime - started) * 1000.0);
    return text;
}
}
@implementation MSIMEHTTPVoiceRequest {
    NSDictionary *_options;
    std::shared_ptr<std::atomic_bool> _cancelled;
    BOOL _started;
    BOOL _recognitionRequired;
}
- (instancetype)initWithOptions:(NSDictionary *)options error:(NSError **)error {
    return [self initWithOptions:options recognitionRequired:YES error:error];
}
- (instancetype)initWithPolishOptions:(NSDictionary *)options error:(NSError **)error {
    return [self initWithOptions:options recognitionRequired:NO error:error];
}
- (instancetype)initWithOptions:(NSDictionary *)options recognitionRequired:(BOOL)recognitionRequired error:(NSError **)error {
    self = [super init];
    if (!self) return nil;
    _cancelled = std::make_shared<std::atomic_bool>(false);
    _recognitionRequired = recognitionRequired;
    if (![options isKindOfClass:NSDictionary.class]) { if (error) *error = Failure(); return nil; }
    NSMutableDictionary *snapshot = [NSMutableDictionary dictionary];
    for (NSString *key in @[@"asr_provider", @"asr_endpoint", @"asr_model", @"asr_model_path", @"asr_token", @"language",
        @"polish_provider", @"polish_endpoint", @"polish_model", @"polish_token", @"polish_thinking", @"polish_prompt_id",
        @"polish_prompt_custom_1", @"polish_prompt_custom_2", @"polish_prompt_custom_3"]) {
        id value = options[key];
        if (value && (![value isKindOfClass:NSString.class] || [value length] > 8192 ||
            ![value dataUsingEncoding:NSUTF8StringEncoding])) {
            if (error) *error = Failure(); return nil;
        }
        if (value) snapshot[key] = [value copy];
    }
    id timeout = options[@"polish_timeout_ms"];
    if ([timeout isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)timeout) != CFBooleanGetTypeID()) snapshot[@"polish_timeout_ms"] = timeout;
    for (NSString *key in @[@"polish_enabled", @"polish_text"]) {
        id value = options[key];
        if ([value isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID())
            snapshot[key] = value;
    }
    for (NSString *key in @[@"asr_token", @"polish_token"]) {
        NSString *token = snapshot[key];
        if (token && [token rangeOfCharacterFromSet:NSCharacterSet.controlCharacterSet].location != NSNotFound) {
            if (error) *error = Failure(); return nil;
        }
    }
    if (recognitionRequired) {
        const auto provider = msime::voice::normalize_voice_provider(String(snapshot, @"asr_provider"));
        const auto endpoint = msime::voice::resolved_asr_endpoint(provider, String(snapshot, @"asr_endpoint"));
        // 只接整句上传的服务（multipart 或 chat_audio），请求体由 shared/voice 按 asr_request_format 拼。豆包是流式 WebSocket，不会走到这里；其他值是过期的配置，不是用户选的服务。
        const auto format = msime::voice::asr_request_format(provider);
        if ((format != "multipart" && format != "chat_audio") ||
            !Endpoint(endpoint) || ![snapshot[@"asr_token"] length]) {
            if (error) *error = Failure(); return nil;
        }
        snapshot[@"asr_provider"] = @(provider.c_str());
        snapshot[@"asr_endpoint"] = @(endpoint.c_str());
        if (![snapshot[@"asr_model"] length]) snapshot[@"asr_model"] = @(msime::voice::default_asr_model(provider).c_str());
    }
    _options = [snapshot copy];
    return self;
}
- (NSUInteger)sampleLimit {
    // 阿里云百炼的 chat_audio 上限（约 218 秒）比 multipart 小得多，录音按它截取，不让整段在上传时被拒。
    return msime::voice::batch_capture_sample_limit_for(String(_options, @"asr_provider"));
}
- (BOOL)recognizePCM:(NSData *)pcm completion:(void (^)(NSString *, NSError *))completion error:(NSError **)error {
    @synchronized(self) {
        if (!_recognitionRequired || _started || _cancelled->load() || !completion || !pcm.length ||
            pcm.length % sizeof(float)) {
            if (error) *error = Failure(); return NO;
        }
        // Submit what was captured up to what the provider can take, as the capture buffer does.
        const std::size_t limit = self.sampleLimit;
        std::vector<float> samples(std::min<std::size_t>(pcm.length / sizeof(float), limit));
        std::memcpy(samples.data(), pcm.bytes, samples.size() * sizeof(float));
        for (float value : samples) if (!std::isfinite(value) || std::fabs(value) > 1) {
            if (error) *error = Failure(); return NO;
        }
        _started = YES;
        auto cancelled = _cancelled;
        NSDictionary *options = _options;
        void (^polishing)(void) = [self.polishingHandler copy];
        __weak MSIMEHTTPVoiceRequest *weakSelf = self;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            NSString *result = nil;
            NSError *failure = nil;
            bool polishFailed = false;
            try {
                auto language = String(options, @"language");
                if (language == "en-US" || language == "en-us") language = "en";
                if (language == "zh-CN") language = "zh-cn";
                auto text = msime::voice::recognize_cloud_asr(samples, String(options, @"asr_provider"),
                    String(options, @"asr_endpoint"), String(options, @"asr_model"), String(options, @"asr_token"), language, cancelled);
                text = Polish(std::move(text), options, cancelled, polishFailed, polishing);
                result = [[NSString alloc] initWithBytes:text.data() length:text.size() encoding:NSUTF8StringEncoding];
                if (!result.length) failure = Failure();
            } catch (const msime::voice::CloudAsrError &cloudError) { failure = Failure(cloudError.user_message()); }
            catch (const std::exception &) { failure = Failure(); }
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEHTTPVoiceRequest *request = weakSelf;
                if (!cancelled->load() && request) {
                    request->_polishFailed = polishFailed;
                    completion(failure ? nil : result, failure);
                }
            });
        });
        return YES;
    }
}
- (BOOL)polishText:(NSString *)text completion:(void (^)(NSString *, NSError *))completion error:(NSError **)error {
    @synchronized(self) {
        NSData *data = [text dataUsingEncoding:NSUTF8StringEncoding];
        if (_recognitionRequired || _started || _cancelled->load() || !completion || !data.length || data.length > 65536) {
            if (error) *error = Failure(); return NO;
        }
        _started = YES;
        auto cancelled = _cancelled;
        NSDictionary *options = _options;
        NSString *original = [text copy];
        __weak MSIMEHTTPVoiceRequest *weakSelf = self;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            bool polishFailed = false;
            auto polished = Polish(std::string(static_cast<const char *>(data.bytes), data.length), options, cancelled, polishFailed);
            NSString *result = [[NSString alloc] initWithBytes:polished.data() length:polished.size() encoding:NSUTF8StringEncoding];
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEHTTPVoiceRequest *request = weakSelf;
                if (!cancelled->load() && request) {
                    request->_polishFailed = polishFailed;
                    completion(result.length ? result : original, nil);
                }
            });
        });
        return YES;
    }
}
- (void)cancel { if (_cancelled) _cancelled->store(true); }
- (void)dealloc { [self cancel]; }
@end
