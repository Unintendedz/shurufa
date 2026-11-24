// Shurufa v2 Tier 1 独立端侧神经解码后台服务（`Shurufa.app/Contents/Helpers/ShurufaNeuralService`）。
// 职责：
// 1. 隔离 Apple Metal 显存占用与潜在驱动崩溃，使输入法主进程永不因大模型或 GPU 异常掉键或退出；
// 2. 通过本机仅同 UID 可连的 `0600` Unix 域套接字接收 `Prefill`/`Decode`/`Health`/`Unload`/`Shutdown` 帧；
// 3. 10 分钟空闲自动卸载模型释放 100% 统一内存、响应 `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` 优雅卸载、
//    监听父进程退出事件防止孤儿进程驻留。

#import <Foundation/Foundation.h>
#import <dispatch/dispatch.h>
#import <os/log.h>

#include "ShurufaNeuralEngine.h"
#include "ShurufaNeuralJson.h"
#include "ShurufaNeuralProtocol.h"

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

using namespace shurufa::neural::ipc;
using shurufa::neural::JsonValue;
using shurufa::neural::dump_json;
using shurufa::neural::parse_json;

os_log_t NeuralServiceLog(void) {
    static os_log_t log = nullptr;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        log = os_log_create("dev.shurufa.ime", "neural-service");
    });
    return log;
}

struct ServiceOptions {
    std::string socket_path;
    std::string model_path;
    std::string model_dir;
    std::string pinyin_db_path;
    double idle_unload_seconds = 600.0;
    pid_t parent_pid = 0;
    bool test_mock = false;
};

struct ServiceState {
    ServiceOptions opts;
    dispatch_queue_t inference_queue = nullptr;
    void *engine = nullptr;
    std::string active_model_path;
    uint64_t last_activity_ns = 0;
    uint64_t pressure_cooldown_until_ns = 0;
    std::atomic<int> listen_fd{-1};
    std::atomic<bool> stopping{false};
    std::string mock_prefilled_context;
    bool mock_loaded = false;
};

std::string DefaultModelDirectory(void) {
    @autoreleasepool {
        NSString *dir = [@"~/Library/Application Support/Shurufa/resource-packs/neural-lm" stringByExpandingTildeInPath];
        return dir.UTF8String ? std::string(dir.UTF8String) : std::string();
    }
}

void UnloadEngineOnInferenceQueue(ServiceState *state, const char *reason) {
    if (!state) return;
    if (state->opts.test_mock) {
        if (state->mock_loaded) {
            state->mock_loaded = false;
            state->mock_prefilled_context.clear();
            os_log_info(NeuralServiceLog(), "neural_service_unload mock=1 reason=%{public}s", reason);
        }
        return;
    }
    if (state->engine) {
        shurufa_neural_destroy(state->engine);
        state->engine = nullptr;
        state->active_model_path.clear();
        os_log_info(NeuralServiceLog(), "neural_service_unload mock=0 reason=%{public}s", reason);
    }
}

bool EnsureEngineLoadedOnInferenceQueue(ServiceState *state, std::string &error_out) {
    if (!state) {
        error_out = "service_uninitialized";
        return false;
    }
    const uint64_t now = MonotonicNowNs();
    if (state->pressure_cooldown_until_ns > now) {
        error_out = "memory_pressure_cooldown";
        return false;
    }
    if (state->opts.test_mock) {
        state->mock_loaded = true;
        state->last_activity_ns = now;
        return true;
    }
    if (state->engine) {
        state->last_activity_ns = now;
        return true;
    }

    std::string resolved_model = state->opts.model_path;
    if (resolved_model.empty()) {
        const std::string dir = state->opts.model_dir.empty() ? DefaultModelDirectory() : state->opts.model_dir;
        char *discovered = shurufa_neural_discover_model_path(
            dir.empty() ? nullptr : dir.c_str(), dir.size());
        if (discovered) {
            resolved_model.assign(discovered);
            shurufa_neural_string_free(discovered);
        }
    }
    if (resolved_model.empty()) {
        error_out = "model_not_found";
        return false;
    }

    JsonValue cfg = JsonValue::make_object();
    cfg.set("model_path", JsonValue::make_string(resolved_model));
    if (!state->opts.pinyin_db_path.empty()) {
        cfg.set("pinyin_db_path", JsonValue::make_string(state->opts.pinyin_db_path));
    }
    cfg.set("n_ctx", JsonValue::make_int(1024));
    cfg.set("n_batch", JsonValue::make_int(512));
    cfg.set("n_seq_max", JsonValue::make_int(20));
    cfg.set("n_gpu_layers", JsonValue::make_int(99));
    cfg.set("n_threads", JsonValue::make_int(4));
    const std::string cfg_str = dump_json(cfg);

    const uint64_t load_start = MonotonicNowNs();
    state->engine = shurufa_neural_create(cfg_str.data(), cfg_str.size());
    const double load_ms = static_cast<double>(MonotonicNowNs() - load_start) / 1e6;
    if (!state->engine) {
        os_log_error(NeuralServiceLog(), "neural_service_load_failed elapsed_ms=%.1f", load_ms);
        error_out = "engine_load_failed";
        return false;
    }
    state->active_model_path = resolved_model;
    state->last_activity_ns = MonotonicNowNs();
    os_log_info(NeuralServiceLog(), "neural_service_loaded elapsed_ms=%.1f", load_ms);
    return true;
}

/// 测试专用确定性模拟响应（仅在显式传入 `--test-mock` 时启用，用于无 378 MB GGUF 模型的 CTest IPC 单测）。
std::string HandleMockDecode(ServiceState *state, const std::string &request_json, bool &ok_out) {
    JsonValue parsed;
    if (!parse_json(request_json.data(), request_json.size(), parsed) || !parsed.is_object()) {
        ok_out = false;
        return "{\"ok\":false,\"error\":\"invalid_request_json\"}";
    }
    const int64_t generation = parsed.get_i64("generation", 0);
    JsonValue root = JsonValue::make_object();
    root.set("ok", JsonValue::make_bool(true));
    root.set("generation", JsonValue::make_int(generation));

    JsonValue scores = JsonValue::make_array();
    if (const JsonValue *cands = parsed.find("candidates"); cands && cands->is_array()) {
        const size_t n = cands->arr_val.size();
        for (size_t i = 0; i < n; ++i) {
            const JsonValue &item = cands->arr_val[i];
            std::string text;
            if (item.is_object()) {
                text = item.get_string("text");
            } else if (item.is_string()) {
                text = item.str_val;
            }
            if (text.empty()) continue;
            JsonValue entry = JsonValue::make_object();
            entry.set("text", JsonValue::make_string(text));
            // 确定性合成分数：略微偏好靠后的候选以供端到端验证重排协议可达。
            const double synth_lp = -10.0 + static_cast<double>(i) * 0.5;
            entry.set("neural_log_prob", JsonValue::make_number(synth_lp));
            entry.set("token_count", JsonValue::make_int(2));
            scores.push(std::move(entry));
        }
    }
    root.set("candidate_scores", std::move(scores));
    root.set("beam_candidates", JsonValue::make_array());
    root.set("prefill_ms", JsonValue::make_number(0.05));
    root.set("score_ms", JsonValue::make_number(0.20));
    root.set("beam_ms", JsonValue::make_number(0.0));
    root.set("total_ms", JsonValue::make_number(0.25));
    root.set("context_cached", JsonValue::make_bool(!state->mock_prefilled_context.empty()));
    ok_out = true;
    return dump_json(root);
}

void RequestShutdown(ServiceState *state, const char *reason) {
    if (!state) return;
    bool expected = false;
    if (!state->stopping.compare_exchange_strong(expected, true)) return;
    os_log_info(NeuralServiceLog(), "neural_service_shutdown reason=%{public}s", reason);
    const int fd = state->listen_fd.exchange(-1, std::memory_order_acq_rel);
    if (fd >= 0) {
        ::shutdown(fd, SHUT_RDWR);
        ::close(fd);
    }
    if (!state->opts.socket_path.empty()) {
        ::unlink(state->opts.socket_path.c_str());
    }
    dispatch_async(state->inference_queue, ^{
        UnloadEngineOnInferenceQueue(state, "shutdown");
        std::_Exit(0);
    });
}

void HandleClientConnection(ServiceState *state, int client_fd) {
    ConfigureSocketNoSigPipe(client_fd);

    // 严格核验对端有效 UID 与当前进程一致，防止同机其他用户接入推理套接字。
    uid_t peer_euid = 0;
    gid_t peer_egid = 0;
    if (::getpeereid(client_fd, &peer_euid, &peer_egid) != 0 || peer_euid != ::geteuid()) {
        ::close(client_fd);
        return;
    }

    while (!state->stopping.load(std::memory_order_relaxed)) {
        FrameHeader req_header{};
        std::string req_payload;
        if (!ReadFrame(client_fd, req_header, req_payload, 0)) {
            break;
        }
        const OpCode op = static_cast<OpCode>(req_header.op);
        __block std::string resp_payload;
        __block bool resp_ok = false;
        __block bool should_shutdown = false;

        dispatch_sync(state->inference_queue, ^{
            @autoreleasepool {
                switch (op) {
                    case OpCode::Health: {
                        const bool loaded = state->opts.test_mock ? state->mock_loaded : (state->engine != nullptr);
                        JsonValue root = JsonValue::make_object();
                        root.set("ok", JsonValue::make_bool(true));
                        root.set("loaded", JsonValue::make_bool(loaded));
                        root.set("mock", JsonValue::make_bool(state->opts.test_mock));
                        root.set("pid", JsonValue::make_int(static_cast<int64_t>(::getpid())));
                        resp_payload = dump_json(root);
                        resp_ok = true;
                        break;
                    }
                    case OpCode::Prefill: {
                        std::string err;
                        if (!EnsureEngineLoadedOnInferenceQueue(state, err)) {
                            JsonValue root = JsonValue::make_object();
                            root.set("ok", JsonValue::make_bool(false));
                            root.set("error", JsonValue::make_string(err));
                            resp_payload = dump_json(root);
                            resp_ok = false;
                            break;
                        }
                        if (state->opts.test_mock) {
                            state->mock_prefilled_context = req_payload;
                            resp_payload = "{\"ok\":true,\"prefill_ms\":0.05,\"cached_tokens\":1}";
                            resp_ok = true;
                            break;
                        }
                        char *raw = shurufa_neural_prefill_context(state->engine,
                                                                   req_payload.data(),
                                                                   req_payload.size());
                        if (raw) {
                            resp_payload.assign(raw);
                            shurufa_neural_string_free(raw);
                            resp_ok = true;
                        } else {
                            resp_payload = "{\"ok\":false,\"error\":\"prefill_failed\"}";
                            resp_ok = false;
                        }
                        state->last_activity_ns = MonotonicNowNs();
                        break;
                    }
                    case OpCode::Decode: {
                        std::string err;
                        if (!EnsureEngineLoadedOnInferenceQueue(state, err)) {
                            JsonValue root = JsonValue::make_object();
                            root.set("ok", JsonValue::make_bool(false));
                            root.set("error", JsonValue::make_string(err));
                            resp_payload = dump_json(root);
                            resp_ok = false;
                            break;
                        }
                        if (state->opts.test_mock) {
                            resp_payload = HandleMockDecode(state, req_payload, resp_ok);
                            break;
                        }
                        char *raw = shurufa_neural_process_request(state->engine,
                                                                   req_payload.data(),
                                                                   req_payload.size());
                        if (raw) {
                            resp_payload.assign(raw);
                            shurufa_neural_string_free(raw);
                            resp_ok = true;
                        } else {
                            resp_payload = "{\"ok\":false,\"error\":\"decode_failed\"}";
                            resp_ok = false;
                        }
                        state->last_activity_ns = MonotonicNowNs();
                        break;
                    }
                    case OpCode::Unload: {
                        UnloadEngineOnInferenceQueue(state, "rpc_unload");
                        resp_payload = "{\"ok\":true}";
                        resp_ok = true;
                        break;
                    }
                    case OpCode::Shutdown: {
                        resp_payload = "{\"ok\":true}";
                        resp_ok = true;
                        should_shutdown = true;
                        break;
                    }
                    default: {
                        resp_payload = "{\"ok\":false,\"error\":\"unknown_op\"}";
                        resp_ok = false;
                        break;
                    }
                }
            }
        });

        const uint16_t flags = resp_ok ? kFlagOk : 0U;
        if (!WriteFrame(client_fd, op, flags, req_header.request_id, resp_payload.data(), resp_payload.size(), 5000)) {
            break;
        }
        if (should_shutdown) {
            RequestShutdown(state, "rpc_shutdown");
            break;
        }
    }

    ::close(client_fd);
}

int CreateListeningSocket(const std::string &socket_path) {
    if (socket_path.empty() || socket_path.size() >= sizeof(sockaddr_un::sun_path)) {
        return -1;
    }
    ::unlink(socket_path.c_str());

    const int fd = ::socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    ConfigureSocketNoSigPipe(fd);

    struct sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    std::memcpy(addr.sun_path, socket_path.c_str(), socket_path.size() + 1);

    const mode_t old_umask = ::umask(0177);
    const int bind_rc = ::bind(fd, reinterpret_cast<struct sockaddr *>(&addr), sizeof(addr));
    ::umask(old_umask);
    if (bind_rc != 0) {
        ::close(fd);
        return -1;
    }
    (void)::chmod(socket_path.c_str(), 0600);
    if (::listen(fd, 8) != 0) {
        ::close(fd);
        ::unlink(socket_path.c_str());
        return -1;
    }
    return fd;
}

bool ParseArguments(int argc, const char *argv[], ServiceOptions &opts) {
    for (int i = 1; i < argc; ++i) {
        const std::string arg = argv[i];
        auto next_value = [&](std::string &out) -> bool {
            if (i + 1 >= argc) return false;
            out = argv[++i];
            return true;
        };
        if (arg == "--socket") {
            if (!next_value(opts.socket_path)) return false;
        } else if (arg == "--model") {
            if (!next_value(opts.model_path)) return false;
        } else if (arg == "--model-dir") {
            if (!next_value(opts.model_dir)) return false;
        } else if (arg == "--pinyin-db") {
            if (!next_value(opts.pinyin_db_path)) return false;
        } else if (arg == "--idle-seconds") {
            std::string val;
            if (!next_value(val)) return false;
            const double parsed = std::strtod(val.c_str(), nullptr);
            if (std::isfinite(parsed) && parsed > 0.0) {
                opts.idle_unload_seconds = parsed;
            }
        } else if (arg == "--parent-pid") {
            std::string val;
            if (!next_value(val)) return false;
            opts.parent_pid = static_cast<pid_t>(std::strtol(val.c_str(), nullptr, 10));
        } else if (arg == "--test-mock") {
            opts.test_mock = true;
        }
    }
    return !opts.socket_path.empty();
}

} // namespace

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        ::signal(SIGPIPE, SIG_IGN);

        static ServiceState state;
        if (!ParseArguments(argc, argv, state.opts)) {
            std::fprintf(stderr, "Usage: ShurufaNeuralService --socket <path> [--model <gguf>] [--model-dir <dir>] [--pinyin-db <db>] [--idle-seconds <sec>] [--parent-pid <pid>]\n");
            return 2;
        }

        dispatch_queue_attr_t inf_attr = dispatch_queue_attr_make_with_qos_class(
            DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0);
        state.inference_queue = dispatch_queue_create("dev.shurufa.neural-service.inference", inf_attr);
        state.last_activity_ns = MonotonicNowNs();

        const int listen_fd = CreateListeningSocket(state.opts.socket_path);
        state.listen_fd.store(listen_fd, std::memory_order_release);
        if (listen_fd < 0) {
            os_log_error(NeuralServiceLog(), "neural_service_bind_failed errno=%d", errno);
            return 1;
        }

        ::signal(SIGTERM, SIG_IGN);
        ::signal(SIGINT, SIG_IGN);
        dispatch_source_t sigterm = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGTERM, 0, dispatch_get_main_queue());
        dispatch_source_set_event_handler(sigterm, ^{ RequestShutdown(&state, "sigterm"); });
        dispatch_resume(sigterm);

        dispatch_source_t sigint = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGINT, 0, dispatch_get_main_queue());
        dispatch_source_set_event_handler(sigint, ^{ RequestShutdown(&state, "sigint"); });
        dispatch_resume(sigint);

        dispatch_source_t parent_watch = nullptr;
        if (state.opts.parent_pid > 1) {
            parent_watch = dispatch_source_create(
                DISPATCH_SOURCE_TYPE_PROC,
                static_cast<uintptr_t>(state.opts.parent_pid),
                DISPATCH_PROC_EXIT,
                dispatch_get_main_queue());
            if (parent_watch) {
                dispatch_source_set_event_handler(parent_watch, ^{ RequestShutdown(&state, "parent_exited"); });
                dispatch_resume(parent_watch);
            }
            if (::kill(state.opts.parent_pid, 0) != 0 && errno == ESRCH) {
                RequestShutdown(&state, "parent_already_gone");
            }
        }

        dispatch_source_t mem_pressure = dispatch_source_create(
            DISPATCH_SOURCE_TYPE_MEMORYPRESSURE,
            0,
            DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL,
            state.inference_queue);
        if (mem_pressure) {
            dispatch_source_set_event_handler(mem_pressure, ^{
                const uintptr_t flags = dispatch_source_get_data(mem_pressure);
                const bool critical = (flags & DISPATCH_MEMORYPRESSURE_CRITICAL) != 0;
                UnloadEngineOnInferenceQueue(&state, critical ? "memory_pressure_critical" : "memory_pressure_warn");
                if (critical) {
                    state.pressure_cooldown_until_ns = MonotonicNowNs() + 15ULL * 1000000000ULL;
                }
            });
            dispatch_resume(mem_pressure);
        }

        const double check_interval_sec = state.opts.idle_unload_seconds < 30.0
            ? (state.opts.idle_unload_seconds * 0.5 > 0.2 ? state.opts.idle_unload_seconds * 0.5 : 0.2)
            : 30.0;
        dispatch_source_t idle_timer = dispatch_source_create(
            DISPATCH_SOURCE_TYPE_TIMER, 0, 0, state.inference_queue);
        if (idle_timer) {
            const uint64_t interval_ns = static_cast<uint64_t>(check_interval_sec * 1e9);
            dispatch_source_set_timer(idle_timer,
                                      dispatch_time(DISPATCH_TIME_NOW, static_cast<int64_t>(interval_ns)),
                                      interval_ns,
                                      interval_ns / 4);
            dispatch_source_set_event_handler(idle_timer, ^{
                if (state.opts.parent_pid > 1 && ::getppid() == 1 &&
                    (::kill(state.opts.parent_pid, 0) != 0 && errno == ESRCH)) {
                    RequestShutdown(&state, "parent_reaped");
                    return;
                }
                const bool loaded = state.opts.test_mock ? state.mock_loaded : (state.engine != nullptr);
                if (!loaded) return;
                const uint64_t idle_ns = MonotonicNowNs() - state.last_activity_ns;
                const uint64_t limit_ns = static_cast<uint64_t>(state.opts.idle_unload_seconds * 1e9);
                if (idle_ns >= limit_ns) {
                    UnloadEngineOnInferenceQueue(&state, "idle_timeout");
                }
            });
            dispatch_resume(idle_timer);
        }

        dispatch_queue_t accept_queue = dispatch_queue_create("dev.shurufa.neural-service.accept", DISPATCH_QUEUE_SERIAL);
        dispatch_queue_t conn_queue = dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0);
        dispatch_async(accept_queue, ^{
            while (!state.stopping.load(std::memory_order_relaxed)) {
                const int listen_fd = state.listen_fd.load(std::memory_order_acquire);
                if (listen_fd < 0) break;
                const int client_fd = ::accept(listen_fd, nullptr, nullptr);
                if (client_fd < 0) {
                    if (errno == EINTR) continue;
                    if (state.stopping.load(std::memory_order_relaxed)) break;
                    usleep(10 * 1000);
                    continue;
                }
                dispatch_async(conn_queue, ^{
                    HandleClientConnection(&state, client_fd);
                });
            }
        });

        os_log_info(NeuralServiceLog(), "neural_service_ready pid=%d mock=%d idle_sec=%.0f",
                    static_cast<int>(::getpid()), state.opts.test_mock ? 1 : 0, state.opts.idle_unload_seconds);

        dispatch_main();
    }
    return 0;
}
