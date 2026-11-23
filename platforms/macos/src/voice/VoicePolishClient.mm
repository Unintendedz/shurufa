#import "VoicePolishClient.h"

#import <dlfcn.h>
#import <os/log.h>

#include <atomic>
#include <cstring>
#include <memory>

#if defined(MSIME_VOICE_POLISH_USE_RUST_FFI) && MSIME_VOICE_POLISH_USE_RUST_FFI
#include "msime_client.h"
#endif

namespace {

constexpr NSUInteger kMaxRequestJSONBytes = 256 * 1024;
constexpr NSUInteger kMaxSSEPayloadBytes = 64 * 1024;
constexpr NSUInteger kDefaultMaxResponseBytes = 1024 * 1024;
constexpr NSUInteger kHardMaxResponseBytes = 4 * 1024 * 1024;
constexpr uint64_t kDefaultTimeoutMilliseconds = 30000;
constexpr uint64_t kMinTimeoutMilliseconds = 100;
constexpr uint64_t kMaxTimeoutMilliseconds = 120000;
constexpr NSUInteger kMaxVocabularyHints = 60;

os_log_t PolishClientLog(void) {
    static os_log_t log = nullptr;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        log = os_log_create("dev.shurufa.ime", "voice-polish-client");
    });
    return log;
}

uint64_t MonotonicNowNanoseconds(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

uint64_t ElapsedMillisecondsSince(uint64_t startNanoseconds) {
    const uint64_t now = MonotonicNowNanoseconds();
    if (now <= startNanoseconds) return 0;
    return (now - startNanoseconds) / 1000000ULL;
}

NSString *Trimmed(NSString * _Nullable value) {
    if (!value) return @"";
    return [value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
}

struct ScopedBridgeString {
    char *pointer = nullptr;
    MSIMEVoicePolishFreeStringFn freeFn = nullptr;

    ScopedBridgeString(char *ptr, MSIMEVoicePolishFreeStringFn fn) : pointer(ptr), freeFn(fn) {}
    ~ScopedBridgeString() {
        if (pointer && freeFn) {
            freeFn(pointer);
        }
    }
    ScopedBridgeString(const ScopedBridgeString &) = delete;
    ScopedBridgeString &operator=(const ScopedBridgeString &) = delete;
};

NSDictionary * _Nullable ParseJSONEnvelope(const char *rawString) {
    if (!rawString || !*rawString) return nil;
    NSData *data = [NSData dataWithBytesNoCopy:const_cast<char *>(rawString)
                                        length:std::strlen(rawString)
                                  freeWhenDone:NO];
    id parsed = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    return [parsed isKindOfClass:NSDictionary.class] ? static_cast<NSDictionary *>(parsed) : nil;
}

NSString *ExtractEnvelopeErrorCode(NSDictionary * _Nullable envelope, NSString *fallbackCode) {
    if (![envelope isKindOfClass:NSDictionary.class]) return fallbackCode;
    id errorField = envelope[@"error"];
    if ([errorField isKindOfClass:NSString.class]) {
        NSString *trimmed = Trimmed(errorField);
        if (trimmed.length) return trimmed;
    } else if ([errorField isKindOfClass:NSDictionary.class]) {
        id code = errorField[@"code"];
        if ([code isKindOfClass:NSString.class] && [Trimmed(code) length]) {
            return Trimmed(code);
        }
        id message = errorField[@"message"];
        if ([message isKindOfClass:NSString.class] && [Trimmed(message) length]) {
            return Trimmed(message);
        }
    }
    return fallbackCode;
}

uint64_t ReadUInt64Field(NSDictionary *dict, NSString *key, uint64_t fallbackValue) {
    id value = dict[key];
    if ([value isKindOfClass:NSNumber.class] &&
        CFGetTypeID((__bridge CFTypeRef)value) != CFBooleanGetTypeID()) {
        NSNumber *number = static_cast<NSNumber *>(value);
        const long long signedVal = number.longLongValue;
        if (signedVal > 0) return static_cast<uint64_t>(signedVal);
    }
    return fallbackValue;
}

BOOL ReadBoolField(NSDictionary *dict, NSString *key, BOOL fallbackValue) {
    id value = dict[key];
    if ([value isKindOfClass:NSNumber.class]) {
        NSNumber *number = static_cast<NSNumber *>(value);
        return number.boolValue;
    }
    return fallbackValue;
}

NSString *ReadStringField(NSDictionary *dict, NSString *key, NSString *fallbackValue) {
    id value = dict[key];
    if ([value isKindOfClass:NSString.class]) {
        NSString *trimmed = Trimmed(value);
        if (trimmed.length) return trimmed;
    }
    return fallbackValue;
}

BOOL IsLoopbackConnectionRetryableError(NSError * _Nullable error) {
    if (!error || ![error.domain isEqualToString:NSURLErrorDomain]) return NO;
    switch (error.code) {
        case NSURLErrorCannotConnectToHost:
        case NSURLErrorNetworkConnectionLost:
        case NSURLErrorCannotParseResponse:
        case NSURLErrorCannotFindHost:
        case NSURLErrorNotConnectedToInternet:
            return YES;
        default:
            return NO;
    }
}

BOOL IsBridgeComplete(const MSIMEVoicePolishBridge &bridge) {
    return bridge.buildHTTPRequest != nullptr &&
           bridge.parseResponse != nullptr &&
           bridge.cleanOutput != nullptr &&
           bridge.freeString != nullptr;
}

} // namespace

MSIMEVoicePolishBridge MSIMEVoicePolishDefaultBridge(void) {
    MSIMEVoicePolishBridge bridge = {};
#if defined(MSIME_VOICE_POLISH_USE_RUST_FFI) && MSIME_VOICE_POLISH_USE_RUST_FFI
    bridge.buildHTTPRequest = msime_client_voice_polish_http_request;
    bridge.parseResponse = msime_client_voice_polish_parse_response;
    bridge.cleanOutput = msime_client_voice_polish_clean;
    bridge.freeString = msime_client_string_free;
#else
    // Wave 1 未更新 msime_client.h 时通过 RTLD_DEFAULT 弱解析，确保无额外链接标志即可独立编译测试。
    bridge.buildHTTPRequest = reinterpret_cast<MSIMEVoicePolishHTTPRequestFn>(
        dlsym(RTLD_DEFAULT, "msime_client_voice_polish_http_request"));
    bridge.parseResponse = reinterpret_cast<MSIMEVoicePolishParseResponseFn>(
        dlsym(RTLD_DEFAULT, "msime_client_voice_polish_parse_response"));
    bridge.cleanOutput = reinterpret_cast<MSIMEVoicePolishCleanFn>(
        dlsym(RTLD_DEFAULT, "msime_client_voice_polish_clean"));
    bridge.freeString = reinterpret_cast<MSIMEVoicePolishFreeStringFn>(
        dlsym(RTLD_DEFAULT, "msime_client_string_free"));
#endif
    return bridge;
}

@implementation MSIMEVoicePolishContext

- (id)copyWithZone:(nullable NSZone *)zone {
    MSIMEVoicePolishContext *copy = [[[self class] allocWithZone:zone] init];
    copy.appBundleID = self.appBundleID;
    copy.appName = self.appName;
    copy.windowTitle = self.windowTitle;
    copy.webDomain = self.webDomain;
    copy.precedingText = self.precedingText;
    copy.followingText = self.followingText;
    copy.selectedText = self.selectedText;
    copy.visibleScreenText = self.visibleScreenText;
    copy.isSecureField = self.isSecureField;
    copy.axBlacklisted = self.axBlacklisted;
    return copy;
}

- (NSDictionary<NSString *, id> *)toJSONDictionary {
    NSMutableDictionary<NSString *, id> *dict = [NSMutableDictionary dictionary];
    if (self.appBundleID.length) dict[@"app_bundle_id"] = self.appBundleID;
    if (self.appName.length) dict[@"app_name"] = self.appName;
    if (self.windowTitle.length) dict[@"window_title"] = self.windowTitle;
    if (self.webDomain.length) dict[@"web_domain"] = self.webDomain;
    if (self.precedingText.length) dict[@"preceding_text"] = self.precedingText;
    if (self.followingText.length) dict[@"following_text"] = self.followingText;
    if (self.selectedText.length) dict[@"selected_text"] = self.selectedText;
    if (self.visibleScreenText.length) dict[@"visible_screen_text"] = self.visibleScreenText;
    dict[@"is_secure_field"] = @(self.isSecureField);
    dict[@"ax_blacklisted"] = @(self.axBlacklisted);
    return dict;
}

- (NSString *)description {
    return [NSString stringWithFormat:@"<MSIMEVoicePolishContext bundle=%@ secure=%d axBlacklisted=%d>",
                                      self.appBundleID ?: @"", self.isSecureField ? 1 : 0, self.axBlacklisted ? 1 : 0];
}

@end

@implementation MSIMEVoicePolishRequest

- (instancetype)init {
    if (!(self = [super init])) return nil;
    _provider = @"";
    _promptID = @"casual";
    _mode = @"auto";
    _enableSelectionRewrite = NO;
    _stream = NO;
    _enablePanguSpacing = YES;
    _rawText = @"";
    return self;
}

- (id)copyWithZone:(nullable NSZone *)zone {
    MSIMEVoicePolishRequest *copy = [[[self class] allocWithZone:zone] init];
    copy.provider = self.provider;
    copy.endpoint = self.endpoint;
    copy.model = self.model;
    copy.token = self.token;
    copy.localModelPath = self.localModelPath;
    copy.promptID = self.promptID;
    copy.customPrompts = self.customPrompts;
    copy.mode = self.mode;
    copy.enableSelectionRewrite = self.enableSelectionRewrite;
    copy.targetLanguage = self.targetLanguage;
    copy.stream = self.stream;
    copy.enablePanguSpacing = self.enablePanguSpacing;
    copy.rawText = self.rawText;
    copy.fallbackText = self.fallbackText;
    copy.context = self.context;
    copy.vocabularyHints = self.vocabularyHints;
    return copy;
}

- (NSString *)description {
    return [NSString stringWithFormat:@"<MSIMEVoicePolishRequest provider=%@ model=%@ mode=%@ stream=%d token=<redacted>>",
                                      self.provider ?: @"", self.model ?: @"", self.mode ?: @"", self.stream ? 1 : 0];
}

@end

@interface MSIMEVoicePolishResult ()
- (instancetype)initWithText:(NSString *)text
                polishFailed:(BOOL)polishFailed
        polishRejectedReason:(nullable NSString *)polishRejectedReason
          polishMilliseconds:(uint64_t)polishMilliseconds
                resolvedMode:(NSString *)resolvedMode
              resolvedAction:(NSString *)resolvedAction
                 appCategory:(NSString *)appCategory
               usedStreaming:(BOOL)usedStreaming;
@end

@implementation MSIMEVoicePolishResult

- (instancetype)initWithText:(NSString *)text
                polishFailed:(BOOL)polishFailed
        polishRejectedReason:(nullable NSString *)polishRejectedReason
          polishMilliseconds:(uint64_t)polishMilliseconds
                resolvedMode:(NSString *)resolvedMode
              resolvedAction:(NSString *)resolvedAction
                 appCategory:(NSString *)appCategory
               usedStreaming:(BOOL)usedStreaming {
    if (!(self = [super init])) return nil;
    _text = [text copy] ?: @"";
    _polishFailed = polishFailed;
    _polishRejectedReason = [polishRejectedReason copy];
    _polishMilliseconds = polishMilliseconds;
    _resolvedMode = [resolvedMode copy] ?: @"dictation";
    _resolvedAction = [resolvedAction copy] ?: @"insert";
    _appCategory = [appCategory copy] ?: @"general";
    _usedStreaming = usedStreaming;
    return self;
}

- (NSDictionary<NSString *, id> *)archiveMetadata {
    NSMutableDictionary<NSString *, id> *meta = [@{
        @"polish_failed" : @(_polishFailed),
        @"polish_ms" : @(_polishMilliseconds),
        @"mode" : _resolvedMode,
        @"app_category" : _appCategory,
    } mutableCopy];
    if (_polishRejectedReason.length) {
        meta[@"polish_rejected_reason"] = _polishRejectedReason;
    }
    return meta;
}

@end

// 描述解析后的 HTTP 传输参数（来自 Rust C.4 描述器）。
@interface MSIMEVoicePolishDescriptor : NSObject
@property(nonatomic, strong) NSURL *url;
@property(nonatomic, copy) NSString *method;
@property(nonatomic, copy) NSArray<NSDictionary<NSString *, NSString *> *> *headers;
@property(nonatomic, strong) NSData *bodyData;
@property(nonatomic, assign) uint64_t timeoutMs;
@property(nonatomic, assign) uint64_t connectTimeoutMs;
@property(nonatomic, assign) NSUInteger maxResponseBytes;
@property(nonatomic, assign) BOOL noProxy;
@property(nonatomic, copy) NSString *resolvedMode;
@property(nonatomic, assign) BOOL expectActionTag;
@property(nonatomic, copy) NSString *appCategory;
@property(nonatomic, assign) BOOL stream;
@end

@implementation MSIMEVoicePolishDescriptor
@end

// 单次 HTTP/SSE 传输状态与 NSURLSession 委托（隔离于客户端实例，避免强引用环）。
@interface MSIMEVoicePolishSessionAttempt : NSObject <NSURLSessionDataDelegate>
- (instancetype)initWithDescriptor:(MSIMEVoicePolishDescriptor *)descriptor
                            bridge:(MSIMEVoicePolishBridge)bridge
                      isCancelledFn:(BOOL (^)(void))isCancelledFn
                         onPartial:(nullable void (^)(NSString *accumulated))onPartial
                        completion:(void (^)(NSString * _Nullable polishedText,
                                             NSString * _Nullable errorReason,
                                             NSError * _Nullable urlError,
                                             BOOL usedStreaming))completion;
- (void)start;
- (void)cancel;
@end

@implementation MSIMEVoicePolishSessionAttempt {
    MSIMEVoicePolishDescriptor *_descriptor;
    MSIMEVoicePolishBridge _bridge;
    BOOL (^_isCancelledFn)(void);
    void (^_onPartial)(NSString *);
    void (^_completion)(NSString * _Nullable, NSString * _Nullable, NSError * _Nullable, BOOL);

    dispatch_queue_t _stateQueue;
    NSOperationQueue *_delegateQueue;
    NSURLSession *_session;
    NSURLSessionDataTask *_dataTask;
    dispatch_source_t _timeoutTimer;

    BOOL _finished;
    BOOL _cancelled;
    BOOL _redirectBlocked;
    BOOL _timedOut;
    BOOL _oversized;
    BOOL _streamingActive;
    BOOL _sseDone;
    BOOL _sseParseFailed;
    NSInteger _statusCode;
    NSUInteger _totalReceivedBytes;
    NSMutableData *_responseBody;
    NSMutableData *_sseLineBuffer;
    NSMutableString *_accumulatedText;
}

- (instancetype)initWithDescriptor:(MSIMEVoicePolishDescriptor *)descriptor
                            bridge:(MSIMEVoicePolishBridge)bridge
                      isCancelledFn:(BOOL (^)(void))isCancelledFn
                         onPartial:(nullable void (^)(NSString *))onPartial
                        completion:(void (^)(NSString * _Nullable, NSString * _Nullable, NSError * _Nullable, BOOL))completion {
    if (!(self = [super init])) return nil;
    _descriptor = descriptor;
    _bridge = bridge;
    _isCancelledFn = [isCancelledFn copy];
    _onPartial = [onPartial copy];
    _completion = [completion copy];
    _stateQueue = dispatch_queue_create("dev.shurufa.voice.polish-attempt", DISPATCH_QUEUE_SERIAL);
    _delegateQueue = [NSOperationQueue new];
    _delegateQueue.maxConcurrentOperationCount = 1;
    _delegateQueue.underlyingQueue = _stateQueue;
    _responseBody = [NSMutableData data];
    _sseLineBuffer = [NSMutableData data];
    _accumulatedText = [NSMutableString string];
    _streamingActive = descriptor.stream;
    return self;
}

- (void)dealloc {
    if (_timeoutTimer) {
        dispatch_source_cancel(_timeoutTimer);
        _timeoutTimer = nil;
    }
}

- (void)start {
    dispatch_async(_stateQueue, ^{
        if (self->_finished || self->_cancelled || (self->_isCancelledFn && self->_isCancelledFn())) {
            [self finishLockedWithText:nil reason:nil urlError:nil];
            return;
        }

        NSURLSessionConfiguration *config = [NSURLSessionConfiguration ephemeralSessionConfiguration];
        config.URLCache = nil;
        config.HTTPCookieStorage = nil;
        config.URLCredentialStorage = nil;
        config.HTTPShouldSetCookies = NO;
        // 非流式本地推理可能需要数秒生成完整回复，用总超时配置 NSURLSession，避免把 connect_timeout 误当首字节超时。
        const NSTimeInterval totalTimeoutSeconds = MAX(0.1, static_cast<double>(self->_descriptor.timeoutMs) / 1000.0);
        config.timeoutIntervalForRequest = totalTimeoutSeconds;
        config.timeoutIntervalForResource = totalTimeoutSeconds;
        if (self->_descriptor.noProxy) {
            config.connectionProxyDictionary = @{};
        }

        NSMutableURLRequest *urlRequest = [NSMutableURLRequest requestWithURL:self->_descriptor.url];
        urlRequest.HTTPMethod = self->_descriptor.method.length ? self->_descriptor.method : @"POST";
        urlRequest.HTTPBody = self->_descriptor.bodyData;
        urlRequest.cachePolicy = NSURLRequestReloadIgnoringLocalAndRemoteCacheData;
        urlRequest.timeoutInterval = totalTimeoutSeconds;
        for (NSDictionary<NSString *, NSString *> *header in self->_descriptor.headers) {
            NSString *name = header[@"name"];
            NSString *value = header[@"value"];
            if (name.length && value != nil) {
                [urlRequest setValue:value forHTTPHeaderField:name];
            }
        }

        self->_session = [NSURLSession sessionWithConfiguration:config delegate:self delegateQueue:self->_delegateQueue];
        self->_dataTask = [self->_session dataTaskWithRequest:urlRequest];

        // 显式墙钟超时守卫，防止连接或流式读取无限悬挂。
        dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, self->_stateQueue);
        if (timer) {
            const uint64_t timeoutNsec = self->_descriptor.timeoutMs * 1000000ULL;
            dispatch_source_set_timer(timer,
                                      dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(timeoutNsec)),
                                      DISPATCH_TIME_FOREVER,
                                      20 * 1000000ULL);
            __weak MSIMEVoicePolishSessionAttempt *weakSelf = self;
            dispatch_source_set_event_handler(timer, ^{
                MSIMEVoicePolishSessionAttempt *strongSelf = weakSelf;
                if (!strongSelf || strongSelf->_finished) return;
                strongSelf->_timedOut = YES;
                [strongSelf->_dataTask cancel];
            });
            self->_timeoutTimer = timer;
            dispatch_resume(timer);
        }

        [self->_dataTask resume];
    });
}

- (void)cancel {
    dispatch_async(_stateQueue, ^{
        self->_cancelled = YES;
        if (self->_timeoutTimer) {
            dispatch_source_cancel(self->_timeoutTimer);
            self->_timeoutTimer = nil;
        }
        [self->_dataTask cancel];
        [self->_session invalidateAndCancel];
        self->_session = nil;
        self->_dataTask = nil;
        [self finishLockedWithText:nil reason:nil urlError:nil];
    });
}

- (void)processSSELineLocked:(NSData *)lineData {
    if (!lineData.length || _sseDone) return;
    NSUInteger length = lineData.length;
    const char *bytes = static_cast<const char *>(lineData.bytes);
    while (length > 0 && (bytes[length - 1] == '\r' || bytes[length - 1] == '\n')) {
        --length;
    }
    if (length == 0 || bytes[0] == ':') return;

    static constexpr char kDataPrefix[] = "data:";
    static constexpr size_t kDataPrefixLen = 5;
    if (length < kDataPrefixLen || std::memcmp(bytes, kDataPrefix, kDataPrefixLen) != 0) {
        return;
    }

    const char *payloadPtr = bytes + kDataPrefixLen;
    NSUInteger payloadLen = length - kDataPrefixLen;
    while (payloadLen > 0 && (*payloadPtr == ' ' || *payloadPtr == '\t')) {
        ++payloadPtr;
        --payloadLen;
    }
    while (payloadLen > 0 && (payloadPtr[payloadLen - 1] == ' ' || payloadPtr[payloadLen - 1] == '\t')) {
        --payloadLen;
    }
    if (payloadLen == 0) return;

    if (payloadLen == 6 && std::memcmp(payloadPtr, "[DONE]", 6) == 0) {
        _sseDone = YES;
        return;
    }
    if (payloadLen > kMaxSSEPayloadBytes) {
        _sseParseFailed = YES;
        return;
    }

    ScopedBridgeString rawResponse(
        _bridge.parseResponse(reinterpret_cast<const uint8_t *>(payloadPtr), payloadLen, true),
        _bridge.freeString);
    NSDictionary *envelope = ParseJSONEnvelope(rawResponse.pointer);
    if (!envelope || ![envelope[@"ok"] isEqual:@YES] || ![envelope[@"value"] isKindOfClass:NSDictionary.class]) {
        _sseParseFailed = YES;
        return;
    }

    NSDictionary *value = envelope[@"value"];
    id delta = value[@"text"];
    if ([delta isKindOfClass:NSString.class] && [static_cast<NSString *>(delta) length] > 0) {
        [_accumulatedText appendString:static_cast<NSString *>(delta)];
        if (_onPartial && !_cancelled && (!_isCancelledFn || !_isCancelledFn())) {
            NSString *snapshot = [_accumulatedText copy];
            _onPartial(snapshot);
        }
    }
    if (ReadBoolField(value, @"done", NO)) {
        _sseDone = YES;
    }
}

- (void)drainSSEBufferLockedFlushRemainder:(BOOL)flushRemainder {
    for (;;) {
        if (!_sseLineBuffer.length) break;
        const char *bytes = static_cast<const char *>(_sseLineBuffer.bytes);
        const void *newline = std::memchr(bytes, '\n', _sseLineBuffer.length);
        if (!newline) {
            if (flushRemainder) {
                NSData *tail = [_sseLineBuffer copy];
                _sseLineBuffer.length = 0;
                [self processSSELineLocked:tail];
            }
            break;
        }
        const NSUInteger lineLen = static_cast<NSUInteger>(static_cast<const char *>(newline) - bytes);
        NSData *lineData = [_sseLineBuffer subdataWithRange:NSMakeRange(0, lineLen)];
        [_sseLineBuffer replaceBytesInRange:NSMakeRange(0, lineLen + 1) withBytes:nullptr length:0];
        [self processSSELineLocked:lineData];
    }
}

- (void)finishLockedWithText:(nullable NSString *)text
                      reason:(nullable NSString *)reason
                    urlError:(nullable NSError *)urlError {
    if (_finished) return;
    _finished = YES;
    if (_timeoutTimer) {
        dispatch_source_cancel(_timeoutTimer);
        _timeoutTimer = nil;
    }
    [_session invalidateAndCancel];
    _session = nil;
    _dataTask = nil;

    void (^completionCopy)(NSString * _Nullable, NSString * _Nullable, NSError * _Nullable, BOOL) = _completion;
    const BOOL usedStreaming = _streamingActive;
    _completion = nil;
    _onPartial = nil;
    if (completionCopy && !_cancelled && (!_isCancelledFn || !_isCancelledFn())) {
        completionCopy(text, reason, urlError, usedStreaming);
    }
}

#pragma mark - NSURLSessionDataDelegate

- (void)URLSession:(NSURLSession *)session
                          task:(NSURLSessionTask *)task
    willPerformHTTPRedirection:(NSHTTPURLResponse *)response
                    newRequest:(NSURLRequest *)request
             completionHandler:(void (^)(NSURLRequest * _Nullable))completionHandler {
    (void)session;
    (void)response;
    (void)request;
    // 严格禁止跟随任何 HTTP 重定向，防止认证头与用户文本泄露给第三方目标。
    _redirectBlocked = YES;
    completionHandler(nil);
    [task cancel];
}

- (void)URLSession:(NSURLSession *)session
          dataTask:(NSURLSessionDataTask *)dataTask
didReceiveResponse:(NSURLResponse *)response
 completionHandler:(void (^)(NSURLSessionResponseDisposition))completionHandler {
    (void)session;
    (void)dataTask;
    if (_cancelled || _redirectBlocked || _timedOut) {
        completionHandler(NSURLSessionResponseCancel);
        return;
    }
    if ([response isKindOfClass:NSHTTPURLResponse.class]) {
        NSHTTPURLResponse *http = static_cast<NSHTTPURLResponse *>(response);
        _statusCode = http.statusCode;
        if (_statusCode >= 300 && _statusCode < 400) {
            _redirectBlocked = YES;
            completionHandler(NSURLSessionResponseCancel);
            return;
        }
        if (_descriptor.stream) {
            NSString *contentType = [http valueForHTTPHeaderField:@"Content-Type"].lowercaseString ?: @"";
            if ([contentType containsString:@"application/json"]) {
                // 服务端未开启 SSE 而直接返回 JSON 时，自动兼容非流式解析。
                _streamingActive = NO;
            }
        }
    }
    if (response.expectedContentLength > 0 &&
        static_cast<uint64_t>(response.expectedContentLength) > _descriptor.maxResponseBytes) {
        _oversized = YES;
        completionHandler(NSURLSessionResponseCancel);
        return;
    }
    completionHandler(NSURLSessionResponseAllow);
}

- (void)URLSession:(NSURLSession *)session
          dataTask:(NSURLSessionDataTask *)dataTask
    didReceiveData:(NSData *)data {
    (void)session;
    if (_finished || _cancelled || _redirectBlocked || _timedOut || _oversized) return;
    if (!data.length) return;

    if (_totalReceivedBytes > _descriptor.maxResponseBytes ||
        data.length > (_descriptor.maxResponseBytes - _totalReceivedBytes)) {
        _oversized = YES;
        [dataTask cancel];
        return;
    }
    _totalReceivedBytes += data.length;

    if (_statusCode < 200 || _statusCode >= 300) {
        return;
    }

    if (_streamingActive) {
        [_sseLineBuffer appendData:data];
        [self drainSSEBufferLockedFlushRemainder:NO];
    } else {
        [_responseBody appendData:data];
    }
}

- (void)URLSession:(NSURLSession *)session
                    task:(NSURLSessionTask *)task
    didCompleteWithError:(nullable NSError *)error {
    (void)session;
    (void)task;
    if (_finished) return;

    if (_redirectBlocked) {
        [self finishLockedWithText:nil reason:@"redirect_blocked" urlError:nil];
        return;
    }
    if (_timedOut) {
        [self finishLockedWithText:nil reason:@"timeout" urlError:error];
        return;
    }
    if (_oversized) {
        [self finishLockedWithText:nil reason:@"response_too_large" urlError:nil];
        return;
    }
    if (_cancelled || (_isCancelledFn && _isCancelledFn())) {
        [self finishLockedWithText:nil reason:nil urlError:nil];
        return;
    }
    if (error != nil) {
        if ([error.domain isEqualToString:NSURLErrorDomain] && error.code == NSURLErrorTimedOut) {
            [self finishLockedWithText:nil reason:@"timeout" urlError:error];
        } else {
            [self finishLockedWithText:nil reason:@"network_error" urlError:error];
        }
        return;
    }
    if (_statusCode < 200 || _statusCode >= 300) {
        NSString *code = [NSString stringWithFormat:@"http_%ld", static_cast<long>(_statusCode)];
        [self finishLockedWithText:nil reason:code urlError:nil];
        return;
    }

    if (_streamingActive) {
        [self drainSSEBufferLockedFlushRemainder:YES];
        if (_accumulatedText.length == 0) {
            [self finishLockedWithText:nil
                                reason:(_sseParseFailed ? @"parse_error" : @"empty_response")
                              urlError:nil];
            return;
        }
        [self finishLockedWithText:[_accumulatedText copy] reason:nil urlError:nil];
        return;
    }

    if (_responseBody.length == 0) {
        [self finishLockedWithText:nil reason:@"empty_response" urlError:nil];
        return;
    }

    ScopedBridgeString rawParsed(
        _bridge.parseResponse(static_cast<const uint8_t *>(_responseBody.bytes), _responseBody.length, false),
        _bridge.freeString);
    NSDictionary *envelope = ParseJSONEnvelope(rawParsed.pointer);
    if (!envelope || ![envelope[@"ok"] isEqual:@YES] || ![envelope[@"value"] isKindOfClass:NSDictionary.class]) {
        [self finishLockedWithText:nil
                            reason:ExtractEnvelopeErrorCode(envelope, @"parse_error")
                          urlError:nil];
        return;
    }

    NSDictionary *value = envelope[@"value"];
    id textField = value[@"text"];
    if (![textField isKindOfClass:NSString.class] || ![static_cast<NSString *>(textField) length]) {
        [self finishLockedWithText:nil reason:@"empty_response" urlError:nil];
        return;
    }
    [self finishLockedWithText:static_cast<NSString *>(textField) reason:nil urlError:nil];
}

@end

@interface MSIMEVoicePolishTask ()
@property(nonatomic, readonly, getter=isCompleted) BOOL completed;
- (void)setActiveAttempt:(nullable MSIMEVoicePolishSessionAttempt *)attempt;
- (void)installAndResumeSlowHintTimer:(dispatch_source_t)timer;
- (BOOL)claimCompletion;
@end

@implementation MSIMEVoicePolishTask {
    std::atomic<bool> _cancelledFlag;
    std::atomic<bool> _completedFlag;
    dispatch_queue_t _lockQueue;
    MSIMEVoicePolishSessionAttempt *_activeAttempt;
    dispatch_source_t _slowHintTimer;
}

- (instancetype)init {
    if (!(self = [super init])) return nil;
    _cancelledFlag.store(false);
    _completedFlag.store(false);
    _lockQueue = dispatch_queue_create("dev.shurufa.voice.polish-task", DISPATCH_QUEUE_SERIAL);
    return self;
}

- (void)dealloc {
    if (_slowHintTimer) {
        dispatch_source_cancel(_slowHintTimer);
        _slowHintTimer = nil;
    }
}

- (BOOL)isCancelled {
    return _cancelledFlag.load() ? YES : NO;
}

- (BOOL)isCompleted {
    return _completedFlag.load() ? YES : NO;
}

- (void)setActiveAttempt:(nullable MSIMEVoicePolishSessionAttempt *)attempt {
    dispatch_sync(_lockQueue, ^{
        self->_activeAttempt = attempt;
        if (self->_cancelledFlag.load()) {
            [attempt cancel];
        }
    });
}

- (void)installAndResumeSlowHintTimer:(dispatch_source_t)timer {
    dispatch_sync(_lockQueue, ^{
        if (self->_slowHintTimer) {
            dispatch_source_cancel(self->_slowHintTimer);
            self->_slowHintTimer = nil;
        }
        if (self->_cancelledFlag.load() || self->_completedFlag.load()) {
            dispatch_source_cancel(timer);
        } else {
            self->_slowHintTimer = timer;
        }
    });
    // 即使已取消也须 resume 一次，防止释放挂起态 dispatch_source 触发崩溃。
    dispatch_resume(timer);
}

- (BOOL)claimCompletion {
    bool expected = false;
    if (!_completedFlag.compare_exchange_strong(expected, true)) {
        return NO;
    }
    dispatch_sync(_lockQueue, ^{
        if (self->_slowHintTimer) {
            dispatch_source_cancel(self->_slowHintTimer);
            self->_slowHintTimer = nil;
        }
        self->_activeAttempt = nil;
    });
    return !_cancelledFlag.load();
}

- (void)cancel {
    _cancelledFlag.store(true);
    dispatch_sync(_lockQueue, ^{
        if (self->_slowHintTimer) {
            dispatch_source_cancel(self->_slowHintTimer);
            self->_slowHintTimer = nil;
        }
        [self->_activeAttempt cancel];
        self->_activeAttempt = nil;
    });
}

@end

@implementation MSIMEVoicePolishClient {
    MSIMEVoicePolishBridge _bridge;
    MSIMELocalPolishServer *_localServer;
    dispatch_queue_t _workQueue;
}

- (instancetype)init {
    return [self initWithBridge:MSIMEVoicePolishDefaultBridge()
                    localServer:MSIMELocalPolishServer.sharedServer];
}

- (instancetype)initWithBridge:(MSIMEVoicePolishBridge)bridge
                   localServer:(nullable MSIMELocalPolishServer *)localServer {
    if (!(self = [super init])) return nil;
    _bridge = bridge;
    _localServer = localServer;
    _slowHintDelay = 7.5;
    _slowHintMessage = @"正在润色，请稍候…";
    _callbackQueue = dispatch_get_main_queue();
    _workQueue = dispatch_queue_create("dev.shurufa.voice.polish-client",
                                       dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_CONCURRENT, QOS_CLASS_USER_INITIATED, 0));
    return self;
}

- (dispatch_queue_t)effectiveCallbackQueue {
    return _callbackQueue ?: dispatch_get_main_queue();
}

- (void)deliverResult:(MSIMEVoicePolishResult *)result
              forTask:(MSIMEVoicePolishTask *)task
           completion:(void (^)(MSIMEVoicePolishResult *))completion {
    if (![task claimCompletion]) return;
    dispatch_queue_t targetQueue = [self effectiveCallbackQueue];
    dispatch_async(targetQueue, ^{
        if (task.cancelled) return;
        completion(result);
    });
}

- (void)degradeTask:(MSIMEVoicePolishTask *)task
            request:(MSIMEVoicePolishRequest *)request
          startedAt:(uint64_t)startedAt
             reason:(NSString *)reason
       resolvedMode:(NSString *)resolvedMode
        appCategory:(NSString *)appCategory
      usedStreaming:(BOOL)usedStreaming
         completion:(void (^)(MSIMEVoicePolishResult *))completion {
    NSString *fallback = request.fallbackText != nil ? request.fallbackText : (request.rawText ?: @"");
    const uint64_t elapsedMs = ElapsedMillisecondsSince(startedAt);
    os_log_info(PolishClientLog(), "语音润色自动降级（provider=%{public}@ mode=%{public}@ reason=%{public}@ ms=%llu）",
                request.provider ?: @"", resolvedMode ?: @"dictation", reason ?: @"unknown",
                static_cast<unsigned long long>(elapsedMs));
    MSIMEVoicePolishResult *result = [[MSIMEVoicePolishResult alloc]
        initWithText:fallback
        polishFailed:YES
polishRejectedReason:reason
  polishMilliseconds:elapsedMs
        resolvedMode:resolvedMode
      resolvedAction:@"insert"
         appCategory:appCategory
       usedStreaming:usedStreaming];
    [self deliverResult:result forTask:task completion:completion];
}

- (nullable MSIMEVoicePolishDescriptor *)buildDescriptorForRequest:(MSIMEVoicePolishRequest *)request
                                                          endpoint:(NSString *)endpoint
                                                             model:(NSString *)model
                                                             token:(nullable NSString *)token
                                                         errorCode:(NSString * _Nullable * _Nonnull)errorCode {
    if (!IsBridgeComplete(_bridge)) {
        *errorCode = @"bridge_unavailable";
        return nil;
    }

    NSMutableDictionary<NSString *, id> *payload = [NSMutableDictionary dictionary];
    payload[@"provider"] = Trimmed(request.provider);
    // 保留 endpoint 原文交由 Rust `ai::endpoint::validate` 校验，避免提前裁剪吞掉 \n/\r/\t 等控制字符。
    payload[@"endpoint"] = endpoint ?: @"";
    payload[@"model"] = Trimmed(model);
    if (token.length) {
        payload[@"token"] = token;
    }
    payload[@"prompt_id"] = Trimmed(request.promptID).length ? Trimmed(request.promptID) : @"casual";
    if (request.customPrompts.count) {
        payload[@"custom_prompts"] = request.customPrompts;
    }
    payload[@"mode"] = Trimmed(request.mode).length ? Trimmed(request.mode) : @"auto";
    payload[@"enable_selection_rewrite"] = @(request.enableSelectionRewrite);
    if (Trimmed(request.targetLanguage).length) {
        payload[@"target_language"] = Trimmed(request.targetLanguage);
    }
    payload[@"stream"] = @(request.stream);
    payload[@"raw_text"] = request.rawText ?: @"";
    if (request.context) {
        payload[@"context"] = [request.context toJSONDictionary];
    }
    if (request.vocabularyHints.count) {
        NSMutableArray<NSString *> *cappedHints = [NSMutableArray array];
        for (NSString *hint in request.vocabularyHints) {
            NSString *trimmedHint = Trimmed(hint);
            if (!trimmedHint.length) continue;
            [cappedHints addObject:trimmedHint];
            if (cappedHints.count >= kMaxVocabularyHints) break;
        }
        if (cappedHints.count) {
            payload[@"vocabulary_hints"] = cappedHints;
        }
    }

    NSData *requestJSON = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    if (!requestJSON) {
        *errorCode = @"invalid_request";
        return nil;
    }
    if (requestJSON.length > kMaxRequestJSONBytes) {
        *errorCode = @"request_too_large";
        return nil;
    }

    ScopedBridgeString rawDescriptor(
        _bridge.buildHTTPRequest(static_cast<const uint8_t *>(requestJSON.bytes), requestJSON.length),
        _bridge.freeString);
    NSDictionary *envelope = ParseJSONEnvelope(rawDescriptor.pointer);
    if (!envelope || ![envelope[@"ok"] isEqual:@YES] || ![envelope[@"value"] isKindOfClass:NSDictionary.class]) {
        *errorCode = ExtractEnvelopeErrorCode(envelope, @"descriptor_failed");
        return nil;
    }

    NSDictionary *value = envelope[@"value"];
    NSString *urlString = ReadStringField(value, @"url", @"");
    NSURL *url = urlString.length ? [NSURL URLWithString:urlString] : nil;
    if (!url || !url.scheme.length || !url.host.length) {
        *errorCode = @"polish_invalid_endpoint";
        return nil;
    }

    NSMutableArray<NSDictionary<NSString *, NSString *> *> *headers = [NSMutableArray array];
    id rawHeaders = value[@"headers"];
    if ([rawHeaders isKindOfClass:NSArray.class]) {
        for (id item in rawHeaders) {
            if (![item isKindOfClass:NSDictionary.class]) continue;
            id name = item[@"name"];
            id headerValue = item[@"value"];
            if ([name isKindOfClass:NSString.class] && [headerValue isKindOfClass:NSString.class]) {
                [headers addObject:@{@"name" : name, @"value" : headerValue}];
            }
        }
    } else if ([rawHeaders isKindOfClass:NSDictionary.class]) {
        for (id key in rawHeaders) {
            id headerValue = rawHeaders[key];
            if ([key isKindOfClass:NSString.class] && [headerValue isKindOfClass:NSString.class]) {
                [headers addObject:@{@"name" : key, @"value" : headerValue}];
            }
        }
    }

    NSData *bodyData = nil;
    id rawBody = value[@"body"];
    if ([rawBody isKindOfClass:NSString.class]) {
        bodyData = [static_cast<NSString *>(rawBody) dataUsingEncoding:NSUTF8StringEncoding];
    } else if ([rawBody isKindOfClass:NSDictionary.class] || [rawBody isKindOfClass:NSArray.class]) {
        bodyData = [NSJSONSerialization dataWithJSONObject:rawBody options:0 error:nil];
    }
    if (!bodyData) {
        *errorCode = @"descriptor_failed";
        return nil;
    }

    uint64_t timeoutMs = ReadUInt64Field(value, @"timeout_ms", kDefaultTimeoutMilliseconds);
    if (timeoutMs < kMinTimeoutMilliseconds) timeoutMs = kMinTimeoutMilliseconds;
    if (timeoutMs > kMaxTimeoutMilliseconds) timeoutMs = kMaxTimeoutMilliseconds;

    uint64_t connectTimeoutMs = ReadUInt64Field(value, @"connect_timeout_ms", 5000);
    if (connectTimeoutMs < kMinTimeoutMilliseconds) connectTimeoutMs = kMinTimeoutMilliseconds;
    if (connectTimeoutMs > timeoutMs) connectTimeoutMs = timeoutMs;

    uint64_t maxBytes = ReadUInt64Field(value, @"max_response_bytes", kDefaultMaxResponseBytes);
    if (maxBytes > kHardMaxResponseBytes) maxBytes = kHardMaxResponseBytes;

    MSIMEVoicePolishDescriptor *descriptor = [MSIMEVoicePolishDescriptor new];
    descriptor.url = url;
    descriptor.method = ReadStringField(value, @"method", @"POST");
    descriptor.headers = headers;
    descriptor.bodyData = bodyData;
    descriptor.timeoutMs = timeoutMs;
    descriptor.connectTimeoutMs = connectTimeoutMs;
    descriptor.maxResponseBytes = static_cast<NSUInteger>(maxBytes);
    // 纵深防御：任意明文 http 地址一律直连不走系统代理，防止本机或局域网凭据泄露到外部代理。
    descriptor.noProxy = ReadBoolField(value, @"no_proxy", NO) ||
                         [url.scheme.lowercaseString isEqualToString:@"http"];
    descriptor.resolvedMode = ReadStringField(value, @"resolved_mode", @"dictation");
    descriptor.expectActionTag = ReadBoolField(value, @"expect_action_tag", NO);
    descriptor.appCategory = ReadStringField(value, @"app_category", @"general");
    descriptor.stream = ReadBoolField(value, @"stream", request.stream);
    return descriptor;
}

- (void)cleanAndFinishPolishedText:(NSString *)polishedText
                        descriptor:(MSIMEVoicePolishDescriptor *)descriptor
                           request:(MSIMEVoicePolishRequest *)request
                              task:(MSIMEVoicePolishTask *)task
                         startedAt:(uint64_t)startedAt
                     usedStreaming:(BOOL)usedStreaming
                        completion:(void (^)(MSIMEVoicePolishResult *))completion {
    if (task.cancelled) return;
    if (!IsBridgeComplete(_bridge)) {
        [self degradeTask:task
                  request:request
                startedAt:startedAt
                   reason:@"bridge_unavailable"
             resolvedMode:descriptor.resolvedMode
              appCategory:descriptor.appCategory
            usedStreaming:usedStreaming
               completion:completion];
        return;
    }

    NSMutableDictionary<NSString *, id> *cleanPayload = [@{
        @"raw_text" : request.rawText ?: @"",
        @"polished_text" : polishedText ?: @"",
        @"mode" : descriptor.resolvedMode ?: @"dictation",
        @"expect_action_tag" : @(descriptor.expectActionTag),
        @"app_category" : descriptor.appCategory ?: @"general",
        @"enable_pangu_spacing" : @(request.enablePanguSpacing),
    } mutableCopy];
    if (request.context.selectedText.length) {
        cleanPayload[@"selected_text"] = request.context.selectedText;
    }

    NSData *cleanJSON = [NSJSONSerialization dataWithJSONObject:cleanPayload options:0 error:nil];
    if (!cleanJSON) {
        [self degradeTask:task
                  request:request
                startedAt:startedAt
                   reason:@"clean_error"
             resolvedMode:descriptor.resolvedMode
              appCategory:descriptor.appCategory
            usedStreaming:usedStreaming
               completion:completion];
        return;
    }

    ScopedBridgeString rawClean(
        _bridge.cleanOutput(static_cast<const uint8_t *>(cleanJSON.bytes), cleanJSON.length),
        _bridge.freeString);
    NSDictionary *envelope = ParseJSONEnvelope(rawClean.pointer);
    if (!envelope || ![envelope[@"ok"] isEqual:@YES] || ![envelope[@"value"] isKindOfClass:NSDictionary.class]) {
        [self degradeTask:task
                  request:request
                startedAt:startedAt
                   reason:ExtractEnvelopeErrorCode(envelope, @"clean_error")
             resolvedMode:descriptor.resolvedMode
              appCategory:descriptor.appCategory
            usedStreaming:usedStreaming
               completion:completion];
        return;
    }

    NSDictionary *value = envelope[@"value"];
    const BOOL accepted = ReadBoolField(value, @"accepted", NO);
    NSString *cleanedText = [value[@"text"] isKindOfClass:NSString.class] ? value[@"text"] : @"";
    NSString *rejectionReason = [value[@"rejection_reason"] isKindOfClass:NSString.class]
                                    ? Trimmed(value[@"rejection_reason"])
                                    : @"";

    if (!accepted || !cleanedText.length) {
        [self degradeTask:task
                  request:request
                startedAt:startedAt
                   reason:(rejectionReason.length ? rejectionReason : @"polish_rejected")
             resolvedMode:descriptor.resolvedMode
              appCategory:descriptor.appCategory
            usedStreaming:usedStreaming
               completion:completion];
        return;
    }

    NSString *rawAction = ReadStringField(value, @"resolved_action", @"insert");
    NSString *resolvedAction = [rawAction isEqualToString:@"replace_selection"] ? @"replace_selection" : @"insert";
    const uint64_t elapsedMs = ElapsedMillisecondsSince(startedAt);
    os_log_info(PolishClientLog(), "语音润色完成（provider=%{public}@ mode=%{public}@ action=%{public}@ stream=%d ms=%llu）",
                request.provider ?: @"", descriptor.resolvedMode ?: @"dictation", resolvedAction,
                usedStreaming ? 1 : 0, static_cast<unsigned long long>(elapsedMs));

    MSIMEVoicePolishResult *result = [[MSIMEVoicePolishResult alloc]
        initWithText:cleanedText
        polishFailed:NO
polishRejectedReason:nil
  polishMilliseconds:elapsedMs
        resolvedMode:descriptor.resolvedMode
      resolvedAction:resolvedAction
         appCategory:descriptor.appCategory
       usedStreaming:usedStreaming];
    [self deliverResult:result forTask:task completion:completion];
}

- (void)executeAttemptWithRequest:(MSIMEVoicePolishRequest *)request
                         endpoint:(NSString *)endpoint
                            model:(NSString *)model
                            token:(nullable NSString *)token
                    localEndpoint:(nullable MSIMELocalPolishServerEndpoint *)localEndpoint
                   alreadyRetried:(BOOL)alreadyRetried
                             task:(MSIMEVoicePolishTask *)task
                        startedAt:(uint64_t)startedAt
                          partial:(nullable void (^)(NSString *))partial
                       completion:(void (^)(MSIMEVoicePolishResult *))completion {
    if (task.cancelled) return;

    NSString *descriptorError = nil;
    MSIMEVoicePolishDescriptor *descriptor = [self buildDescriptorForRequest:request
                                                                    endpoint:endpoint
                                                                       model:model
                                                                       token:token
                                                                   errorCode:&descriptorError];
    if (!descriptor) {
        [self degradeTask:task
                  request:request
                startedAt:startedAt
                   reason:(descriptorError ?: @"descriptor_failed")
             resolvedMode:@"dictation"
              appCategory:@"general"
            usedStreaming:NO
               completion:completion];
        return;
    }

    __weak MSIMEVoicePolishClient *weakSelf = self;
    void (^partialForward)(NSString *) = nil;
    if (partial) {
        partialForward = ^(NSString *accumulatedText) {
            MSIMEVoicePolishClient *strongSelf = weakSelf;
            if (!strongSelf || task.cancelled || task.completed) return;
            dispatch_queue_t targetQueue = [strongSelf effectiveCallbackQueue];
            dispatch_async(targetQueue, ^{
                if (task.cancelled || task.completed) return;
                partial(accumulatedText);
            });
        };
    }

    MSIMEVoicePolishSessionAttempt *attempt = [[MSIMEVoicePolishSessionAttempt alloc]
        initWithDescriptor:descriptor
                    bridge:_bridge
             isCancelledFn:^BOOL { return task.cancelled; }
                 onPartial:partialForward
                completion:^(NSString * _Nullable polishedText,
                             NSString * _Nullable errorReason,
                             NSError * _Nullable urlError,
                             BOOL usedStreaming) {
                    MSIMEVoicePolishClient *strongSelf = weakSelf;
                    if (!strongSelf || task.cancelled) return;

                    if (errorReason != nil) {
                        // 本机托管服务遇到回环连接被拒或中断时，最多尝试一次受限频保护的重启。
                        if (localEndpoint != nil &&
                            !alreadyRetried &&
                            strongSelf->_localServer != nil &&
                            IsLoopbackConnectionRetryableError(urlError)) {
                            os_log_info(PolishClientLog(), "本机润色端点连接中断，触发一次自动重启重试");
                            [strongSelf->_localServer
                                handleConnectionFailureForEndpoint:localEndpoint
                                                        completion:^(MSIMELocalPolishServerEndpoint * _Nullable restarted,
                                                                     NSError * _Nullable restartError) {
                                                            MSIMEVoicePolishClient *retrySelf = weakSelf;
                                                            if (!retrySelf || task.cancelled) return;
                                                            if (!restarted || restartError != nil) {
                                                                NSString *code = (restartError.code == MSIMELocalPolishServerErrorRateLimited)
                                                                                     ? @"local_server_rate_limited"
                                                                                     : @"local_server_unavailable";
                                                                [retrySelf degradeTask:task
                                                                               request:request
                                                                             startedAt:startedAt
                                                                                reason:code
                                                                          resolvedMode:descriptor.resolvedMode
                                                                           appCategory:descriptor.appCategory
                                                                         usedStreaming:usedStreaming
                                                                            completion:completion];
                                                                return;
                                                            }
                                                            NSString *retryModel = Trimmed(request.model).length ? Trimmed(request.model) : restarted.modelAlias;
                                                            [retrySelf executeAttemptWithRequest:request
                                                                                        endpoint:restarted.chatCompletionsURL
                                                                                           model:retryModel
                                                                                           token:restarted.apiKey
                                                                                   localEndpoint:restarted
                                                                                  alreadyRetried:YES
                                                                                            task:task
                                                                                       startedAt:startedAt
                                                                                         partial:partial
                                                                                      completion:completion];
                                                        }];
                            return;
                        }

                        [strongSelf degradeTask:task
                                        request:request
                                      startedAt:startedAt
                                         reason:errorReason
                                   resolvedMode:descriptor.resolvedMode
                                    appCategory:descriptor.appCategory
                                  usedStreaming:usedStreaming
                                     completion:completion];
                        return;
                    }

                    if (localEndpoint != nil) {
                        [strongSelf->_localServer touchActivity];
                    }

                    dispatch_async(strongSelf->_workQueue, ^{
                        [strongSelf cleanAndFinishPolishedText:(polishedText ?: @"")
                                                    descriptor:descriptor
                                                       request:request
                                                          task:task
                                                     startedAt:startedAt
                                                 usedStreaming:usedStreaming
                                                    completion:completion];
                    });
                }];

    [task setActiveAttempt:attempt];
    [attempt start];
}

- (MSIMEVoicePolishTask *)polishRequest:(MSIMEVoicePolishRequest *)request
                               slowHint:(nullable void (^)(NSString *))slowHint
                                partial:(nullable void (^)(NSString *))partial
                             completion:(void (^)(MSIMEVoicePolishResult *))completion {
    MSIMEVoicePolishRequest *requestCopy = [request copy];
    void (^slowHintCopy)(NSString *) = [slowHint copy];
    void (^partialCopy)(NSString *) = [partial copy];
    void (^completionCopy)(MSIMEVoicePolishResult *) = [completion copy];
    MSIMEVoicePolishTask *task = [MSIMEVoicePolishTask new];
    const uint64_t startedAt = MonotonicNowNanoseconds();

    // 7.5 秒慢响应提示定时器（可在测试中缩短延迟验证）。
    const NSTimeInterval hintDelay = _slowHintDelay;
    NSString *hintMessage = [_slowHintMessage copy] ?: @"正在润色，请稍候…";
    if (slowHintCopy && hintDelay > 0.0) {
        dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, _workQueue);
        if (timer) {
            const uint64_t delayNsec = static_cast<uint64_t>(hintDelay * static_cast<double>(NSEC_PER_SEC));
            dispatch_source_set_timer(timer,
                                      dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(delayNsec)),
                                      DISPATCH_TIME_FOREVER,
                                      20 * 1000000ULL);
            __weak MSIMEVoicePolishClient *weakSelf = self;
            dispatch_source_set_event_handler(timer, ^{
                MSIMEVoicePolishClient *strongSelf = weakSelf;
                if (!strongSelf || task.cancelled || task.completed) return;
                dispatch_queue_t targetQueue = [strongSelf effectiveCallbackQueue];
                dispatch_async(targetQueue, ^{
                    if (task.cancelled || task.completed) return;
                    slowHintCopy(hintMessage);
                });
            });
            [task installAndResumeSlowHintTimer:timer];
        }
    }

    NSString *providerLower = Trimmed(requestCopy.provider).lowercaseString;
    if ([providerLower isEqualToString:@"local"]) {
        if (!_localServer) {
            dispatch_async(_workQueue, ^{
                [self degradeTask:task
                          request:requestCopy
                        startedAt:startedAt
                           reason:@"local_server_unavailable"
                     resolvedMode:@"dictation"
                      appCategory:@"general"
                    usedStreaming:NO
                       completion:completionCopy];
            });
            return task;
        }

        __weak MSIMEVoicePolishClient *weakSelf = self;
        [_localServer ensureReadyWithModelPath:requestCopy.localModelPath
                                    completion:^(MSIMELocalPolishServerEndpoint * _Nullable localEndpoint,
                                                 NSError * _Nullable error) {
                                        MSIMEVoicePolishClient *strongSelf = weakSelf;
                                        if (!strongSelf || task.cancelled) return;
                                        if (!localEndpoint || error != nil) {
                                            NSString *reason = (error.code == MSIMELocalPolishServerErrorRateLimited)
                                                                   ? @"local_server_rate_limited"
                                                                   : @"local_server_unavailable";
                                            [strongSelf degradeTask:task
                                                            request:requestCopy
                                                          startedAt:startedAt
                                                             reason:reason
                                                       resolvedMode:@"dictation"
                                                        appCategory:@"general"
                                                      usedStreaming:NO
                                                         completion:completionCopy];
                                            return;
                                        }
                                        NSString *effectiveModel = Trimmed(requestCopy.model).length
                                                                       ? Trimmed(requestCopy.model)
                                                                       : localEndpoint.modelAlias;
                                        [strongSelf executeAttemptWithRequest:requestCopy
                                                                     endpoint:localEndpoint.chatCompletionsURL
                                                                        model:effectiveModel
                                                                        token:localEndpoint.apiKey
                                                                localEndpoint:localEndpoint
                                                               alreadyRetried:NO
                                                                         task:task
                                                                    startedAt:startedAt
                                                                      partial:partialCopy
                                                                   completion:completionCopy];
                                    }];
        return task;
    }

    dispatch_async(_workQueue, ^{
        [self executeAttemptWithRequest:requestCopy
                               endpoint:(requestCopy.endpoint ?: @"")
                                  model:(requestCopy.model ?: @"")
                                  token:requestCopy.token
                          localEndpoint:nil
                         alreadyRetried:NO
                                   task:task
                              startedAt:startedAt
                                partial:partialCopy
                             completion:completionCopy];
    });
    return task;
}

@end
