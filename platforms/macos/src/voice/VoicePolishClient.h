#pragma once

#import <Foundation/Foundation.h>
#import "LocalPolishServer.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

NS_ASSUME_NONNULL_BEGIN

#ifdef __cplusplus
extern "C" {
#endif

/// 对接 Rust C ABI（蓝图 B.3 / C.4）的函数指针桥接表。
/// Wave 1 默认通过 `dlsym(RTLD_DEFAULT, ...)` 弱解析符号或由测试注入合成实现；
/// Wave 2 定义 `MSIME_VOICE_POLISH_USE_RUST_FFI=1` 后直接静态绑定 `msime_client.h` 中的 FFI 符号。
typedef char * _Nullable (*MSIMEVoicePolishHTTPRequestFn)(const uint8_t * _Nonnull request, size_t length);
typedef char * _Nullable (*MSIMEVoicePolishParseResponseFn)(const uint8_t * _Nonnull body, size_t length, bool is_sse);
typedef char * _Nullable (*MSIMEVoicePolishCleanFn)(const uint8_t * _Nonnull request, size_t length);
typedef void (*MSIMEVoicePolishFreeStringFn)(char * _Nullable ptr);

typedef struct MSIMEVoicePolishBridge {
    MSIMEVoicePolishHTTPRequestFn _Nullable buildHTTPRequest;
    MSIMEVoicePolishParseResponseFn _Nullable parseResponse;
    MSIMEVoicePolishCleanFn _Nullable cleanOutput;
    MSIMEVoicePolishFreeStringFn _Nullable freeString;
} MSIMEVoicePolishBridge;

/// 返回默认 Rust C ABI 桥接表。
FOUNDATION_EXPORT MSIMEVoicePolishBridge MSIMEVoicePolishDefaultBridge(void);

#ifdef __cplusplus
}
#endif

/// 传递给 Rust 润色引擎的上下文快照（与 C.4 `context` 字段严格对齐）。
@interface MSIMEVoicePolishContext : NSObject <NSCopying>
@property(nonatomic, copy, nullable) NSString *appBundleID;
@property(nonatomic, copy, nullable) NSString *appName;
@property(nonatomic, copy, nullable) NSString *windowTitle;
@property(nonatomic, copy, nullable) NSString *webDomain;
@property(nonatomic, copy, nullable) NSString *precedingText;
@property(nonatomic, copy, nullable) NSString *selectedText;
@property(nonatomic, assign) BOOL isSecureField;
@property(nonatomic, assign) BOOL axBlacklisted;

/// 转为 C.4 约定的 snake_case JSON 字典。
- (NSDictionary<NSString *, id> *)toJSONDictionary;
@end

/// 单次语音润色/改写/翻译请求参数。
@interface MSIMEVoicePolishRequest : NSObject <NSCopying>
/// 润色服务标识（如 `"local"`、`"ollama"`、`"siliconflow"`、`"custom"` 等）。
@property(nonatomic, copy) NSString *provider;
/// 润色端点 URL（`provider == "local"` 时自动由 `MSIMELocalPolishServer` 填充）。
@property(nonatomic, copy, nullable) NSString *endpoint;
/// 润色模型名称（`provider == "local"` 且为空时默认 `"local"`）。
@property(nonatomic, copy, nullable) NSString *model;
/// 认证令牌（`provider == "local"` 时由 `MSIMELocalPolishServer` 内存态密钥覆盖；本机/局域网可为空）。
@property(nonatomic, copy, nullable) NSString *token;
/// 本机托管 GGUF 显式模型路径（仅 `provider == "local"` 生效，为空时自动发现）。
@property(nonatomic, copy, nullable) NSString *localModelPath;
/// 提示词预设标识（默认 `"casual"`）。
@property(nonatomic, copy) NSString *promptID;
/// 可选自定义提示词映射（键为 `"custom_1"` / `"custom_2"` / `"custom_3"`）。
@property(nonatomic, copy, nullable) NSDictionary<NSString *, NSString *> *customPrompts;
/// 润色模式：`"auto"` / `"dictation"` / `"rewrite_selection"` / `"translate"`，默认 `"auto"`。
@property(nonatomic, copy) NSString *mode;
/// 是否在 `"auto"` 模式下允许触发选区两层意图改写，默认 `NO`。
@property(nonatomic, assign) BOOL enableSelectionRewrite;
/// 翻译目标语言（如 `"en"` / `"zh-CN"`）。
@property(nonatomic, copy, nullable) NSString *targetLanguage;
/// 是否请求 SSE 流式返回，默认 `NO`。
@property(nonatomic, assign) BOOL stream;
/// 清洗阶段是否启用 Pangu 中英空格规范，默认 `YES`。
@property(nonatomic, assign) BOOL enablePanguSpacing;
/// 原始（或经确定性后处理的）待润色文本。
@property(nonatomic, copy) NSString *rawText;
/// 失败或拒收时的降级回退文本（为 `nil` 时回退到 `rawText`）。
@property(nonatomic, copy, nullable) NSString *fallbackText;
/// 目标应用上下文快照。
@property(nonatomic, copy, nullable) MSIMEVoicePolishContext *context;
/// 最多 60 条个人词表提示词。
@property(nonatomic, copy, nullable) NSArray<NSString *> *vocabularyHints;
@end

/// 单次润色结果（失败或防幻觉拒收时自动降级，绝不丢弃用户语音）。
@interface MSIMEVoicePolishResult : NSObject
/// 最终可直接上屏的文本；降级时为 `fallbackText` 或 `rawText`。
@property(nonatomic, copy, readonly) NSString *text;
/// 是否发生错误、超时、拦截或防幻觉拒收。
@property(nonatomic, assign, readonly) BOOL polishFailed;
/// 稳定可归档的降级/拒收原因码（成功时为 `nil`）。
@property(nonatomic, copy, readonly, nullable) NSString *polishRejectedReason;
/// 端到端润色耗时（毫秒）。
@property(nonatomic, assign, readonly) uint64_t polishMilliseconds;
/// Rust 引擎裁定的最终模式（`"dictation"` / `"rewrite_selection"` / `"translate"`）。
@property(nonatomic, copy, readonly) NSString *resolvedMode;
/// 最终上屏动作（`"replace_selection"` 或 `"insert"`；降级时强制为 `"insert"`）。
@property(nonatomic, copy, readonly) NSString *resolvedAction;
/// Rust 引擎裁定的应用场景分类。
@property(nonatomic, copy, readonly) NSString *appCategory;
/// 本次响应是否走了 SSE 流式解析路径。
@property(nonatomic, assign, readonly) BOOL usedStreaming;

/// 返回可直接合并到语音存档记录中的结构化元数据字典。
- (NSDictionary<NSString *, id> *)archiveMetadata;
@end

/// 可取消的在途润色任务句柄。
@interface MSIMEVoicePolishTask : NSObject
@property(nonatomic, readonly, getter=isCancelled) BOOL cancelled;
- (void)cancel;
@end

/// 独立于 `HTTPVoiceRequest` 的语音润色 HTTP/SSE 传输客户端。
/// 强制使用 `ephemeralSessionConfiguration`、对 `no_proxy` 禁用系统代理、严禁跟随 HTTP 重定向，
/// 并在 7.5 秒慢响应时触发界面提示，失败/超时/清洗拒收时自动无损降级。
@interface MSIMEVoicePolishClient : NSObject

/// 慢响应提示触发延迟（秒），默认 `7.5`（对齐 Typeless `taking_longer_alert_delay_ms`）。
@property(nonatomic, assign) NSTimeInterval slowHintDelay;
/// 慢响应提示文案，默认 `@"正在润色，请稍候…"`。
@property(nonatomic, copy) NSString *slowHintMessage;
/// 回调投递队列，默认为主队列（`dispatch_get_main_queue()`）。
@property(nonatomic, strong, nullable) dispatch_queue_t callbackQueue;

- (instancetype)init;
- (instancetype)initWithBridge:(MSIMEVoicePolishBridge)bridge
                   localServer:(nullable MSIMELocalPolishServer *)localServer NS_DESIGNATED_INITIALIZER;

/// 发起一次异步润色请求。取消任务后保证不再触发 `slowHint`、`partial` 或 `completion`。
- (MSIMEVoicePolishTask *)polishRequest:(MSIMEVoicePolishRequest *)request
                               slowHint:(nullable void (^)(NSString *message))slowHint
                                partial:(nullable void (^)(NSString *accumulatedText))partial
                             completion:(void (^)(MSIMEVoicePolishResult *result))completion;

@end

NS_ASSUME_NONNULL_END
