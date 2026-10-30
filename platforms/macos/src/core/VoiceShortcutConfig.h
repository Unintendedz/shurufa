#pragma once
#import <Foundation/Foundation.h>
#import "EditionIdentity.h"

// 修饰键位是跨文档的明确数值：Control=1、Option=2、Shift=4、Command=8。
static inline BOOL MSIMEVoiceModifierKey(unsigned short key) {
    return key == 54 || key == 55 || key == 56 || key == 58 || key == 59 || key == 60 || key == 61 || key == 62;
}
static inline BOOL MSIMEVoiceFunctionKey(unsigned short key) {
    switch (key) {
        case 122: case 120: case 99: case 118: case 96: case 97:
        case 98: case 100: case 101: case 109: case 103: case 111: return YES;
        default: return NO;
    }
}
static inline BOOL MSIMEVoiceShortcutValid(id value) {
    if (![value isKindOfClass:NSDictionary.class]) return NO;
    // right_modifiers 可选：modifiers 里必须用右侧键按的位，其余位用左侧键；没有它时左右都算。
    id right = value[@"right_modifiers"];
    if ([value count] != (right ? 3 : 2)) return NO;
    id key = value[@"key_code"], mods = value[@"modifiers"];
    if (![key isKindOfClass:NSNumber.class] || ![mods isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)key) == CFBooleanGetTypeID() ||
        CFGetTypeID((__bridge CFTypeRef)mods) == CFBooleanGetTypeID()) return NO;
    NSInteger code = [key integerValue], flags = [mods integerValue];
    if ([key doubleValue] != code || [mods doubleValue] != flags || code < 0 || code > 126 ||
        flags < 0 || flags > 15 || code == 53 || code == 57 || code == 63) return NO;
    if (right && (![right isKindOfClass:NSNumber.class] || CFGetTypeID((__bridge CFTypeRef)right) == CFBooleanGetTypeID() ||
        [right doubleValue] != [right integerValue] || [right integerValue] < 0 || ([right integerValue] & ~flags) != 0)) return NO;
    if (MSIMEVoiceModifierKey(code)) {
        const unsigned own = code == 54 || code == 55 ? 8 : code == 56 || code == 60 ? 4 :
            code == 58 || code == 61 ? 2 : 1;
        return (flags & own) == 0;
    }
    return (flags & 11) != 0 || MSIMEVoiceFunctionKey(code);
}
static inline NSString *MSIMEVoiceHotkeyMode(NSUserDefaults *defaults) {
    NSString *mode = [defaults stringForKey:@"MSIMEClientVoiceHotkeyMode"];
    if ([@[@"hold", @"toggle"] containsObject:mode ?: @""]) return mode;
    if (MSIMEEditionIsShurufa()) {
        id binding = [defaults objectForKey:@"MSIMEClientVoiceMacOSHotkey"];
        return !MSIMEVoiceShortcutValid(binding) || (MSIMEVoiceModifierKey([binding[@"key_code"] unsignedShortValue]) &&
            [binding[@"modifiers"] unsignedIntValue] == 0) ? @"hold" : @"toggle";
    }
    return @"legacy";
}
static inline NSDictionary *MSIMEVoiceCustomShortcut(NSUserDefaults *defaults) {
    id value = [defaults objectForKey:@"MSIMEClientVoiceMacOSHotkey"];
    return MSIMEVoiceShortcutValid(value) ? value : MSIMEEditionIsShurufa() ? @{@"key_code":@61,@"modifiers":@0} : nil;
}
static inline BOOL MSIMEVoiceShortcutOccupies(NSUserDefaults *defaults, NSArray<NSNumber *> *keys, unsigned modifiers) {
    NSDictionary *binding = MSIMEVoiceCustomShortcut(defaults);
    return binding && [keys containsObject:binding[@"key_code"]] && [binding[@"modifiers"] unsignedIntValue] == modifiers;
}
