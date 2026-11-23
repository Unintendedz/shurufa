#pragma once

#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSErrorDomain const MSIMELocalPolishServerErrorDomain;

typedef NS_ERROR_ENUM(MSIMELocalPolishServerErrorDomain, MSIMELocalPolishServerErrorCode) {
    MSIMELocalPolishServerErrorBinaryNotFound = 1,
    MSIMELocalPolishServerErrorModelNotFound = 2,
    MSIMELocalPolishServerErrorPortAllocationFailed = 3,
    MSIMELocalPolishServerErrorKeyGenerationFailed = 4,
    MSIMELocalPolishServerErrorLaunchFailed = 5,
    MSIMELocalPolishServerErrorHealthTimeout = 6,
    MSIMELocalPolishServerErrorProcessExited = 7,
    MSIMELocalPolishServerErrorRateLimited = 8,
    MSIMELocalPolishServerErrorStopped = 9,
};

#ifdef __cplusplus
extern "C" {
#endif

/// 按约定优先级发现本机 `llama-server` 可执行文件（遵循软链接）：
/// `SHURUFA_LLAMA_SERVER_BIN` -> `<bundle>/Contents/Helpers/llama-server` ->
/// `/opt/homebrew/bin/llama-server` -> `/usr/local/bin/llama-server`。
FOUNDATION_EXPORT NSString * _Nullable MSIMELocalPolishServerDiscoverExecutable(
    NSString * _Nullable bundlePath,
    NSDictionary<NSString *, NSString *> * _Nullable environment);

/// 返回默认本机 GGUF 存放目录（优先环境变量 `SHURUFA_LOCAL_LLM_DIR`，
/// 否则为 `~/Library/Application Support/Shurufa/local-llm`）。
FOUNDATION_EXPORT NSString *MSIMELocalPolishServerDefaultModelDirectory(
    NSDictionary<NSString *, NSString *> * _Nullable environment);

/// 按优先级发现本机 GGUF 模型路径（支持软链接，跳过 `mmproj` 视觉投影文件，
/// 同时存在通用聊天模型与 `hy-mt` 翻译模型时优先选择通用模型）。
FOUNDATION_EXPORT NSString * _Nullable MSIMELocalPolishServerDiscoverModelPath(
    NSString * _Nullable explicitModelPath,
    NSString * _Nullable localLLMDirectory);

/// 按物理内存分级返回空闲自动回收时间（秒）：
/// `>= 48 GiB`: 1800s，`24–47 GiB`: 600s，`< 24 GiB`: 180s。
FOUNDATION_EXPORT NSTimeInterval MSIMELocalPolishServerIdleTimeoutForMemoryBytes(uint64_t physicalMemoryBytes);

/// 按物理内存分级返回 `--ctx-size` 上下文窗口大小：
/// `>= 24 GiB`: 4096，`< 24 GiB`: 2048。
FOUNDATION_EXPORT NSUInteger MSIMELocalPolishServerContextSizeForMemoryBytes(uint64_t physicalMemoryBytes);

/// 构造严格仅绑定 `127.0.0.1` 的 `llama-server` 安全启动参数列表（包含 `--api-key`，严禁直接写入日志）。
FOUNDATION_EXPORT NSArray<NSString *> *MSIMELocalPolishServerBuildLaunchArguments(
    uint16_t port,
    NSString *modelPath,
    NSString *modelAlias,
    NSUInteger contextSize,
    NSString *apiKey);

#ifdef __cplusplus
}
#endif

/// `MSIMELocalPolishServer` 的可测试配置对象；生产环境使用默认配置即可。
@interface MSIMELocalPolishServerConfiguration : NSObject <NSCopying>
/// 显式覆盖可执行文件路径（为空时走 `MSIMELocalPolishServerDiscoverExecutable`）。
@property(nonatomic, copy, nullable) NSString *executablePath;
/// 显式覆盖本机模型目录（为 `nil` 时走 `MSIMELocalPolishServerDefaultModelDirectory`）。
@property(nonatomic, copy, nullable) NSString *modelDirectory;
/// 显式覆盖应用包路径（用于测试 `Contents/Helpers/llama-server` 发现逻辑）。
@property(nonatomic, copy, nullable) NSString *bundlePath;
/// 显式覆盖环境变量（为 `nil` 时读取当前进程环境变量）。
@property(nonatomic, copy, nullable) NSDictionary<NSString *, NSString *> *environment;
/// 暴露给 OpenAI 兼容接口的模型别名，默认 `@"local"`。
@property(nonatomic, copy) NSString *modelAlias;
/// 优先监听端口；为 `0` 时自动分配本机空闲端口。
@property(nonatomic, assign) uint16_t preferredPort;
/// 覆盖物理内存字节数用于分级策略测试；为 `0` 时读取系统实际物理内存。
@property(nonatomic, assign) uint64_t physicalMemoryBytesOverride;
/// 覆盖空闲回收秒数；`<= 0` 时按物理内存自动计算。
@property(nonatomic, assign) NSTimeInterval idleTimeoutOverride;
/// 覆盖 `--ctx-size`；为 `0` 时按物理内存自动计算。
@property(nonatomic, assign) NSUInteger contextSizeOverride;
/// `/health` 启动就绪等待超时（秒），默认 `15.0`。
@property(nonatomic, assign) NSTimeInterval healthTimeout;
/// `/health` 轮询间隔（秒），默认 `0.10`。
@property(nonatomic, assign) NSTimeInterval healthPollInterval;
/// 停止子进程时 `SIGTERM` 到 `SIGKILL` 的宽限时间（秒），默认 `2.0`。
@property(nonatomic, assign) NSTimeInterval terminateGraceTimeout;
/// 异常重启限频窗口（秒），默认 `60.0`。
@property(nonatomic, assign) NSTimeInterval restartRateWindow;
/// 单个限频窗口内允许的最大异常重启次数，默认 `2`。
@property(nonatomic, assign) NSUInteger maxRestartsInWindow;
/// 是否监听系统 `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`（Warning/Critical），默认 `YES`。
@property(nonatomic, assign) BOOL listenToSystemMemoryPressure;
/// 是否监听 `NSApplicationWillTerminateNotification` 在退出时停止子进程，默认 `YES`。
@property(nonatomic, assign) BOOL listenToApplicationTermination;
/// 低内存机器（`< 24 GiB`）启动前回调，用于通知 `textstream` 评估内存余量。
@property(nonatomic, copy, nullable) void (^onLowMemoryPreLaunch)(uint64_t physicalMemoryBytes);
@end

/// 当前已就绪的本机 `llama-server` 端点快照（凭据仅存内存，`-description` 自动脱敏）。
@interface MSIMELocalPolishServerEndpoint : NSObject
@property(nonatomic, copy, readonly) NSString *baseURL;
@property(nonatomic, copy, readonly) NSString *chatCompletionsURL;
@property(nonatomic, copy, readonly) NSString *apiKey;
@property(nonatomic, copy, readonly) NSString *modelPath;
@property(nonatomic, copy, readonly) NSString *modelAlias;
@property(nonatomic, assign, readonly) uint16_t port;
@property(nonatomic, assign, readonly) uint64_t generation;
@end

/// 管理本机 `llama-server` 子进程生命周期的守护控制器。
/// 所有进程状态变更与启动合并均在串行队列内完成，健康轮询全异步，绝不阻塞主线程。
@interface MSIMELocalPolishServer : NSObject

/// 生产环境共享单例。
+ (instancetype)sharedServer;

/// 使用自定义配置创建独立实例（适合隔离测试）。
- (instancetype)initWithConfiguration:(nullable MSIMELocalPolishServerConfiguration *)configuration NS_DESIGNATED_INITIALIZER;

/// 当前是否处于就绪运行态。
@property(nonatomic, readonly) BOOL isRunning;
/// 当前子进程 PID（未运行时为 `0`）。
@property(nonatomic, readonly) pid_t processIdentifier;
/// 当前监听的本机回环端口（未运行时为 `0`）。
@property(nonatomic, readonly) uint16_t listeningPort;
/// 当前生效的空闲自动回收时长（秒）。
@property(nonatomic, readonly) NSTimeInterval activeIdleTimeout;
/// 当前生效的 `--ctx-size`。
@property(nonatomic, readonly) NSUInteger activeContextSize;
/// 当前限频窗口内已消耗的异常重启次数。
@property(nonatomic, readonly) NSUInteger recentRestartCount;
/// 最近保留的有界脱敏 stderr 诊断行（最多 32 行）。
@property(nonatomic, copy, readonly) NSArray<NSString *> *recentSanitizedStderrLines;

/// 确保本机 `llama-server` 已启动并通过 `/health` 就绪；并发调用自动合并为一次启动。
/// `completion` 始终在后台队列异步触发，不阻塞主线程。
- (void)ensureReadyWithModelPath:(nullable NSString *)modelPath
                      completion:(void (^)(MSIMELocalPolishServerEndpoint * _Nullable endpoint,
                                           NSError * _Nullable error))completion;

/// 录音开始时在后台轻量预热或保活本机模型，避免松手后出现冷启动开销。
- (void)prewarmWithModelPath:(nullable NSString *)modelPath;

/// 刷新活跃时间戳，推迟空闲自动回收。
- (void)touchActivity;

/// 当润色请求命中回环连接被拒或中断时触发一次受限频保护的重启。
- (void)handleConnectionFailureForEndpoint:(MSIMELocalPolishServerEndpoint *)failedEndpoint
                                completion:(void (^)(MSIMELocalPolishServerEndpoint * _Nullable endpoint,
                                                     NSError * _Nullable error))completion;

/// 响应内存压力信号，立即优雅停止子进程以释放显存与内存（不计入异常崩溃限频）。
- (void)handleMemoryPressure;

/// 异步优雅停止子进程（`SIGTERM` 后按宽限期兜底 `SIGKILL`）。
- (void)stop;

/// 同步优雅停止子进程并等待退出（用于应用退出钩子与测试收尾）。
- (void)stopSynchronously;

@end

NS_ASSUME_NONNULL_END
