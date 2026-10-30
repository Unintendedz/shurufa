#pragma once
#import <AppKit/AppKit.h>
#include <IOKit/hidsystem/IOLLEvent.h>
#import "../core/VoiceShortcutConfig.h"
#import "VoiceHoldShortcut.h"
#include <initializer_list>

static inline unsigned MSIMEVoiceModifierBits(NSEventModifierFlags flags) {
    return ((flags & NSEventModifierFlagControl) ? 1 : 0) |
        ((flags & NSEventModifierFlagOption) ? 2 : 0) |
        ((flags & NSEventModifierFlagShift) ? 4 : 0) |
        ((flags & NSEventModifierFlagCommand) ? 8 : 0);
}
static inline NSEventModifierFlags MSIMEVoiceSideFlag(unsigned short code) {
    switch (code) {
        case 54: return NX_DEVICERCMDKEYMASK; case 55: return NX_DEVICELCMDKEYMASK;
        case 56: return NX_DEVICELSHIFTKEYMASK; case 60: return NX_DEVICERSHIFTKEYMASK;
        case 58: return NX_DEVICELALTKEYMASK; case 61: return NX_DEVICERALTKEYMASK;
        case 59: return NX_DEVICELCTLKEYMASK; case 62: return NX_DEVICERCTLKEYMASK;
        default: return 0;
    }
}
static inline unsigned MSIMEVoiceModifierBitForKey(unsigned short code) {
    switch (code) {
        case 54: case 55: return 8; case 56: case 60: return 4;
        case 58: case 61: return 2; case 59: case 62: return 1;
        default: return 0;
    }
}
static inline NSEventModifierFlags MSIMEVoiceModifierGroupSides(unsigned group) {
    return group == 8 ? NX_DEVICELCMDKEYMASK | NX_DEVICERCMDKEYMASK :
        group == 4 ? NX_DEVICELSHIFTKEYMASK | NX_DEVICERSHIFTKEYMASK :
        group == 2 ? NX_DEVICELALTKEYMASK | NX_DEVICERALTKEYMASK : NX_DEVICELCTLKEYMASK | NX_DEVICERCTLKEYMASK;
}

// 录下的组合里每个修饰键是右侧（right 的对应位）还是左侧；事件里同组两侧都没报时不判断侧别。
static inline unsigned MSIMEVoiceRightModifierBits(NSEventModifierFlags flags) {
    return ((flags & NX_DEVICERCTLKEYMASK) ? 1 : 0) | ((flags & NX_DEVICERALTKEYMASK) ? 2 : 0) |
        ((flags & NX_DEVICERSHIFTKEYMASK) ? 4 : 0) | ((flags & NX_DEVICERCMDKEYMASK) ? 8 : 0);
}
static inline bool MSIMEVoiceSidesMatch(NSEventModifierFlags flags, unsigned required, id right) {
    if (![right isKindOfClass:NSNumber.class]) return true;
    const unsigned wantRight = [right unsignedIntValue];
    for (unsigned group : {1u, 2u, 4u, 8u}) {
        if (!(required & group)) continue;
        const NSEventModifierFlags sides = MSIMEVoiceModifierGroupSides(group);
        const NSEventModifierFlags rightSide = sides & (NX_DEVICERCTLKEYMASK | NX_DEVICERALTKEYMASK | NX_DEVICERSHIFTKEYMASK | NX_DEVICERCMDKEYMASK);
        if (!(flags & sides)) continue;
        const NSEventModifierFlags expected = (wantRight & group) ? rightSide : sides & ~rightSide;
        if ((flags & sides) != expected) return false;
    }
    return true;
}

// 明确的按下/松开状态机。切换模式的裸修饰键等松开才开始，组合键不经过计时猜测。
class MSIMEConfiguredVoiceShortcut {
public:
    using Action = MSIMEVoiceHoldShortcut::Action;
    using Result = MSIMEVoiceHoldShortcut::Result;
    struct Options {
        NSDictionary *custom;
        NSString *mode;
        bool rightOption, controlCommand, rightControlOption;
        bool toggleMatch;
    };
    void reset() { *this = MSIMEConfiguredVoiceShortcut{}; }
    bool locked() const { return locked_; }
    bool began() const { return began_; }
    Result observe(NSEvent *event, Options options, bool recording, NSEventModifierFlags physicalFlags = 0) {
        began_ = false;
        const auto type = event.type;
        const auto key = event.keyCode;
        const unsigned modifiers = MSIMEVoiceModifierBits(event.modifierFlags);
        if (key == 53 && type == NSEventTypeKeyDown && recording) {
            active_ = false; suppressedKey_ = 53; locked_ = false;
            return {true, Action::Cancel};
        }
        if (type == NSEventTypeKeyUp && key == suppressedKey_) { suppressedKey_ = -1; return {true}; }
        // IMK 从不转发 KeyUp，只能靠它解除的吞键会一直留着；非自动重复的 KeyDown 说明这个键已经松开又按下，是新的一次按键。
        if (type == NSEventTypeKeyDown && key == suppressedKey_) {
            if (event.isARepeat) return {true};
            suppressedKey_ = -1;
        }
        if (type == NSEventTypeFlagsChanged) {
            const auto side = MSIMEVoiceSideFlag(key);
            const unsigned group = MSIMEVoiceModifierBitForKey(key);
            // 只更新当前事件的侧别；物理快照不提前计入尚未分发的另一侧按键。
            const auto groupSides = MSIMEVoiceModifierGroupSides(group);
            const auto sideFlags = (event.modifierFlags & groupSides) ? event.modifierFlags : physicalFlags;
            const bool down = (modifiers & group) && (sideFlags & side);
            if (side) {
                if (down) sides_ |= side;
                else sides_ &= ~side;
            }
        }
        // IMK 客户端可能不转发组合键的 KeyUp。新的非重复 KeyDown 本身就是新一轮按下，
        // 不让 toggle 永远卡在上一轮；同一时间戳的全局/IMK 双入口仍只处理一次。
        if (type == NSEventTypeKeyDown && key == primary_ && event.timestamp > 0) {
            if (active_ && event.timestamp == primaryDownTimestamp_) return {true};
            if (active_ && !hold_ && !modifier_ && !event.isARepeat && modifiers == expected_)
                active_ = false;
        }
        if (active_) {
            const bool release = modifier_ ? !(sides_ & MSIMEVoiceSideFlag(primary_)) ||
                (modifiers & required_) != required_ :
                (type == NSEventTypeKeyUp && key == primary_) || (modifiers & required_) != required_;
            if (release) {
                active_ = false;
                const bool consumed = key == primary_;
                if (!modifier_ && !consumed) suppressedKey_ = primary_;
                if (cancelled_) return {consumed};
                if (hold_) return {consumed, recording && !locked_ ? Action::Toggle : Action::None, true};
                return {consumed, modifier_ ? Action::Toggle : Action::None};
            }
            if (type == NSEventTypeKeyDown && key == 49 && hold_ && recording && modifier_ && !cancelled_) {
                locked_ = true; suppressedKey_ = 49; return {true};
            }
            if ((type == NSEventTypeKeyDown && key != primary_) ||
                (type == NSEventTypeFlagsChanged && (modifiers != expected_ ||
                    (key != primary_ && (sides_ & MSIMEVoiceSideFlag(key)))))) {
                cancelled_ = true;
                // 普通组合键继续传给应用，取消录音而不提交或整理。
                return {false, (started_ || hold_) && recording && !locked_ ? Action::Cancel : Action::None};
            }
            return {key == primary_};
        }
        if (type == NSEventTypeKeyDown && event.isARepeat) return {};
        bool match = false, modifier = false;
        unsigned required = modifiers;
        if (options.custom) {
            const unsigned short code = [options.custom[@"key_code"] unsignedShortValue];
            modifier = MSIMEVoiceModifierKey(code);
            required = [options.custom[@"modifiers"] unsignedIntValue];
            const auto primarySide = MSIMEVoiceSideFlag(code);
            const auto primaryGroup = MSIMEVoiceModifierGroupSides(MSIMEVoiceModifierBitForKey(code));
            match = key == code && MSIMEVoiceSidesMatch(event.modifierFlags, required, options.custom[@"right_modifiers"]) && (modifier
                ? type == NSEventTypeFlagsChanged && (sides_ & primaryGroup) == primarySide && modifiers == (required | MSIMEVoiceModifierBitForKey(code))
                : type == NSEventTypeKeyDown && modifiers == required);
        } else if (type == NSEventTypeKeyDown && options.toggleMatch) {
            match = true;
        } else if (type == NSEventTypeFlagsChanged && (sides_ & MSIMEVoiceSideFlag(key))) {
            modifier = true;
            if (key == 61 && options.rightControlOption && modifiers == 3 && (sides_ & NX_DEVICERCTLKEYMASK)) { match = true; required = 1; }
            else if (key == 61 && options.rightOption && modifiers == 2) { match = true; required = 0; }
            else if ((key == 54 || key == 55) && options.controlCommand && modifiers == 9) { match = true; required = 1; }
        }
        if (!match) return {};
        active_ = true; primary_ = key; modifier_ = modifier; required_ = required; expected_ = modifiers;
        if (type == NSEventTypeKeyDown) primaryDownTimestamp_ = event.timestamp;
        began_ = true;
        cancelled_ = false; locked_ = false; started_ = !recording;
        hold_ = [options.mode isEqual:@"hold"] || ([options.mode isEqual:@"legacy"] && modifier && required == 0);
        // 已在录音时也等这次按住完整松开，避免普通修饰键组合提前提交或整理。
        return {true, (hold_ && !recording) || (!hold_ && !modifier) ? Action::Toggle : Action::None};
    }
private:
    NSEventModifierFlags sides_ = 0;
    unsigned short primary_ = 0;
    unsigned required_ = 0, expected_ = 0;
    NSTimeInterval primaryDownTimestamp_ = 0;
    int suppressedKey_ = -1;
    bool active_ = false, modifier_ = false, hold_ = false, started_ = false, cancelled_ = false, locked_ = false;
    bool began_ = false;
};
