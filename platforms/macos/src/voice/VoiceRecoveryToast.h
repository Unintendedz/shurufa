#pragma once

#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN

/// 语音容错与防丢字浮窗卡片（屏幕右上角不抢焦点毛玻璃浮窗）。
/// 统一承接三种高价值容错交互：
/// 1. 焦点不在可输入区域 / 切换窗口后的「结果已保留 · 一键复制」卡片；
/// 2. 识别或润色失败/超时后的「录音已保留 · 一键重试 / 打开听写历史」卡片；
/// 3. 麦克风掉线自动热切换至替代设备后的轻量提示条。
@interface MSIMEVoiceRecoveryToast : NSObject

@property(nonatomic, readonly, getter=isVisible) BOOL visible;
@property(nonatomic, readonly, copy) NSString *currentTitle;
@property(nonatomic, readonly, copy) NSString *currentBody;
@property(nonatomic, readonly) NSRect panelFrame;
@property(nonatomic, readonly) NSWindowLevel panelLevel;

+ (instancetype)sharedToast;

/// 纯函数：将语音转写文本规整为适合浮窗预览的带引号摘要（自动折行变空格、按组合字符边界截断）。
+ (NSString *)previewSnippetForText:(NSString *)text maxCharacters:(NSUInteger)maxChars;

/// 展示「未能直接上屏，结果已保留」卡片，带转写预览与一键复制按钮；绝不激活窗口或抢占输入焦点。
- (void)showUndeliveredText:(NSString *)text
                      title:(NSString *)title
                     copied:(BOOL)copied
                copyHandler:(nullable BOOL (^)(NSString *text))copyHandler;

/// 展示「识别失败/超时，录音已保留」卡片，带一键重试与打开听写历史按钮。
- (void)showRetryableFailureTitle:(NSString *)title
                           detail:(nullable NSString *)detail
                     retryHandler:(nullable void (^)(void))retryHandler
                   historyHandler:(nullable void (^)(void))historyHandler;

/// 展示麦克风热切换或起录自动回退的轻量状态提示（自动淡出，不带冗余按钮）。
- (void)showRouteNoticeTitle:(NSString *)title
                      detail:(nullable NSString *)detail;

/// 立即收起卡片并取消自动消失定时器。
- (void)dismiss;

@end

NS_ASSUME_NONNULL_END
