#import "../../src/voice/LocalVoiceRequest.h"
#import "../../src/voice/TextStreamSupport.h"
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

// 用 MSIMELocalVoiceRequest 的 TextStream 通道对着 shurufa-textstream 说协议。
//   normal       脚本化替身（tests/voice/textstream_launcher_fixture.py）：start/audio/finish/final、cancel、热词、没装好时的拒绝。
//   unavailable  替身报告 hello.available=false：请求以指向“重新安装语音模型”的中文错误结束。
//   real <wav>   真启动器（MSIME_TEXTSTREAM_LAUNCHER）加已安装的运行环境，流式识别一段 16 kHz 单声道 16 位录音；没装好时以 77 退出（CTest 的跳过码）。

namespace {
BOOL Spin(BOOL (^done)(void), NSTimeInterval seconds) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (!done() && deadline.timeIntervalSinceNow > 0)
        [NSRunLoop.currentRunLoop runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    return done();
}

struct Outcome {
    NSMutableArray<NSString *> *partials = [NSMutableArray array];
    NSString *final = nil;
    NSError *error = nil;
};

MSIMEDoubaoResult Recorder(Outcome *outcome) {
    return ^(NSString *text, BOOL final, NSError *error) {
        assert([NSThread isMainThread]);
        if (error) { assert(!outcome->error && !outcome->final); outcome->error = error; return; }
        if (final) { assert(!outcome->final); outcome->final = text; return; }
        [outcome->partials addObject:text];
    };
}

NSData *Samples(NSUInteger count, float value) {
    std::vector<float> samples(count, value);
    return [NSData dataWithBytes:samples.data() length:samples.size() * sizeof(float)];
}

NSString *MakeRoot(BOOL ready) {
    NSString *root = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
    NSString *model = [root stringByAppendingPathComponent:@"model"];
    assert([NSFileManager.defaultManager createDirectoryAtPath:model withIntermediateDirectories:YES attributes:nil error:nil]);
    if (ready) {
        for (NSString *file in @[@"config.json", @"model.safetensors"])
            assert([@"x" writeToFile:[model stringByAppendingPathComponent:file] atomically:YES encoding:NSUTF8StringEncoding error:nil]);
        assert([@"stamp" writeToFile:[root stringByAppendingPathComponent:@".ready"] atomically:YES encoding:NSUTF8StringEncoding error:nil]);
    }
    return root;
}

NSArray<NSDictionary *> *Logged(NSString *log) {
    NSString *contents = [NSString stringWithContentsOfFile:log encoding:NSUTF8StringEncoding error:nil] ?: @"";
    NSMutableArray *requests = [NSMutableArray array];
    for (NSString *line in [contents componentsSeparatedByString:@"\n"])
        if (line.length) [requests addObject:[NSJSONSerialization JSONObjectWithData:[line dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil]];
    return requests;
}

int RunReal(NSString *wav) {
    if (!MSIMETextStreamReady() || !MSIMETextStreamLauncherPath()) { std::printf("textstream runtime not installed, skipping\n"); return 77; }
    NSData *file = [NSData dataWithContentsOfFile:wav];
    assert(file.length > 44 && std::memcmp(file.bytes, "RIFF", 4) == 0);
    const int16_t *pcm = reinterpret_cast<const int16_t *>(static_cast<const uint8_t *>(file.bytes) + 44);
    const NSUInteger count = (file.length - 44) / 2;
    MSIMELocalVoiceRequest *request = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:@{@"language" : @"zh-cn"} hostOptions:nil error:nil];
    assert(request);
    Outcome outcome;
    Outcome *state = &outcome;
    assert([request startWithResult:Recorder(&outcome) error:nil]);
    for (NSUInteger offset = 0; offset < count; offset += 1600) {
        const NSUInteger length = MIN((NSUInteger)1600, count - offset);
        std::vector<float> samples(length);
        for (NSUInteger index = 0; index < length; ++index) samples[index] = pcm[offset + index] / 32768.0f;
        assert([request appendPCM:[NSData dataWithBytes:samples.data() length:length * sizeof(float)] error:nil]);
        Spin(^{ return NO; }, 0.1);
    }
    assert([request finishWithError:nil]);
    assert(Spin(^{ return (BOOL)(state->final || state->error); }, 120));
    assert(!outcome.error);
    std::printf("final after %lu partials: %s\n", static_cast<unsigned long>(outcome.partials.count), outcome.final.UTF8String);
    assert(outcome.final.length && outcome.partials.count);
    return 0;
}
} // namespace

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc == 3 && std::strcmp(argv[1], "real") == 0) return RunReal(@(argv[2]));
        const BOOL unavailable = argc == 2 && std::strcmp(argv[1], "unavailable") == 0;
        NSString *log = NSProcessInfo.processInfo.environment[@"MSIME_TS_FIXTURE_LOG"];
        assert(log.length && MSIMETextStreamLauncherPath());
        [NSFileManager.defaultManager removeItemAtPath:log error:nil];

        // 没装好（没有 .ready）时拒绝，并指向设置里的安装。
        setenv("SHURUFA_TEXTSTREAM_ROOT", MakeRoot(NO).UTF8String, 1);
        assert(!MSIMETextStreamReady());
        NSError *error = nil;
        assert(![[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:@{} hostOptions:nil error:&error] && error);
        assert([error.userInfo[NSLocalizedFailureReasonErrorKey] containsString:@"安装语音模型"]);

        NSString *root = MakeRoot(YES);
        setenv("SHURUFA_TEXTSTREAM_ROOT", root.UTF8String, 1);
        assert(MSIMETextStreamReady());
        assert([MSIMETextStreamModelDirectory() isEqual:[root stringByAppendingPathComponent:@"model"]]);
        // 缺了模型文件也算没装好。
        assert([NSFileManager.defaultManager removeItemAtPath:[MSIMETextStreamModelDirectory() stringByAppendingPathComponent:@"model.safetensors"] error:nil]);
        assert(!MSIMETextStreamReady());
        assert([@"x" writeToFile:[MSIMETextStreamModelDirectory() stringByAppendingPathComponent:@"model.safetensors"] atomically:YES encoding:NSUTF8StringEncoding error:nil]);
        assert(MSIMETextStreamReady());

        if (unavailable) {
            MSIMELocalVoiceRequest *request = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:@{@"language" : @"zh-cn"} hostOptions:nil error:nil];
            assert(request);
            Outcome outcome;
            Outcome *state = &outcome;
            assert([request startWithResult:Recorder(&outcome) error:nil]);
            assert(Spin(^{ return (BOOL)(state->error != nil); }, 20));
            NSString *detail = outcome.error.userInfo[NSLocalizedFailureReasonErrorKey];
            assert([detail containsString:@"重新安装语音模型"] && !outcome.final);
            return 0;
        }

        // start/audio/finish/final：partial 逐条回到主线程，final 只来一次；start 带的是 TextStream 的模型目录和语言，进程参数里有 --idle-exit。
        {
            MSIMELocalVoiceRequest *request = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:@{@"language" : @"zh-cn"} hostOptions:nil error:nil];
            assert(request);
            Outcome outcome;
            Outcome *state = &outcome;
            assert([request startWithResult:Recorder(&outcome) error:nil]);
            assert([request appendPCM:Samples(1600, 0.1f) error:nil]);
            assert(Spin(^{ return state->partials.count == 1; }, 20));
            assert([request appendPCM:Samples(1600, 0.1f) error:nil]);
            assert([request finishWithError:nil]);
            assert(Spin(^{ return state->final != nil; }, 20));
            assert([outcome.partials isEqual:(@[@"你好", @"你好世界"])] && [outcome.final isEqual:@"你好世界。"] && !outcome.error);
            assert(![request appendPCM:Samples(16, 0.0f) error:nil]);
        }
        NSArray<NSDictionary *> *requests = Logged(log);
        assert([requests[0][@"argv"] isEqual:(@[@"--idle-exit", @"120"])]);
        NSDictionary *start = requests[1];
        assert([start[@"op"] isEqual:@"start"] && [start[@"model"] isEqual:MSIMETextStreamModelDirectory()] && [start[@"language"] isEqual:@"zh-cn"]);
        assert(requests.count == 5 && [requests[4][@"op"] isEqual:@"finish"]);

        // cancel：只发一次 cancel，之后不再有回调；同一个进程接着服务下一次会话。
        {
            MSIMELocalVoiceRequest *request = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:@{} hostOptions:nil error:nil];
            Outcome outcome;
            Outcome *state = &outcome;
            assert([request startWithResult:Recorder(&outcome) error:nil]);
            assert([request appendPCM:Samples(1600, 0.1f) error:nil]);
            assert(Spin(^{ return state->partials.count == 1; }, 20));
            [request cancel];
            Spin(^{ return NO; }, 0.5);
            assert(!outcome.final && !outcome.error && outcome.partials.count == 1);
            MSIMELocalVoiceRequest *next = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:@{} hostOptions:nil error:nil];
            Outcome second;
            Outcome *secondState = &second;
            assert([next startWithResult:Recorder(&second) error:nil]);
            assert([next appendPCM:Samples(1600, 0.1f) error:nil] && [next finishWithError:nil]);
            assert(Spin(^{ return secondState->final != nil; }, 20));
            NSUInteger cancels = 0;
            for (NSDictionary *logged in Logged(log)) if ([logged[@"op"] isEqual:@"cancel"]) ++cancels;
            assert(cancels == 1);
        }
        std::printf("textstream request ok\n");
    }
    return 0;
}
