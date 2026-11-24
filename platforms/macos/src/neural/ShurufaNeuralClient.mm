#import "ShurufaNeuralClient.h"

#import <AppKit/AppKit.h>
#import <os/lock.h>
#import <os/log.h>

#include "ShurufaNeuralProtocol.h"

#include <errno.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <string>

namespace {

using namespace shurufa::neural::ipc;

constexpr double kAvailabilityCacheSeconds = 5.0;
constexpr double kColdLoadTimeoutSeconds = 3.0;
constexpr double kBackoffBaseSeconds = 1.0;
constexpr double kBackoffMaxSeconds = 30.0;
constexpr double kHealthyUptimeResetSeconds = 60.0;

os_log_t NeuralClientLog(void) {
    static os_log_t log = nullptr;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        log = os_log_create("dev.shurufa.ime", "neural-client");
    });
    return log;
}

double NowSeconds(void) {
    return static_cast<double>(MonotonicNowNs()) / 1e9;
}

BOOL IsRegularFileFollowingSymlinks(NSString * _Nullable path, BOOL requireExecutable) {
    if (!path.length || !path.isAbsolutePath) return NO;
    const char *fsPath = path.fileSystemRepresentation;
    if (!fsPath || !*fsPath) return NO;
    struct stat st{};
    if (::stat(fsPath, &st) != 0) return NO;
    if (!S_ISREG(st.st_mode)) return NO;
    return ::access(fsPath, requireExecutable ? X_OK : R_OK) == 0;
}

BOOL DirectoryHasGGUFModel(NSString * _Nullable directory) {
    if (!directory.length || !directory.isAbsolutePath) return NO;
    const char *dirFs = directory.fileSystemRepresentation;
    if (!dirFs || !*dirFs) return NO;
    struct stat st{};
    if (::stat(dirFs, &st) != 0 || !S_ISDIR(st.st_mode)) return NO;
    NSArray<NSString *> *entries = [NSFileManager.defaultManager contentsOfDirectoryAtPath:directory error:nil];
    for (NSString *name in entries) {
        if (![name.pathExtension.lowercaseString isEqualToString:@"gguf"]) continue;
        if ([name.lowercaseString containsString:@"mmproj"]) continue;
        NSString *full = [directory stringByAppendingPathComponent:name];
        if (IsRegularFileFollowingSymlinks(full, NO)) return YES;
    }
    return NO;
}

NSString * _Nullable DiscoverNeuralServiceBinary(NSString * _Nullable configuredPath) {
    if (configuredPath.length) {
        NSString *expanded = configuredPath.stringByExpandingTildeInPath;
        return IsRegularFileFollowingSymlinks(expanded, YES) ? expanded : nil;
    }
    NSDictionary<NSString *, NSString *> *env = NSProcessInfo.processInfo.environment;
    NSString *overridePath = env[@"SHURUFA_NEURAL_SERVICE_BIN"];
    if (overridePath.length) {
        NSString *expanded = overridePath.stringByExpandingTildeInPath;
        if (IsRegularFileFollowingSymlinks(expanded, YES)) return expanded;
    }
    NSString *bundlePath = NSBundle.mainBundle.bundlePath;
    if (bundlePath.length) {
        NSString *bundled = [bundlePath stringByAppendingPathComponent:@"Contents/Helpers/ShurufaNeuralService"];
        if (IsRegularFileFollowingSymlinks(bundled, YES)) return bundled;
    }
    return nil;
}

NSString *DefaultNeuralModelDirectory(NSString * _Nullable configuredDir) {
    if (configuredDir.length) return configuredDir.stringByExpandingTildeInPath;
    NSDictionary<NSString *, NSString *> *env = NSProcessInfo.processInfo.environment;
    NSString *overrideDir = env[@"SHURUFA_NEURAL_MODEL_DIR"];
    if (overrideDir.length) return overrideDir.stringByExpandingTildeInPath;
    return [@"~/Library/Application Support/Shurufa/resource-packs/neural-lm" stringByExpandingTildeInPath];
}

NSString *AllocatePrivateSocketPath(NSString * _Nullable configuredPath, uint64_t instanceTag) {
    if (configuredPath.length) return configuredPath;
    NSString *fileName = [NSString stringWithFormat:@"shurufa-n-%d-%llu.sock",
                          static_cast<int>(::getpid()),
                          static_cast<unsigned long long>(instanceTag)];
    NSString *tempDir = NSTemporaryDirectory();
    if (tempDir.length) {
        NSString *candidate = [tempDir stringByAppendingPathComponent:fileName];
        if (std::strlen(candidate.fileSystemRepresentation) < sizeof(sockaddr_un::sun_path) - 2) {
            return candidate;
        }
    }
    return [NSString stringWithFormat:@"/tmp/shurufa-n-%u-%d-%llu.sock",
            static_cast<unsigned>(::geteuid()),
            static_cast<int>(::getpid()),
            static_cast<unsigned long long>(instanceTag)];
}

int ConnectUnixSocket(NSString *socketPath) {
    if (!socketPath.length) return -1;
    const char *path = socketPath.fileSystemRepresentation;
    if (!path || std::strlen(path) >= sizeof(sockaddr_un::sun_path)) return -1;
    const int fd = ::socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    ConfigureSocketNoSigPipe(fd);
    struct sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    std::memcpy(addr.sun_path, path, std::strlen(path) + 1);
    if (::connect(fd, reinterpret_cast<struct sockaddr *>(&addr), sizeof(addr)) != 0) {
        ::close(fd);
        return -1;
    }
    return fd;
}

} // namespace

@implementation ShurufaNeuralClientConfiguration

- (instancetype)init {
    if (!(self = [super init])) return nil;
    _idleUnloadSeconds = 600.0;
    _requestTimeoutSeconds = 0.25;
    _requireModelFile = YES;
    _listenToApplicationTermination = YES;
    return self;
}

- (id)copyWithZone:(nullable NSZone *)zone {
    ShurufaNeuralClientConfiguration *copy = [[[self class] allocWithZone:zone] init];
    copy.helperPath = self.helperPath;
    copy.modelDirectory = self.modelDirectory;
    copy.pinyinDBPath = self.pinyinDBPath;
    copy.socketPath = self.socketPath;
    copy.extraLaunchArguments = self.extraLaunchArguments;
    copy.idleUnloadSeconds = self.idleUnloadSeconds;
    copy.requestTimeoutSeconds = self.requestTimeoutSeconds;
    copy.requireModelFile = self.requireModelFile;
    copy.listenToApplicationTermination = self.listenToApplicationTermination;
    return copy;
}

@end

@implementation ShurufaNeuralClient {
    ShurufaNeuralClientConfiguration *_config;
    dispatch_queue_t _ioQueue;
    os_unfair_lock _lock;

    // 仅在 `_lock` 保护下访问的轻量待派发槽位（主线程入队纳秒级完成）。
    NSData *_pendingDecodeData;
    uint64_t _pendingDecodeGeneration;
    uint64_t _pendingDecodeEnqueuedNs;
    void (^_pendingDecodeCompletion)(NSData * _Nullable, uint64_t, double);
    uint64_t _latestDecodeGeneration;

    NSString *_pendingPrefillContext;
    BOOL _hasPendingPrefill;
    BOOL _workerScheduled;
    BOOL _stopped;

    NSString *_resourcesDirectory;
    BOOL _cachedAvailable;
    double _cachedAvailableAt;

    // 以下状态仅在串行 `_ioQueue` 上访问，无锁竞争。
    NSTask *_task;
    NSString *_socketPath;
    int _socketFd;
    uint32_t _nextRequestId;
    BOOL _serviceWarmed;
    double _serviceStartedAt;
    NSUInteger _consecutiveFailures;
    double _backoffUntil;
    std::atomic<pid_t> _atomicPID;
    id _terminateObserver;
}

+ (instancetype)sharedClient {
    static ShurufaNeuralClient *shared = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        shared = [[ShurufaNeuralClient alloc] initWithConfiguration:nil];
    });
    return shared;
}

- (instancetype)init {
    return [self initWithConfiguration:nil];
}

- (instancetype)initWithConfiguration:(nullable ShurufaNeuralClientConfiguration *)configuration {
    if (!(self = [super init])) return nil;
    static std::atomic<uint64_t> gInstanceCounter{0};
    const uint64_t instanceTag = ++gInstanceCounter;

    _config = configuration ? [configuration copy] : [[ShurufaNeuralClientConfiguration alloc] init];
    _lock = OS_UNFAIR_LOCK_INIT;
    _socketFd = -1;
    _socketPath = AllocatePrivateSocketPath(_config.socketPath, instanceTag);
    _atomicPID.store(0, std::memory_order_relaxed);

    dispatch_queue_attr_t attr = dispatch_queue_attr_make_with_qos_class(
        DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0);
    _ioQueue = dispatch_queue_create("dev.shurufa.neural-client.io", attr);

    if (_config.listenToApplicationTermination) {
        __weak ShurufaNeuralClient *weakSelf = self;
        _terminateObserver = [[NSNotificationCenter defaultCenter]
            addObserverForName:NSApplicationWillTerminateNotification
                        object:nil
                         queue:nil
                    usingBlock:^(NSNotification *note) {
                        (void)note;
                        [weakSelf stop];
                    }];
    }
    return self;
}

- (void)dealloc {
    if (_terminateObserver) {
        [[NSNotificationCenter defaultCenter] removeObserver:_terminateObserver];
        _terminateObserver = nil;
    }
    if (_socketFd >= 0) {
        ::close(_socketFd);
        _socketFd = -1;
    }
    if (_task && _task.running) {
        const pid_t pid = _task.processIdentifier;
        [_task terminate];
        if (pid > 0) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(500ULL * NSEC_PER_MSEC)),
                           dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
                               if (::kill(pid, 0) == 0) {
                                   ::kill(pid, SIGKILL);
                               }
                           });
        }
    }
    if (_socketPath.length) {
        ::unlink(_socketPath.fileSystemRepresentation);
    }
}

- (pid_t)servicePID {
    return _atomicPID.load(std::memory_order_relaxed);
}

- (void)configureResourcesDirectory:(nullable NSString *)resourcesDirectory {
    os_unfair_lock_lock(&_lock);
    _resourcesDirectory = resourcesDirectory.length ? [resourcesDirectory copy] : nil;
    os_unfair_lock_unlock(&_lock);
}

- (BOOL)isAvailable {
    NSDictionary<NSString *, NSString *> *env = NSProcessInfo.processInfo.environment;
    if ([env[@"SHURUFA_DISABLE_NEURAL_V2"] isEqualToString:@"1"]) return NO;

    const double now = NowSeconds();
    os_unfair_lock_lock(&_lock);
    if (_stopped) {
        os_unfair_lock_unlock(&_lock);
        return NO;
    }
    if (_cachedAvailableAt > 0.0 && (now - _cachedAvailableAt) < kAvailabilityCacheSeconds) {
        const BOOL cached = _cachedAvailable;
        os_unfair_lock_unlock(&_lock);
        return cached;
    }
    os_unfair_lock_unlock(&_lock);

    NSString *helper = DiscoverNeuralServiceBinary(_config.helperPath);
    BOOL ready = helper.length > 0;
    if (ready && _config.requireModelFile) {
        ready = DirectoryHasGGUFModel(DefaultNeuralModelDirectory(_config.modelDirectory));
    }

    os_unfair_lock_lock(&_lock);
    _cachedAvailable = ready;
    _cachedAvailableAt = now;
    os_unfair_lock_unlock(&_lock);
    return ready;
}

- (void)prefillContext:(NSString *)context {
    if (![context isKindOfClass:NSString.class]) return;
    if (!self.isAvailable) return;

    BOOL shouldSchedule = NO;
    os_unfair_lock_lock(&_lock);
    if (!_stopped) {
        _pendingPrefillContext = [context copy];
        _hasPendingPrefill = YES;
        if (!_workerScheduled) {
            _workerScheduled = YES;
            shouldSchedule = YES;
        }
    }
    os_unfair_lock_unlock(&_lock);

    if (shouldSchedule) {
        __weak ShurufaNeuralClient *weakSelf = self;
        dispatch_async(_ioQueue, ^{
            [weakSelf drainWorkerQueue];
        });
    }
}

- (void)decodeRequestData:(NSData *)requestData
               generation:(uint64_t)generation
               completion:(void (^)(NSData * _Nullable, uint64_t, double))completion {
    if (!requestData.length || !completion) return;
    if (!self.isAvailable) {
        dispatch_async(dispatch_get_main_queue(), ^{
            completion(nil, generation, 0.0);
        });
        return;
    }

    void (^supersededCompletion)(NSData * _Nullable, uint64_t, double) = nil;
    uint64_t supersededGen = 0;
    BOOL shouldSchedule = NO;
    const uint64_t nowNs = MonotonicNowNs();

    os_unfair_lock_lock(&_lock);
    if (_stopped) {
        os_unfair_lock_unlock(&_lock);
        dispatch_async(dispatch_get_main_queue(), ^{
            completion(nil, generation, 0.0);
        });
        return;
    }
    if (_pendingDecodeCompletion) {
        supersededCompletion = _pendingDecodeCompletion;
        supersededGen = _pendingDecodeGeneration;
    }
    _pendingDecodeData = [requestData copy];
    _pendingDecodeGeneration = generation;
    _pendingDecodeEnqueuedNs = nowNs;
    _pendingDecodeCompletion = [completion copy];
    if (generation > _latestDecodeGeneration) {
        _latestDecodeGeneration = generation;
    }
    if (!_workerScheduled) {
        _workerScheduled = YES;
        shouldSchedule = YES;
    }
    os_unfair_lock_unlock(&_lock);

    if (supersededCompletion) {
        dispatch_async(dispatch_get_main_queue(), ^{
            supersededCompletion(nil, supersededGen, 0.0);
        });
    }
    if (shouldSchedule) {
        __weak ShurufaNeuralClient *weakSelf = self;
        dispatch_async(_ioQueue, ^{
            [weakSelf drainWorkerQueue];
        });
    }
}

- (void)cancelPendingDecodeRequests {
    void (^cancelledCompletion)(NSData * _Nullable, uint64_t, double) = nil;
    uint64_t cancelledGen = 0;

    os_unfair_lock_lock(&_lock);
    if (_pendingDecodeCompletion) {
        cancelledCompletion = _pendingDecodeCompletion;
        cancelledGen = _pendingDecodeGeneration;
    }
    _pendingDecodeData = nil;
    _pendingDecodeCompletion = nil;
    _pendingDecodeGeneration = 0;
    ++_latestDecodeGeneration;
    os_unfair_lock_unlock(&_lock);

    if (cancelledCompletion) {
        dispatch_async(dispatch_get_main_queue(), ^{
            cancelledCompletion(nil, cancelledGen, 0.0);
        });
    }
}

- (void)checkHealthWithCompletion:(void (^)(NSDictionary<NSString *, id> * _Nullable))completion {
    if (!completion) return;
    __weak ShurufaNeuralClient *weakSelf = self;
    dispatch_async(_ioQueue, ^{
        ShurufaNeuralClient *strongSelf = weakSelf;
        if (!strongSelf) return;
        NSDictionary<NSString *, id> *info = nil;
        if ([strongSelf ensureConnectedOnIOQueue]) {
            FrameHeader respHeader{};
            std::string respPayload;
            const uint32_t reqId = ++strongSelf->_nextRequestId;
            if (WriteFrame(strongSelf->_socketFd, OpCode::Health, 0U, reqId, nullptr, 0U, 1000) &&
                ReadFrame(strongSelf->_socketFd, respHeader, respPayload, 1000) &&
                respHeader.request_id == reqId && !respPayload.empty()) {
                NSData *data = [NSData dataWithBytes:respPayload.data() length:respPayload.size()];
                id parsed = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
                if ([parsed isKindOfClass:NSDictionary.class]) {
                    info = parsed;
                }
            } else {
                [strongSelf handleTransportFailureOnIOQueue];
            }
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            completion(info);
        });
    });
}

- (void)stop {
    void (^cancelledCompletion)(NSData * _Nullable, uint64_t, double) = nil;
    uint64_t cancelledGen = 0;

    os_unfair_lock_lock(&_lock);
    _stopped = YES;
    if (_pendingDecodeCompletion) {
        cancelledCompletion = _pendingDecodeCompletion;
        cancelledGen = _pendingDecodeGeneration;
    }
    _pendingDecodeData = nil;
    _pendingDecodeCompletion = nil;
    _pendingPrefillContext = nil;
    _hasPendingPrefill = NO;
    os_unfair_lock_unlock(&_lock);

    if (cancelledCompletion) {
        dispatch_async(dispatch_get_main_queue(), ^{
            cancelledCompletion(nil, cancelledGen, 0.0);
        });
    }

    // 同步停止后台子进程，确保进程退出或测试收尾时不残留孤儿服务进程。
    dispatch_sync(_ioQueue, ^{
        [self tearDownServiceOnIOQueue];
    });
}

#pragma mark - 内部串行 IO 队列方法

- (void)closeSocketOnIOQueue {
    if (_socketFd >= 0) {
        ::close(_socketFd);
        _socketFd = -1;
    }
}

- (void)tearDownServiceOnIOQueue {
    if (_socketFd >= 0) {
        const uint32_t reqId = ++_nextRequestId;
        (void)WriteFrame(_socketFd, OpCode::Shutdown, 0U, reqId, nullptr, 0U, 200);
        [self closeSocketOnIOQueue];
    }
    if (_task) {
        const pid_t pid = _task.processIdentifier;
        if (_task.running) {
            [_task terminate];
            const double deadline = NowSeconds() + 0.5;
            while (_task.running && NowSeconds() < deadline) {
                usleep(10 * 1000);
            }
            if (_task.running && pid > 0) {
                ::kill(pid, SIGKILL);
            }
        }
        _task = nil;
    }
    _atomicPID.store(0, std::memory_order_relaxed);
    _serviceWarmed = NO;
    if (_socketPath.length) {
        ::unlink(_socketPath.fileSystemRepresentation);
    }
}

- (void)recordFailureAndBackoffOnIOQueue {
    const double now = NowSeconds();
    if (_serviceStartedAt > 0.0 && (now - _serviceStartedAt) >= kHealthyUptimeResetSeconds) {
        _consecutiveFailures = 0;
    }
    ++_consecutiveFailures;
    const double delay = std::min(kBackoffMaxSeconds,
                                  kBackoffBaseSeconds * std::pow(2.0, static_cast<double>(_consecutiveFailures - 1)));
    _backoffUntil = now + delay;
    os_log_error(NeuralClientLog(), "neural_service_backoff failures=%lu delay_sec=%.1f",
                 static_cast<unsigned long>(_consecutiveFailures), delay);
}

- (void)handleTransportFailureOnIOQueue {
    [self closeSocketOnIOQueue];
    if (_task && !_task.running) {
        os_log_error(NeuralClientLog(), "neural_service_crashed status=%d", _task.terminationStatus);
        _task = nil;
        _atomicPID.store(0, std::memory_order_relaxed);
        _serviceWarmed = NO;
        [self recordFailureAndBackoffOnIOQueue];
    }
}

- (NSString * _Nullable)resolvedPinyinDBPathOnIOQueue {
    if (_config.pinyinDBPath.length && IsRegularFileFollowingSymlinks(_config.pinyinDBPath, NO)) {
        return _config.pinyinDBPath;
    }
    NSString *resDir = nil;
    os_unfair_lock_lock(&_lock);
    resDir = _resourcesDirectory;
    os_unfair_lock_unlock(&_lock);
    if (resDir.length) {
        NSString *candidate = [resDir stringByAppendingPathComponent:@"msime-pinyin.db"];
        if (IsRegularFileFollowingSymlinks(candidate, NO)) return candidate;
    }
    NSString *bundleRes = NSBundle.mainBundle.resourcePath;
    if (bundleRes.length) {
        NSString *bundled = [bundleRes stringByAppendingPathComponent:@"EngineResources/msime-pinyin.db"];
        if (IsRegularFileFollowingSymlinks(bundled, NO)) return bundled;
    }
    return nil;
}

- (BOOL)ensureConnectedOnIOQueue {
    os_unfair_lock_lock(&_lock);
    const BOOL stopped = _stopped;
    os_unfair_lock_unlock(&_lock);
    if (stopped) return NO;

    if (_socketFd >= 0 && _task && _task.running) {
        return YES;
    }
    [self closeSocketOnIOQueue];

    if (_task && !_task.running) {
        _task = nil;
        _atomicPID.store(0, std::memory_order_relaxed);
        _serviceWarmed = NO;
        [self recordFailureAndBackoffOnIOQueue];
    }

    const double now = NowSeconds();
    if (_backoffUntil > 0.0 && now < _backoffUntil) {
        return NO;
    }

    if (!_task) {
        NSString *helper = DiscoverNeuralServiceBinary(_config.helperPath);
        if (!helper.length) return NO;
        if (_config.requireModelFile && !DirectoryHasGGUFModel(DefaultNeuralModelDirectory(_config.modelDirectory))) {
            return NO;
        }
        ::unlink(_socketPath.fileSystemRepresentation);

        NSMutableArray<NSString *> *args = [NSMutableArray arrayWithArray:@[
            @"--socket", _socketPath,
            @"--model-dir", DefaultNeuralModelDirectory(_config.modelDirectory),
            @"--idle-seconds", [NSString stringWithFormat:@"%.1f", _config.idleUnloadSeconds > 0.0 ? _config.idleUnloadSeconds : 600.0],
            @"--parent-pid", [NSString stringWithFormat:@"%d", static_cast<int>(::getpid())],
        ]];
        NSString *pinyinDB = [self resolvedPinyinDBPathOnIOQueue];
        if (pinyinDB.length) {
            [args addObjectsFromArray:@[@"--pinyin-db", pinyinDB]];
        }
        if (_config.extraLaunchArguments.count) {
            [args addObjectsFromArray:_config.extraLaunchArguments];
        }

        NSTask *task = [[NSTask alloc] init];
        task.executableURL = [NSURL fileURLWithPath:helper];
        task.arguments = args;
        task.standardInput = [NSFileHandle fileHandleWithNullDevice];
        task.standardOutput = [NSFileHandle fileHandleWithNullDevice];
        task.standardError = [NSFileHandle fileHandleWithNullDevice];
        NSError *launchError = nil;
        if (![task launchAndReturnError:&launchError]) {
            os_log_error(NeuralClientLog(), "neural_service_launch_failed code=%ld",
                         static_cast<long>(launchError.code));
            [self recordFailureAndBackoffOnIOQueue];
            return NO;
        }
        _task = task;
        _serviceStartedAt = now;
        _serviceWarmed = NO;
        _atomicPID.store(task.processIdentifier, std::memory_order_relaxed);
        os_log_info(NeuralClientLog(), "neural_service_spawned pid=%d", task.processIdentifier);
    }

    // 轮询等待 Unix 域套接字监听就绪（服务绑定只需 ~2–5 ms，最多等 1.5 s）。
    const double connectDeadline = NowSeconds() + 1.5;
    while (NowSeconds() < connectDeadline) {
        if (!_task || !_task.running) {
            [self handleTransportFailureOnIOQueue];
            return NO;
        }
        const int fd = ConnectUnixSocket(_socketPath);
        if (fd >= 0) {
            _socketFd = fd;
            return YES;
        }
        usleep(5 * 1000);
    }
    os_log_error(NeuralClientLog(), "neural_service_connect_timeout");
    [self tearDownServiceOnIOQueue];
    [self recordFailureAndBackoffOnIOQueue];
    return NO;
}

- (void)drainWorkerQueue {
    for (;;) {
        @autoreleasepool {
            NSData *decodeData = nil;
            uint64_t decodeGen = 0;
            uint64_t enqueuedNs = 0;
            void (^decodeCompletion)(NSData * _Nullable, uint64_t, double) = nil;
            NSString *prefillContext = nil;
            BOOL doPrefill = NO;

            os_unfair_lock_lock(&_lock);
            if (_stopped) {
                _workerScheduled = NO;
                os_unfair_lock_unlock(&_lock);
                return;
            }
            // 解码任务优先于空闲预热，确保用户输入低延迟响应。
            if (_pendingDecodeData && _pendingDecodeCompletion) {
                decodeData = _pendingDecodeData;
                decodeGen = _pendingDecodeGeneration;
                enqueuedNs = _pendingDecodeEnqueuedNs;
                decodeCompletion = _pendingDecodeCompletion;
                _pendingDecodeData = nil;
                _pendingDecodeCompletion = nil;
                _pendingDecodeGeneration = 0;
            } else if (_hasPendingPrefill) {
                prefillContext = _pendingPrefillContext ?: @"";
                _pendingPrefillContext = nil;
                _hasPendingPrefill = NO;
                doPrefill = YES;
            } else {
                _workerScheduled = NO;
                os_unfair_lock_unlock(&_lock);
                return;
            }
            os_unfair_lock_unlock(&_lock);

            if (decodeCompletion) {
                NSData *responseData = nil;
                if ([self ensureConnectedOnIOQueue]) {
                    const double timeoutSec = _serviceWarmed
                        ? (_config.requestTimeoutSeconds > 0.0 ? _config.requestTimeoutSeconds : 0.25)
                        : std::max(kColdLoadTimeoutSeconds, _config.requestTimeoutSeconds);
                    const int timeoutMs = static_cast<int>(timeoutSec * 1000.0);
                    const uint32_t reqId = ++_nextRequestId;
                    FrameHeader respHeader{};
                    std::string respPayload;
                    const bool ok = WriteFrame(_socketFd, OpCode::Decode, 0U, reqId,
                                               decodeData.bytes, decodeData.length, timeoutMs) &&
                                    ReadFrame(_socketFd, respHeader, respPayload, timeoutMs) &&
                                    respHeader.request_id == reqId;
                    if (ok) {
                        _serviceWarmed = YES;
                        if ((respHeader.flags & kFlagOk) != 0 && !respPayload.empty()) {
                            responseData = [NSData dataWithBytes:respPayload.data() length:respPayload.size()];
                        }
                    } else {
                        [self handleTransportFailureOnIOQueue];
                    }
                }
                const double elapsedMs = static_cast<double>(MonotonicNowNs() - enqueuedNs) / 1e6;
                dispatch_async(dispatch_get_main_queue(), ^{
                    decodeCompletion(responseData, decodeGen, elapsedMs);
                });
                continue;
            }

            if (doPrefill) {
                if ([self ensureConnectedOnIOQueue]) {
                    NSData *utf8 = [prefillContext dataUsingEncoding:NSUTF8StringEncoding] ?: [NSData data];
                    const int timeoutMs = _serviceWarmed ? 1000 : static_cast<int>(kColdLoadTimeoutSeconds * 1000.0);
                    const uint32_t reqId = ++_nextRequestId;
                    FrameHeader respHeader{};
                    std::string respPayload;
                    const bool ok = WriteFrame(_socketFd, OpCode::Prefill, 0U, reqId,
                                               utf8.bytes, utf8.length, timeoutMs) &&
                                    ReadFrame(_socketFd, respHeader, respPayload, timeoutMs) &&
                                    respHeader.request_id == reqId;
                    if (ok) {
                        _serviceWarmed = YES;
                    } else {
                        [self handleTransportFailureOnIOQueue];
                    }
                }
            }
        }
    }
}

@end
