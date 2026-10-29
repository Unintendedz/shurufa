#import "../../src/voice/HTTPVoiceRequest.h"
#include <cassert>
#import <objc/runtime.h>

@interface PolishEditionFixture : NSObject
@end
@implementation PolishEditionFixture
- (NSDictionary *)infoDictionary { return @{@"MSIMEEdition":@"shurufa"}; }
@end
static id PolishShurufaBundle(id, SEL) { return [PolishEditionFixture new]; }
// Long enough for the deliberately slow fixture responses; the loop leaves as soon as the work finishes,
// so raising the ceiling costs the passing cases nothing.
static void Wait(BOOL (^done)(void)) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:20];
    while (!done() && deadline.timeIntervalSinceNow > 0)
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.001]];
    assert(done());
}
int main(int argc, char **argv) {
    @autoreleasepool {
        assert(argc == 2);
        NSString *base = @(argv[1]);
        NSMutableString *model = [@"fixture-model" mutableCopy];
        NSMutableDictionary *options = [@{@"polish_enabled": @YES, @"polish_provider": @"openai",
            @"polish_endpoint": [base stringByAppendingString:@"/polish"], @"polish_model": model,
            @"polish_token": @"fixture-token", @"polish_prompt_id": @"custom_2", @"polish_prompt_custom_2": @"synthetic prompt"} mutableCopy];
        MSIMEHTTPVoiceRequest *request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        assert(request);
        [model setString:@"mutated"];
        NSMutableString *input = [@"synthetic transcript" mutableCopy];
        __block BOOL done = NO;
        assert([request polishText:input completion:^(NSString *text, NSError *error) {
            assert(NSThread.isMainThread && !error && !request.polishFailed && [text isEqual:@"synthetic polished"]); done = YES;
        } error:nil]);
        [input setString:@"mutated input"];
        Wait(^BOOL { return done; });
        assert(![request polishText:@"second" completion:^(NSString *, NSError *) {} error:nil]);
        options[@"polish_model"] = @"fixture-model";
        // 非 Shurufa 继续原样使用完整自定义地址，API 根目录的 301 不跟随。
        options[@"polish_endpoint"] = [base stringByAppendingString:@"/v1"];
        request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        done = NO;
        assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
            assert(!error && request.polishFailed && [text isEqual:@"synthetic transcript"]); done = YES;
        } error:nil]);
        Wait(^BOOL { return done; });
        Method mainBundle = class_getClassMethod(NSBundle.class, @selector(mainBundle));
        IMP originalBundle = method_setImplementation(mainBundle, (IMP)PolishShurufaBundle);
        // Shurufa API 前缀补全，完整对话和自定义路由均保持可用。
        options[@"polish_provider"] = @"deepseek";
        options[@"polish_thinking"] = @"disabled";
        for (NSString *path in @[@"/v1", @"/v1/", @"/v1/chat/completions", @"/v1/chat/completions/",
                                 @"/prefix/v1?tenant=synthetic", @"", @"/polish", @"/v1/clean/", @"/api/v3", @"/v1beta/openai"]) {
            options[@"polish_endpoint"] = [base stringByAppendingString:path];
            request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
            done = NO;
            assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
                assert(!error && !request.polishFailed && [text isEqual:@"synthetic polished"]); done = YES;
            } error:nil]);
            Wait(^BOOL { return done; });
        }
        method_setImplementation(mainBundle, originalBundle);
        // 整理明确失败仍返回原文，同时设置只读状态供最终投递提示使用。
        for (NSString *path in @[@"/failure", @"/empty", @"/length", @"/content_filter", @"/error"]) {
            options[@"polish_endpoint"] = [base stringByAppendingString:path];
            request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
            done = NO;
            assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
                assert(!error && request.polishFailed && [text isEqual:@"synthetic transcript"]); done = YES;
            } error:nil]);
            Wait(^BOOL { return done; });
        }
        // Six seconds is slow for a chat completion and well inside the budget this host asks for, so the
        // answer is waited for and used. It used to be abandoned at three, which is under what cleaning a
        // minute of transcript takes - the request went out and its reply was thrown away every time.
        for (NSString *path in @[@"/stall-headers", @"/stall-body"]) {
            options[@"polish_endpoint"] = [base stringByAppendingString:path];
            request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
            done = NO;
            const NSTimeInterval started = NSProcessInfo.processInfo.systemUptime;
            assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
                assert(!error && !request.polishFailed && [text isEqual:@"synthetic polished"]); done = YES;
            } error:nil]);
            Wait(^BOOL { return done; });
            const NSTimeInterval elapsed = NSProcessInfo.processInfo.systemUptime - started;
            assert(elapsed >= 5.5);
        }
        // 超时返回原文与失败状态，不丢识别文字。
        options[@"polish_endpoint"] = [base stringByAppendingString:@"/stall-headers"];
        options[@"polish_timeout_ms"] = @1000;
        request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        done = NO;
        const NSTimeInterval timeoutStarted = NSProcessInfo.processInfo.systemUptime;
        assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
            assert(!error && request.polishFailed && [text isEqual:@"synthetic transcript"]); done = YES;
        } error:nil]);
        Wait(^BOOL { return done; });
        assert(NSProcessInfo.processInfo.systemUptime - timeoutStarted < 4.0);
        [options removeObjectForKey:@"polish_timeout_ms"];
        options[@"polish_endpoint"] = @"https://example.invalid/v1#invalid";
        request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        done = NO;
        assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
            assert(!error && request.polishFailed && [text isEqual:@"synthetic transcript"]); done = YES;
        } error:nil]);
        Wait(^BOOL { return done; });
        options[@"polish_enabled"] = @NO;
        request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        done = NO;
        assert([request polishText:@"synthetic transcript" completion:^(NSString *text, NSError *error) {
            assert(!error && !request.polishFailed && [text isEqual:@"synthetic transcript"]); done = YES;
        } error:nil]);
        Wait(^BOOL { return done; });
        request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        assert([request polishText:@"cancelled" completion:^(NSString *, NSError *) { assert(false); } error:nil]);
        [request cancel];
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
        request = [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil];
        assert(![request polishText:@"" completion:^(NSString *, NSError *) {} error:nil]);
        assert(![request polishText:[@"x" stringByPaddingToLength:65537 withString:@"x" startingAtIndex:0] completion:^(NSString *, NSError *) {} error:nil]);
        unichar invalid = 0xd800;
        options[@"polish_prompt_custom_1"] = [NSString stringWithCharacters:&invalid length:1];
        assert(![[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:options error:nil]);
    }
}
