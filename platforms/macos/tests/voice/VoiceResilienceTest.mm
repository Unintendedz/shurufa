#import "../../src/voice/VoiceCaptureDevice.h"
#import "../../src/voice/VoicePCMBuffer.h"
#import "../../src/voice/VoiceInputService.h"
#import "../../src/voice/VoiceTargetEditability.h"
#import "../../src/voice/VoiceTextCommit.h"
#import "../../src/voice/VoiceRecoveryToast.h"
#include <cassert>
#include <cmath>
#include <cstring>

static void PumpMainRunLoop(NSTimeInterval seconds) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while ([deadline timeIntervalSinceNow] > 0) {
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.005]];
    }
}

static AVAudioPCMBuffer *MakeBuffer(double rate, AVAudioChannelCount channels, AVAudioFrameCount frames, float value) {
    AVAudioFormat *format = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:rate channels:channels];
    AVAudioPCMBuffer *buffer = [[AVAudioPCMBuffer alloc] initWithPCMFormat:format frameCapacity:frames];
    buffer.frameLength = frames;
    for (AVAudioChannelCount ch = 0; ch < channels; ++ch) {
        for (AVAudioFrameCount i = 0; i < frames; ++i) {
            buffer.floatChannelData[ch][i] = value;
        }
    }
    return buffer;
}

static AVAudioPCMBuffer *MakeSineBuffer(double rate, AVAudioChannelCount channels, AVAudioFrameCount frames, float amp) {
    AVAudioFormat *format = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:rate channels:channels];
    AVAudioPCMBuffer *buffer = [[AVAudioPCMBuffer alloc] initWithPCMFormat:format frameCapacity:frames];
    buffer.frameLength = frames;
    for (AVAudioFrameCount i = 0; i < frames; ++i) {
        const float sample = amp * std::sinf(2.0f * 3.14159265f * 440.0f * static_cast<float>(i) / static_cast<float>(rate));
        for (AVAudioChannelCount ch = 0; ch < channels; ++ch) {
            buffer.floatChannelData[ch][i] = sample;
        }
    }
    return buffer;
}

namespace FallbackDeviceFixture {
BOOL includeUSB = YES;
BOOL includeAny = YES;

OSStatus Size(AudioObjectID object, const AudioObjectPropertyAddress *address,
              UInt32 qualifierSize, const void *qualifier, UInt32 *bytes) {
    (void)qualifierSize; (void)qualifier;
    if (object == kAudioObjectSystemObject && address->mSelector == kAudioHardwarePropertyDevices) {
        *bytes = !includeAny ? 0 : (includeUSB ? 2 : 1) * sizeof(AudioDeviceID);
        return noErr;
    }
    if (address->mSelector == kAudioDevicePropertyStreams && address->mScope == kAudioDevicePropertyScopeInput) {
        *bytes = sizeof(AudioStreamID);
        return noErr;
    }
    return -1;
}

OSStatus Get(AudioObjectID object, const AudioObjectPropertyAddress *address,
             UInt32 qualifierSize, const void *qualifier, UInt32 *bytes, void *data) {
    (void)qualifierSize; (void)qualifier;
    if (object == kAudioObjectSystemObject && address->mSelector == kAudioHardwarePropertyDevices) {
        AudioDeviceID ids[2] = {101, 102};
        const UInt32 count = !includeAny ? 0 : (includeUSB ? 2 : 1);
        std::memcpy(data, ids, count * sizeof(AudioDeviceID));
        *bytes = count * sizeof(AudioDeviceID);
        return noErr;
    }
    if (object == kAudioObjectSystemObject && address->mSelector == kAudioHardwarePropertyDefaultInputDevice) {
        *static_cast<AudioDeviceID *>(data) = includeAny ? 101 : kAudioObjectUnknown;
        return noErr;
    }
    if (address->mSelector == kAudioDevicePropertyDeviceUID) {
        *static_cast<CFStringRef *>(data) = (object == 101) ? CFSTR("uid-builtin") : CFSTR("uid-usb");
        return noErr;
    }
    if (address->mSelector == kAudioObjectPropertyName) {
        *static_cast<CFStringRef *>(data) = (object == 101) ? CFSTR("Built-in Mic") : CFSTR("USB Mic");
        return noErr;
    }
    return -1;
}
} // namespace FallbackDeviceFixture

@interface FakeAudioInputNode : NSObject
@property(nonatomic) double sampleRate;
@property(nonatomic) AVAudioChannelCount channels;
@property(nonatomic, copy) AVAudioNodeTapBlock tap;
@property(nonatomic) NSUInteger removeTapCount;
@end

@implementation FakeAudioInputNode
- (AudioUnit)audioUnit { return nullptr; }
- (AVAudioFormat *)inputFormatForBus:(AVAudioNodeBus)bus {
    (void)bus;
    return [[AVAudioFormat alloc] initStandardFormatWithSampleRate:self.sampleRate channels:self.channels];
}
- (void)installTapOnBus:(AVAudioNodeBus)bus bufferSize:(AVAudioFrameCount)size format:(AVAudioFormat *)format block:(AVAudioNodeTapBlock)tap {
    (void)bus; (void)size; (void)format;
    self.tap = tap;
}
- (void)removeTapOnBus:(AVAudioNodeBus)bus {
    (void)bus;
    ++self.removeTapCount;
    self.tap = nil;
}
@end

@interface FakeAudioEngine : NSObject
@property(nonatomic, strong) FakeAudioInputNode *inputNode;
@property(nonatomic) BOOL failStart;
@property(nonatomic) BOOL running;
@end

@implementation FakeAudioEngine
- (BOOL)isRunning { return self.running; }
- (BOOL)startAndReturnError:(NSError **)error {
    if (self.failStart) {
        if (error) *error = [NSError errorWithDomain:@"test" code:1 userInfo:@{NSLocalizedDescriptionKey: @"mock device unavailable"}];
        return NO;
    }
    self.running = YES;
    return YES;
}
- (void)stop { self.running = NO; }
@end

@interface FakeArchive : NSObject <MSIMEVoiceArchiving>
@property(nonatomic) NSUInteger beginCount;
@property(nonatomic) NSUInteger endCount;
@property(nonatomic) BOOL lastDiscarded;
@property(nonatomic, strong) NSMutableData *archivedPCM;
@end

@implementation FakeArchive
- (instancetype)init {
    if ((self = [super init])) _archivedPCM = [NSMutableData data];
    return self;
}
- (nullable NSString *)beginRecordingForApp:(nullable NSString *)app provider:(nullable NSString *)provider {
    (void)app; (void)provider;
    ++self.beginCount;
    return [NSString stringWithFormat:@"rec-%lu", (unsigned long)self.beginCount];
}
- (void)appendPCM:(NSData *)pcm { [self.archivedPCM appendData:pcm]; }
- (void)endCaptureDiscarding:(BOOL)discard { ++self.endCount; self.lastDiscarded = discard; }
- (void)noteText:(NSString *)text forRecording:(NSString *)identifier { (void)text; (void)identifier; }
- (void)noteFailure:(NSString *)reason forRecording:(NSString *)identifier { (void)reason; (void)identifier; }
- (void)noteCancelledRecording:(NSString *)identifier { (void)identifier; }
@end

@interface HotSwapVoiceService : MSIMEVoiceInputService
@property(nonatomic, strong) NSMutableArray<FakeAudioEngine *> *engineQueue;
@property(nonatomic, strong) FakeAudioEngine *currentEngine;
@end

@implementation HotSwapVoiceService
- (AVAuthorizationStatus)microphoneAuthorizationStatus { return AVAuthorizationStatusAuthorized; }
- (AVAudioEngine *)makeAudioEngine {
    FakeAudioEngine *next = self.engineQueue.firstObject;
    if (next) [self.engineQueue removeObjectAtIndex:0];
    else {
        next = [FakeAudioEngine new];
        next.inputNode = [FakeAudioInputNode new];
        next.inputNode.sampleRate = 16000;
        next.inputNode.channels = 1;
        next.failStart = YES;
    }
    self.currentEngine = next;
    return (id)next;
}
@end

static void TestCaptureDeviceFallback() {
    MSIMEVoiceCaptureDeviceListAPI api{FallbackDeviceFixture::Get, FallbackDeviceFixture::Size};
    NSString *uid = nil;
    NSString *name = nil;
    BOOL fellBack = NO;

    FallbackDeviceFixture::includeAny = YES;
    FallbackDeviceFixture::includeUSB = YES;
    assert(MSIMEVoiceResolveCaptureDeviceWithFallback(@"uid-usb", &uid, &name, &fellBack, api));
    assert([uid isEqualToString:@"uid-usb"] && [name isEqualToString:@"USB Mic"] && !fellBack);

    // 模拟首选 USB 麦克风拔出：自动回退到存活的系统默认内置麦，而非硬失败。
    FallbackDeviceFixture::includeUSB = NO;
    assert(MSIMEVoiceResolveCaptureDeviceWithFallback(@"uid-usb", &uid, &name, &fellBack, api));
    assert([uid isEqualToString:@"uid-builtin"] && [name isEqualToString:@"Built-in Mic"] && fellBack);

    // 模拟 USB 麦克风假连接卡死（两台都在列表中）：主动绕开卡死设备并优先选择内置麦。
    FallbackDeviceFixture::includeUSB = YES;
    NSString *altName = nil;
    NSString *altUID = MSIMEVoiceAlternativeCaptureDeviceUID(@"uid-usb", &altName, api);
    assert([altUID isEqualToString:@"uid-builtin"] && [altName isEqualToString:@"Built-in Mic"]);
    altUID = MSIMEVoiceAlternativeCaptureDeviceUID(@"uid-builtin", &altName, api);
    assert([altUID isEqualToString:@"uid-usb"] && [altName isEqualToString:@"USB Mic"]);

    // 模拟系统完全没有可用输入设备。
    FallbackDeviceFixture::includeAny = NO;
    assert(!MSIMEVoiceResolveCaptureDeviceWithFallback(@"uid-usb", &uid, &name, &fellBack, api));
    assert(MSIMEVoiceAlternativeCaptureDeviceUID(@"uid-usb", &altName, api) == nil);
}

static void TestPCMBufferFormatTransition() {
    MSIMEVoicePCMBuffer *buf = [[MSIMEVoicePCMBuffer alloc] initWithSampleLimit:NSUIntegerMax];
    NSMutableData *streamed = [NSMutableData data];

    // 段 1：16 kHz 单声道 0.5 秒
    assert([buf append:MakeBuffer(16000, 1, 8000, 0.25f) error:nil]);
    [streamed appendData:[buf drainWithError:nil]];
    assert([buf transitionInputFormatWithError:nil]);

    // 段 2：48 kHz 双声道 0.5 秒（模拟 AirPods 切到内置/USB 麦）
    assert([buf append:MakeBuffer(48000, 2, 24000, 0.50f) error:nil]);
    [streamed appendData:[buf drainWithError:nil]];
    assert([buf transitionInputFormatWithError:nil]);

    // 段 3：44.1 kHz 单声道 0.5 秒
    assert([buf append:MakeBuffer(44100, 1, 22050, 0.125f) error:nil]);
    [streamed appendData:[buf drainWithError:nil]];

    NSData *tail = [buf finishWithError:nil];
    assert(tail != nil);
    NSData *tailDrain = [buf drainWithError:nil];
    [streamed appendData:tailDrain];

    const NSUInteger totalFrames = streamed.length / sizeof(float);
    assert(std::abs((long)totalFrames - 24000L) <= 4);
    const float *samples = static_cast<const float *>(streamed.bytes);
    assert(std::fabs(samples[4000] - 0.25f) < 0.02f);
    assert(std::fabs(samples[12000] - 0.50f) < 0.02f);
    assert(std::fabs(samples[20000] - 0.125f) < 0.02f);

    // 未声明切换时直接改变输入格式仍必须严格失败（保持既有安全契约）。
    MSIMEVoicePCMBuffer *strict = [MSIMEVoicePCMBuffer new];
    assert([strict append:MakeBuffer(16000, 1, 1600, 0.2f) error:nil]);
    assert(![strict append:MakeBuffer(48000, 1, 480, 0.2f) error:nil]);
}

static void TestHotSwapMigrationAndDeviceLossSalvage() {
    FakeArchive *archive = [FakeArchive new];
    id<MSIMEVoiceArchiving> prevArchive = MSIMEVoiceInputService.archive;
    MSIMEVoiceInputService.archive = archive;

    HotSwapVoiceService *service = [HotSwapVoiceService new];
    FakeAudioEngine *engine1 = [FakeAudioEngine new];
    engine1.inputNode = [FakeAudioInputNode new];
    engine1.inputNode.sampleRate = 16000;
    engine1.inputNode.channels = 1;

    FakeAudioEngine *engine2 = [FakeAudioEngine new];
    engine2.inputNode = [FakeAudioInputNode new];
    engine2.inputNode.sampleRate = 48000;
    engine2.inputNode.channels = 2;

    service.engineQueue = [NSMutableArray arrayWithObjects:engine1, engine2, nil];

    NSMutableData *streamed = [NSMutableData data];
    __block NSUInteger routeChanges = 0;
    __block BOOL lastRecovered = NO;
    service.captureRouteDidChangeHandler = ^(NSString *deviceName, BOOL recovered) {
        (void)deviceName;
        ++routeChanges;
        lastRecovered = recovered;
    };

    assert([service startPCMStreaming:^(NSData *pcm, NSError *error) {
        assert(!error && pcm.length > 0);
        [streamed appendData:pcm];
    } deviceUID:nil error:nil]);
    assert(archive.beginCount == 1);
    NSString *initialRecordingID = service.archiveRecordingID;
    assert(initialRecordingID.length > 0);

    // 引擎 1 采集 0.5 秒语音
    AVAudioTime *ts = [AVAudioTime timeWithSampleTime:0 atRate:16000];
    engine1.inputNode.tap(MakeSineBuffer(16000, 1, 8000, 0.25f), ts);
    assert(std::fabs(service.recordedDuration - 0.5) < 0.01);

    // 模拟设备中途闪断触发无缝迁移至引擎 2（48 kHz 2ch）
    assert([service recoverAudioCaptureForConfigurationChange]);
    assert(routeChanges == 1 && lastRecovered);
    assert(service.currentEngine == engine2);
    assert(archive.beginCount == 1); // 仍属同一条录音存档，不割裂！
    assert([service.archiveRecordingID isEqualToString:initialRecordingID]);

    // 引擎 2 继续采集 0.5 秒语音
    AVAudioTime *ts2 = [AVAudioTime timeWithSampleTime:0 atRate:48000];
    engine2.inputNode.tap(MakeSineBuffer(48000, 2, 24000, 0.25f), ts2);
    assert(std::fabs(service.recordedDuration - 1.0) < 0.02);

    NSData *tail = [service finishPCMStreamingWithError:nil];
    assert(tail != nil);
    [streamed appendData:tail];
    assert(std::abs((long)(streamed.length / sizeof(float)) - 16000L) <= 8);
    assert(archive.endCount == 1 && !archive.lastDiscarded);
    assert(archive.archivedPCM.length == streamed.length);

    // 验证彻底失去所有麦克风时的抢救回调：已录 0.6 秒语音仍被完整保留，不清空！
    HotSwapVoiceService *lossService = [HotSwapVoiceService new];
    FakeAudioEngine *liveEngine = [FakeAudioEngine new];
    liveEngine.inputNode = [FakeAudioInputNode new];
    liveEngine.inputNode.sampleRate = 16000;
    liveEngine.inputNode.channels = 1;
    lossService.engineQueue = [NSMutableArray arrayWithObject:liveEngine];

    __block BOOL lostFired = NO;
    lossService.captureDeviceLostHandler = ^(NSError *error) {
        assert(error != nil);
        lostFired = YES;
    };
    assert([lossService startPCMStreaming:^(NSData *pcm, NSError *error) {
        (void)pcm; (void)error;
    } deviceUID:nil error:nil]);
    liveEngine.inputNode.tap(MakeSineBuffer(16000, 1, 9600, 0.25f), ts);
    assert(std::fabs(lossService.recordedDuration - 0.6) < 0.02);
    assert(lossService.hasLikelySpeech);

    // 触发配置变更；后续队列为空全部 failStart，多次退避重试后应触发 deviceLost 回调且保留已录时长与 PCM。
    [lossService recoverAudioCaptureForConfigurationChange];
    // 4 次退避总计 0.22*(1+2+3+4) = 2.2s，等待回调触发。
    PumpMainRunLoop(2.6);
    assert(lostFired);
    assert(std::fabs(lossService.recordedDuration - 0.6) < 0.02);
    assert(lossService.hasLikelySpeech);
    NSData *salvageTail = [lossService finishPCMStreamingWithError:nil];
    assert(salvageTail != nil);

    MSIMEVoiceInputService.archive = prevArchive;
}

static void TestTargetEditabilityClassification() {
    // 1. 前台应用切换导致与 IMK 客户端不一致 -> 明确不可编辑（无需 AX 权限即可拦截丢字）。
    {
        MSIMEVoiceTargetEditableInput in;
        in.frontmostBundleID = @"com.apple.finder";
        in.clientBundleID = @"com.google.Chrome";
        in.axTrusted = NO;
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateNotEditable);
    }
    // 2. 终端 / JetBrains / Zed 只要前台匹配，默认放行。
    for (NSString *bundle in @[@"com.apple.Terminal", @"com.mitchellh.ghostty", @"com.jetbrains.intellij", @"dev.zed.Zed"]) {
        MSIMEVoiceTargetEditableInput in;
        in.frontmostBundleID = bundle;
        in.clientBundleID = bundle;
        in.axTrusted = NO;
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateEditable);
    }
    // 3. AX 未授权且前台应用匹配 -> 保守未知（允许 IMK 尝试）。
    {
        MSIMEVoiceTargetEditableInput in;
        in.frontmostBundleID = @"com.apple.Notes";
        in.clientBundleID = @"com.apple.Notes";
        in.axTrusted = NO;
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateUnknown);
    }
    // 4. AX 已授权但无焦点元素 / 按钮 / 静态文本 / 无可编辑祖先的空白网页 -> 明确不可编辑。
    {
        MSIMEVoiceTargetEditableInput in;
        in.frontmostBundleID = @"com.google.Chrome";
        in.clientBundleID = @"com.google.Chrome";
        in.axTrusted = YES;
        in.hasFocusedAXElement = NO;
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateNotEditable);

        in.hasFocusedAXElement = YES;
        in.axRole = @"AXButton";
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateNotEditable);

        in.axRole = @"AXStaticText";
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateNotEditable);

        in.axRole = @"AXWebArea";
        in.axHasEditableAncestor = NO;
        in.axValueSettable = NO;
        in.axHasSelectedTextRange = NO;
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateNotEditable);
    }
    // 5. 真正可编辑元素（AXTextArea / AXEditableAncestor / settable AXValue）-> 可编辑。
    {
        MSIMEVoiceTargetEditableInput in;
        in.frontmostBundleID = @"com.google.Chrome";
        in.clientBundleID = @"com.google.Chrome";
        in.axTrusted = YES;
        in.hasFocusedAXElement = YES;
        in.axRole = @"AXWebArea";
        in.axHasEditableAncestor = YES;
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateEditable);

        in.axHasEditableAncestor = NO;
        in.axRole = @"AXTextArea";
        assert(MSIMEClassifyVoiceTargetEditability(in) == MSIMEVoiceTargetEditableStateEditable);
    }
    // 6. 无 bundleIdentifier 的合成测试客户端 -> Unknown（不碰真实 GUI）。
    assert(MSIMEProbeVoiceTargetEditability([NSObject new]) == MSIMEVoiceTargetEditableStateUnknown);
}

static void TestPasteboardSnapshotAndRestore() {
    NSPasteboard *board = [NSPasteboard pasteboardWithUniqueName];
    [board clearContents];
    assert([board setString:@"user original clipboard content" forType:NSPasteboardTypeString]);

    const MSIMEVoicePasteboardSnapshot saved = MSIMEVoicePasteboardSnapshot::Capture(board);
    assert(saved.valid);

    assert(MSIMEWriteTransientVoiceClipboard(@"transient voice paste", board));
    assert([[board stringForType:NSPasteboardTypeString] isEqualToString:@"transient voice paste"]);
    assert([board.types containsObject:@"org.nspasteboard.TransientType"]);
    assert([board.types containsObject:@"org.nspasteboard.ConcealedType"]);
    assert([board.types containsObject:@"org.nspasteboard.AutoGeneratedType"]);

    const NSInteger transientCount = board.changeCount;
    assert(saved.restoreIfUnchanged(board, transientCount));
    assert([[board stringForType:NSPasteboardTypeString] isEqualToString:@"user original clipboard content"]);

    // 若用户在粘贴后又复制了新内容，恢复必须自动放弃，绝不覆盖用户新剪贴板。
    assert(MSIMEWriteTransientVoiceClipboard(@"another voice paste", board));
    const NSInteger oldCount = board.changeCount;
    [board clearContents];
    assert([board setString:@"newer user copy" forType:NSPasteboardTypeString]);
    assert(!saved.restoreIfUnchanged(board, oldCount));
    assert([[board stringForType:NSPasteboardTypeString] isEqualToString:@"newer user copy"]);

    [board releaseGlobally];
}

static void TestRecoveryToastFormattingAndLifecycle() {
    NSString *collapsed = [MSIMEVoiceRecoveryToast previewSnippetForText:@"第一行\n\n  第二行\t内容" maxCharacters:40];
    assert([collapsed isEqualToString:@"“第一行 第二行 内容”"]);

    NSString *truncated = [MSIMEVoiceRecoveryToast previewSnippetForText:@"这是一段比较长的测试语音转写内容😀用于验证截断边界" maxCharacters:8];
    assert([truncated hasPrefix:@"“"] && [truncated hasSuffix:@"…”"]);

    MSIMEVoiceRecoveryToast *toast = [[MSIMEVoiceRecoveryToast alloc] init];
    [toast showUndeliveredText:@"刚刚说出的测试文本" title:@"当前位置不可输入 · 结果已复制" copied:YES copyHandler:^BOOL(NSString *text) {
        return text.length > 0;
    }];
    assert([toast.currentTitle isEqualToString:@"当前位置不可输入 · 结果已复制"]);
    assert([toast.currentBody isEqualToString:@"“刚刚说出的测试文本”"]);

    [toast showRetryableFailureTitle:@"语音识别失败 · 录音已保留" detail:@"连接超时" retryHandler:^{} historyHandler:^{}];
    assert([toast.currentTitle isEqualToString:@"语音识别失败 · 录音已保留"]);
    assert([toast.currentBody isEqualToString:@"连接超时"]);

    [toast showRouteNoticeTitle:@"麦克风已自动切换" detail:@"当前使用「Built-in Mic」"];
    assert([toast.currentTitle isEqualToString:@"麦克风已自动切换"]);
    [toast dismiss];
    assert(!toast.visible);
}

int main() {
    @autoreleasepool {
        TestCaptureDeviceFallback();
        TestPCMBufferFormatTransition();
        TestHotSwapMigrationAndDeviceLossSalvage();
        TestTargetEditabilityClassification();
        TestPasteboardSnapshotAndRestore();
        TestRecoveryToastFormattingAndLifecycle();
    }
    return 0;
}
