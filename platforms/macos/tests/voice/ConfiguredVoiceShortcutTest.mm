#import "../../src/voice/ConfiguredVoiceShortcut.h"
#include <cassert>
#include <initializer_list>

static NSEvent *Event(unsigned short key, NSEventModifierFlags flags, NSEventType type) {
    return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:flags timestamp:1 windowNumber:0 context:nil
        characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:key];
}
int main() {
    @autoreleasepool {
        using Action = MSIMEConfiguredVoiceShortcut::Action;
        const auto cmd = NSEventModifierFlagCommand | NX_DEVICERCMDKEYMASK;
        const auto option = NSEventModifierFlagOption | NX_DEVICERALTKEYMASK;
        for (NSNumber *code in @[@54, @61]) {
            const auto flags = code.intValue == 54 ? cmd : option;
            MSIMEConfiguredVoiceShortcut::Options config{@{@"key_code":code, @"modifiers":@0}, @"toggle", true, true, true, false};
            MSIMEConfiguredVoiceShortcut shortcut;
            auto down = Event(code.unsignedShortValue, flags, NSEventTypeFlagsChanged);
            auto up = Event(code.unsignedShortValue, 0, NSEventTypeFlagsChanged);
            assert(shortcut.observe(down, config, false).action == Action::None);
            assert(shortcut.observe(up, config, false).action == Action::Toggle);
            assert(shortcut.observe(up, config, true).action == Action::None);
            assert(shortcut.observe(down, config, true).action == Action::None);
            assert(shortcut.observe(up, config, true).action == Action::Toggle);
            shortcut.reset();
            assert(shortcut.observe(down, config, false).action == Action::None);
            auto chord = shortcut.observe(Event(8, flags, NSEventTypeKeyDown), config, false);
            assert(!chord.consumed && chord.action == Action::None);
            assert(shortcut.observe(up, config, false).action == Action::None);
            // 同组的另一侧也是另一个实体键，不能当作仍然单按右侧。
            shortcut.reset();
            assert(shortcut.observe(down, config, false).action == Action::None);
            const unsigned short otherKey = code.intValue == 54 ? 55 : 58;
            const auto both = flags | (code.intValue == 54 ? NX_DEVICELCMDKEYMASK : NX_DEVICELALTKEYMASK);
            assert(shortcut.observe(Event(otherKey, both, NSEventTypeFlagsChanged), config, false).action == Action::None);
            assert(shortcut.observe(up, config, false).action == Action::None);
            for (NSString *mode in @[@"hold", @"legacy"]) {
                config.mode = mode; shortcut.reset();
                assert(shortcut.observe(down, config, false).action == Action::Toggle);
                auto finish = shortcut.observe(up, config, true);
                assert(finish.action == Action::Toggle && finish.onRelease);
                shortcut.reset();
                assert(shortcut.observe(down, config, false).action == Action::Toggle);
                chord = shortcut.observe(Event(8, flags, NSEventTypeKeyDown), config, true);
                assert(!chord.consumed && chord.action == Action::Cancel);
                assert(shortcut.observe(up, config, false).action == Action::None);
                shortcut.reset();
                // 已在录音时按普通组合键不能在修饰键按下时提前提交。
                assert(shortcut.observe(down, config, true).action == Action::None);
                assert(shortcut.observe(Event(8, flags, NSEventTypeKeyDown), config, true).action == Action::Cancel);
                assert(shortcut.observe(up, config, false).action == Action::None);
            }
        }
        // 组合键的原有行为是切换；明确选按住时，松开主键或修饰键均结束。
        for (NSString *mode in @[@"legacy", @"hold", @"toggle"]) {
            MSIMEConfiguredVoiceShortcut shortcut;
            MSIMEConfiguredVoiceShortcut::Options config{@{@"key_code":@9, @"modifiers":@3}, mode, true, true, true, false};
            const auto flags = NSEventModifierFlagControl | NSEventModifierFlagOption;
            assert(shortcut.observe(Event(9, flags, NSEventTypeKeyDown), config, false).action == Action::Toggle);
            assert(shortcut.observe(Event(9, flags, NSEventTypeKeyDown), config, true).action == Action::None);
            const auto release = shortcut.observe(Event(9, flags, NSEventTypeKeyUp), config, true);
            assert(release.action == ([mode isEqual:@"hold"] ? Action::Toggle : Action::None));
            assert(shortcut.observe(Event(53, 0, NSEventTypeKeyDown), config, true).action == Action::Cancel);
            assert(shortcut.observe(Event(53, 0, NSEventTypeKeyUp), config, false).consumed);
            shortcut.reset();
            assert(shortcut.observe(Event(9, flags, NSEventTypeKeyDown), config, false).action == Action::Toggle);
            auto modifierUp = shortcut.observe(Event(59, NSEventModifierFlagOption, NSEventTypeFlagsChanged), config, true);
            assert(modifierUp.action == ([mode isEqual:@"hold"] ? Action::Toggle : Action::None));
            assert(shortcut.observe(Event(9, 0, NSEventTypeKeyUp), config, false).consumed);
        }
        // 内置 Control+F9 和 Control+Option+V 也受明确模式控制。
        for (unsigned short key : {101, 9}) {
            MSIMEConfiguredVoiceShortcut shortcut;
            MSIMEConfiguredVoiceShortcut::Options config{nil, @"hold", true, true, true, true};
            auto flags = NSEventModifierFlagControl | (key == 9 ? NSEventModifierFlagOption : 0);
            assert(shortcut.observe(Event(key, flags, NSEventTypeKeyDown), config, false).action == Action::Toggle);
            assert(shortcut.observe(Event(key, flags, NSEventTypeKeyUp), config, true).action == Action::Toggle);
            shortcut.reset(); config.mode = @"toggle";
            assert(shortcut.observe(Event(key, flags, NSEventTypeKeyDown), config, false).action == Action::Toggle);
            assert(shortcut.observe(Event(key, flags, NSEventTypeKeyUp), config, true).action == Action::None);
        }
        for (id invalid in @[@{@"key_code":@0, @"modifiers":@0}, @{@"key_code":@0, @"modifiers":@4},
            @{@"key_code":@53, @"modifiers":@8}, @{@"key_code":@57, @"modifiers":@0},
            @{@"key_code":@54, @"modifiers":@8}, @{@"key_code":@1.5, @"modifiers":@1}]) assert(!MSIMEVoiceShortcutValid(invalid));
        assert(MSIMEVoiceShortcutValid(@{@"key_code":@54, @"modifiers":@1}));
        // Control 已报侧别而 Option 没报时，只回退 Option 自己的侧别。
        MSIMEConfiguredVoiceShortcut fallback;
        MSIMEConfiguredVoiceShortcut::Options options{nil, @"hold", false, false, true, false};
        const auto control = NSEventModifierFlagControl | NX_DEVICERCTLKEYMASK;
        fallback.observe(Event(62, control, NSEventTypeFlagsChanged), options, false);
        const auto result = fallback.observe(Event(61, control | NSEventModifierFlagOption, NSEventTypeFlagsChanged),
            options, false, control | NSEventModifierFlagOption | NX_DEVICERALTKEYMASK);
        assert(result.action == Action::Toggle);
    }
}
