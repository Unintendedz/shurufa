#pragma once
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// 语音上下文快照各字段的硬长度上界，与 Rust 润色契约保持一致。
static const NSUInteger MSIMEVoiceContextMaxBundleIDBytes = 128;
static const NSUInteger MSIMEVoiceContextMaxAppNameBytes = 128;
static const NSUInteger MSIMEVoiceContextMaxWindowTitleUTF16 = 200;
static const NSUInteger MSIMEVoiceContextMaxWebDomainBytes = 253;
static const NSUInteger MSIMEVoiceContextMaxPrecedingTextUTF16 = 500;
static const NSUInteger MSIMEVoiceContextMaxSelectedTextUTF16 = 4000;

/// 录音启动瞬间抓取的脱敏宿主上下文快照（不可变纯数据对象）。
/// 绝不携带完整 URL 路径、查询参数、片段或密码框内容。
@interface MSIMEVoiceContextSnapshot : NSObject <NSCopying>

@property(nonatomic, copy, readonly) NSString *appBundleID;
@property(nonatomic, copy, readonly) NSString *appName;
@property(nonatomic, copy, readonly) NSString *windowTitle;
@property(nonatomic, copy, readonly) NSString *webDomain;
@property(nonatomic, copy, readonly) NSString *precedingText;
@property(nonatomic, copy, readonly) NSString *selectedText;
@property(nonatomic, readonly) BOOL isSecureField;
@property(nonatomic, readonly) BOOL axBlacklisted;

- (instancetype)initWithAppBundleID:(nullable NSString *)appBundleID
                            appName:(nullable NSString *)appName
                        windowTitle:(nullable NSString *)windowTitle
                          webDomain:(nullable NSString *)webDomain
                      precedingText:(nullable NSString *)precedingText
                       selectedText:(nullable NSString *)selectedText
                      isSecureField:(BOOL)isSecureField
                      axBlacklisted:(BOOL)axBlacklisted NS_DESIGNATED_INITIALIZER;

/// 构造全空上下文快照；密码框场景传入 `YES`，确保任何文本字段均为空串。
+ (instancetype)emptySnapshotWithSecureField:(BOOL)isSecureField;

/// 序列化为与 Rust `msime_client_voice_polish_http_request` 的 `context` 字段严格对齐的字典。
- (NSDictionary<NSString *, id> *)toJSONDictionary;

@end

#ifdef __cplusplus
extern "C" {
#endif

/// 判断 bundle identifier 是否属于已知浏览器（仅对浏览器尝试读取网页域名）。
BOOL MSIMEIsBrowserBundleIdentifier(NSString * _Nullable bundleID);

/// 判断 bundle identifier 是否命中内建 AX 防卡死黑名单（如微信、Sublime Text 4）。
BOOL MSIMEIsAXBlacklistedBundleIdentifier(NSString * _Nullable bundleID);

/// 判断原始 URL 是否命中内建 AX 防卡死页面黑名单（如 `docs.google.com/document/d`）。
BOOL MSIMEIsAXBlacklistedURLString(NSString * _Nullable urlString);

/// 组合判断当前应用或页面是否命中 AX 防卡死黑名单。
BOOL MSIMEIsAXBlacklistedContext(NSString * _Nullable bundleID, NSString * _Nullable rawURLString);

/// 从原始 URL 中仅提取小写 host（严格剥离协议、凭据、端口、路径、查询参数与片段）。
/// 非 http(s) URL 或非法地址返回空串。
NSString *MSIMEExtractWebDomainFromURLString(NSString * _Nullable urlString);

/// 清洗并截断选区文本：强制剥离 `U+FFFC`（QQ/富文本对象占位符）、`U+FEFF`、零宽字符、
/// 非换行控制字符及首尾空白；若剥离后为空则返回空串，彻底杜绝假选区误触发改写。
NSString *MSIMESanitizeSelectedText(NSString * _Nullable rawText, NSUInteger maxUTF16Units);

/// 按 UTF-16 单元上限从头部安全截断，保证不截断代理对。
NSString *MSIMEClampUTF16String(NSString * _Nullable text, NSUInteger maxUTF16Units);

/// 按 UTF-8 字节上限安全截断，保证不截断多字节字符。
NSString *MSIMEClampUTF8BytesString(NSString * _Nullable text, NSUInteger maxUTF8Bytes);

/// 统一的纯函数快照装配器：集中执行密码框硬屏蔽、AX 黑名单降级、终端跳过前文、
/// 浏览器域名剥离与假选区清洗，实时采集与单元测试共用同一份逻辑。
MSIMEVoiceContextSnapshot *MSIMEBuildVoiceContextSnapshot(
    BOOL isSecureEventInput,
    BOOL isSecureAXField,
    NSString * _Nullable appBundleID,
    NSString * _Nullable appName,
    NSString * _Nullable windowTitle,
    NSString * _Nullable rawURLString,
    NSString * _Nullable precedingText,
    NSString * _Nullable selectedText);

/// 在录音启动瞬间同步抓取当前焦点应用的受保护上下文快照。
/// `client` 为当前 `IMKTextInput` 客户端（可为 nil）；总 AX 预算严格控制在 25ms 内，不弹窗请求权限。
MSIMEVoiceContextSnapshot *MSIMECaptureVoiceContext(id _Nullable client);

#ifdef __cplusplus
}
#endif

NS_ASSUME_NONNULL_END
