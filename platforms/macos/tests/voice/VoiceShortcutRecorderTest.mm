#import "../../src/voice/VoiceProviderSettings.h"
#include <cassert>

@interface RecorderApplication : NSApplication
@property NSWindow *fixtureWindow;
@end
@implementation RecorderApplication
- (NSWindow *)keyWindow { return self.fixtureWindow; }
@end

@interface MetasequoiaVoiceProviderSettingsView (RecorderTest)
- (void)recordHotkey:(id)sender;
@end
@interface RecorderView : MetasequoiaVoiceProviderSettingsView
@property NSDictionary *savedPatch;
@property BOOL didCommit;
@end
@implementation RecorderView
- (BOOL)saveHotkeyPatch:(NSDictionary *)patch { self.savedPatch = patch; return YES; }
- (void)commit:(id)sender { (void)sender; self.didCommit = YES; }
@end

// 只投递到本测试的 NSApplication，不使用 CGEvent 或系统键盘模拟。
static void Deliver(NSEventType type, unsigned short code, NSEventModifierFlags flags, NSWindow *window) {
    NSEvent *event = [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:flags
        timestamp:0 windowNumber:window.windowNumber context:nil characters:@"" charactersIgnoringModifiers:@""
        isARepeat:NO keyCode:code];
    [NSApp postEvent:event atStart:YES];
    NSEvent *queued = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate dateWithTimeIntervalSinceNow:0.05]
        inMode:NSDefaultRunLoopMode dequeue:YES];
    if (queued) [NSApp sendEvent:queued];
}

int main() {
    @autoreleasepool {
        [RecorderApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSUserDefaults.standardUserDefaults setVolatileDomain:@{
            @"MSIMEClientVoiceASRProvider":@"textstream",
            @"MSIMEClientVoicePolishTokens":@{@"deepseek":@"synthetic-only"}
        } forName:NSArgumentDomain];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,680,1200)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        ((RecorderApplication *)NSApp).fixtureWindow = window;
        RecorderView *form = [[RecorderView alloc] initWithFrame:window.contentView.bounds];
        [window.contentView addSubview:form];
        [form recordHotkey:nil];
        Deliver(NSEventTypeFlagsChanged,54,NSEventModifierFlagCommand,window);
        assert(form.savedPatch == nil);
        Deliver(NSEventTypeFlagsChanged,54,0,window);
        assert(([form.savedPatch[@"macos_hotkey"] isEqual:@{@"key_code":@54,@"modifiers":@0}]));
        form.savedPatch = nil;
        [form recordHotkey:nil];
        Deliver(NSEventTypeKeyDown,9,NSEventModifierFlagControl | NSEventModifierFlagOption,window);
        assert(([form.savedPatch[@"macos_hotkey"] isEqual:@{@"key_code":@9,@"modifiers":@3}]));
        form.savedPatch = nil;
        [form recordHotkey:nil];
        Deliver(NSEventTypeKeyDown,53,0,window);
        assert(form.savedPatch == nil && [form valueForKey:@"hotkeyMonitor"] == nil);
        // 无效地址在联网前拒绝，按钮能恢复，手动填写和选中模型仍走原保存入口。
        NSTextField *endpoint = [form valueForKey:@"polishEndpoint"];
        endpoint.stringValue = @"invalid-address";
        NSButton *fetch = [form valueForKey:@"fetchPolishModels"];
        [NSApp sendAction:fetch.action to:fetch.target from:fetch];
        assert(!fetch.enabled);
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:3];
        while (!fetch.enabled && deadline.timeIntervalSinceNow > 0)
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
        assert(fetch.enabled);
        assert([[[form valueForKey:@"polishNotice"] stringValue] containsString:@"服务地址或密钥格式不正确"]);
        NSTextField *model = [form valueForKey:@"polishModel"];
        assert(model.isEditable);
        NSMenuItem *choice = [[NSMenuItem alloc] initWithTitle:@"synthetic-model" action:@selector(selectPolishModel:) keyEquivalent:@""];
        choice.representedObject = @"synthetic-model";
        [NSApp sendAction:choice.action to:form from:choice];
        assert([model.stringValue isEqual:@"synthetic-model"] && form.didCommit);
        [form removeFromSuperview];
    }
    return 0;
}
