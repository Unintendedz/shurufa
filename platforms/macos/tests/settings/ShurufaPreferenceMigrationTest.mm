#import "../../src/settings/ShurufaPreferenceMigration.h"
#include <cassert>

int main() {
    @autoreleasepool {
        NSString *suite = [@"shurufa.synthetic.migration." stringByAppendingString:NSUUID.UUID.UUIDString];
        NSUserDefaults *defaults = [[NSUserDefaults alloc] initWithSuiteName:suite];
        __block NSDictionary *document = @{@"format_version":@1, @"revision":@7,
            @"preferences":@{@"scheme":@"shuangpin", @"voice_input":@{@"enabled":@YES},
                @"keybindings":@{@"switch_language_caps_lock":@NO, @"switch_language_shift":@NO}}};
        __block NSUInteger saves = 0;
        auto load = ^NSDictionary *{ return document; };
        auto save = ^NSDictionary *(uint64_t revision, NSDictionary *value) {
            assert(revision == 7);
            ++saves;
            document = value;
            return value;
        };
        // 保存失败时不标记完成，原有偏好和本地镜像都不改。
        assert(!MSIMEMigrateShurufaCapsLock(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert(![defaults boolForKey:@"ShurufaCapsLockMigrationV1"]);
        assert(MSIMEMigrateShurufaCapsLock(defaults, load, save));
        assert(saves == 1 && [defaults boolForKey:@"MSIMEClientCapsLockTapShortcut"]);
        NSDictionary *preferences = document[@"preferences"];
        assert([preferences[@"scheme"] isEqual:@"shuangpin"]);
        assert([preferences[@"voice_input"] isEqual:@{@"enabled":@YES}]);
        assert([preferences[@"keybindings"][@"switch_language_caps_lock"] isEqual:@YES]);
        assert([preferences[@"keybindings"][@"switch_language_shift"] isEqual:@NO]);
        // 用户之后关闭，重复安装不能再次覆盖。
        [defaults setBool:NO forKey:@"MSIMEClientCapsLockTapShortcut"];
        assert(MSIMEMigrateShurufaCapsLock(defaults, load, save));
        assert(saves == 1 && ![defaults boolForKey:@"MSIMEClientCapsLockTapShortcut"]);
        for (NSDictionary *test in @[
            @{@"voice":@{},@"key":@61,@"modifiers":@0,@"mode":@"hold"},
            @{@"voice":@{@"macos_hotkey":@{@"key_code":@123,@"modifiers":@2},@"hotkey_mode":@"toggle"},@"key":@123,@"modifiers":@2,@"mode":@"toggle"},
            @{@"voice":@{@"macos_hotkey":@{@"key_code":@123,@"modifiers":@2},@"hotkey_mode":@"legacy"},@"key":@123,@"modifiers":@2,@"mode":@"toggle"},
            @{@"voice":@{@"macos_hotkey":@{@"key_code":@54,@"modifiers":@0},@"hotkey_mode":@"legacy"},@"key":@54,@"modifiers":@0,@"mode":@"hold"}
        ]) {
            [defaults removePersistentDomainForName:suite];
            document = @{@"revision":@7,@"preferences":@{@"voice_input":test[@"voice"]}};
            saves = 0;
            assert(!MSIMEMigrateShurufaVoiceShortcut(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
            assert(![defaults boolForKey:@"ShurufaVoiceShortcutMigrationV2"]);
            assert(MSIMEMigrateShurufaVoiceShortcut(defaults, load, save));
            NSDictionary *voice = document[@"preferences"][@"voice_input"];
            assert([voice[@"macos_hotkey"][@"key_code"] isEqual:test[@"key"]]);
            assert([voice[@"macos_hotkey"][@"modifiers"] isEqual:test[@"modifiers"]]);
            assert([voice[@"hotkey_mode"] isEqual:test[@"mode"]]);
            for (NSString *key in @[@"hotkey_ctrl_f9",@"hotkey_ralt",@"hotkey_ctrl_win",@"hotkey_rctrl_ralt"])
                assert([voice[key] isEqual:@NO]);
            assert(MSIMEMigrateShurufaVoiceShortcut(defaults, load, save) && saves == 1);
        }
        [defaults removePersistentDomainForName:suite];
        document = @{@"revision":@7,@"preferences":@{@"voice_input":@{@"stream_inline_preedit":@YES,@"asr_provider":@"textstream"}}};
        [defaults setBool:YES forKey:@"MSIMEClientVoiceStreamInlinePreedit"];
        saves = 0;
        assert(!MSIMEMigrateShurufaVoicePresentation(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert([defaults boolForKey:@"MSIMEClientVoiceStreamInlinePreedit"] && ![defaults boolForKey:@"ShurufaVoiceOverlayMigrationV1"]);
        assert(MSIMEMigrateShurufaVoicePresentation(defaults, load, save));
        assert(![defaults boolForKey:@"MSIMEClientVoiceStreamInlinePreedit"] && saves == 1);
        assert([document[@"preferences"][@"voice_input"][@"stream_inline_preedit"] isEqual:@NO]);
        assert([document[@"preferences"][@"voice_input"][@"asr_provider"] isEqual:@"textstream"]);
        [defaults setBool:YES forKey:@"MSIMEClientVoiceStreamInlinePreedit"];
        assert(MSIMEMigrateShurufaVoicePresentation(defaults, load, save) && saves == 1);
        assert([defaults boolForKey:@"MSIMEClientVoiceStreamInlinePreedit"]);
        [defaults removePersistentDomainForName:suite];
    }
}
