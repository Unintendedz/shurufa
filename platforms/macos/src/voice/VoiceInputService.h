#pragma once
#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
#import <Speech/Speech.h>
#import "../../../../shared/apple/MSIMEClientSession.h"
NS_ASSUME_NONNULL_BEGIN
typedef void (^MSIMEVoiceInputResult)(NSDictionary * _Nullable, NSError * _Nullable);
typedef void (^MSIMEVoiceAudioBuffer)(AVAudioPCMBuffer *);
typedef void (^MSIMEVoicePCMChunk)(NSData * _Nullable pcm, NSError * _Nullable error);
// 语音存档的接口。输入法进程启动时把 MSIMEVoiceArchive 交给 MSIMEVoiceInputService.archive；没有设置时（单元测试）不存档。
@protocol MSIMEVoiceArchiving <NSObject>
- (nullable NSString *)beginRecordingForApp:(nullable NSString *)app provider:(nullable NSString *)provider;
// 16 kHz 单声道 f32 PCM，可在采集回调里调用。
- (void)appendPCM:(NSData *)pcm;
- (void)endCaptureDiscarding:(BOOL)discard;
- (void)noteText:(NSString *)text forRecording:(NSString *)identifier;
- (void)noteFailure:(NSString *)reason forRecording:(NSString *)identifier;
- (void)noteCancelledRecording:(NSString *)identifier;
@end
@interface MSIMEVoiceInputService : NSObject
@property(class, nonatomic, strong, nullable) id<MSIMEVoiceArchiving> archive;
- (BOOL)startWithSession:(MSIMEClientSession *)session generation:(uint64_t *)generation error:(NSError **)error;
- (BOOL)cancelWithError:(NSError **)error;
- (BOOL)startMicrophoneCapture:(MSIMEVoiceAudioBuffer)handler deviceUID:(NSString * _Nullable)deviceUID error:(NSError **)error;
- (void)stopMicrophoneCapture;
// Main-thread lifecycle; raw buffers are converted on the capture callback.
// Finish stops capture and returns one immutable recording; cancel discards it.
- (BOOL)startPCMRecording:(MSIMEVoiceAudioBuffer)handler deviceUID:(NSString * _Nullable)deviceUID error:(NSError **)error;
// Conversion failure cancels capture and delivers failure once on main, only
// while this recording still owns the service. No audio is included in errors.
- (BOOL)startPCMRecording:(MSIMEVoiceAudioBuffer)handler deviceUID:(NSString * _Nullable)deviceUID failure:(void (^ _Nullable)(NSError *))failure error:(NSError **)error;
- (NSData * _Nullable)finishPCMRecordingWithError:(NSError **)error;
// Main-thread lifecycle. Handler runs on the capture callback, must return
// promptly and must not stop/destroy capture; dispatch host cancellation to main.
// Each chunk is immutable mono 16 kHz float PCM. An error is delivered once.
// Finish stops capture and returns only the not-yet-delivered converter tail.
- (BOOL)startPCMStreaming:(MSIMEVoicePCMChunk)handler deviceUID:(NSString * _Nullable)deviceUID error:(NSError **)error;
- (NSData * _Nullable)finishPCMStreamingWithError:(NSError **)error;
- (BOOL)startTranscriptionWithLanguage:(NSString *)language textHandler:(void (^)(NSString *, BOOL))handler error:(NSError **)error;
- (void)stopTranscription;
- (AVAuthorizationStatus)microphoneAuthorizationStatus;
- (void)requestMicrophonePermission:(void (^)(BOOL))completion;
- (SFSpeechRecognizerAuthorizationStatus)speechAuthorizationStatus;
- (void)requestSpeechPermission:(void (^)(BOOL))completion;
- (void)applyText:(NSString *)text generation:(uint64_t)generation completion:(MSIMEVoiceInputResult)completion;
@property(nonatomic, readonly, getter=isActive) BOOL active;
// 语音存档：下一次开麦时记下的前台应用和识别服务，由宿主在开麦前设置。
@property(nonatomic, copy, nullable) NSString *archiveApp;
@property(nonatomic, copy, nullable) NSString *archiveProvider;
// 本次激活（startWithSession 之后）实际录下的那条存档的 id；还没开麦或存档不可用时为 nil。识别结果和失败按它记。
@property(nonatomic, readonly, nullable) NSString *archiveRecordingID;
// Actual admitted capture frames; frozen at stop and reset for each new tap.
@property(nonatomic, readonly) NSTimeInterval recordedDuration;
@end
NS_ASSUME_NONNULL_END
