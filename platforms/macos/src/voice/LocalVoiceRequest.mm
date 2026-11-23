#import "LocalVoiceRequest.h"
#import "../core/EditionIdentity.h"
#import "VoiceFailureMessages.h"
#include "msime_client.h"
#include "../../../common/HostApiString.h"
#import <os/log.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <cctype>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <fcntl.h>
#include <filesystem>
#include <iterator>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

// 宿主主程序与控制器测试会链接 DiagnosticLog.cpp 的强定义；独立请求单测不链接该文件时，
// 由此处的弱定义兜底，确保零未定义符号且无需改动 CMakeLists.txt。
__attribute__((weak)) void msime_macos_diagnostic_write(std::string_view event) noexcept;
__attribute__((weak)) void msime_macos_diagnostic_write(std::string_view event) noexcept {
    (void)event;
}
__attribute__((weak)) void msime_macos_diagnostic_host_line(const char *line) noexcept;
__attribute__((weak)) void msime_macos_diagnostic_host_line(const char *line) noexcept {
    (void)line;
}

namespace {
NSString *const LocalVoiceDomain = @"app.msime.client.voice.local";
// 协议单行上限：正常音频帧或句子只有数 KB，超长行说明子进程状态异常。
constexpr NSUInteger MaximumHelperLine = 4 * 1024 * 1024;
// 安装器拒绝超过 64 KiB 的模型清单；读取时也必须限制累计字节数，避免检查后文件膨胀。
constexpr NSUInteger MaximumModelManifestBytes = 64 * 1024;
// 子进程 stderr 诊断环形缓冲上限：保留最近 32 行、每行不超过 256 字节。
constexpr size_t kStderrRingCapacity = 32;
constexpr size_t kStderrMaxLineBytes = 256;
constexpr size_t kStderrPendingCap = 1024;
// 按本机物理内存分级设定 TextStream 模型空闲驻留时长（>= 32 GiB 为 900s，否则 180s）。
constexpr uint64_t kHighMemoryThresholdBytes = 32ULL * 1024ULL * 1024ULL * 1024ULL;

NSError *LocalVoiceFailure(NSString *detail = nil) {
    NSMutableDictionary *info = [@{NSLocalizedDescriptionKey : @"本地语音识别失败，请检查模型或重试"} mutableCopy];
    if (detail.length) info[MSIMEVoiceFailureDetailKey] = detail;
    return [NSError errorWithDomain:LocalVoiceDomain code:1 userInfo:info];
}

// 线程安全的子进程 stderr 32 行环形缓冲：写入前剥离用户 home 路径前缀并清理控制字符，绝不记录用户语音文本。
class StderrRingBuffer {
public:
    explicit StderrRingBuffer(std::string homePrefix)
        : homePrefix_(std::move(homePrefix)) {}

    void append(const char *bytes, size_t count) {
        if (!bytes || count == 0) return;
        std::lock_guard<std::mutex> lock(mutex_);
        for (size_t i = 0; i < count; ++i) {
            const char ch = bytes[i];
            if (ch == '\n') {
                commitPendingLocked();
                droppingLine_ = false;
                continue;
            }
            if (droppingLine_) continue;
            if (pending_.size() < kStderrPendingCap) {
                pending_.push_back(ch);
            } else {
                droppingLine_ = true;
            }
        }
    }

    std::vector<std::string> drainSnapshot() {
        std::lock_guard<std::mutex> lock(mutex_);
        commitPendingLocked();
        droppingLine_ = false;
        std::vector<std::string> out(lines_.begin(), lines_.end());
        lines_.clear();
        return out;
    }

private:
    std::string sanitizeLineLocked(const std::string &raw) const {
        std::string text;
        text.reserve(std::min(raw.size(), kStderrPendingCap));
        for (char c : raw) {
            if (c == '\r') continue;
            const auto byte = static_cast<unsigned char>(c);
            text.push_back((byte < 0x20 || byte == 0x7F) ? ' ' : c);
        }
        // 剥离用户 home 目录前缀，避免将本机用户名带入任何本地诊断缓冲。
        if (homePrefix_.size() > 1) {
            size_t pos = 0;
            while ((pos = text.find(homePrefix_, pos)) != std::string::npos) {
                text.replace(pos, homePrefix_.size(), "~");
                pos += 1;
            }
        }
        const size_t first = text.find_first_not_of(' ');
        if (first == std::string::npos) return {};
        const size_t last = text.find_last_not_of(' ');
        text = text.substr(first, last - first + 1);
        if (text.size() > kStderrMaxLineBytes) {
            size_t cut = kStderrMaxLineBytes;
            while (cut > 0 && (static_cast<unsigned char>(text[cut]) & 0xC0) == 0x80) --cut;
            text.resize(cut);
        }
        return text;
    }

    void commitPendingLocked() {
        if (pending_.empty()) return;
        std::string clean = sanitizeLineLocked(pending_);
        pending_.clear();
        if (clean.empty()) return;
        if (lines_.size() >= kStderrRingCapacity) lines_.pop_front();
        lines_.push_back(std::move(clean));
    }

    const std::string homePrefix_;
    std::mutex mutex_;
    std::deque<std::string> lines_;
    std::string pending_;
    bool droppingLine_ = false;
};

// 将脱敏 stderr 归纳为固定安全错误分类码（diagnostic.log 契约禁止写入路径或自由文本）。
const char *ClassifyStderrKind(const std::vector<std::string> &lines) {
    if (lines.empty()) return "none";
    bool sawTraceback = false;
    for (const std::string &line : lines) {
        std::string lower;
        lower.reserve(line.size());
        for (unsigned char ch : line) lower.push_back(static_cast<char>(std::tolower(ch)));
        if (lower.find("metal") != std::string::npos ||
            lower.find("mlx::core") != std::string::npos ||
            lower.find("command buffer") != std::string::npos ||
            lower.find("out of memory") != std::string::npos) {
            return "metal_error";
        }
        if (lower.find("modulenotfounderror") != std::string::npos ||
            lower.find("importerror") != std::string::npos ||
            lower.find("no module named") != std::string::npos ||
            lower.find("dyld") != std::string::npos ||
            lower.find("dlopen") != std::string::npos) {
            return "import_error";
        }
        if (lower.find("safetensors") != std::string::npos ||
            lower.find("model files are missing") != std::string::npos ||
            lower.find("runtime unavailable") != std::string::npos ||
            lower.find("config.json") != std::string::npos) {
            return "model_error";
        }
        if (lower.find("traceback (most recent call last)") != std::string::npos ||
            lower.find("runtimeerror:") != std::string::npos ||
            lower.find("valueerror:") != std::string::npos ||
            lower.find("oserror:") != std::string::npos) {
            sawTraceback = true;
        }
    }
    return sawTraceback ? "traceback" : "other";
}

// One host-api JSON call: the `value` of an `{"ok":true}` response, or nil for a failed call or a request that is not JSON.
id HostValue(char *(*function)(const uint8_t *, size_t), NSDictionary *request) {
    if (![NSJSONSerialization isValidJSONObject:request]) return nil;
    NSData *body = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    if (!body) return nil;
    auto raw = msime::host_api::own_string(
        function(static_cast<const uint8_t *>(body.bytes), body.length));
    if (!raw) return nil;
    id response = [NSJSONSerialization JSONObjectWithData:[NSData dataWithBytesNoCopy:raw.get() length:std::strlen(raw.get()) freeWhenDone:NO]
                                                  options:0 error:nil];
    if (![response isKindOfClass:NSDictionary.class] || ![response[@"ok"] isEqual:@YES]) return nil;
    return response[@"value"];
}

// 用户自己的词库词，权重高的在前，按 client-providers 的挑选方式取出。词库正忙或读不出来时，代价只是本次会话没有热词。
NSArray<NSDictionary *> *FetchHotwords(NSDictionary *hostOptions) {
    id value = HostValue(msime_client_voice_hotwords, @{@"options" : hostOptions, @"limit" : @200});
    id words = [value isKindOfClass:NSDictionary.class] ? value[@"hotwords"] : nil;
    if (![words isKindOfClass:NSArray.class]) return @[];
    NSMutableArray<NSDictionary *> *hotwords = [NSMutableArray array];
    for (id word in words)
        if ([word isKindOfClass:NSDictionary.class] && [word[@"text"] isKindOfClass:NSString.class] && [word[@"text"] length] &&
            [word[@"pinyin"] isKindOfClass:NSString.class])
            [hotwords addObject:@{@"text" : word[@"text"], @"pinyin" : word[@"pinyin"]}];
    return hotwords;
}

NSString *CorrectedText(NSString *text, NSArray<NSDictionary *> *hotwords) {
    id value = HostValue(msime_client_voice_hotword_correct, @{@"text" : text, @"hotwords" : hotwords});
    id corrected = [value isKindOfClass:NSDictionary.class] ? value[@"text"] : nil;
    return [corrected isKindOfClass:NSString.class] ? corrected : text;
}

NSData *BoundedModelManifestData(NSString *path) {
    const int descriptor = open(path.fileSystemRepresentation, O_RDONLY | O_NOFOLLOW | O_NONBLOCK);
    if (descriptor < 0) return nil;
    struct stat fileStat = {};
    if (fstat(descriptor, &fileStat) != 0 || !S_ISREG(fileStat.st_mode) ||
        fileStat.st_size < 0 ||
        static_cast<uint64_t>(fileStat.st_size) > MaximumModelManifestBytes) {
        close(descriptor);
        return nil;
    }
    NSMutableData *data = [NSMutableData dataWithCapacity:static_cast<NSUInteger>(fileStat.st_size)];
    std::array<uint8_t, 8192> buffer{};
    for (;;) {
        const ssize_t count = read(descriptor, buffer.data(), buffer.size());
        if (count == 0) break;
        if (count < 0) {
            if (errno == EINTR) continue;
            close(descriptor);
            return nil;
        }
        if (data.length > MaximumModelManifestBytes - static_cast<NSUInteger>(count)) {
            close(descriptor);
            return nil;
        }
        [data appendBytes:buffer.data() length:static_cast<NSUInteger>(count)];
    }
    close(descriptor);
    return data;
}

bool TrustedModelPathLink(const std::filesystem::path &path) {
    const std::filesystem::path expected = path == "/var"   ? "/private/var"
                                         : path == "/tmp"   ? "/private/tmp"
                                                             : std::filesystem::path();
    if (expected.empty()) return false;
    std::error_code error;
    const auto target = std::filesystem::read_symlink(path, error);
    return !error && (path.parent_path() / target).lexically_normal() == expected;
}

bool ModelDirectoryHasRealAncestors(NSString *path) {
    const std::filesystem::path directory(path.fileSystemRepresentation);
    if (!directory.is_absolute()) return false;
    std::vector<std::filesystem::path> ancestors;
    for (auto current = directory;; current = current.parent_path()) {
        ancestors.push_back(current);
        if (current.parent_path() == current) break;
    }
    std::error_code error;
    bool sawTrustedLink = false;
    for (auto it = ancestors.rbegin(); it != ancestors.rend(); ++it) {
        const auto status = std::filesystem::symlink_status(*it, error);
        if (error) return false;
        if (std::filesystem::is_symlink(status)) {
            const bool last = std::next(it) == ancestors.rend();
            if (last || sawTrustedLink || !TrustedModelPathLink(*it)) return false;
            sawTrustedLink = true;
        } else if (!std::filesystem::is_directory(status)) {
            return false;
        }
    }
    return true;
}

NSDictionary *ModelManifest(NSString *path) {
    if (!path.isAbsolutePath) return nil;
    if (!ModelDirectoryHasRealAncestors(path)) return nil;
    struct stat directoryStat = {};
    if (lstat(path.fileSystemRepresentation, &directoryStat) != 0 || !S_ISDIR(directoryStat.st_mode)) return nil;
    NSString *manifestPath = [path stringByAppendingPathComponent:@"msime-model.json"];
    struct stat manifestStat = {};
    if (lstat(manifestPath.fileSystemRepresentation, &manifestStat) != 0 || !S_ISREG(manifestStat.st_mode)) return nil;
    NSData *data = BoundedModelManifestData(manifestPath);
    id manifest = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
    return [manifest isKindOfClass:NSDictionary.class] ? manifest : nil;
}
} // namespace

BOOL MSIMELocalVoiceModelDirectory(NSString *path) {
    return ModelManifest(path) != nil;
}

NSString *MSIMELocalVoiceHelperPath(void) {
    NSFileManager *files = NSFileManager.defaultManager;
    NSString *configured = NSProcessInfo.processInfo.environment[MSIMEEditionIsShurufa() ? @"SHURUFA_VOICE_LOCAL_HELPER" : @"MSIME_VOICE_LOCAL_HELPER"];
    if (configured.length) return configured.isAbsolutePath && [files isExecutableFileAtPath:configured] ? configured : nil;
    NSString *bundled = [NSBundle.mainBundle.executablePath.stringByDeletingLastPathComponent stringByAppendingPathComponent:MSIMELocalVoiceHelperName()];
    return bundled.isAbsolutePath && [files isExecutableFileAtPath:bundled] ? bundled : nil;
}

NSString *MSIMELocalVoiceRuntimePath(NSDictionary *hostOptions) {
    static NSString *const Library = @"libsherpa-onnx-c-api.dylib";
    NSFileManager *files = NSFileManager.defaultManager;
    const char *helper = std::getenv(MSIMEEditionIsShurufa() ? "SHURUFA_VOICE_LOCAL_HELPER" : "MSIME_VOICE_LOCAL_HELPER");
    if (helper && *helper) return @"";
    NSString *bundled = [[NSBundle.mainBundle.bundlePath stringByAppendingPathComponent:@"Contents/Frameworks"] stringByAppendingPathComponent:Library];
    if ([files fileExistsAtPath:bundled]) return @"";
    id root = [hostOptions isKindOfClass:NSDictionary.class] ? hostOptions[@"preferences_directory"] : nil;
    if (![root isKindOfClass:NSString.class] || ![root isAbsolutePath]) return nil;
    // 共享层核对 msime-model.json 记录的字节与编译进来的锁一致、文件都是普通文件；没装完或过期的包报 missing 或 outdated。
    id packs = HostValue(msime_client_resource_packs, @{@"state_root" : root});
    BOOL installed = NO;
    if ([packs isKindOfClass:NSArray.class])
        for (id pack in packs)
            if ([pack isKindOfClass:NSDictionary.class] && [pack[@"id"] isEqual:@"voice-runtime-macos"] && [pack[@"state"] isEqual:@"installed"]) installed = YES;
    if (!installed) return nil;
    NSString *path = [[[root stringByAppendingPathComponent:@"resource-packs"] stringByAppendingPathComponent:@"voice-runtime-macos"] stringByAppendingPathComponent:Library];
    struct stat library = {};
    return lstat(path.fileSystemRepresentation, &library) == 0 && S_ISREG(library.st_mode) ? path : nil;
}

@interface MSIMELocalVoiceRequest ()
@property(nonatomic, copy, readwrite, nullable) NSString *lastFinalizedText;
@property(nonatomic, copy, readwrite, nullable) NSString *lastDraftText;
@property(nonatomic, copy, readwrite, nullable) NSDictionary<NSString *, id> *lastFinalMetrics;
- (void)helperMessage:(NSDictionary * _Nonnull)message;
- (void)helperFailed:(NSError * _Nonnull)error session:(NSNumber * _Nonnull)session;
@end

// 单例本地识别辅助进程与通信管道管理器。所有进程启停、写入与 stderr 读取关闭都在同一串行 `queue` 上完成，
// 确保无 FD 并发读/关竞态；TextStream 通道额外监听系统内存压力并回收 stderr 诊断分类。
@interface MSIMELocalVoiceHelper : NSObject
@property(nonatomic, readonly) dispatch_queue_t queue;
@property(nonatomic, weak) MSIMELocalVoiceRequest *activeRequest; // main only
+ (instancetype)shared;
+ (instancetype)textStream;
- (BOOL)startSession:(NSNumber *)session request:(MSIMELocalVoiceRequest *)request message:(NSDictionary *)message runtime:(NSString *)runtime error:(NSError **)error;
- (void)sendMessage:(NSDictionary *)message session:(NSNumber *)session;
- (void)endSession:(NSNumber *)session;
- (void)reportDiagnosticStderrWithCategory:(NSString *)category;
@end

@implementation MSIMELocalVoiceHelper {
    BOOL _textStream;
    NSTask *_task;
    NSFileHandle *_input;
    dispatch_source_t _stderrSource;
    std::shared_ptr<StderrRingBuffer> _stderrRing;
    dispatch_source_t _memoryPressureSource;
    BOOL _releaseAfterSession;
    uint64_t _generation;
    // 当前正在由子进程处理的会话标识，用于进程异常退出时通知对应请求失败。
    NSNumber *_session;
    __weak MSIMELocalVoiceRequest *_sessionRequest;
}

+ (instancetype)shared {
    static MSIMELocalVoiceHelper *helper;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ helper = [MSIMELocalVoiceHelper new]; });
    return helper;
}

+ (instancetype)textStream {
    static MSIMELocalVoiceHelper *helper;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ helper = [[MSIMELocalVoiceHelper alloc] initForTextStream:YES]; });
    return helper;
}

- (instancetype)init { return [self initForTextStream:NO]; }

- (instancetype)initForTextStream:(BOOL)textStream {
    if (!(self = [super init])) return nil;
    _textStream = textStream;
    _queue = dispatch_queue_create(textStream ? "app.msime.client.voice.textstream" : "app.msime.client.voice.local", dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INITIATED, 0));
    if (textStream) {
        // 监听系统内存压力（Warn / Critical），在 worker 空闲时立即发 release 释放 MLX 权重与 Metal 缓存。
        _memoryPressureSource = dispatch_source_create(
            DISPATCH_SOURCE_TYPE_MEMORYPRESSURE, 0,
            DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL, _queue);
        if (_memoryPressureSource) {
            __weak MSIMELocalVoiceHelper *weakSelf = self;
            dispatch_source_set_event_handler(_memoryPressureSource, ^{
                MSIMELocalVoiceHelper *helper = weakSelf;
                if (helper) [helper handleMemoryPressure];
            });
            dispatch_resume(_memoryPressureSource);
        }
    }
    return self;
}

- (void)dealloc {
    if (_memoryPressureSource) {
        dispatch_source_cancel(_memoryPressureSource);
        _memoryPressureSource = nil;
    }
    if (_stderrSource) {
        dispatch_source_cancel(_stderrSource);
        _stderrSource = nil;
    }
}

- (void)route:(NSDictionary *)message {
    [self.activeRequest helperMessage:message];
}

- (void)handleMemoryPressure {
    if (!_task || !_task.running) return;
    if (!_session) {
        [self write:@{@"op" : @"release"} error:nil];
        msime_macos_diagnostic_write("textstream_memory_pressure action=release_sent");
    } else {
        // 录音进行中不中断用户口述，待本次会话结束后立刻释放权重。
        _releaseAfterSession = YES;
        msime_macos_diagnostic_write("textstream_memory_pressure action=release_deferred");
    }
}

// 仅在串行 `_queue` 上调用，与 `DISPATCH_SOURCE_TYPE_READ` 事件处理与关闭回调严格互斥，零 FD 竞态。
- (void)drainStderrOnQueue {
    if (!_stderrSource || !_stderrRing || dispatch_source_testcancel(_stderrSource) != 0) return;
    const int fd = static_cast<int>(dispatch_source_get_handle(_stderrSource));
    if (fd < 0) return;
    std::array<char, 1024> buf{};
    for (;;) {
        const ssize_t n = ::read(fd, buf.data(), buf.size());
        if (n > 0) {
            _stderrRing->append(buf.data(), static_cast<size_t>(n));
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) break;
        dispatch_source_cancel(_stderrSource);
        _stderrSource = nil;
        break;
    }
}

- (void)closeStderrSourceOnQueue {
    if (_stderrSource) {
        dispatch_source_cancel(_stderrSource);
        _stderrSource = nil;
    }
    _stderrRing.reset();
}

- (void)emitDiagnosticStderrWithCategory:(NSString *)category exitStatus:(int)status hasExitStatus:(BOOL)hasExitStatus {
    [self drainStderrOnQueue];
    const std::vector<std::string> lines = _stderrRing ? _stderrRing->drainSnapshot() : std::vector<std::string>{};
    const std::string prefix = _textStream ? "textstream_stderr" : "local_voice_stderr";
    const std::string cat = category.UTF8String ? category.UTF8String : "event";
    const char *kind = ClassifyStderrKind(lines);
    if (hasExitStatus || !lines.empty()) {
        // `diagnostic.log` 严格仅写单条固定结构化事件摘要（状态码、行数、错误分类），绝不写入路径或自由文本。
        const std::string summary = prefix + " cat=" + cat + " status=" +
                                    (hasExitStatus ? std::to_string(status) : "none") +
                                    " lines=" + std::to_string(lines.size()) +
                                    " kind=" + kind;
        msime_macos_diagnostic_write(summary);
        // 同步上报固定分类标签至 host_line 通道，便于既有汇总检视器识别故障归类。
        const std::string hostCategory = prefix + "_" + kind + ": " + cat;
        msime_macos_diagnostic_host_line(hostCategory.c_str());
    }
    // 详细脱敏 stderr 仅记入系统统一日志（标记 private 默认脱敏，不污染 diagnostic.log）。
    for (size_t i = 0; i < lines.size(); ++i) {
        os_log_error(OS_LOG_DEFAULT, "%{public}s [%{public}s/%{public}s #%zu]: %{private}s",
                     prefix.c_str(), cat.c_str(), kind, i + 1, lines[i].c_str());
    }
}

- (void)reportDiagnosticStderrWithCategory:(NSString *)category {
    [self emitDiagnosticStderrWithCategory:category exitStatus:0 hasExitStatus:NO];
}

// `runtime` 是 MSIMELocalVoiceRuntimePath 的结果：非空时作为 `--runtime` 交给辅助进程。已经在运行的辅助进程沿用它启动时加载的运行库。
- (BOOL)launch:(NSString *)runtime error:(NSError **)error {
    NSString *path = _textStream ? MSIMETextStreamLauncherPath() : MSIMELocalVoiceHelperPath();
    if (!path) {
        if (error) *error = LocalVoiceFailure(_textStream ? @"找不到本地识别组件 shurufa-textstream，请重新安装输入法。" : @"找不到本地识别组件 msime-voice-local，请重新安装输入法。");
        return NO;
    }
    NSTask *task = [NSTask new];
    task.executableURL = [NSURL fileURLWithPath:path];
    if (_textStream) {
        // 保留 `--idle-exit 120` 兼容既有启动参数契约；模型实际驻留时长按物理内存分级经环境变量传给 worker
        // （>= 32 GiB 默认 900s，避免大内存机型频繁重载 4.4 GB 权重；< 32 GiB 默认 180s；已显式设置时尊重调用方）。
        task.arguments = @[ @"--idle-exit", @"120" ];
        NSMutableDictionary<NSString *, NSString *> *environment = [NSProcessInfo.processInfo.environment mutableCopy];
        if (!environment[@"SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC"].length) {
            const uint64_t physicalBytes = NSProcessInfo.processInfo.physicalMemory;
            environment[@"SHURUFA_TEXTSTREAM_MODEL_IDLE_SEC"] =
                physicalBytes >= kHighMemoryThresholdBytes ? @"900" : @"180";
        }
        task.environment = environment;
    } else if (runtime.length) {
        task.arguments = @[ @"--runtime", runtime ];
    }
    NSPipe *input = [NSPipe pipe];
    NSPipe *output = [NSPipe pipe];
    NSPipe *errorPipe = [NSPipe pipe];
    task.standardInput = input;
    task.standardOutput = output;
    task.standardError = errorPipe;
    task.qualityOfService = NSQualityOfServiceUserInitiated;
    const uint64_t generation = ++_generation;
    __weak MSIMELocalVoiceHelper *weakSelf = self;

    NSMutableData *pending = [NSMutableData data];
    __block BOOL droppingOversizedLine = NO;
    output.fileHandleForReading.readabilityHandler = ^(NSFileHandle *handle) {
        NSData *chunk = handle.availableData;
        if (!chunk.length) { handle.readabilityHandler = nil; return; }
        NSMutableArray<NSDictionary *> *messages = [NSMutableArray array];
        @synchronized(pending) {
            [pending appendData:chunk];
            for (;;) {
                if (!pending.length) break;
                const char *bytes = static_cast<const char *>(pending.bytes);
                const void *newline = std::memchr(bytes, '\n', pending.length);
                if (!newline) {
                    if (droppingOversizedLine || pending.length > MaximumHelperLine) {
                        pending.length = 0;
                        droppingOversizedLine = YES;
                    }
                    break;
                }
                const NSUInteger length = static_cast<NSUInteger>(static_cast<const char *>(newline) - bytes);
                if (droppingOversizedLine || length > MaximumHelperLine) {
                    [pending replaceBytesInRange:NSMakeRange(0, length + 1) withBytes:nullptr length:0];
                    droppingOversizedLine = NO;
                    continue;
                }
                id message = [NSJSONSerialization JSONObjectWithData:[pending subdataWithRange:NSMakeRange(0, length)] options:0 error:nil];
                if ([message isKindOfClass:NSDictionary.class]) [messages addObject:message];
                [pending replaceBytesInRange:NSMakeRange(0, length + 1) withBytes:nullptr length:0];
            }
        }
        if (messages.count)
            dispatch_async(dispatch_get_main_queue(), ^{
                for (NSDictionary *message in messages) [weakSelf route:message];
            });
    };
    task.terminationHandler = ^(NSTask *ended) {
        const int status = ended.terminationStatus;
        const BOOL uncaughtSignal = (ended.terminationReason == NSTaskTerminationReasonUncaughtSignal);
        MSIMELocalVoiceHelper *helper = weakSelf;
        if (helper) {
            dispatch_async(helper->_queue, ^{
                [helper process:generation endedWithDetail:@"本地识别进程意外退出。"
                     exitStatus:status uncaughtSignal:uncaughtSignal];
            });
        }
    };
    NSFileHandle *stderrRead = errorPipe.fileHandleForReading;
    NSError *launchError = nil;
    if (![task launchAndReturnError:&launchError]) {
        output.fileHandleForReading.readabilityHandler = nil;
        [stderrRead closeFile];
        if (error) *error = LocalVoiceFailure(_textStream ? @"无法启动本地识别组件 shurufa-textstream。" : @"无法启动本地识别组件 msime-voice-local。");
        return NO;
    }

    // 启动成功后在 `_queue` 上挂载非阻塞 DISPATCH_SOURCE_TYPE_READ 监听 stderr，
    // 并在 cancel handler 中统一关闭读端句柄，杜绝并发 ::read() 与 FD 复用关闭竞态。
    NSString *homeDirectory = NSHomeDirectory() ?: @"";
    auto stderrRing = std::make_shared<StderrRingBuffer>(homeDirectory.UTF8String ?: "");
    const int stderrFd = stderrRead.fileDescriptor;
    dispatch_source_t stderrSource = nil;
    if (stderrFd >= 0) {
        const int flags = fcntl(stderrFd, F_GETFL, 0);
        if (flags >= 0) (void)fcntl(stderrFd, F_SETFL, flags | O_NONBLOCK);
        stderrSource = dispatch_source_create(DISPATCH_SOURCE_TYPE_READ, static_cast<uintptr_t>(stderrFd), 0, _queue);
    }
    if (stderrSource) {
        dispatch_source_set_event_handler(stderrSource, ^{
            MSIMELocalVoiceHelper *helper = weakSelf;
            if (helper && helper->_generation == generation) [helper drainStderrOnQueue];
        });
        dispatch_source_set_cancel_handler(stderrSource, ^{
            [stderrRead closeFile];
        });
        dispatch_resume(stderrSource);
    } else {
        [stderrRead closeFile];
    }

    // 禁止向已退出子进程写入时触发 SIGPIPE 杀掉输入法主进程。
    fcntl(input.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1);
    _task = task;
    _input = input.fileHandleForWriting;
    _stderrSource = stderrSource;
    _stderrRing = stderrRing;
    _releaseAfterSession = NO;
    _session = nil;
    _sessionRequest = nil;
    return YES;
}

// 子进程退出或通信失败时的统一清理：按需上报结构化 stderr 诊断分类，串行取消 stderr 监听并通知在途会话失败。
- (void)process:(uint64_t)generation endedWithDetail:(NSString *)detail exitStatus:(int)status uncaughtSignal:(BOOL)uncaughtSignal {
    if (generation != _generation || !_task) return;
    const BOOL abnormal = uncaughtSignal || status != 0 || _session != nil;
    if (abnormal) {
        [self emitDiagnosticStderrWithCategory:@"exit" exitStatus:status hasExitStatus:YES];
    }
    if (_task.running) [_task terminate];
    [_input closeFile];
    [self closeStderrSourceOnQueue];
    _task = nil;
    _input = nil;
    _releaseAfterSession = NO;
    NSNumber *session = _session;
    MSIMELocalVoiceRequest *request = _sessionRequest;
    _session = nil;
    _sessionRequest = nil;
    if (session && request)
        dispatch_async(dispatch_get_main_queue(), ^{ [request helperFailed:LocalVoiceFailure(detail) session:session]; });
}

- (void)process:(uint64_t)generation endedWithDetail:(NSString *)detail {
    [self process:generation endedWithDetail:detail exitStatus:-1 uncaughtSignal:NO];
}

- (BOOL)write:(NSDictionary *)message error:(NSError **)error {
    NSMutableData *line = [[NSJSONSerialization dataWithJSONObject:message options:0 error:nil] mutableCopy];
    if (!line) {
        if (error) *error = LocalVoiceFailure();
        return NO;
    }
    [line appendBytes:"\n" length:1];
    const int descriptor = _input.fileDescriptor;
    const uint8_t *bytes = static_cast<const uint8_t *>(line.bytes);
    size_t remaining = line.length;
    while (remaining) {
        const ssize_t written = ::write(descriptor, bytes, remaining);
        if (written < 0 && errno == EINTR) continue;
        if (written <= 0) {
            [self process:_generation endedWithDetail:@"本地识别进程已停止响应。"];
            if (error) *error = LocalVoiceFailure(@"本地识别进程已停止响应。");
            return NO;
        }
        bytes += written;
        remaining -= static_cast<size_t>(written);
    }
    return YES;
}

- (BOOL)startSession:(NSNumber *)session request:(MSIMELocalVoiceRequest *)request message:(NSDictionary *)message runtime:(NSString *)runtime error:(NSError **)error {
    if (!_task && ![self launch:runtime error:error]) return NO;
    if (![self write:message error:error]) return NO;
    _session = session;
    _sessionRequest = request;
    return YES;
}

- (void)sendMessage:(NSDictionary *)message session:(NSNumber *)session {
    if (!_task || ![_session isEqual:session]) return;
    [self write:message error:nil];
}

- (void)endSession:(NSNumber *)session {
    if (![_session isEqual:session]) return;
    _session = nil;
    _sessionRequest = nil;
    if (_releaseAfterSession) {
        _releaseAfterSession = NO;
        if (_task && _task.running) {
            [self write:@{@"op" : @"release"} error:nil];
            msime_macos_diagnostic_write("textstream_memory_pressure action=release_after_session");
        }
    }
}
@end

namespace {
enum class LocalRequestState : int { idle, streaming, finishing, closed };
}

@implementation MSIMELocalVoiceRequest {
    BOOL _textStream;
    MSIMELocalVoiceHelper *_helper;
    NSString *_model;
    NSString *_runtime;
    NSString *_language;
    NSString *_hotwordMode;
    NSDictionary *_hostOptions;
    NSNumber *_session;
    MSIMEDoubaoResult _result; // main only
    std::atomic<LocalRequestState> _state;
    NSArray<NSDictionary *> *_hotwords; // helper queue only
}

- (instancetype)initWithOptions:(NSDictionary *)options hostOptions:(NSDictionary *)hostOptions error:(NSError **)error {
    if (!(self = [super init])) return nil;
    id model = options[@"asr_model_path"];
    NSDictionary *manifest = [model isKindOfClass:NSString.class] ? ModelManifest(model) : nil;
    if (!manifest) {
        if (error) *error = LocalVoiceFailure(@"本地语音模型未安装或已被移动，请在语音设置中重新下载。");
        return nil;
    }
    if (!MSIMELocalVoiceHelperPath()) {
        if (error) *error = LocalVoiceFailure(@"找不到本地识别组件 msime-voice-local，请重新安装输入法。");
        return nil;
    }
    NSString *runtime = MSIMELocalVoiceRuntimePath(hostOptions);
    if (!runtime) {
        if (error) *error = LocalVoiceFailure(@"本地语音识别运行库还没有下载，请在设置的“语音输入”分区下载。");
        return nil;
    }
    _runtime = [runtime copy];
    _model = [model copy];
    id language = options[@"language"];
    _language = [language isKindOfClass:NSString.class] ? [language copy] : @"";
    id hotwords = manifest[@"hotwords"];
    _hotwordMode = [hotwords isKindOfClass:NSString.class] ? [hotwords copy] : @"";
    _hostOptions = [hostOptions isKindOfClass:NSDictionary.class] ? [hostOptions copy] : nil;
    _helper = MSIMELocalVoiceHelper.shared;
    _state.store(LocalRequestState::idle);
    return self;
}

- (instancetype)initWithTextStreamOptions:(NSDictionary *)options hostOptions:(NSDictionary *)hostOptions error:(NSError **)error {
    if (!(self = [super init])) return nil;
    if (!MSIMETextStreamReady()) {
        if (error) *error = LocalVoiceFailure(@"本地语音模型还没有安装，请在输入法的语音设置里点“安装语音模型”。");
        return nil;
    }
    if (!MSIMETextStreamLauncherPath()) {
        if (error) *error = LocalVoiceFailure(@"找不到本地识别组件 shurufa-textstream，请重新安装输入法。");
        return nil;
    }
    _textStream = YES;
    _helper = MSIMELocalVoiceHelper.textStream;
    _model = [MSIMETextStreamModelDirectory() copy];
    _runtime = @"";
    id language = options[@"language"];
    _language = [language isKindOfClass:NSString.class] ? [language copy] : @"";
    // 不把用户词库当作 Qwen3-ASR 的上下文：上下文是提示文本，小模型在停顿或听不清时会把它原样吐进转写，
    // 用户看到的就是以前输入过的内容冒出来。
    _hotwordMode = @"";
    _hostOptions = [hostOptions isKindOfClass:NSDictionary.class] ? [hostOptions copy] : nil;
    _state.store(LocalRequestState::idle);
    return self;
}

- (void)dealloc {
    const LocalRequestState previous = _state.exchange(LocalRequestState::closed);
    if (previous == LocalRequestState::streaming || previous == LocalRequestState::finishing) {
        MSIMELocalVoiceHelper *helper = _helper;
        NSNumber *session = _session;
        dispatch_async(helper.queue, ^{
            [helper sendMessage:@{@"op" : @"cancel"} session:session];
            [helper endSession:session];
        });
    }
}

- (BOOL)startWithResult:(MSIMEDoubaoResult)result error:(NSError **)error {
    LocalRequestState expected = LocalRequestState::idle;
    if (!result || !_state.compare_exchange_strong(expected, LocalRequestState::streaming)) {
        if (error) *error = LocalVoiceFailure();
        return NO;
    }
    static uint64_t sessions = 0;
    _session = @(++sessions);
    _result = [result copy];
    _lastFinalizedText = @"";
    _lastDraftText = @"";
    _lastFinalMetrics = nil;
    MSIMELocalVoiceHelper *helper = _helper;
    helper.activeRequest = self;
    NSNumber *session = _session;
    NSDictionary *hostOptions = _hostOptions;
    const BOOL native = [_hotwordMode isEqual:@"native"];
    const BOOL pinyin = [_hotwordMode isEqual:@"pinyin"];
    NSDictionary *start = @{@"op" : @"start", @"id" : session, @"model" : _model, @"language" : _language, @"threads" : @0};
    __weak MSIMELocalVoiceRequest *weakSelf = self;
    dispatch_async(helper.queue, ^{
        MSIMELocalVoiceRequest *request = weakSelf;
        if (!request || request->_state.load() == LocalRequestState::closed) return;
        // Read here rather than on main: the dictionary lives behind the same store the rest of the session uses, and the first audio chunks queue behind this start instead of being dropped by a helper that has no session yet.
        NSArray<NSDictionary *> *hotwords = (native || pinyin) && hostOptions ? FetchHotwords(hostOptions) : @[];
        request->_hotwords = pinyin ? hotwords : @[];
        NSMutableDictionary *message = [start mutableCopy];
        if (native) {
            NSArray *words = [hotwords valueForKey:@"text"];
            // TextStream 把热词当作解码提示的上下文，太长会拖慢每次解码。
            message[@"hotwords"] = request->_textStream && words.count > 40 ? [words subarrayWithRange:NSMakeRange(0, 40)] : words;
        }
        NSError *failure = nil;
        if (![helper startSession:session request:request message:message runtime:request->_runtime error:&failure])
            dispatch_async(dispatch_get_main_queue(), ^{ [request helperFailed:failure session:session]; });
    });
    return YES;
}

- (BOOL)appendPCM:(NSData *)pcm error:(NSError **)error {
    if (_state.load() != LocalRequestState::streaming || pcm.length % sizeof(float)) {
        if (error) *error = LocalVoiceFailure();
        return NO;
    }
    if (!pcm.length) return YES;
    const float *samples = static_cast<const float *>(pcm.bytes);
    const NSUInteger count = pcm.length / sizeof(float);
    std::vector<uint8_t> bytes(count * 2);
    for (NSUInteger index = 0; index < count; ++index) {
        const float sample = std::isfinite(samples[index]) ? std::fmax(-1.0f, std::fmin(1.0f, samples[index])) : 0.0f;
        const auto value = static_cast<uint16_t>(static_cast<int16_t>(std::lrint(sample * 32767.0f)));
        bytes[2 * index] = static_cast<uint8_t>(value & 0xFF);
        bytes[2 * index + 1] = static_cast<uint8_t>(value >> 8);
    }
    NSString *encoded = [[NSData dataWithBytes:bytes.data() length:bytes.size()] base64EncodedStringWithOptions:0];
    MSIMELocalVoiceHelper *helper = _helper;
    NSNumber *session = _session;
    dispatch_async(helper.queue, ^{ [helper sendMessage:@{@"op" : @"audio", @"pcm16" : encoded} session:session]; });
    return YES;
}

- (BOOL)finishWithError:(NSError **)error {
    LocalRequestState expected = LocalRequestState::streaming;
    if (!_state.compare_exchange_strong(expected, LocalRequestState::finishing)) {
        if (error) *error = LocalVoiceFailure();
        return NO;
    }
    MSIMELocalVoiceHelper *helper = _helper;
    NSNumber *session = _session;
    dispatch_async(helper.queue, ^{ [helper sendMessage:@{@"op" : @"finish"} session:session]; });
    return YES;
}

- (void)cancel {
    const LocalRequestState previous = _state.exchange(LocalRequestState::closed);
    _result = nil;
    _partialStateHandler = nil;
    MSIMELocalVoiceHelper *helper = _helper;
    if (helper.activeRequest == self) helper.activeRequest = nil;
    if (previous != LocalRequestState::streaming && previous != LocalRequestState::finishing) return;
    NSNumber *session = _session;
    dispatch_async(helper.queue, ^{
        [helper sendMessage:@{@"op" : @"cancel"} session:session];
        [helper endSession:session];
    });
}

// Ends the request with its last callback. Later helper messages for this session find it closed.
- (void)closeWithText:(NSString *)text error:(NSError *)error {
    _state.store(LocalRequestState::closed);
    MSIMELocalVoiceHelper *helper = _helper;
    if (helper.activeRequest == self) helper.activeRequest = nil;
    NSNumber *session = _session;
    dispatch_async(helper.queue, ^{ [helper endSession:session]; });
    MSIMEDoubaoResult result = _result;
    _result = nil;
    _partialStateHandler = nil;
    if (result) result(error ? nil : (text ?: @""), !error, error);
}

- (void)helperMessage:(NSDictionary *)message {
    if (_textStream && [message[@"type"] isEqual:@"hello"] && [message[@"available"] isEqual:@NO]) {
        const LocalRequestState current = _state.load();
        if (current != LocalRequestState::streaming && current != LocalRequestState::finishing) return;
        MSIMELocalVoiceHelper *helper = _helper;
        dispatch_async(helper.queue, ^{ [helper reportDiagnosticStderrWithCategory:@"hello_unavailable"]; });
        id why = message[@"error"];
        NSString *detail = [why isKindOfClass:NSString.class] && [why length] ? [NSString stringWithFormat:@"本地语音识别运行环境不可用（%@），请在语音设置里重新安装语音模型。", why] : @"本地语音识别运行环境不可用，请在语音设置里重新安装语音模型。";
        [self closeWithText:nil error:LocalVoiceFailure(detail)];
        return;
    }
    if (![message[@"id"] isEqual:_session]) return;
    const LocalRequestState state = _state.load();
    if (state != LocalRequestState::streaming && state != LocalRequestState::finishing) return;
    NSString *type = [message[@"type"] isKindOfClass:NSString.class] ? message[@"type"] : @"";
    NSString *text = [message[@"text"] isKindOfClass:NSString.class] ? message[@"text"] : @"";
    if ([type isEqual:@"partial"]) {
        // 解析双态流式字段；旧协议未附带 finalized / draft 时退化为全文定稿、草稿为空。
        NSString *finalized = [message[@"finalized"] isKindOfClass:NSString.class] ? message[@"finalized"] : nil;
        NSString *draft = [message[@"draft"] isKindOfClass:NSString.class] ? message[@"draft"] : nil;
        if (!finalized && !draft) {
            finalized = text;
            draft = @"";
        } else {
            finalized = finalized ?: @"";
            draft = draft ?: @"";
        }
        _lastFinalizedText = [finalized copy];
        _lastDraftText = [draft copy];
        if (_partialStateHandler) _partialStateHandler(finalized, draft);
        if (_result) _result(text, NO, nil);
    } else if ([type isEqual:@"final"]) {
        _state.store(LocalRequestState::closed);
        // 记录可选统计与分段信息（供上层存档与性能观测使用，旧协议缺失时为空）。
        NSMutableDictionary<NSString *, id> *metrics = [NSMutableDictionary dictionary];
        if ([message[@"segments"] isKindOfClass:NSArray.class]) metrics[@"segments"] = message[@"segments"];
        for (NSString *key in @[@"audio_ms", @"speech_ms", @"asr_ms", @"segment_count"]) {
            if ([message[key] isKindOfClass:NSNumber.class]) metrics[key] = message[key];
        }
        _lastFinalMetrics = metrics.count ? [metrics copy] : nil;
        MSIMELocalVoiceHelper *helper = _helper;
        if (!text.length) { [self closeWithText:text error:nil]; return; }
        // Pinyin correction reads the hotwords the start fetched, which only the helper queue touches.
        __weak MSIMELocalVoiceRequest *weakSelf = self;
        dispatch_async(helper.queue, ^{
            MSIMELocalVoiceRequest *request = weakSelf;
            if (!request) return;
            NSArray<NSDictionary *> *hotwords = request->_hotwords;
            NSString *corrected = hotwords.count ? CorrectedText(text, hotwords) : text;
            dispatch_async(dispatch_get_main_queue(), ^{ [weakSelf closeWithText:corrected error:nil]; });
        });
    } else if ([type isEqual:@"error"] || [type isEqual:@"cancelled"]) {
        if ([type isEqual:@"error"]) {
            MSIMELocalVoiceHelper *helper = _helper;
            dispatch_async(helper.queue, ^{ [helper reportDiagnosticStderrWithCategory:@"protocol_error"]; });
        }
        id detail = message[@"message"];
        [self closeWithText:nil error:LocalVoiceFailure([detail isKindOfClass:NSString.class] ? detail : nil)];
    }
}

- (void)helperFailed:(NSError *)error session:(NSNumber *)session {
    if (![session isEqual:_session]) return;
    const LocalRequestState state = _state.load();
    if (state != LocalRequestState::streaming && state != LocalRequestState::finishing) return;
    [self closeWithText:nil error:error ?: LocalVoiceFailure()];
}
@end
