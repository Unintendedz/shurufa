#import "../../src/core/WindowPresentation.h"
#import <objc/runtime.h>
#include <cassert>

// 只在测试进程内使用独立剪贴板，不读取或覆盖用户的系统剪贴板。
@interface SettingsPasteboardFixture : NSObject
+ (NSPasteboard *)generalPasteboard;
@end
@implementation SettingsPasteboardFixture
+ (NSPasteboard *)generalPasteboard {
    static NSPasteboard *board = [NSPasteboard pasteboardWithUniqueName];
    return board;
}
@end

// 无需系统授予测试进程前台焦点；仅指定进程内的响应链窗口。
@interface SettingsTestApplication : NSApplication
@property NSWindow *fixtureWindow;
@end
@implementation SettingsTestApplication
- (NSWindow *)keyWindow { return self.fixtureWindow; }
- (NSWindow *)mainWindow { return self.fixtureWindow; }
@end

static NSEvent *Key(NSString *text, unsigned short code) {
    return [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint
        modifierFlags:NSEventModifierFlagCommand timestamp:0 windowNumber:NSApp.keyWindow.windowNumber
        context:nil characters:text charactersIgnoringModifiers:text isARepeat:NO keyCode:code];
}

int main() {
    @autoreleasepool {
        [SettingsTestApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        method_exchangeImplementations(class_getClassMethod(NSPasteboard.class, @selector(generalPasteboard)),
            class_getClassMethod(SettingsPasteboardFixture.class, @selector(generalPasteboard)));
        MSIMEInstallSettingsEditMenu();
        MSIMEInstallSettingsEditMenu();
        assert(NSApp.mainMenu.numberOfItems == 1);
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 400, 180)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        ((SettingsTestApplication *)NSApp).fixtureWindow = window;
        for (Class type in @[NSTextField.class, NSSecureTextField.class]) {
            NSTextField *field = [[type alloc] initWithFrame:NSMakeRect(10, 20, 380, 28)];
            [window.contentView addSubview:field];
            [window makeFirstResponder:field];
            NSTextView *editor = (NSTextView *)window.firstResponder;
            assert([editor isKindOfClass:NSTextView.class]);
            editor.allowsUndo = YES;
            [[NSPasteboard generalPasteboard] clearContents];
            [[NSPasteboard generalPasteboard] setString:@"synthetic-value" forType:NSPasteboardTypeString];
            assert([NSApp.mainMenu performKeyEquivalent:Key(@"v", 9)]);
            assert([editor.string isEqual:@"synthetic-value"]);
            assert([NSApp.mainMenu performKeyEquivalent:Key(@"a", 0)]);
            assert(editor.selectedRange.length == editor.string.length);
            if (type == NSTextField.class) {
                assert([NSApp.mainMenu performKeyEquivalent:Key(@"c", 8)]);
                while (editor.undoManager.groupingLevel > 0) [editor.undoManager endUndoGrouping];
                [editor.undoManager beginUndoGrouping];
                assert([NSApp.mainMenu performKeyEquivalent:Key(@"x", 7)]);
                [editor.undoManager endUndoGrouping];
                assert(editor.string.length == 0);
                assert([NSApp.mainMenu performKeyEquivalent:Key(@"z", 6)]);
                assert([editor.string isEqual:@"synthetic-value"]);
            }
            // 右键菜单中的粘贴走同一个标准响应链，安全文本框也保留粘贴能力。
            NSMenu *context = [editor menuForEvent:[NSEvent mouseEventWithType:NSEventTypeRightMouseDown
                location:NSMakePoint(20, 30) modifierFlags:0 timestamp:0 windowNumber:window.windowNumber
                context:nil eventNumber:1 clickCount:1 pressure:0]];
            BOOL hasPaste = NO;
            for (NSMenuItem *item in context.itemArray) if (item.action == @selector(paste:)) hasPaste = YES;
            assert(hasPaste);
            [window makeFirstResponder:nil];
            [field removeFromSuperview];
        }
        [window orderOut:nil];
        [[NSPasteboard generalPasteboard] releaseGlobally];
    }
    return 0;
}
