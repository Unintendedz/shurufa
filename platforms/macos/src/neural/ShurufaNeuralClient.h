#pragma once

#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// `ShurufaNeuralClient` 的可测试配置对象；生产环境使用 `[ShurufaNeuralClient sharedClient]` 默认配置即可。
@interface ShurufaNeuralClientConfiguration : NSObject <NSCopying>
/// 显式覆盖后台服务可执行路径；为 nil 时按 `SHURUFA_NEURAL_SERVICE_BIN` -> `Contents/Helpers/ShurufaNeuralService` 解析。
@property(nonatomic, copy, nullable) NSString *helperPath;
/// 显式覆盖 GGUF 模型目录；为 nil 时默认 `~/Library/Application Support/Shurufa/resource-packs/neural-lm`。
@property(nonatomic, copy, nullable) NSString *modelDirectory;
/// 可选显式传入 `msime-pinyin.db` 路径供受限束搜索建索引。
@property(nonatomic, copy, nullable) NSString *pinyinDBPath;
/// 可选显式覆盖 Unix 域套接字路径（单元测试隔离用）。
@property(nonatomic, copy, nullable) NSString *socketPath;
/// 可选追加的后台服务启动参数（例如单元测试注入 `--test-mock`）。
@property(nonatomic, copy, nullable) NSArray<NSString *> *extraLaunchArguments;
/// 空闲卸载显存超时时间（秒），默认 `600.0`（10 分钟）。
@property(nonatomic, assign) NSTimeInterval idleUnloadSeconds;
/// 暖态单次神经解码 IPC 硬超时（秒），默认 `0.25`（250 ms）。
@property(nonatomic, assign) NSTimeInterval requestTimeoutSeconds;
/// 是否要求目录中存在 `*.gguf` 模型文件才视为可用，默认 `YES`。
@property(nonatomic, assign) BOOL requireModelFile;
/// 是否监听 `NSApplicationWillTerminateNotification` 自动停机，默认 `YES`。
@property(nonatomic, assign) BOOL listenToApplicationTermination;
@end

/// Shurufa v2 Tier 1 非阻塞端侧神经解码 IPC 客户端：
/// - 主进程零直接链接 `libllama`/`Metal`，所有 GPU 推理严格隔离在独立子进程 `ShurufaNeuralService`；
/// - `prefillContext:` 与 `decodeRequestData:` 均在纳秒级入队并立即返回，绝不阻塞主线程与按键事件；
/// - 自动合并最新代次请求、抛弃过期积压、支持子进程崩溃指数退避自愈与零掉键优雅降级 Tier 0。
@interface ShurufaNeuralClient : NSObject

+ (instancetype)sharedClient;
- (instancetype)initWithConfiguration:(nullable ShurufaNeuralClientConfiguration *)configuration;

/// 辅助进程与（当 `requireModelFile` 为 YES 时的）GGUF 模型是否就绪。
/// 使用短周期缓存，主线程查询无磁盘阻塞风险。
@property(nonatomic, readonly, getter=isAvailable) BOOL available;

/// 当前后台服务子进程 PID（未运行时为 0，仅用于诊断与单测校验崩溃自愈）。
@property(nonatomic, readonly) pid_t servicePID;

/// 配置输入法词库资源目录（自动查找其中的 `msime-pinyin.db` 传给后台服务）。
- (void)configureResourcesDirectory:(nullable NSString *)resourcesDirectory;

/// 异步预热完整上文 KV-Cache（最新值覆盖合并，绝不阻塞调用方）。
- (void)prefillContext:(NSString *)context;

/// 异步派发 Tier 1 神经解码请求（原始 UTF-8 `NeuralDecodeRequest` JSON 字节）。
/// `completion` 总在主队列回调；当服务不可用、过期被取代、崩溃或超时时，`responseData` 为 nil。
- (void)decodeRequestData:(NSData *)requestData
               generation:(uint64_t)generation
               completion:(void (^)(NSData * _Nullable responseData,
                                    uint64_t generation,
                                    double elapsedMs))completion;

/// 取消当前尚未发出的排队解码请求（例如上屏、取消组字或失焦时调用）。
- (void)cancelPendingDecodeRequests;

/// 异步健康检查（主要供单元测试与诊断使用）。
- (void)checkHealthWithCompletion:(void (^)(NSDictionary<NSString *, id> * _Nullable info))completion;

/// 停止后台子进程、关闭套接字并清理临时文件。
- (void)stop;

@end

NS_ASSUME_NONNULL_END
