#pragma once

#import <Foundation/Foundation.h>
#import "VoiceContextCapture.h"
#import "VoicePolishClient.h"

NS_ASSUME_NONNULL_BEGIN

/// Rust `msime_client_voice_postprocess` 确定性语音后处理的桥接结果。
@interface MSIMEVoicePostprocessResult : NSObject
/// 经确定性清洗后的文本；当 `dismissed == YES` 时为空串。
@property(nonatomic, copy, readonly) NSString *text;
/// 是否应静默丢弃（纯标点/语气词、短静音幻觉或复读死循环）。
@property(nonatomic, assign, readonly) BOOL dismissed;
/// 稳定丢弃原因码（`empty` / `filler_only` / `silence_hallucination` / `repetition_loop`），未丢弃时为 `nil`。
@property(nonatomic, copy, readonly, nullable) NSString *dismissedReason;
/// 与输入文本相比是否发生修改。
@property(nonatomic, assign, readonly) BOOL changed;
@end

/// 从 Rust `msime_client_voice_vocabulary` 聚合得到的个人语音词表与 Prompt Top-K 提示词。
@interface MSIMEVoiceVocabularyBundle : NSObject
/// 完整词条数组（元素为与 Rust `VocabularyTerm` 对齐的字典），供确定性后处理使用。
@property(nonatomic, copy, readonly) NSArray<NSDictionary<NSString *, id> *> *terms;
/// 面向润色 Prompt 筛选后的规范词提示（至多 60 条），供 `MSIMEVoicePolishRequest.vocabularyHints` 使用。
@property(nonatomic, copy, readonly) NSArray<NSString *> *promptHints;
@end

#ifdef __cplusplus
extern "C" {
#endif

/// 根据脱敏上下文快照推导应用场景分类（与 Rust `polish_context::classify_app` 完全对齐：
/// `web_domain` → `bundle_id` → `app_name`，浏览器结合 `window_title` 消歧）。
FOUNDATION_EXPORT NSString *MSIMEVoiceInferAppCategory(MSIMEVoiceContextSnapshot * _Nullable snapshot);

/// 通过 Rust C ABI `msime_client_voice_vocabulary` 聚合用户词库与 `voice-vocabulary.json`，
/// 并在提供 `rawText` 时一并筛选最多 60 条润色 Prompt 词表提示。
/// 涉及只读磁盘与词库读取，必须在后台工作线程调用；失败时安全返回空词表。
FOUNDATION_EXPORT MSIMEVoiceVocabularyBundle *MSIMEVoiceLoadVocabulary(
    NSDictionary * _Nullable hostOptions,
    NSString * _Nullable rawText,
    NSString * _Nullable appCategory);

/// 基于已在后台预加载到内存的词条数组，调用 `msime_client_voice_vocabulary`（`cached_terms` 模式）
/// 零磁盘/SQLite I/O 筛选最多 60 条润色 Prompt 词表提示。
FOUNDATION_EXPORT NSArray<NSString *> *MSIMEVoiceSelectPromptHints(
    NSArray<NSDictionary<NSString *, id> *> * _Nullable cachedTerms,
    NSString * _Nullable rawText,
    NSString * _Nullable appCategory);

/// 通过 Rust C ABI `msime_client_voice_postprocess` 执行无网络确定性后处理。
/// 若 FFI 返回异常或非法封装，安全原样回退到 `rawText`，绝不丢弃用户语音。
FOUNDATION_EXPORT MSIMEVoicePostprocessResult *MSIMEVoiceRunPostprocess(
    NSString *rawText,
    NSString * _Nullable modelKind,
    NSNumber * _Nullable audioMs,
    NSString * _Nullable mode,
    NSString * _Nullable appCategory,
    NSArray<NSDictionary<NSString *, id> *> * _Nullable vocabularyTerms);

/// 将录音启动瞬间抓取的 `MSIMEVoiceContextSnapshot` 转为润色请求使用的 `MSIMEVoicePolishContext`。
/// 密码框（`isSecureField == YES`）始终保留阻断标记且清空全部文本字段；非密码框仅在 `contextAware == YES` 时透传上下文。
FOUNDATION_EXPORT MSIMEVoicePolishContext * _Nullable MSIMEVoicePolishContextFromSnapshot(
    MSIMEVoiceContextSnapshot * _Nullable snapshot,
    BOOL contextAware);

/// 判断当前会话选项是否应启用新版 `MSIMEVoicePolishClient` 管线：
/// - 支持 `local` / `ollama` 及本机/私网/Tailscale 端点免 Token 润色；
/// - 云端服务仍要求非空 Token；
/// - 自定义提示词超过 8192 字符时置 `*creationFailedOut = YES` 并返回 `NO`（保持与既有测试契约一致）。
FOUNDATION_EXPORT BOOL MSIMEVoiceShouldUsePolishClient(
    NSDictionary * _Nullable options,
    BOOL * _Nullable creationFailedOut);

/// 读取配置中是否启用了上下文感知（未显式写入 `options` 时回退到 Shurufa/非 Shurufa 出厂策略）。
FOUNDATION_EXPORT BOOL MSIMEVoiceOptionsContextAware(NSDictionary * _Nullable options);

/// 读取配置中是否启用了选区口头改写。
FOUNDATION_EXPORT BOOL MSIMEVoiceOptionsSelectionRewriteEnabled(NSDictionary * _Nullable options);

/// 从语音查询选项中解析本次录音需要预热的本机 GGUF 模型路径（仅 `polish_provider == "local"` 时有效）。
/// 若无需预热返回 `NO`；需要预热时返回 `YES`，并通过 `*modelPathOut` 返回显式路径或 `nil`（表示默认目录自动发现）。
FOUNDATION_EXPORT BOOL MSIMEVoiceResolveLocalPolishPrewarmPath(
    NSDictionary * _Nullable options,
    NSString * _Nullable * _Nullable modelPathOut);

/// 依据语音查询选项、Pre-LLM 确定性后处理后的待润色文本、兜底文本、上下文快照与词表提示构造 `MSIMEVoicePolishRequest`。
FOUNDATION_EXPORT MSIMEVoicePolishRequest *MSIMEVoiceBuildPolishRequest(
    NSDictionary *options,
    NSString *inputText,
    NSString *fallbackText,
    MSIMEVoiceContextSnapshot * _Nullable snapshot,
    NSArray<NSString *> * _Nullable vocabularyHints);

/// 汇总语音存档可接受的白名单元数据字典（`raw_text` / `asr_ms` / `polish_ms` / `mode` / `app_category` / `polish_provider` / `polish_model` / `polish_prompt_id` / `polish_failed` / `polish_rejected_reason`）。
FOUNDATION_EXPORT NSDictionary<NSString *, id> *MSIMEVoiceBuildArchiveMetadata(
    NSString * _Nullable rawText,
    uint64_t asrMs,
    MSIMEVoicePolishResult * _Nullable polishResult,
    NSDictionary * _Nullable options,
    NSString * _Nullable appCategory,
    BOOL polishFailedFallback);

#ifdef __cplusplus
}
#endif

NS_ASSUME_NONNULL_END
