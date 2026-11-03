#pragma once
#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN

/// 设置窗口「听写历史」页的内容：保留期限、加密存档里的每条录音，以及播放、复制、重新听写和删除。
/// 只编进输入法本体，设置窗口按类名在运行时取它，测试程序不链接钥匙串和存档。
@interface MSIMEVoiceHistoryView : NSView
- (void)reloadHistory;
@end

NS_ASSUME_NONNULL_END
