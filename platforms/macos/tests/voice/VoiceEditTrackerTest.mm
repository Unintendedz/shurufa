#import "../../src/voice/VoiceEditTracker.h"
#import "../../src/voice/VoiceLearnedToast.h"
#include "../../../common/HostApiString.h"
#include "msime_client.h"

#include <cassert>
#include <cstring>

namespace {

NSDictionary *CallManage(NSDictionary *payload) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    assert(data.length > 0);
    auto owned = msime::host_api::own_string(
        msime_client_voice_vocabulary_manage(static_cast<const uint8_t *>(data.bytes), data.length));
    assert(owned);
    NSData *raw = [NSData dataWithBytes:owned.get() length:std::strlen(owned.get())];
    NSDictionary *env = [NSJSONSerialization JSONObjectWithData:raw options:0 error:nil];
    assert([env isKindOfClass:NSDictionary.class]);
    return env;
}

void PumpRunLoopUntil(BOOL (^predicate)(void), NSTimeInterval timeoutSec) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeoutSec];
    while (!predicate() && [deadline timeIntervalSinceNow] > 0) {
        [NSRunLoop.currentRunLoop runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    }
}

NSEvent *KeyDownEvent(unsigned short keyCode) {
    return [NSEvent keyEventWithType:NSEventTypeKeyDown
                            location:NSZeroPoint
                       modifierFlags:0
                           timestamp:1.0
                        windowNumber:0
                             context:nil
                          characters:@""
         charactersIgnoringModifiers:@""
                           isARepeat:NO
                             keyCode:keyCode];
}

} // 匿名命名空间

int main() {
    @autoreleasepool {
        // 1. 气泡文案格式化校验。
        assert([[MSIMEVoiceLearnedToast formattedMessageForTerms:@[@"Claude"]]
            isEqualToString:@"已将「Claude」加入语音词库"]);
        NSArray<NSString *> *twoTerms = @[@"Claude", @"Shurufa"];
        assert([[MSIMEVoiceLearnedToast formattedMessageForTerms:twoTerms]
            isEqualToString:@"已将「Claude」「Shurufa」加入语音词库"]);
        assert([[MSIMEVoiceLearnedToast formattedMessageForTerms:@[]] isEqualToString:@""]);

        NSString *tempDir = [NSTemporaryDirectory()
            stringByAppendingPathComponent:[@"shurufa-edit-tracker-test-" stringByAppendingString:NSUUID.UUID.UUIDString]];
        assert([NSFileManager.defaultManager createDirectoryAtPath:tempDir withIntermediateDirectories:YES attributes:nil error:nil]);

        // 2. 完整闭环：语音上屏 -> 用户编辑纠错 -> 按回车发送（输入框变空）-> 自动学词 + 触发气泡 + 一键撤销还原。
        {
            MSIMEVoiceEditTracker *tracker = [MSIMEVoiceEditTracker new];
            tracker.initialSnapshotDelaySeconds = 0.02;
            tracker.editPollDebounceSeconds = 0.02;
            tracker.trackingWindowSeconds = 5.0;

            __block NSString *simulatedBox = @"早安，用可劳德写一下测试";
            tracker.fieldTextReader = ^NSString *(pid_t pid, NSString *bundleID) {
                (void)pid;
                (void)bundleID;
                return simulatedBox;
            };

            __block NSArray<NSString *> *learnedTerms = nil;
            __block void (^capturedUndo)(void) = nil;
            tracker.learnedHandler = ^(NSArray<NSString *> *terms, void (^undoHandler)(void)) {
                learnedTerms = terms;
                capturedUndo = [undoHandler copy];
            };

            [tracker beginTrackingInsertedText:@"用可劳德写一下测试"
                          preferencesDirectory:tempDir
                                     targetPID:12345
                                      bundleID:@"com.example.chat"
                                 isSecureField:NO
                                 axBlacklisted:NO];
            assert(tracker.tracking);

            // 等待初始快照在主队列落盘。
            PumpRunLoopUntil(^BOOL { return [tracker valueForKey:@"_originalBox"] != nil; }, 2.0);
            assert(tracker.tracking);

            // 模拟用户把「可劳德」改成「Claude」并等待防抖采样写回主队列。
            simulatedBox = @"早安，用Claude写一下测试";
            [tracker noteKeyEvent:KeyDownEvent(0)];
            PumpRunLoopUntil(^BOOL {
                return [[tracker valueForKey:@"_lastEditedBox"] isEqualToString:@"早安，用Claude写一下测试"];
            }, 2.0);

            // 模拟用户按 Enter 发送，输入框随之清空；应复用刚才的非空快照完成学词。
            simulatedBox = @"";
            [tracker noteKeyEvent:KeyDownEvent(36)];
            PumpRunLoopUntil(^BOOL { return learnedTerms != nil; }, 1.0);

            assert(!tracker.tracking);
            assert(learnedTerms.count == 1);
            assert([learnedTerms[0] isEqualToString:@"Claude"]);
            assert(capturedUndo != nil);

            // 核验 voice-vocabulary.json 已持久化。
            NSDictionary *listResp = CallManage(@{@"action" : @"list", @"preferences_directory" : tempDir});
            assert([listResp[@"ok"] isEqual:@YES]);
            NSArray *terms = listResp[@"value"][@"terms"];
            assert(terms.count == 1);
            assert([terms[0][@"canonical"] isEqualToString:@"Claude"]);
            assert([terms[0][@"source"] isEqualToString:@"auto_learned"]);

            // 执行气泡「撤销」回调，核验词条被精确清除。
            __block BOOL gotChangeNotification = NO;
            id observer = [NSNotificationCenter.defaultCenter
                addObserverForName:MSIMEVoiceVocabularyDidChangeNotification
                            object:nil
                             queue:NSOperationQueue.mainQueue
                        usingBlock:^(NSNotification * _Nonnull note) {
                            (void)note;
                            gotChangeNotification = YES;
                        }];
            capturedUndo();
            PumpRunLoopUntil(^BOOL { return gotChangeNotification; }, 1.0);
            [NSNotificationCenter.defaultCenter removeObserver:observer];

            NSDictionary *afterUndo = CallManage(@{@"action" : @"list", @"preferences_directory" : tempDir});
            assert([afterUndo[@"ok"] isEqual:@YES]);
            assert([afterUndo[@"value"][@"terms"] count] == 0);
        }

        // 3. 密码框与 AX 黑名单应用必须直接跳过，绝不启动追踪。
        {
            MSIMEVoiceEditTracker *tracker = [MSIMEVoiceEditTracker new];
            tracker.fieldTextReader = ^NSString *(pid_t pid, NSString *bundleID) {
                (void)pid;
                (void)bundleID;
                return @"should never be called";
            };
            [tracker beginTrackingInsertedText:@"测试密码"
                          preferencesDirectory:tempDir
                                     targetPID:12345
                                      bundleID:@"com.example.app"
                                 isSecureField:YES
                                 axBlacklisted:NO];
            assert(!tracker.tracking);

            [tracker beginTrackingInsertedText:@"测试黑名单"
                          preferencesDirectory:tempDir
                                     targetPID:12345
                                      bundleID:@"com.tencent.xinWeChat"
                                 isSecureField:NO
                                 axBlacklisted:YES];
            assert(!tracker.tracking);
        }

        [NSFileManager.defaultManager removeItemAtPath:tempDir error:nil];
    }
    return 0;
}
