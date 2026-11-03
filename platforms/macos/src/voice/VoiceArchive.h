#pragma once
#import <Foundation/Foundation.h>
#import "VoiceInputService.h"

NS_ASSUME_NONNULL_BEGIN

/// 语音存档的宿主一侧：登录钥匙串里的 32 字节密钥、存档目录，以及对共享层 `msime_client_voice_archive_*` 的调用。
/// 存储格式、加密和保留期限都在 Rust（client-providers 的 voice::archive）；这里只负责取密钥、排队和转换类型。
///
/// 音频追加与录音收尾在同一条串行队列上按顺序执行，采集回调只做入队。其余调用同步执行，可在任意线程。
/// 钥匙串里还没有密钥时第一次使用就生成一把；钥匙串读失败（锁着、被拒）时不生成新密钥，存档保持关闭，免得旧记录再也解不开。
@interface MSIMEVoiceArchive : NSObject <MSIMEVoiceArchiving>
+ (instancetype)sharedArchive;
/// 存档能否使用；密钥或目录取不到时为 NO，录音照常进行，只是不存。
@property(nonatomic, readonly, getter=isAvailable) BOOL available;
/// 开始一条录音，返回它的 id；存档不可用时返回 nil。
- (nullable NSString *)beginRecordingForApp:(nullable NSString *)app provider:(nullable NSString *)provider;
/// 16 kHz 单声道 f32 PCM。
- (void)appendPCM:(NSData *)pcm;
/// 录音停止。`discard` 为 YES 时（误触的过短录音）连同记录一起删掉。之后按当前保留期限清理一次。
- (void)endCaptureDiscarding:(BOOL)discard;
- (void)noteText:(NSString *)text forRecording:(NSString *)identifier;
- (void)noteFailure:(NSString *)reason forRecording:(NSString *)identifier;
- (void)noteCancelledRecording:(NSString *)identifier;
- (BOOL)noteRetranscription:(NSString *)text forRecording:(NSString *)identifier;
/// 全部记录，新的在前；字段见 msime_client.h 的 list。`unreadable` 收到解不开的记录数。
- (nullable NSArray<NSDictionary *> *)entriesWithUnreadableCount:(NSUInteger *_Nullable)unreadable;
- (nullable NSData *)WAVForRecording:(NSString *)identifier;
/// 重新听写用的 16 kHz 单声道 f32 PCM。
- (nullable NSData *)PCMForRecording:(NSString *)identifier;
- (BOOL)deleteRecording:(NSString *)identifier;
/// 按 NSUserDefaults 里镜像的共享偏好清理过期的音频和记录。
- (void)purge;
@end

FOUNDATION_EXPORT NSString *const MSIMEVoiceArchiveDidChangeNotification;

NS_ASSUME_NONNULL_END
