#import "../../src/voice/LocalPolishServer.h"

#include <atomic>
#include <cassert>
#include <csignal>
#include <memory>
#include <sys/stat.h>
#include <unistd.h>

namespace {

constexpr uint64_t kGiB = 1024ULL * 1024ULL * 1024ULL;

// 仅使用 Python 3.8+ 标准库的合成假 llama-server 脚本，用于离线验证进程托管、鉴权与生命周期。
static constexpr char kFakeLlamaServerScript[] = R"PY(#!/usr/bin/env python3
import argparse
import http.server
import json
import os
import signal
import socketserver
import sys
import threading
import time


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--alias", default="local")
    parser.add_argument("--ctx-size", type=int, default=4096)
    parser.add_argument("--n-gpu-layers", type=int, default=99)
    parser.add_argument("--flash-attn", default="on")
    parser.add_argument("--reasoning-budget", type=int, default=0)
    parser.add_argument("--parallel", type=int, default=1)
    parser.add_argument("--no-webui", action="store_true")
    parser.add_argument("--log-disable", action="store_true")
    parser.add_argument("--api-key", required=True)
    return parser.parse_args()


def watch_stdin_eof():
    try:
        sys.stdin.buffer.read()
    except Exception:
        pass
    os._exit(0)


def main():
    args = parse_args()
    signal.signal(signal.SIGTERM, lambda _sig, _frame: os._exit(0))
    threading.Thread(target=watch_stdin_eof, daemon=True).start()

    record_path = os.environ.get("FAKE_RECORD_PATH", "")
    if record_path:
        with open(record_path, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "argv": sys.argv[1:],
                    "host": args.host,
                    "port": args.port,
                    "model": args.model,
                    "alias": args.alias,
                    "ctx_size": args.ctx_size,
                    "n_gpu_layers": args.n_gpu_layers,
                    "flash_attn": args.flash_attn,
                    "reasoning_budget": args.reasoning_budget,
                    "parallel": args.parallel,
                    "no_webui": args.no_webui,
                    "log_disable": args.log_disable,
                    "api_key": args.api_key,
                    "pid": os.getpid(),
                },
                handle,
            )

    # 故意向 stderr 输出包含敏感片段的合成日志行，验证宿主侧自动脱敏。
    sys.stderr.write("synthetic startup diagnostic --api-key " + args.api_key + "\n")
    sys.stderr.flush()

    if os.environ.get("FAKE_CRASH_IMMEDIATELY") == "1":
        sys.stderr.write("synthetic crash before bind\n")
        sys.stderr.flush()
        os._exit(42)

    state = {
        "health_503_remaining": int(os.environ.get("FAKE_HEALTH_503_COUNT", "0")),
        "hang_health": os.environ.get("FAKE_HANG_HEALTH") == "1",
        "lock": threading.Lock(),
    }

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format_str, *fmt_args):
            return

        def send_json(self, status_code, payload):
            encoded = json.dumps(payload).encode("utf-8")
            self.send_response(status_code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            if self.path == "/health":
                with state["lock"]:
                    if state["hang_health"]:
                        self.send_json(503, {"error": {"message": "loading model"}})
                        return
                    if state["health_503_remaining"] > 0:
                        state["health_503_remaining"] -= 1
                        self.send_json(503, {"error": {"message": "loading model"}})
                        return
                self.send_json(200, {"status": "ok"})
                return
            self.send_json(404, {"error": "not_found"})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            _ = self.rfile.read(length) if length > 0 else b""
            auth = self.headers.get("Authorization", "")
            if auth != "Bearer " + args.api_key:
                self.send_json(401, {"error": {"message": "unauthorized"}})
                return
            if self.path == "/v1/chat/completions":
                self.send_json(
                    200,
                    {
                        "choices": [
                            {
                                "message": {"content": "synthetic ok"},
                                "finish_reason": "stop",
                            }
                        ]
                    },
                )
                return
            self.send_json(404, {"error": "not_found"})

    class ReusableServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
        allow_reuse_address = True
        daemon_threads = True

    server = ReusableServer((args.host, args.port), Handler)
    server.serve_forever(poll_interval=0.05)


if __name__ == "__main__":
    main()
)PY";

struct ScopedTempDir {
    NSString *path = nil;

    ScopedTempDir() {
        NSString *name = [NSString stringWithFormat:@"shurufa-local-polish-test-%@", NSUUID.UUID.UUIDString];
        path = [NSTemporaryDirectory() stringByAppendingPathComponent:name];
        const BOOL created = [NSFileManager.defaultManager createDirectoryAtPath:path
                                                     withIntermediateDirectories:YES
                                                                      attributes:nil
                                                                           error:nil];
        assert(created);
    }

    ~ScopedTempDir() {
        if (path.length) {
            [NSFileManager.defaultManager removeItemAtPath:path error:nil];
        }
    }
};

NSString *WriteExecutableScript(NSString *directory, NSString *fileName, NSString *content) {
    NSString *filePath = [directory stringByAppendingPathComponent:fileName];
    NSData *data = [content dataUsingEncoding:NSUTF8StringEncoding];
    assert([data writeToFile:filePath atomically:YES]);
    assert(::chmod(filePath.fileSystemRepresentation, 0755) == 0);
    return filePath;
}

NSString *WriteDummyFile(NSString *directory, NSString *fileName, NSString *content) {
    NSString *filePath = [directory stringByAppendingPathComponent:fileName];
    NSString *parent = filePath.stringByDeletingLastPathComponent;
    assert([NSFileManager.defaultManager createDirectoryAtPath:parent
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:nil]);
    NSData *data = [content dataUsingEncoding:NSUTF8StringEncoding];
    assert([data writeToFile:filePath atomically:YES]);
    return filePath;
}

NSString *WriteWrapperBinary(NSString *directory,
                             NSString *wrapperName,
                             NSString *baseScriptPath,
                             NSDictionary<NSString *, NSString *> *extraEnv) {
    NSMutableString *lines = [NSMutableString stringWithString:@"#!/bin/sh\n"];
    for (NSString *key in extraEnv) {
        [lines appendFormat:@"export %@='%@'\n", key, extraEnv[key]];
    }
    [lines appendFormat:@"exec '%@' \"$@\"\n", baseScriptPath];
    return WriteExecutableScript(directory, wrapperName, lines);
}

BOOL IsHex64Lowercase(NSString *value) {
    if (value.length != 64) return NO;
    for (NSUInteger i = 0; i < value.length; ++i) {
        const unichar ch = [value characterAtIndex:i];
        const BOOL digit = (ch >= '0' && ch <= '9');
        const BOOL hexLower = (ch >= 'a' && ch <= 'f');
        if (!digit && !hexLower) return NO;
    }
    return YES;
}

NSInteger SendDirectPost(NSString *urlString, NSString * _Nullable authHeader) {
    NSURLSessionConfiguration *config = [NSURLSessionConfiguration ephemeralSessionConfiguration];
    config.connectionProxyDictionary = @{};
    config.timeoutIntervalForRequest = 5.0;
    NSURLSession *session = [NSURLSession sessionWithConfiguration:config];

    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:[NSURL URLWithString:urlString]];
    request.HTTPMethod = @"POST";
    request.HTTPBody = [@"{\"model\":\"local\",\"messages\":[]}" dataUsingEncoding:NSUTF8StringEncoding];
    [request setValue:@"application/json" forHTTPHeaderField:@"Content-Type"];
    if (authHeader.length) {
        [request setValue:authHeader forHTTPHeaderField:@"Authorization"];
    }

    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block NSInteger statusCode = -1;
    NSURLSessionDataTask *task = [session dataTaskWithRequest:request
                                            completionHandler:^(NSData * _Nullable data,
                                                                NSURLResponse * _Nullable response,
                                                                NSError * _Nullable error) {
                                                (void)data;
                                                if (!error && [response isKindOfClass:NSHTTPURLResponse.class]) {
                                                    statusCode = static_cast<NSHTTPURLResponse *>(response).statusCode;
                                                }
                                                dispatch_semaphore_signal(sem);
                                            }];
    [task resume];
    const long waited = dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
    [session invalidateAndCancel];
    assert(waited == 0);
    return statusCode;
}

BOOL WaitUntil(NSTimeInterval timeoutSeconds, BOOL (^predicate)(void)) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeoutSeconds];
    while (!predicate() && deadline.timeIntervalSinceNow > 0.0) {
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
    }
    return predicate();
}

void TestMemoryTierPolicies(void) {
    assert(MSIMELocalPolishServerIdleTimeoutForMemoryBytes(64ULL * kGiB) == 1800.0);
    assert(MSIMELocalPolishServerIdleTimeoutForMemoryBytes(48ULL * kGiB) == 1800.0);
    assert(MSIMELocalPolishServerIdleTimeoutForMemoryBytes(47ULL * kGiB) == 600.0);
    assert(MSIMELocalPolishServerIdleTimeoutForMemoryBytes(24ULL * kGiB) == 600.0);
    assert(MSIMELocalPolishServerIdleTimeoutForMemoryBytes(16ULL * kGiB) == 180.0);
    assert(MSIMELocalPolishServerIdleTimeoutForMemoryBytes(8ULL * kGiB) == 180.0);

    assert(MSIMELocalPolishServerContextSizeForMemoryBytes(64ULL * kGiB) == 4096U);
    assert(MSIMELocalPolishServerContextSizeForMemoryBytes(24ULL * kGiB) == 4096U);
    assert(MSIMELocalPolishServerContextSizeForMemoryBytes(16ULL * kGiB) == 2048U);
}

void TestExecutableAndModelDiscovery(const ScopedTempDir &tempDir) {
    NSString *realBin = WriteExecutableScript(tempDir.path, @"real-llama-server", @"#!/bin/sh\nexit 0\n");
    NSString *symlinkBin = [tempDir.path stringByAppendingPathComponent:@"symlink-llama-server"];
    assert([NSFileManager.defaultManager createSymbolicLinkAtPath:symlinkBin withDestinationPath:realBin error:nil]);

    NSString *bundlePath = [tempDir.path stringByAppendingPathComponent:@"FakeShurufa.app"];
    NSString *helpersDir = [bundlePath stringByAppendingPathComponent:@"Contents/Helpers"];
    assert([NSFileManager.defaultManager createDirectoryAtPath:helpersDir
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:nil]);
    NSString *bundledBin = WriteExecutableScript(helpersDir, @"llama-server", @"#!/bin/sh\nexit 0\n");

    // 环境变量优先级高于应用包内置 Helpers。
    NSString *fromEnv = MSIMELocalPolishServerDiscoverExecutable(bundlePath, @{@"SHURUFA_LLAMA_SERVER_BIN" : symlinkBin});
    assert([fromEnv isEqualToString:symlinkBin]);

    // 未配置环境变量时回退到应用包内置 Helpers。
    NSString *fromBundle = MSIMELocalPolishServerDiscoverExecutable(bundlePath, @{});
    assert([fromBundle isEqualToString:bundledBin]);

    // 模型发现：跳过 mmproj，优先通用模型而非 hy-mt，并支持软链接。
    NSString *cacheDir = [tempDir.path stringByAppendingPathComponent:@"hf-cache"];
    NSString *realGeneral = WriteDummyFile(cacheDir, @"Qwen-Synthetic-9B.gguf", @"synthetic-gguf");
    NSString *modelDir = [tempDir.path stringByAppendingPathComponent:@"local-llm"];
    assert([NSFileManager.defaultManager createDirectoryAtPath:modelDir
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:nil]);
    WriteDummyFile(modelDir, @"mmproj-vision-f16.gguf", @"synthetic-mmproj");
    NSString *hyMtPath = WriteDummyFile(modelDir, @"Hy-MT2-1.8B-Q4_K_M.gguf", @"synthetic-hymt");
    NSString *linkedGeneral = [modelDir stringByAppendingPathComponent:@"Qwen-Synthetic-9B.gguf"];
    assert([NSFileManager.defaultManager createSymbolicLinkAtPath:linkedGeneral withDestinationPath:realGeneral error:nil]);

    NSString *discovered = MSIMELocalPolishServerDiscoverModelPath(nil, modelDir);
    assert([discovered isEqualToString:linkedGeneral]);

    // 显式指定模型路径优先级最高。
    NSString *explicitDiscovered = MSIMELocalPolishServerDiscoverModelPath(hyMtPath, modelDir);
    assert([explicitDiscovered isEqualToString:hyMtPath]);

    // 仅存在翻译模型时仍可回退发现。
    assert([NSFileManager.defaultManager removeItemAtPath:linkedGeneral error:nil]);
    NSString *fallbackHyMt = MSIMELocalPolishServerDiscoverModelPath(nil, modelDir);
    assert([fallbackHyMt isEqualToString:hyMtPath]);

    // 空目录返回 nil。
    NSString *emptyDir = [tempDir.path stringByAppendingPathComponent:@"empty-llm"];
    assert([NSFileManager.defaultManager createDirectoryAtPath:emptyDir
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:nil]);
    assert(MSIMELocalPolishServerDiscoverModelPath(nil, emptyDir) == nil);
}

void TestLaunchSecurityCoalescingAndMemoryPressure(const ScopedTempDir &tempDir, NSString *baseScript) {
    NSString *modelPath = WriteDummyFile(tempDir.path, @"models/synthetic-9b.gguf", @"synthetic-weights");
    NSString *recordPath = [tempDir.path stringByAppendingPathComponent:@"launch-record.json"];
    NSString *wrapper = WriteWrapperBinary(tempDir.path, @"fake-llama-ok.sh", baseScript, @{
        @"FAKE_RECORD_PATH" : recordPath,
        @"FAKE_HEALTH_503_COUNT" : @"2",
    });

    auto notifiedLowMemBytes = std::make_shared<std::atomic<uint64_t>>(0);
    MSIMELocalPolishServerConfiguration *config = [MSIMELocalPolishServerConfiguration new];
    config.executablePath = wrapper;
    config.modelDirectory = modelPath.stringByDeletingLastPathComponent;
    config.physicalMemoryBytesOverride = 16ULL * kGiB;
    config.healthTimeout = 8.0;
    config.healthPollInterval = 0.04;
    config.listenToSystemMemoryPressure = NO;
    config.listenToApplicationTermination = NO;
    config.onLowMemoryPreLaunch = ^(uint64_t bytes) {
        notifiedLowMemBytes->store(bytes);
    };

    MSIMELocalPolishServer *server = [[MSIMELocalPolishServer alloc] initWithConfiguration:config];
    assert(server.activeIdleTimeout == 180.0);
    assert(server.activeContextSize == 2048U);

    // 并发 3 次 ensureReady 应合并为一次拉起，返回相同的端点、代次与密钥。
    NSMutableArray<MSIMELocalPolishServerEndpoint *> *results = [NSMutableArray array];
    dispatch_group_t group = dispatch_group_create();
    for (int index = 0; index < 3; ++index) {
        dispatch_group_enter(group);
        [server ensureReadyWithModelPath:nil
                              completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint,
                                           NSError * _Nullable error) {
                                  assert(error == nil);
                                  assert(endpoint != nil);
                                  @synchronized(results) {
                                      [results addObject:endpoint];
                                  }
                                  dispatch_group_leave(group);
                              }];
    }
    const long groupWait = dispatch_group_wait(group, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));
    assert(groupWait == 0);
    assert(results.count == 3);

    MSIMELocalPolishServerEndpoint *first = results[0];
    for (MSIMELocalPolishServerEndpoint *endpoint in results) {
        assert(endpoint.port == first.port);
        assert(endpoint.generation == first.generation);
        assert([endpoint.apiKey isEqualToString:first.apiKey]);
    }
    assert(server.isRunning);
    assert(server.listeningPort == first.port);
    assert(IsHex64Lowercase(first.apiKey));
    assert(![first.description containsString:first.apiKey]);
    assert(WaitUntil(2.0, ^BOOL { return notifiedLowMemBytes->load() == 16ULL * kGiB; }));

    // 校验传递给子进程的安全参数。
    NSData *recordData = [NSData dataWithContentsOfFile:recordPath];
    assert(recordData != nil);
    NSDictionary *record = [NSJSONSerialization JSONObjectWithData:recordData options:0 error:nil];
    assert([record isKindOfClass:NSDictionary.class]);
    assert([record[@"host"] isEqualToString:@"127.0.0.1"]);
    assert([record[@"port"] unsignedShortValue] == first.port);
    assert([record[@"ctx_size"] unsignedIntegerValue] == 2048U);
    assert([record[@"n_gpu_layers"] integerValue] == 99);
    assert([record[@"flash_attn"] isEqualToString:@"on"]);
    assert([record[@"reasoning_budget"] integerValue] == 0);
    assert([record[@"parallel"] integerValue] == 1);
    assert([record[@"no_webui"] boolValue] == YES);
    assert([record[@"log_disable"] boolValue] == YES);
    assert([record[@"api_key"] isEqualToString:first.apiKey]);

    // 校验 stderr 诊断缓存不泄露 apiKey。
    assert(WaitUntil(2.0, ^BOOL { return server.recentSanitizedStderrLines.count > 0; }));
    for (NSString *line in server.recentSanitizedStderrLines) {
        assert(![line containsString:first.apiKey]);
    }

    // 校验未携带或错误 Bearer 密钥时被拒（401），正确密钥时通过（200）。
    assert(SendDirectPost(first.chatCompletionsURL, nil) == 401);
    assert(SendDirectPost(first.chatCompletionsURL, @"Bearer wrong-synthetic-token") == 401);
    assert(SendDirectPost(first.chatCompletionsURL, [@"Bearer " stringByAppendingString:first.apiKey]) == 200);

    // 内存压力回收：优雅停止、不计入异常重启限频、再次启动会轮换一次性密钥。
    const pid_t oldPid = server.processIdentifier;
    assert(oldPid > 0);
    [server handleMemoryPressure];
    assert(WaitUntil(3.0, ^BOOL { return !server.isRunning && ::kill(oldPid, 0) != 0; }));
    assert(server.recentRestartCount == 0);

    dispatch_semaphore_t restartSem = dispatch_semaphore_create(0);
    __block MSIMELocalPolishServerEndpoint *second = nil;
    [server ensureReadyWithModelPath:nil
                          completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
                              assert(error == nil && endpoint != nil);
                              second = endpoint;
                              dispatch_semaphore_signal(restartSem);
                          }];
    assert(dispatch_semaphore_wait(restartSem, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC)) == 0);
    assert(second.generation > first.generation);
    assert(![second.apiKey isEqualToString:first.apiKey]);

    [server stopSynchronously];
    assert(!server.isRunning);
}

void TestIdleReclamation(const ScopedTempDir &tempDir, NSString *baseScript) {
    NSString *modelPath = WriteDummyFile(tempDir.path, @"idle/synthetic.gguf", @"synthetic");
    NSString *wrapper = WriteWrapperBinary(tempDir.path, @"fake-llama-idle.sh", baseScript, @{});

    MSIMELocalPolishServerConfiguration *config = [MSIMELocalPolishServerConfiguration new];
    config.executablePath = wrapper;
    config.modelDirectory = modelPath.stringByDeletingLastPathComponent;
    config.idleTimeoutOverride = 0.35;
    config.healthTimeout = 5.0;
    config.healthPollInterval = 0.03;
    config.listenToSystemMemoryPressure = NO;
    config.listenToApplicationTermination = NO;

    MSIMELocalPolishServer *server = [[MSIMELocalPolishServer alloc] initWithConfiguration:config];
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [server ensureReadyWithModelPath:nil
                          completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
                              assert(endpoint != nil && error == nil);
                              dispatch_semaphore_signal(sem);
                          }];
    assert(dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 8 * NSEC_PER_SEC)) == 0);
    const pid_t pid = server.processIdentifier;
    assert(server.isRunning && pid > 0);

    // 刷新活跃时间可短暂续期，随后空闲回收并彻底退出进程。
    usleep(150 * 1000);
    [server touchActivity];
    assert(server.isRunning);

    assert(WaitUntil(3.0, ^BOOL { return !server.isRunning && ::kill(pid, 0) != 0; }));
    assert(server.recentRestartCount == 0);
    [server stopSynchronously];
}

void TestCrashRateLimitProtection(const ScopedTempDir &tempDir, NSString *baseScript) {
    NSString *modelPath = WriteDummyFile(tempDir.path, @"crash/synthetic.gguf", @"synthetic");
    NSString *wrapper = WriteWrapperBinary(tempDir.path, @"fake-llama-crash.sh", baseScript, @{
        @"FAKE_CRASH_IMMEDIATELY" : @"1",
    });

    MSIMELocalPolishServerConfiguration *config = [MSIMELocalPolishServerConfiguration new];
    config.executablePath = wrapper;
    config.modelDirectory = modelPath.stringByDeletingLastPathComponent;
    config.healthTimeout = 5.0;
    config.healthPollInterval = 0.03;
    config.restartRateWindow = 60.0;
    config.maxRestartsInWindow = 2;
    config.listenToSystemMemoryPressure = NO;
    config.listenToApplicationTermination = NO;

    MSIMELocalPolishServer *server = [[MSIMELocalPolishServer alloc] initWithConfiguration:config];

    auto runOnce = ^NSError *(void) {
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        __block NSError *captured = nil;
        [server ensureReadyWithModelPath:nil
                              completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
                                  assert(endpoint == nil);
                                  captured = error;
                                  dispatch_semaphore_signal(sem);
                              }];
        assert(dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 8 * NSEC_PER_SEC)) == 0);
        return captured;
    };

    // 第 1 次为首次启动失败，第 2、3 次为允许范围内的 2 次重启，第 4 次触发限频拦截。
    NSError *err1 = runOnce();
    assert(err1.code == MSIMELocalPolishServerErrorProcessExited);
    NSError *err2 = runOnce();
    assert(err2.code == MSIMELocalPolishServerErrorProcessExited);
    NSError *err3 = runOnce();
    assert(err3.code == MSIMELocalPolishServerErrorProcessExited);
    NSError *err4 = runOnce();
    assert(err4.code == MSIMELocalPolishServerErrorRateLimited);
    assert(server.recentRestartCount == 2);
    [server stopSynchronously];
}

void TestHealthTimeoutAndMissingAssets(const ScopedTempDir &tempDir, NSString *baseScript) {
    // 1. 缺少模型。
    NSString *emptyDir = [tempDir.path stringByAppendingPathComponent:@"no-models"];
    assert([NSFileManager.defaultManager createDirectoryAtPath:emptyDir
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:nil]);
    {
        MSIMELocalPolishServerConfiguration *config = [MSIMELocalPolishServerConfiguration new];
        config.modelDirectory = emptyDir;
        config.listenToSystemMemoryPressure = NO;
        config.listenToApplicationTermination = NO;
        MSIMELocalPolishServer *server = [[MSIMELocalPolishServer alloc] initWithConfiguration:config];
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        __block NSError *captured = nil;
        [server ensureReadyWithModelPath:nil
                              completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
                                  assert(endpoint == nil);
                                  captured = error;
                                  dispatch_semaphore_signal(sem);
                              }];
        assert(dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
        assert(captured.code == MSIMELocalPolishServerErrorModelNotFound);
    }

    // 2. 缺少可执行文件。
    NSString *modelPath = WriteDummyFile(tempDir.path, @"timeout/synthetic.gguf", @"synthetic");
    {
        MSIMELocalPolishServerConfiguration *config = [MSIMELocalPolishServerConfiguration new];
        config.executablePath = [tempDir.path stringByAppendingPathComponent:@"does-not-exist-llama-server"];
        config.modelDirectory = modelPath.stringByDeletingLastPathComponent;
        config.listenToSystemMemoryPressure = NO;
        config.listenToApplicationTermination = NO;
        MSIMELocalPolishServer *server = [[MSIMELocalPolishServer alloc] initWithConfiguration:config];
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        __block NSError *captured = nil;
        [server ensureReadyWithModelPath:nil
                              completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
                                  assert(endpoint == nil);
                                  captured = error;
                                  dispatch_semaphore_signal(sem);
                              }];
        assert(dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
        assert(captured.code == MSIMELocalPolishServerErrorBinaryNotFound);
    }

    // 3. /health 持续 503 导致启动超时并回收子进程。
    NSString *hangWrapper = WriteWrapperBinary(tempDir.path, @"fake-llama-hang.sh", baseScript, @{
        @"FAKE_HANG_HEALTH" : @"1",
    });
    {
        MSIMELocalPolishServerConfiguration *config = [MSIMELocalPolishServerConfiguration new];
        config.executablePath = hangWrapper;
        config.modelDirectory = modelPath.stringByDeletingLastPathComponent;
        config.healthTimeout = 0.45;
        config.healthPollInterval = 0.05;
        config.listenToSystemMemoryPressure = NO;
        config.listenToApplicationTermination = NO;
        MSIMELocalPolishServer *server = [[MSIMELocalPolishServer alloc] initWithConfiguration:config];
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        __block NSError *captured = nil;
        [server ensureReadyWithModelPath:nil
                              completion:^(MSIMELocalPolishServerEndpoint * _Nullable endpoint, NSError * _Nullable error) {
                                  assert(endpoint == nil);
                                  captured = error;
                                  dispatch_semaphore_signal(sem);
                              }];
        assert(dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 8 * NSEC_PER_SEC)) == 0);
        assert(captured.code == MSIMELocalPolishServerErrorHealthTimeout);
        assert(!server.isRunning);
        [server stopSynchronously];
    }
}

} // namespace

int main(void) {
    @autoreleasepool {
        ScopedTempDir tempDir;
        NSString *baseScript = WriteExecutableScript(
            tempDir.path,
            @"fake_llama_server.py",
            [NSString stringWithUTF8String:kFakeLlamaServerScript]);

        TestMemoryTierPolicies();
        TestExecutableAndModelDiscovery(tempDir);
        TestLaunchSecurityCoalescingAndMemoryPressure(tempDir, baseScript);
        TestIdleReclamation(tempDir, baseScript);
        TestCrashRateLimitProtection(tempDir, baseScript);
        TestHealthTimeoutAndMissingAssets(tempDir, baseScript);
    }
    return 0;
}
