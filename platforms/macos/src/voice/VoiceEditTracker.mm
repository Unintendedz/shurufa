#import "VoiceEditTracker.h"
#import "VoiceContextCapture.h"
#import "VoiceLearnedToast.h"
#import "../core/AXPrecedingContext.h"
#include "../../../common/HostApiString.h"
#include "msime_client.h"

#import <ApplicationServices/ApplicationServices.h>
#import <Carbon/Carbon.h>
#include <cstring>
#include <memory>

NSNotificationName const MSIMEVoiceVocabularyDidChangeNotification = @"MSIMEVoiceVocabularyDidChangeNotification";

namespace {

constexpr NSTimeInterval kDefaultTrackingWindowSec = 15.0;
constexpr NSTimeInterval kDefaultInitialDelaySec = 0.12;
constexpr NSTimeInterval kDefaultEditDebounceSec = 0.30;
constexpr NSUInteger kMaxTrackedFieldUTF16 = 8000;
constexpr float kTrackerAXTimeoutSec = 0.015f;

using ScopedAXElement = std::unique_ptr<const __AXUIElement, decltype(&CFRelease)>;

/// 在后台队列读取目标进程焦点输入框的完整文本（密码框、安全输入态与 AX 黑名单硬拒绝）。
NSString * _Nullable DefaultReadFocusedFieldText(pid_t targetPID, NSString * _Nullable bundleID) {
    if (IsSecureEventInputEnabled() || !AXIsProcessTrusted()) return nil;
    if (targetPID <= 0 || targetPID == NSProcessInfo.processInfo.processIdentifier) return nil;
    if (MSIMEIsAXBlacklistedBundleIdentifier(bundleID)) return nil;

    ScopedAXElement app(AXUIElementCreateApplication(targetPID), CFRelease);
    if (!app || AXUIElementSetMessagingTimeout(app.get(), kTrackerAXTimeoutSec) != kAXErrorSuccess) {
        return nil;
    }
    CFTypeRef rawFocus = nullptr;
    if (AXUIElementCopyAttributeValue(app.get(), kAXFocusedUIElementAttribute, &rawFocus) != kAXErrorSuccess || !rawFocus) {
        if (rawFocus) CFRelease(rawFocus);
        return nil;
    }
    id focusObj = CFBridgingRelease(rawFocus);
    if (CFGetTypeID((__bridge CFTypeRef)focusObj) != AXUIElementGetTypeID()) return nil;
    AXUIElementRef focus = (__bridge AXUIElementRef)focusObj;
    pid_t focusPID = 0;
    if (AXUIElementGetPid(focus, &focusPID) != kAXErrorSuccess || focusPID != targetPID) return nil;
    if (AXUIElementSetMessagingTimeout(focus, kTrackerAXTimeoutSec) != kAXErrorSuccess) return nil;
    if (msime::mac::IsAXElementSecureTextField(focus)) return nil;
    return msime::mac::ReadAXFullFieldTextFromElement(focus, kMaxTrackedFieldUTF16);
}

id _Nullable CallVocabularyManageJSON(NSDictionary<NSString *, id> *payload) {
    if (![NSJSONSerialization isValidJSONObject:payload]) return nil;
    NSData *body = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    if (!body.length) return nil;
    auto owned = msime::host_api::own_string(
        msime_client_voice_vocabulary_manage(static_cast<const uint8_t *>(body.bytes), body.length));
    if (!owned) return nil;
    NSData *raw = [NSData dataWithBytes:owned.get() length:std::strlen(owned.get())];
    NSDictionary *envelope = [NSJSONSerialization JSONObjectWithData:raw options:0 error:nil];
    if (![envelope isKindOfClass:NSDictionary.class] || ![envelope[@"ok"] isEqual:@YES]) {
        return nil;
    }
    return envelope[@"value"];
}

BOOL IsReturnOrEnterKeyCode(unsigned short keyCode) {
    return keyCode == 36 || keyCode == 76;
}

} // 匿名命名空间

@implementation MSIMEVoiceEditTracker {
    dispatch_queue_t _workQueue;
    uint64_t _generation;
    BOOL _tracking;
    NSString *_insertedText;
    NSString *_preferencesDirectory;
    pid_t _targetPID;
    NSString *_bundleID;
    NSString *_originalBox;
    NSString *_lastEditedBox;
    dispatch_source_t _windowTimer;
    dispatch_source_t _pollTimer;
}

- (instancetype)init {
    self = [super init];
    if (!self) return nil;
    _workQueue = dispatch_queue_create("dev.shurufa.voice-edit-tracker", DISPATCH_QUEUE_SERIAL);
    _trackingWindowSeconds = kDefaultTrackingWindowSec;
    _initialSnapshotDelaySeconds = kDefaultInitialDelaySec;
    _editPollDebounceSeconds = kDefaultEditDebounceSec;
    return self;
}

- (void)dealloc {
    [self cancelTimersOnMain];
}

- (BOOL)isTracking {
    return _tracking;
}

- (void)cancelTimersOnMain {
    if (_windowTimer) {
        dispatch_source_cancel(_windowTimer);
        _windowTimer = nil;
    }
    if (_pollTimer) {
        dispatch_source_cancel(_pollTimer);
        _pollTimer = nil;
    }
}

- (void)clearSessionOnMain {
    ++_generation;
    _tracking = NO;
    [self cancelTimersOnMain];
    _insertedText = nil;
    _preferencesDirectory = nil;
    _targetPID = 0;
    _bundleID = nil;
    _originalBox = nil;
    _lastEditedBox = nil;
}

- (void)cancel {
    if ([NSThread isMainThread]) {
        [self clearSessionOnMain];
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            [self clearSessionOnMain];
        });
    }
}

- (void)beginTrackingInsertedText:(NSString *)insertedText
             preferencesDirectory:(NSString *)preferencesDirectory
                        targetPID:(pid_t)targetPID
                         bundleID:(nullable NSString *)bundleID
                    isSecureField:(BOOL)isSecureField
                    axBlacklisted:(BOOL)axBlacklisted {
    [self clearSessionOnMain];
    if (isSecureField || axBlacklisted) return;
    if (![insertedText isKindOfClass:NSString.class] || !insertedText.length) return;
    if (![preferencesDirectory isKindOfClass:NSString.class] || !preferencesDirectory.length) return;
    if (targetPID <= 0 && !self.fieldTextReader) return;
    if (MSIMEIsAXBlacklistedBundleIdentifier(bundleID)) return;

    const uint64_t gen = _generation;
    _tracking = YES;
    _insertedText = [insertedText copy];
    _preferencesDirectory = [preferencesDirectory copy];
    _targetPID = targetPID;
    _bundleID = [bundleID copy] ?: @"";
    _originalBox = nil;
    _lastEditedBox = nil;

    // 15 秒总窗口到期后自动结算并释放资源。
    __weak MSIMEVoiceEditTracker *weakSelf = self;
    dispatch_source_t windowTimer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    const uint64_t windowNs = static_cast<uint64_t>(MAX(0.05, self.trackingWindowSeconds) * NSEC_PER_SEC);
    dispatch_source_set_timer(windowTimer, dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(windowNs)), DISPATCH_TIME_FOREVER, 20 * NSEC_PER_MSEC);
    dispatch_source_set_event_handler(windowTimer, ^{
        MSIMEVoiceEditTracker *strongSelf = weakSelf;
        if (!strongSelf || !strongSelf->_tracking || strongSelf->_generation != gen) return;
        [strongSelf finalizeTrackingWithReason:@"timeout"];
    });
    _windowTimer = windowTimer;
    dispatch_resume(windowTimer);

    // 延迟极短时间待宿主把刚插入的文本写入 AXValue 后采集初始输入框全文。
    NSString * _Nullable (^reader)(pid_t, NSString *) = [self.fieldTextReader copy];
    NSString *capturedInserted = _insertedText;
    NSString *capturedBundle = _bundleID;
    const pid_t capturedPID = _targetPID;
    const int64_t initDelayNs = static_cast<int64_t>(MAX(0.0, self.initialSnapshotDelaySeconds) * NSEC_PER_SEC);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, initDelayNs), _workQueue, ^{
        NSString *box = reader ? reader(capturedPID, capturedBundle) : DefaultReadFocusedFieldText(capturedPID, capturedBundle);
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEVoiceEditTracker *strongSelf = weakSelf;
            if (!strongSelf || !strongSelf->_tracking || strongSelf->_generation != gen) return;
            if (!box.length || ![box containsString:capturedInserted]) {
                // 无法在当前输入框定位刚上屏文本（如终端、游戏或不支持 AXValue 的控件），直接安静结束追踪。
                [strongSelf clearSessionOnMain];
                return;
            }
            strongSelf->_originalBox = [box copy];
            strongSelf->_lastEditedBox = [box copy];
        });
    });
}

- (void)noteKeyEvent:(NSEvent *)event {
    if (!_tracking || !event || event.type != NSEventTypeKeyDown) return;
    if (IsReturnOrEnterKeyCode(event.keyCode)) {
        [self finalizeTrackingWithReason:@"return_key"];
        return;
    }
    // Esc 直接放弃本次追踪（用户通常在取消/关闭弹窗）。
    if (event.keyCode == 53) {
        [self clearSessionOnMain];
        return;
    }
    [self scheduleDebouncedEditPoll];
}

- (void)noteClientDeactivated {
    if (!_tracking) return;
    [self finalizeTrackingWithReason:@"blur"];
}

- (void)scheduleDebouncedEditPoll {
    if (!_tracking) return;
    if (_pollTimer) {
        dispatch_source_cancel(_pollTimer);
        _pollTimer = nil;
    }
    const uint64_t gen = _generation;
    __weak MSIMEVoiceEditTracker *weakSelf = self;
    dispatch_source_t pollTimer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    const uint64_t delayNs = static_cast<uint64_t>(MAX(0.01, self.editPollDebounceSeconds) * NSEC_PER_SEC);
    dispatch_source_set_timer(pollTimer, dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(delayNs)), DISPATCH_TIME_FOREVER, 10 * NSEC_PER_MSEC);
    dispatch_source_set_event_handler(pollTimer, ^{
        MSIMEVoiceEditTracker *strongSelf = weakSelf;
        if (!strongSelf || !strongSelf->_tracking || strongSelf->_generation != gen) return;
        [strongSelf pollCurrentFieldOnBackgroundForGeneration:gen];
    });
    _pollTimer = pollTimer;
    dispatch_resume(pollTimer);
}

- (void)pollCurrentFieldOnBackgroundForGeneration:(uint64_t)gen {
    NSString * _Nullable (^reader)(pid_t, NSString *) = [self.fieldTextReader copy];
    const pid_t pid = _targetPID;
    NSString *bundle = _bundleID;
    __weak MSIMEVoiceEditTracker *weakSelf = self;
    dispatch_async(_workQueue, ^{
        NSString *currentBox = reader ? reader(pid, bundle) : DefaultReadFocusedFieldText(pid, bundle);
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEVoiceEditTracker *strongSelf = weakSelf;
            if (!strongSelf || !strongSelf->_tracking || strongSelf->_generation != gen) return;
            if (currentBox == nil) return;
            NSString *trimmed = [currentBox stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
            if (!trimmed.length && strongSelf->_originalBox.length) {
                // 输入框突然清空（典型如聊天软件点发送按钮或快捷键发送），立刻用上一次非空编辑快照结算。
                [strongSelf finalizeTrackingWithReason:@"box_cleared"];
                return;
            }
            if (trimmed.length) {
                strongSelf->_lastEditedBox = [currentBox copy];
            }
        });
    });
}

- (void)finalizeTrackingWithReason:(NSString *)reason {
    (void)reason;
    if (!_tracking) return;
    NSString *inserted = _insertedText;
    NSString *prefsDir = _preferencesDirectory;
    NSString *originalBox = _originalBox;
    NSString *cachedEdited = _lastEditedBox;
    const pid_t pid = _targetPID;
    NSString *bundle = _bundleID;
    NSString * _Nullable (^reader)(pid_t, NSString *) = [self.fieldTextReader copy];
    void (^customLearnedHandler)(NSArray<NSString *> *, void (^)(void)) = [self.learnedHandler copy];

    [self clearSessionOnMain];
    if (!inserted.length || !prefsDir.length || !originalBox.length) return;

    dispatch_queue_t workQueue = _workQueue;
    dispatch_async(workQueue, ^{
        // 结算瞬间尽量再读一次当前输入框（覆盖“改完最后一个字立刻按 Enter”还没来得及跑防抖采样的情况）。
        // 如果宿主已把输入框清空（已发送），则自动回退到最后一次非空缓存 `cachedEdited`。
        NSString *liveBox = reader ? reader(pid, bundle) : DefaultReadFocusedFieldText(pid, bundle);
        NSString *liveTrimmed = [liveBox stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        NSString *editedBox = liveTrimmed.length ? liveBox : cachedEdited;
        if (!editedBox.length || [editedBox isEqualToString:originalBox]) return;

        NSDictionary<NSString *, id> *payload = @{
            @"action" : @"learn",
            @"preferences_directory" : prefsDir,
            @"inserted_text" : inserted,
            @"original_box" : originalBox,
            @"edited_box" : editedBox,
        };
        id value = CallVocabularyManageJSON(payload);
        if (![value isKindOfClass:NSDictionary.class]) return;
        NSArray *learned = [value[@"learned"] isKindOfClass:NSArray.class] ? value[@"learned"] : @[];
        if (!learned.count) return;

        NSMutableArray<NSString *> *termNames = [NSMutableArray arrayWithCapacity:learned.count];
        NSMutableArray<NSDictionary<NSString *, id> *> *undoSnapshots = [NSMutableArray arrayWithCapacity:learned.count];
        for (id item in learned) {
            if (![item isKindOfClass:NSDictionary.class]) continue;
            NSString *canonical = item[@"canonical"];
            if (![canonical isKindOfClass:NSString.class] || !canonical.length) continue;
            [termNames addObject:canonical];
            NSMutableDictionary<NSString *, id> *undoItem = [@{@"canonical" : canonical} mutableCopy];
            if ([item[@"previous_term"] isKindOfClass:NSDictionary.class]) {
                undoItem[@"previous_term"] = item[@"previous_term"];
            }
            [undoSnapshots addObject:undoItem];
        }
        if (!termNames.count) return;

        void (^undoBlock)(void) = ^{
            dispatch_async(workQueue, ^{
                for (NSDictionary<NSString *, id> *entry in undoSnapshots) {
                    NSMutableDictionary<NSString *, id> *restoreReq = [@{
                        @"action" : @"restore",
                        @"preferences_directory" : prefsDir,
                        @"canonical" : entry[@"canonical"],
                    } mutableCopy];
                    if (entry[@"previous_term"]) {
                        restoreReq[@"previous_term"] = entry[@"previous_term"];
                    }
                    (void)CallVocabularyManageJSON(restoreReq);
                }
                dispatch_async(dispatch_get_main_queue(), ^{
                    [NSNotificationCenter.defaultCenter postNotificationName:MSIMEVoiceVocabularyDidChangeNotification
                                                                      object:nil];
                });
            });
        };

        dispatch_async(dispatch_get_main_queue(), ^{
            [NSNotificationCenter.defaultCenter postNotificationName:MSIMEVoiceVocabularyDidChangeNotification
                                                              object:nil];
            if (customLearnedHandler) {
                customLearnedHandler([termNames copy], undoBlock);
            } else {
                [[MSIMEVoiceLearnedToast sharedToast] showLearnedTerms:[termNames copy] undoHandler:undoBlock];
            }
        });
    });
}

@end
