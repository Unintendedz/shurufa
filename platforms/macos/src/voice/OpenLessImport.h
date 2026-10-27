#pragma once
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

// OpenLess 把凭据放在登录钥匙串的通用密码里：service 为 com.openless.app，account 为 credentials.v2。
FOUNDATION_EXPORT NSString *const MSIMEOpenLessKeychainService;
FOUNDATION_EXPORT NSString *const MSIMEOpenLessKeychainAccount;
FOUNDATION_EXPORT NSString *MSIMEOpenLessSupportDirectory(void);

// 从 OpenLess 导入“整理”配置：Ark 的接口地址、模型和密钥，以及用户当前在用的整理风格的系统提示词。只读 preferences.json、style-packs.json 和钥匙串里的凭据条目，不碰历史、录音和速记。
//
// 导入计划是一个字典：`endpoint`、`model`、`apiKey`（只在内存里，从不进摘要和日志）、`prompt`、`promptSource`（custom、style-pack 或 system）、`thinking`。
@interface MSIMEOpenLessImport : NSObject
// 读钥匙串条目的原始字节。读取会弹出系统授权框，由用户点“允许”；条目不存在时返回 nil 且 `error` 说明原因。阻塞调用，放在后台线程。
+ (nullable NSData *)readKeychainPayloadWithService:(NSString *)service account:(NSString *)account error:(NSError **)error;
// 解析凭据载荷（UTF-8 JSON）里的 Ark 字段，并结合 `directory` 下的偏好文件生成导入计划。
+ (nullable NSDictionary *)planFromCredentialPayload:(NSData *)payload directory:(NSString *)directory error:(NSError **)error;
// 把计划写进输入法的偏好：整理服务、提示词（自定义槽 1，并选中它）和密钥，并把识别服务设为 textstream。
+ (BOOL)applyPlan:(NSDictionary *)plan toDefaults:(NSUserDefaults *)defaults error:(NSError **)error;
// 给用户看的结果说明：模型名、提示词来源和字数、密钥已设置，不含密钥。
+ (NSString *)summaryForPlan:(NSDictionary *)plan;
@end

NS_ASSUME_NONNULL_END
