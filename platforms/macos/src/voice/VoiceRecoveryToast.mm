#import "VoiceRecoveryToast.h"

namespace {

constexpr CGFloat kCardWidth = 360.0;
constexpr CGFloat kCardCornerRadius = 14.0;
constexpr CGFloat kCardEdgeInsetRight = 18.0;
constexpr CGFloat kCardEdgeInsetTop = 18.0;
constexpr NSTimeInterval kUndeliveredCopiedDelaySec = 12.0;
constexpr NSTimeInterval kUndeliveredNotCopiedDelaySec = 18.0;
constexpr NSTimeInterval kRetryableFailureDelaySec = 15.0;
constexpr NSTimeInterval kRouteNoticeDelaySec = 4.2;
constexpr NSTimeInterval kPostCopyDismissDelaySec = 1.8;

} // 匿名命名空间

/// 不抢焦点的无边框浮层面板。
@interface MSIMEVoiceRecoveryToastPanel : NSPanel
@end

@implementation MSIMEVoiceRecoveryToastPanel
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

/// 带悬停检测的毛玻璃内容视图：鼠标停留在卡片上时暂停自动淡出，方便用户阅读预览或点击复制/重试。
@interface MSIMEVoiceRecoveryToastContentView : NSVisualEffectView
@property(nonatomic, copy, nullable) void (^onHoverChanged)(BOOL hovering);
@end

@implementation MSIMEVoiceRecoveryToastContentView {
    NSTrackingArea *_trackingArea;
}

- (void)updateTrackingAreas {
    [super updateTrackingAreas];
    if (_trackingArea) {
        [self removeTrackingArea:_trackingArea];
        _trackingArea = nil;
    }
    _trackingArea = [[NSTrackingArea alloc]
        initWithRect:self.bounds
             options:NSTrackingMouseEnteredAndExited | NSTrackingActiveAlways | NSTrackingInVisibleRect
               owner:self
            userInfo:nil];
    [self addTrackingArea:_trackingArea];
}

- (void)mouseEntered:(NSEvent *)event {
    (void)event;
    if (self.onHoverChanged) self.onHoverChanged(YES);
}

- (void)mouseExited:(NSEvent *)event {
    (void)event;
    if (self.onHoverChanged) self.onHoverChanged(NO);
}

@end

@implementation MSIMEVoiceRecoveryToast {
    MSIMEVoiceRecoveryToastPanel *_panel;
    MSIMEVoiceRecoveryToastContentView *_effectView;
    NSTextField *_titleLabel;
    NSTextField *_bodyLabel;
    NSButton *_closeButton;
    NSButton *_primaryButton;
    NSButton *_secondaryButton;
    NSStackView *_buttonRow;
    NSString *_currentTitle;
    NSString *_currentBody;
    NSString *_undeliveredText;
    BOOL (^_copyHandler)(NSString *);
    void (^_retryHandler)(void);
    void (^_historyHandler)(void);
    NSTimer *_dismissTimer;
    NSTimeInterval _activeDismissDelay;
    BOOL _hovering;
}

+ (instancetype)sharedToast {
    static MSIMEVoiceRecoveryToast *shared = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        shared = [[self alloc] init];
    });
    return shared;
}

+ (NSString *)previewSnippetForText:(NSString *)text maxCharacters:(NSUInteger)maxChars {
    if (![text isKindOfClass:NSString.class] || !text.length) return @"";
    // 将连续空白和换行压成单空格，避免多行空隙撑爆浮窗。
    NSArray<NSString *> *parts = [text componentsSeparatedByCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    NSMutableArray<NSString *> *nonEmpty = [NSMutableArray arrayWithCapacity:parts.count];
    for (NSString *part in parts) {
        if (part.length) [nonEmpty addObject:part];
    }
    NSString *collapsed = [nonEmpty componentsJoinedByString:@" "];
    if (!collapsed.length) return @"";
    if (maxChars == 0 || collapsed.length <= maxChars) {
        return [NSString stringWithFormat:@"“%@”", collapsed];
    }
    // 沿组合字符序列边界截断，绝不把代理对或组合表情劈成半个字符。
    NSRange boundary = [collapsed rangeOfComposedCharacterSequenceAtIndex:maxChars];
    NSString *head = [collapsed substringToIndex:boundary.location];
    return [NSString stringWithFormat:@"“%@…”", head];
}

- (BOOL)isVisible {
    return _panel != nil && _panel.isVisible;
}

- (NSString *)currentTitle {
    return _currentTitle ?: @"";
}

- (NSString *)currentBody {
    return _currentBody ?: @"";
}

- (void)ensurePanel {
    if (_panel) return;
    _panel = [[MSIMEVoiceRecoveryToastPanel alloc]
        initWithContentRect:NSMakeRect(0, 0, kCardWidth, 96.0)
                  styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
                    backing:NSBackingStoreBuffered
                      defer:YES];
    _panel.opaque = NO;
    _panel.backgroundColor = NSColor.clearColor;
    _panel.hasShadow = YES;
    _panel.level = NSStatusWindowLevel;
    _panel.hidesOnDeactivate = NO;
    _panel.ignoresMouseEvents = NO;
    _panel.releasedWhenClosed = NO;
    _panel.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                NSWindowCollectionBehaviorFullScreenAuxiliary |
                                NSWindowCollectionBehaviorTransient |
                                NSWindowCollectionBehaviorIgnoresCycle;

    _effectView = [[MSIMEVoiceRecoveryToastContentView alloc] initWithFrame:NSMakeRect(0, 0, kCardWidth, 96.0)];
    _effectView.material = NSVisualEffectMaterialPopover;
    _effectView.blendingMode = NSVisualEffectBlendingModeBehindWindow;
    _effectView.state = NSVisualEffectStateActive;
    _effectView.wantsLayer = YES;
    _effectView.layer.cornerRadius = kCardCornerRadius;
    _effectView.layer.masksToBounds = YES;
    _effectView.layer.borderWidth = 0.5;
    _effectView.layer.borderColor = [NSColor.separatorColor colorWithAlphaComponent:0.55].CGColor;

    __weak MSIMEVoiceRecoveryToast *weakSelf = self;
    _effectView.onHoverChanged = ^(BOOL hovering) {
        MSIMEVoiceRecoveryToast *strongSelf = weakSelf;
        if (!strongSelf) return;
        strongSelf->_hovering = hovering;
        if (hovering) {
            [strongSelf invalidateTimer];
        } else if (strongSelf.visible) {
            [strongSelf scheduleDismissAfter:strongSelf->_activeDismissDelay];
        }
    };

    _titleLabel = [NSTextField labelWithString:@""];
    _titleLabel.font = [NSFont systemFontOfSize:13.0 weight:NSFontWeightSemibold];
    _titleLabel.textColor = NSColor.labelColor;
    _titleLabel.lineBreakMode = NSLineBreakByTruncatingTail;
    _titleLabel.translatesAutoresizingMaskIntoConstraints = NO;
    [_titleLabel setContentCompressionResistancePriority:NSLayoutPriorityDefaultLow
                                          forOrientation:NSLayoutConstraintOrientationHorizontal];

    _closeButton = [NSButton buttonWithTitle:@"×" target:self action:@selector(handleClose:)];
    _closeButton.bordered = NO;
    _closeButton.font = [NSFont systemFontOfSize:15.0 weight:NSFontWeightRegular];
    _closeButton.contentTintColor = NSColor.secondaryLabelColor;
    _closeButton.refusesFirstResponder = YES;
    _closeButton.accessibilityLabel = @"关闭";
    _closeButton.translatesAutoresizingMaskIntoConstraints = NO;
    [_closeButton setContentCompressionResistancePriority:NSLayoutPriorityRequired
                                           forOrientation:NSLayoutConstraintOrientationHorizontal];

    NSStackView *headerRow = [NSStackView stackViewWithViews:@[_titleLabel, _closeButton]];
    headerRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    headerRow.alignment = NSLayoutAttributeCenterY;
    headerRow.distribution = NSStackViewDistributionFill;
    headerRow.spacing = 8.0;
    headerRow.translatesAutoresizingMaskIntoConstraints = NO;

    _bodyLabel = [NSTextField wrappingLabelWithString:@""];
    _bodyLabel.font = [NSFont systemFontOfSize:12.5 weight:NSFontWeightRegular];
    _bodyLabel.textColor = NSColor.secondaryLabelColor;
    _bodyLabel.maximumNumberOfLines = 3;
    _bodyLabel.lineBreakMode = NSLineBreakByTruncatingTail;
    _bodyLabel.preferredMaxLayoutWidth = kCardWidth - 28.0;
    _bodyLabel.translatesAutoresizingMaskIntoConstraints = NO;

    _primaryButton = [NSButton buttonWithTitle:@"复制" target:self action:@selector(handlePrimaryAction:)];
    _primaryButton.bezelStyle = NSBezelStyleRounded;
    _primaryButton.controlSize = NSControlSizeSmall;
    _primaryButton.font = [NSFont systemFontOfSize:12.0 weight:NSFontWeightMedium];
    _primaryButton.refusesFirstResponder = YES;
    _primaryButton.translatesAutoresizingMaskIntoConstraints = NO;

    _secondaryButton = [NSButton buttonWithTitle:@"听写历史" target:self action:@selector(handleSecondaryAction:)];
    _secondaryButton.bezelStyle = NSBezelStyleRounded;
    _secondaryButton.controlSize = NSControlSizeSmall;
    _secondaryButton.font = [NSFont systemFontOfSize:12.0 weight:NSFontWeightRegular];
    _secondaryButton.refusesFirstResponder = YES;
    _secondaryButton.translatesAutoresizingMaskIntoConstraints = NO;

    NSView *spacer = [NSView new];
    spacer.translatesAutoresizingMaskIntoConstraints = NO;
    [spacer setContentHuggingPriority:NSLayoutPriorityDefaultLow forOrientation:NSLayoutConstraintOrientationHorizontal];

    _buttonRow = [NSStackView stackViewWithViews:@[spacer, _secondaryButton, _primaryButton]];
    _buttonRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    _buttonRow.alignment = NSLayoutAttributeCenterY;
    _buttonRow.spacing = 8.0;
    _buttonRow.translatesAutoresizingMaskIntoConstraints = NO;

    NSStackView *column = [NSStackView stackViewWithViews:@[headerRow, _bodyLabel, _buttonRow]];
    column.orientation = NSUserInterfaceLayoutOrientationVertical;
    column.alignment = NSLayoutAttributeLeading;
    column.spacing = 6.0;
    column.translatesAutoresizingMaskIntoConstraints = NO;
    [_effectView addSubview:column];

    [NSLayoutConstraint activateConstraints:@[
        [column.leadingAnchor constraintEqualToAnchor:_effectView.leadingAnchor constant:14.0],
        [column.trailingAnchor constraintEqualToAnchor:_effectView.trailingAnchor constant:-14.0],
        [column.topAnchor constraintEqualToAnchor:_effectView.topAnchor constant:12.0],
        [column.bottomAnchor constraintEqualToAnchor:_effectView.bottomAnchor constant:-12.0],
        [headerRow.widthAnchor constraintEqualToAnchor:column.widthAnchor],
        [_bodyLabel.widthAnchor constraintEqualToAnchor:column.widthAnchor],
        [_buttonRow.widthAnchor constraintEqualToAnchor:column.widthAnchor],
    ]];

    _panel.contentView = _effectView;
}

- (NSScreen *)targetScreen {
    const NSPoint mouse = NSEvent.mouseLocation;
    for (NSScreen *screen in NSScreen.screens) {
        if (NSPointInRect(mouse, screen.frame)) return screen;
    }
    return NSScreen.mainScreen ?: NSScreen.screens.firstObject;
}

- (void)invalidateTimer {
    [_dismissTimer invalidate];
    _dismissTimer = nil;
}

- (void)scheduleDismissAfter:(NSTimeInterval)delay {
    [self invalidateTimer];
    _activeDismissDelay = MAX(1.0, delay);
    __weak MSIMEVoiceRecoveryToast *weakSelf = self;
    _dismissTimer = [NSTimer scheduledTimerWithTimeInterval:_activeDismissDelay
                                                    repeats:NO
                                                      block:^(NSTimer * _Nonnull timer) {
                                                          (void)timer;
                                                          MSIMEVoiceRecoveryToast *strongSelf = weakSelf;
                                                          if (!strongSelf || strongSelf->_hovering) return;
                                                          [strongSelf dismiss];
                                                      }];
}

- (void)presentCardWithDismissDelay:(NSTimeInterval)delay {
    [self ensurePanel];
    _hovering = NO;

    _titleLabel.stringValue = _currentTitle ?: @"";
    _bodyLabel.stringValue = _currentBody ?: @"";
    _bodyLabel.hidden = (_currentBody.length == 0);
    _buttonRow.hidden = (_primaryButton.hidden && _secondaryButton.hidden);

    [_effectView layoutSubtreeIfNeeded];
    const NSSize fitting = [_effectView fittingSize];
    const CGFloat height = MAX(56.0, MIN(176.0, ceil(fitting.height)));

    NSScreen *screen = [self targetScreen];
    const NSRect visible = screen ? screen.visibleFrame : NSMakeRect(0, 0, 1440, 900);
    const CGFloat x = NSMaxX(visible) - kCardWidth - kCardEdgeInsetRight;
    const CGFloat y = NSMaxY(visible) - height - kCardEdgeInsetTop;

    [_panel setFrame:NSMakeRect(x, y, kCardWidth, height) display:YES];
    _effectView.frame = NSMakeRect(0, 0, kCardWidth, height);
    _panel.alphaValue = 1.0;
    [_panel orderFrontRegardless];
    [self scheduleDismissAfter:delay];
}

- (void)handleClose:(id)sender {
    (void)sender;
    [self dismiss];
}

- (void)handlePrimaryAction:(id)sender {
    (void)sender;
    if (_retryHandler) {
        void (^retry)(void) = _retryHandler;
        [self dismiss];
        retry();
        return;
    }
    if (_undeliveredText.length) {
        BOOL ok = NO;
        if (_copyHandler) {
            ok = _copyHandler(_undeliveredText);
        } else {
            NSPasteboard *board = NSPasteboard.generalPasteboard;
            [board clearContents];
            ok = [board setString:_undeliveredText forType:NSPasteboardTypeString];
        }
        _primaryButton.title = ok ? @"已复制 ✓" : @"复制失败";
        if (ok) {
            [self scheduleDismissAfter:kPostCopyDismissDelaySec];
        }
    }
}

- (void)handleSecondaryAction:(id)sender {
    (void)sender;
    void (^history)(void) = _historyHandler;
    [self dismiss];
    if (history) history();
}

- (void)showUndeliveredText:(NSString *)text
                      title:(NSString *)title
                     copied:(BOOL)copied
                copyHandler:(nullable BOOL (^)(NSString *text))copyHandler {
    if (![text isKindOfClass:NSString.class] || !text.length) return;
    [self ensurePanel];
    _undeliveredText = [text copy];
    _copyHandler = [copyHandler copy];
    _retryHandler = nil;
    _historyHandler = nil;
    _currentTitle = title.length ? [title copy] : (copied ? @"当前位置不可输入 · 已复制" : @"当前位置不可输入 · 点击复制");
    _currentBody = [MSIMEVoiceRecoveryToast previewSnippetForText:text maxCharacters:120];
    _primaryButton.title = copied ? @"再次复制" : @"复制";
    _primaryButton.hidden = NO;
    _secondaryButton.hidden = YES;
    [self presentCardWithDismissDelay:copied ? kUndeliveredCopiedDelaySec : kUndeliveredNotCopiedDelaySec];
}

- (void)showRetryableFailureTitle:(NSString *)title
                           detail:(nullable NSString *)detail
                     retryHandler:(nullable void (^)(void))retryHandler
                   historyHandler:(nullable void (^)(void))historyHandler {
    [self ensurePanel];
    _undeliveredText = nil;
    _copyHandler = nil;
    _retryHandler = [retryHandler copy];
    _historyHandler = [historyHandler copy];
    _currentTitle = title.length ? [title copy] : @"语音识别失败 · 录音已保留";
    _currentBody = [detail isKindOfClass:NSString.class] && detail.length
        ? [detail copy]
        : @"您的语音已安全保存在本机听写历史，可点击重试继续转写。";
    _primaryButton.title = @"重试识别";
    _primaryButton.hidden = (_retryHandler == nil);
    _secondaryButton.title = @"听写历史";
    _secondaryButton.hidden = (_historyHandler == nil);
    [self presentCardWithDismissDelay:kRetryableFailureDelaySec];
}

- (void)showRouteNoticeTitle:(NSString *)title detail:(nullable NSString *)detail {
    [self ensurePanel];
    _undeliveredText = nil;
    _copyHandler = nil;
    _retryHandler = nil;
    _historyHandler = nil;
    _currentTitle = title.length ? [title copy] : @"麦克风已自动切换";
    _currentBody = [detail isKindOfClass:NSString.class] ? [detail copy] : @"";
    _primaryButton.hidden = YES;
    _secondaryButton.hidden = YES;
    [self presentCardWithDismissDelay:kRouteNoticeDelaySec];
}

- (void)dismiss {
    [self invalidateTimer];
    _undeliveredText = nil;
    _copyHandler = nil;
    _retryHandler = nil;
    _historyHandler = nil;
    _hovering = NO;
    [_panel orderOut:nil];
}

- (void)dealloc {
    [self invalidateTimer];
}

@end
