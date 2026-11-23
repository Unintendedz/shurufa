#include "../../src/voice/VoiceAudioPreprocess.h"

#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstdint>
#include <limits>
#include <vector>

namespace {

constexpr double kPi = 3.14159265358979323846;

// 生成合成正弦波（16 kHz 单声道 float）。
std::vector<float> MakeSine(double hz, float amplitude, size_t samples) {
    std::vector<float> out(samples, 0.0f);
    for (size_t i = 0; i < samples; ++i) {
        out[i] = amplitude * static_cast<float>(
            std::sin(2.0 * kPi * hz * static_cast<double>(i) / 16000.0));
    }
    return out;
}

float Rms(const std::vector<float> &samples, size_t begin, size_t end) {
    assert(end > begin && end <= samples.size());
    double sum = 0.0;
    for (size_t i = begin; i < end; ++i) {
        sum += static_cast<double>(samples[i]) * static_cast<double>(samples[i]);
    }
    return static_cast<float>(std::sqrt(sum / static_cast<double>(end - begin)));
}

// 确定性伪随机底噪，不依赖外部随机源或真实音频。
std::vector<float> MakeDeterministicNoise(float amplitude, size_t samples) {
    std::vector<float> out(samples, 0.0f);
    uint32_t state = 0x12345678U;
    for (size_t i = 0; i < samples; ++i) {
        state = state * 1664525U + 1013904223U;
        const float normalized = static_cast<float>(static_cast<int32_t>(state)) / 2147483648.0f;
        out[i] = normalized * amplitude;
    }
    return out;
}

} // namespace

int main() {
    using msime::voice::VoiceAudioPreprocessor;

    // 1. 80 Hz 二阶 Butterworth 高通：滤除直流偏移，且纯直流不触发语音活跃判定。
    {
        VoiceAudioPreprocessor dsp;
        std::vector<float> dc(16000, 0.5f);
        dsp.processInPlace(dc.data(), dc.size());
        const float tailRms = Rms(dc, 8000, dc.size());
        assert(tailRms < 0.002f);
        assert(!dsp.hasLikelySpeech());
        assert(dsp.isLikelyAccidentalSilence(1.0));
        assert(dsp.totalProcessedMs() == 1000);
    }

    // 2. 低频衰减、80 Hz 截止点与 1 kHz 语音频带通带保真。
    {
        VoiceAudioPreprocessor::Config hpfOnly;
        hpfOnly.enable_agc = false;

        // 20 Hz 低频风噪/嗡声：理论二阶 Butterworth 衰减约 -24 dB（幅值比 ~0.0625）。
        VoiceAudioPreprocessor lowFilter(hpfOnly);
        auto rumble = MakeSine(20.0, 0.2f, 16000);
        const float rumbleInRms = Rms(rumble, 8000, 16000);
        lowFilter.processInPlace(rumble.data(), rumble.size());
        const float rumbleOutRms = Rms(rumble, 8000, 16000);
        assert(rumbleOutRms / rumbleInRms < 0.10f);

        // 80 Hz 截止频率：幅值比应在 1/sqrt(2) ≈ 0.707 附近。
        VoiceAudioPreprocessor cutoffFilter(hpfOnly);
        auto cutoffTone = MakeSine(80.0, 0.2f, 16000);
        const float cutoffInRms = Rms(cutoffTone, 8000, 16000);
        cutoffFilter.processInPlace(cutoffTone.data(), cutoffTone.size());
        const float cutoffRatio = Rms(cutoffTone, 8000, 16000) / cutoffInRms;
        assert(cutoffRatio > 0.65f && cutoffRatio < 0.76f);

        // 1 kHz 语音频带（-20 dBFS）：高通与默认 AGC 下应保持接近单位增益。
        VoiceAudioPreprocessor passFilter;
        auto speechTone = MakeSine(1000.0, 0.14142135f, 16000); // RMS = 0.10 (-20 dBFS)
        const float speechInRms = Rms(speechTone, 8000, 16000);
        passFilter.processInPlace(speechTone.data(), speechTone.size());
        const float speechRatio = Rms(speechTone, 8000, 16000) / speechInRms;
        assert(speechRatio > 0.94f && speechRatio < 1.06f);
        assert(passFilter.hasLikelySpeech());
    }

    // 3. 噪声门控温和 AGC：不抬高 -60 dBFS 纯底噪；温和提升轻声并遵守 +12 dB 上限。
    {
        VoiceAudioPreprocessor noiseDsp;
        auto noise = MakeDeterministicNoise(0.0015f, 24000); // RMS 远低于 -48 dBFS 底噪门
        noiseDsp.processInPlace(noise.data(), noise.size());
        assert(noiseDsp.currentGain() <= 1.05f);
        assert(!noiseDsp.hasLikelySpeech());

        // -32 dBFS 轻声（RMS ≈ 0.025），应平滑向 -20 dBFS（0.10）提升。
        VoiceAudioPreprocessor quietDsp;
        auto quiet = MakeSine(440.0, 0.0355f, 32000); // 2.0 s，RMS ≈ 0.0251
        quietDsp.processInPlace(quiet.data(), quiet.size());
        const float quietTailRms = Rms(quiet, 24000, 32000);
        assert(quietDsp.currentGain() > 2.5f && quietDsp.currentGain() <= 3.99f);
        assert(quietTailRms > 0.075f && quietTailRms < 0.115f);
        assert(quietDsp.hasLikelySpeech());

        // 极轻声（-42 dBFS，RMS ≈ 0.008 > -48 dBFS）：增益严格不超过 +12 dB（≈ 3.981）。
        VoiceAudioPreprocessor veryQuietDsp;
        auto veryQuiet = MakeSine(440.0, 0.0113f, 40000); // 2.5 s
        veryQuietDsp.processInPlace(veryQuiet.data(), veryQuiet.size());
        assert(veryQuietDsp.currentGain() <= 3.99f);
    }

    // 4. 软限幅 [-0.95, 0.95] 与 NaN/Inf 防护。
    {
        VoiceAudioPreprocessor limiterDsp;
        std::vector<float> extreme(4800, 0.0f);
        for (size_t i = 0; i < extreme.size(); ++i) {
            extreme[i] = (i % 2 == 0) ? 2.5f : -3.2f;
        }
        extreme[10] = std::numeric_limits<float>::quiet_NaN();
        extreme[11] = std::numeric_limits<float>::infinity();
        extreme[12] = -std::numeric_limits<float>::infinity();
        limiterDsp.processInPlace(extreme.data(), extreme.size());
        for (float sample : extreme) {
            assert(std::isfinite(sample));
            assert(std::fabs(sample) <= 0.95f + 1e-5f);
        }
    }

    // 5. 语音活跃时长统计、误触静音拦截与分块无关性。
    {
        // 纯静音 0.8 s。
        VoiceAudioPreprocessor silent;
        std::vector<float> zeros(12800, 0.0f);
        silent.processInPlace(zeros.data(), zeros.size());
        assert(silent.activeSpeechMs() == 0);
        assert(!silent.hasLikelySpeech());
        assert(silent.isLikelyAccidentalSilence(0.8));

        // 0.5 s 录音中仅 80 ms 短脉冲：低于 120 ms 活跃门限。
        VoiceAudioPreprocessor brief;
        std::vector<float> briefSignal(8000, 0.0f);
        auto burst80 = MakeSine(500.0, 0.25f, 1280); // 80 ms
        std::copy(burst80.begin(), burst80.end(), briefSignal.begin() + 1600);
        brief.processInPlace(briefSignal.data(), briefSignal.size());
        assert(brief.activeSpeechMs() < 120);
        assert(!brief.hasLikelySpeech(120));
        assert(brief.hasLikelySpeech(50));
        assert(brief.isLikelyAccidentalSilence(0.5));

        // 0.5 s 录音中含 200 ms 清晰人声频段信号：超过 120 ms，判定为有效语音。
        VoiceAudioPreprocessor valid;
        std::vector<float> validSignal(8000, 0.0f);
        auto burst200 = MakeSine(500.0, 0.25f, 3200); // 200 ms
        std::copy(burst200.begin(), burst200.end(), validSignal.begin() + 1600);
        auto chunkedCopy = validSignal;
        valid.processInPlace(validSignal.data(), validSignal.size());
        assert(valid.activeSpeechMs() >= 180);
        assert(valid.peakAbs() > 0.15f);
        assert(valid.hasLikelySpeech(120));
        assert(!valid.isLikelyAccidentalSilence(0.5));

        // 超过 2.5 s 阈值时，即使活跃时长不足，也不在采集层静默吞掉长按意图。
        assert(!silent.isLikelyAccidentalSilence(3.0));

        // 任意非对齐分块大小（137 采样）与整块处理结果完全一致。
        VoiceAudioPreprocessor chunked;
        for (size_t offset = 0; offset < chunkedCopy.size(); offset += 137) {
            const size_t len = std::min<size_t>(137, chunkedCopy.size() - offset);
            chunked.processInPlace(chunkedCopy.data() + offset, len);
        }
        assert(chunked.activeSpeechMs() == valid.activeSpeechMs());
        for (size_t i = 0; i < validSignal.size(); ++i) {
            assert(std::fabs(chunkedCopy[i] - validSignal[i]) < 1e-6f);
        }
    }

    // 6. reset() 完整清除状态并可复现相同输出。
    {
        VoiceAudioPreprocessor dsp;
        auto first = MakeSine(440.0, 0.03f, 16000);
        auto second = first;
        dsp.processInPlace(first.data(), first.size());
        assert(dsp.activeSpeechMs() > 0 && dsp.peakAbs() > 0.0f && dsp.currentGain() > 1.2f);
        dsp.reset();
        assert(dsp.activeSpeechMs() == 0);
        assert(dsp.totalProcessedMs() == 0);
        assert(dsp.peakAbs() == 0.0f);
        assert(std::fabs(dsp.currentGain() - 1.0f) < 1e-6f);
        dsp.processInPlace(second.data(), second.size());
        for (size_t i = 0; i < first.size(); ++i) {
            assert(std::fabs(first[i] - second[i]) < 1e-6f);
        }
    }

    return 0;
}
