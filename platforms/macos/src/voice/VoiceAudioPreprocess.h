#pragma once

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

namespace msime::voice {

/// 本地语音前端预处理参数配置（独立于宿主类声明，避免 Clang DR 1397 默认实参限制）。
struct VoiceAudioPreprocessConfig {
    double highpass_cutoff_hz = 80.0;
    /// 目标 RMS：-20 dBFS（10^(-20/20) = 0.10）。
    float target_rms = 0.10f;
    /// AGC 底噪门限：-48 dBFS（10^(-48/20) ≈ 0.003981）。低于此值时不提升增益。
    float noise_gate_rms = 0.0039810717f;
    /// 语音活跃能量门限：-45 dBFS（10^(-45/20) ≈ 0.005623）。
    float speech_threshold_rms = 0.0056234133f;
    /// AGC 最大增益上限：+12 dB（10^(12/20) ≈ 3.98107）。
    float max_gain = 3.9810717f;
    /// AGC 最小增益下限：-12 dB（10^(-12/20) ≈ 0.25119），避免近场高声压削波。
    float min_gain = 0.25118864f;
    /// 软限幅拐点与绝对上限：拐点以下保持线性零失真，上限锁死在 0.95。
    float limiter_knee = 0.80f;
    float limiter_ceiling = 0.95f;
    bool enable_agc = true;
};

/// 本地语音前端音频预处理（纯 C++，无 Objective-C / CoreAudio 依赖）。
///
/// 面向 16 kHz 单声道 `float` PCM 样本，每实例绑定一次录音会话，提供：
/// 1. 80 Hz 二阶 Butterworth 高通滤波器，滤除麦克风直流偏移、低频风噪与桌面敲击震动；
/// 2. 噪声门控温和 AGC（目标 RMS -20 dBFS、底噪门 -48 dBFS、增益上限 +12 dB）；
/// 3. 软限幅器（Soft-Knee Limiter），将输出严格约束在 `[-0.95, 0.95]` 防止爆音削波；
/// 4. 整段语音活跃时长 `activeSpeechMs()` 与峰值统计，支持停录时判定静音误触。
class VoiceAudioPreprocessor {
public:
    using Config = VoiceAudioPreprocessConfig;
    static constexpr uint32_t kSampleRate = 16000;
    static constexpr uint32_t kAnalysisFrameMs = 10;
    static constexpr size_t kAnalysisFrameSamples = kSampleRate * kAnalysisFrameMs / 1000; // 160 采样

    VoiceAudioPreprocessor() noexcept : VoiceAudioPreprocessor(Config{}) {}

    explicit VoiceAudioPreprocessor(const Config &config) noexcept
        : config_(config) {
        computeFilterCoefficients();
        computeSmoothingCoefficients();
        reset();
    }

    /// 清空滤波器、AGC 增益与语音活跃统计状态，用于开启新一次录音会话。
    void reset() noexcept {
        z1_ = 0.0;
        z2_ = 0.0;
        current_gain_ = 1.0f;
        target_gain_ = 1.0f;
        in_speech_gate_ = false;
        frame_energy_sum_ = 0.0;
        frame_samples_ = 0;
        total_samples_ = 0;
        active_speech_ms_ = 0;
        peak_abs_ = 0.0f;
    }

    /// 原地处理一批 16 kHz 单声道 `float` 样本。
    void processInPlace(float *samples, size_t count) noexcept {
        if (!samples || count == 0) return;
        for (size_t i = 0; i < count; ++i) {
            const float raw = samples[i];
            const double input = std::isfinite(raw) ? static_cast<double>(raw) : 0.0;

            // 1. 二阶 Butterworth 高通（Direct Form II Transposed，双精度内部状态避免极点漂移）。
            const double hpDouble = b0_ * input + z1_;
            z1_ = b1_ * input - a1_ * hpDouble + z2_;
            z2_ = b2_ * input - a2_ * hpDouble;
            const float hp = static_cast<float>(hpDouble);

            // 2. 在高通之后、AGC 之前累计语音活跃度与信号峰值，避免直流偏移或底噪抬升造成误判。
            const float absHp = std::fabs(hp);
            if (absHp > peak_abs_) peak_abs_ = absHp;
            frame_energy_sum_ += hpDouble * hpDouble;
            ++frame_samples_;
            ++total_samples_;
            if (frame_samples_ >= kAnalysisFrameSamples) {
                finalizeAnalysisFrame();
            }

            // 3. 逐采样平滑追踪 AGC 增益，消除分帧切换的台阶爆音。
            if (config_.enable_agc) {
                float alpha = alpha_relax_;
                if (in_speech_gate_) {
                    alpha = (target_gain_ < current_gain_) ? alpha_attack_ : alpha_release_;
                }
                current_gain_ += alpha * (target_gain_ - current_gain_);
                current_gain_ = std::clamp(current_gain_, config_.min_gain, config_.max_gain);
            } else {
                current_gain_ = 1.0f;
            }

            // 4. 应用增益与软限幅，输出严格落在 [-0.95, 0.95]。
            samples[i] = softLimit(hp * current_gain_);
        }
    }

    /// 已累计超过语音活跃门限（-45 dBFS）的帧总时长（毫秒）。
    uint32_t activeSpeechMs() const noexcept { return active_speech_ms_; }

    /// 已处理的音频总时长（毫秒）。
    uint32_t totalProcessedMs() const noexcept {
        return static_cast<uint32_t>((total_samples_ * 1000ULL) / kSampleRate);
    }

    /// 高通后观测到的最大样本绝对值。
    float peakAbs() const noexcept { return peak_abs_; }

    /// 当前 AGC 线性增益（测试与诊断用）。
    float currentGain() const noexcept { return current_gain_; }

    /// 整段录音是否包含可信语音（活跃语音时长达到 `minActiveMs`，默认 120 ms）。
    bool hasLikelySpeech(uint32_t minActiveMs = 120) const noexcept {
        return active_speech_ms_ >= minActiveMs;
    }

    /// 结合总时长与活跃语音时长判定是否为明显的静音误触（默认 < 2.5 s 且活跃语音 < 120 ms）。
    bool isLikelyAccidentalSilence(double totalDurationSec,
                                   uint32_t minActiveMs = 120,
                                   double maxAccidentalDurationSec = 2.5) const noexcept {
        if (!std::isfinite(totalDurationSec)) return true;
        return totalDurationSec < maxAccidentalDurationSec && !hasLikelySpeech(minActiveMs);
    }

private:
    void computeFilterCoefficients() noexcept {
        constexpr double kPi = 3.14159265358979323846;
        constexpr double kSqrt2 = 1.41421356237309504880;
        const double cutoff = std::clamp(config_.highpass_cutoff_hz, 5.0, kSampleRate * 0.45);
        const double w0 = 2.0 * kPi * cutoff / static_cast<double>(kSampleRate);
        const double cosW0 = std::cos(w0);
        const double sinW0 = std::sin(w0);
        // 二阶 Butterworth 的 Q = 1 / sqrt(2)，故 alpha = sin(w0) / (2 * Q) = sin(w0) / sqrt(2)。
        const double alpha = sinW0 / kSqrt2;
        const double a0 = 1.0 + alpha;
        b0_ = ((1.0 + cosW0) * 0.5) / a0;
        b1_ = -(1.0 + cosW0) / a0;
        b2_ = b0_;
        a1_ = (-2.0 * cosW0) / a0;
        a2_ = (1.0 - alpha) / a0;
    }

    void computeSmoothingCoefficients() noexcept {
        // 慢攻慢放时间常数：压缩（80 ms）较快以压住突发强音，提升（400 ms）较慢避免呼吸/停顿抽吸，
        // 静默段（1500 ms）缓慢回落至 1.0 避免纯底噪保持高增益。
        const double fs = static_cast<double>(kSampleRate);
        alpha_attack_ = static_cast<float>(1.0 - std::exp(-1.0 / (fs * 0.080)));
        alpha_release_ = static_cast<float>(1.0 - std::exp(-1.0 / (fs * 0.400)));
        alpha_relax_ = static_cast<float>(1.0 - std::exp(-1.0 / (fs * 1.500)));
    }

    void finalizeAnalysisFrame() noexcept {
        const float rms = static_cast<float>(
            std::sqrt(frame_energy_sum_ / static_cast<double>(frame_samples_)));
        frame_energy_sum_ = 0.0;
        frame_samples_ = 0;

        if (rms >= config_.speech_threshold_rms) {
            active_speech_ms_ += kAnalysisFrameMs;
        }

        if (rms > config_.noise_gate_rms) {
            in_speech_gate_ = true;
            const float desired = config_.target_rms / rms;
            target_gain_ = std::clamp(desired, config_.min_gain, config_.max_gain);
        } else {
            in_speech_gate_ = false;
            target_gain_ = 1.0f;
        }
    }

    float softLimit(float x) const noexcept {
        if (!std::isfinite(x)) return 0.0f;
        const float absX = std::fabs(x);
        const float knee = config_.limiter_knee;
        const float ceiling = config_.limiter_ceiling;
        if (absX <= knee) return x;
        const float headroom = std::max(1e-4f, ceiling - knee);
        const float excess = (absX - knee) / headroom;
        const float limited = std::min(ceiling, knee + headroom * std::tanh(excess));
        return x >= 0.0f ? limited : -limited;
    }

    Config config_{};
    double b0_ = 1.0;
    double b1_ = 0.0;
    double b2_ = 0.0;
    double a1_ = 0.0;
    double a2_ = 0.0;
    double z1_ = 0.0;
    double z2_ = 0.0;

    float alpha_attack_ = 0.0f;
    float alpha_release_ = 0.0f;
    float alpha_relax_ = 0.0f;
    float current_gain_ = 1.0f;
    float target_gain_ = 1.0f;
    bool in_speech_gate_ = false;

    double frame_energy_sum_ = 0.0;
    size_t frame_samples_ = 0;
    uint64_t total_samples_ = 0;
    uint32_t active_speech_ms_ = 0;
    float peak_abs_ = 0.0f;
};

} // namespace msime::voice
