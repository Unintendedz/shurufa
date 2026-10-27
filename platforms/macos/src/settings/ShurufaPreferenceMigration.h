#pragma once
#import <Foundation/Foundation.h>

// 用户要求为现有安装开启 Caps Lock 切换；只迁移一次，之后设置页的关闭选择保留。
static inline BOOL MSIMEMigrateShurufaCapsLock(NSUserDefaults *defaults,
    NSDictionary *(^load)(void), NSDictionary *(^save)(uint64_t, NSDictionary *)) {
    NSString *marker = @"ShurufaCapsLockMigrationV1";
    if ([defaults boolForKey:marker]) return YES;
    NSDictionary *snapshot = load();
    if (![snapshot[@"preferences"] isKindOfClass:NSDictionary.class] ||
        ![snapshot[@"revision"] isKindOfClass:NSNumber.class]) return NO;
    NSMutableDictionary *updated = [snapshot mutableCopy];
    NSMutableDictionary *preferences = [snapshot[@"preferences"] mutableCopy];
    NSMutableDictionary *keys = [preferences[@"keybindings"] mutableCopy] ?: [NSMutableDictionary dictionary];
    keys[@"switch_language_caps_lock"] = @YES;
    preferences[@"keybindings"] = keys;
    updated[@"preferences"] = preferences;
    if (!save([snapshot[@"revision"] unsignedLongLongValue], updated)) return NO;
    [defaults setBool:YES forKey:@"MSIMEClientCapsLockTapShortcut"];
    [defaults setBool:YES forKey:marker];
    [defaults synchronize];
    return YES;
}
