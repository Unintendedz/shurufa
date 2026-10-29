#pragma once
#import <Foundation/Foundation.h>
#import "../core/SharedVoicePreferences.h"

// 三档旧配置归一到单一绑定，已录制的键和已选择的 hold/toggle 保留。
static inline BOOL MSIMEMigrateShurufaVoiceShortcut(NSUserDefaults *defaults,
    NSDictionary *(^load)(void), NSDictionary *(^save)(uint64_t, NSDictionary *)) {
    NSString *marker = @"ShurufaVoiceShortcutMigrationV2";
    if ([defaults boolForKey:marker]) return YES;
    NSDictionary *snapshot = load();
    if (![snapshot[@"preferences"] isKindOfClass:NSDictionary.class] || ![snapshot[@"revision"] isKindOfClass:NSNumber.class]) return NO;
    NSMutableDictionary *updated = [snapshot mutableCopy];
    NSMutableDictionary *preferences = [snapshot[@"preferences"] mutableCopy];
    NSMutableDictionary *voice = [preferences[@"voice_input"] mutableCopy] ?: [NSMutableDictionary dictionary];
    id binding = voice[@"macos_hotkey"];
    if (!MSIMEVoiceShortcutValid(binding)) binding = [defaults objectForKey:@"MSIMEClientVoiceMacOSHotkey"];
    if (!MSIMEVoiceShortcutValid(binding)) binding = @{@"key_code":@61,@"modifiers":@0};
    NSString *mode = voice[@"hotkey_mode"] ?: [defaults stringForKey:@"MSIMEClientVoiceHotkeyMode"];
    if (![@[@"hold",@"toggle"] containsObject:mode ?: @""])
        mode = MSIMEVoiceModifierKey([binding[@"key_code"] unsignedShortValue]) && [binding[@"modifiers"] unsignedIntValue] == 0 ? @"hold" : @"toggle";
    voice[@"macos_hotkey"] = binding; voice[@"hotkey_mode"] = mode;
    for (NSString *field in @[@"hotkey_ctrl_f9",@"hotkey_ralt",@"hotkey_ctrl_win",@"hotkey_rctrl_ralt"]) voice[field] = @NO;
    preferences[@"voice_input"] = voice; updated[@"preferences"] = preferences;
    if (!save([snapshot[@"revision"] unsignedLongLongValue], updated)) return NO;
    MSIMEApplySharedVoicePreferences(voice, defaults);
    [defaults setBool:YES forKey:marker]; [defaults synchronize];
    return YES;
}

// 用户要求交还系统 Caps Lock；新标记覆盖曾经开启本功能的 V1 安装。
static inline BOOL MSIMEMigrateShurufaCapsLock(NSUserDefaults *defaults,
    NSDictionary *(^load)(void), NSDictionary *(^save)(uint64_t, NSDictionary *)) {
    NSString *marker = @"ShurufaCapsLockMigrationV2";
    if ([defaults boolForKey:marker]) return YES;
    NSDictionary *snapshot = load();
    if (![snapshot[@"preferences"] isKindOfClass:NSDictionary.class] ||
        ![snapshot[@"revision"] isKindOfClass:NSNumber.class]) return NO;
    NSMutableDictionary *updated = [snapshot mutableCopy];
    NSMutableDictionary *preferences = [snapshot[@"preferences"] mutableCopy];
    NSMutableDictionary *keys = [preferences[@"keybindings"] mutableCopy] ?: [NSMutableDictionary dictionary];
    keys[@"switch_language_caps_lock"] = @NO;
    preferences[@"keybindings"] = keys;
    updated[@"preferences"] = preferences;
    if (!save([snapshot[@"revision"] unsignedLongLongValue], updated)) return NO;
    [defaults setBool:NO forKey:@"MSIMEClientCapsLockTapShortcut"];
    [defaults setBool:YES forKey:marker];
    [defaults synchronize];
    return YES;
}

// 用户明确要求现有安装改为浮层预览；共享保存成功后再更新本机镜像和一次性标记。
static inline BOOL MSIMEMigrateShurufaVoicePresentation(NSUserDefaults *defaults,
    NSDictionary *(^load)(void), NSDictionary *(^save)(uint64_t, NSDictionary *)) {
    NSString *marker = @"ShurufaVoiceOverlayMigrationV1";
    if ([defaults boolForKey:marker]) return YES;
    NSDictionary *snapshot = load();
    if (![snapshot[@"preferences"] isKindOfClass:NSDictionary.class] || ![snapshot[@"revision"] isKindOfClass:NSNumber.class]) return NO;
    NSMutableDictionary *updated = [snapshot mutableCopy];
    NSMutableDictionary *preferences = [snapshot[@"preferences"] mutableCopy];
    NSMutableDictionary *voice = [preferences[@"voice_input"] mutableCopy] ?: [NSMutableDictionary dictionary];
    voice[@"stream_inline_preedit"] = @NO;
    preferences[@"voice_input"] = voice; updated[@"preferences"] = preferences;
    if (!save([snapshot[@"revision"] unsignedLongLongValue], updated)) return NO;
    [defaults setBool:NO forKey:@"MSIMEClientVoiceStreamInlinePreedit"];
    [defaults setBool:YES forKey:marker];
    [defaults synchronize];
    return YES;
}
