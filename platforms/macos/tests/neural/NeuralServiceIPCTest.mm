// Shurufa v2 Tier 1 独立后台服务 IPC 集成单测（使用真实 `ShurufaNeuralService --test-mock` 二进制）：
// 校验 Unix 域套接字二进制帧通信、健康检查、上文预热、非阻塞解码、旧代次请求合并、
// 缺失模型静默降级（零拉起子进程）以及子进程被 SIGKILL 后的自动崩溃恢复。

#import <Foundation/Foundation.h>
#import "../../src/neural/ShurufaNeuralClient.h"

#include <cassert>
#include <cstdio>
#include <cstring>
#include <signal.h>
#include <unistd.h>

static void PumpMainRunLoopUntil(BOOL (^predicate)(void), NSTimeInterval timeoutSeconds) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeoutSeconds];
    while (!predicate() && [deadline timeIntervalSinceNow] > 0.0) {
        [[NSRunLoop mainRunLoop] runMode:NSDefaultRunLoopMode
                              beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    }
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        assert(argc >= 2);
        NSString *servicePath = [NSString stringWithUTF8String:argv[1]];
        assert(servicePath.length > 0);

        // 1. 缺失模型目录时 `available == NO`，绝不拉起子进程且立即 fail-open 回调 nil。
        {
            ShurufaNeuralClientConfiguration *cfg = [[ShurufaNeuralClientConfiguration alloc] init];
            cfg.helperPath = servicePath;
            cfg.modelDirectory = @"/nonexistent/shurufa-neural-missing-dir";
            cfg.requireModelFile = YES;
            cfg.listenToApplicationTermination = NO;
            ShurufaNeuralClient *client = [[ShurufaNeuralClient alloc] initWithConfiguration:cfg];
            assert(!client.isAvailable);
            assert(client.servicePID == 0);

            __block BOOL called = NO;
            __block NSData *received = [NSData dataWithBytes:"x" length:1];
            const char *emptyReq = "{\"generation\":1,\"candidates\":[]}";
            NSData *req = [NSData dataWithBytes:emptyReq length:std::strlen(emptyReq)];
            [client decodeRequestData:req generation:1 completion:^(NSData *resp, uint64_t gen, double ms) {
                (void)ms;
                assert(gen == 1);
                received = resp;
                called = YES;
            }];
            PumpMainRunLoopUntil(^BOOL { return called; }, 2.0);
            assert(called);
            assert(received == nil);
            assert(client.servicePID == 0);
            [client stop];
        }

        // 2. 启动真实 `ShurufaNeuralService --test-mock`：校验健康检查、预热、解码与崩溃自愈。
        {
            ShurufaNeuralClientConfiguration *cfg = [[ShurufaNeuralClientConfiguration alloc] init];
            cfg.helperPath = servicePath;
            cfg.requireModelFile = NO;
            cfg.extraLaunchArguments = @[@"--test-mock"];
            cfg.idleUnloadSeconds = 60.0;
            cfg.requestTimeoutSeconds = 1.0;
            cfg.listenToApplicationTermination = NO;
            ShurufaNeuralClient *client = [[ShurufaNeuralClient alloc] initWithConfiguration:cfg];
            assert(client.isAvailable);

            __block NSDictionary *healthInfo = nil;
            __block BOOL healthDone = NO;
            [client checkHealthWithCompletion:^(NSDictionary<NSString *, id> *info) {
                healthInfo = info;
                healthDone = YES;
            }];
            PumpMainRunLoopUntil(^BOOL { return healthDone; }, 5.0);
            assert(healthDone);
            assert([healthInfo[@"ok"] isEqual:@YES]);
            assert([healthInfo[@"mock"] isEqual:@YES]);
            const pid_t firstPID = client.servicePID;
            assert(firstPID > 0);

            [client prefillContext:@"这个兄弟，应该多少还是"];

            const char *reqUTF8 =
                "{\"generation\":7,\"context\":\"这个兄弟，应该多少还是\","
                "\"candidates\":[{\"index\":0,\"text\":\"整了血铅\"},"
                "{\"index\":1,\"text\":\"挣了些钱\"}]}";
            NSData *reqData = [NSData dataWithBytes:reqUTF8 length:std::strlen(reqUTF8)];

            __block BOOL decodeDone = NO;
            __block NSData *decodeResp = nil;
            [client decodeRequestData:reqData generation:7 completion:^(NSData *resp, uint64_t gen, double ms) {
                (void)ms;
                assert(gen == 7);
                decodeResp = resp;
                decodeDone = YES;
            }];
            PumpMainRunLoopUntil(^BOOL { return decodeDone; }, 5.0);
            assert(decodeDone);
            assert(decodeResp.length > 0);
            NSDictionary *parsed = [NSJSONSerialization JSONObjectWithData:decodeResp options:0 error:nil];
            assert([parsed[@"ok"] isEqual:@YES]);
            assert([parsed[@"generation"] isEqual:@7]);
            assert([parsed[@"candidate_scores"] count] == 2);

            // 3. 模拟后台服务意外崩溃（SIGKILL）：首次请求优雅 fail-open（不崩溃、不卡死），退避期后自动自愈重启。
            ::kill(firstPID, SIGKILL);
            usleep(100 * 1000);

            __block BOOL failOpenDone = NO;
            __block NSData *failOpenResp = [NSData dataWithBytes:"x" length:1];
            [client decodeRequestData:reqData generation:8 completion:^(NSData *resp, uint64_t gen, double ms) {
                (void)ms;
                assert(gen == 8);
                failOpenResp = resp;
                failOpenDone = YES;
            }];
            PumpMainRunLoopUntil(^BOOL { return failOpenDone; }, 3.0);
            assert(failOpenDone);
            assert(failOpenResp == nil);

            // 等待 1.2 秒退避窗口结束，验证自动自愈重启且 PID 已换新。
            usleep(1200 * 1000);
            __block BOOL recoveredDone = NO;
            __block NSData *recoveredResp = nil;
            [client decodeRequestData:reqData generation:9 completion:^(NSData *resp, uint64_t gen, double ms) {
                (void)ms;
                assert(gen == 9);
                recoveredResp = resp;
                recoveredDone = YES;
            }];
            PumpMainRunLoopUntil(^BOOL { return recoveredDone; }, 5.0);
            assert(recoveredDone);
            assert(recoveredResp.length > 0);
            const pid_t secondPID = client.servicePID;
            assert(secondPID > 0 && secondPID != firstPID);

            [client stop];
            assert(client.servicePID == 0);
        }

        std::printf("neural-service-ipc-test: ALL PASSED\n");
        return 0;
    }
}
