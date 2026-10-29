#pragma once
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <IOKit/hidsystem/IOHIDLib.h>

// 与外接键盘 Caps Lock 共用输入监控授权；只在主线程检查，系统询问不阻塞输入。
static inline bool MSIMEEnsureInputMonitoringAccess() {
    static bool accessRequested = false;
    const IOHIDAccessType access = IOHIDCheckAccess(kIOHIDRequestTypeListenEvent);
    if (access == kIOHIDAccessTypeUnknown && !accessRequested) {
        accessRequested = true;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            IOHIDRequestAccess(kIOHIDRequestTypeListenEvent);
        });
    }
    return access == kIOHIDAccessTypeGranted;
}

// 录音期间的只读监听；不改写、不吞掉、不向系统注入任何按键。
static inline void MSIMEQueueVoiceRecordingEvent(void (^handler)(NSEvent *), CGEventRef event) {
    NSEvent *snapshot = [NSEvent eventWithCGEvent:event];
    dispatch_async(dispatch_get_main_queue(), ^{ handler(snapshot); });
}

class MSIMEVoiceRecordingMonitor {
public:
    ~MSIMEVoiceRecordingMonitor() { stop(); }
    bool start(void (^handler)(NSEvent *)) {
        stop();
        if (!MSIMEEnsureInputMonitoringAccess()) return false;
        handler_ = [handler copy];
        const CGEventMask mask = CGEventMaskBit(kCGEventKeyDown) | CGEventMaskBit(kCGEventKeyUp) | CGEventMaskBit(kCGEventFlagsChanged);
        tap_ = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionListenOnly,
            mask, callback, this);
        if (!tap_) { handler_ = nil; return false; }
        source_ = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap_, 0);
        if (!source_) { stop(); return false; }
        CFRunLoopAddSource(CFRunLoopGetMain(), source_, kCFRunLoopCommonModes);
        CGEventTapEnable(tap_, true);
        return true;
    }
    void stop() {
        if (source_) { CFRunLoopRemoveSource(CFRunLoopGetMain(), source_, kCFRunLoopCommonModes); CFRelease(source_); source_ = nullptr; }
        if (tap_) { CFMachPortInvalidate(tap_); CFRelease(tap_); tap_ = nullptr; }
        handler_ = nil;
    }
private:
    static CGEventRef callback(CGEventTapProxy, CGEventType type, CGEventRef event, void *context) {
        auto *monitor = static_cast<MSIMEVoiceRecordingMonitor *>(context);
        if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
            CFMachPortRef tap = monitor->tap_;
            if (tap) {
                CFRetain(tap);
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (CFMachPortIsValid(tap)) CGEventTapEnable(tap, true);
                    CFRelease(tap);
                });
            }
        } else if (monitor->handler_ && event) {
            // 先让 tap 返回。停止音频、锁和设备拆卸只能在随后的主队列任务中执行。
            MSIMEQueueVoiceRecordingEvent(monitor->handler_, event);
        }
        return event;
    }
    CFMachPortRef tap_ = nullptr;
    CFRunLoopSourceRef source_ = nullptr;
    void (^handler_)(NSEvent *) = nil;
};
