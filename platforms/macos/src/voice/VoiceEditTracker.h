#pragma once

#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN

/// 当 `voice-vocabulary.json` 因手动编辑、自动学词或撤销发生变化时广播，便于设置页刷新与控制器失效内存缓存。
FOUNDATION_EXPORT NSNotificationName const MSIMEVoiceVocabularyDidChangeNotification;

/// 语音上屏后 15 秒内的事后编辑追踪器：
/// 监听用户在输入框内对刚上屏文本的局部手动纠错，并在按回车发送、输入框清空、切焦点或窗口到期时
/// 提取高置信度纠错词写入 `<preferences_directory>/voice-vocabulary.json`，弹出不抢焦点的气泡提示。
@interface MSIMEVoiceEditTracker : NSObject

/// 追踪窗口时长（默认 15 秒，测试可缩短）。
@property(nonatomic, assign) NSTimeInterval trackingWindowSeconds;
/// 上屏后等待宿主落地文本的初始采样延迟（默认 0.12 秒，测试可缩短）。
@property(nonatomic, assign) NSTimeInterval initialSnapshotDelaySeconds;
/// 普通编辑键之后的防抖采样延迟（默认 0.30 秒，测试可缩短）。
@property(nonatomic, assign) NSTimeInterval editPollDebounceSeconds;
/// 当前是否存在活跃追踪会话。
@property(nonatomic, readonly, getter=isTracking) BOOL tracking;

/// 可注入测试夹具：读取目标进程当前焦点输入框完整文本（返回 nil 表示不可读或已失焦/密码框）。
@property(nonatomic, copy, nullable) NSString * _Nullable (^fieldTextReader)(pid_t pid, NSString *bundleID);
/// 可注入测试夹具：通知学到新词与一键撤销闭包（默认展示 `MSIMEVoiceLearnedToast`）。
@property(nonatomic, copy, nullable) void (^learnedHandler)(NSArray<NSString *> *terms, void (^undoHandler)(void));

/// 在语音文本成功上屏后启动一次追踪（自动取消未完成的旧会话）。
/// 若处于密码框、命中 AX 黑名单或 `insertedText`/`preferencesDirectory` 为空，则直接忽略。
- (void)beginTrackingInsertedText:(NSString *)insertedText
             preferencesDirectory:(NSString *)preferencesDirectory
                        targetPID:(pid_t)targetPID
                         bundleID:(nullable NSString *)bundleID
                    isSecureField:(BOOL)isSecureField
                    axBlacklisted:(BOOL)axBlacklisted;

/// 由 `InputController` 透传的键盘事件（绝不阻塞主线程，也绝不消费事件）。
/// 按 Enter/Return 立即触发结算；普通编辑键触发后台防抖采样。
- (void)noteKeyEvent:(NSEvent *)event;

/// 客户端失焦或切换输入上下文时调用，立即用最新编辑快照结算本次追踪。
- (void)noteClientDeactivated;

/// 静默取消当前追踪会话（如新一轮录音开始或控制器销毁）。
- (void)cancel;

@end

NS_ASSUME_NONNULL_END
