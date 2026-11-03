#import "../../src/voice/ConfiguredVoiceShortcut.h"
#include <cassert>
#include <initializer_list>

static NSEvent *Event(unsigned short key, NSEventModifierFlags flags, NSEventType type, NSTimeInterval timestamp = 1, BOOL repeat = NO) {
    return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:flags timestamp:timestamp windowNumber:0 context:nil
        characters:@"" charactersIgnoringModifiers:@"" isARepeat:repeat keyCode:key];
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
            // 另一侧先按下时，同样不能把双侧组合识别成单个右侧修饰键。
            shortcut.reset();
            const auto otherFlags = code.intValue == 54
                ? NSEventModifierFlagCommand | NX_DEVICELCMDKEYMASK
                : NSEventModifierFlagOption | NX_DEVICELALTKEYMASK;
            shortcut.observe(Event(otherKey, otherFlags, NSEventTypeFlagsChanged), config, false);
            assert(shortcut.observe(Event(code.unsignedShortValue, both, NSEventTypeFlagsChanged), config, false).action == Action::None);
            assert(shortcut.observe(Event(code.unsignedShortValue, otherFlags, NSEventTypeFlagsChanged), config, false).action == Action::None);
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
        // 切换模式用组合键开始录音后，主键还没松开就按别的键（如 Option+→）只交给应用，不取消录音。
        {
            MSIMEConfiguredVoiceShortcut shortcut;
            MSIMEConfiguredVoiceShortcut::Options config{@{@"key_code":@123, @"modifiers":@2, @"right_modifiers":@2}, @"toggle", false, false, false, false};
            assert(shortcut.observe(Event(123, option, NSEventTypeKeyDown, 40), config, false).action == Action::Toggle);
            const auto other = shortcut.observe(Event(124, option, NSEventTypeKeyDown, 41), config, true);
            assert(!other.consumed && other.action == Action::None);
            assert(shortcut.observe(Event(123, option, NSEventTypeKeyUp, 42), config, true).action == Action::None);
            assert(shortcut.observe(Event(61, 0, NSEventTypeFlagsChanged, 43), config, true).action == Action::None);
            // 之后的普通方向键不是快捷键，再按一次组合键才结束。
            const auto plain = shortcut.observe(Event(123, 0, NSEventTypeKeyDown, 44), config, true);
            assert(!plain.consumed && plain.action == Action::None);
            assert(shortcut.observe(Event(123, option, NSEventTypeKeyDown, 45), config, true).action == Action::Toggle);
        }
        // IMK 只转发 KeyDown 和 FlagsChanged，从不转发 KeyUp：先松开修饰键后，第二次按下组合键仍要结束录音。
        {
            MSIMEConfiguredVoiceShortcut shortcut;
            MSIMEConfiguredVoiceShortcut::Options config{@{@"key_code":@123, @"modifiers":@2}, @"toggle", false, false, false, false};
            const auto arrow = NSEventModifierFlagOption | NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;
            assert(shortcut.observe(Event(123, arrow, NSEventTypeKeyDown, 10), config, false).action == Action::Toggle);
            assert(shortcut.observe(Event(58, 0, NSEventTypeFlagsChanged, 10.1), config, true).action == Action::None);
            assert(shortcut.observe(Event(58, NSEventModifierFlagOption, NSEventTypeFlagsChanged, 12), config, true).action == Action::None);
            const auto second = shortcut.observe(Event(123, arrow, NSEventTypeKeyDown, 12.1), config, true);
            assert(second.consumed && second.action == Action::Toggle);
            assert(shortcut.observe(Event(58, 0, NSEventTypeFlagsChanged, 12.2), config, false).action == Action::None);
            assert(shortcut.observe(Event(123, arrow, NSEventTypeKeyDown, 14), config, false).action == Action::Toggle);
        }
        // 录的是右 Option+←：左 Option+← 不触发、照常交给应用，右侧才开始和结束。
        {
            MSIMEConfiguredVoiceShortcut shortcut;
            NSDictionary *binding = @{@"key_code":@123, @"modifiers":@2, @"right_modifiers":@2};
            assert(MSIMEVoiceShortcutValid(binding));
            assert(!MSIMEVoiceShortcutValid((@{@"key_code":@123, @"modifiers":@2, @"right_modifiers":@4})));
            assert(!MSIMEVoiceShortcutValid((@{@"key_code":@123, @"modifiers":@2, @"right_modifiers":@YES})));
            MSIMEConfiguredVoiceShortcut::Options config{binding, @"toggle", false, false, false, false};
            const auto arrow = NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;
            const auto left = arrow | NSEventModifierFlagOption | NX_DEVICELALTKEYMASK;
            const auto right = arrow | NSEventModifierFlagOption | NX_DEVICERALTKEYMASK;
            const auto ignored = shortcut.observe(Event(123, left, NSEventTypeKeyDown, 40), config, false);
            assert(!ignored.consumed && ignored.action == Action::None);
            assert(shortcut.observe(Event(58, 0, NSEventTypeFlagsChanged, 40.1), config, false).action == Action::None);
            assert(shortcut.observe(Event(123, right, NSEventTypeKeyDown, 41), config, false).action == Action::Toggle);
            assert(shortcut.observe(Event(61, 0, NSEventTypeFlagsChanged, 41.1), config, true).action == Action::None);
            const auto leftWhileRecording = shortcut.observe(Event(123, left, NSEventTypeKeyDown, 42), config, true);
            assert(!leftWhileRecording.consumed && leftWhileRecording.action == Action::None);
            assert(shortcut.observe(Event(58, 0, NSEventTypeFlagsChanged, 42.1), config, true).action == Action::None);
            assert(shortcut.observe(Event(123, right, NSEventTypeKeyDown, 43), config, true).action == Action::Toggle);
            // 旧绑定没有侧别时左右都算。
            MSIMEConfiguredVoiceShortcut legacy;
            MSIMEConfiguredVoiceShortcut::Options either{@{@"key_code":@123, @"modifiers":@2}, @"toggle", false, false, false, false};
            assert(legacy.observe(Event(123, left, NSEventTypeKeyDown, 50), either, false).action == Action::Toggle);
        }
        // 按住模式先松开修饰键：自动重复的 KeyDown 仍被吞掉，新的一次按下照常开始。
        {
            MSIMEConfiguredVoiceShortcut shortcut;
            MSIMEConfiguredVoiceShortcut::Options config{@{@"key_code":@123, @"modifiers":@2}, @"hold", false, false, false, false};
            const auto arrow = NSEventModifierFlagOption | NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;
            assert(shortcut.observe(Event(123, arrow, NSEventTypeKeyDown, 20), config, false).action == Action::Toggle);
            const auto release = shortcut.observe(Event(58, 0, NSEventTypeFlagsChanged, 20.5), config, true);
            assert(release.action == Action::Toggle && release.onRelease);
            const auto repeat = shortcut.observe(Event(123, NSEventModifierFlagFunction, NSEventTypeKeyDown, 20.6, YES), config, false);
            assert(repeat.consumed && repeat.action == Action::None);
            assert(shortcut.observe(Event(123, arrow, NSEventTypeKeyDown, 22), config, false).action == Action::Toggle);
        }
        // Esc 取消后收不到 KeyUp，下一次 Esc 要交还给应用。
        {
            MSIMEConfiguredVoiceShortcut shortcut;
            MSIMEConfiguredVoiceShortcut::Options config{@{@"key_code":@123, @"modifiers":@2}, @"toggle", false, false, false, false};
            assert(shortcut.observe(Event(53, 0, NSEventTypeKeyDown, 30), config, true).action == Action::Cancel);
            const auto next = shortcut.observe(Event(53, 0, NSEventTypeKeyDown, 31), config, false);
            assert(!next.consumed && next.action == Action::None);
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
