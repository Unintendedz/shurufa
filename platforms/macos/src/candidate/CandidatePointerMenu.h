#pragma once
#import <AppKit/AppKit.h>

// 候选右键菜单的弹出层。NSMenu 一弹出，WindowServer 就把键盘焦点交给菜单所在的进程（StealKeyFocus），
// 输入法开菜单等于让客户端窗口失去键盘焦点：Zen、Firefox 这类 Gecko 客户端失焦时自己结束组字，
// 输入法随即收到 commitComposition:，候选窗收起，菜单里的操作也全部失效。这里按 NSMenu 的内容画一组
// 不能成为 key 的非激活面板，键盘焦点始终留在客户端；菜单只接受指针操作，Esc 由输入法控制器转来关闭。
// 菜单项照旧是 NSMenuItem，选中时按它的 action 和 target 派发，和 NSMenu 选中一项时一样。

@interface MSIMEPointerMenu : NSObject
@property(nonatomic, readonly, getter=isVisible) BOOL visible;
// 根菜单在前，打开的子菜单在后；测试和点击外部判断用。
@property(nonatomic, readonly) NSArray<NSWindow *> *windows;
// screenPoint 是菜单左上角希望贴住的指针位置；放不下时像 NSMenu 一样翻到指针的左侧或上方。
- (void)popUpMenu:(NSMenu *)menu atScreenPoint:(NSPoint)screenPoint level:(NSWindowLevel)level;
- (void)dismiss;
// 菜单行的回调。
- (void)rowHovered:(NSView *)row;
- (void)rowExited:(NSView *)row;
- (void)rowClicked:(NSView *)row;
@end

@interface MSIMEPointerMenuWindow : NSPanel
@end
@implementation MSIMEPointerMenuWindow
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

static const CGFloat MSIMEPointerMenuRowHeight = 24.0;
static const CGFloat MSIMEPointerMenuSeparatorHeight = 11.0;
static const CGFloat MSIMEPointerMenuPadding = 5.0;
static const CGFloat MSIMEPointerMenuCornerRadius = 10.0;
static const CGFloat MSIMEPointerMenuHighlightRadius = 6.0;
static const CGFloat MSIMEPointerMenuCheckColumn = 20.0;
static const CGFloat MSIMEPointerMenuTextInset = 10.0;
static const CGFloat MSIMEPointerMenuMinimumWidth = 120.0;

// 和系统菜单一样按 ⌃⌥⇧⌘ 的顺序写修饰键，字母键大写。
static NSString *MSIMEPointerMenuKeyEquivalentText(NSMenuItem *item)
{
    if (!item.keyEquivalent.length) return @"";
    const NSEventModifierFlags flags = item.keyEquivalentModifierMask;
    NSMutableString *text = [NSMutableString string];
    if (flags & NSEventModifierFlagControl) [text appendString:@"⌃"];
    if (flags & NSEventModifierFlagOption) [text appendString:@"⌥"];
    if (flags & NSEventModifierFlagShift) [text appendString:@"⇧"];
    if (flags & NSEventModifierFlagCommand) [text appendString:@"⌘"];
    [text appendString:[item.keyEquivalent isEqual:@" "] ? @"Space" : item.keyEquivalent.uppercaseString];
    return text;
}

static NSDictionary *MSIMEPointerMenuTextAttributes(NSColor *color)
{
    return @{NSFontAttributeName:[NSFont menuFontOfSize:0], NSForegroundColorAttributeName:color};
}

@interface MSIMEPointerMenuRow : NSView
@property(nonatomic, strong, readonly) NSMenuItem *item;
@property(nonatomic, weak) MSIMEPointerMenu *owner;
@property(nonatomic) BOOL highlighted;
@property(nonatomic) BOOL reservesCheckColumn;
@end
@implementation MSIMEPointerMenuRow
{
    NSTrackingArea *_trackingArea;
}
- (instancetype)initWithItem:(NSMenuItem *)item owner:(MSIMEPointerMenu *)owner
{
    self = [super initWithFrame:NSZeroRect];
    if (!self) return nil;
    _item = item;
    _owner = owner;
    return self;
}
- (BOOL)isFlipped { return YES; }
- (BOOL)acceptsFirstMouse:(NSEvent *)event { (void)event; return YES; }
- (BOOL)selectable { return !self.item.separatorItem && self.item.enabled; }
- (void)updateTrackingAreas
{
    if (_trackingArea) [self removeTrackingArea:_trackingArea];
    _trackingArea = [[NSTrackingArea alloc] initWithRect:NSZeroRect
        options:NSTrackingMouseEnteredAndExited | NSTrackingMouseMoved | NSTrackingActiveAlways | NSTrackingInVisibleRect
        owner:self userInfo:nil];
    [self addTrackingArea:_trackingArea];
    [super updateTrackingAreas];
}
- (void)mouseEntered:(NSEvent *)event { (void)event; [self.owner rowHovered:self]; }
- (void)mouseMoved:(NSEvent *)event { (void)event; [self.owner rowHovered:self]; }
- (void)mouseExited:(NSEvent *)event { (void)event; [self.owner rowExited:self]; }
// 按下只为让松开落到这一行；和 NSMenu 一样，左右键松开都算选中。
- (void)mouseDown:(NSEvent *)event { (void)event; }
- (void)rightMouseDown:(NSEvent *)event { (void)event; }
- (void)mouseUp:(NSEvent *)event { [self releaseWithEvent:event]; }
- (void)rightMouseUp:(NSEvent *)event { [self releaseWithEvent:event]; }
- (void)releaseWithEvent:(NSEvent *)event
{
    if (NSPointInRect([self convertPoint:event.locationInWindow fromView:nil], self.bounds)) [self.owner rowClicked:self];
}
- (void)setHighlighted:(BOOL)highlighted
{
    if (_highlighted == highlighted) return;
    _highlighted = highlighted;
    self.needsDisplay = YES;
}
- (CGFloat)textLeft
{
    return MSIMEPointerMenuPadding + (self.reservesCheckColumn ? MSIMEPointerMenuCheckColumn : MSIMEPointerMenuTextInset);
}
- (CGFloat)preferredWidth
{
    if (self.item.separatorItem) return 0;
    CGFloat width = self.textLeft + [self.item.title sizeWithAttributes:MSIMEPointerMenuTextAttributes(NSColor.labelColor)].width;
    NSString *key = MSIMEPointerMenuKeyEquivalentText(self.item);
    if (key.length) width += 24.0 + [key sizeWithAttributes:MSIMEPointerMenuTextAttributes(NSColor.labelColor)].width;
    else if (self.item.submenu) width += 24.0 + 8.0;
    return ceil(width + MSIMEPointerMenuTextInset + MSIMEPointerMenuPadding);
}
- (void)drawRect:(NSRect)dirtyRect
{
    (void)dirtyRect;
    const NSRect bounds = self.bounds;
    if (self.item.separatorItem) {
        [NSColor.separatorColor setFill];
        NSRectFill(NSMakeRect(MSIMEPointerMenuPadding + MSIMEPointerMenuTextInset, floor(NSMidY(bounds)),
            MAX(0.0, NSWidth(bounds) - 2 * (MSIMEPointerMenuPadding + MSIMEPointerMenuTextInset)), 1.0));
        return;
    }
    const BOOL lit = self.highlighted && self.selectable;
    if (lit) {
        [NSColor.selectedContentBackgroundColor setFill];
        [[NSBezierPath bezierPathWithRoundedRect:NSInsetRect(bounds, MSIMEPointerMenuPadding, 0)
            xRadius:MSIMEPointerMenuHighlightRadius yRadius:MSIMEPointerMenuHighlightRadius] fill];
    }
    NSColor *color = !self.item.enabled ? NSColor.tertiaryLabelColor : lit ? NSColor.selectedMenuItemTextColor : NSColor.labelColor;
    NSDictionary *attributes = MSIMEPointerMenuTextAttributes(color);
    const auto drawCentered = [&](NSString *text, CGFloat x) {
        const NSSize size = [text sizeWithAttributes:attributes];
        [text drawAtPoint:NSMakePoint(x, floor((NSHeight(bounds) - size.height) / 2.0)) withAttributes:attributes];
        return size.width;
    };
    if (self.item.state == NSControlStateValueOn) drawCentered(@"✓", MSIMEPointerMenuPadding + 5.0);
    else if (self.item.state == NSControlStateValueMixed) drawCentered(@"–", MSIMEPointerMenuPadding + 5.0);
    drawCentered(self.item.title, self.textLeft);
    const CGFloat right = NSWidth(bounds) - MSIMEPointerMenuPadding - MSIMEPointerMenuTextInset;
    NSString *key = MSIMEPointerMenuKeyEquivalentText(self.item);
    if (key.length) {
        NSDictionary *keyAttributes = MSIMEPointerMenuTextAttributes(lit ? color : NSColor.secondaryLabelColor);
        const NSSize size = [key sizeWithAttributes:keyAttributes];
        [key drawAtPoint:NSMakePoint(right - size.width, floor((NSHeight(bounds) - size.height) / 2.0)) withAttributes:keyAttributes];
    } else if (self.item.submenu) {
        NSBezierPath *chevron = [NSBezierPath bezierPath];
        const CGFloat x = right - 4.0, y = NSMidY(bounds);
        [chevron moveToPoint:NSMakePoint(x - 3.5, y - 4.5)];
        [chevron lineToPoint:NSMakePoint(x + 0.5, y)];
        [chevron lineToPoint:NSMakePoint(x - 3.5, y + 4.5)];
        chevron.lineWidth = 1.5;
        chevron.lineCapStyle = NSLineCapStyleRound;
        chevron.lineJoinStyle = NSLineJoinStyleRound;
        [color setStroke];
        [chevron stroke];
    }
}
@end

@implementation MSIMEPointerMenu
{
    NSMenu *_menu;
    NSWindowLevel _level;
    MSIMEPointerMenuWindow *_rootWindow;
    MSIMEPointerMenuWindow *_submenuWindow;
    MSIMEPointerMenuRow *_submenuParent;
    id _localMonitor;
    id _globalMonitor;
}

- (void)dealloc
{
    [self dismiss];
}

- (BOOL)isVisible
{
    return _rootWindow != nil;
}

- (NSArray<NSWindow *> *)windows
{
    NSMutableArray<NSWindow *> *windows = [NSMutableArray array];
    if (_rootWindow) [windows addObject:_rootWindow];
    if (_submenuWindow) [windows addObject:_submenuWindow];
    return windows;
}

static NSImage *MSIMEPointerMenuMask(CGFloat radius)
{
    const CGFloat edge = radius * 2.0 + 1.0;
    NSImage *mask = [NSImage imageWithSize:NSMakeSize(edge, edge) flipped:NO drawingHandler:^BOOL(NSRect rect) {
        [NSColor.blackColor setFill];
        [[NSBezierPath bezierPathWithRoundedRect:rect xRadius:radius yRadius:radius] fill];
        return YES;
    }];
    mask.capInsets = NSEdgeInsetsMake(radius, radius, radius, radius);
    mask.resizingMode = NSImageResizingModeStretch;
    return mask;
}

static NSScreen *MSIMEPointerMenuScreen(NSPoint point)
{
    for (NSScreen *screen in NSScreen.screens)
        if (NSPointInRect(point, screen.frame)) return screen;
    return NSScreen.mainScreen;
}

- (MSIMEPointerMenuWindow *)windowForMenu:(NSMenu *)menu
{
    BOOL reservesCheckColumn = NO;
    for (NSMenuItem *item in menu.itemArray)
        if (!item.hidden && item.state != NSControlStateValueOff) reservesCheckColumn = YES;
    NSMutableArray<MSIMEPointerMenuRow *> *rows = [NSMutableArray array];
    CGFloat width = MSIMEPointerMenuMinimumWidth, height = MSIMEPointerMenuPadding;
    for (NSMenuItem *item in menu.itemArray) {
        if (item.hidden) continue;
        MSIMEPointerMenuRow *row = [[MSIMEPointerMenuRow alloc] initWithItem:item owner:self];
        row.reservesCheckColumn = reservesCheckColumn;
        width = MAX(width, row.preferredWidth);
        height += item.separatorItem ? MSIMEPointerMenuSeparatorHeight : MSIMEPointerMenuRowHeight;
        [rows addObject:row];
    }
    height += MSIMEPointerMenuPadding;
    NSVisualEffectView *background = [[NSVisualEffectView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
    background.material = NSVisualEffectMaterialMenu;
    background.blendingMode = NSVisualEffectBlendingModeBehindWindow;
    background.state = NSVisualEffectStateActive;
    background.maskImage = MSIMEPointerMenuMask(MSIMEPointerMenuCornerRadius);
    CGFloat top = height - MSIMEPointerMenuPadding;
    for (MSIMEPointerMenuRow *row in rows) {
        const CGFloat rowHeight = row.item.separatorItem ? MSIMEPointerMenuSeparatorHeight : MSIMEPointerMenuRowHeight;
        top -= rowHeight;
        row.frame = NSMakeRect(0, top, width, rowHeight);
        [background addSubview:row];
    }
    MSIMEPointerMenuWindow *window = [[MSIMEPointerMenuWindow alloc] initWithContentRect:background.frame
        styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    window.opaque = NO;
    window.backgroundColor = NSColor.clearColor;
    window.hasShadow = YES;
    window.hidesOnDeactivate = NO;
    window.becomesKeyOnlyIfNeeded = YES;
    window.level = _level;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
    window.appearance = _menu.appearance;
    window.contentView = background;
    return window;
}

// 先贴住锚点的左上角，放不下时翻到另一侧，最后夹进屏幕的可见区域。
static NSRect MSIMEPointerMenuPlace(NSSize size, NSPoint anchor, CGFloat flippedX, NSRect visible)
{
    NSRect frame = NSMakeRect(anchor.x, anchor.y - size.height, size.width, size.height);
    if (NSMaxX(frame) > NSMaxX(visible)) frame.origin.x = flippedX - size.width;
    if (NSMinY(frame) < NSMinY(visible)) frame.origin.y = anchor.y;
    frame.origin.x = MAX(NSMinX(visible), MIN(NSMinX(frame), NSMaxX(visible) - size.width));
    frame.origin.y = MAX(NSMinY(visible), MIN(NSMinY(frame), NSMaxY(visible) - size.height));
    return frame;
}

- (void)popUpMenu:(NSMenu *)menu atScreenPoint:(NSPoint)screenPoint level:(NSWindowLevel)level
{
    [self dismiss];
    if (!menu.numberOfItems) return;
    _menu = menu;
    _level = level;
    _rootWindow = [self windowForMenu:menu];
    const NSRect visible = MSIMEPointerMenuScreen(screenPoint).visibleFrame;
    [_rootWindow setFrame:MSIMEPointerMenuPlace(_rootWindow.frame.size, screenPoint, screenPoint.x, visible) display:NO];
    [_rootWindow orderFrontRegardless];
    [_rootWindow invalidateShadow];
    __weak MSIMEPointerMenu *weakSelf = self;
    const NSEventMask clicks = NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown | NSEventMaskOtherMouseDown;
    // 点到菜单以外就收起；点击本身照常交给它落到的窗口，所以菜单开着时直接点另一个候选仍然选中它。
    _localMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:clicks handler:^NSEvent *(NSEvent *event) {
        MSIMEPointerMenu *menu = weakSelf;
        if (menu && ![menu.windows containsObject:event.window]) [menu dismiss];
        return event;
    }];
    _globalMonitor = [NSEvent addGlobalMonitorForEventsMatchingMask:clicks handler:^(NSEvent *event) {
        (void)event;
        [weakSelf dismiss];
    }];
}

- (void)dismiss
{
    if (_localMonitor) [NSEvent removeMonitor:_localMonitor];
    if (_globalMonitor) [NSEvent removeMonitor:_globalMonitor];
    _localMonitor = nil;
    _globalMonitor = nil;
    [self closeSubmenu];
    [_rootWindow orderOut:nil];
    _rootWindow = nil;
    _menu = nil;
}

- (void)closeSubmenu
{
    [_submenuWindow orderOut:nil];
    _submenuWindow = nil;
    _submenuParent.highlighted = NO;
    _submenuParent = nil;
}

- (void)openSubmenuForRow:(MSIMEPointerMenuRow *)row
{
    if (_submenuParent == row) return;
    [self closeSubmenu];
    if (!row.item.submenu.numberOfItems) return;
    _submenuParent = row;
    _submenuWindow = [self windowForMenu:row.item.submenu];
    const NSRect rowFrame = [_rootWindow convertRectToScreen:[row convertRect:row.bounds toView:nil]];
    const NSRect parent = _rootWindow.frame;
    const NSPoint anchor = NSMakePoint(NSMaxX(parent) - 3.0, NSMaxY(rowFrame) + MSIMEPointerMenuPadding);
    [_submenuWindow setFrame:MSIMEPointerMenuPlace(_submenuWindow.frame.size, anchor, NSMinX(parent) + 3.0,
        MSIMEPointerMenuScreen(anchor).visibleFrame) display:NO];
    [_submenuWindow orderFrontRegardless];
    [_submenuWindow invalidateShadow];
}

- (void)rowHovered:(NSView *)view
{
    MSIMEPointerMenuRow *row = (MSIMEPointerMenuRow *)view;
    if (!row.selectable) return;
    for (MSIMEPointerMenuRow *sibling in row.superview.subviews)
        if ([sibling isKindOfClass:MSIMEPointerMenuRow.class] && sibling != row) sibling.highlighted = NO;
    row.highlighted = YES;
    if (row.window != _rootWindow) {
        _submenuParent.highlighted = YES;
        return;
    }
    if (row.item.submenu) [self openSubmenuForRow:row];
    else [self closeSubmenu];
}

- (void)rowExited:(NSView *)view
{
    MSIMEPointerMenuRow *row = (MSIMEPointerMenuRow *)view;
    // 指针移进子菜单时，打开它的那一行保持高亮。
    if (row != _submenuParent) row.highlighted = NO;
}

- (void)rowClicked:(NSView *)view
{
    MSIMEPointerMenuRow *row = (MSIMEPointerMenuRow *)view;
    if (!row.selectable) return;
    if (row.item.submenu) {
        [self openSubmenuForRow:row];
        return;
    }
    NSMenuItem *item = row.item;
    [self dismiss];
    if (item.action) [NSApp sendAction:item.action to:item.target from:item];
}
@end
