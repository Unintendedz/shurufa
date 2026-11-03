#pragma once
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreGraphics/CoreGraphics.h>
#include <atomic>
#include <mutex>
#include <optional>
#import "../core/VoiceShortcutConfig.h"
#import "ConfiguredVoiceShortcut.h"
#import "VoiceTextCommit.h"
#import "VoiceShortcutPermissions.h"

struct MSIMEVoiceShortcutBinding {
    unsigned short key = 0;
    unsigned modifiers = 0;
    int rightModifiers = -1;
};

// 只有主键不是修饰键的自定义绑定需要在系统层吞键；裸修饰键不产生字符也不移动光标。
static inline std::optional<MSIMEVoiceShortcutBinding> MSIMEVoiceSwallowBinding(NSDictionary *custom) {
    if (!MSIMEVoiceShortcutValid(custom)) return std::nullopt;
    const unsigned short key = [custom[@"key_code"] unsignedShortValue];
    if (MSIMEVoiceModifierKey(key)) return std::nullopt;
    id right = custom[@"right_modifiers"];
    return MSIMEVoiceShortcutBinding{key, [custom[@"modifiers"] unsignedIntValue],
        [right isKindOfClass:NSNumber.class] ? (int)[right unsignedIntValue] : -1};
}

// 纯判定，只在 tap 线程使用；返回 true 表示丢弃这个事件。
class MSIMEVoiceShortcutSwallow {
public:
    bool observe(CGEventType type, unsigned short key, CGEventFlags flags, bool isRepeat, bool selfPosted,
                 const MSIMEVoiceShortcutBinding &binding) {
        if (selfPosted || (type != kCGEventKeyDown && type != kCGEventKeyUp)) return false;
        if (type == kCGEventKeyUp) {
            if (key != swallowed_) return false;
            swallowed_ = -1;
            return true;
        }
        if (MSIMEVoiceComboMatches(key, (NSEventModifierFlags)flags, binding.key, binding.modifiers, binding.rightModifiers)) {
            swallowed_ = key;
            return true;
        }
        if (key != swallowed_) return false;
        if (isRepeat) return true;
        swallowed_ = -1;
        return false;
    }
    void reset() { swallowed_ = -1; }
private:
    int swallowed_ = -1;
};

// 进程级单例，主线程调用 update；回调在独立线程，主线程卡住时不会拖住全系统按键。
class MSIMEVoiceShortcutTap {
public:
    void update(std::optional<MSIMEVoiceShortcutBinding> binding, void (^handler)(NSEvent *)) {
        const uint64_t packed = binding ? pack(*binding) : 0;
        if (packed == lastPacked_ && (!binding || tap_)) return;
        if (binding) {
            setHandler(handler);
            if (!ensureTap()) { lastPacked_ = 0; return; }
            lastPacked_ = packed;
            state_.store((generation_.fetch_add(1) + 1) << 32 | packed, std::memory_order_release);
            CGEventTapEnable(tap_, true);
        } else {
            lastPacked_ = 0;
            state_.store(0, std::memory_order_release);
            if (tap_) CGEventTapEnable(tap_, false);
            setHandler(nil);
        }
    }
private:
    // 低 32 位：1 位已武装 + 16 位键码 + 4 位修饰键 + 5 位右侧信息（含 -1 标记）；高 32 位为代次。
    static uint64_t pack(const MSIMEVoiceShortcutBinding &b) {
        const uint64_t right = b.rightModifiers < 0 ? 0x10 : (uint64_t)(b.rightModifiers & 0xf);
        return 1ull << 31 | (uint64_t)b.key << 9 | (uint64_t)(b.modifiers & 0xf) << 5 | right;
    }
    static MSIMEVoiceShortcutBinding unpack(uint64_t v) {
        MSIMEVoiceShortcutBinding b;
        b.key = (v >> 9) & 0xffff;
        b.modifiers = (v >> 5) & 0xf;
        b.rightModifiers = (v & 0x10) ? -1 : (int)(v & 0xf);
        return b;
    }
    bool ensureTap() {
        if (tap_) return true;
        const auto permissions = MSIMECurrentVoiceShortcutPermissions();
        if (!permissions.granted()) {
            if (!promptRequested_) {
                promptRequested_ = true;
                MSIMERequestVoiceShortcutPermissions(permissions);
            }
            if (!MSIMECurrentVoiceShortcutPermissions().granted()) return false;
        }
        const CGEventMask mask = CGEventMaskBit(kCGEventKeyDown) | CGEventMaskBit(kCGEventKeyUp);
        tap_ = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault, mask, callback, this);
        if (!tap_) return false;
        CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap_, 0);
        if (!source) { CFMachPortInvalidate(tap_); CFRelease(tap_); tap_ = nullptr; return false; }
        dispatch_semaphore_t ready = dispatch_semaphore_create(0);
        thread_ = [[NSThread alloc] initWithBlock:^{
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
            dispatch_semaphore_signal(ready);
            CFRunLoopRun();
        }];
        thread_.name = @"MSIMEVoiceShortcutTap";
        thread_.qualityOfService = NSQualityOfServiceUserInteractive;
        [thread_ start];
        dispatch_semaphore_wait(ready, DISPATCH_TIME_FOREVER);
        CFRelease(source);
        return true;
    }
    static CGEventRef callback(CGEventTapProxy, CGEventType type, CGEventRef event, void *context) {
        auto *self = static_cast<MSIMEVoiceShortcutTap *>(context);
        const uint64_t state = self->state_.load(std::memory_order_acquire);
        if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
            if ((state & (1ull << 31)) && self->tap_) CGEventTapEnable(self->tap_, true);
            return event;
        }
        if (!(state & (1ull << 31)) || !event) {
            self->swallow_.reset();
            self->seenState_ = 0;
            return event;
        }
        if (state != self->seenState_) { self->swallow_.reset(); self->seenState_ = state; }
        const auto binding = unpack(state & 0xffffffffull);
        const auto key = (unsigned short)CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode);
        const bool repeat = CGEventGetIntegerValueField(event, kCGKeyboardEventAutorepeat) != 0;
        const bool selfPosted = CGEventGetIntegerValueField(event, kCGEventSourceUserData) == MSIMEVoiceCommitEventTag;
        if (!self->swallow_.observe(type, key, CGEventGetFlags(event), repeat, selfPosted, binding)) return event;
        void (^handler)(NSEvent *) = self->currentHandler();
        CGEventRef copy = CGEventCreateCopy(event);
        if (copy) {
            NSEvent *snapshot = [NSEvent eventWithCGEvent:copy];
            CFRelease(copy);
            if (snapshot && handler) dispatch_async(dispatch_get_main_queue(), ^{ handler(snapshot); });
        }
        return NULL;
    }
    void setHandler(void (^handler)(NSEvent *)) {
        std::lock_guard<std::mutex> lock(handlerMutex_);
        handler_ = [handler copy];
    }
    void (^currentHandler())(NSEvent *) {
        std::lock_guard<std::mutex> lock(handlerMutex_);
        return handler_;
    }
    CFMachPortRef tap_ = nullptr;
    NSThread *thread_ = nil;
    std::mutex handlerMutex_;
    void (^handler_)(NSEvent *) = nil;
    uint64_t lastPacked_ = 0;
    bool promptRequested_ = false;
    std::atomic<uint64_t> state_{0};
    std::atomic<uint64_t> generation_{0};
    MSIMEVoiceShortcutSwallow swallow_;
    uint64_t seenState_ = 0;
};
