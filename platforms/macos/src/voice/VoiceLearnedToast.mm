#import "VoiceLearnedToast.h"
#import "../candidate/CandidateWindowLevel.h"

namespace {

constexpr NSTimeInterval kAutoDismissDelaySec = 4.8;
constexpr CGFloat kToastMinWidth = 220.0;
constexpr CGFloat kToastMaxWidth = 380.0;
constexpr CGFloat kToastHeight = 44.0;
constexpr CGFloat kToastCornerRadius = 12.0;
constexpr CGFloat kToastEdgeInsetRight = 18.0;
constexpr CGFloat kToastEdgeInsetTop = 18.0;

} // 匿名命名空间

/// 不抢焦点的无边框浮层面板。
@interface MSIMEVoiceLearnedToastPanel : NSPanel
@end

@implementation MSIMEVoiceLearnedToastPanel
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

/// 带悬停检测的毛玻璃内容视图，鼠标移入期间暂停自动消失。
@interface MSIMEVoiceLearnedToastContentView : NSVisualEffectView
@property(nonatomic, copy, nullable) void (^onHoverChanged)(BOOL hovering);
@end

@implementation MSIMEVoiceLearnedToastContentView {
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

@implementation MSIMEVoiceLearnedToast {
    MSIMEVoiceLearnedToastPanel *_panel;
    MSIMEVoiceLearnedToastContentView *_effectView;
    NSTextField *_messageLabel;
    NSButton *_undoButton;
    void (^_undoHandler)(void);
    NSTimer *_dismissTimer;
    BOOL _hovering;
}

+ (instancetype)sharedToast {
    static MSIMEVoiceLearnedToast *shared = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        shared = [[self alloc] init];
    });
    return shared;
}

+ (NSString *)formattedMessageForTerms:(NSArray<NSString *> *)terms {
    NSMutableArray<NSString *> *clean = [NSMutableArray array];
    for (id item in terms) {
        if (![item isKindOfClass:NSString.class]) continue;
        NSString *trimmed = [(NSString *)item stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        if (trimmed.length) [clean addObject:trimmed];
        if (clean.count >= 2) break;
    }
    if (!clean.count) return @"";
    if (clean.count == 1) {
        return [NSString stringWithFormat:@"已将「%@」加入语音词库", clean[0]];
    }
    return [NSString stringWithFormat:@"已将「%@」「%@」加入语音词库", clean[0], clean[1]];
}

- (BOOL)isVisible {
    return _panel != nil && _panel.isVisible;
}

- (void)ensurePanel {
    if (_panel) return;
    _panel = [[MSIMEVoiceLearnedToastPanel alloc]
        initWithContentRect:NSMakeRect(0, 0, 280, kToastHeight)
                  styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
                    backing:NSBackingStoreBuffered
                      defer:NO];
    _panel.opaque = NO;
    _panel.backgroundColor = NSColor.clearColor;
    _panel.hasShadow = YES;
    _panel.floatingPanel = YES;
    _panel.level = MSIMECandidateWindowLevel();
    _panel.hidesOnDeactivate = NO;
    _panel.ignoresMouseEvents = NO;
    _panel.releasedWhenClosed = NO;
    _panel.animationBehavior = NSWindowAnimationBehaviorNone;
    _panel.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                NSWindowCollectionBehaviorFullScreenAuxiliary |
                                NSWindowCollectionBehaviorTransient |
                                NSWindowCollectionBehaviorIgnoresCycle;

    _effectView = [[MSIMEVoiceLearnedToastContentView alloc] initWithFrame:NSMakeRect(0, 0, 280, kToastHeight)];
    _effectView.material = NSVisualEffectMaterialPopover;
    _effectView.blendingMode = NSVisualEffectBlendingModeBehindWindow;
    _effectView.state = NSVisualEffectStateActive;
    _effectView.wantsLayer = YES;
    _effectView.layer.cornerRadius = kToastCornerRadius;
    _effectView.layer.masksToBounds = YES;
    _effectView.layer.borderWidth = 0.5;
    _effectView.layer.borderColor = [NSColor.separatorColor colorWithAlphaComponent:0.55].CGColor;
    __weak MSIMEVoiceLearnedToast *weakSelf = self;
    _effectView.onHoverChanged = ^(BOOL hovering) {
        MSIMEVoiceLearnedToast *strongSelf = weakSelf;
        if (!strongSelf) return;
        strongSelf->_hovering = hovering;
        if (hovering) {
            [strongSelf invalidateTimer];
        } else if (strongSelf.visible) {
            [strongSelf scheduleDismissTimer];
        }
    };

    _messageLabel = [NSTextField labelWithString:@""];
    _messageLabel.font = [NSFont systemFontOfSize:13.0 weight:NSFontWeightMedium];
    _messageLabel.textColor = NSColor.labelColor;
    _messageLabel.lineBreakMode = NSLineBreakByTruncatingMiddle;
    _messageLabel.translatesAutoresizingMaskIntoConstraints = NO;
    [_messageLabel setContentCompressionResistancePriority:NSLayoutPriorityDefaultLow
                                            forOrientation:NSLayoutConstraintOrientationHorizontal];

    _undoButton = [NSButton buttonWithTitle:@"撤销" target:self action:@selector(handleUndo:)];
    _undoButton.bezelStyle = NSBezelStyleInline;
    _undoButton.font = [NSFont systemFontOfSize:12.0 weight:NSFontWeightMedium];
    _undoButton.translatesAutoresizingMaskIntoConstraints = NO;
    [_undoButton setContentCompressionResistancePriority:NSLayoutPriorityRequired
                                          forOrientation:NSLayoutConstraintOrientationHorizontal];

    NSStackView *row = [NSStackView stackViewWithViews:@[_messageLabel, _undoButton]];
    row.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    row.alignment = NSLayoutAttributeCenterY;
    row.spacing = 10.0;
    row.translatesAutoresizingMaskIntoConstraints = NO;
    [_effectView addSubview:row];
    [NSLayoutConstraint activateConstraints:@[
        [row.leadingAnchor constraintEqualToAnchor:_effectView.leadingAnchor constant:14.0],
        [row.trailingAnchor constraintEqualToAnchor:_effectView.trailingAnchor constant:-12.0],
        [row.centerYAnchor constraintEqualToAnchor:_effectView.centerYAnchor],
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

- (void)scheduleDismissTimer {
    [self invalidateTimer];
    __weak MSIMEVoiceLearnedToast *weakSelf = self;
    _dismissTimer = [NSTimer scheduledTimerWithTimeInterval:kAutoDismissDelaySec
                                                    repeats:NO
                                                      block:^(NSTimer * _Nonnull timer) {
                                                          (void)timer;
                                                          MSIMEVoiceLearnedToast *strongSelf = weakSelf;
                                                          if (!strongSelf || strongSelf->_hovering) return;
                                                          [strongSelf dismiss];
                                                      }];
}

- (void)handleUndo:(id)sender {
    (void)sender;
    void (^handler)(void) = _undoHandler;
    _undoHandler = nil;
    [self dismiss];
    if (handler) handler();
}

- (void)showLearnedTerms:(NSArray<NSString *> *)terms
             undoHandler:(nullable void (^)(void))undoHandler {
    NSString *message = [MSIMEVoiceLearnedToast formattedMessageForTerms:terms];
    if (!message.length) return;
    [self ensurePanel];
    _hovering = NO;
    _undoHandler = [undoHandler copy];
    _messageLabel.stringValue = message;
    _undoButton.hidden = (undoHandler == nil);

    const CGFloat textWidth = ceil([_messageLabel sizeThatFits:NSMakeSize(kToastMaxWidth, kToastHeight)].width);
    const CGFloat buttonWidth = undoHandler ? 54.0 : 0.0;
    const CGFloat width = MIN(kToastMaxWidth, MAX(kToastMinWidth, textWidth + buttonWidth + 36.0));
    NSScreen *screen = [self targetScreen];
    const NSRect visible = screen ? screen.visibleFrame : NSMakeRect(0, 0, 1440, 900);
    const CGFloat x = NSMaxX(visible) - width - kToastEdgeInsetRight;
    const CGFloat y = NSMaxY(visible) - kToastHeight - kToastEdgeInsetTop;

    [_panel setFrame:NSMakeRect(x, y, width, kToastHeight) display:YES];
    _effectView.frame = NSMakeRect(0, 0, width, kToastHeight);
    _panel.alphaValue = 1.0;
    [_panel orderFrontRegardless];
    [self scheduleDismissTimer];
}

- (void)dismiss {
    [self invalidateTimer];
    _undoHandler = nil;
    _hovering = NO;
    [_panel orderOut:nil];
}

- (void)dealloc {
    [self invalidateTimer];
}

@end
