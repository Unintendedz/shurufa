#pragma once
#import <Foundation/Foundation.h>

// One batch recognition request over HTTPS: a frozen configuration and a cancellation token, taking the whole recording at once and delivering on the main queue; cancellation suppresses delivery. No audio capture. On-device models never come here; they stream through the msime-voice-local helper (LocalVoiceRequest.h).
@interface MSIMEHTTPVoiceRequest : NSObject
// Optional main-queue phase notification, snapshotted at request start.
@property(copy) void (^polishingHandler)(void);
// 主队列完成回调前更新；整理失败仍返回原始转写，取消不交付结果。
@property(nonatomic, readonly) BOOL polishFailed;
- (instancetype)init NS_UNAVAILABLE;
+ (instancetype)new NS_UNAVAILABLE;
- (instancetype)initWithOptions:(NSDictionary *)options error:(NSError **)error;
// Text-only optional polishing; no ASR provider or audio credentials required.
- (instancetype)initWithPolishOptions:(NSDictionary *)options error:(NSError **)error;
- (BOOL)polishText:(NSString *)text completion:(void (^)(NSString *, NSError *))completion error:(NSError **)error;
// The most 16 kHz samples recognizePCM: submits: the 20 MiB batch upload budget MSIME-Windows uses. The host ends the recording once it has captured this much, so nothing the user says after that point is silently left out.
@property(nonatomic, readonly) NSUInteger sampleLimit;
- (BOOL)recognizePCM:(NSData *)pcm completion:(void (^)(NSString *, NSError *))completion error:(NSError **)error;
- (void)cancel;
@end
