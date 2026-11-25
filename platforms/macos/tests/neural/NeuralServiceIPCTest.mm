// Shurufa v2 Tier 1 纯 Rust UDS 神经服务客户端 + 两阶段状态机集成单测（使用真实 `ShurufaNeuralService --test-mock`）：
// 校验缺失模型静默降级（零子进程）、Unix 域套接字二进制帧往返、`updateHandler` + `pollNeural` 状态推进、
// 暖态快敲空格 `Condvar` 有界同步收口，以及子进程被 SIGKILL 后的 fail-open 与退避自愈重启。

#import <Foundation/Foundation.h>
#import "MSIMEClientSession.h"
#include "msime_client.h"

#include <cassert>
#include <cstdio>
#include <signal.h>
#include <sqlite3.h>
#include <unistd.h>

static void PumpMainRunLoopUntil(BOOL (^predicate)(void), NSTimeInterval timeoutSeconds) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeoutSeconds];
    while (!predicate() && [deadline timeIntervalSinceNow] > 0.0) {
        [[NSRunLoop mainRunLoop] runMode:NSDefaultRunLoopMode
                              beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    }
}

static MSIMEClientSession *CreateTestSession(NSString *rootDir) {
    NSMutableDictionary *options = [@{
        @"api_version": @1,
        @"edition": @"shurufa",
        @"preferences": @{
            @"scheme": @"quanpin",
            @"default_ime_mode": @"chinese",
            @"candidate_page_size": @6,
            @"learning": @NO,
            @"chinese_punctuation": @YES,
        },
    } mutableCopy];
    for (NSString *name in @[@"resources", @"user_data", @"cache", @"dictionaries"]) {
        NSString *path = [rootDir stringByAppendingPathComponent:name];
        assert([NSFileManager.defaultManager createDirectoryAtPath:path
                                       withIntermediateDirectories:YES
                                                        attributes:nil
                                                             error:nil]);
        options[name] = path;
    }
    sqlite3 *database = nullptr;
    NSString *dbPath = [options[@"dictionaries"] stringByAppendingPathComponent:@"msime-pinyin.db"];
    assert(sqlite3_open(dbPath.fileSystemRepresentation, &database) == SQLITE_OK);
    assert(sqlite3_exec(database,
                        "CREATE TABLE tbl_2_n(key TEXT,jp TEXT,value TEXT,weight INTEGER);"
                        "INSERT INTO tbl_2_n VALUES('ni''hao','nh','你好',1000);"
                        "CREATE TABLE tbl_2_j(key TEXT,jp TEXT,value TEXT,weight INTEGER);"
                        "INSERT INTO tbl_2_j VALUES('jia''she','js','假设',1000);"
                        "INSERT INTO tbl_2_j VALUES('jia''she','js','建设',900);"
                        "CREATE TABLE tbl_4_z(key TEXT,jp TEXT,value TEXT,weight INTEGER);"
                        "INSERT INTO tbl_4_z VALUES('zheng''le''xie''qian','zlxq','整了血铅',1200);"
                        "INSERT INTO tbl_4_z VALUES('zheng''le''xie''qian','zlxq','挣了些钱',800);",
                        nullptr, nullptr, nullptr) == SQLITE_OK);
    assert(sqlite3_close(database) == SQLITE_OK);

    NSError *error = nil;
    MSIMEClientSession *session = [[MSIMEClientSession alloc] initWithOptions:options error:&error];
    if (!session || error) {
        std::fprintf(stderr, "CreateTestSession failed: %s\n", error ? error.localizedDescription.UTF8String : "(null)");
    }
    assert(session && !error);
    assert([session setFocused:YES error:&error] && !error);
    return session;
}

static NSDictionary *TypeText(MSIMEClientSession *session, NSString *letters) {
    NSDictionary *last = nil;
    for (NSUInteger i = 0; i < letters.length; ++i) {
        unichar ch = [letters characterAtIndex:i];
        NSError *error = nil;
        last = [session typeASCII:static_cast<uint8_t>(ch) shift:NO error:&error];
        assert(last && !error);
    }
    return last;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        assert(argc >= 2);
        NSString *servicePath = [NSString stringWithUTF8String:argv[1]];
        assert(servicePath.length > 0);

        NSString *rootDir = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        MSIMEClientSession *session = CreateTestSession(rootDir);

        // 1. 缺失模型目录时静默降级：available == NO，enabled == NO，pid == 0，且组字纯 Tier 0 正常工作。
        {
            NSError *error = nil;
            NSDictionary *status = [session configureNeuralServiceWithOptions:@{
                @"enabled": @YES,
                @"helper_path": servicePath,
                @"model_directory": @"/nonexistent/shurufa-neural-missing-dir",
                @"socket_path": [rootDir stringByAppendingPathComponent:@"missing.sock"],
                @"require_model_file": @YES,
            } updateHandler:^(uint64_t generation) {
                (void)generation;
            } error:&error];
            assert(status && !error);
            assert([status[@"available"] isEqual:@NO]);
            assert([status[@"enabled"] isEqual:@NO]);
            assert([status[@"pid"] unsignedLongLongValue] == 0);
            assert(!session.neuralV2Enabled);

            NSDictionary *tr = TypeText(session, @"nihao");
            assert([tr[@"view"][@"provisional"] boolValue] == NO);
            assert([session command:MSIME_CANCEL error:nil] != nil);
        }

        // 2. 启动真实 `ShurufaNeuralService --test-mock`：校验 wakeup 唤醒、pollNeural 状态推进、
        // 暖态快敲空格 Condvar 同步等待以及 SIGKILL 崩溃自愈重启。
        {
            NSString *socketPath = [NSString stringWithFormat:@"/tmp/shurufa-ipc-test-%d.sock", ::getpid()];
            NSDictionary *cfg = @{
                @"enabled": @YES,
                @"helper_path": servicePath,
                @"model_directory": rootDir,
                @"socket_path": socketPath,
                @"extra_launch_arguments": @[@"--test-mock"],
                @"idle_unload_seconds": @60.0,
                @"request_timeout_ms": @1000,
                @"require_model_file": @NO,
            };

            __block NSMutableArray<NSDictionary *> *polls = [NSMutableArray array];
            __weak MSIMEClientSession *weakSession = session;
            void (^handler)(uint64_t) = ^(uint64_t generation) {
                (void)generation;
                MSIMEClientSession *s = weakSession;
                if (!s) return;
                NSDictionary *polled = [s pollNeuralWithError:nil];
                if (polled) [polls addObject:polled];
            };

            NSError *error = nil;
            NSDictionary *status = [session configureNeuralServiceWithOptions:cfg
                                                                updateHandler:handler
                                                                        error:&error];
            assert(status && !error);
            assert([status[@"available"] isEqual:@YES]);
            assert([status[@"enabled"] isEqual:@YES]);
            assert(session.neuralV2Enabled);

            assert([session setSurroundingContext:@"这个兄弟，应该多少还是" error:&error] && !error);
            NSDictionary *composed = TypeText(session, @"zhenglexieqian");
            assert([composed[@"view"][@"provisional"] isEqual:@YES]);

            PumpMainRunLoopUntil(^BOOL {
                for (NSDictionary *p in polls) {
                    if ([p[@"ready"] isEqual:@YES]) return YES;
                }
                return NO;
            }, 5.0);
            assert(polls.count > 0);
            NSDictionary *lastPoll = polls.lastObject;
            assert([lastPoll[@"ready"] isEqual:@YES]);
            assert([lastPoll[@"provisional"] isEqual:@NO]);
            assert([lastPoll[@"view"][@"candidates"] count] > 0);
            const pid_t firstPID = static_cast<pid_t>([lastPoll[@"pid"] intValue]);
            assert(firstPID > 0);
            assert([session command:MSIME_CANCEL error:&error] && !error);

            // 2b. 暖机状态下快敲空格，直接由 Rust Condvar 同步等待后提交（不泵主循环）。
            [polls removeAllObjects];
            NSDictionary *fastTyped = TypeText(session, @"jiashe");
            assert([fastTyped[@"view"][@"provisional"] isEqual:@YES]);
            NSDictionary *committed = [session command:MSIME_COMMIT_CANDIDATE error:&error];
            if (!committed || error || ![committed[@"commit"] isKindOfClass:NSString.class] || ![committed[@"commit"] length]) {
                std::fprintf(stderr, "fastTyped=%s\ncommitted=%s\nerror=%s\n",
                             fastTyped.description.UTF8String,
                             committed.description.UTF8String,
                             error ? error.localizedDescription.UTF8String : "(null)");
            }
            assert(committed && !error);
            assert([committed[@"commit"] isKindOfClass:NSString.class] && [committed[@"commit"] length] > 0);

            // 3. 模拟子进程 SIGKILL 崩溃：首次 Decode 优雅 fail-open；1.2 秒退避后自愈重起新 PID。
            ::kill(firstPID, SIGKILL);
            usleep(120 * 1000);
            [polls removeAllObjects];
            TypeText(session, @"zhenglexieqian");
            PumpMainRunLoopUntil(^BOOL {
                for (NSDictionary *p in polls) {
                    if ([p[@"ready"] isEqual:@YES]) return YES;
                }
                return NO;
            }, 3.0);
            assert([session command:MSIME_CANCEL error:nil] != nil);

            usleep(1200 * 1000);
            [polls removeAllObjects];
            TypeText(session, @"zhenglexieqian");
            PumpMainRunLoopUntil(^BOOL {
                for (NSDictionary *p in polls) {
                    if ([p[@"ready"] isEqual:@YES]) return YES;
                }
                return NO;
            }, 5.0);

            pid_t secondPID = 0;
            for (NSDictionary *p in polls) {
                if ([p[@"ready"] isEqual:@YES] && [p[@"pid"] intValue] > 0) {
                    secondPID = static_cast<pid_t>([p[@"pid"] intValue]);
                }
            }
            assert(secondPID > 0 && secondPID != firstPID);
        }

        NSError *closeErr = nil;
        assert([session closeWithError:&closeErr] && !closeErr);
        [NSFileManager.defaultManager removeItemAtPath:rootDir error:nil];
        std::printf("neural-service-ipc-test: ALL PASSED\n");
        return 0;
    }
}
