#import "../../src/voice/VoicePolishClient.h"

#include <cassert>
#include <cstdlib>
#include <cstring>
#include <sys/stat.h>
#include <unistd.h>

namespace {

// 合成回环测试服务器：覆盖普通 JSON、SSE 流式、重定向拦截、慢响应、超时、500 与超大响应。
static constexpr char kMockPolishHttpServerScript[] = R"PY(#!/usr/bin/env python3
import http.server
import json
import os
import signal
import socketserver
import sys
import threading
import time


def watch_stdin_eof():
    try:
        sys.stdin.buffer.read()
    except Exception:
        pass
    os._exit(0)


def main():
    signal.signal(signal.SIGTERM, lambda _sig, _frame: os._exit(0))
    threading.Thread(target=watch_stdin_eof, daemon=True).start()

    state = {"redirect_hits": 0, "lock": threading.Lock()}

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format_str, *fmt_args):
            return

        def send_json(self, code, payload):
            encoded = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def send_chat(self, content):
            self.send_json(
                200,
                {
                    "choices": [
                        {
                            "message": {"content": content},
                            "finish_reason": "stop",
                        }
                    ]
                },
            )

        def do_GET(self):
            if self.path == "/redirect-target":
                with state["lock"]:
                    state["redirect_hits"] += 1
                self.send_chat("should never be fetched")
                return
            if self.path == "/redirect-hits":
                with state["lock"]:
                    hits = state["redirect_hits"]
                self.send_json(200, {"hits": hits})
                return
            self.send_json(404, {"error": "not_found"})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            _ = self.rfile.read(length) if length > 0 else b""

            if self.path == "/redirect-target":
                with state["lock"]:
                    state["redirect_hits"] += 1
                self.send_chat("should never be fetched")
                return
            if self.path == "/redirect":
                self.send_response(302)
                self.send_header("Location", "http://127.0.0.1:%d/redirect-target" % self.server.server_port)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if self.path == "/v1/chat/completions":
                self.send_chat("合成润色后的标准文本。")
                return
            if self.path == "/think":
                self.send_chat("<think>内部推理草稿</think>去除思考标签后的合成结果")
                return
            if self.path == "/unclosed-think":
                self.send_chat("<think>未闭合的内部思考")
                return
            if self.path == "/meta-reply":
                self.send_chat("好的，我来帮你整理一下这段语音内容：合成文本")
                return
            if self.path == "/action-rewrite":
                self.send_chat("<action>rewrite</action>改写完成后的合成选区文本")
                return
            if self.path == "/action-dictate":
                self.send_chat("<action>dictate</action>仅作为普通听写的合成文本")
                return
            if self.path == "/slow":
                time.sleep(0.30)
                self.send_chat("慢响应完成后的合成结果")
                return
            if self.path == "/timeout":
                time.sleep(1.50)
                self.send_chat("超时后才返回的文本")
                return
            if self.path == "/http-500":
                self.send_json(500, {"error": {"message": "synthetic server error"}})
                return
            if self.path == "/oversized":
                self.send_chat("A" * 512)
                return
            if self.path == "/sse":
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()
                chunks = ["合成", "流式", "润色结果"]
                for piece in chunks:
                    frame = "data: " + json.dumps(
                        {"choices": [{"delta": {"content": piece}, "finish_reason": None}]},
                        ensure_ascii=False,
                    ) + "\n\n"
                    self.wfile.write(frame.encode("utf-8"))
                    self.wfile.flush()
                    time.sleep(0.03)
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
                return
            self.send_json(404, {"error": "unknown_route"})

    class ReusableServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
        allow_reuse_address = True
        daemon_threads = True

    server = ReusableServer(("127.0.0.1", 0), Handler)
    sys.stdout.write(str(server.server_port) + "\n")
    sys.stdout.flush()
    server.serve_forever(poll_interval=0.05)


if __name__ == "__main__":
    main()
)PY";

// 合成假 llama-server：校验一次性 Bearer 密钥，并支持单次模拟连接中途中止以验证自动重启重试。
static constexpr char kFakeLocalLlamaForClientScript[] = R"PY(#!/usr/bin/env python3
import argparse
import http.server
import json
import os
import signal
import socketserver
import sys
import threading


def watch_stdin_eof():
    try:
        sys.stdin.buffer.read()
    except Exception:
        pass
    os._exit(0)


def main():
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
    args = parser.parse_args()

    signal.signal(signal.SIGTERM, lambda _sig, _frame: os._exit(0))
    threading.Thread(target=watch_stdin_eof, daemon=True).start()
    drop_marker = os.environ.get("FAKE_DROP_ONCE_MARKER", "")

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format_str, *fmt_args):
            return

        def send_json(self, code, payload):
            encoded = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            if self.path == "/health":
                self.send_json(200, {"status": "ok"})
                return
            self.send_json(404, {"error": "not_found"})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length) if length > 0 else b""
            if self.headers.get("Authorization", "") != "Bearer " + args.api_key:
                self.send_json(401, {"error": "unauthorized"})
                return
            if b"trigger_drop_once" in body and drop_marker and not os.path.exists(drop_marker):
                with open(drop_marker, "w", encoding="utf-8") as handle:
                    handle.write("dropped")
                # 不回传 HTTP 响应直接退出进程，制造真实回环连接中断以验证单次自动重启重试。
                os._exit(1)
            self.send_json(
                200,
                {
                    "choices": [
                        {
                            "message": {"content": "本地模型润色结果"},
                            "finish_reason": "stop",
                        }
                    ]
                },
            )

    class ReusableServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
        allow_reuse_address = True
        daemon_threads = True

    server = ReusableServer((args.host, args.port), Handler)
    server.serve_forever(poll_interval=0.05)


if __name__ == "__main__":
    main()
)PY";

char *DuplicateJSONString(NSDictionary *dict) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:dict options:0 error:nil];
    if (!data) return nullptr;
    char *copy = static_cast<char *>(std::malloc(data.length + 1));
    if (!copy) return nullptr;
    std::memcpy(copy, data.bytes, data.length);
    copy[data.length] = '\0';
    return copy;
}

void SyntheticFreeString(char * _Nullable ptr) {
    std::free(ptr);
}

// 严格按蓝图 C.4 契约实现的合成请求描述器桩函数。
char * _Nullable SyntheticBuildHTTPRequest(const uint8_t *requestBytes, size_t length) {
    NSData *data = [NSData dataWithBytes:requestBytes length:length];
    NSDictionary *input = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![input isKindOfClass:NSDictionary.class]) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"invalid_request"}});
    }

    NSString *endpoint = [input[@"endpoint"] isKindOfClass:NSString.class] ? input[@"endpoint"] : @"";
    NSURL *url = endpoint.length ? [NSURL URLWithString:endpoint] : nil;
    if (!url || !url.host.length) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"polish_invalid_endpoint"}});
    }

    const BOOL isLoopback = [url.host isEqualToString:@"127.0.0.1"] || [url.host isEqualToString:@"localhost"];
    NSDictionary *context = [input[@"context"] isKindOfClass:NSDictionary.class] ? input[@"context"] : nil;
    const BOOL isSecureField = [context[@"is_secure_field"] boolValue];
    if (isSecureField && !isLoopback) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"polish_secure_field_blocked"}});
    }

    NSString *token = [input[@"token"] isKindOfClass:NSString.class] ? input[@"token"] : @"";
    NSString *provider = [input[@"provider"] isKindOfClass:NSString.class] ? input[@"provider"] : @"";
    if (!isLoopback && ![provider isEqualToString:@"local"] && ![provider isEqualToString:@"ollama"] && !token.length) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"polish_missing_token"}});
    }

    NSMutableArray<NSDictionary *> *headers = [@[
        @{@"name" : @"Content-Type", @"value" : @"application/json"},
    ] mutableCopy];
    if (token.length) {
        [headers addObject:@{@"name" : @"Authorization", @"value" : [@"Bearer " stringByAppendingString:token]}];
    }

    NSString *promptID = [input[@"prompt_id"] isKindOfClass:NSString.class] ? input[@"prompt_id"] : @"casual";
    uint64_t timeoutMs = 5000;
    uint64_t maxBytes = 1024 * 1024;
    if ([promptID isEqualToString:@"test_short_timeout"]) {
        timeoutMs = 250;
    } else if ([promptID isEqualToString:@"test_small_cap"]) {
        maxBytes = 64;
    }

    NSString *mode = [input[@"mode"] isKindOfClass:NSString.class] ? input[@"mode"] : @"auto";
    NSString *selectedText = [context[@"selected_text"] isKindOfClass:NSString.class] ? context[@"selected_text"] : @"";
    const BOOL enableRewrite = [input[@"enable_selection_rewrite"] boolValue];
    NSString *resolvedMode = @"dictation";
    BOOL expectActionTag = NO;
    if ([mode isEqualToString:@"translate"]) {
        resolvedMode = @"translate";
    } else if ([mode isEqualToString:@"rewrite_selection"]) {
        resolvedMode = @"rewrite_selection";
    } else if (enableRewrite && selectedText.length > 0) {
        resolvedMode = @"rewrite_selection";
        expectActionTag = YES;
    }

    NSString *rawText = [input[@"raw_text"] isKindOfClass:NSString.class] ? input[@"raw_text"] : @"";
    const BOOL stream = [input[@"stream"] boolValue];
    NSDictionary *httpBodyObj = @{
        @"model" : ([input[@"model"] isKindOfClass:NSString.class] ? input[@"model"] : @"synthetic-model"),
        @"stream" : @(stream),
        @"messages" : @[@{@"role" : @"user", @"content" : rawText}],
    };
    NSData *bodyJSON = [NSJSONSerialization dataWithJSONObject:httpBodyObj options:0 error:nil];
    NSString *bodyString = [[NSString alloc] initWithData:bodyJSON encoding:NSUTF8StringEncoding] ?: @"{}";

    return DuplicateJSONString(@{
        @"ok" : @YES,
        @"value" : @{
            @"url" : endpoint,
            @"method" : @"POST",
            @"headers" : headers,
            @"body" : bodyString,
            @"timeout_ms" : @(timeoutMs),
            @"connect_timeout_ms" : @2000,
            @"max_response_bytes" : @(maxBytes),
            @"no_proxy" : @(isLoopback),
            @"resolved_mode" : resolvedMode,
            @"expect_action_tag" : @(expectActionTag),
            @"app_category" : @"general",
            @"stream" : @(stream),
        },
    });
}

// 严格按蓝图 C.4 契约实现的合成响应/SSE 解析桩函数。
char * _Nullable SyntheticParseResponse(const uint8_t *bodyBytes, size_t length, bool isSSE) {
    NSData *data = [NSData dataWithBytes:bodyBytes length:length];
    NSDictionary *json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![json isKindOfClass:NSDictionary.class]) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"parse_error"}});
    }
    NSArray *choices = [json[@"choices"] isKindOfClass:NSArray.class] ? json[@"choices"] : nil;
    NSDictionary *first = choices.count && [choices[0] isKindOfClass:NSDictionary.class] ? choices[0] : nil;
    if (!first) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"parse_error"}});
    }

    NSString *text = nil;
    if (isSSE) {
        NSDictionary *delta = [first[@"delta"] isKindOfClass:NSDictionary.class] ? first[@"delta"] : nil;
        if ([delta[@"content"] isKindOfClass:NSString.class]) {
            text = delta[@"content"];
        }
    } else {
        NSDictionary *message = [first[@"message"] isKindOfClass:NSDictionary.class] ? first[@"message"] : nil;
        if ([message[@"content"] isKindOfClass:NSString.class]) {
            text = message[@"content"];
        }
    }
    id finishReason = [first[@"finish_reason"] isKindOfClass:NSString.class] ? first[@"finish_reason"] : NSNull.null;
    const BOOL done = !isSSE || finishReason != NSNull.null;
    return DuplicateJSONString(@{
        @"ok" : @YES,
        @"value" : @{
            @"text" : (text ? static_cast<id>(text) : static_cast<id>(NSNull.null)),
            @"finish_reason" : finishReason,
            @"done" : @(done),
        },
    });
}

// 严格按蓝图 B.3 契约实现的合成防幻觉清洗桩函数。
char * _Nullable SyntheticCleanOutput(const uint8_t *requestBytes, size_t length) {
    NSData *data = [NSData dataWithBytes:requestBytes length:length];
    NSDictionary *input = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![input isKindOfClass:NSDictionary.class]) {
        return DuplicateJSONString(@{@"ok" : @NO, @"error" : @{@"code" : @"clean_error"}});
    }

    NSString *polished = [input[@"polished_text"] isKindOfClass:NSString.class] ? input[@"polished_text"] : @"";
    NSString *mode = [input[@"mode"] isKindOfClass:NSString.class] ? input[@"mode"] : @"dictation";
    const BOOL expectActionTag = [input[@"expect_action_tag"] boolValue];

    // 1. 剥离 <think>...</think>，未闭合则拒收。
    NSMutableString *working = [polished mutableCopy];
    for (;;) {
        NSRange start = [working rangeOfString:@"<think>"];
        if (start.location == NSNotFound) break;
        NSRange searchRange = NSMakeRange(NSMaxRange(start), working.length - NSMaxRange(start));
        NSRange end = [working rangeOfString:@"</think>" options:0 range:searchRange];
        if (end.location == NSNotFound) {
            return DuplicateJSONString(@{
                @"ok" : @YES,
                @"value" : @{
                    @"accepted" : @NO,
                    @"text" : @"",
                    @"resolved_action" : @"insert",
                    @"rejection_reason" : @"unclosed_think",
                },
            });
        }
        [working deleteCharactersInRange:NSMakeRange(start.location, NSMaxRange(end) - start.location)];
    }

    NSString *trimmed = [working stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    // 2. 助手元回复拒收。
    if ([trimmed hasPrefix:@"好的，我来帮你"] || [trimmed hasPrefix:@"As an AI"]) {
        return DuplicateJSONString(@{
            @"ok" : @YES,
            @"value" : @{
                @"accepted" : @NO,
                @"text" : @"",
                @"resolved_action" : @"insert",
                @"rejection_reason" : @"assistant_meta_reply",
            },
        });
    }

    // 3. 解析 <action> 标签决定替换选区还是插入。
    NSString *resolvedAction = [mode isEqualToString:@"rewrite_selection"] ? @"replace_selection" : @"insert";
    if (expectActionTag) {
        if ([trimmed hasPrefix:@"<action>rewrite</action>"]) {
            resolvedAction = @"replace_selection";
            trimmed = [[trimmed substringFromIndex:[@"<action>rewrite</action>" length]]
                stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        } else if ([trimmed hasPrefix:@"<action>dictate</action>"]) {
            resolvedAction = @"insert";
            trimmed = [[trimmed substringFromIndex:[@"<action>dictate</action>" length]]
                stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        } else {
            resolvedAction = @"insert";
        }
    }

    return DuplicateJSONString(@{
        @"ok" : @YES,
        @"value" : @{
            @"accepted" : @(trimmed.length > 0),
            @"text" : trimmed,
            @"resolved_action" : resolvedAction,
            @"rejection_reason" : (trimmed.length > 0 ? static_cast<id>(NSNull.null) : static_cast<id>(@"empty_response")),
        },
    });
}

MSIMEVoicePolishBridge MakeSyntheticBridge(void) {
    MSIMEVoicePolishBridge bridge = {};
    bridge.buildHTTPRequest = SyntheticBuildHTTPRequest;
    bridge.parseResponse = SyntheticParseResponse;
    bridge.cleanOutput = SyntheticCleanOutput;
    bridge.freeString = SyntheticFreeString;
    return bridge;
}

struct ScopedTempDir {
    NSString *path = nil;

    ScopedTempDir() {
        NSString *name = [NSString stringWithFormat:@"shurufa-polish-client-test-%@", NSUUID.UUID.UUIDString];
        path = [NSTemporaryDirectory() stringByAppendingPathComponent:name];
        assert([NSFileManager.defaultManager createDirectoryAtPath:path
                                       withIntermediateDirectories:YES
                                                        attributes:nil
                                                             error:nil]);
    }

    ~ScopedTempDir() {
        if (path.length) {
            [NSFileManager.defaultManager removeItemAtPath:path error:nil];
        }
    }
};

struct ScopedMockServer {
    NSTask *task = nil;
    NSPipe *stdinPipe = nil;
    NSString *baseURL = nil;

    ScopedMockServer(NSString *tempDir) {
        NSString *scriptPath = [tempDir stringByAppendingPathComponent:@"mock_polish_http.py"];
        NSData *data = [[NSString stringWithUTF8String:kMockPolishHttpServerScript] dataUsingEncoding:NSUTF8StringEncoding];
        assert([data writeToFile:scriptPath atomically:YES]);
        assert(::chmod(scriptPath.fileSystemRepresentation, 0755) == 0);

        task = [NSTask new];
        task.executableURL = [NSURL fileURLWithPath:scriptPath];
        stdinPipe = [NSPipe pipe];
        NSPipe *stdoutPipe = [NSPipe pipe];
        task.standardInput = stdinPipe;
        task.standardOutput = stdoutPipe;
        task.standardError = NSFileHandle.fileHandleWithNullDevice;
        assert([task launchAndReturnError:nil]);

        NSMutableData *line = [NSMutableData data];
        NSFileHandle *reader = stdoutPipe.fileHandleForReading;
        while (true) {
            NSData *byte = [reader readDataOfLength:1];
            assert(byte.length == 1);
            if (*static_cast<const char *>(byte.bytes) == '\n') break;
            [line appendData:byte];
        }
        NSString *portStr = [[NSString alloc] initWithData:line encoding:NSUTF8StringEncoding];
        assert(portStr.integerValue > 0);
        baseURL = [NSString stringWithFormat:@"http://127.0.0.1:%@", portStr];
    }

    ~ScopedMockServer() {
        [stdinPipe.fileHandleForWriting closeAndReturnError:nil];
        if (task.running) {
            [task terminate];
            [task waitUntilExit];
        }
    }
};

void WaitMainRunLoop(NSTimeInterval timeoutSeconds, BOOL (^done)(void)) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeoutSeconds];
    while (!done() && deadline.timeIntervalSinceNow > 0.0) {
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.005]];
    }
    assert(done());
}

NSInteger FetchRedirectHitCount(NSString *baseURL) {
    NSURLSessionConfiguration *config = [NSURLSessionConfiguration ephemeralSessionConfiguration];
    config.connectionProxyDictionary = @{};
    NSURLSession *session = [NSURLSession sessionWithConfiguration:config];
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block NSInteger hits = -1;
    NSURL *url = [NSURL URLWithString:[baseURL stringByAppendingString:@"/redirect-hits"]];
    [[session dataTaskWithURL:url
            completionHandler:^(NSData * _Nullable data, NSURLResponse * _Nullable response, NSError * _Nullable error) {
                (void)response;
                if (!error && data) {
                    NSDictionary *json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
                    if ([json isKindOfClass:NSDictionary.class]) {
                        hits = [json[@"hits"] integerValue];
                    }
                }
                dispatch_semaphore_signal(sem);
            }] resume];
    assert(dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
    [session invalidateAndCancel];
    return hits;
}

MSIMEVoicePolishRequest *MakeBaseRequest(NSString *endpoint) {
    MSIMEVoicePolishRequest *request = [MSIMEVoicePolishRequest new];
    request.provider = @"ollama";
    request.endpoint = endpoint;
    request.model = @"synthetic-model";
    request.rawText = @"原始合成语音转写";
    request.fallbackText = @"确定性后处理兜底文本";
    return request;
}

void TestNormalNonStreamingAndMainThreadDelivery(MSIMEVoicePolishClient *client, NSString *baseURL) {
    MSIMEVoicePolishRequest *request = MakeBaseRequest([baseURL stringByAppendingString:@"/v1/chat/completions"]);
    __block MSIMEVoicePolishResult *captured = nil;
    __block BOOL onMainThread = NO;
    [client polishRequest:request
                 slowHint:nil
                  partial:nil
               completion:^(MSIMEVoicePolishResult *result) {
                   onMainThread = NSThread.isMainThread;
                   captured = result;
               }];
    WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
    assert(onMainThread);
    assert(!captured.polishFailed);
    assert(captured.polishRejectedReason == nil);
    assert([captured.text isEqualToString:@"合成润色后的标准文本。"]);
    assert([captured.resolvedAction isEqualToString:@"insert"]);
    assert(!captured.usedStreaming);
    NSDictionary *meta = [captured archiveMetadata];
    assert([meta[@"polish_failed"] isEqual:@NO]);
    assert(meta[@"polish_rejected_reason"] == nil);
    assert([meta[@"mode"] isEqualToString:@"dictation"]);
}

void TestThinkStripAndRejectionDegradation(MSIMEVoicePolishClient *client, NSString *baseURL) {
    // 1. 正常剥离 <think>...</think>。
    {
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/think"])
                     slowHint:nil
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(!captured.polishFailed);
        assert([captured.text isEqualToString:@"去除思考标签后的合成结果"]);
    }

    // 2. 未闭合 <think> 触发拒收并无损降级到 fallbackText。
    {
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/unclosed-think"])
                     slowHint:nil
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"unclosed_think"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
        assert([captured.resolvedAction isEqualToString:@"insert"]);
    }

    // 3. 助手元回复触发拒收并无损降级。
    {
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/meta-reply"])
                     slowHint:nil
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"assistant_meta_reply"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    }
}

void TestActionTagSelectionRouting(MSIMEVoicePolishClient *client, NSString *baseURL) {
    MSIMEVoicePolishContext *context = [MSIMEVoicePolishContext new];
    context.selectedText = @"旧的合成选区内容";

    // rewrite 标签 -> replace_selection。
    {
        MSIMEVoicePolishRequest *req = MakeBaseRequest([baseURL stringByAppendingString:@"/action-rewrite"]);
        req.enableSelectionRewrite = YES;
        req.context = context;
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req
                     slowHint:nil
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(!captured.polishFailed);
        assert([captured.resolvedAction isEqualToString:@"replace_selection"]);
        assert([captured.text isEqualToString:@"改写完成后的合成选区文本"]);
    }

    // dictate 标签 -> insert（避免误改偶然选中的内容）。
    {
        MSIMEVoicePolishRequest *req = MakeBaseRequest([baseURL stringByAppendingString:@"/action-dictate"]);
        req.enableSelectionRewrite = YES;
        req.context = context;
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req
                     slowHint:nil
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(!captured.polishFailed);
        assert([captured.resolvedAction isEqualToString:@"insert"]);
        assert([captured.text isEqualToString:@"仅作为普通听写的合成文本"]);
    }
}

void TestRedirectStrictlyBlocked(MSIMEVoicePolishClient *client, NSString *baseURL) {
    MSIMEVoicePolishRequest *req = MakeBaseRequest([baseURL stringByAppendingString:@"/redirect"]);
    req.token = @"synthetic-secret-token";
    __block MSIMEVoicePolishResult *captured = nil;
    [client polishRequest:req
                 slowHint:nil
                  partial:nil
               completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
    WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
    assert(captured.polishFailed);
    assert([captured.polishRejectedReason isEqualToString:@"redirect_blocked"]);
    assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    // 验证 302 目标端点从未被访问。
    assert(FetchRedirectHitCount(baseURL) == 0);
}

void TestSSEStreamingIncrementalPartial(MSIMEVoicePolishClient *client, NSString *baseURL) {
    MSIMEVoicePolishRequest *req = MakeBaseRequest([baseURL stringByAppendingString:@"/sse"]);
    req.stream = YES;
    NSMutableArray<NSString *> *partials = [NSMutableArray array];
    __block MSIMEVoicePolishResult *captured = nil;
    [client polishRequest:req
                 slowHint:nil
                  partial:^(NSString *accumulatedText) {
                      assert(NSThread.isMainThread);
                      [partials addObject:accumulatedText];
                  }
               completion:^(MSIMEVoicePolishResult *result) {
                   assert(NSThread.isMainThread);
                   captured = result;
               }];
    WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
    assert(!captured.polishFailed);
    assert(captured.usedStreaming);
    assert([captured.text isEqualToString:@"合成流式润色结果"]);
    assert(partials.count == 3);
    assert([partials[0] isEqualToString:@"合成"]);
    assert([partials[1] isEqualToString:@"合成流式"]);
    assert([partials[2] isEqualToString:@"合成流式润色结果"]);
}

void TestSlowHintAndCancellation(MSIMEVoicePolishClient *client, NSString *baseURL) {
    // 1. 慢响应触发主线程 slowHint 回调。
    client.slowHintDelay = 0.10;
    {
        __block NSString *hintReceived = nil;
        __block BOOL hintBeforeCompletion = NO;
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/slow"])
                     slowHint:^(NSString *message) {
                         assert(NSThread.isMainThread);
                         hintBeforeCompletion = (captured == nil);
                         hintReceived = message;
                     }
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *result) { captured = result; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(!captured.polishFailed);
        assert([captured.text isEqualToString:@"慢响应完成后的合成结果"]);
        assert([hintReceived isEqualToString:@"正在润色，请稍候…"]);
        assert(hintBeforeCompletion);
    }

    // 2. 取消在途任务后不得触发任何回调。
    {
        __block BOOL anyCallbackFired = NO;
        MSIMEVoicePolishTask *task = [client polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/timeout"])
                                                  slowHint:^(NSString *message) {
                                                      (void)message;
                                                      anyCallbackFired = YES;
                                                  }
                                                   partial:^(NSString *accumulated) {
                                                       (void)accumulated;
                                                       anyCallbackFired = YES;
                                                   }
                                                completion:^(MSIMEVoicePolishResult *result) {
                                                    (void)result;
                                                    anyCallbackFired = YES;
                                                }];
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.03]];
        [task cancel];
        assert(task.cancelled);
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.35]];
        assert(!anyCallbackFired);
    }
    client.slowHintDelay = 7.5;
}

void TestTimeoutHttp500OversizedAndGuardDegradation(MSIMEVoicePolishClient *client, NSString *baseURL) {
    // 1. 请求超时降级。
    {
        MSIMEVoicePolishRequest *req = MakeBaseRequest([baseURL stringByAppendingString:@"/timeout"]);
        req.promptID = @"test_short_timeout";
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req slowHint:nil partial:nil completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"timeout"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    }

    // 2. HTTP 500 降级。
    {
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/http-500"])
                     slowHint:nil
                      partial:nil
                   completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"http_500"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    }

    // 3. 响应体积超限拦截。
    {
        MSIMEVoicePolishRequest *req = MakeBaseRequest([baseURL stringByAppendingString:@"/oversized"]);
        req.promptID = @"test_small_cap";
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req slowHint:nil partial:nil completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"response_too_large"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    }

    // 4. 密码框拦截公网端点。
    {
        MSIMEVoicePolishRequest *req = MakeBaseRequest(@"https://example.invalid/v1/chat/completions");
        req.provider = @"openai";
        req.token = @"synthetic-cloud-token";
        MSIMEVoicePolishContext *ctx = [MSIMEVoicePolishContext new];
        ctx.isSecureField = YES;
        req.context = ctx;
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req slowHint:nil partial:nil completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"polish_secure_field_blocked"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    }

    // 5. 桥接表缺失时优雅降级。
    {
        MSIMEVoicePolishBridge emptyBridge = {};
        MSIMEVoicePolishClient *unwired = [[MSIMEVoicePolishClient alloc] initWithBridge:emptyBridge localServer:nil];
        __block MSIMEVoicePolishResult *captured = nil;
        [unwired polishRequest:MakeBaseRequest([baseURL stringByAppendingString:@"/v1/chat/completions"])
                      slowHint:nil
                       partial:nil
                    completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"bridge_unavailable"]);
        assert([captured.text isEqualToString:@"确定性后处理兜底文本"]);
    }
}

void TestEndToEndWithLocalPolishServerAndCrashRecovery(const ScopedTempDir &tempDir) {
    NSString *scriptPath = [tempDir.path stringByAppendingPathComponent:@"fake_local_llama.py"];
    NSData *scriptData = [[NSString stringWithUTF8String:kFakeLocalLlamaForClientScript] dataUsingEncoding:NSUTF8StringEncoding];
    assert([scriptData writeToFile:scriptPath atomically:YES]);
    assert(::chmod(scriptPath.fileSystemRepresentation, 0755) == 0);

    NSString *dropMarker = [tempDir.path stringByAppendingPathComponent:@"dropped-once.marker"];
    NSString *wrapperPath = [tempDir.path stringByAppendingPathComponent:@"fake-local-wrapper.sh"];
    NSString *wrapperBody = [NSString stringWithFormat:@"#!/bin/sh\nexport FAKE_DROP_ONCE_MARKER='%@'\nexec '%@' \"$@\"\n",
                                                       dropMarker, scriptPath];
    assert([[wrapperBody dataUsingEncoding:NSUTF8StringEncoding] writeToFile:wrapperPath atomically:YES]);
    assert(::chmod(wrapperPath.fileSystemRepresentation, 0755) == 0);

    NSString *modelDir = [tempDir.path stringByAppendingPathComponent:@"local-models"];
    assert([NSFileManager.defaultManager createDirectoryAtPath:modelDir
                                   withIntermediateDirectories:YES
                                                    attributes:nil
                                                         error:nil]);
    NSString *modelPath = [modelDir stringByAppendingPathComponent:@"Qwen-Synthetic-9B.gguf"];
    assert([[@"synthetic-gguf" dataUsingEncoding:NSUTF8StringEncoding] writeToFile:modelPath atomically:YES]);

    MSIMELocalPolishServerConfiguration *serverConfig = [MSIMELocalPolishServerConfiguration new];
    serverConfig.executablePath = wrapperPath;
    serverConfig.modelDirectory = modelDir;
    serverConfig.healthTimeout = 8.0;
    serverConfig.healthPollInterval = 0.03;
    serverConfig.listenToSystemMemoryPressure = NO;
    serverConfig.listenToApplicationTermination = NO;
    MSIMELocalPolishServer *localServer = [[MSIMELocalPolishServer alloc] initWithConfiguration:serverConfig];

    MSIMEVoicePolishClient *client = [[MSIMEVoicePolishClient alloc] initWithBridge:MakeSyntheticBridge()
                                                                        localServer:localServer];

    // 1. 首次请求自动拉起本机服务并携带一次性内存密钥完成润色。
    {
        MSIMEVoicePolishRequest *req = [MSIMEVoicePolishRequest new];
        req.provider = @"local";
        req.rawText = @"原始合成输入";
        req.fallbackText = @"本地兜底文本";
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req slowHint:nil partial:nil completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(8.0, ^BOOL { return captured != nil; });
        assert(!captured.polishFailed);
        assert([captured.text isEqualToString:@"本地模型润色结果"]);
    }
    const pid_t firstPid = localServer.processIdentifier;
    assert(firstPid > 0);

    // 2. 模拟子进程在推理中途崩溃断连，验证客户端自动触发单次重启重试并成功完成润色。
    {
        MSIMEVoicePolishRequest *req = [MSIMEVoicePolishRequest new];
        req.provider = @"local";
        req.rawText = @"trigger_drop_once 触发单次断连重拉";
        req.fallbackText = @"本地兜底文本";
        __block MSIMEVoicePolishResult *captured = nil;
        [client polishRequest:req slowHint:nil partial:nil completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(8.0, ^BOOL { return captured != nil; });
        assert(!captured.polishFailed);
        assert([captured.text isEqualToString:@"本地模型润色结果"]);
        assert(localServer.processIdentifier != firstPid);
        assert(localServer.recentRestartCount == 1);
    }

    [localServer stopSynchronously];

    // 3. 本机服务不可用（无模型/无二进制）时自动降级，绝不丢弃用户语音。
    {
        MSIMELocalPolishServerConfiguration *brokenConfig = [MSIMELocalPolishServerConfiguration new];
        brokenConfig.executablePath = [tempDir.path stringByAppendingPathComponent:@"missing-llama-server"];
        brokenConfig.modelDirectory = modelDir;
        brokenConfig.listenToSystemMemoryPressure = NO;
        brokenConfig.listenToApplicationTermination = NO;
        MSIMELocalPolishServer *brokenServer = [[MSIMELocalPolishServer alloc] initWithConfiguration:brokenConfig];
        MSIMEVoicePolishClient *brokenClient = [[MSIMEVoicePolishClient alloc] initWithBridge:MakeSyntheticBridge()
                                                                                  localServer:brokenServer];

        MSIMEVoicePolishRequest *req = [MSIMEVoicePolishRequest new];
        req.provider = @"local";
        req.rawText = @"原始合成语音";
        req.fallbackText = @"后处理保底文本";
        __block MSIMEVoicePolishResult *captured = nil;
        [brokenClient polishRequest:req slowHint:nil partial:nil completion:^(MSIMEVoicePolishResult *r) { captured = r; }];
        WaitMainRunLoop(5.0, ^BOOL { return captured != nil; });
        assert(captured.polishFailed);
        assert([captured.polishRejectedReason isEqualToString:@"local_server_unavailable"]);
        assert([captured.text isEqualToString:@"后处理保底文本"]);
        assert([captured.resolvedAction isEqualToString:@"insert"]);
    }
}

} // namespace

int main(void) {
    @autoreleasepool {
        ScopedTempDir tempDir;
        ScopedMockServer mockServer(tempDir.path);
        MSIMEVoicePolishClient *client = [[MSIMEVoicePolishClient alloc] initWithBridge:MakeSyntheticBridge()
                                                                            localServer:nil];

        TestNormalNonStreamingAndMainThreadDelivery(client, mockServer.baseURL);
        TestThinkStripAndRejectionDegradation(client, mockServer.baseURL);
        TestActionTagSelectionRouting(client, mockServer.baseURL);
        TestRedirectStrictlyBlocked(client, mockServer.baseURL);
        TestSSEStreamingIncrementalPartial(client, mockServer.baseURL);
        TestSlowHintAndCancellation(client, mockServer.baseURL);
        TestTimeoutHttp500OversizedAndGuardDegradation(client, mockServer.baseURL);
        TestEndToEndWithLocalPolishServerAndCrashRecovery(tempDir);
    }
    return 0;
}
