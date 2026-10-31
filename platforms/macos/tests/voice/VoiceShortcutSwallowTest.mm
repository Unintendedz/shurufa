#import "../../src/voice/VoiceShortcutTap.h"
#include <cassert>

static const CGEventFlags kRightOption = NSEventModifierFlagOption | NX_DEVICERALTKEYMASK | NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;
static const CGEventFlags kLeftOption = NSEventModifierFlagOption | NX_DEVICELALTKEYMASK | NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;
static const CGEventFlags kFunctionOnly = NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;

static bool Down(MSIMEVoiceShortcutSwallow &s, unsigned short key, CGEventFlags flags, const MSIMEVoiceShortcutBinding &b, bool repeat = false, bool selfPosted = false) {
    return s.observe(kCGEventKeyDown, key, flags, repeat, selfPosted, b);
}
static bool Up(MSIMEVoiceShortcutSwallow &s, unsigned short key, CGEventFlags flags, const MSIMEVoiceShortcutBinding &b) {
    return s.observe(kCGEventKeyUp, key, flags, false, false, b);
}

int main() {
    @autoreleasepool {
        const MSIMEVoiceShortcutBinding rightOptionLeft{123, 2, 2};
        const MSIMEVoiceShortcutBinding anyOptionLeft{123, 2, -1};
        {
            MSIMEVoiceShortcutSwallow s;
            assert(Down(s, 123, kRightOption, rightOptionLeft));
            assert(Down(s, 123, kRightOption, rightOptionLeft, true));
            assert(Up(s, 123, 0, rightOptionLeft));
            assert(!Up(s, 123, 0, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(!Down(s, 123, kLeftOption, rightOptionLeft));
            assert(!Up(s, 123, kLeftOption, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(Down(s, 123, kLeftOption, anyOptionLeft));
            assert(Up(s, 123, 0, anyOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(Down(s, 123, kRightOption, rightOptionLeft));
            assert(Down(s, 123, kFunctionOnly, rightOptionLeft, true));
            assert(Up(s, 123, kFunctionOnly, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(!Down(s, 123, kRightOption | NSEventModifierFlagShift | NX_DEVICELSHIFTKEYMASK, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(Down(s, 123, kRightOption, rightOptionLeft));
            assert(!Down(s, 0, kFunctionOnly, rightOptionLeft));
            assert(!Up(s, 0, kFunctionOnly, rightOptionLeft));
            assert(Up(s, 123, kFunctionOnly, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(!s.observe(kCGEventFlagsChanged, 123, kRightOption, false, false, rightOptionLeft));
            assert(!Down(s, 123, kRightOption, rightOptionLeft, false, true));
            assert(!Up(s, 123, 0, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(Down(s, 123, kRightOption, rightOptionLeft));
            assert(!Down(s, 123, kFunctionOnly, rightOptionLeft));
            assert(!Up(s, 123, kFunctionOnly, rightOptionLeft));
        }
        {
            MSIMEVoiceShortcutSwallow s;
            assert(Down(s, 123, kRightOption, rightOptionLeft));
            s.reset();
            assert(!Up(s, 123, 0, rightOptionLeft));
        }
        assert(!MSIMEVoiceSwallowBinding(nil));
        assert(!MSIMEVoiceSwallowBinding(@{@"key_code":@61, @"modifiers":@0}));
        assert(!MSIMEVoiceSwallowBinding(@{@"key_code":@58, @"modifiers":@0}));
        auto plain = MSIMEVoiceSwallowBinding(@{@"key_code":@123, @"modifiers":@2});
        assert(plain && plain->key == 123 && plain->modifiers == 2 && plain->rightModifiers == -1);
        auto sided = MSIMEVoiceSwallowBinding(@{@"key_code":@123, @"modifiers":@2, @"right_modifiers":@2});
        assert(sided && sided->rightModifiers == 2);
        auto f5 = MSIMEVoiceSwallowBinding(@{@"key_code":@96, @"modifiers":@0});
        assert(f5 && f5->key == 96 && f5->modifiers == 0);
    }
    return 0;
}
