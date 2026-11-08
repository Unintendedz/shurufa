#import <AppKit/AppKit.h>
#import "../../src/candidate/CandidatePointerMenu.h"

#include <cassert>

@interface PointerMenuRecorder : NSObject
@property(nonatomic) NSInteger lastTag;
@property(nonatomic) NSUInteger calls;
@end
@implementation PointerMenuRecorder
- (void)perform:(NSMenuItem *)item
{
    self.lastTag = item.tag;
    ++self.calls;
}
@end

static NSMenuItem *Item(NSString *title, NSInteger tag, PointerMenuRecorder *target)
{
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:@selector(perform:) keyEquivalent:@""];
    item.target = target;
    item.tag = tag;
    return item;
}

static NSArray<MSIMEPointerMenuRow *> *Rows(NSWindow *window)
{
    NSMutableArray<MSIMEPointerMenuRow *> *rows = [NSMutableArray array];
    for (NSView *view in window.contentView.subviews)
        if ([view isKindOfClass:MSIMEPointerMenuRow.class]) [rows addObject:(MSIMEPointerMenuRow *)view];
    return rows;
}

static NSEvent *PointerEvent(NSEventType type, NSView *view)
{
    const NSPoint center = [view convertPoint:NSMakePoint(NSMidX(view.bounds), NSMidY(view.bounds)) toView:nil];
    if (type == NSEventTypeMouseExited)
        return [NSEvent enterExitEventWithType:type location:center modifierFlags:0
            timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:view.window.windowNumber
            context:nil eventNumber:0 trackingNumber:0 userData:nullptr];
    return [NSEvent mouseEventWithType:type location:center modifierFlags:0
        timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:view.window.windowNumber
        context:nil eventNumber:0 clickCount:1 pressure:1];
}

int main()
{
    @autoreleasepool
    {
        [NSApplication sharedApplication];
        PointerMenuRecorder *recorder = [PointerMenuRecorder new];
        NSMenu *menu = [[NSMenu alloc] initWithTitle:@"候选操作"];
        menu.autoenablesItems = NO;
        [menu addItem:Item(@"置顶", 0, recorder)];
        NSMenuItem *fixed = [[NSMenuItem alloc] initWithTitle:@"固定排位" action:nil keyEquivalent:@""];
        NSMenu *positions = [[NSMenu alloc] initWithTitle:@"固定排位"];
        positions.autoenablesItems = NO;
        for (NSInteger position = 1; position <= 5; ++position)
            [positions addItem:Item([NSString stringWithFormat:@"第 %ld 位", (long)position], 10 + position, recorder)];
        [positions addItem:NSMenuItem.separatorItem];
        [positions addItem:Item(@"取消固定", 2, recorder)];
        fixed.submenu = positions;
        [menu addItem:fixed];
        NSMenuItem *disabled = Item(@"不可用", 7, recorder);
        disabled.enabled = NO;
        [menu addItem:disabled];
        NSMenuItem *remove = Item(@"删除", 1, recorder);
        remove.keyEquivalent = @"2";
        remove.keyEquivalentModifierMask = NSEventModifierFlagControl | NSEventModifierFlagOption | NSEventModifierFlagShift;
        [menu addItem:remove];
        assert([MSIMEPointerMenuKeyEquivalentText(remove) isEqual:@"⌃⌥⇧2"]);
        assert([MSIMEPointerMenuKeyEquivalentText(fixed) isEqual:@""]);

        // 菜单窗口永远不能成为 key：这正是它和 NSMenu 的区别，客户端窗口的键盘焦点不会被拿走。
        MSIMEPointerMenu *pointerMenu = [MSIMEPointerMenu new];
        const NSRect visible = NSScreen.mainScreen.visibleFrame;
        const NSPoint anchor = NSMakePoint(NSMidX(visible), NSMidY(visible));
        [pointerMenu popUpMenu:menu atScreenPoint:anchor level:NSPopUpMenuWindowLevel];
        assert(pointerMenu.visible && pointerMenu.windows.count == 1);
        NSWindow *root = pointerMenu.windows[0];
        assert(!root.canBecomeKeyWindow && !root.canBecomeMainWindow);
        assert(root.styleMask & NSWindowStyleMaskNonactivatingPanel);
        assert(root.isVisible && root.level == NSPopUpMenuWindowLevel);
        assert(NSMinX(root.frame) == anchor.x && NSMaxY(root.frame) == anchor.y);
        NSArray<MSIMEPointerMenuRow *> *rows = Rows(root);
        assert(rows.count == 4);

        // 指针移到「固定排位」打开子菜单，子菜单同样不能成为 key，开在根菜单右侧。
        [rows[1] mouseMoved:PointerEvent(NSEventTypeMouseMoved, rows[1])];
        assert(pointerMenu.windows.count == 2 && rows[1].highlighted);
        NSWindow *submenu = pointerMenu.windows[1];
        assert(!submenu.canBecomeKeyWindow && submenu.isVisible && NSMinX(submenu.frame) > NSMinX(root.frame));
        NSArray<MSIMEPointerMenuRow *> *positionRows = Rows(submenu);
        assert(positionRows.count == 7);
        [positionRows[2] mouseMoved:PointerEvent(NSEventTypeMouseMoved, positionRows[2])];
        assert(positionRows[2].highlighted && rows[1].highlighted);
        [rows[1] mouseExited:PointerEvent(NSEventTypeMouseExited, rows[1])];
        assert(rows[1].highlighted);
        // 子菜单的分隔线不响应。
        [positionRows[5] mouseUp:PointerEvent(NSEventTypeLeftMouseUp, positionRows[5])];
        assert(recorder.calls == 0 && pointerMenu.visible);
        [positionRows[2] mouseUp:PointerEvent(NSEventTypeLeftMouseUp, positionRows[2])];
        assert(recorder.calls == 1 && recorder.lastTag == 13);
        assert(!pointerMenu.visible && pointerMenu.windows.count == 0 && !root.isVisible && !submenu.isVisible);

        // 指针移到别的行收起子菜单；不可用的行既不高亮也不派发；右键松开和左键一样选中。
        [pointerMenu popUpMenu:menu atScreenPoint:anchor level:NSPopUpMenuWindowLevel];
        rows = Rows(pointerMenu.windows[0]);
        [rows[1] mouseMoved:PointerEvent(NSEventTypeMouseMoved, rows[1])];
        assert(pointerMenu.windows.count == 2);
        [rows[0] mouseMoved:PointerEvent(NSEventTypeMouseMoved, rows[0])];
        assert(pointerMenu.windows.count == 1 && rows[0].highlighted && !rows[1].highlighted);
        [rows[2] mouseMoved:PointerEvent(NSEventTypeMouseMoved, rows[2])];
        [rows[2] mouseUp:PointerEvent(NSEventTypeLeftMouseUp, rows[2])];
        assert(!rows[2].highlighted && recorder.calls == 1 && pointerMenu.visible);
        [rows[3] rightMouseUp:PointerEvent(NSEventTypeRightMouseUp, rows[3])];
        assert(recorder.calls == 2 && recorder.lastTag == 1 && !pointerMenu.visible);

        // 贴着屏幕右下角打开时翻到指针的左上方，整个菜单留在可见区域里。
        const NSPoint corner = NSMakePoint(NSMaxX(visible) - 2, NSMinY(visible) + 2);
        [pointerMenu popUpMenu:menu atScreenPoint:corner level:NSPopUpMenuWindowLevel];
        root = pointerMenu.windows[0];
        assert(NSContainsRect(visible, root.frame));
        assert(NSMaxX(root.frame) == corner.x && NSMinY(root.frame) == corner.y);
        rows = Rows(root);
        [rows[1] mouseMoved:PointerEvent(NSEventTypeMouseMoved, rows[1])];
        submenu = pointerMenu.windows[1];
        assert(NSContainsRect(visible, submenu.frame) && NSMaxX(submenu.frame) < NSMaxX(root.frame));
        [pointerMenu dismiss];
        assert(!pointerMenu.visible && !root.isVisible && !submenu.isVisible);

        // 空菜单不开窗口。
        [pointerMenu popUpMenu:[[NSMenu alloc] initWithTitle:@""] atScreenPoint:anchor level:NSPopUpMenuWindowLevel];
        assert(!pointerMenu.visible);
    }
    return 0;
}
