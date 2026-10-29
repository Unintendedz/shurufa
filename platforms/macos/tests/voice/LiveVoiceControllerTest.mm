#import "../../src/input/InputController.mm"
#import "VoiceCueFixture.h"
#import "VoiceClientFixture.h"
#import "VoiceMeterFixture.h"
#include <cassert>
#include <cstring>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#import <objc/runtime.h>

@interface LiveCaptureFixture : MSIMEVoiceInputService
@property(copy) MSIMEVoiceAudioBuffer bufferHandler;
@property(copy) MSIMEVoicePCMChunk chunk;
@property NSTimeInterval capturedSeconds;
@property(copy) void (^transcript)(NSString *, BOOL);
@property NSUInteger captureStops;
@property NSUInteger transcriptionStops;
@property BOOL needsMicrophonePermission;
@property BOOL needsSpeechPermission;
@property NSUInteger microphoneRequests;
@property NSUInteger speechRequests;
@property(copy) void (^speechPermission)(BOOL);
@property BOOL failCapture;
@property(copy) void (^permission)(BOOL);
// 最近一次采集用的麦克风 UID，nil 是系统默认设备。
@property(copy) NSString *captureDevice;
@end
@implementation LiveCaptureFixture
- (NSTimeInterval)recordedDuration { return self.capturedSeconds; }
- (AVAuthorizationStatus)microphoneAuthorizationStatus { return self.needsMicrophonePermission ? AVAuthorizationStatusNotDetermined : AVAuthorizationStatusAuthorized; }
- (void)requestMicrophonePermission:(void (^)(BOOL))completion { ++self.microphoneRequests; self.permission = completion; }
- (SFSpeechRecognizerAuthorizationStatus)speechAuthorizationStatus { return self.needsSpeechPermission ? SFSpeechRecognizerAuthorizationStatusNotDetermined : SFSpeechRecognizerAuthorizationStatusAuthorized; }
- (void)requestSpeechPermission:(void (^)(BOOL))completion { ++self.speechRequests; self.speechPermission = completion; }
- (BOOL)startTranscriptionWithLanguage:(NSString *)language textHandler:(void (^)(NSString *, BOOL))handler error:(NSError **)error {
    (void)language; (void)error; self.transcript = handler; return YES;
}
- (BOOL)startMicrophoneCapture:(MSIMEVoiceAudioBuffer)handler deviceUID:(NSString *)device error:(NSError **)error {
    (void)error; self.captureDevice = device; self.bufferHandler = handler; return !self.failCapture;
}
- (BOOL)startPCMStreaming:(MSIMEVoicePCMChunk)handler deviceUID:(NSString *)device error:(NSError **)error {
    (void)device; (void)error; self.chunk = handler; return YES;
}
- (NSData *)finishPCMStreamingWithError:(NSError **)error {
    (void)error; ++self.captureStops; return [NSData data];
}
- (void)stopMicrophoneCapture { ++self.captureStops; }
- (void)stopTranscription { ++self.transcriptionStops; }
@end
@interface LiveTextFixture : NSObject <MSIMETextClient>
@property(copy) NSString *marked;
@property NSMutableArray *commits;
@property NSUInteger unmarks;
@end
@implementation LiveTextFixture
- (void)unmarkText { ++self.unmarks; self.marked = @""; }
// The controller asks the client for the caret whenever it repositions the overlay; an empty rectangle reads as "no usable position".
- (NSDictionary *)attributesForCharacterIndex:(NSUInteger)index lineHeightRectangle:(NSRect *)rectangle {
    (void)index; if (rectangle) *rectangle = NSZeroRect; return @{};
}
- (void)insertText:(id)text replacementRange:(NSRange)range {
    (void)range; if (!self.commits) self.commits = [NSMutableArray array]; [self.commits addObject:text]; self.marked = @"";
}
- (void)setMarkedText:(id)text selectionRange:(NSRange)selection replacementRange:(NSRange)replacement {
    (void)selection; (void)replacement; self.marked = text;
}
@end
@interface LivePresentationFixture : NSObject
@property(copy) NSString *detail;
// The controller picks the overlay screen from the caret on every failure; model the real overlay property so the assignment lands somewhere.
@property(nonatomic, weak) NSScreen *preferredScreen;
@property float lastLevel;
@property NSUInteger levelUpdates;
@property(copy) void (^actionHandler)(BOOL);
@property BOOL dismissed;
@property(copy) NSString *preview;
@property NSUInteger phase;
@property NSUInteger failure;
@property NSUInteger failures;
@property BOOL locked;
- (void)setListening:(BOOL)listening;
- (void)setProcessing:(BOOL)polishing;
- (void)restore;
@end
@implementation LivePresentationFixture
- (void)setPersistentRecording:(BOOL)value { (void)value; }
- (void)setShortcutMonitoringAvailable:(BOOL)value { (void)value; }
- (void)showCopyResult:(BOOL)copied { self.preview = copied ? @"已复制" : @"复制失败"; }
- (void)setInputLevel:(float)level { self.lastLevel = level; ++self.levelUpdates; }
- (void)dismissProcessing { self.dismissed = YES; }
- (void)setListening:(BOOL)listening { self.phase = listening ? 1 : 0; self.failure = 0; self.preview = @""; self.locked = NO; }
- (void)setRecordingLocked:(BOOL)locked { self.locked = locked; }
- (void)setTranscript:(NSString *)text { self.preview = text; }
- (void)showFailure:(MSIMEVoiceFailure)failure detail:(NSString *)detail { self.failure = failure; self.detail = detail; self.phase = 4; ++self.failures; self.preview = @""; }
- (void)dismissFailure { if (self.failure) [self setListening:NO]; }
- (void)setProcessing:(BOOL)polishing { self.phase = polishing ? 3 : 2; }
- (void)applyThemePreferences:(NSDictionary *)preferences { (void)preferences; }
- (void)restore {}
@end
@interface LivePolishFixture : NSObject
@property(copy) void (^completion)(NSString *, NSError *);
@property(copy) NSString *input;
@property NSUInteger submissions;
@property NSUInteger cancellations;
@property BOOL failStart;
@end
@implementation LivePolishFixture
- (BOOL)polishText:(NSString *)text completion:(void (^)(NSString *, NSError *))completion error:(NSError **)error {
    (void)error; ++self.submissions; self.input = text; self.completion = completion; return !self.failStart;
}
- (void)cancel { ++self.cancellations; }
@end
@interface LiveControllerFixture : MSIMEInputController
@property BOOL usePolishFixture;
@property LivePolishFixture *polishFixture;
@property(copy) NSDictionary *polishOptions;
@property NSUInteger externalCommits;
@end
@implementation LiveControllerFixture
- (MSIMEVoiceCommitOutcome)postVoiceText:(NSString *)text route:(const MSIMEVoiceCommitRoute &)route {
    assert([text isEqual:@"synthetic routed"] && ![route.mode isEqual:@"tsf"]);
    ++self.externalCommits; return MSIMEVoiceCommitOutcome::posted;
}
- (void)ensureAppearance {}
- (void)apply:(NSDictionary *)transition { MSIMEApplyTransition(transition, [self valueForKey:@"activeClient"]); }
- (MSIMEHTTPVoiceRequest *)makeLiveVoicePolishRequest:(NSDictionary *)options {
    if (!self.usePolishFixture) return [super makeLiveVoicePolishRequest:options];
    self.polishOptions = options; self.polishFixture = [LivePolishFixture new]; return (id)self.polishFixture;
}
@end
// TextStream 工厂后的流协议替身；控制器继续走实际的采集、失焦和最终投递路径。
@interface PersistentStreamFixture : NSObject <MSIMEStreamingVoiceRequest>
@property(copy) MSIMEDoubaoResult result;
@property NSUInteger finishes;
@property NSUInteger cancellations;
@end
@implementation PersistentStreamFixture
- (BOOL)startWithResult:(MSIMEDoubaoResult)result error:(NSError **)error { (void)error; self.result = result; return YES; }
- (BOOL)appendPCM:(NSData *)pcm error:(NSError **)error { (void)pcm; (void)error; return YES; }
- (BOOL)finishWithError:(NSError **)error { (void)error; ++self.finishes; return YES; }
- (void)cancel { ++self.cancellations; }
@end
@interface PersistentControllerFixture : LiveControllerFixture
@property(copy) void (^recordingMonitor)(NSEvent *);
@property BOOL monitorDenied;
@property BOOL copyFails;
@property PersistentStreamFixture *streamFixture;
@property(copy) NSString *copiedText;
@end
@implementation PersistentControllerFixture
- (BOOL)startVoiceRecordingMonitor:(void (^)(NSEvent *))handler { self.recordingMonitor = handler; return !self.monitorDenied; }
- (NSString *)selectedSystemInputModeID { return MSIMEChineseInputModeID; }
- (id<MSIMEStreamingVoiceRequest>)makeTextStreamVoiceRequest:(NSDictionary *)options error:(NSError **)error {
    (void)options; (void)error; self.streamFixture = [PersistentStreamFixture new]; return self.streamFixture;
}
- (MSIMEHTTPVoiceRequest *)makeDoubaoPolishRequest:(NSDictionary *)options { return [self makeLiveVoicePolishRequest:options]; }
- (BOOL)copyVoiceText:(NSString *)text { if (self.copyFails) return NO; self.copiedText = text; return YES; }
@end
static NSBundle *PersistentTestBundle;
static id PersistentMainBundle(id, SEL) { return PersistentTestBundle; }
static NSUInteger PersistentBaseDeactivations;
// 夹具没有 IMKServer，只替换系统基类；仍执行产品完整的失焦处理。
static void PersistentBaseDeactivate(id, SEL, id) { ++PersistentBaseDeactivations; }
// The socket route only engages when the configured path is a real Unix socket (MSIMEVoiceProviderSocketFromConfiguration rejects regular and stale files), so the test has to bind one rather than hope one is lying around - without it the controller quietly falls back to native Speech and every provider assertion below fails for a reason that has nothing to do with the code.
static NSString *MSIMESyntheticVoiceSocket(void)
{
    NSString *path = NSProcessInfo.processInfo.environment[@"MSIME_VOICE_PROVIDER_SOCKET"];
    return path.isAbsolutePath ? path : @"/tmp/synthetic-live.sock";
}

static int MSIMEBindSyntheticVoiceSocket(NSString *path)
{
    const char *bytes = path.fileSystemRepresentation;
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    assert(std::strlen(bytes) < sizeof(address.sun_path));
    std::strcpy(address.sun_path, bytes);
    unlink(bytes);
    int descriptor = socket(AF_UNIX, SOCK_STREAM, 0);
    assert(descriptor >= 0);
    int bound = bind(descriptor, reinterpret_cast<const sockaddr *>(&address), sizeof(address));
    int listening = bound == 0 ? listen(descriptor, 1) : -1;
    assert(bound == 0 && listening == 0);
    return descriptor;
}

@interface LiveHostFixture : MSIMEClientSession
@property BOOL rejectVoiceResult;
@property(atomic) NSUInteger providerStops;
@property(atomic) NSUInteger providerCancels;
@property(atomic) BOOL providerStarted;
@property(atomic, copy) MSIMEVoiceProviderUpdate providerUpdate;
@property(atomic, copy) MSIMEVoiceProviderPhase providerPhase;
@property dispatch_semaphore_t providerDone;
@end
@implementation LiveHostFixture
- (NSDictionary *)applyVoiceText:(NSString *)text generation:(uint64_t)generation error:(NSError **)error {
    return self.rejectVoiceResult ? nil : [super applyVoiceText:text generation:generation error:error];
}
- (BOOL)voiceProviderStream:(NSDictionary *)query socket:(NSString *)socket update:(MSIMEVoiceProviderUpdate)update phase:(MSIMEVoiceProviderPhase)phase error:(NSError **)error {
    (void)error;
    assert([socket isEqual:MSIMESyntheticVoiceSocket()] && [query[@"generation"] unsignedLongLongValue]);
    self.providerUpdate = update; self.providerPhase = phase;
    phase(0); phase(0); // Duplicate recording notifications must not replay cues.
    self.providerStarted = YES;
    assert(dispatch_semaphore_wait(self.providerDone, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
    return YES;
}
- (BOOL)voiceProviderStopSocket:(NSString *)socket generation:(uint64_t)generation error:(NSError **)error {
    (void)error; assert([socket isEqual:MSIMESyntheticVoiceSocket()] && generation); self.providerStops += 1; return YES;
}
- (BOOL)voiceProviderCancelSocket:(NSString *)socket generation:(uint64_t)generation error:(NSError **)error {
    (void)error; assert([socket isEqual:MSIMESyntheticVoiceSocket()] && generation); self.providerCancels += 1;
    if (self.providerDone) dispatch_semaphore_signal(self.providerDone);
    return YES;
}
@end
int main(int argc, char **argv) {
    @autoreleasepool {
        NSString *root = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        NSMutableDictionary *options = [@{@"api_version": @1, @"preferences": @{@"scheme": @"quanpin", @"learning": @NO, @"candidate_page_size": @5, @"chinese_punctuation": @YES}} mutableCopy];
        for (NSString *name in @[@"resources", @"user_data", @"cache", @"dictionaries"]) {
            NSString *path = [root stringByAppendingPathComponent:name];
            assert([NSFileManager.defaultManager createDirectoryAtPath:path withIntermediateDirectories:YES attributes:nil error:nil]); options[name] = path;
        }
        LiveHostFixture *session = [[LiveHostFixture alloc] initWithOptions:options error:nil]; assert(session);
        LiveControllerFixture *controller = [LiveControllerFixture alloc];
        LiveCaptureFixture *capture = [LiveCaptureFixture new];
        capture.capturedSeconds = 0.25;
        LiveTextFixture *client = [LiveTextFixture new];
        LivePresentationFixture *presentation = [LivePresentationFixture new];
        MSIMEVoiceCueFixture *cues = [MSIMEVoiceCueFixture new];
        [controller setValue:session forKey:@"session"]; [controller setValue:client forKey:@"activeClient"];
        [controller setValue:capture forKey:@"voiceService"];
        for (NSString *key in @[@"voiceOverlay", @"voiceAudioMuter"]) [controller setValue:presentation forKey:key];
        [controller setValue:cues forKey:@"voiceCuePlayer"];
        NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
        NSDictionary *old = [defaults volatileDomainForName:NSArgumentDomain];
        NSMutableDictionary *voiceArguments = [@{@"MSIMEClientVoiceEnabled": @YES, @"MSIMEClientVoiceASRProvider": @"system", @"MSIMEClientVoiceSoundEnabled": @YES, @"MSIMEClientVoiceStartSound": @YES, @"MSIMEClientVoiceEndSound": @YES, @"MSIMEClientVoiceMuteSystemAudio": @NO, @"MSIMEClientVoiceStreamInlinePreedit": @YES, @"MSIMEClientVoiceHotkeyRightAlt": @YES, @"MSIMEClientVoiceHotkeyHoldSpace": @YES} mutableCopy];
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        // Never inherit real polish settings in the synthetic capture fixture.
        voiceArguments[@"MSIMEClientVoicePolish"] = @NO;
        voiceArguments[@"MSIMEClientVoicePolishText"] = @NO;
        voiceArguments[@"MSIMEClientVoicePolishToken"] = @"";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        if (argc == 3 && std::strcmp(argv[1], "--persistent") == 0) {
            PersistentTestBundle = [NSBundle bundleWithPath:[NSString stringWithUTF8String:argv[2]]];
            assert(PersistentTestBundle);
            Method method = class_getClassMethod(NSBundle.class, @selector(mainBundle));
            IMP original = method_setImplementation(method, (IMP)PersistentMainBundle);
            Method deactivate = class_getInstanceMethod(IMKInputController.class, @selector(deactivateServer:));
            IMP originalDeactivate = method_setImplementation(deactivate, (IMP)PersistentBaseDeactivate);
            assert(MSIMEEditionIsShurufa());
            PersistentControllerFixture *owner = [PersistentControllerFixture alloc];
            LiveCaptureFixture *source = [LiveCaptureFixture new]; source.capturedSeconds = 0.5;
            LiveTextFixture *origin = [LiveTextFixture new];
            LivePresentationFixture *overlay = [LivePresentationFixture new];
            [owner setValue:session forKey:@"session"]; [owner setValue:origin forKey:@"activeClient"];
            [owner setValue:source forKey:@"voiceService"];
            [owner setValue:overlay forKey:@"voiceOverlay"]; [owner setValue:overlay forKey:@"voiceAudioMuter"];
            [owner setValue:[MSIMEVoiceCueFixture new] forKey:@"voiceCuePlayer"];
            [owner toggleVoiceInput:nil];
            assert(source.active);
            source.transcript(@"synthetic partial before blur", NO);
            assert(origin.marked.length && !origin.commits.count);
            [owner deactivateServer:origin];
            assert(source.active && !origin.marked.length && !origin.commits.count && origin.unmarks == 1);
            source.transcript(@"synthetic partial after blur", NO);
            assert(source.active && [overlay.preview isEqual:@"synthetic partial after blur"] && !origin.marked.length);
            [owner cancelLiveVoiceInput];
            assert(!source.active && !origin.commits.count && !MSIMEVoiceRecordingOwner);
            // 从首个 partial 起只更新浮层；整理完成后才向编辑框提交一次。
            assert(!MSIMEVoiceStreamInlinePreeditEnabled(nil));
            voiceArguments[@"MSIMEClientVoiceStreamInlinePreedit"] = @NO;
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            [owner setValue:origin forKey:@"activeClient"]; MSIMEFocusedController = owner;
            owner.usePolishFixture = YES;
            [owner toggleVoiceInput:nil];
            source.transcript(@"synthetic overlay first partial", NO);
            assert(!origin.marked.length && !origin.commits.count && [overlay.preview isEqual:@"synthetic overlay first partial"]);
            overlay.actionHandler(NO);
            source.transcript(@"synthetic overlay final", YES);
            assert(!origin.commits.count && owner.polishFixture.submissions == 1);
            owner.polishFixture.completion(@"synthetic final polished once", nil);
            assert(origin.commits.count == 1 && [origin.commits.lastObject isEqual:@"synthetic final polished once"]);
            origin.commits = [NSMutableArray array]; owner.usePolishFixture = NO;
            voiceArguments[@"MSIMEClientVoiceStreamInlinePreedit"] = @YES;
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            puts("PASS overlay-only: first partial, no marked text, one polished commit");
            for (NSUInteger route=0; route<3; ++route) {
                LiveTextFixture *next = [LiveTextFixture new];
                PersistentControllerFixture *destination = [PersistentControllerFixture alloc];
                [destination setValue:next forKey:@"activeClient"];
                [owner setValue:origin forKey:@"activeClient"];
                origin.commits = [NSMutableArray array]; owner.copiedText = nil;
                owner.usePolishFixture = YES;
                [owner toggleVoiceInput:nil];
                auto finalReply = source.transcript;
                finalReply(@"synthetic draft", NO);
                [owner deactivateServer:origin];
                assert(source.active && !origin.marked.length && !origin.commits.count);
                finalReply(@"synthetic continued", NO);
                if (route == 0) { [owner setValue:origin forKey:@"activeClient"]; MSIMEFocusedController = owner; }
                else MSIMEFocusedController = route == 1 ? destination : nil;
                overlay.actionHandler(NO);
                assert([[owner valueForKey:@"liveVoiceProcessing"] boolValue]);
                finalReply(@"synthetic final", YES);
                assert(owner.polishFixture.submissions == 1);
                owner.polishFixture.completion(@"synthetic polished", nil);
                finalReply(@"synthetic late duplicate", YES);
                assert(!source.active && !MSIMEVoiceRecordingOwner && !origin.marked.length);
                assert(origin.commits.count == (route == 0 ? 1 : 0));
                assert(next.commits.count == (route == 1 ? 1 : 0));
                if (route == 2) assert([owner.copiedText isEqual:@"synthetic polished"] && [overlay.preview isEqual:@"已复制"]);
                else assert(owner.copiedText == nil);
            }
            puts("PASS persistent: blur, return/new client/clipboard, polish, duplicate final");
            // 事件仅作为状态机参数，不向系统注入按键。
            auto event = [](NSEventType type, unsigned short code, NSEventModifierFlags flags, NSTimeInterval time) {
                return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:flags timestamp:time windowNumber:0
                    context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:code];
            };
            voiceArguments[@"MSIMEClientVoiceHotkeyMode"] = @"toggle";
            voiceArguments[@"MSIMEClientVoiceMacOSHotkey"] = @{@"key_code":@123,@"modifiers":@2};
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            void (^previousMonitor)(NSEvent *) = nil;
            for (NSUInteger action = 0; action < 3; ++action) {
                [owner setValue:origin forKey:@"activeClient"];
                origin.commits = [NSMutableArray array]; owner.copiedText = nil;
                owner.monitorDenied = action == 2;
                [owner handleConfiguredVoiceShortcut:event(NSEventTypeKeyDown,123,NSEventModifierFlagOption,10+action*10)];
                assert(source.active);
                // 上一代 tap 排队的 Esc 不能取消新录音。
                if (previousMonitor) previousMonitor(event(NSEventTypeKeyDown,53,0,99));
                assert(source.active);
                previousMonitor = owner.recordingMonitor;
                auto finalReply = source.transcript;
                finalReply(@"synthetic before shortcut", NO);
                [owner deactivateServer:origin];
                MSIMEFocusedController = nil;
                if (action == 0) {
                    // 缺 KeyUp，结束键从录音全局监听到达；同一事件重复到达只结束一次。
                    NSEvent *finish = event(NSEventTypeKeyDown,123,NSEventModifierFlagOption,11);
                    const NSUInteger stops = source.captureStops;
                    MSIMEQueueVoiceRecordingEvent(owner.recordingMonitor, finish.CGEvent);
                    assert(source.captureStops == stops && ![[owner valueForKey:@"liveVoiceProcessing"] boolValue]);
                    MSIMEVoiceMeterFixturePump();
                    assert(source.captureStops == stops + 1);
                    owner.recordingMonitor(finish); owner.recordingMonitor(finish);
                    assert(source.active && source.captureStops == stops + 1);
                    assert([[owner valueForKey:@"liveVoiceProcessing"] boolValue]);
                    finalReply(@"synthetic shortcut final", YES);
                    assert(owner.polishFixture.submissions == 1);
                    owner.polishFixture.completion(@"synthetic shortcut polished", nil);
                    assert([owner.copiedText isEqual:@"synthetic shortcut polished"]);
                } else {
                    if (action == 1) owner.recordingMonitor(event(NSEventTypeKeyDown,53,0,22));
                    else overlay.actionHandler(YES);
                    finalReply(@"synthetic cancelled late final", YES);
                    assert(!owner.polishFixture.submissions && !owner.copiedText && !origin.commits.count);
                }
                assert(!source.active && !MSIMEVoiceRecordingOwner);
            }
            puts("PASS persistent: global finish/Esc, missing KeyUp, denied-monitor cancel button");
            // 整理期间 Esc 仍可取消，最终结束后旧按住状态不能吞掉下一次右 Option。
            [owner setValue:origin forKey:@"activeClient"];
            owner.monitorDenied = NO;
            [owner toggleVoiceInput:nil];
            source.transcript(@"synthetic pending polish", YES);
            assert(source.active && owner.polishFixture.submissions == 1);
            owner.recordingMonitor(event(NSEventTypeKeyDown,53,0,60));
            assert(!source.active);
            voiceArguments[@"MSIMEClientVoiceHotkeyMode"] = @"hold";
            voiceArguments[@"MSIMEClientVoiceMacOSHotkey"] = @{@"key_code":@61,@"modifiers":@0};
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            const NSEventModifierFlags rightOption = NSEventModifierFlagOption | NX_DEVICERALTKEYMASK;
            [owner handleConfiguredVoiceShortcut:event(NSEventTypeFlagsChanged,61,rightOption,61)];
            assert(source.active);
            auto completedMonitor = owner.recordingMonitor;
            source.transcript(@"synthetic early final", YES);
            owner.polishFixture.completion(@"synthetic polished early", nil);
            assert(!source.active);
            [owner handleConfiguredVoiceShortcut:event(NSEventTypeFlagsChanged,61,rightOption,62)];
            assert(source.active && ![[owner valueForKey:@"liveVoiceProcessing"] boolValue]);
            completedMonitor(event(NSEventTypeFlagsChanged,61,0,61.5));
            assert(source.active && ![[owner valueForKey:@"liveVoiceProcessing"] boolValue]);
            owner.recordingMonitor(event(NSEventTypeFlagsChanged,61,0,62.5));
            assert([[owner valueForKey:@"liveVoiceProcessing"] boolValue]);
            [owner cancelLiveVoiceInput];
            voiceArguments[@"MSIMEClientVoiceHotkeyMode"] = @"toggle";
            voiceArguments[@"MSIMEClientVoiceMacOSHotkey"] = @{@"key_code":@123,@"modifiers":@2};
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            puts("PASS queued monitor: processing Esc, early final, new hold, stale KeyUp");
            // TextStream 使用实际工厂分支，替身只替换模型与麦克风。
            for (NSUInteger action = 0; action < 3; ++action) {
                [owner setValue:origin forKey:@"activeClient"];
                origin.commits = [NSMutableArray array]; owner.copiedText = nil;
                owner.monitorDenied = YES;
                uint64_t generation = 0;
                assert([source startWithSession:session generation:&generation error:nil]);
                [owner setValue:@(generation) forKey:@"voiceGeneration"];
                assert(([owner startDoubaoVoiceInputWithOptions:@{@"asr_provider":@"textstream",@"stream":@YES}]));
                PersistentStreamFixture *request = owner.streamFixture;
                request.result(@"synthetic textstream partial", NO, nil);
                [owner deactivateServer:origin];
                assert(source.active && !origin.marked.length && !origin.commits.count);
                request.result(@"synthetic textstream continued", NO, nil);
                assert([overlay.preview isEqual:@"synthetic textstream continued"]);
                MSIMEFocusedController = nil;
                if (action == 0) { [owner setValue:origin forKey:@"activeClient"]; MSIMEFocusedController = owner; }
                overlay.actionHandler(action == 2);
                if (action != 2) {
                    assert(request.finishes == 1);
                    request.result(@"synthetic textstream final", YES, nil);
                    assert(owner.polishFixture.submissions == 1);
                    owner.polishFixture.completion(@"synthetic textstream polished", nil);
                    request.result(@"synthetic duplicate", YES, nil);
                    assert(origin.commits.count == (action == 0 ? 1 : 0));
                    assert(action == 0 || [owner.copiedText isEqual:@"synthetic textstream polished"]);
                } else {
                    request.result(@"synthetic cancelled final", YES, nil);
                    assert(!owner.polishFixture.submissions && !origin.commits.count && !owner.copiedText);
                }
                assert(!source.active && !MSIMEVoiceRecordingOwner);
            }
            puts("PASS persistent: TextStream blur, return/clipboard, denied-monitor finish button, cancel");
            // 失败发生在失焦之后仍须明确反馈；最终插入应更新目标前文。
            [owner setValue:origin forKey:@"activeClient"];
            [owner toggleVoiceInput:nil];
            [owner deactivateServer:origin];
            [owner reportVoiceFailure:MSIMEVoiceFailureNoSpeech];
            const BOOL failureVisible = overlay.failure == MSIMEVoiceFailureNoSpeech;
            printf("persistent blur failure visible: %d\n", failureVisible);
            [owner setValue:origin forKey:@"activeClient"];
            MSIMEFocusedController = owner;
            owner.usePolishFixture = NO;
            [owner noteSmartPunctuationShadow:'1'];
            [owner toggleVoiceInput:nil];
            source.transcript(@"合成结果", YES);
            const BOOL shadowUpdated = [owner precedingForSmartPunctuation:origin] == u'果';
            printf("persistent final shadow updated: %d\n", shadowUpdated);
            fflush(stdout);
            assert(failureVisible && shadowUpdated);
            owner.copyFails = YES; owner.copiedText = nil;
            MSIMEFocusedController = nil;
            [owner toggleVoiceInput:nil];
            source.transcript(@"synthetic failed clipboard", YES);
            assert([overlay.preview isEqual:@"复制失败"] && !owner.copiedText && !source.active);
            puts("PASS persistent: blur error, punctuation shadow, clipboard failure");
            NSString *outputSuite = [@"shurufa.synthetic.voice-output." stringByAppendingString:NSUUID.UUID.UUIDString];
            NSUserDefaults *outputDefaults = [[NSUserDefaults alloc] initWithSuiteName:outputSuite];
            [outputDefaults setBool:YES forKey:@"MSIMEClientTraditionalOutput"];
            [owner setValue:[[MSIMEAppearancePreferences alloc] initWithDefaults:outputDefaults] forKey:@"appearance"];
            owner.copyFails = NO;
            MSIMEFocusedController = owner;
            [session setEnglishMode:NO error:nil];
            [owner toggleVoiceInput:nil];
            source.transcript(@"汉语", YES);
            assert([origin.commits.lastObject isEqual:@"漢語"]);
            owner.copiedText = nil;
            [owner toggleVoiceInput:nil];
            [owner deactivateServer:origin];
            MSIMEFocusedController = nil;
            source.transcript(@"汉语", YES);
            assert([owner.copiedText isEqual:@"漢語"]);
            [outputDefaults removePersistentDomainForName:outputSuite];
            puts("PASS persistent: traditional insertion and clipboard");
            [owner setValue:origin forKey:@"activeClient"];
            [owner toggleVoiceInput:nil];
            [owner deactivateServer:origin];
            [source cancelWithError:nil];
            [owner reportVoiceFailure:MSIMEVoiceFailureCapture];
            assert(overlay.failure == MSIMEVoiceFailureCapture && !MSIMEVoiceRecordingOwner);
            [owner setValue:origin forKey:@"activeClient"];
            [owner toggleVoiceInput:nil];
            session.rejectVoiceResult = YES;
            source.transcript(@"synthetic unavailable final", YES);
            assert(overlay.failure == MSIMEVoiceFailureSession && !source.active);
            session.rejectVoiceResult = NO;
            [NSApplication sharedApplication];
            [owner setValue:nil forKey:@"voiceOverlay"];
            voiceArguments[@"MSIMEClientVoiceASRProvider"] = @"openai";
            voiceArguments[@"MSIMEClientVoiceASRToken"] = @"";
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            [owner toggleVoiceInput:nil];
            MSIMEVoiceWaveOverlay *startupFailure = [owner valueForKey:@"voiceOverlay"];
            assert(startupFailure.collectionBehavior & NSWindowCollectionBehaviorCanJoinAllSpaces);
            assert(startupFailure.collectionBehavior & NSWindowCollectionBehaviorFullScreenAuxiliary);
            [startupFailure setListening:NO];
            MSIMEFocusedController = nil;
            method_setImplementation(method, original);
            method_setImplementation(deactivate, originalDeactivate);
            assert(PersistentBaseDeactivations == 13);
            [defaults setVolatileDomain:old forName:NSArgumentDomain];
            assert([session closeWithError:nil]);
            assert([NSFileManager.defaultManager removeItemAtPath:root error:nil]);
            return 0;
        }
        if (argc == 2) {
            NSString *socketPath = MSIMESyntheticVoiceSocket();
            int socketDescriptor = MSIMEBindSyntheticVoiceSocket(socketPath);
            controller.usePolishFixture = YES;
            session.providerDone = dispatch_semaphore_create(0);
            [controller toggleVoiceInput:nil];
            NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:3];
            while (!session.providerStarted && deadline.timeIntervalSinceNow > 0)
                [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
            assert(session.providerStarted && !capture.transcript);
            assert(!controller.polishFixture); // External provider owns its polish stage.
            session.providerUpdate(@"socket partial", NO);
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
            assert(cues.starts == 1 && cues.stops == 0);
            assert([client.marked isEqual:@"socket partial"] && !client.commits.count);
            presentation.actionHandler(NO);
            presentation.actionHandler(NO);
            assert(presentation.dismissed && capture.active);
            session.providerPhase(1); session.providerPhase(2); session.providerPhase(0);
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
            assert(presentation.phase == 3);
            session.providerUpdate(@"socket final", YES);
            while ((capture.active || !session.providerStops || !session.providerCancels) && deadline.timeIntervalSinceNow > 0)
                [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
            assert(!capture.active && session.providerStops == 1 && session.providerCancels == 1);
            session.providerPhase(2);
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
            assert(presentation.phase == 0); // Late progress cannot reopen a completed session.
            assert(client.commits.count == 1 && [client.commits[0] isEqual:@"socket final"]);
            assert(cues.starts == 1 && cues.stops == 1);
            session.providerDone = dispatch_semaphore_create(0);
            session.providerStarted = NO;
            voiceArguments[@"MSIMEClientVoiceStreamInlinePreedit"] = @NO;
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            [controller toggleVoiceInput:nil];
            deadline = [NSDate dateWithTimeIntervalSinceNow:3];
            while (!session.providerStarted && deadline.timeIntervalSinceNow > 0)
                [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
            assert(session.providerStarted && capture.active);
            session.providerUpdate(@"synthetic socket preview", NO);
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
            assert([presentation.preview isEqual:@"synthetic socket preview"] && !client.marked.length);
            dispatch_semaphore_signal(session.providerDone); // Provider exits without a final result.
            while (capture.active && deadline.timeIntervalSinceNow > 0)
                [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
            assert(!capture.active && presentation.failure == MSIMEVoiceFailureProvider);
            assert(!presentation.preview.length);
            [defaults setVolatileDomain:old forName:NSArgumentDomain];
            close(socketDescriptor);
            [NSFileManager.defaultManager removeItemAtPath:socketPath error:nil];
            assert([session closeWithError:nil]); assert([NSFileManager.defaultManager removeItemAtPath:root error:nil]);
            return 0;
        }
        // 别的平台写下的录音后端不再让录音被拒绝：连同它的设备一起当作系统默认设备，交给采集的设备为 nil。
        voiceArguments[@"MSIMEClientVoiceCaptureBackend"] = @"windows";
        voiceArguments[@"MSIMEClientVoiceCaptureDevice"] = @"wasapi:fixture";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller toggleVoiceInput:nil]; assert(capture.active && !capture.captureDevice);
        assert(presentation.failure != MSIMEVoiceFailureCapture);
        voiceArguments[@"MSIMEClientVoiceCaptureBackend"] = @"macos";
        [voiceArguments removeObjectForKey:@"MSIMEClientVoiceCaptureDevice"];
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        assert(cues.starts == 1 && cues.stops == 0);
        MSIMEVoiceAudioBuffer oldMeter = capture.bufferHandler;
        oldMeter(MSIMEVoiceMeterFixtureBuffer()); MSIMEVoiceMeterFixturePump();
        assert(presentation.lastLevel > 0.5f && presentation.lastLevel < 0.7f && presentation.levelUpdates == 1);
        void (^first)(NSString *, BOOL) = capture.transcript;
        first(@"synthetic partial", NO);
        assert(!presentation.preview.length); // Inline mode does not duplicate client text.
        assert(client.commits.count == 0 && [client.marked isEqual:@"synthetic partial"]);
        NSUInteger cancelled = capture.transcriptionStops;
        void (^oldAction)(BOOL) = presentation.actionHandler;
        presentation.actionHandler(NO);
        assert(capture.active && capture.captureStops && capture.transcriptionStops == cancelled);
        assert(presentation.phase == 2);
        oldMeter(MSIMEVoiceMeterFixtureBuffer()); MSIMEVoiceMeterFixturePump();
        assert(presentation.levelUpdates == 1);
        presentation.actionHandler(NO);
        assert(presentation.dismissed && capture.active);
        first(@"synthetic final", YES);
        assert(!capture.active && client.commits.count == 1 && [client.commits[0] isEqual:@"synthetic final"]);
        assert(presentation.phase == 0);
        first(@"duplicate", YES); assert(client.commits.count == 1);
        assert(cues.starts == 1 && cues.stops == 1);
        [controller toggleVoiceInput:nil];
        oldAction(YES); oldAction(NO);
        oldMeter(MSIMEVoiceMeterFixtureBuffer()); MSIMEVoiceMeterFixturePump();
        assert(presentation.levelUpdates == 1);
        assert(capture.active && ![[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        first(@"old callback", YES); assert(capture.active && client.commits.count == 1);
        capture.transcript(@"cancelled partial", NO);
        void (^cancelledTranscript)(NSString *, BOOL) = capture.transcript;
        presentation.actionHandler(YES); assert(!capture.active && !client.marked.length);
        cancelledTranscript(@"synthetic cancelled final", YES); assert(client.commits.count == 1);
        [controller toggleVoiceInput:nil];
        [controller setValue:[LiveTextFixture new] forKey:@"activeClient"];
        capture.transcript(@"wrong focus", YES); assert(client.commits.count == 1);
        [controller setValue:client forKey:@"activeClient"];
        [controller toggleVoiceInput:nil]; capture.transcript(@"", YES); assert(!capture.active);
        [controller toggleVoiceInput:nil]; [controller toggleVoiceInput:nil]; [controller toggleVoiceInput:nil]; assert(!capture.active);
        uint64_t generation = 0;
        assert([capture startWithSession:session generation:&generation error:nil]);
        [controller setValue:@(generation) forKey:@"voiceGeneration"];
        id token = [controller beginLiveVoiceWithOptions:@{@"stream": @NO} socket:MSIMESyntheticVoiceSocket()];
        [controller applyLiveVoiceText:@"hidden partial" final:NO token:token]; assert(!client.marked.length);
        assert([presentation.preview isEqual:@"hidden partial"]);
        [controller applyLiveVoiceText:[@"x" stringByPaddingToLength:65537 withString:@"x" startingAtIndex:0] final:NO token:token];
        assert([presentation.preview isEqual:@"hidden partial"]);
        [controller finishLiveVoiceInput]; assert(capture.active);
        assert([presentation.preview isEqual:@"hidden partial"]);
        [controller applyLiveVoiceText:@"socket final" final:YES token:token];
        assert(client.commits.count == 2 && [client.commits.lastObject isEqual:@"socket final"]);
        assert(!presentation.preview.length);
        [controller applyLiveVoiceText:@"stale preview" final:NO token:token];
        assert(!presentation.preview.length);
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:3];
        while ((!session.providerStops || !session.providerCancels) && deadline.timeIntervalSinceNow > 0)
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        assert(session.providerStops == 1 && session.providerCancels == 1);
        auto key = [](unsigned short code, NSEventModifierFlags flags, NSEventType type) {
            return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:flags timestamp:1 windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:code];
        };
        const auto option = NSEventModifierFlagOption | NX_DEVICERALTKEYMASK;
        assert([controller handleEvent:key(61, option, NSEventTypeFlagsChanged) client:client]);
        assert(capture.active);
        assert(!presentation.locked);
        assert([controller handleEvent:key(49, option, NSEventTypeKeyDown) client:client]);
        assert(presentation.locked); // Locking the hold shows the overlay's actions.
        assert([controller handleEvent:key(61, 0, NSEventTypeFlagsChanged) client:client]);
        assert([controller handleEvent:key(49, 0, NSEventTypeKeyUp) client:client]);
        assert(capture.active && ![[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        assert([controller handleEvent:key(61, option, NSEventTypeFlagsChanged) client:client]);
        assert([controller handleEvent:key(61, 0, NSEventTypeFlagsChanged) client:client]);
        assert(capture.active && [[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        capture.transcript(@"locked final", YES);
        assert(!capture.active && client.commits.count == 3 && [client.commits.lastObject isEqual:@"locked final"]);
        assert([controller handleEvent:key(61, option, NSEventTypeFlagsChanged) client:client]);
        [controller cancelLiveVoiceInput];
        [controller toggleVoiceInput:nil]; // A separately started recording owns a new generation.
        assert([controller handleEvent:key(61, 0, NSEventTypeFlagsChanged) client:client]);
        assert(capture.active && ![[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        [controller cancelLiveVoiceInput];
        capture.needsMicrophonePermission = YES;
        assert([controller handleEvent:key(61, option, NSEventTypeFlagsChanged) client:client]);
        assert(capture.permission && !capture.active);
        assert([controller handleEvent:key(61, 0, NSEventTypeFlagsChanged) client:client]);
        capture.needsMicrophonePermission = NO;
        capture.permission(YES);
        assert(!capture.active);
        // Disabling stops capture but finishes the in-flight recognition, as on
        // Windows. Use volatile preferences so no real settings are written.
        [controller toggleVoiceInput:nil]; assert(capture.active);
        void (^disabledResult)(NSString *, BOOL) = capture.transcript;
        disabledResult(@"synthetic disabled partial", NO);
        voiceArguments[@"MSIMEClientVoiceEnabled"] = @NO;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [NSApplication sharedApplication];
        [controller applySharedToolbarPreferences:@{}];
        assert(capture.active && [[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        const auto disabledCaptureStops = capture.captureStops;
        [controller applySharedToolbarPreferences:@{}];
        [controller toggleVoiceInput:nil];
        assert(capture.active && capture.captureStops == disabledCaptureStops);
        disabledResult(@"synthetic disabled final", YES);
        disabledResult(@"synthetic duplicate disabled final", YES);
        assert(!capture.active && !client.marked.length && client.commits.count == 4);
        [controller toggleVoiceInput:nil]; assert(!capture.active);
        assert(![controller handleEvent:key(61, option, NSEventTypeFlagsChanged) client:client]);
        assert(![controller handleEvent:key(61, 0, NSEventTypeFlagsChanged) client:client]);
        assert(![controller handleEvent:key(101, NSEventModifierFlagControl, NSEventTypeKeyDown) client:client]);
        assert(!capture.active);
        voiceArguments[@"MSIMEClientVoiceEnabled"] = @YES;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        // A previously requested permission must not override a later disable.
        capture.needsMicrophonePermission = YES;
        capture.permission = nil;
        [controller toggleVoiceInput:nil]; assert(capture.permission);
        voiceArguments[@"MSIMEClientVoiceEnabled"] = @NO;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        capture.needsMicrophonePermission = NO;
        capture.permission(YES); assert(!capture.active);
        voiceArguments[@"MSIMEClientVoiceEnabled"] = @YES;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller setValue:nil forKey:@"activeClient"];
        [controller toggleVoiceInput:nil]; assert(!capture.active);
        [controller setValue:client forKey:@"activeClient"];
        assert([controller handleEvent:key(61, option, NSEventTypeFlagsChanged) client:client]);
        assert(capture.active);
        [controller cancelLiveVoiceInput];
        assert([controller handleEvent:key(61, 0, NSEventTypeFlagsChanged) client:client]);
        voiceArguments[@"MSIMEClientVoiceHotkeyRightAlt"] = @NO;
        voiceArguments[@"MSIMEClientVoiceHotkeyCtrlOption"] = @YES;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        const auto rightControl = NSEventModifierFlagControl | NX_DEVICERCTLKEYMASK;
        assert(![controller handleEvent:key(62, rightControl, NSEventTypeFlagsChanged) client:client]);
        assert([controller handleEvent:key(61, rightControl | option, NSEventTypeFlagsChanged) client:client]);
        assert(capture.active);
        [controller cancelLiveVoiceInput];
        const auto failureStarts = cues.starts, failureStops = cues.stops;
        capture.failCapture = YES;
        [controller toggleVoiceInput:nil];
        assert(presentation.failure == MSIMEVoiceFailureCapture);
        assert(!capture.active && cues.starts == failureStarts && cues.stops == failureStops);
        capture.failCapture = NO;
        // Exercise all shared sound switches at the actual capture entry/exit.
        for (NSNumber *master in @[@NO, @YES]) for (NSNumber *start in @[@NO, @YES]) for (NSNumber *end in @[@NO, @YES]) {
            voiceArguments[@"MSIMEClientVoiceSoundEnabled"] = master;
            voiceArguments[@"MSIMEClientVoiceStartSound"] = start;
            voiceArguments[@"MSIMEClientVoiceEndSound"] = end;
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            const auto starts = cues.starts, stops = cues.stops;
            [controller toggleVoiceInput:nil]; assert(capture.active);
            [controller cancelLiveVoiceInput]; [controller cancelLiveVoiceInput];
            // macOS 只读总开关：开始、结束提示音各自存下的值不再起作用。
            assert(cues.starts == starts + master.boolValue);
            assert(cues.stops == stops + master.boolValue);
        }
        MSIMEInputController *builder = [MSIMEInputController alloc];
        assert((![builder makeLiveVoicePolishRequest:@{@"polish_enabled": @NO, @"polish_token": @"fixture-only"}]));
        assert(![builder makeLiveVoicePolishRequest:@{@"polish_enabled": @YES}]);
        MSIMEHTTPVoiceRequest *configured = [builder makeLiveVoicePolishRequest:@{@"polish_enabled": @YES, @"polish_token": @"fixture-only"}];
        assert(configured); [configured cancel];
        configured = [builder makeLiveVoicePolishRequest:@{@"polish_enabled": @NO, @"polish_text": @YES, @"polish_token": @"fixture-only"}];
        assert(configured); [configured cancel];
        // 用户关闭整理后，旧 polish_text=true 也不应创建真实整理请求。
        controller.usePolishFixture = NO;
        voiceArguments[@"MSIMEClientVoicePolish"] = @NO;
        voiceArguments[@"MSIMEClientVoicePolishText"] = @YES;
        voiceArguments[@"MSIMEClientVoicePolishToken"] = @"fixture-only";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller toggleVoiceInput:nil];
        assert(capture.active && ![controller valueForKey:@"livePolishRequest"]);
        const NSUInteger beforeDisabledPolish = client.commits.count;
        capture.transcript(@"synthetic polish disabled", YES);
        assert(!capture.active && client.commits.count == beforeDisabledPolish + 1);
        assert([client.commits.lastObject isEqual:@"synthetic polish disabled"]);
        controller.usePolishFixture = YES;
        voiceArguments[@"MSIMEClientVoicePolish"] = @YES;
        voiceArguments[@"MSIMEClientVoicePolishText"] = @YES;
        voiceArguments[@"MSIMEClientVoicePolishToken"] = @"fixture-only";
        voiceArguments[@"MSIMEClientVoicePolishPromptCustom1"] = @"synthetic original prompt";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller toggleVoiceInput:nil];
        LivePolishFixture *polish = controller.polishFixture;
        const auto beforePolish = client.commits.count;
        const auto beforePolishStops = capture.captureStops;
        capture.transcript(@"synthetic partial before polish", NO);
        assert(!polish.submissions);
        voiceArguments[@"MSIMEClientVoicePolishPromptCustom1"] = @"synthetic later prompt";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        capture.transcript(@"synthetic original", YES);
        capture.transcript(@"duplicate final", YES);
        capture.transcript(@"late partial", NO);
        assert(polish.submissions == 1 && [polish.input isEqual:@"synthetic original"]);
        assert(presentation.phase == 3);
        assert([controller.polishOptions[@"polish_prompt_custom_1"] isEqual:@"synthetic original prompt"]);
        assert([controller.polishOptions[@"polish_text"] isEqual:@YES] && [controller.polishOptions[@"polish_enabled"] isEqual:@YES]);
        assert(capture.captureStops > beforePolishStops && capture.active && client.commits.count == beforePolish);
        assert([client.marked isEqual:@"synthetic partial before polish"]);
        polish.completion(@"synthetic polished", nil);
        polish.completion(@"duplicate polished", nil);
        assert(!capture.active && client.commits.count == beforePolish + 1 && [client.commits.lastObject isEqual:@"synthetic polished"]);
        assert(polish.cancellations == 1);
        for (NSUInteger failure = 0; failure < 3; ++failure) {
            [controller toggleVoiceInput:nil]; polish = controller.polishFixture;
            polish.failStart = failure == 2;
            NSMutableString *original = [@"synthetic fallback" mutableCopy];
            capture.transcript(original, YES);
            [original setString:@"mutated after callback"];
            if (failure < 2) polish.completion(failure ? @"ignored" : @"", failure ? [NSError errorWithDomain:@"synthetic" code:1 userInfo:nil] : nil);
            assert(!capture.active && [client.commits.lastObject isEqual:@"synthetic fallback"]);
        }
        const auto beforeStale = client.commits.count;
        [controller toggleVoiceInput:nil]; polish = controller.polishFixture;
        capture.transcript(@"cancel during polish", YES);
        [controller toggleVoiceInput:nil]; // Second stop cancels pending polish.
        assert(!capture.active && polish.cancellations == 1);
        [controller toggleVoiceInput:nil];
        polish.completion(@"old polished result", nil);
        assert(capture.active && client.commits.count == beforeStale);
        [controller cancelLiveVoiceInput];
        for (NSString *field in @[@"activeClient", @"session", @"voiceGeneration"]) {
            [controller toggleVoiceInput:nil]; polish = controller.polishFixture;
            capture.transcript(@"synthetic stale focus", YES);
            id original = [controller valueForKey:field];
            [controller setValue:[field isEqual:@"voiceGeneration"] ? @999999 : [MSIMEVoiceClientFixture new] forKey:field];
            polish.completion(@"wrong owner", nil);
            [controller setValue:original forKey:field];
            assert(!capture.active && client.commits.count == beforeStale);
        }
        [controller toggleVoiceInput:nil]; polish = controller.polishFixture;
        capture.transcript(@"", YES);
        assert(!capture.active && !polish.submissions && client.commits.count == beforeStale);
        controller.usePolishFixture = NO;
        voiceArguments[@"MSIMEClientVoicePolishText"] = @NO;
        voiceArguments[@"MSIMEClientVoicePolishToken"] = @"";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        capture.needsSpeechPermission = YES; capture.needsMicrophonePermission = YES;
        [controller toggleVoiceInput:nil];
        assert(capture.speechRequests == 1 && !capture.active);
        void (^speechPermission)(BOOL) = capture.speechPermission;
        capture.needsSpeechPermission = NO;
        speechPermission(YES);
        id microphoneToken = [controller valueForKey:@"voicePermissionToken"];
        assert(microphoneToken && !capture.active);
        speechPermission(YES);
        assert([controller valueForKey:@"voicePermissionToken"] == microphoneToken);
        void (^microphonePermission)(BOOL) = capture.permission;
        capture.needsMicrophonePermission = NO;
        microphonePermission(YES); assert(capture.active);
        microphonePermission(YES); speechPermission(YES);
        assert(capture.active && ![[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        [controller cancelLiveVoiceInput];
        // A second user toggle cancels intent without issuing a second sheet.
        capture.needsMicrophonePermission = YES;
        [controller toggleVoiceInput:nil]; microphonePermission = capture.permission;
        const auto requests = capture.microphoneRequests;
        [controller toggleVoiceInput:nil];
        assert(capture.microphoneRequests == requests && ![controller valueForKey:@"voicePermissionToken"]);
        capture.needsMicrophonePermission = NO;
        microphonePermission(YES); assert(!capture.active);
        // Cancel, focus transitions and settings changes invalidate old intent.
        for (NSUInteger cancellation = 0; cancellation < 4; ++cancellation) {
            capture.needsMicrophonePermission = YES;
            [controller toggleVoiceInput:nil]; microphonePermission = capture.permission;
            if (cancellation == 0) assert([controller handleEvent:key(53, 0, NSEventTypeKeyDown) client:client]);
            else if (cancellation == 1) [controller voiceProviderSettingsChanged:nil];
            else if (cancellation == 2) {
                LiveTextFixture *other = [LiveTextFixture new];
                [controller handleEvent:key(62, NSEventModifierFlagControl, NSEventTypeFlagsChanged) client:other];
                [controller handleEvent:key(62, 0, NSEventTypeFlagsChanged) client:client];
            } else [controller handleEvent:key(62, 0, NSEventTypeFlagsChanged) client:nil];
            capture.needsMicrophonePermission = NO;
            microphonePermission(YES); assert(!capture.active);
        }
        for (NSString *field in @[@"activeClient", @"session", @"voiceGeneration", @"voiceService"]) {
            capture.needsMicrophonePermission = YES;
            [controller toggleVoiceInput:nil]; microphonePermission = capture.permission;
            id original = [controller valueForKey:field];
            [controller setValue:[field isEqual:@"voiceGeneration"] ? @999999 : [MSIMEVoiceClientFixture new] forKey:field];
            capture.needsMicrophonePermission = NO;
            microphonePermission(YES);
            [controller setValue:original forKey:field];
            assert(!capture.active);
        }
        capture.needsMicrophonePermission = YES;
        [controller toggleVoiceInput:nil]; microphonePermission = capture.permission;
        id pending = [controller valueForKey:@"voicePermissionToken"];
        [controller applySharedToolbarPreferences:@{@"voice_input": @{@"asr_provider": @"system"}}];
        assert([controller valueForKey:@"voicePermissionToken"] == pending); // Unchanged reload.
        microphonePermission(NO); assert(![controller valueForKey:@"voicePermissionToken"]);
        assert(presentation.failure == MSIMEVoiceFailureMicrophonePermission);
        [controller toggleVoiceInput:nil];
        void (^newPermission)(BOOL) = capture.permission;
        microphonePermission(YES); assert(!capture.active && [controller valueForKey:@"voicePermissionToken"]);
        capture.needsMicrophonePermission = NO;
        newPermission(YES); assert(capture.active);
        [controller cancelLiveVoiceInput];
        capture.needsMicrophonePermission = YES;
        [controller toggleVoiceInput:nil]; microphonePermission = capture.permission;
        id originalGeneration = [controller valueForKey:@"voiceGeneration"];
        voiceArguments[@"MSIMEClientVoiceASRProvider"] = @"openai";
        voiceArguments[@"MSIMEClientVoiceASRToken"] = @"";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        capture.needsMicrophonePermission = NO;
        microphonePermission(YES);
        assert(!capture.active && [[controller valueForKey:@"voiceGeneration"] isEqual:originalGeneration]);
        // Exercise the exact block registered with AppKit without installing a
        // real global monitor or requesting accessibility/microphone access.
        voiceArguments[@"MSIMEClientVoiceASRProvider"] = @"system";
        voiceArguments[@"MSIMEClientVoiceHotkeyCtrlF9"] = @YES;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        void (^globalHotkey)(NSEvent *) = [controller globalVoiceHotkeyHandler];
        NSEvent *ctrlF9 = key(101, NSEventModifierFlagControl, NSEventTypeKeyDown);
        assert(NSThread.isMainThread);
        globalHotkey(ctrlF9);
        assert(capture.active); // Must start before a later focus transition.
        globalHotkey(ctrlF9);
        assert([[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        capture.transcript(@"synthetic global final", YES);
        assert(!capture.active);
        // Invalid chords and autorepeat must not toggle recording.
        globalHotkey(key(100, NSEventModifierFlagControl, NSEventTypeKeyDown));
        globalHotkey(key(101, 0, NSEventTypeKeyDown));
        for (NSNumber *extra in @[@(NSEventModifierFlagShift), @(NSEventModifierFlagOption), @(NSEventModifierFlagCommand)])
            globalHotkey(key(101, NSEventModifierFlagControl | extra.unsignedLongLongValue, NSEventTypeKeyDown));
        globalHotkey([NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:NSEventModifierFlagControl timestamp:1 windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:YES keyCode:101]);
        assert(!capture.active);
        for (NSString *setting in @[@"MSIMEClientVoiceEnabled", @"MSIMEClientVoiceHotkeyCtrlF9"]) {
            voiceArguments[setting] = @NO;
            [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
            globalHotkey(ctrlF9); assert(!capture.active);
            voiceArguments[setting] = @YES;
        }
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller setValue:nil forKey:@"activeClient"];
        globalHotkey(ctrlF9); assert(!capture.active);
        [controller setValue:client forKey:@"activeClient"];
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
        assert(!capture.active); // No queued gesture leaks into restored focus.
        globalHotkey(ctrlF9); assert(capture.active);
        [controller cancelLiveVoiceInput];
        // 额外绑定独立于固定 Ctrl+F9；同一共享解码同时供全局监视器与 IMK 事件入口使用。
        MSIMEAppearancePreferences *voiceAppearance = [[MSIMEAppearancePreferences alloc] initWithDefaults:defaults];
        [voiceAppearance applySharedInputPreferences:@{@"keybindings": @{@"toggle_voice_input": @"Ctrl+Alt+F8"}}];
        [controller setValue:voiceAppearance forKey:@"appearance"];
        voiceArguments[@"MSIMEClientVoiceHotkeyCtrlF9"] = @NO;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        NSEventModifierFlags customFlags = NSEventModifierFlagControl | NSEventModifierFlagOption;
        NSEvent *customVoice = key(100, customFlags, NSEventTypeKeyDown);
        globalHotkey(ctrlF9); assert(!capture.active);
        globalHotkey(customVoice); assert(capture.active);
        globalHotkey([NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:customFlags timestamp:1 windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:YES keyCode:100]);
        assert(capture.active && ![[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        globalHotkey(customVoice); assert([[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        capture.transcript(@"synthetic custom final", YES); assert(!capture.active);
        [voiceAppearance applySharedInputPreferences:@{@"keybindings": @{@"toggle_voice_input": @"Ctrl+Alt+F7"}}];
        globalHotkey(customVoice); assert(!capture.active);
        NSEvent *reboundVoice = key(98, customFlags, NSEventTypeKeyDown);
        assert([controller handleEvent:reboundVoice client:client] && capture.active);
        [controller cancelLiveVoiceInput];
        voiceArguments[@"MSIMEClientVoiceEnabled"] = @NO;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        globalHotkey(reboundVoice); assert(!capture.active);
        voiceArguments[@"MSIMEClientVoiceEnabled"] = @YES;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller setValue:nil forKey:@"activeClient"];
        globalHotkey(reboundVoice); assert(!capture.active);
        [controller setValue:client forKey:@"activeClient"];
        [voiceAppearance applySharedInputPreferences:@{@"keybindings": @{@"toggle_voice_input": @""}}];
        globalHotkey(reboundVoice); assert(!capture.active);
        voiceArguments[@"MSIMEClientVoiceHotkeyCtrlF9"] = @YES;
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        LiveTextFixture *nextClient = [LiveTextFixture new];
        [controller setValue:nextClient forKey:@"activeClient"];
        [controller toggleVoiceInput:nil]; assert(capture.active);
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
        assert(capture.active && ![[controller valueForKey:@"liveVoiceProcessing"] boolValue]);
        [controller cancelLiveVoiceInput];
        [controller setValue:client forKey:@"activeClient"];
        capture.needsSpeechPermission = YES;
        [controller toggleVoiceInput:nil];
        capture.speechPermission(NO);
        assert(presentation.failure == MSIMEVoiceFailureSpeechPermission && !capture.active);
        [controller handleEvent:key(53, 0, NSEventTypeKeyDown) client:client];
        assert(presentation.failure == 0);
        capture.needsSpeechPermission = NO;
        [controller toggleVoiceInput:nil];
        id expired = [controller valueForKey:@"liveVoiceToken"];
        [controller expireLiveVoice:expired];
        assert(presentation.failure == MSIMEVoiceFailureTimeout && !capture.active);
        [controller toggleVoiceInput:nil];
        const NSUInteger beforeExpired = presentation.failures;
        [controller expireLiveVoice:expired];
        assert(capture.active && presentation.failures == beforeExpired);
        capture.transcript(@"", YES);
        assert(presentation.failure == MSIMEVoiceFailureNoSpeech && !capture.active);
        [controller voiceProviderSettingsChanged:nil];
        assert(presentation.failure == 0);
        capture.needsMicrophonePermission = YES;
        [controller toggleVoiceInput:nil];
        const NSUInteger beforeDenied = presentation.failures;
        [controller setValue:nextClient forKey:@"activeClient"];
        capture.permission(NO);
        assert(presentation.failures == beforeDenied);
        [controller setValue:client forKey:@"activeClient"];
        capture.needsMicrophonePermission = NO;
        controller.usePolishFixture = YES;
        voiceArguments[@"MSIMEClientVoiceStreamInlinePreedit"] = @NO;
        voiceArguments[@"MSIMEClientVoicePolishText"] = @YES;
        voiceArguments[@"MSIMEClientVoicePolishToken"] = @"fixture-only";
        [defaults setVolatileDomain:voiceArguments forName:NSArgumentDomain];
        [controller toggleVoiceInput:nil];
        capture.transcript(@"synthetic overlay partial", NO);
        assert([presentation.preview isEqual:@"synthetic overlay partial"] && !client.marked.length);
        capture.transcript(@"synthetic overlay final", YES);
        assert([presentation.preview isEqual:@"synthetic overlay final"] && presentation.phase == 3);
        controller.polishFixture.completion(@"synthetic overlay polished", nil);
        assert(!presentation.preview.length && !capture.active);
        const NSUInteger beforeShort = client.commits.count, beforeShortFailures = presentation.failures;
        for (NSNumber *duration in @[@0, @0.2499375]) {
            capture.capturedSeconds = duration.doubleValue;
            [controller toggleVoiceInput:nil];
            void (^shortReply)(NSString *, BOOL) = capture.transcript;
            shortReply(@"synthetic short partial", NO);
            presentation.actionHandler(NO);
            shortReply(@"synthetic late final", YES);
            assert(!capture.active && !client.marked.length && !presentation.preview.length);
            assert(client.commits.count == beforeShort && presentation.failures == beforeShortFailures);
            [controller toggleVoiceInput:nil];
            capture.transcript(@"synthetic early final", YES);
            assert(!capture.active && !controller.polishFixture.submissions && client.commits.count == beforeShort);
        }
        capture.capturedSeconds = 0.25;
        [defaults setVolatileDomain:old forName:NSArgumentDomain];
        controller.usePolishFixture = NO;
        for (NSString *mode in @[@"sendinput", @"ctrl_v"]) {
            uint64_t routeGeneration = 0;
            assert([capture startWithSession:session generation:&routeGeneration error:nil]);
            [controller setValue:@(routeGeneration) forKey:@"voiceGeneration"];
            const NSUInteger imk = client.commits.count, external = controller.externalCommits;
            id routeToken = [controller beginLiveVoiceWithOptions:@{@"stream": @YES, @"commit_mode": mode} socket:nil];
            assert(routeToken && ![[controller valueForKey:@"liveVoiceInline"] boolValue]);
            [controller applyLiveVoiceText:@"synthetic routed" final:YES token:routeToken];
            [controller applyLiveVoiceText:@"synthetic routed" final:YES token:routeToken];
            assert(client.commits.count == imk && controller.externalCommits == external + 1);
        }
        {
            [defaults setVolatileDomain:@{
                @"MSIMEClientVoiceEnabled": @YES, @"MSIMEClientVoiceHotkeyRightAlt": @YES,
                @"MSIMEClientVoiceASRProvider": @"system", @"MSIMEClientVoicePolish": @NO,
                @"MSIMEClientVoiceSoundEnabled": @NO, @"MSIMEClientVoiceMuteSystemAudio": @NO
            } forName:NSArgumentDomain];
            LiveControllerFixture *holdController = [LiveControllerFixture alloc];
            LiveCaptureFixture *holdCapture = [LiveCaptureFixture new];
            holdCapture.capturedSeconds = 0.25;
            [holdController setValue:session forKey:@"session"];
            [holdController setValue:client forKey:@"activeClient"];
            [holdController setValue:holdCapture forKey:@"voiceService"];
            [holdController setValue:[LivePresentationFixture new] forKey:@"voiceOverlay"];
            [holdController setValue:[LivePresentationFixture new] forKey:@"voiceAudioMuter"];
            [holdController setValue:[MSIMEVoiceCueFixture new] forKey:@"voiceCuePlayer"];
            const auto systemFlags = MSIMEPhysicalModifierFlagsState;
            MSIMEPhysicalModifierFlagsState = +[](CGEventSourceStateID) -> CGEventFlags {
                return NSEventModifierFlagOption | NX_DEVICERALTKEYMASK;
            };
            // 经过宿主的真实查询边界和录音状态机，采集器仍使用合成夹具。
            assert([holdController handleEvent:key(61, NSEventModifierFlagOption, NSEventTypeFlagsChanged) client:client]);
            assert(holdCapture.active && holdCapture.captureStops == 0);
            assert([holdController handleEvent:key(61, NSEventModifierFlagOption, NSEventTypeFlagsChanged) client:client]);
            assert(holdCapture.active && holdCapture.captureStops == 0);
            MSIMEPhysicalModifierFlagsState = +[](CGEventSourceStateID) -> CGEventFlags {
                return NSEventModifierFlagOption | NX_DEVICELALTKEYMASK;
            };
            assert([holdController handleEvent:key(61, NSEventModifierFlagOption, NSEventTypeFlagsChanged) client:client]);
            assert(holdCapture.captureStops == 1);
            [holdController cancelLiveVoiceInput];
            MSIMEPhysicalModifierFlagsState = systemFlags;
            [defaults setVolatileDomain:old forName:NSArgumentDomain];
        }
        {
            // 自定义快捷键走真实控制器，采集、转写、整理均为合成替身。
            NSMutableDictionary *configured = [@{
                @"MSIMEClientVoiceEnabled":@YES, @"MSIMEClientVoiceASRProvider":@"system",
                @"MSIMEClientVoiceHotkeyMode":@"hold", @"MSIMEClientVoiceMacOSHotkey":@{@"key_code":@54, @"modifiers":@0},
                @"MSIMEClientVoicePolish":@YES, @"MSIMEClientVoicePolishText":@YES, @"MSIMEClientVoicePolishToken":@"synthetic-only",
                @"MSIMEClientVoiceSoundEnabled":@NO, @"MSIMEClientVoiceMuteSystemAudio":@NO,
                @"MSIMEClientVoiceStreamInlinePreedit":@YES
            } mutableCopy];
            [defaults setVolatileDomain:configured forName:NSArgumentDomain];
            LiveControllerFixture *customController = [LiveControllerFixture alloc];
            customController.usePolishFixture = YES;
            LiveCaptureFixture *customCapture = [LiveCaptureFixture new]; customCapture.capturedSeconds = 0.5;
            LiveTextFixture *customClient = [LiveTextFixture new];
            [customController setValue:session forKey:@"session"];
            [customController setValue:customClient forKey:@"activeClient"];
            [customController setValue:customCapture forKey:@"voiceService"];
            [customController setValue:[LivePresentationFixture new] forKey:@"voiceOverlay"];
            [customController setValue:[LivePresentationFixture new] forKey:@"voiceAudioMuter"];
            [customController setValue:[MSIMEVoiceCueFixture new] forKey:@"voiceCuePlayer"];
            const auto command = NSEventModifierFlagCommand | NX_DEVICERCMDKEYMASK;
            auto down = key(54, command, NSEventTypeFlagsChanged), up = key(54, 0, NSEventTypeFlagsChanged);
            assert([customController handleConfiguredVoiceShortcut:down] && customCapture.active);
            auto late = customCapture.transcript;
            late(@"synthetic cancelled partial", NO);
            assert(customClient.marked.length);
            assert(![customController handleConfiguredVoiceShortcut:key(8, command, NSEventTypeKeyDown)]);
            assert(!customCapture.active && !customClient.marked.length && !customClient.commits.count);
            assert(customController.polishFixture.submissions == 0);
            late(@"synthetic late final", YES);
            assert(!customClient.commits.count && customController.polishFixture.submissions == 0);
            [customController handleConfiguredVoiceShortcut:up];
            configured[@"MSIMEClientVoiceHotkeyMode"] = @"toggle";
            [defaults setVolatileDomain:configured forName:NSArgumentDomain];
            assert([customController handleConfiguredVoiceShortcut:down] && !customCapture.active);
            assert([customController handleConfiguredVoiceShortcut:up] && customCapture.active);
            assert([customController handleConfiguredVoiceShortcut:key(53, 0, NSEventTypeKeyDown)]);
            assert(!customCapture.active && !customClient.commits.count && customController.polishFixture.submissions == 0);
            [customController handleConfiguredVoiceShortcut:key(53, 0, NSEventTypeKeyUp)];
            [customController handleConfiguredVoiceShortcut:down];
            [customController handleConfiguredVoiceShortcut:up];
            assert(customCapture.active);
            [customController handleConfiguredVoiceShortcut:down];
            [customController handleConfiguredVoiceShortcut:up];
            assert([[customController valueForKey:@"liveVoiceProcessing"] boolValue]);
            customCapture.transcript(@"synthetic toggle final", YES);
            assert(customController.polishFixture.submissions == 1);
            customController.polishFixture.completion(@"synthetic toggle polished", nil);
            assert(customClient.commits.count == 1 && !customCapture.active);
            // 经完整控制器入口复验 Option+左方向键，在 marked text 存在时再次按下应结束。
            configured[@"MSIMEClientVoiceMacOSHotkey"] = @{@"key_code":@123,@"modifiers":@2};
            [defaults setVolatileDomain:configured forName:NSArgumentDomain];
            const auto arrowFlags = NSEventModifierFlagOption | NSEventModifierFlagFunction | NSEventModifierFlagNumericPad;
            auto arrowDown = key(123, arrowFlags, NSEventTypeKeyDown);
            auto arrowUp = key(123, arrowFlags, NSEventTypeKeyUp);
            assert([customController handleEvent:arrowDown client:customClient] && customCapture.active);
            [customController handleEvent:arrowUp client:customClient];
            customCapture.transcript(@"synthetic arrow partial", NO);
            assert(customClient.marked.length);
            assert([customController handleEvent:arrowDown client:customClient]);
            assert([[customController valueForKey:@"liveVoiceProcessing"] boolValue]);
            [customController cancelLiveVoiceInput];
            [customController voiceProviderSettingsChanged:nil];
            auto missingReleaseKey = [&](NSTimeInterval timestamp) {
                return [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:arrowFlags
                    timestamp:timestamp windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@""
                    isARepeat:NO keyCode:123];
            };
            assert([customController handleEvent:missingReleaseKey(10) client:customClient] && customCapture.active);
            customCapture.transcript(@"synthetic missing-release partial", NO);
            assert([customController handleEvent:missingReleaseKey(11) client:customClient]);
            assert([[customController valueForKey:@"liveVoiceProcessing"] boolValue]);
            [customController cancelLiveVoiceInput];
            [defaults setVolatileDomain:old forName:NSArgumentDomain];
        }
        CGEventRef injected = CGEventCreateKeyboardEvent(nullptr, 0, true);
        CGEventSetIntegerValueField(injected, kCGEventSourceUserData, MSIMEVoiceCommitEventTag);
        assert(![controller handleEvent:[NSEvent eventWithCGEvent:injected] client:client]);
        CFRelease(injected);
        assert([session closeWithError:nil]); assert([NSFileManager.defaultManager removeItemAtPath:root error:nil]);
    }
}
