#import "../../src/settings/ShurufaPreferenceMigration.h"
#include <cassert>

int main() {
    @autoreleasepool {
        NSString *suite = [@"shurufa.synthetic.migration." stringByAppendingString:NSUUID.UUID.UUIDString];
        NSUserDefaults *defaults = [[NSUserDefaults alloc] initWithSuiteName:suite];
        __block NSDictionary *document = @{@"format_version":@1, @"revision":@7,
            @"preferences":@{@"scheme":@"shuangpin", @"voice_input":@{@"enabled":@YES},
                @"keybindings":@{@"switch_language_caps_lock":@YES, @"switch_language_shift":@NO}}};
        __block NSUInteger saves = 0;
        auto load = ^NSDictionary *{ return document; };
        auto save = ^NSDictionary *(uint64_t revision, NSDictionary *value) {
            assert(revision == 7);
            ++saves;
            document = value;
            return value;
        };
        [defaults setBool:YES forKey:@"ShurufaCapsLockMigrationV1"];
        [defaults setBool:YES forKey:@"MSIMEClientCapsLockTapShortcut"];
        // 保存失败时不标记完成，原有偏好和本地镜像都不改。
        assert(!MSIMEMigrateShurufaCapsLock(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert(![defaults boolForKey:@"ShurufaCapsLockMigrationV2"]);
        assert([defaults boolForKey:@"MSIMEClientCapsLockTapShortcut"]);
        assert(MSIMEMigrateShurufaCapsLock(defaults, load, save));
        assert(saves == 1 && ![defaults boolForKey:@"MSIMEClientCapsLockTapShortcut"]);
        NSDictionary *preferences = document[@"preferences"];
        assert([preferences[@"scheme"] isEqual:@"shuangpin"]);
        assert([preferences[@"voice_input"] isEqual:@{@"enabled":@YES}]);
        assert([preferences[@"keybindings"][@"switch_language_caps_lock"] isEqual:@NO]);
        assert([preferences[@"keybindings"][@"switch_language_shift"] isEqual:@NO]);
        // 重复安装不重复保存。
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
        document = @{@"revision":@7,@"preferences":@{@"quanpin_helpcode":@{@"enabled":@YES,@"schema":@"ziranma"},@"traditional_chinese_output":@YES}};
        [defaults setBool:YES forKey:@"MSIMEClientQuanpinHelpcodeEnabled"];
        saves = 0;
        assert(!MSIMEMigrateShurufaQuanpinHelpcode(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert([defaults boolForKey:@"MSIMEClientQuanpinHelpcodeEnabled"] && ![defaults boolForKey:@"ShurufaQuanpinHelpcodeMigrationV1"]);
        assert(MSIMEMigrateShurufaQuanpinHelpcode(defaults, load, save));
        assert(![defaults boolForKey:@"MSIMEClientQuanpinHelpcodeEnabled"] && saves == 1);
        assert([document[@"preferences"][@"quanpin_helpcode"][@"enabled"] isEqual:@NO]);
        assert([document[@"preferences"][@"quanpin_helpcode"][@"schema"] isEqual:@"ziranma"]);
        assert([document[@"preferences"][@"traditional_chinese_output"] isEqual:@YES]);
        [defaults setBool:YES forKey:@"MSIMEClientQuanpinHelpcodeEnabled"];
        assert(MSIMEMigrateShurufaQuanpinHelpcode(defaults, load, save) && saves == 1);
        assert([defaults boolForKey:@"MSIMEClientQuanpinHelpcodeEnabled"]);
        // 首次安装将默认/none 的 grammar_model 迁至 wanxiang；若已有自定义非 none 模型则保留；迁移完成后尊重用户关闭选择。
        [defaults removePersistentDomainForName:suite];
        document = @{@"revision":@7,@"preferences":@{@"sentence_association":@{@"grammar_model":@"none",@"word_lattice":@YES}}};
        [defaults setBool:NO forKey:@"MSIMEClientSentenceAssociationWanxiangGrammar"];
        saves = 0;
        assert(!MSIMEMigrateShurufaGrammarModel(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert(![defaults boolForKey:@"MSIMEClientSentenceAssociationWanxiangGrammar"] && ![defaults boolForKey:@"ShurufaGrammarModelMigrationV1"]);
        assert(MSIMEMigrateShurufaGrammarModel(defaults, load, save));
        assert([defaults boolForKey:@"MSIMEClientSentenceAssociationWanxiangGrammar"] && saves == 1);
        assert([document[@"preferences"][@"sentence_association"][@"grammar_model"] isEqual:@"wanxiang"]);
        assert([document[@"preferences"][@"sentence_association"][@"word_lattice"] isEqual:@YES]);
        // 用户随后手动关闭（none）时，重复运行迁移不再改回 wanxiang。
        document = @{@"revision":@7,@"preferences":@{@"sentence_association":@{@"grammar_model":@"none"}}};
        [defaults setBool:NO forKey:@"MSIMEClientSentenceAssociationWanxiangGrammar"];
        assert(MSIMEMigrateShurufaGrammarModel(defaults, load, save) && saves == 1);
        assert(![defaults boolForKey:@"MSIMEClientSentenceAssociationWanxiangGrammar"]);
        assert([document[@"preferences"][@"sentence_association"][@"grammar_model"] isEqual:@"none"]);
        // 已有手工配置（如 baguwen_compact）时迁移保持原模型且本地万象开关为 NO。
        [defaults removePersistentDomainForName:suite];
        document = @{@"revision":@7,@"preferences":@{@"sentence_association":@{@"grammar_model":@"baguwen_compact"}}};
        saves = 0;
        assert(MSIMEMigrateShurufaGrammarModel(defaults, load, save) && saves == 1);
        assert(![defaults boolForKey:@"MSIMEClientSentenceAssociationWanxiangGrammar"]);
        assert([document[@"preferences"][@"sentence_association"][@"grammar_model"] isEqual:@"baguwen_compact"]);
        // 内联 Emoji 一次性启用迁移：将旧 false 迁至 true，之后用户手动关闭不再改回。
        [defaults removePersistentDomainForName:suite];
        document = @{@"revision":@7,@"preferences":@{@"mixed_input":@{@"emoji":@NO,@"english":@YES}}};
        saves = 0;
        assert(!MSIMEMigrateShurufaInlineEmoji(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert(![defaults boolForKey:@"ShurufaInlineEmojiMigrationV1"]);
        assert(MSIMEMigrateShurufaInlineEmoji(defaults, load, save));
        assert(saves == 1 && [document[@"preferences"][@"mixed_input"][@"emoji"] isEqual:@YES]);
        assert([document[@"preferences"][@"mixed_input"][@"english"] isEqual:@YES]);
        assert([[defaults dictionaryForKey:@"MSIMEClientMixedInput"][@"emoji"] isEqual:@YES]);
        document = @{@"revision":@7,@"preferences":@{@"mixed_input":@{@"emoji":@NO}}};
        assert(MSIMEMigrateShurufaInlineEmoji(defaults, load, save) && saves == 1);
        assert([document[@"preferences"][@"mixed_input"][@"emoji"] isEqual:@NO]);
        [defaults removePersistentDomainForName:suite];
    }
}
