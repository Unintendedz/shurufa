#pragma once

// Shurufa v2 Tier 1 本地 Unix 域套接字（UDS）二进制帧协议：
// 16 字节定长网络字节序帧头 + 至多 512 KiB 的 UTF-8 载荷。
// 载荷按 `op` 直通原始上下文或 Rust 引擎产生的 `NeuralDecodeRequest`/`NeuralDecodeResponse` JSON，
// 零二次 JSON 封装/转义开销，客户端与服务端严格共用本头文件。

#include <arpa/inet.h>
#include <errno.h>
#include <poll.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

namespace shurufa::neural::ipc {

inline constexpr uint32_t kFrameMagic = 0x5348524EU; // 'SHRN'
inline constexpr uint8_t kProtocolVersion = 1U;
inline constexpr size_t kFrameHeaderBytes = 16U;
inline constexpr uint32_t kMaxFramePayloadBytes = 512U * 1024U;

enum class OpCode : uint8_t {
    Health = 1U,
    Prefill = 2U,
    Decode = 3U,
    Unload = 4U,
    Shutdown = 5U,
};

inline constexpr uint16_t kFlagOk = 1U << 0;

struct FrameHeader {
    uint32_t magic = kFrameMagic;
    uint8_t version = kProtocolVersion;
    uint8_t op = 0U;
    uint16_t flags = 0U;
    uint32_t payload_len = 0U;
    uint32_t request_id = 0U;
};

inline uint64_t MonotonicNowNs() {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

inline void ConfigureSocketNoSigPipe(int fd) {
    if (fd < 0) return;
#ifdef SO_NOSIGPIPE
    int one = 1;
    (void)::setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof(one));
#endif
}

inline void EncodeHeader(const FrameHeader &header, uint8_t out[kFrameHeaderBytes]) {
    const uint32_t magic_be = htonl(header.magic);
    const uint16_t flags_be = htons(header.flags);
    const uint32_t len_be = htonl(header.payload_len);
    const uint32_t id_be = htonl(header.request_id);
    std::memcpy(out + 0, &magic_be, 4);
    out[4] = header.version;
    out[5] = header.op;
    std::memcpy(out + 6, &flags_be, 2);
    std::memcpy(out + 8, &len_be, 4);
    std::memcpy(out + 12, &id_be, 4);
}

inline bool DecodeHeader(const uint8_t in[kFrameHeaderBytes], FrameHeader &out) {
    uint32_t magic_be = 0;
    uint16_t flags_be = 0;
    uint32_t len_be = 0;
    uint32_t id_be = 0;
    std::memcpy(&magic_be, in + 0, 4);
    std::memcpy(&flags_be, in + 6, 2);
    std::memcpy(&len_be, in + 8, 4);
    std::memcpy(&id_be, in + 12, 4);
    out.magic = ntohl(magic_be);
    out.version = in[4];
    out.op = in[5];
    out.flags = ntohs(flags_be);
    out.payload_len = ntohl(len_be);
    out.request_id = ntohl(id_be);
    return out.magic == kFrameMagic &&
           out.version == kProtocolVersion &&
           out.payload_len <= kMaxFramePayloadBytes;
}

/// 在绝对单调截止时间前写满 `length` 字节；超时或对端断开返回 false。
inline bool WriteExactWithDeadline(int fd, const uint8_t *data, size_t length, uint64_t deadline_ns) {
    size_t written = 0;
    while (written < length) {
        const uint64_t now = MonotonicNowNs();
        if (deadline_ns > 0 && now >= deadline_ns) return false;
        const int timeout_ms = deadline_ns > 0
            ? static_cast<int>((deadline_ns - now + 999999ULL) / 1000000ULL)
            : -1;
        struct pollfd pfd{};
        pfd.fd = fd;
        pfd.events = POLLOUT;
        const int pr = ::poll(&pfd, 1, timeout_ms);
        if (pr < 0) {
            if (errno == EINTR) continue;
            return false;
        }
        if (pr == 0) return false;
        if ((pfd.revents & (POLLERR | POLLHUP | POLLNVAL)) != 0 && (pfd.revents & POLLOUT) == 0) {
            return false;
        }
        const ssize_t n = ::write(fd, data + written, length - written);
        if (n < 0) {
            if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK) continue;
            return false;
        }
        if (n == 0) return false;
        written += static_cast<size_t>(n);
    }
    return true;
}

/// 在绝对单调截止时间前读满 `length` 字节；超时或对端断开返回 false。
inline bool ReadExactWithDeadline(int fd, uint8_t *data, size_t length, uint64_t deadline_ns) {
    size_t offset = 0;
    while (offset < length) {
        const uint64_t now = MonotonicNowNs();
        if (deadline_ns > 0 && now >= deadline_ns) return false;
        const int timeout_ms = deadline_ns > 0
            ? static_cast<int>((deadline_ns - now + 999999ULL) / 1000000ULL)
            : -1;
        struct pollfd pfd{};
        pfd.fd = fd;
        pfd.events = POLLIN;
        const int pr = ::poll(&pfd, 1, timeout_ms);
        if (pr < 0) {
            if (errno == EINTR) continue;
            return false;
        }
        if (pr == 0) return false;
        if ((pfd.revents & (POLLERR | POLLNVAL)) != 0) return false;
        const ssize_t n = ::read(fd, data + offset, length - offset);
        if (n < 0) {
            if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK) continue;
            return false;
        }
        if (n == 0) return false;
        offset += static_cast<size_t>(n);
    }
    return true;
}

inline bool WriteFrame(int fd,
                       OpCode op,
                       uint16_t flags,
                       uint32_t request_id,
                       const void *payload,
                       size_t payload_len,
                       int timeout_ms) {
    if (payload_len > kMaxFramePayloadBytes) return false;
    const uint64_t deadline_ns = timeout_ms > 0
        ? MonotonicNowNs() + static_cast<uint64_t>(timeout_ms) * 1000000ULL
        : 0ULL;
    FrameHeader header{};
    header.op = static_cast<uint8_t>(op);
    header.flags = flags;
    header.payload_len = static_cast<uint32_t>(payload_len);
    header.request_id = request_id;
    uint8_t raw_header[kFrameHeaderBytes];
    EncodeHeader(header, raw_header);
    if (!WriteExactWithDeadline(fd, raw_header, kFrameHeaderBytes, deadline_ns)) {
        return false;
    }
    if (payload_len > 0 && payload != nullptr) {
        if (!WriteExactWithDeadline(fd, static_cast<const uint8_t *>(payload), payload_len, deadline_ns)) {
            return false;
        }
    }
    return true;
}

inline bool ReadFrame(int fd, FrameHeader &out_header, std::string &out_payload, int timeout_ms) {
    const uint64_t deadline_ns = timeout_ms > 0
        ? MonotonicNowNs() + static_cast<uint64_t>(timeout_ms) * 1000000ULL
        : 0ULL;
    uint8_t raw_header[kFrameHeaderBytes];
    if (!ReadExactWithDeadline(fd, raw_header, kFrameHeaderBytes, deadline_ns)) {
        return false;
    }
    if (!DecodeHeader(raw_header, out_header)) {
        return false;
    }
    out_payload.resize(out_header.payload_len);
    if (out_header.payload_len > 0) {
        if (!ReadExactWithDeadline(fd,
                                   reinterpret_cast<uint8_t *>(out_payload.data()),
                                   out_header.payload_len,
                                   deadline_ns)) {
            out_payload.clear();
            return false;
        }
    }
    return true;
}

} // namespace shurufa::neural::ipc
