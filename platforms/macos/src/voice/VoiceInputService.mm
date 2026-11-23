#import "VoiceInputService.h"
#import "VoicePCMBuffer.h"
#import "VoiceCaptureDevice.h"
#import "../backend/voice/BackendSpeechAnalyzer.h"
#include "VoiceAudioPreprocess.h"
#include "../../../../shared/voice/CaptureDuration.h"
#include "../../../../shared/voice/LocalAsr.h"
#import <AVFoundation/AVFoundation.h>
#import <CoreAudio/CoreAudio.h>
#include <atomic>
#include <memory>
#include <mutex>

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
}
static id<MSIMEVoiceArchiving> MSIMEVoiceInputArchive;
+ (id<MSIMEVoiceArchiving>)archive { return MSIMEVoiceInputArchive; }
+ (void)setArchive:(id<MSIMEVoiceArchiving>)archive { MSIMEVoiceInputArchive = archive; }
- (NSString *)archiveRecordingID { return _archiveRecordingID; }
- (NSTimeInterval)recordedDuration { return _captureDuration ? _captureDuration->seconds() : _recordedDuration; }
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
    if (_pcmRecording || _audioEngine || _speechTask) {
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
    if (!handler || _pcmRecording || _audioEngine || _speechTask) {
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
- (BOOL)startMicrophoneCapture:(MSIMEVoiceAudioBuffer)bufferHandler deviceUID:(NSString *)deviceUID error:(NSError **)error {
    if ([self microphoneAuthorizationStatus] != AVAuthorizationStatusAuthorized) { if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:1 userInfo:@{NSLocalizedDescriptionKey: @"麦克风权限未授权"}]; return NO; }
    if (_audioEngine) return YES;
    _recordedDuration = 0;
    _captureDuration.reset();
    _hasPreprocessStats = NO;
    _lastHasLikelySpeech = YES;
    _lastActiveSpeechMs = 0;
    _lastPeakAbs = 0.0f;
    _audioEngine = [self makeAudioEngine];
    AVAudioInputNode *input = _audioEngine.inputNode;
    if (!MSIMEConfigureVoiceCaptureDevice(deviceUID, input.audioUnit, error)) {
        _audioEngine = nil;
        return NO;
    }
    // 每次开麦都存一份 16 kHz 单声道音频，与交给哪个识别服务无关；成功、失败、取消、中断都留着。
    id<MSIMEVoiceArchiving> archive = MSIMEVoiceInputArchive;
    NSString *archiveID = [archive beginRecordingForApp:_archiveApp provider:_archiveProvider];
    if (archiveID) { _archiveRecordingID = archiveID; _archiving = YES; }
    AVAudioFormat *format = [input inputFormatForBus:0];
    NSError *tapError = nil;
    SFSpeechAudioBufferRecognitionRequest *speechRequest = _speechRequest;
    id<MSIMEBackendSpeechAnalyzerSession> analyzer = _analyzer;
    auto duration = std::make_shared<msime::voice::CaptureDuration>(format.sampleRate);
    auto preprocessor = std::make_shared<msime::voice::VoiceAudioPreprocessor>();
    auto tapMutex = std::make_shared<std::mutex>();
    _captureDuration = duration;
    _audioPreprocessor = preprocessor;
    _tapMutex = tapMutex;

    const PCMCaptureMode mode = _pcmCaptureMode;
    MSIMEVoicePCMBuffer *sharedRecording = (mode != PCMCaptureMode::None) ? _pcmRecording : nil;
    MSIMEVoicePCMBuffer *standalonePCM = (mode == PCMCaptureMode::None)
        ? [[MSIMEVoicePCMBuffer alloc] initWithSampleLimit:NSUIntegerMax]
        : nil;
    _tapStandalonePCM = standalonePCM;
    auto streamLive = _pcmStreamLive;
    MSIMEVoicePCMChunk streamHandler = _pcmStreamHandler;
    NSMutableData *batchAccumulated = _pcmBatchAccumulated;
    void (^batchFailure)(NSError *) = _pcmBatchFailure;
    auto batchFailed = _pcmBatchFailed;
    if (mode != PCMCaptureMode::None) {
        _pcmTapHandled = YES;
        if (_pendingTapHandled) _pendingTapHandled->store(true, std::memory_order_release);
    }
    __weak MSIMEVoiceInputService *weakSelf = self;

    // 合并存档与识别为单次 16 kHz 单声道 f32 转换，经 VoiceAudioPreprocessor 处理后同时供给存档与识别。
    AVAudioNodeTapBlock capture = ^(AVAudioPCMBuffer *buffer, AVAudioTime *time) {
        (void)time;
        if (!duration->append(buffer.frameLength)) return;
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
    // AVAudioInputNode exposes the tap API without an NSError parameter on
    // the macOS SDKs supported by this host. Keep the deprecated warning
    // local to this call rather than selecting a non-existent overload.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    [input installTapOnBus:0 bufferSize:1024 format:format block:capture];
#pragma clang diagnostic pop
    if (tapError) {
        duration->finish();
        _captureDuration.reset();
        _audioEngine = nil;
        _tapStandalonePCM = nil;
        _audioPreprocessor.reset();
        _tapMutex.reset();
        _pcmTapHandled = NO;
        [self endArchiveDiscarding:YES];
        if (error) *error = tapError;
        return NO;
    }
    NSError *startError = nil;
    if (![_audioEngine startAndReturnError:&startError]) {
        duration->finish();
        _captureDuration.reset();
        [input removeTapOnBus:0];
        _audioEngine = nil;
        _tapStandalonePCM = nil;
        _audioPreprocessor.reset();
        _tapMutex.reset();
        _pcmTapHandled = NO;
        [self endArchiveDiscarding:YES];
        if (error) *error = startError;
        return NO;
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
    if (_captureDuration) { _recordedDuration = _captureDuration->finish(); _captureDuration.reset(); }
    if (!_audioEngine) return;
    [_audioEngine.inputNode removeTapOnBus:0];
    [_audioEngine stop];
    _audioEngine = nil;
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
