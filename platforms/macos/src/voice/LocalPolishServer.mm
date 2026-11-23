#import "LocalPolishServer.h"

#import <Security/Security.h>
#import <arpa/inet.h>
#import <errno.h>
#import <fcntl.h>
#import <netinet/in.h>
#import <os/log.h>
#import <signal.h>
#import <sys/socket.h>
#import <sys/stat.h>
#import <unistd.h>

#include <array>
#include <cstdint>
#include <cstring>
#include <vector>

NSErrorDomain const MSIMELocalPolishServerErrorDomain = @"dev.shurufa.voice.local-polish-server";

namespace {

constexpr uint64_t kGiB = 1024ULL * 1024ULL * 1024ULL;
constexpr uint64_t kLowMemoryThresholdBytes = 24ULL * kGiB;
constexpr uint64_t kHighMemoryThresholdBytes = 48ULL * kGiB;
constexpr NSUInteger kMaxStderrLines = 32;
constexpr NSUInteger kMaxStderrLineLength = 256;
constexpr NSUInteger kMaxStderrPendingBytes = 8192;
constexpr size_t kApiKeyRandomBytes = 32;

os_log_t LocalPolishLog(void) {
    static os_log_t log = nullptr;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        log = os_log_create("dev.shurufa.ime", "local-polish-server");
    });
    return log;
}

void ZeroSensitiveBuffer(void *buffer, size_t length) {
    if (!buffer || length == 0) return;
    volatile uint8_t *cursor = static_cast<volatile uint8_t *>(buffer);
    for (size_t index = 0; index < length; ++index) {
        cursor[index] = 0;
    }
}

NSString *TrimmedString(NSString * _Nullable value) {
    if (!value) return @"";
    return [value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
}

BOOL IsRegularFileAtPathFollowingSymlinks(NSString *path, BOOL requireExecutable) {
    if (!path.length || !path.isAbsolutePath) return NO;
    const char *fsPath = path.fileSystemRepresentation;
    if (!fsPath || !*fsPath) return NO;
    // 使用 stat 跟随软链接，兼容 Homebrew 二进制和 HuggingFace 缓存软链。
    struct stat fileStat = {};
    if (::stat(fsPath, &fileStat) != 0) return NO;
    if (!S_ISREG(fileStat.st_mode)) return NO;
    const int mode = requireExecutable ? X_OK : R_OK;
    return ::access(fsPath, mode) == 0;
}

BOOL IsDirectoryAtPathFollowingSymlinks(NSString *path) {
    if (!path.length || !path.isAbsolutePath) return NO;
    const char *fsPath = path.fileSystemRepresentation;
    if (!fsPath || !*fsPath) return NO;
    struct stat dirStat = {};
    if (::stat(fsPath, &dirStat) != 0) return NO;
    return S_ISDIR(dirStat.st_mode);
}

BOOL IsLikelyTranslationOnlyGGUF(NSString *fileName) {
    NSString *lower = fileName.lowercaseString;
    return [lower containsString:@"hy-mt"] || [lower containsString:@"hymt"];
}

NSError *MakeServerError(MSIMELocalPolishServerErrorCode code, NSString *description) {
    return [NSError errorWithDomain:MSIMELocalPolishServerErrorDomain
                               code:code
                           userInfo:@{NSLocalizedDescriptionKey : description ?: @"本机润色服务不可用"}];
}

uint16_t AllocateLoopbackPort(void) {
    const int fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return 0;
    int reuse = 1;
    (void)::setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
    struct sockaddr_in address = {};
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = htons(0);
    if (::bind(fd, reinterpret_cast<struct sockaddr *>(&address), sizeof(address)) != 0) {
        ::close(fd);
        return 0;
    }
    socklen_t length = sizeof(address);
    if (::getsockname(fd, reinterpret_cast<struct sockaddr *>(&address), &length) != 0) {
        ::close(fd);
        return 0;
    }
    const uint16_t port = ntohs(address.sin_port);
    ::close(fd);
    return port;
}

NSString * _Nullable GenerateEphemeralApiKey(void) {
    std::array<uint8_t, kApiKeyRandomBytes> randomBytes{};
    if (SecRandomCopyBytes(kSecRandomDefault, randomBytes.size(), randomBytes.data()) != errSecSuccess) {
        ZeroSensitiveBuffer(randomBytes.data(), randomBytes.size());
        return nil;
    }
    static constexpr char kHexDigits[] = "0123456789abcdef";
    std::array<char, kApiKeyRandomBytes * 2 + 1> hexBuffer{};
    for (size_t index = 0; index < randomBytes.size(); ++index) {
        hexBuffer[index * 2] = kHexDigits[(randomBytes[index] >> 4) & 0x0F];
        hexBuffer[index * 2 + 1] = kHexDigits[randomBytes[index] & 0x0F];
    }
    hexBuffer[kApiKeyRandomBytes * 2] = '\0';
    NSString *key = [[NSString alloc] initWithBytes:hexBuffer.data()
                                             length:kApiKeyRandomBytes * 2
                                           encoding:NSASCIIStringEncoding];
    ZeroSensitiveBuffer(randomBytes.data(), randomBytes.size());
    ZeroSensitiveBuffer(hexBuffer.data(), hexBuffer.size());
    return key;
}

NSString *SanitizeDiagnosticLine(NSString *rawLine, NSString * _Nullable activeApiKey) {
    NSString *line = [rawLine stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (!line.length) return @"";
    if (activeApiKey.length) {
        line = [line stringByReplacingOccurrencesOfString:activeApiKey withString:@"<redacted>"];
    }
    NSRange apiKeyFlag = [line rangeOfString:@"--api-key" options:NSCaseInsensitiveSearch];
    if (apiKeyFlag.location != NSNotFound) {
        line = [[line substringToIndex:apiKeyFlag.location] stringByAppendingString:@"--api-key <redacted>"];
    }
    NSRange bearerRange = [line rangeOfString:@"Bearer " options:NSCaseInsensitiveSearch];
    if (bearerRange.location != NSNotFound) {
        line = [[line substringToIndex:bearerRange.location] stringByAppendingString:@"Bearer <redacted>"];
    }
    if (line.length > kMaxStderrLineLength) {
        NSRange boundary = [line rangeOfComposedCharacterSequenceAtIndex:kMaxStderrLineLength];
        const NSUInteger cutIndex = boundary.location < kMaxStderrLineLength ? boundary.location : kMaxStderrLineLength;
        line = [line substringToIndex:cutIndex];
    }
    return line;
}

double MonotonicNowSeconds(void) {
    return static_cast<double>(clock_gettime_nsec_np(CLOCK_UPTIME_RAW)) / 1e9;
}

void TerminateSubprocess(NSTask * _Nullable task,
                         NSPipe * _Nullable stdinPipe,
                         NSPipe * _Nullable stderrPipe,
                         NSTimeInterval graceTimeout,
                         BOOL synchronous) {
    if (stderrPipe) {
        stderrPipe.fileHandleForReading.readabilityHandler = nil;
    }
    if (stdinPipe) {
        [stdinPipe.fileHandleForWriting closeAndReturnError:nil];
    }
    if (!task) return;

    const pid_t pid = task.processIdentifier;
    const NSTimeInterval grace = graceTimeout > 0.0 ? graceTimeout : 2.0;

    if (task.running) {
        [task terminate];
    }

    if (synchronous) {
        const double deadline = MonotonicNowSeconds() + grace;
        while (task.running && MonotonicNowSeconds() < deadline) {
            usleep(20 * 1000);
        }
        if (task.running && pid > 0) {
            ::kill(pid, SIGKILL);
            const double killDeadline = MonotonicNowSeconds() + 0.5;
            while (task.running && MonotonicNowSeconds() < killDeadline) {
                usleep(10 * 1000);
            }
        }
        return;
    }

    // 异步停止不在串行队列上阻塞等待宽限期，后台定时器超时后兜底 SIGKILL。
    if (task.running && pid > 0) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(grace * static_cast<double>(NSEC_PER_SEC))),
                       dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
                           if (task.running) {
                               ::kill(pid, SIGKILL);
                           }
                       });
    }
}

enum class ServerState : int {
    Idle = 0,
    Starting = 1,
    Running = 2,
};

} // namespace

NSString * _Nullable MSIMELocalPolishServerDiscoverExecutable(
    NSString * _Nullable bundlePath,
    NSDictionary<NSString *, NSString *> * _Nullable environment) {
    NSDictionary<NSString *, NSString *> *env = environment ?: NSProcessInfo.processInfo.environment;
    NSString *envOverride = TrimmedString(env[@"SHURUFA_LLAMA_SERVER_BIN"]);
    if (envOverride.length) {
        NSString *expanded = envOverride.stringByExpandingTildeInPath;
        if (IsRegularFileAtPathFollowingSymlinks(expanded, YES)) {
            return expanded;
        }
    }

    NSString *effectiveBundle = TrimmedString(bundlePath);
    if (!effectiveBundle.length) {
        effectiveBundle = NSBundle.mainBundle.bundlePath;
    }
    if (effectiveBundle.length) {
        NSString *bundledHelper = [effectiveBundle stringByAppendingPathComponent:@"Contents/Helpers/llama-server"];
        if (IsRegularFileAtPathFollowingSymlinks(bundledHelper, YES)) {
            return bundledHelper;
        }
    }

    NSArray<NSString *> *systemCandidates = @[
        @"/opt/homebrew/bin/llama-server",
        @"/usr/local/bin/llama-server",
    ];
    for (NSString *candidate in systemCandidates) {
        if (IsRegularFileAtPathFollowingSymlinks(candidate, YES)) {
            return candidate;
        }
    }
    return nil;
}

NSString *MSIMELocalPolishServerDefaultModelDirectory(
    NSDictionary<NSString *, NSString *> * _Nullable environment) {
    NSDictionary<NSString *, NSString *> *env = environment ?: NSProcessInfo.processInfo.environment;
    NSString *envDir = TrimmedString(env[@"SHURUFA_LOCAL_LLM_DIR"]);
    if (envDir.length) {
        NSString *expanded = envDir.stringByExpandingTildeInPath;
        if (expanded.isAbsolutePath) {
            return expanded;
        }
    }
    return [@"~/Library/Application Support/Shurufa/local-llm" stringByExpandingTildeInPath];
}

NSString * _Nullable MSIMELocalPolishServerDiscoverModelPath(
    NSString * _Nullable explicitModelPath,
    NSString * _Nullable localLLMDirectory) {
    NSString *explicitTrimmed = TrimmedString(explicitModelPath);
    if (explicitTrimmed.length) {
        NSString *expanded = explicitTrimmed.stringByExpandingTildeInPath;
        if (IsRegularFileAtPathFollowingSymlinks(expanded, NO)) {
            return expanded;
        }
    }

    NSString *directory = localLLMDirectory != nil
                              ? TrimmedString(localLLMDirectory).stringByExpandingTildeInPath
                              : MSIMELocalPolishServerDefaultModelDirectory(nil);
    if (!IsDirectoryAtPathFollowingSymlinks(directory)) {
        return nil;
    }

    NSArray<NSString *> *entries = [NSFileManager.defaultManager contentsOfDirectoryAtPath:directory error:nil];
    if (!entries.count) return nil;

    NSArray<NSString *> *sorted = [entries sortedArrayUsingComparator:^NSComparisonResult(NSString *lhs, NSString *rhs) {
        NSComparisonResult ci = [lhs caseInsensitiveCompare:rhs];
        return ci != NSOrderedSame ? ci : [lhs compare:rhs];
    }];

    NSString *fallbackTranslationModel = nil;
    for (NSString *name in sorted) {
        if (![name.pathExtension.lowercaseString isEqualToString:@"gguf"]) continue;
        if ([name.lowercaseString containsString:@"mmproj"]) continue;
        NSString *fullPath = [directory stringByAppendingPathComponent:name];
        if (!IsRegularFileAtPathFollowingSymlinks(fullPath, NO)) continue;
        if (IsLikelyTranslationOnlyGGUF(name)) {
            if (!fallbackTranslationModel) fallbackTranslationModel = fullPath;
            continue;
        }
        return fullPath;
    }
    return fallbackTranslationModel;
}

NSTimeInterval MSIMELocalPolishServerIdleTimeoutForMemoryBytes(uint64_t physicalMemoryBytes) {
    if (physicalMemoryBytes >= kHighMemoryThresholdBytes) return 1800.0;
    if (physicalMemoryBytes >= kLowMemoryThresholdBytes) return 600.0;
    return 180.0;
}

NSUInteger MSIMELocalPolishServerContextSizeForMemoryBytes(uint64_t physicalMemoryBytes) {
    return physicalMemoryBytes >= kLowMemoryThresholdBytes ? 4096U : 2048U;
}

NSArray<NSString *> *MSIMELocalPolishServerBuildLaunchArguments(
    uint16_t port,
    NSString *modelPath,
    NSString *modelAlias,
    NSUInteger contextSize,
    NSString *apiKey) {
    NSString *alias = TrimmedString(modelAlias);
    if (!alias.length) alias = @"local";
    const NSUInteger effectiveCtx = contextSize > 0 ? contextSize : 4096U;
    return @[
        @"--host", @"127.0.0.1",
        @"--port", [NSString stringWithFormat:@"%u", static_cast<unsigned>(port)],
        @"--model", modelPath,
        @"--alias", alias,
        @"--ctx-size", [NSString stringWithFormat:@"%lu", static_cast<unsigned long>(effectiveCtx)],
        @"--n-gpu-layers", @"99",
        @"--flash-attn", @"on",
        @"--reasoning-budget", @"0",
        @"--parallel", @"1",
        @"--no-webui",
        @"--log-disable",
        @"--api-key", apiKey,
    ];
}

// 独立健康检查会话委托，避免强引用 MSIMELocalPolishServer 形成引用环，同时严格禁止跟随重定向。
@interface MSIMELocalPolishHealthDelegate : NSObject <NSURLSessionTaskDelegate>
@end

@implementation MSIMELocalPolishHealthDelegate
- (void)URLSession:(NSURLSession *)session
                          task:(NSURLSessionTask *)task
    willPerformHTTPRedirection:(NSHTTPURLResponse *)response
                    newRequest:(NSURLRequest *)request
             completionHandler:(void (^)(NSURLRequest * _Nullable))completionHandler {
    (void)session;
    (void)task;
    (void)response;
    (void)request;
    completionHandler(nil);
}
@end

@implementation MSIMELocalPolishServerConfiguration

- (instancetype)init {
    if (!(self = [super init])) return nil;
    _modelAlias = @"local";
    _preferredPort = 0;
    _physicalMemoryBytesOverride = 0;
    _idleTimeoutOverride = 0.0;
    _contextSizeOverride = 0;
    _healthTimeout = 15.0;
    _healthPollInterval = 0.10;
    _terminateGraceTimeout = 2.0;
    _restartRateWindow = 60.0;
    _maxRestartsInWindow = 2;
    _listenToSystemMemoryPressure = YES;
    _listenToApplicationTermination = YES;
    return self;
}

- (id)copyWithZone:(nullable NSZone *)zone {
    MSIMELocalPolishServerConfiguration *copy = [[[self class] allocWithZone:zone] init];
    copy.executablePath = self.executablePath;
    copy.modelDirectory = self.modelDirectory;
    copy.bundlePath = self.bundlePath;
    copy.environment = self.environment;
    copy.modelAlias = self.modelAlias;
    copy.preferredPort = self.preferredPort;
    copy.physicalMemoryBytesOverride = self.physicalMemoryBytesOverride;
    copy.idleTimeoutOverride = self.idleTimeoutOverride;
    copy.contextSizeOverride = self.contextSizeOverride;
    copy.healthTimeout = self.healthTimeout;
    copy.healthPollInterval = self.healthPollInterval;
    copy.terminateGraceTimeout = self.terminateGraceTimeout;
    copy.restartRateWindow = self.restartRateWindow;
    copy.maxRestartsInWindow = self.maxRestartsInWindow;
    copy.listenToSystemMemoryPressure = self.listenToSystemMemoryPressure;
    copy.listenToApplicationTermination = self.listenToApplicationTermination;
    copy.onLowMemoryPreLaunch = self.onLowMemoryPreLaunch;
    return copy;
}

@end

@interface MSIMELocalPolishServerEndpoint ()
- (instancetype)initWithPort:(uint16_t)port
                      apiKey:(NSString *)apiKey
                   modelPath:(NSString *)modelPath
                  modelAlias:(NSString *)modelAlias
                  generation:(uint64_t)generation;
@end

@implementation MSIMELocalPolishServerEndpoint

- (instancetype)initWithPort:(uint16_t)port
                      apiKey:(NSString *)apiKey
                   modelPath:(NSString *)modelPath
                  modelAlias:(NSString *)modelAlias
                  generation:(uint64_t)generation {
    if (!(self = [super init])) return nil;
    _port = port;
    _baseURL = [NSString stringWithFormat:@"http://127.0.0.1:%u", static_cast<unsigned>(port)];
    _chatCompletionsURL = [NSString stringWithFormat:@"http://127.0.0.1:%u/v1/chat/completions", static_cast<unsigned>(port)];
    _apiKey = [apiKey copy];
    _modelPath = [modelPath copy];
    _modelAlias = [modelAlias copy];
    _generation = generation;
    return self;
}

- (NSString *)description {
    return [NSString stringWithFormat:@"<MSIMELocalPolishServerEndpoint baseURL=%@ alias=%@ gen=%llu apiKey=<redacted>>",
                                      _baseURL, _modelAlias, static_cast<unsigned long long>(_generation)];
}

- (NSString *)debugDescription {
    return [self description];
}

@end

typedef void (^MSIMELocalPolishReadyCallback)(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error);

@implementation MSIMELocalPolishServer {
    MSIMELocalPolishServerConfiguration *_config;
    dispatch_queue_t _queue;
    dispatch_queue_t _callbackQueue;
    NSURLSession *_healthSession;
    MSIMELocalPolishHealthDelegate *_healthDelegate;
    dispatch_source_t _memoryPressureSource;
    dispatch_source_t _idleTimer;
    id<NSObject> _terminationObserver;

    ServerState _state;
    uint64_t _generation;
    NSTask *_task;
    NSPipe *_stdinPipe;
    NSPipe *_stderrPipe;
    MSIMELocalPolishServerEndpoint *_startingEndpoint;
    MSIMELocalPolishServerEndpoint *_runningEndpoint;
    NSMutableArray<MSIMELocalPolishReadyCallback> *_pendingCallbacks;
    NSMutableArray<NSString *> *_stderrLines;
    std::vector<double> _restartTimestamps;
    BOOL _lastRunEndedUnexpectedly;
}

+ (instancetype)sharedServer {
    static MSIMELocalPolishServer *shared = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        shared = [[MSIMELocalPolishServer alloc] initWithConfiguration:nil];
    });
    return shared;
}

- (instancetype)init {
    return [self initWithConfiguration:nil];
}

- (instancetype)initWithConfiguration:(nullable MSIMELocalPolishServerConfiguration *)configuration {
    if (!(self = [super init])) return nil;
    _config = configuration ? [configuration copy] : [MSIMELocalPolishServerConfiguration new];
    _queue = dispatch_queue_create("dev.shurufa.voice.local-polish-server",
                                   dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INITIATED, 0));
    _callbackQueue = dispatch_queue_create("dev.shurufa.voice.local-polish-server.callback",
                                           dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_CONCURRENT, QOS_CLASS_USER_INITIATED, 0));
    _pendingCallbacks = [NSMutableArray array];
    _stderrLines = [NSMutableArray array];
    _state = ServerState::Idle;

    // 独立无代理、无缓存、禁止重定向的健康检查会话。
    NSURLSessionConfiguration *sessionConfig = [NSURLSessionConfiguration ephemeralSessionConfiguration];
    sessionConfig.connectionProxyDictionary = @{};
    sessionConfig.URLCache = nil;
    sessionConfig.HTTPCookieStorage = nil;
    sessionConfig.URLCredentialStorage = nil;
    sessionConfig.HTTPShouldSetCookies = NO;
    sessionConfig.timeoutIntervalForRequest = 2.0;
    sessionConfig.timeoutIntervalForResource = 2.0;
    _healthDelegate = [MSIMELocalPolishHealthDelegate new];
    _healthSession = [NSURLSession sessionWithConfiguration:sessionConfig
                                                   delegate:_healthDelegate
                                              delegateQueue:nil];

    if (_config.listenToSystemMemoryPressure) {
        [self installMemoryPressureSource];
    }
    if (_config.listenToApplicationTermination) {
        [self installApplicationTerminationObserver];
    }
    return self;
}

- (void)dealloc {
    if (_terminationObserver) {
        [NSNotificationCenter.defaultCenter removeObserver:_terminationObserver];
        _terminationObserver = nil;
    }
    if (_memoryPressureSource) {
        dispatch_source_cancel(_memoryPressureSource);
        _memoryPressureSource = nil;
    }
    if (_idleTimer) {
        dispatch_source_cancel(_idleTimer);
        _idleTimer = nil;
    }
    // dealloc 期间禁止通过块再次强引用 self，直接同步回收子进程与管道。
    TerminateSubprocess(_task, _stdinPipe, _stderrPipe, _config.terminateGraceTimeout, YES);
    _task = nil;
    _stdinPipe = nil;
    _stderrPipe = nil;
    [_healthSession invalidateAndCancel];
    _healthSession = nil;
}

#pragma mark - Configuration & Read-only State

- (uint64_t)effectivePhysicalMemoryBytes {
    if (_config.physicalMemoryBytesOverride > 0) {
        return _config.physicalMemoryBytesOverride;
    }
    return static_cast<uint64_t>(NSProcessInfo.processInfo.physicalMemory);
}

- (NSTimeInterval)activeIdleTimeout {
    if (_config.idleTimeoutOverride > 0.0) {
        return _config.idleTimeoutOverride;
    }
    return MSIMELocalPolishServerIdleTimeoutForMemoryBytes([self effectivePhysicalMemoryBytes]);
}

- (NSUInteger)activeContextSize {
    if (_config.contextSizeOverride > 0) {
        return _config.contextSizeOverride;
    }
    return MSIMELocalPolishServerContextSizeForMemoryBytes([self effectivePhysicalMemoryBytes]);
}

- (BOOL)isRunning {
    __block BOOL running = NO;
    dispatch_sync(_queue, ^{
        running = (self->_state == ServerState::Running && self->_task != nil && self->_task.running);
    });
    return running;
}

- (pid_t)processIdentifier {
    __block pid_t pid = 0;
    dispatch_sync(_queue, ^{
        pid = (self->_task != nil && self->_task.running) ? self->_task.processIdentifier : 0;
    });
    return pid;
}

- (uint16_t)listeningPort {
    __block uint16_t port = 0;
    dispatch_sync(_queue, ^{
        if (self->_state == ServerState::Running && self->_runningEndpoint) {
            port = self->_runningEndpoint.port;
        } else if (self->_state == ServerState::Starting && self->_startingEndpoint) {
            port = self->_startingEndpoint.port;
        }
    });
    return port;
}

- (NSUInteger)recentRestartCount {
    __block NSUInteger count = 0;
    dispatch_sync(_queue, ^{
        [self pruneExpiredRestartTimestampsLocked];
        count = static_cast<NSUInteger>(self->_restartTimestamps.size());
    });
    return count;
}

- (NSArray<NSString *> *)recentSanitizedStderrLines {
    __block NSArray<NSString *> *copy = nil;
    dispatch_sync(_queue, ^{
        copy = [self->_stderrLines copy];
    });
    return copy ?: @[];
}

#pragma mark - System Observers

- (void)installMemoryPressureSource {
    __weak MSIMELocalPolishServer *weakSelf = self;
    dispatch_source_t source = dispatch_source_create(
        DISPATCH_SOURCE_TYPE_MEMORYPRESSURE,
        0,
        DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL,
        _queue);
    if (!source) return;
    dispatch_source_set_event_handler(source, ^{
        MSIMELocalPolishServer *strongSelf = weakSelf;
        if (!strongSelf) return;
        os_log_info(LocalPolishLog(), "收到系统内存压力信号，立即优雅停止本机 llama-server");
        [strongSelf stopLockedIntentional:YES synchronous:NO pendingError:nil];
    });
    _memoryPressureSource = source;
    dispatch_resume(source);
}

- (void)installApplicationTerminationObserver {
    __weak MSIMELocalPolishServer *weakSelf = self;
    // 以字符串常量监听 AppKit 退出通知，保持本文件仅依赖 Foundation 与 Security。
    _terminationObserver = [NSNotificationCenter.defaultCenter
        addObserverForName:@"NSApplicationWillTerminateNotification"
                    object:nil
                     queue:nil
                usingBlock:^(NSNotification * _Nonnull note) {
                    (void)note;
                    MSIMELocalPolishServer *strongSelf = weakSelf;
                    if (!strongSelf) return;
                    [strongSelf stopSynchronously];
                }];
}

#pragma mark - Public API

- (void)ensureReadyWithModelPath:(nullable NSString *)modelPath
                      completion:(void (^)(MSIMELocalPolishServerEndpoint * _Nullable, NSError * _Nullable))completion {
    NSString *requestedPath = [modelPath copy];
    MSIMELocalPolishReadyCallback callbackCopy = [completion copy];
    dispatch_async(_queue, ^{
        [self ensureReadyLockedWithModelPath:requestedPath completion:callbackCopy];
    });
}

- (void)prewarmWithModelPath:(nullable NSString *)modelPath {
    [self ensureReadyWithModelPath:modelPath completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
        (void)endpoint;
        (void)error;
    }];
}

- (void)touchActivity {
    dispatch_async(_queue, ^{
        if (self->_state == ServerState::Running && self->_task != nil && self->_task.running) {
            [self armIdleTimerLocked];
        }
    });
}

- (void)handleConnectionFailureForEndpoint:(MSIMELocalPolishServerEndpoint *)failedEndpoint
                                completion:(void (^)(MSIMELocalPolishServerEndpoint * _Nullable, NSError * _Nullable))completion {
    MSIMELocalPolishReadyCallback callbackCopy = [completion copy];
    dispatch_async(_queue, ^{
        if (self->_runningEndpoint && self->_runningEndpoint.generation == failedEndpoint.generation) {
            os_log_error(LocalPolishLog(), "本机润色端点连接异常（gen=%llu），尝试受限频保护的重启",
                         static_cast<unsigned long long>(failedEndpoint.generation));
            [self stopLockedIntentional:NO synchronous:NO pendingError:nil];
        }
        [self ensureReadyLockedWithModelPath:failedEndpoint.modelPath completion:callbackCopy];
    });
}

- (void)handleMemoryPressure {
    dispatch_async(_queue, ^{
        os_log_info(LocalPolishLog(), "触发显式内存压力回收，停止本机 llama-server");
        [self stopLockedIntentional:YES synchronous:NO pendingError:nil];
    });
}

- (void)stop {
    dispatch_async(_queue, ^{
        [self stopLockedIntentional:YES synchronous:NO pendingError:nil];
    });
}

- (void)stopSynchronously {
    dispatch_sync(_queue, ^{
        [self stopLockedIntentional:YES synchronous:YES pendingError:nil];
    });
}

#pragma mark - Queue-internal Lifecycle

- (void)dispatchCallback:(MSIMELocalPolishReadyCallback)callback
                endpoint:(nullable MSIMELocalPolishServerEndpoint *)endpoint
                   error:(nullable NSError *)error {
    if (!callback) return;
    dispatch_async(_callbackQueue, ^{
        callback(endpoint, error);
    });
}

- (void)flushPendingCallbacksLockedWithEndpoint:(nullable MSIMELocalPolishServerEndpoint *)endpoint
                                          error:(nullable NSError *)error {
    if (!_pendingCallbacks.count) return;
    NSArray<MSIMELocalPolishReadyCallback> *callbacks = [_pendingCallbacks copy];
    [_pendingCallbacks removeAllObjects];
    dispatch_async(_callbackQueue, ^{
        for (MSIMELocalPolishReadyCallback callback in callbacks) {
            callback(endpoint, error);
        }
    });
}

- (void)pruneExpiredRestartTimestampsLocked {
    const double now = MonotonicNowSeconds();
    const double window = _config.restartRateWindow > 0.0 ? _config.restartRateWindow : 60.0;
    std::vector<double> kept;
    kept.reserve(_restartTimestamps.size());
    for (double timestamp : _restartTimestamps) {
        if ((now - timestamp) <= window) {
            kept.push_back(timestamp);
        }
    }
    _restartTimestamps.swap(kept);
}

- (void)cancelIdleTimerLocked {
    if (_idleTimer) {
        dispatch_source_cancel(_idleTimer);
        _idleTimer = nil;
    }
}

- (void)armIdleTimerLocked {
    [self cancelIdleTimerLocked];
    const NSTimeInterval timeout = [self activeIdleTimeout];
    if (timeout <= 0.0) return;

    const uint64_t generation = _generation;
    dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, _queue);
    if (!timer) return;
    const uint64_t deltaNsec = static_cast<uint64_t>(timeout * static_cast<double>(NSEC_PER_SEC));
    const uint64_t leewayNsec = static_cast<uint64_t>(MIN(1.0, timeout * 0.1) * static_cast<double>(NSEC_PER_SEC));
    dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(deltaNsec)), DISPATCH_TIME_FOREVER, leewayNsec);
    __weak MSIMELocalPolishServer *weakSelf = self;
    dispatch_source_set_event_handler(timer, ^{
        MSIMELocalPolishServer *strongSelf = weakSelf;
        if (!strongSelf) return;
        if (strongSelf->_generation != generation || strongSelf->_state != ServerState::Running) return;
        os_log_info(LocalPolishLog(), "本机 llama-server 空闲超过 %.1f 秒，执行自动回收", timeout);
        [strongSelf stopLockedIntentional:YES synchronous:NO pendingError:nil];
    });
    _idleTimer = timer;
    dispatch_resume(timer);
}

- (void)ensureReadyLockedWithModelPath:(nullable NSString *)explicitModelPath
                            completion:(MSIMELocalPolishReadyCallback)completion {
    NSString *resolvedModel = MSIMELocalPolishServerDiscoverModelPath(explicitModelPath, _config.modelDirectory);
    if (!resolvedModel.length) {
        [self dispatchCallback:completion
                      endpoint:nil
                         error:MakeServerError(MSIMELocalPolishServerErrorModelNotFound,
                                               @"未找到可用的本机 GGUF 模型文件")];
        return;
    }

    if (_state == ServerState::Running && _task != nil && _task.running && _runningEndpoint != nil) {
        if ([_runningEndpoint.modelPath isEqualToString:resolvedModel]) {
            [self armIdleTimerLocked];
            [self dispatchCallback:completion endpoint:_runningEndpoint error:nil];
            return;
        }
        // 切换模型属于主动重启，不计入异常崩溃限频。
        [self stopLockedIntentional:YES synchronous:NO pendingError:nil];
    } else if (_state == ServerState::Starting && _task != nil && _task.running && _startingEndpoint != nil) {
        if ([_startingEndpoint.modelPath isEqualToString:resolvedModel]) {
            if (completion) [_pendingCallbacks addObject:completion];
            return;
        }
        [self stopLockedIntentional:YES synchronous:NO pendingError:nil];
    } else if (_state != ServerState::Idle) {
        // 进程已经意外消失但退出回调尚未到达，立即清理并冲刷旧等待回调，避免永久挂起。
        [self stopLockedIntentional:NO
                        synchronous:NO
                       pendingError:MakeServerError(MSIMELocalPolishServerErrorProcessExited,
                                                    @"本机润色服务启动期间意外退出")];
    }

    // 异常退出后的重启须受限频保护（默认 60 秒内最多 2 次）。
    if (_lastRunEndedUnexpectedly) {
        [self pruneExpiredRestartTimestampsLocked];
        if (_restartTimestamps.size() >= _config.maxRestartsInWindow) {
            os_log_error(LocalPolishLog(), "本机 llama-server 触发重启限频保护（窗口内已重启 %lu 次）",
                         static_cast<unsigned long>(_restartTimestamps.size()));
            [self dispatchCallback:completion
                          endpoint:nil
                             error:MakeServerError(MSIMELocalPolishServerErrorRateLimited,
                                                   @"本机润色服务短时间内多次异常重启，已暂停自动拉起")];
            return;
        }
        _restartTimestamps.push_back(MonotonicNowSeconds());
    }

    NSString *executable = TrimmedString(_config.executablePath).stringByExpandingTildeInPath;
    if (executable.length) {
        if (!IsRegularFileAtPathFollowingSymlinks(executable, YES)) {
            executable = nil;
        }
    } else {
        executable = MSIMELocalPolishServerDiscoverExecutable(_config.bundlePath, _config.environment);
    }
    if (!executable.length) {
        [self dispatchCallback:completion
                      endpoint:nil
                         error:MakeServerError(MSIMELocalPolishServerErrorBinaryNotFound,
                                               @"未找到本机 llama-server 可执行文件")];
        return;
    }

    const uint16_t port = _config.preferredPort != 0 ? _config.preferredPort : AllocateLoopbackPort();
    if (port == 0) {
        [self dispatchCallback:completion
                      endpoint:nil
                         error:MakeServerError(MSIMELocalPolishServerErrorPortAllocationFailed,
                                               @"无法为本机润色服务分配回环监听端口")];
        return;
    }

    NSString *apiKey = GenerateEphemeralApiKey();
    if (!apiKey.length) {
        [self dispatchCallback:completion
                      endpoint:nil
                         error:MakeServerError(MSIMELocalPolishServerErrorKeyGenerationFailed,
                                               @"无法生成本机润色服务的一次性访问密钥")];
        return;
    }

    const uint64_t memoryBytes = [self effectivePhysicalMemoryBytes];
    const NSUInteger ctxSize = [self activeContextSize];
    if (memoryBytes < kLowMemoryThresholdBytes && _config.onLowMemoryPreLaunch) {
        void (^lowMemoryHook)(uint64_t) = [_config.onLowMemoryPreLaunch copy];
        dispatch_async(_callbackQueue, ^{
            lowMemoryHook(memoryBytes);
        });
    }

    NSString *alias = TrimmedString(_config.modelAlias);
    if (!alias.length) alias = @"local";

    const uint64_t generation = ++_generation;
    MSIMELocalPolishServerEndpoint *endpoint = [[MSIMELocalPolishServerEndpoint alloc]
        initWithPort:port
              apiKey:apiKey
           modelPath:resolvedModel
          modelAlias:alias
          generation:generation];

    NSTask *task = [NSTask new];
    task.executableURL = [NSURL fileURLWithPath:executable];
    task.arguments = MSIMELocalPolishServerBuildLaunchArguments(port, resolvedModel, alias, ctxSize, apiKey);
    task.qualityOfService = NSQualityOfServiceUserInitiated;

    // 保持 stdin 管道打开，父进程异常退出时子进程可感知 EOF。
    NSPipe *stdinPipe = [NSPipe pipe];
    NSPipe *stderrPipe = [NSPipe pipe];
    task.standardInput = stdinPipe;
    task.standardOutput = NSFileHandle.fileHandleWithNullDevice;
    task.standardError = stderrPipe;

    [_stderrLines removeAllObjects];
    NSMutableData *stderrPending = [NSMutableData data];
    __weak MSIMELocalPolishServer *weakSelf = self;
    NSString *keyForSanitize = [apiKey copy];
    stderrPipe.fileHandleForReading.readabilityHandler = ^(NSFileHandle *handle) {
        NSData *chunk = handle.availableData;
        if (!chunk.length) {
            handle.readabilityHandler = nil;
            return;
        }
        NSMutableArray<NSString *> *capturedLines = [NSMutableArray array];
        @synchronized(stderrPending) {
            if (stderrPending.length + chunk.length > kMaxStderrPendingBytes) {
                stderrPending.length = 0;
            }
            [stderrPending appendData:chunk];
            for (;;) {
                if (!stderrPending.length) break;
                const char *bytes = static_cast<const char *>(stderrPending.bytes);
                const void *newline = std::memchr(bytes, '\n', stderrPending.length);
                if (!newline) break;
                const NSUInteger lineLength = static_cast<NSUInteger>(static_cast<const char *>(newline) - bytes);
                NSString *rawLine = [[NSString alloc] initWithBytes:bytes length:lineLength encoding:NSUTF8StringEncoding];
                if (rawLine.length) {
                    NSString *sanitized = SanitizeDiagnosticLine(rawLine, keyForSanitize);
                    if (sanitized.length) [capturedLines addObject:sanitized];
                }
                [stderrPending replaceBytesInRange:NSMakeRange(0, lineLength + 1) withBytes:nullptr length:0];
            }
        }
        if (capturedLines.count) {
            MSIMELocalPolishServer *strongSelf = weakSelf;
            if (!strongSelf) return;
            dispatch_async(strongSelf->_queue, ^{
                if (strongSelf->_generation != generation) return;
                for (NSString *line in capturedLines) {
                    if (strongSelf->_stderrLines.count >= kMaxStderrLines) {
                        [strongSelf->_stderrLines removeObjectAtIndex:0];
                    }
                    [strongSelf->_stderrLines addObject:line];
                }
            });
        }
    };

    task.terminationHandler = ^(NSTask *endedTask) {
        const int terminationStatus = endedTask.terminationStatus;
        MSIMELocalPolishServer *strongSelf = weakSelf;
        if (!strongSelf) return;
        dispatch_async(strongSelf->_queue, ^{
            [strongSelf handleProcessTerminatedLockedForGeneration:generation status:terminationStatus];
        });
    };

    NSError *launchError = nil;
    if (![task launchAndReturnError:&launchError]) {
        stderrPipe.fileHandleForReading.readabilityHandler = nil;
        _lastRunEndedUnexpectedly = YES;
        os_log_error(LocalPolishLog(), "启动本机 llama-server 失败（code=%ld）",
                     static_cast<long>(launchError.code));
        [self dispatchCallback:completion
                      endpoint:nil
                         error:MakeServerError(MSIMELocalPolishServerErrorLaunchFailed,
                                               @"无法启动本机 llama-server 进程")];
        return;
    }

    (void)::fcntl(stdinPipe.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1);
    _task = task;
    _stdinPipe = stdinPipe;
    _stderrPipe = stderrPipe;
    _startingEndpoint = endpoint;
    _runningEndpoint = nil;
    _state = ServerState::Starting;
    _lastRunEndedUnexpectedly = NO;
    if (completion) {
        [_pendingCallbacks addObject:completion];
    }

    os_log_info(LocalPolishLog(), "已启动本机 llama-server（pid=%d port=%u ctx=%lu gen=%llu）",
                task.processIdentifier,
                static_cast<unsigned>(port),
                static_cast<unsigned long>(ctxSize),
                static_cast<unsigned long long>(generation));

    const double deadline = MonotonicNowSeconds() + (_config.healthTimeout > 0.0 ? _config.healthTimeout : 15.0);
    [self pollHealthLockedForGeneration:generation endpoint:endpoint deadline:deadline];
}

- (void)pollHealthLockedForGeneration:(uint64_t)generation
                             endpoint:(MSIMELocalPolishServerEndpoint *)endpoint
                             deadline:(double)deadline {
    if (_generation != generation || _state != ServerState::Starting) return;
    if (!_task || !_task.running) {
        [self handleProcessTerminatedLockedForGeneration:generation status:_task ? _task.terminationStatus : -1];
        return;
    }
    if (MonotonicNowSeconds() >= deadline) {
        os_log_error(LocalPolishLog(), "本机 llama-server 健康检查超时（gen=%llu）",
                     static_cast<unsigned long long>(generation));
        [self stopLockedIntentional:NO
                        synchronous:NO
                       pendingError:MakeServerError(MSIMELocalPolishServerErrorHealthTimeout,
                                                    @"本机润色服务启动超时")];
        return;
    }

    NSURL *healthURL = [NSURL URLWithString:[endpoint.baseURL stringByAppendingString:@"/health"]];
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:healthURL];
    request.HTTPMethod = @"GET";
    request.cachePolicy = NSURLRequestReloadIgnoringLocalAndRemoteCacheData;
    [request setValue:[@"Bearer " stringByAppendingString:endpoint.apiKey] forHTTPHeaderField:@"Authorization"];

    __weak MSIMELocalPolishServer *weakSelf = self;
    NSURLSessionDataTask *dataTask = [_healthSession
        dataTaskWithRequest:request
          completionHandler:^(NSData * _Nullable data, NSURLResponse * _Nullable response, NSError * _Nullable error) {
              (void)data;
              MSIMELocalPolishServer *strongSelf = weakSelf;
              if (!strongSelf) return;
              NSInteger statusCode = 0;
              if ([response isKindOfClass:NSHTTPURLResponse.class]) {
                  statusCode = static_cast<NSHTTPURLResponse *>(response).statusCode;
              }
              dispatch_async(strongSelf->_queue, ^{
                  if (strongSelf->_generation != generation || strongSelf->_state != ServerState::Starting) return;
                  if (error == nil && statusCode == 200) {
                      strongSelf->_state = ServerState::Running;
                      strongSelf->_runningEndpoint = endpoint;
                      strongSelf->_startingEndpoint = nil;
                      strongSelf->_lastRunEndedUnexpectedly = NO;
                      [strongSelf armIdleTimerLocked];
                      os_log_info(LocalPolishLog(), "本机 llama-server 已就绪（port=%u gen=%llu）",
                                  static_cast<unsigned>(endpoint.port),
                                  static_cast<unsigned long long>(generation));
                      [strongSelf flushPendingCallbacksLockedWithEndpoint:endpoint error:nil];
                      return;
                  }

                  const double remaining = deadline - MonotonicNowSeconds();
                  if (remaining <= 0.0) {
                      os_log_error(LocalPolishLog(), "本机 llama-server 健康检查超时（status=%ld gen=%llu）",
                                   static_cast<long>(statusCode),
                                   static_cast<unsigned long long>(generation));
                      [strongSelf stopLockedIntentional:NO
                                            synchronous:NO
                                           pendingError:MakeServerError(MSIMELocalPolishServerErrorHealthTimeout,
                                                                        @"本机润色服务启动超时")];
                      return;
                  }

                  const NSTimeInterval interval = strongSelf->_config.healthPollInterval > 0.0
                                                      ? strongSelf->_config.healthPollInterval
                                                      : 0.10;
                  const NSTimeInterval delay = MIN(interval, remaining);
                  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(delay * static_cast<double>(NSEC_PER_SEC))),
                                 strongSelf->_queue, ^{
                                     [strongSelf pollHealthLockedForGeneration:generation endpoint:endpoint deadline:deadline];
                                 });
              });
          }];
    [dataTask resume];
}

- (void)handleProcessTerminatedLockedForGeneration:(uint64_t)generation status:(int)status {
    if (_generation != generation) return;
    const ServerState previousState = _state;
    [self cancelIdleTimerLocked];
    if (_stderrPipe) {
        _stderrPipe.fileHandleForReading.readabilityHandler = nil;
    }
    if (_stdinPipe) {
        [_stdinPipe.fileHandleForWriting closeAndReturnError:nil];
    }
    _task = nil;
    _stdinPipe = nil;
    _stderrPipe = nil;
    _startingEndpoint = nil;
    _runningEndpoint = nil;
    _state = ServerState::Idle;
    _lastRunEndedUnexpectedly = YES;

    os_log_error(LocalPolishLog(), "本机 llama-server 意外退出（status=%d gen=%llu）",
                 status, static_cast<unsigned long long>(generation));

    if (previousState == ServerState::Starting) {
        [self flushPendingCallbacksLockedWithEndpoint:nil
                                                error:MakeServerError(MSIMELocalPolishServerErrorProcessExited,
                                                                      @"本机润色服务启动期间意外退出")];
    }
}

- (void)stopLockedIntentional:(BOOL)intentional
                  synchronous:(BOOL)synchronous
                 pendingError:(nullable NSError *)pendingError {
    [self cancelIdleTimerLocked];

    NSTask *oldTask = _task;
    NSPipe *oldStdin = _stdinPipe;
    NSPipe *oldStderr = _stderrPipe;

    // 递增代次，令旧进程退出回调与在途健康轮询立即失效。
    ++_generation;
    _task = nil;
    _stdinPipe = nil;
    _stderrPipe = nil;
    _startingEndpoint = nil;
    _runningEndpoint = nil;
    _state = ServerState::Idle;
    _lastRunEndedUnexpectedly = !intentional;

    // 无论主动或异常离开 Starting，都必须冲刷所有等待回调，杜绝调用方永久挂起。
    if (_pendingCallbacks.count > 0) {
        NSError *flushError = pendingError;
        if (!flushError) {
            flushError = intentional
                             ? MakeServerError(MSIMELocalPolishServerErrorStopped, @"本机润色服务已停止")
                             : MakeServerError(MSIMELocalPolishServerErrorProcessExited, @"本机润色服务意外退出");
        }
        [self flushPendingCallbacksLockedWithEndpoint:nil error:flushError];
    }

    TerminateSubprocess(oldTask, oldStdin, oldStderr, _config.terminateGraceTimeout, synchronous);
}

@end
