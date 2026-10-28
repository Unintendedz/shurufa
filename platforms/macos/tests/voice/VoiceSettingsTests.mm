// Validation of the voice form's stored configuration. validate: reads only the properties, so this never touches the Keychain or the defaults the form saves to.
#import "../../src/voice/VoiceProviderSettings.h"
#import "../../src/settings/SettingsLayout.h"
#include <cstdio>
#include <cstdlib>

namespace
{
void Require(bool value, const char *message)
{
    if (!value)
    {
        std::fprintf(stderr, "%s\n", message);
        std::exit(1);
    }
}
} // namespace
int main(int argc, const char **argv)
{
    @autoreleasepool
    {
        MetasequoiaVoiceProviderSettings *settings = [MetasequoiaVoiceProviderSettings new];
        settings.provider = @"openai";
        settings.endpoint = @"https://example.test/v1/audio/transcriptions";
        settings.model = @"fixture-model";
        settings.token = @"fixture-token";
        settings.modelPath = @"";
        settings.polishEnabled = NO;
        settings.polishEndpoint = @"";
        settings.polishModel = @"";
        settings.polishToken = @"";
        Require([settings validate:nil], "valid cloud configuration rejected");
        for (NSString *invalid in @[
                 @"http://example.test/asr", @"https:///", @"https://user:password@example.test/asr",
                 @"https://example.test/asr#fragment", @"file:///tmp/asr"
             ])
        {
            settings.endpoint = invalid;
            NSError *error = nil;
            Require(![settings validate:&error] && error != nil, "unsafe endpoint accepted");
        }
        settings.endpoint = @"https://example.test/asr";
        settings.token = @"";
        Require(![settings validate:nil], "missing ASR token accepted");
        settings.token = @"fixture-token";
        settings.model = @"";
        Require(![settings validate:nil], "missing ASR model accepted");
        settings.model = @"fixture-model";
        // Doubao streams over a WebSocket, so an HTTPS address is not one it can use.
        settings.provider = @"doubao";
        Require(![settings validate:nil], "HTTPS endpoint accepted for the streaming provider");
        settings.endpoint = @"wss://example.test/asr";
        Require([settings validate:nil], "valid streaming configuration rejected");
        settings.provider = @"openai";
        settings.endpoint = @"https://example.test/asr";
        settings.polishEnabled = YES;
        Require(![settings validate:nil], "incomplete opt-in polish accepted");
        settings.polishEndpoint = @"https://polish.test/v1/chat/completions";
        settings.polishModel = @"fixture-model";
        settings.polishToken = @"fixture-token";
        Require([settings validate:nil], "valid polish rejected");
        settings.polishEnabled = NO;
        settings.provider = @"local";
        settings.token = @"";
        settings.modelPath = @"/nonexistent/msime-voice-model";
        Require(![settings validate:nil], "missing local model accepted");
        settings.modelPath = NSTemporaryDirectory();
        Require(![settings validate:nil], "directory without a model manifest accepted as model");
        NSString *path = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        Require([@"model fixture" writeToFile:path atomically:YES encoding:NSUTF8StringEncoding error:nil],
                "model fixture creation failed");
        settings.modelPath = path;
        Require(![settings validate:nil], "a model file accepted in place of an installed model directory");
        [[NSFileManager defaultManager] removeItemAtPath:path error:nil];
        Require([[NSFileManager defaultManager] createDirectoryAtPath:path withIntermediateDirectories:NO attributes:nil error:nil],
                "model directory fixture creation failed");
        Require([@"{}" writeToFile:[path stringByAppendingPathComponent:@"msime-model.json"] atomically:YES
                          encoding:NSUTF8StringEncoding error:nil], "model manifest fixture creation failed");
        settings.modelPath = path;
        Require([settings validate:nil], "local mode requires unused cloud credentials");
        [[NSFileManager defaultManager] removeItemAtPath:path error:nil];
        NSString *external = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        Require([[NSFileManager defaultManager] createDirectoryAtPath:external withIntermediateDirectories:NO attributes:nil error:nil],
                "external model directory creation failed");
        Require([@"{}" writeToFile:[external stringByAppendingPathComponent:@"msime-model.json"] atomically:YES
                          encoding:NSUTF8StringEncoding error:nil], "external manifest creation failed");
        NSString *linked = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        Require([[NSFileManager defaultManager] createSymbolicLinkAtPath:linked withDestinationPath:external error:nil],
                "linked model directory creation failed");
        settings.modelPath = linked;
        Require(![settings validate:nil], "symlinked model directory accepted");
        [[NSFileManager defaultManager] removeItemAtPath:linked error:nil];
        Require([[NSFileManager defaultManager] createDirectoryAtPath:linked withIntermediateDirectories:NO attributes:nil error:nil],
                "model directory creation failed");
        Require([[NSFileManager defaultManager] createSymbolicLinkAtPath:[linked stringByAppendingPathComponent:@"msime-model.json"]
                                                       withDestinationPath:[external stringByAppendingPathComponent:@"msime-model.json"] error:nil],
                "linked manifest creation failed");
        settings.modelPath = linked;
        Require(![settings validate:nil], "symlinked model manifest accepted");
        [[NSFileManager defaultManager] removeItemAtPath:linked error:nil];
        [[NSFileManager defaultManager] removeItemAtPath:external error:nil];
        settings.provider = @"unknown";
        Require(![settings validate:nil], "unknown provider accepted");
        settings.provider = @"textstream";
        settings.polishEnabled = YES;
        settings.polishToken = @"";
        Require([settings validate:nil], "pending TextStream polish blocks recognition settings");
        if (argc >= 2) {
            // 真实 AppKit 表单的离屏截图；使用合成偏好，不访问用户密钥或发送按键。
            [NSApplication sharedApplication];
            NSApp.appearance = [NSAppearance appearanceNamed:argc == 3 ? NSAppearanceNameAqua : NSAppearanceNameDarkAqua];
            NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
            [defaults setVolatileDomain:@{
                @"MSIMEClientVoiceASRProvider":@"textstream", @"MSIMEClientVoicePolishToken":@"synthetic-only", @"MSIMEClientVoicePolish":@YES,
                @"MSIMEClientVoicePolishTokens":@{@"deepseek":@"synthetic-only"},
                @"MSIMEClientVoiceHotkeyMode":@"toggle", @"MSIMEClientVoiceMacOSHotkey":@{@"key_code":@54, @"modifiers":@0}
            } forName:NSArgumentDomain];
            MetasequoiaVoiceProviderSettingsView *form = [[MetasequoiaVoiceProviderSettingsView alloc] initWithFrame:NSMakeRect(0, 0, 640, 1200)];
            NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 680, 1400)
                styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
            NSView *content = window.contentView;
            content.wantsLayer = YES;
            [NSApp.appearance performAsCurrentDrawingAppearance:^{ content.layer.backgroundColor = MSIMESettingsSurfaceColor().CGColor; }];
            [content addSubview:form];
            [NSLayoutConstraint activateConstraints:@[
                [form.leadingAnchor constraintEqualToAnchor:content.leadingAnchor constant:20],
                [form.trailingAnchor constraintEqualToAnchor:content.trailingAnchor constant:-20],
                [form.topAnchor constraintEqualToAnchor:content.topAnchor constant:20]
            ]];
            [content layoutSubtreeIfNeeded];
            [window setContentSize:NSMakeSize(680, form.fittingSize.height + 40)];
            [content layoutSubtreeIfNeeded];
            [content display];
            NSBitmapImageRep *image = [content bitmapImageRepForCachingDisplayInRect:content.bounds];
            [content cacheDisplayInRect:content.bounds toBitmapImageRep:image];
            Require([[image representationUsingType:NSBitmapImageFileTypePNG properties:@{}]
                writeToFile:[NSString stringWithUTF8String:argv[1]] atomically:YES], "snapshot export failed");
        }
    }
    return 0;
}
