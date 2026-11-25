#import "VoiceInputService.h"
#import "VoicePCMBuffer.h"
#import "VoiceCaptureDevice.h"
#import "BackendSpeechAnalyzer.h"
#include "VoiceAudioPreprocess.h"
#include "../../../../shared/voice/CaptureDuration.h"
#include "../../../../shared/voice/LocalAsr.h"
#import <AVFoundation/AVFoundation.h>
#import <CoreAudio/CoreAudio.h>
#include <atomic>
#include <memory>
#include <mutex>
#include <time.h>

static BOOL MSIMEVoiceStrictGeneration(id value, uint64_t *result) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) return NO;
    NSNumber *number = (NSNumber *)value;
    if ([number compare:@0] == NSOrderedAscending) return NO;
    uint64_t generation = number.unsignedLongLongValue;
    if ([number compare:@(generation)] != NSOrderedSame) return NO;
    if (result) *result = generation;
    return YES;
}

namespace {
struct PCMStreamAdmission { std::mutex mutex; bool live = true; };

enum class PCMCaptureMode : int { None = 0, BatchRecording, Streaming };

constexpr NSUInteger kMaxEngineRecoveryRetries = 4;
constexpr NSUInteger kMaxSessionMigrations = 8;
constexpr NSTimeInterval kConfigChangeDebounceSec = 0.12;
constexpr NSTimeInterval kStallWatchdogIntervalSec = 0.6;
constexpr uint64_t kInitialStallGraceNs = 2400000000ULL; // 2.4 秒：容忍蓝牙 HFP 初次协商握手
constexpr uint64_t kActiveStallThresholdNs = 1800000000ULL; // 1.8 秒无任何音频回调视为采集流卡死

inline uint64_t MonotonicUptimeNs() {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

// 对 16 kHz 单声道 float PCM 执行前端预处理，返回独立的可安全传递数据块，不修改入参底层缓冲。
NSData *PreprocessPCMData(NSData *pcm, msime::voice::VoiceAudioPreprocessor *preprocessor) {
    if (!pcm.length || !preprocessor) return pcm;
    NSMutableData *processed = [pcm mutableCopy];
    float *samples = static_cast<float *>(processed.mutableBytes);
    const size_t count = processed.length / sizeof(float);
    preprocessor->processInPlace(samples, count);
    return processed;
}
} // namespace

@implementation MSIMEVoiceInputService {
    __weak MSIMEClientSession *_session;
    BOOL _active;
    AVAudioEngine *_audioEngine;
    SFSpeechRecognizer *_recognizer;
    SFSpeechAudioBufferRecognitionRequest *_speechRequest;
    SFSpeechRecognitionTask *_speechTask;
    id<MSIMEBackendSpeechAnalyzerSession> _analyzer;
    uint64_t _transcriptionGeneration;
    MSIMEVoicePCMBuffer *_pcmRecording;
    std::shared_ptr<PCMStreamAdmission> _pcmStreamLive;
    std::shared_ptr<msime::voice::CaptureDuration> _captureDuration;
    // 跨多次麦克风热切换累计的已录制时长（秒）。
    NSTimeInterval _accumulatedDurationSeconds;
    NSTimeInterval _recordedDuration;
    NSString *_archiveRecordingID;
    BOOL _archiving;

    // 单次 16 kHz 重采样与前端预处理状态：生产 tap 中共享给存档与识别，消除双重 AVAudioConverter 开销；
    // 测试子类覆写 startMicrophoneCapture: 时保留既有兼容路径。
    PCMCaptureMode _pcmCaptureMode;
    BOOL _pcmTapHandled;
    std::shared_ptr<std::atomic_bool> _pendingTapHandled;
    MSIMEVoicePCMChunk _pcmStreamHandler;
    void (^_pcmBatchFailure)(NSError *);
    std::shared_ptr<std::atomic_bool> _pcmBatchFailed;
    NSMutableData *_pcmBatchAccumulated;
    MSIMEVoicePCMBuffer *_tapStandalonePCM;
    std::shared_ptr<std::mutex> _tapMutex;
    std::shared_ptr<msime::voice::VoiceAudioPreprocessor> _audioPreprocessor;
    BOOL _hasPreprocessStats;
    BOOL _lastHasLikelySpeech;
    uint32_t _lastActiveSpeechMs;
    float _lastPeakAbs;

    // 音频设备热迁移与卡死恢复状态（仅在主线程读写，`_lastAudioBufferNs` 由采集回调原子更新）。
    NSUInteger _captureSessionGeneration;
    BOOL _captureSessionOpen;
    MSIMEVoiceAudioBuffer _activeBufferHandler;
    NSString *_requestedDeviceUID;
    NSString *_activeCaptureDeviceUID;
    NSString *_activeCaptureDeviceName;
    id _audioConfigObserver;
    AudioObjectPropertyListenerBlock _halDefaultInputListener;
    AudioObjectPropertyListenerBlock _halDevicesListener;
    BOOL _halListenersRegistered;
    NSTimer *_captureStallWatchdog;
    std::shared_ptr<std::atomic<uint64_t>> _lastAudioBufferNs;
    uint64_t _captureEngineStartedNs;
    BOOL _recoveryScheduled;
    NSUInteger _recoveryRetryCount;
    NSUInteger _sessionMigrationCount;
}
static id<MSIMEVoiceArchiving> MSIMEVoiceInputArchive;
+ (id<MSIMEVoiceArchiving>)archive { return MSIMEVoiceInputArchive; }
+ (void)setArchive:(id<MSIMEVoiceArchiving>)archive { MSIMEVoiceInputArchive = archive; }
- (NSString *)archiveRecordingID { return _archiveRecordingID; }
- (NSString *)activeCaptureDeviceName { return _activeCaptureDeviceName; }
- (NSTimeInterval)recordedDuration {
    if (_captureDuration) return _accumulatedDurationSeconds + _captureDuration->seconds();
    return _captureSessionOpen ? _accumulatedDurationSeconds : _recordedDuration;
}
- (BOOL)hasLikelySpeech {
    if (_audioPreprocessor && _tapMutex) {
        std::lock_guard<std::mutex> lock(*_tapMutex);
        return _audioPreprocessor->hasLikelySpeech() ? YES : NO;
    }
    return _hasPreprocessStats ? _lastHasLikelySpeech : YES;
}
- (uint32_t)activeSpeechMs {
    if (_audioPreprocessor && _tapMutex) {
        std::lock_guard<std::mutex> lock(*_tapMutex);
        return _audioPreprocessor->activeSpeechMs();
    }
    return _hasPreprocessStats ? _lastActiveSpeechMs : 0;
}
- (float)peakAbs {
    if (_audioPreprocessor && _tapMutex) {
        std::lock_guard<std::mutex> lock(*_tapMutex);
        return _audioPreprocessor->peakAbs();
    }
    return _hasPreprocessStats ? _lastPeakAbs : 0.0f;
}
- (BOOL)isLikelyAccidentalSilence {
    if (_audioPreprocessor && _tapMutex) {
        std::lock_guard<std::mutex> lock(*_tapMutex);
        return _audioPreprocessor->isLikelyAccidentalSilence(self.recordedDuration) ? YES : NO;
    }
    return _hasPreprocessStats ? (_recordedDuration < 2.5 && !_lastHasLikelySpeech) : NO;
}
- (AVAuthorizationStatus)microphoneAuthorizationStatus { return [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]; }
- (void)requestMicrophonePermission:(void (^)(BOOL))completion { [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) { dispatch_async(dispatch_get_main_queue(), ^{ completion(granted); }); }]; }
- (SFSpeechRecognizerAuthorizationStatus)speechAuthorizationStatus { return [SFSpeechRecognizer authorizationStatus]; }
- (void)requestSpeechPermission:(void (^)(BOOL))completion { [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) { dispatch_async(dispatch_get_main_queue(), ^{ completion(status == SFSpeechRecognizerAuthorizationStatusAuthorized); }); }]; }
- (BOOL)isActive { return _active; }
- (void)clearPCMPipelineState {
    _pcmCaptureMode = PCMCaptureMode::None;
    _pcmTapHandled = NO;
    _pendingTapHandled.reset();
    _pcmStreamHandler = nil;
    _pcmBatchFailure = nil;
    _pcmBatchFailed.reset();
    _pcmBatchAccumulated = nil;
    _tapStandalonePCM = nil;
    _audioPreprocessor.reset();
    _tapMutex.reset();
}
- (BOOL)startPCMRecording:(MSIMEVoiceAudioBuffer)handler deviceUID:(NSString *)deviceUID error:(NSError **)error {
    return [self startPCMRecording:handler deviceUID:deviceUID failure:nil error:error];
}
- (BOOL)startPCMRecording:(MSIMEVoiceAudioBuffer)handler deviceUID:(NSString *)deviceUID failure:(void (^)(NSError *))failure error:(NSError **)error {
    if (_pcmRecording || _audioEngine || _captureSessionOpen || _speechTask) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:4
            userInfo:@{NSLocalizedDescriptionKey: @"录音已在进行中"}];
        return NO;
    }
    MSIMEVoicePCMBuffer *recording = [MSIMEVoicePCMBuffer new];
    auto failed = std::make_shared<std::atomic_bool>(false);
    auto tapHandled = std::make_shared<std::atomic_bool>(false);
    _pcmRecording = recording;
    _pcmCaptureMode = PCMCaptureMode::BatchRecording;
    _pcmTapHandled = NO;
    _pendingTapHandled = tapHandled;
    _pcmBatchFailure = [failure copy];
    _pcmBatchFailed = failed;
    _pcmBatchAccumulated = [NSMutableData data];
    __weak MSIMEVoiceInputService *weakSelf = self;
    // 捕获本次录音对象而非可变槽位：停止或取消后的迟到回调绝不能写入下一轮录音。
    BOOL started = [self startMicrophoneCapture:^(AVAudioPCMBuffer *buffer) {
        if (tapHandled->load(std::memory_order_acquire)) {
            if (handler) handler(buffer);
            return;
        }
        NSError *conversionError = nil;
        if ([recording append:buffer error:&conversionError]) {
            if (handler) handler(buffer);
        } else if (!failed->exchange(true)) {
            // 严禁在 AVAudioEngine 采集回调内部直接停机。
            dispatch_async(dispatch_get_main_queue(), ^{
                MSIMEVoiceInputService *service = weakSelf;
                if (!service || service->_pcmRecording != recording) return;
                [service cancelWithError:nil];
                if (failure) failure(conversionError);
            });
        }
    } deviceUID:deviceUID error:error];
    _pendingTapHandled.reset();
    if (!started) {
        [recording cancel];
        _pcmRecording = nil;
        [self clearPCMPipelineState];
    }
    return started;
}
- (NSData *)finishPCMRecordingWithError:(NSError **)error {
    MSIMEVoicePCMBuffer *recording = _pcmRecording;
    if (!recording) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:5
            userInfo:@{NSLocalizedDescriptionKey: @"没有可提交的录音"}];
        return nil;
    }
    const BOOL tapHandled = _pcmTapHandled;
    const PCMCaptureMode mode = _pcmCaptureMode;
    NSMutableData *accumulated = _pcmBatchAccumulated;
    auto preprocessor = _audioPreprocessor;
    auto tapMutex = _tapMutex;

    [self stopPCMStreamDelivery];
    _pcmRecording = nil;
    if (!tapHandled) {
        [self clearPCMPipelineState];
        [self stopMicrophoneCapture];
        return [recording finishWithError:error];
    }

    // 生产单次重采样路径：先停采集引擎并完成尾帧预处理，再按语音活跃度决定是否保留存档。
    [self stopAudioEngineCapture];
    NSData *tail = nil;
    NSData *result = nil;
    {
        std::unique_lock<std::mutex> lock;
        if (tapMutex) lock = std::unique_lock<std::mutex>(*tapMutex);
        tail = [recording finishWithError:error];
        if (tail) {
            if (mode == PCMCaptureMode::Streaming) {
                tail = [recording drainWithError:error];
            }
            if (tail) {
                NSData *processedTail = tail.length ? PreprocessPCMData(tail, preprocessor.get()) : tail;
                if (processedTail.length && _archiving) [MSIMEVoiceInputArchive appendPCM:processedTail];
                if (mode == PCMCaptureMode::BatchRecording) {
                    if (processedTail.length) [accumulated appendData:processedTail];
                    result = [accumulated copy] ?: NSData.data;
                } else {
                    result = processedTail;
                }
            }
        }
        if (preprocessor) {
            _hasPreprocessStats = YES;
            _lastHasLikelySpeech = preprocessor->hasLikelySpeech() ? YES : NO;
            _lastActiveSpeechMs = preprocessor->activeSpeechMs();
            _lastPeakAbs = preprocessor->peakAbs();
        }
    }
    [self clearPCMPipelineState];
    const BOOL discard = (!result) || [self shouldDiscardArchiveForCurrentCapture];
    [self endArchiveDiscarding:discard];
    return result;
}
- (BOOL)startPCMStreaming:(MSIMEVoicePCMChunk)handler deviceUID:(NSString *)deviceUID error:(NSError **)error {
    if (!handler || _pcmRecording || _audioEngine || _captureSessionOpen || _speechTask) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:4
            userInfo:@{NSLocalizedDescriptionKey:@"无法开始流式录音"}];
        return NO;
    }
    // 流式录音按块排空，不设总时长上限，仅暂存尚未投递给识别器的尾部样本。
    MSIMEVoicePCMBuffer *recording = [[MSIMEVoicePCMBuffer alloc] initWithSampleLimit:NSUIntegerMax];
    auto live = std::make_shared<PCMStreamAdmission>();
    auto tapHandled = std::make_shared<std::atomic_bool>(false);
    _pcmRecording = recording;
    _pcmStreamLive = live;
    _pcmCaptureMode = PCMCaptureMode::Streaming;
    _pcmTapHandled = NO;
    _pendingTapHandled = tapHandled;
    _pcmStreamHandler = [handler copy];
    // 录音缓冲与准入令牌均绑定本次 tap，不跨轮复用。
    BOOL started = [self startMicrophoneCapture:^(AVAudioPCMBuffer *buffer) {
        if (tapHandled->load(std::memory_order_acquire)) return;
        std::lock_guard<std::mutex> lock(live->mutex);
        if (!live->live) return;
        NSError *failure = nil;
        NSData *pcm = [recording append:buffer error:&failure] ? [recording drainWithError:&failure] : nil;
        if (!pcm) {
            live->live = false; handler(nil, failure);
        } else if (pcm.length) handler(pcm, nil);
    } deviceUID:deviceUID error:error];
    _pendingTapHandled.reset();
    if (!started) {
        [self stopPCMStreamDelivery];
        [recording cancel];
        _pcmRecording = nil;
        [self clearPCMPipelineState];
    }
    return started;
}
- (void)stopPCMStreamDelivery {
    auto live = _pcmStreamLive;
    if (!live) return;
    {
        // 等待正在回调中的流式排空与派发完成，再于锁外拆 tap 并停引擎。
        std::lock_guard<std::mutex> lock(live->mutex);
        live->live = false;
    }
    _pcmStreamLive.reset();
}
- (NSData *)finishPCMStreamingWithError:(NSError **)error {
    MSIMEVoicePCMBuffer *recording = _pcmRecording;
    const BOOL tapHandled = _pcmTapHandled;
    NSData *finished = [self finishPCMRecordingWithError:error];
    if (!finished) return nil;
    // 生产单次重采样路径已在 finishPCMRecordingWithError: 中排空并完成尾帧预处理。
    return tapHandled ? finished : [recording drainWithError:error];
}

// 拆除当前 AVAudioEngine 与配置变更观察者，但保留整个录音会话（存档、预处理器、缓冲与流式回调）以供热迁移复用。
- (void)teardownAudioEngineInstanceOnly {
    if (_audioConfigObserver) {
        [NSNotificationCenter.defaultCenter removeObserver:_audioConfigObserver];
        _audioConfigObserver = nil;
    }
    if (_captureDuration) {
        _accumulatedDurationSeconds += _captureDuration->finish();
        _captureDuration.reset();
    }
    if (_audioEngine) {
        [_audioEngine.inputNode removeTapOnBus:0];
        [_audioEngine stop];
        _audioEngine = nil;
    }
}

- (void)stopCaptureResilienceMonitors {
    if (_captureStallWatchdog) {
        [_captureStallWatchdog invalidate];
        _captureStallWatchdog = nil;
    }
    if (_halListenersRegistered) {
        _halListenersRegistered = NO;
        AudioObjectPropertyAddress defaultInputAddr = {
            kAudioHardwarePropertyDefaultInputDevice,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain
        };
        AudioObjectPropertyAddress devicesAddr = {
            kAudioHardwarePropertyDevices,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain
        };
        if (_halDefaultInputListener) {
            AudioObjectRemovePropertyListenerBlock(kAudioObjectSystemObject, &defaultInputAddr,
                                                   dispatch_get_main_queue(), _halDefaultInputListener);
            _halDefaultInputListener = nil;
        }
        if (_halDevicesListener) {
            AudioObjectRemovePropertyListenerBlock(kAudioObjectSystemObject, &devicesAddr,
                                                   dispatch_get_main_queue(), _halDevicesListener);
            _halDevicesListener = nil;
        }
    }
}

- (void)startCaptureResilienceMonitorsForRealEngine {
    if (_halListenersRegistered) return;
    __weak MSIMEVoiceInputService *weakSelf = self;
    const NSUInteger generation = _captureSessionGeneration;
    AudioObjectPropertyAddress defaultInputAddr = {
        kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain
    };
    AudioObjectPropertyAddress devicesAddr = {
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain
    };
    _halDefaultInputListener = ^(UInt32 numberAddresses, const AudioObjectPropertyAddress *addresses) {
        (void)numberAddresses; (void)addresses;
        MSIMEVoiceInputService *service = weakSelf;
        if (!service || !service->_captureSessionOpen || service->_captureSessionGeneration != generation) return;
        // 仅当用户采用系统默认设备或当前使用设备已变更/掉线时才触发无缝切换，避免无意义的重启闪烁。
        if (!service->_requestedDeviceUID.length) {
            [service scheduleDebouncedCaptureRecoveryForGeneration:generation];
        } else {
            BOOL fellBack = NO;
            NSString *resolvedUID = nil;
            NSString *resolvedName = nil;
            MSIMEVoiceResolveCaptureDeviceWithFallback(service->_requestedDeviceUID, &resolvedUID, &resolvedName, &fellBack);
            if (fellBack || (resolvedName.length && ![resolvedName isEqualToString:service->_activeCaptureDeviceName])) {
                [service scheduleDebouncedCaptureRecoveryForGeneration:generation];
            }
        }
    };
    _halDevicesListener = ^(UInt32 numberAddresses, const AudioObjectPropertyAddress *addresses) {
        (void)numberAddresses; (void)addresses;
        MSIMEVoiceInputService *service = weakSelf;
        if (!service || !service->_captureSessionOpen || service->_captureSessionGeneration != generation) return;
        if (service->_audioEngine && !service->_audioEngine.isRunning) {
            [service scheduleDebouncedCaptureRecoveryForGeneration:generation];
            return;
        }
        if (service->_requestedDeviceUID.length) {
            BOOL fellBack = NO;
            NSString *resolvedUID = nil;
            NSString *resolvedName = nil;
            if (!MSIMEVoiceResolveCaptureDeviceWithFallback(service->_requestedDeviceUID, &resolvedUID, &resolvedName, &fellBack) || fellBack) {
                [service scheduleDebouncedCaptureRecoveryForGeneration:generation];
            }
        }
    };
    OSStatus s1 = AudioObjectAddPropertyListenerBlock(kAudioObjectSystemObject, &defaultInputAddr,
                                                      dispatch_get_main_queue(), _halDefaultInputListener);
    OSStatus s2 = AudioObjectAddPropertyListenerBlock(kAudioObjectSystemObject, &devicesAddr,
                                                      dispatch_get_main_queue(), _halDevicesListener);
    _halListenersRegistered = (s1 == noErr || s2 == noErr);
    if (s1 != noErr) _halDefaultInputListener = nil;
    if (s2 != noErr) _halDevicesListener = nil;

    _captureStallWatchdog = [NSTimer timerWithTimeInterval:kStallWatchdogIntervalSec repeats:YES block:^(NSTimer *timer) {
        MSIMEVoiceInputService *service = weakSelf;
        if (!service || !service->_captureSessionOpen || service->_captureSessionGeneration != generation) {
            [timer invalidate];
            return;
        }
        if (service->_recoveryScheduled) return;
        const uint64_t now = MonotonicUptimeNs();
        const uint64_t lastFrame = service->_lastAudioBufferNs ? service->_lastAudioBufferNs->load(std::memory_order_acquire) : 0;
        const BOOL engineStopped = (service->_audioEngine != nil && !service->_audioEngine.isRunning);
        const BOOL stalledAfterFirstFrame = (lastFrame > 0 && now > lastFrame && (now - lastFrame) >= kActiveStallThresholdNs);
        const BOOL stalledWithoutAnyFrame = (lastFrame == 0 && now > service->_captureEngineStartedNs &&
                                             (now - service->_captureEngineStartedNs) >= kInitialStallGraceNs);
        if (engineStopped || stalledAfterFirstFrame || stalledWithoutAnyFrame) {
            [service scheduleDebouncedCaptureRecoveryForGeneration:generation preferAlternativeDevice:(stalledAfterFirstFrame || stalledWithoutAnyFrame)];
        }
    }];
    [NSRunLoop.mainRunLoop addTimer:_captureStallWatchdog forMode:NSRunLoopCommonModes];
}

- (void)scheduleDebouncedCaptureRecoveryForGeneration:(NSUInteger)generation {
    [self scheduleDebouncedCaptureRecoveryForGeneration:generation preferAlternativeDevice:NO];
}

- (void)scheduleDebouncedCaptureRecoveryForGeneration:(NSUInteger)generation preferAlternativeDevice:(BOOL)preferAlternative {
    if (!_captureSessionOpen || _captureSessionGeneration != generation) return;
    if (_recoveryScheduled) return;
    _recoveryScheduled = YES;
    __weak MSIMEVoiceInputService *weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kConfigChangeDebounceSec * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
        MSIMEVoiceInputService *service = weakSelf;
        if (!service || !service->_captureSessionOpen || service->_captureSessionGeneration != generation) return;
        service->_recoveryScheduled = NO;
        if (preferAlternative) {
            NSString *altName = nil;
            NSString *altUID = MSIMEVoiceAlternativeCaptureDeviceUID(service->_activeCaptureDeviceUID ?: service->_requestedDeviceUID, &altName);
            if (altUID.length) {
                // 当前设备持续 0 帧卡死（常见于假连接蓝牙 HFP）：本次会话优先切到另一台存活麦克风，避免在死设备上死循环。
                service->_requestedDeviceUID = [altUID copy];
            }
        }
        [service recoverAudioCaptureForConfigurationChange];
    });
}

// 启动或重建 AVAudioEngine 并挂载共享采集 tap；`isRecovery == YES` 时复用既有存档、预处理器、缓冲与流回调。
- (BOOL)attachAndStartAudioEngineWithDeviceUID:(NSString *)deviceUID
                                    isRecovery:(BOOL)isRecovery
                                  usedFallback:(BOOL *)usedFallbackOut
                                  resolvedName:(NSString * __autoreleasing *)resolvedNameOut
                                         error:(NSError **)error {
    if (usedFallbackOut) *usedFallbackOut = NO;
    if (resolvedNameOut) *resolvedNameOut = nil;

    AVAudioEngine *engine = [self makeAudioEngine];
    if (!engine) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:6
            userInfo:@{NSLocalizedDescriptionKey: @"无法创建音频采集引擎"}];
        return NO;
    }
    AVAudioInputNode *input = engine.inputNode;
    BOOL usedFallback = NO;
    NSString *resolvedName = nil;
    if (!MSIMEConfigureVoiceCaptureDeviceWithFallback(deviceUID, input.audioUnit, &usedFallback, &resolvedName, error)) {
        return NO;
    }
    AVAudioFormat *format = [input inputFormatForBus:0];
    if (!format || !std::isfinite(format.sampleRate) || format.sampleRate < 8000.0 || format.channelCount < 1) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:7
            userInfo:@{NSLocalizedDescriptionKey: @"麦克风音频格式暂不可用"}];
        return NO;
    }

    auto duration = std::make_shared<msime::voice::CaptureDuration>(format.sampleRate);
    _captureDuration = duration;
    _audioEngine = engine;
    _activeCaptureDeviceName = resolvedName.length ? [resolvedName copy] : nil;
    if ([engine isKindOfClass:AVAudioEngine.class]) {
        NSString *resolvedUID = nil;
        NSString *ignoredName = nil;
        BOOL ignoredFallback = NO;
        if (MSIMEVoiceResolveCaptureDeviceWithFallback(deviceUID, &resolvedUID, &ignoredName, &ignoredFallback)) {
            _activeCaptureDeviceUID = [resolvedUID copy];
        } else {
            _activeCaptureDeviceUID = [deviceUID copy];
        }
    } else {
        _activeCaptureDeviceUID = [deviceUID copy];
    }

    id<MSIMEVoiceArchiving> archive = MSIMEVoiceInputArchive;
    NSString *archiveID = _archiveRecordingID;
    auto preprocessor = _audioPreprocessor;
    auto tapMutex = _tapMutex;
    const PCMCaptureMode mode = _pcmCaptureMode;
    MSIMEVoicePCMBuffer *sharedRecording = (mode != PCMCaptureMode::None) ? _pcmRecording : nil;
    MSIMEVoicePCMBuffer *standalonePCM = _tapStandalonePCM;
    auto streamLive = _pcmStreamLive;
    MSIMEVoicePCMChunk streamHandler = _pcmStreamHandler;
    NSMutableData *batchAccumulated = _pcmBatchAccumulated;
    void (^batchFailure)(NSError *) = _pcmBatchFailure;
    auto batchFailed = _pcmBatchFailed;
    SFSpeechAudioBufferRecognitionRequest *speechRequest = _speechRequest;
    id<MSIMEBackendSpeechAnalyzerSession> analyzer = _analyzer;
    MSIMEVoiceAudioBuffer bufferHandler = _activeBufferHandler;
    auto lastBufferNs = _lastAudioBufferNs;
    __weak MSIMEVoiceInputService *weakSelf = self;

    // 合并存档与识别为单次 16 kHz 单声道 f32 转换，经 VoiceAudioPreprocessor 处理后同时供给存档与识别。
    AVAudioNodeTapBlock capture = ^(AVAudioPCMBuffer *buffer, AVAudioTime *time) {
        (void)time;
        if (!duration->append(buffer.frameLength)) return;
        if (lastBufferNs) lastBufferNs->store(MonotonicUptimeNs(), std::memory_order_release);
        if (mode == PCMCaptureMode::Streaming && sharedRecording && streamLive) {
            std::lock_guard<std::mutex> streamLock(streamLive->mutex);
            if (!streamLive->live) return;
            std::lock_guard<std::mutex> dspLock(*tapMutex);
            NSError *failure = nil;
            NSData *rawPCM = [sharedRecording append:buffer error:&failure] ? [sharedRecording drainWithError:&failure] : nil;
            if (!rawPCM) {
                streamLive->live = false;
                if (streamHandler) streamHandler(nil, failure);
                return;
            }
            if (rawPCM.length) {
                NSData *processed = PreprocessPCMData(rawPCM, preprocessor.get());
                if (archiveID) [archive appendPCM:processed];
                if (streamHandler) streamHandler(processed, nil);
            }
            return;
        }
        if (mode == PCMCaptureMode::BatchRecording && sharedRecording) {
            NSError *conversionError = nil;
            BOOL converted = NO;
            {
                std::lock_guard<std::mutex> dspLock(*tapMutex);
                if ([sharedRecording append:buffer error:&conversionError]) {
                    NSData *rawPCM = [sharedRecording drainWithError:&conversionError];
                    if (rawPCM) {
                        if (rawPCM.length) {
                            NSData *processed = PreprocessPCMData(rawPCM, preprocessor.get());
                            [batchAccumulated appendData:processed];
                            if (archiveID) [archive appendPCM:processed];
                        }
                        converted = YES;
                    }
                }
            }
            if (converted) {
                if (bufferHandler) bufferHandler(buffer);
            } else if (batchFailed && !batchFailed->exchange(true)) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    MSIMEVoiceInputService *service = weakSelf;
                    if (!service || service->_pcmRecording != sharedRecording) return;
                    [service cancelWithError:nil];
                    if (batchFailure) batchFailure(conversionError);
                });
            }
            return;
        }
        // 系统 Speech / SpeechAnalyzer 路径：同样复用单次 16 kHz 转换完成前端语音活跃统计与存档写入。
        if (standalonePCM) {
            std::lock_guard<std::mutex> dspLock(*tapMutex);
            if ([standalonePCM append:buffer error:nil]) {
                NSData *rawPCM = [standalonePCM drainWithError:nil];
                if (rawPCM.length) {
                    NSData *processed = PreprocessPCMData(rawPCM, preprocessor.get());
                    if (archiveID) [archive appendPCM:processed];
                }
            }
        }
        [speechRequest appendAudioPCMBuffer:buffer];
        [analyzer appendBuffer:buffer];
        if (bufferHandler) bufferHandler(buffer);
    };

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    [input installTapOnBus:0 bufferSize:1024 format:format block:capture];
#pragma clang diagnostic pop

    NSError *startError = nil;
    if (![engine startAndReturnError:&startError]) {
        duration->finish();
        _captureDuration.reset();
        [input removeTapOnBus:0];
        _audioEngine = nil;
        if (error) *error = startError;
        return NO;
    }

    _captureEngineStartedNs = MonotonicUptimeNs();
    if (_lastAudioBufferNs) _lastAudioBufferNs->store(0, std::memory_order_release);

    const NSUInteger generation = _captureSessionGeneration;
    _audioConfigObserver = [NSNotificationCenter.defaultCenter
        addObserverForName:AVAudioEngineConfigurationChangeNotification
                    object:engine
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification *notification) {
                    (void)notification;
                    MSIMEVoiceInputService *service = weakSelf;
                    if (!service || !service->_captureSessionOpen || service->_captureSessionGeneration != generation) return;
                    [service scheduleDebouncedCaptureRecoveryForGeneration:generation];
                }];

    if ([engine isKindOfClass:AVAudioEngine.class]) {
        [self startCaptureResilienceMonitorsForRealEngine];
    }

    if (usedFallbackOut) *usedFallbackOut = usedFallback;
    if (resolvedNameOut) *resolvedNameOut = resolvedName;
    (void)isRecovery;
    return YES;
}

- (BOOL)recoverAudioCaptureForConfigurationChange {
    if (!_captureSessionOpen) return NO;
    const NSUInteger generation = _captureSessionGeneration;
    if (_sessionMigrationCount >= kMaxSessionMigrations) {
        NSError *limitError = [NSError errorWithDomain:@"app.msime.client.voice" code:8
            userInfo:@{NSLocalizedDescriptionKey: @"音频设备频繁断开，无法继续切换"}];
        [self stopCaptureResilienceMonitors];
        [self teardownAudioEngineInstanceOnly];
        if (self.captureDeviceLostHandler) self.captureDeviceLostHandler(limitError);
        return NO;
    }

    // 1. 拆除旧引擎（同步冻结已录时长）并冲刷现有重采样器，为新设备的采样率/声道数无缝衔接做好准备。
    [self teardownAudioEngineInstanceOnly];
    if (_tapMutex) {
        std::lock_guard<std::mutex> dspLock(*_tapMutex);
        if (_pcmRecording) [_pcmRecording transitionInputFormatWithError:nil];
        if (_tapStandalonePCM) [_tapStandalonePCM transitionInputFormatWithError:nil];
    }

    // 2. 重建引擎并自动回退至存活麦克风（若首选外设已拔出或断开，立即切到系统默认麦克风）。
    BOOL usedFallback = NO;
    NSString *resolvedName = nil;
    NSError *reconfigError = nil;
    if ([self attachAndStartAudioEngineWithDeviceUID:_requestedDeviceUID
                                          isRecovery:YES
                                        usedFallback:&usedFallback
                                        resolvedName:&resolvedName
                                               error:&reconfigError]) {
        _recoveryRetryCount = 0;
        ++_sessionMigrationCount;
        if (self.captureRouteDidChangeHandler) {
            self.captureRouteDidChangeHandler(resolvedName.length ? resolvedName : @"系统默认麦克风", YES);
        }
        return YES;
    }

    // 3. 蓝牙/CoreAudio 聚合设备断开后常有 200~800 ms 的重建窗口：退避重试而不急于宣判失败。
    if (_recoveryRetryCount < kMaxEngineRecoveryRetries) {
        ++_recoveryRetryCount;
        const NSTimeInterval delay = 0.22 * static_cast<NSTimeInterval>(_recoveryRetryCount);
        _recoveryScheduled = YES;
        __weak MSIMEVoiceInputService *weakSelf = self;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(delay * NSEC_PER_SEC)),
                       dispatch_get_main_queue(), ^{
            MSIMEVoiceInputService *service = weakSelf;
            if (!service || !service->_captureSessionOpen || service->_captureSessionGeneration != generation) return;
            service->_recoveryScheduled = NO;
            [service recoverAudioCaptureForConfigurationChange];
        });
        return NO;
    }

    // 4. 所有重试耗尽（确实已无任何可用输入设备）：停止看门狗并回调宿主抢救已录音频，绝不静默丢弃已说内容。
    [self stopCaptureResilienceMonitors];
    NSError *lostError = reconfigError ?: [NSError errorWithDomain:@"app.msime.client.voice" code:9
        userInfo:@{NSLocalizedDescriptionKey: @"麦克风已断开且没有可用的替代设备"}];
    if (self.captureDeviceLostHandler) self.captureDeviceLostHandler(lostError);
    return NO;
}

- (BOOL)startMicrophoneCapture:(MSIMEVoiceAudioBuffer)bufferHandler deviceUID:(NSString *)deviceUID error:(NSError **)error {
    if ([self microphoneAuthorizationStatus] != AVAuthorizationStatusAuthorized) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:1
            userInfo:@{NSLocalizedDescriptionKey: @"麦克风权限未授权"}];
        return NO;
    }
    if (_audioEngine || _captureSessionOpen) return YES;

    ++_captureSessionGeneration;
    _captureSessionOpen = YES;
    _accumulatedDurationSeconds = 0;
    _recordedDuration = 0;
    _captureDuration.reset();
    _hasPreprocessStats = NO;
    _lastHasLikelySpeech = YES;
    _lastActiveSpeechMs = 0;
    _lastPeakAbs = 0.0f;
    _activeBufferHandler = [bufferHandler copy];
    _requestedDeviceUID = [deviceUID copy];
    _activeCaptureDeviceName = nil;
    _recoveryScheduled = NO;
    _recoveryRetryCount = 0;
    _sessionMigrationCount = 0;
    _lastAudioBufferNs = std::make_shared<std::atomic<uint64_t>>(0);

    // 每次开麦都存一份 16 kHz 单声道音频，与交给哪个识别服务无关；成功、失败、取消、中断都留着。
    id<MSIMEVoiceArchiving> archive = MSIMEVoiceInputArchive;
    NSString *archiveID = [archive beginRecordingForApp:_archiveApp provider:_archiveProvider];
    if (archiveID) { _archiveRecordingID = archiveID; _archiving = YES; }

    _audioPreprocessor = std::make_shared<msime::voice::VoiceAudioPreprocessor>();
    _tapMutex = std::make_shared<std::mutex>();
    _tapStandalonePCM = (_pcmCaptureMode == PCMCaptureMode::None)
        ? [[MSIMEVoicePCMBuffer alloc] initWithSampleLimit:NSUIntegerMax]
        : nil;
    if (_pcmCaptureMode != PCMCaptureMode::None) {
        _pcmTapHandled = YES;
        if (_pendingTapHandled) _pendingTapHandled->store(true, std::memory_order_release);
    }

    BOOL usedFallback = NO;
    NSString *resolvedName = nil;
    if (![self attachAndStartAudioEngineWithDeviceUID:deviceUID
                                           isRecovery:NO
                                         usedFallback:&usedFallback
                                         resolvedName:&resolvedName
                                                error:error]) {
        _captureSessionOpen = NO;
        ++_captureSessionGeneration;
        [self stopCaptureResilienceMonitors];
        [self teardownAudioEngineInstanceOnly];
        _tapStandalonePCM = nil;
        _audioPreprocessor.reset();
        _tapMutex.reset();
        _activeBufferHandler = nil;
        _requestedDeviceUID = nil;
        _lastAudioBufferNs.reset();
        _pcmTapHandled = NO;
        [self endArchiveDiscarding:YES];
        return NO;
    }

    if (usedFallback && self.captureRouteDidChangeHandler) {
        NSString *name = resolvedName.length ? [resolvedName copy] : @"系统默认麦克风";
        void (^routeHandler)(NSString *, BOOL) = [self.captureRouteDidChangeHandler copy];
        dispatch_async(dispatch_get_main_queue(), ^{
            routeHandler(name, NO);
        });
    }
    return YES;
}
- (BOOL)shouldDiscardArchiveForCurrentCapture {
    // 过短误触（< 0.25 s）或极端数字静音（< 2.5 s、无活跃帧且峰值 < -54 dBFS）才自动丢弃存档；
    // 80–110 ms 的单音节短词（如“好/对/行”）即使低于 120 ms 默认阈值也保留存档，避免误删。
    if (msime::voice::short_capture(_recordedDuration)) return YES;
    constexpr float kDigitalSilencePeakThreshold = 0.002f; // 约 -54 dBFS
    return _hasPreprocessStats && _recordedDuration < 2.5 &&
           _lastActiveSpeechMs == 0 && _lastPeakAbs < kDigitalSilencePeakThreshold;
}
- (void)endArchiveDiscarding:(BOOL)discard {
    if (!_archiving) return;
    _archiving = NO;
    [MSIMEVoiceInputArchive endCaptureDiscarding:discard];
}
- (AVAudioEngine *)makeAudioEngine { return [AVAudioEngine new]; }
- (void)stopAudioEngineCapture {
    ++_captureSessionGeneration;
    _captureSessionOpen = NO;
    _recoveryScheduled = NO;
    [self stopCaptureResilienceMonitors];
    [self teardownAudioEngineInstanceOnly];
    _recordedDuration = _accumulatedDurationSeconds;
    _accumulatedDurationSeconds = 0;
    _activeBufferHandler = nil;
    _requestedDeviceUID = nil;
    _activeCaptureDeviceUID = nil;
    _lastAudioBufferNs.reset();
    [_speechRequest endAudio];
    [_analyzer finishAudio];
}
- (void)stopMicrophoneCapture {
    [self stopAudioEngineCapture];
    if (_audioPreprocessor && _tapMutex) {
        std::lock_guard<std::mutex> lock(*_tapMutex);
        if (_tapStandalonePCM) {
            NSData *rawTail = [_tapStandalonePCM finishWithError:nil];
            if (rawTail.length) {
                NSData *processedTail = PreprocessPCMData(rawTail, _audioPreprocessor.get());
                if (_archiving) [MSIMEVoiceInputArchive appendPCM:processedTail];
            }
            _tapStandalonePCM = nil;
        }
        _hasPreprocessStats = YES;
        _lastHasLikelySpeech = _audioPreprocessor->hasLikelySpeech() ? YES : NO;
        _lastActiveSpeechMs = _audioPreprocessor->activeSpeechMs();
        _lastPeakAbs = _audioPreprocessor->peakAbs();
    }
    _audioPreprocessor.reset();
    _tapMutex.reset();
    _pcmTapHandled = NO;
    // 不到 1/4 秒的误触或极端数字静音不留存档，短单字录音继续保留。
    [self endArchiveDiscarding:[self shouldDiscardArchiveForCurrentCapture]];
}
- (BOOL)startTranscriptionWithLanguage:(NSString *)language textHandler:(void (^)(NSString *, BOOL))handler error:(NSError **)error {
    if ([SFSpeechRecognizer authorizationStatus] != SFSpeechRecognizerAuthorizationStatusAuthorized) { if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:2 userInfo:@{NSLocalizedDescriptionKey: @"语音识别权限未授权"}]; return NO; }
    [self stopTranscription];
    __weak MSIMEVoiceInputService *weakSelf = self;
    const uint64_t generation = _transcriptionGeneration;
    // macOS 26 and later: SpeechAnalyzer, fully on device, once the locale's model is installed. Its handler already runs on main. SpeechTranscriber can leave a space before Chinese punctuation ("调整 ，"), so its text goes through the same transcript rules as the local models'.
    _analyzer = MSIMEStartBackendSpeechAnalyzer(language ?: @"zh-CN", ^(NSString *text, BOOL final) {
        MSIMEVoiceInputService *service = weakSelf;
        if (!service || service->_transcriptionGeneration != generation) return;
        handler(@(msime::voice::tidy_local_transcript(text.UTF8String ?: "").c_str()), final);
        if (final && service->_transcriptionGeneration == generation) [service stopTranscription];
    });
    if (_analyzer) return YES;
    _recognizer = [[SFSpeechRecognizer alloc] initWithLocale:[[NSLocale alloc] initWithLocaleIdentifier:language ?: @"zh-CN"]];
    _speechRequest = [[SFSpeechAudioBufferRecognitionRequest alloc] init];
    // Keep the audio on this Mac whenever the locale has an on-device model; only a locale without one still goes to Apple's servers.
    if (_recognizer.supportsOnDeviceRecognition) _speechRequest.requiresOnDeviceRecognition = YES;
    _speechTask = [_recognizer recognitionTaskWithRequest:_speechRequest resultHandler:^(SFSpeechRecognitionResult *result, NSError *recognitionError) {
        // Speech can deliver a queued result after cancellation. Serialize with
        // host lifecycle operations and never let an old task affect its successor.
        dispatch_async(dispatch_get_main_queue(), ^{
            MSIMEVoiceInputService *service = weakSelf;
            if (!service || service->_transcriptionGeneration != generation) return;
            if (result) handler(result.bestTranscription.formattedString, result.isFinal);
            // Empty final is a terminal failure signal, never a partial commit.
            if (recognitionError && !result.isFinal && service->_transcriptionGeneration == generation)
                handler(@"", YES);
            // The handler may itself stop or replace the recognition task.
            if (service->_transcriptionGeneration == generation && (recognitionError || result.isFinal))
                [service stopTranscription];
        });
    }];
    return _speechTask != nil;
}
- (void)stopTranscription { ++_transcriptionGeneration; [_speechTask cancel]; _speechTask = nil; _speechRequest = nil; _recognizer = nil; [_analyzer cancel]; _analyzer = nil; }
- (BOOL)startWithSession:(MSIMEClientSession *)session generation:(uint64_t *)generation error:(NSError **)error { if (_active) return YES; _archiveRecordingID = nil; NSDictionary *result = [session startVoiceWithError:error]; if (!result) return NO; uint64_t admittedGeneration = 0; if (!MSIMEVoiceStrictGeneration(result[@"generation"], &admittedGeneration)) return NO; _session = session; _active = YES; if (generation) *generation = admittedGeneration; return YES; }
- (BOOL)cancelWithError:(NSError **)error { [self stopPCMStreamDelivery]; [_pcmRecording cancel]; _pcmRecording = nil; [self clearPCMPipelineState]; if (!_active) { [self stopMicrophoneCapture]; [self stopTranscription]; return YES; } BOOL ok = [_session cancelVoiceWithError:error]; [self stopMicrophoneCapture]; [self stopTranscription]; _active = NO; _session = nil; return ok; }
- (void)applyText:(NSString *)text generation:(uint64_t)generation completion:(MSIMEVoiceInputResult)completion {
    MSIMEClientSession *session = _session;
    dispatch_async(dispatch_get_main_queue(), ^{
        NSError *error = nil;
        NSDictionary *result = [session applyVoiceText:text generation:generation error:&error];
        completion(result, error);
    });
}
- (void)dealloc { [self stopPCMStreamDelivery]; [_pcmRecording cancel]; [self clearPCMPipelineState]; [self stopMicrophoneCapture]; [self stopTranscription]; }
@end
