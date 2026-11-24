#ifndef SHURUFA_NEURAL_ENGINE_H
#define SHURUFA_NEURAL_ENGINE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// 确保 `libshurufa_neural.dylib` 在 `-fvisibility=hidden` 构建下完整导出 C ABI 符号。
#if defined(__GNUC__) || defined(__clang__)
#define SHURUFA_NEURAL_API __attribute__((visibility("default")))
#else
#define SHURUFA_NEURAL_API
#endif

/// 按目录自动发现首个可用 `*.gguf` 模型文件，返回堆分配 UTF-8 路径；未找到返回 NULL。
/// 调用方须通过 `shurufa_neural_string_free` 释放。
SHURUFA_NEURAL_API char *shurufa_neural_discover_model_path(const char *model_dir, size_t len);

/// 创建端侧神经解码引擎（`libllama` Apple Metal）。
/// `cfg_json` 可为 NULL（使用默认参数并从标准资源包目录自动发现 `*.gguf`），或传入 JSON：
/// `{"model_path":"...", "model_dir":"...", "pinyin_db_path":"...", "n_ctx":1024, "n_batch":512, "n_seq_max":20, "n_gpu_layers":99, "n_threads":4}`。
/// 失败时返回 NULL。
SHURUFA_NEURAL_API void *shurufa_neural_create(const char *cfg_json, size_t len);

/// 销毁引擎并释放全部 Metal / KV-Cache 内存。
SHURUFA_NEURAL_API void shurufa_neural_destroy(void *handle);

/// 释放本库任一 C ABI 函数返回的堆字符串。
SHURUFA_NEURAL_API void shurufa_neural_string_free(char *ptr);

/// 返回当前加载的模型绝对路径（JSON 响应或空指针安全，须用 `shurufa_neural_string_free` 释放）。
SHURUFA_NEURAL_API char *shurufa_neural_info(void *handle);

/// 能力 A：空闲态上文 KV-Cache 预热（维护 `seq_id = 0`，完整保留中文标点/空格/ASCII/Emoji，不加聊天模板）。
/// 相同上文复用耗时 0 ms；尾部追加上文时仅增量解码新增后缀。
/// 返回 JSON：`{"ok":true,"total_tokens":N,"reused_tokens":M,"decoded_tokens":K,"elapsed_ms":...}`。
SHURUFA_NEURAL_API char *shurufa_neural_prefill_context(
    void *handle,
    const char *ctx_utf8,
    size_t len);

/// 能力 B：Token 前缀字典树共享 KV 批量精准打分。
/// 入参 JSON：`{"context":"...", "candidates":["整了些钱","挣了些钱",...]}`（`candidates` 也支持对象数组 `[{"text":"..."}]`）。
/// 返回 JSON：`{"ok":true,"scores":[-14.2,-11.8],"candidate_scores":[{"text":"...","neural_log_prob":...}],"stats":{...}}`。
SHURUFA_NEURAL_API char *shurufa_neural_score_candidates(
    void *handle,
    const char *req_json,
    size_t len);

/// 能力 C：拼音约束神经束搜索（基于已预热的 `seq_0` 上文 KV-Cache 与启动时预建的汉字拼音倒排词表）。
/// 入参 JSON：`{"context":"...", "syllable_paths":[{"syllables":["zheng","le","xie","qian"],"channel_log_prob":0.0}], "beam_width":5, "max_beam_results":4}`
/// （`syllable_paths` 也兼容字符串数组 `["zheng'le'xie'qian"]`）。
/// 返回 JSON：`{"ok":true,"beam_candidates":[{"text":"挣了些钱","neural_log_prob":-11.8,"path_index":0}],"stats":{...}}`。
SHURUFA_NEURAL_API char *shurufa_neural_decode_constrained(
    void *handle,
    const char *req_json,
    size_t len);

/// 一步合一处理接口：直接消费 `msime_client_take_neural_request` 产出的 `NeuralDecodeRequest` JSON，
/// 在同一次调用中完成上文 KV 同步 +（可选）受限束搜索 + 前缀树共享 KV 批量候选打分，
/// 并直接返回 `msime_client_apply_neural_response` 可消费的 `NeuralDecodeResponse` JSON。
SHURUFA_NEURAL_API char *shurufa_neural_process_request(
    void *handle,
    const char *req_json,
    size_t len);

#ifdef __cplusplus
}

#include <memory>
#include <string>
#include <vector>

namespace shurufa::neural {

struct EngineConfig {
    std::string model_path;
    std::string model_dir;
    std::string pinyin_db_path;
    std::string pinyin_tsv_path;
    int32_t n_ctx = 1024;
    int32_t n_batch = 512;
    int32_t n_seq_max = 20;
    int32_t n_gpu_layers = 99;
    int32_t n_threads = 4;
    int32_t flash_attn = 1;
};

struct PrefillStats {
    int32_t total_tokens = 0;
    int32_t reused_tokens = 0;
    int32_t decoded_tokens = 0;
    double elapsed_ms = 0.0;
};

struct ScoreStats {
    int32_t candidate_count = 0;
    int32_t unique_trie_nodes = 0;
    int32_t internal_decoded_nodes = 0;
    int32_t total_candidate_tokens = 0;
    int32_t decode_batches = 0;
    double elapsed_ms = 0.0;
};

struct SyllablePathInput {
    std::vector<std::string> syllables;
    double channel_log_prob = 0.0;
    bool is_complete = true;
};

struct BeamCandidate {
    std::string text;
    double log_prob = 0.0;
    int32_t path_index = 0;
    int32_t token_count = 0;
};

struct BeamStats {
    int32_t path_count = 0;
    int32_t result_count = 0;
    int32_t decode_calls = 0;
    int32_t decoded_tokens = 0;
    double elapsed_ms = 0.0;
};

struct VocabIndexStats {
    int32_t hanzi_entries = 0;
    int32_t indexed_tokens = 0;
    int32_t syllable_keys = 0;
    double build_ms = 0.0;
};

/// 内部 C++ 引擎接口（供微基准测试与 `ShurufaNeuralService` 直接调用，零额外 JSON 序列化损耗）。
class SHURUFA_NEURAL_API NeuralEngine {
public:
    static std::string default_model_dir();
    static std::string discover_model_in_dir(const std::string &dir);

    static std::unique_ptr<NeuralEngine> create(const EngineConfig &config, std::string &error_out);
    ~NeuralEngine();

    NeuralEngine(const NeuralEngine &) = delete;
    NeuralEngine &operator=(const NeuralEngine &) = delete;

    const std::string &model_path() const;
    bool has_constrained_vocab() const;
    VocabIndexStats vocab_index_stats() const;

    bool prefill_context(const std::string &context_utf8, PrefillStats &stats_out, std::string &error_out);

    /// 前缀树共享 KV 批量打分（生产路径）。
    bool score_candidates(
        const std::string *context_utf8,
        const std::vector<std::string> &candidates_utf8,
        std::vector<double> &out_log_probs,
        ScoreStats &stats_out,
        std::string &error_out);

    /// 逐条独立序列参考打分（仅用于微基准测试校验前缀树批处理的数值等价性）。
    bool score_candidates_sequential_reference(
        const std::string *context_utf8,
        const std::vector<std::string> &candidates_utf8,
        std::vector<double> &out_log_probs,
        double &elapsed_ms_out,
        std::string &error_out);

    /// 拼音约束神经束搜索。
    bool decode_constrained(
        const std::string *context_utf8,
        const std::vector<SyllablePathInput> &paths,
        int32_t beam_width,
        int32_t max_results,
        std::vector<BeamCandidate> &out_results,
        BeamStats &stats_out,
        std::string &error_out);

    /// 内部一步合一 JSON 请求处理入口（供 C ABI `shurufa_neural_process_request` 直接转发）。
    std::string process_request_raw_json(const char *req_json, size_t len);

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
    explicit NeuralEngine(std::unique_ptr<Impl> impl);
};

}  // namespace shurufa::neural

#endif  // __cplusplus

#endif  // SHURUFA_NEURAL_ENGINE_H
