#pragma once

#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN

/// 语音自动学词后的轻量不抢焦点提示气泡（屏幕右上角毛玻璃浮窗，附带「撤销」按钮与自动淡出）。
@interface MSIMEVoiceLearnedToast : NSObject

@property(nonatomic, readonly, getter=isVisible) BOOL visible;

+ (instancetype)sharedToast;

/// 将新学词列表格式化为气泡展示文案（纯函数，供界面与单元测试共用）。
+ (NSString *)formattedMessageForTerms:(NSArray<NSString *> *)terms;

/// 展示自动学词提示气泡；点击「撤销」执行 `undoHandler` 并立即收起。
/// 必须在主线程调用，绝不激活窗口或抢占输入焦点。
- (void)showLearnedTerms:(NSArray<NSString *> *)terms
             undoHandler:(nullable void (^)(void))undoHandler;

/// 立即隐藏气泡并取消自动消失定时器。
- (void)dismiss;

@end

NS_ASSUME_NONNULL_END
