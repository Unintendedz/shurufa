#pragma once
#import "ConfiguredVoiceShortcut.h"
#import "../settings/AppearancePreferences.h"
#import "../input/ToggleHotkeyEvent.h"
#import "../settings/KeycapPresentation.h"

// 录制时连同修饰键的左右侧一起记下：按的是右 Option，左 Option 就不会触发。
static inline NSDictionary *MSIMERecordedVoiceShortcut(unsigned short code, unsigned modifiers, NSEventModifierFlags flags) {
    if (!modifiers) return @{@"key_code":@(code), @"modifiers":@0};
    return @{@"key_code":@(code), @"modifiers":@(modifiers), @"right_modifiers":@(MSIMEVoiceRightModifierBits(flags) & modifiers)};
}

static inline NSString *MSIMEVoiceShortcutTitle(NSDictionary *binding) {
    if (!binding) return @"未设置";
    const unsigned short code = [binding[@"key_code"] unsignedShortValue];
    NSDictionary *modifiers = @{@54:@"右 Command", @55:@"左 Command", @56:@"左 Shift", @60:@"右 Shift",
        @58:@"左 Option", @61:@"右 Option", @59:@"左 Control", @62:@"右 Control"};
    NSString *primary = modifiers[@(code)];
    for (const auto &entry : msime::mac::detail::kToggleChordKeyCodes) {
        if (entry.keyCode != code) continue;
        primary = entry.character ? [[NSString stringWithFormat:@"%c", entry.character] uppercaseString] :
            [NSString stringWithUTF8String:entry.name];
        break;
    }
    if (!primary) primary = @{@36:@"Return", @48:@"Tab", @51:@"Delete", @123:@"左方向键", @124:@"右方向键",
        @125:@"下方向键", @126:@"上方向键"}[@(code)] ?: [NSString stringWithFormat:@"键码 %u", code];
    NSMutableArray *parts = [NSMutableArray array];
    const unsigned flags = [binding[@"modifiers"] unsignedIntValue];
    id right = binding[@"right_modifiers"];
    for (NSArray *item in @[@[@1,@"Control"],@[@2,@"Option"],@[@4,@"Shift"],@[@8,@"Command"]]) {
        const unsigned bit = [item[0] unsignedIntValue];
        if (!(flags & bit)) continue;
        NSString *side = !right ? @"" : ([right unsignedIntValue] & bit) ? @"右 " : @"左 ";
        [parts addObject:[side stringByAppendingString:item[1]]];
    }
    [parts addObject:primary];
    return [parts componentsJoinedByString:@"+"];
}

static inline NSView *MSIMEVoiceShortcutKeycaps(NSDictionary *binding) {
    if (!binding) return [NSTextField labelWithString:@"未设置"];
    NSMutableArray<NSView *> *keys = [NSMutableArray array];
    unsigned flags = [binding[@"modifiers"] unsignedIntValue];
    id right = binding[@"right_modifiers"];
    for (NSArray *item in @[@[@1,@"⌃"],@[@2,@"⌥"],@[@4,@"⇧"],@[@8,@"⌘"]]) {
        const unsigned bit = [item[0] unsignedIntValue];
        if (!(flags & bit)) continue;
        NSString *side = !right ? nil : ([right unsignedIntValue] & bit) ? @"右" : @"左";
        [keys addObject:MSIMEKeycap(item[1], side)];
    }
    NSNumber *code = binding[@"key_code"];
    NSDictionary *symbols = @{@54:@"⌘",@55:@"⌘",@56:@"⇧",@60:@"⇧",@58:@"⌥",@61:@"⌥",@59:@"⌃",@62:@"⌃",
        @123:@"←",@124:@"→",@125:@"↓",@126:@"↑",@49:@"空格",@36:@"↩",@48:@"⇥",@51:@"⌫"};
    NSString *symbol = symbols[code] ?: MSIMEVoiceShortcutTitle(@{@"key_code":code,@"modifiers":@0});
    NSString *side = MSIMEVoiceModifierKey(code.unsignedShortValue) ?
        ([@[@54,@60,@61,@62] containsObject:code] ? @"右" : @"左") : nil;
    [keys addObject:MSIMEKeycap(symbol, side)];
    return MSIMEKeycapRow(keys, MSIMEVoiceShortcutTitle(binding));
}

static inline NSString *MSIMEVoiceShortcutConflict(NSDictionary *binding, MSIMEAppearancePreferences *appearance) {
    const unsigned short code = [binding[@"key_code"] unsignedShortValue];
    const unsigned flags = [binding[@"modifiers"] unsignedIntValue];
    if (code == 53) return @"Esc 留作取消录音，不能设为语音快捷键。";
    if (code == 57) return @"Caps Lock 留作中英文切换，不能设为语音快捷键。";
    if (!MSIMEVoiceShortcutValid(binding)) return @"请使用单个修饰键、功能键，或带 Control、Option、Command 的组合键，避免占用正常打字。";
    if (MSIMEEditionIsShurufa()) return nil;
    if (flags == 7 && [@[@8,@15,@17] containsObject:@(code)])
        return @"这个组合键已用于输入法维护，请选择其他快捷键。";
    if (!MSIMEEditionIsShurufa() && code == 40 && flags == 13)
        return @"这个组合键已用于屏幕键盘，请选择其他快捷键。";
    if (!appearance) return nil;
    if (flags == 0 && ((code == 56 || code == 60) && appearance.shiftTapShortcut))
        return @"这个 Shift 键已用于切换中英文，请先在按键设置中关闭该切换方式。";
    if (flags == 0 && ((code == 59 || code == 62) && appearance.controlTapShortcut))
        return @"这个 Control 键已用于切换中英文，请先在按键设置中关闭该切换方式。";
    if (code == 49 && ((flags == 4 && appearance.inputModeShortcut) || (flags == 3 && appearance.controlOptionSpaceShortcut)))
        return @"这个组合键已用于切换中英文，请选择其他快捷键。";
    NSEventModifierFlags native = ((flags & 1) ? NSEventModifierFlagControl : 0) |
        ((flags & 2) ? NSEventModifierFlagOption : 0) | ((flags & 4) ? NSEventModifierFlagShift : 0) |
        ((flags & 8) ? NSEventModifierFlagCommand : 0);
    NSEvent *event = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:native
        timestamp:0 windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:code];
    const auto character = msime::mac::ToggleHotkeyCharacter(event);
    for (auto chord : {appearance.punctuationToggleChord, appearance.englishCandidatesToggleChord, appearance.fullWidthToggleChord})
        if (msime::mac::ToggleChordMatches(chord, code, character, flags & 1, flags & 2, flags & 4, flags & 8))
            return @"这个组合键已用于输入法的标点、英文候选或全半角切换，请选择其他快捷键。";
    if ((character == 'f' && flags == 5 && appearance.characterSetShortcut) ||
        (character == 'h' && flags == 6 && appearance.fullWidthShortcut))
        return @"这个组合键已用于输入法的简繁或全半角切换，请选择其他快捷键。";
    return nil;
}
