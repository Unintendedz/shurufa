#include "ShurufaNeuralEngine.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

using namespace shurufa::neural;

namespace {

double percentile(std::vector<double> values, double p) {
    if (values.empty()) return 0.0;
    std::sort(values.begin(), values.end());
    const size_t idx = std::min(values.size() - 1, static_cast<size_t>(std::floor(p * static_cast<double>(values.size() - 1))));
    return values[idx];
}

}  // namespace

int main(int argc, char **argv) {
    EngineConfig cfg{};
    cfg.n_ctx = 2048;
    cfg.n_batch = 512;
    cfg.n_seq_max = 32;
    cfg.n_gpu_layers = 99;
    cfg.n_threads = 4;

    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], "--model") == 0 && i + 1 < argc) {
            cfg.model_path = argv[++i];
        } else if (std::strcmp(argv[i], "--pinyin-db") == 0 && i + 1 < argc) {
            cfg.pinyin_db_path = argv[++i];
        }
    }

    std::string err;
    auto engine = NeuralEngine::create(cfg, err);
    if (!engine) {
        std::fprintf(stderr, "引擎初始化失败: %s\n", err.c_str());
        return 1;
    }

    const auto vs = engine->vocab_index_stats();
    std::printf("=== ShurufaNeuralEngine M3 Max Metal 微基准 ===\n");
    std::printf("模型路径: %s\n", engine->model_path().c_str());
    std::printf("受限词表: ready=%d, hanzi=%d, indexed_tokens=%d, syllable_keys=%d, build=%.2f ms\n\n",
                engine->has_constrained_vocab() ? 1 : 0,
                vs.hanzi_entries,
                vs.indexed_tokens,
                vs.syllable_keys,
                vs.build_ms);

    // 1. 能力 A：上文 KV-Cache 预热、0 ms 复用与增量追加验证。
    const std::string ctx1 = "这个兄弟 应该多少还是";
    PrefillStats pf1{}, pf2{}, pf3{}, pf_empty{};
    if (!engine->prefill_context(ctx1, pf1, err)) {
        std::fprintf(stderr, "prefill 1 失败: %s\n", err.c_str());
        return 1;
    }
    if (!engine->prefill_context(ctx1, pf2, err)) {
        std::fprintf(stderr, "prefill 2 失败: %s\n", err.c_str());
        return 1;
    }
    const std::string ctx2 = "这个兄弟 应该多少还是挣了些钱，今年过年";
    if (!engine->prefill_context(ctx2, pf3, err)) {
        std::fprintf(stderr, "prefill 3 失败: %s\n", err.c_str());
        return 1;
    }
    if (!engine->prefill_context("", pf_empty, err)) {
        std::fprintf(stderr, "prefill empty 失败: %s\n", err.c_str());
        return 1;
    }
    std::printf("[能力 A] 上文预热:\n");
    std::printf("  冷预热 \"%s\": total=%d reused=%d decoded=%d %.3f ms\n",
                ctx1.c_str(), pf1.total_tokens, pf1.reused_tokens, pf1.decoded_tokens, pf1.elapsed_ms);
    std::printf("  同文复用: total=%d reused=%d decoded=%d %.3f ms\n",
                pf2.total_tokens, pf2.reused_tokens, pf2.decoded_tokens, pf2.elapsed_ms);
    std::printf("  尾部增量追加: total=%d reused=%d decoded=%d %.3f ms\n",
                pf3.total_tokens, pf3.reused_tokens, pf3.decoded_tokens, pf3.elapsed_ms);
    std::printf("  空上文清空: total=%d reused=%d decoded=%d %.3f ms\n\n",
                pf_empty.total_tokens, pf_empty.reused_tokens, pf_empty.decoded_tokens, pf_empty.elapsed_ms);

    if (pf2.decoded_tokens != 0 || pf2.reused_tokens != pf2.total_tokens) {
        std::fprintf(stderr, "断言失败: 相同上文未命中 0-token 复用!\n");
        return 2;
    }
    if (pf3.reused_tokens <= 0 || pf3.decoded_tokens >= pf3.total_tokens) {
        std::fprintf(stderr, "断言失败: 尾部追加未命中前缀增量复用!\n");
        return 2;
    }

    // 2. 能力 B：30 条候选前缀树共享 KV 批量打分，对照逐条精确参考打分。
    const std::vector<std::string> candidates = {
        "整了些钱", "挣了些钱", "正了些钱", "争了些钱", "蒸了些钱",
        "整了血铅", "挣了血铅", "正了血铅", "政了血铅", "症了血铅",
        "挣了些浅", "挣了些前", "挣了些千", "挣了些欠", "挣了写钱",
        "整了些浅", "整了些前", "整了写钱", "正了些前", "争了些前",
        "挣了斜签", "整了斜签", "正了斜签", "挣了鞋钱", "整了鞋钱",
        "挣了些", "整了些", "挣了", "整了", "挣"
    };

    if (!engine->prefill_context(ctx1, pf1, err)) {
        std::fprintf(stderr, "重新预热 ctx1 失败: %s\n", err.c_str());
        return 1;
    }

    std::vector<double> ref_scores;
    double ref_ms = 0.0;
    if (!engine->score_candidates_sequential_reference(nullptr, candidates, ref_scores, ref_ms, err)) {
        std::fprintf(stderr, "逐条参考打分失败: %s\n", err.c_str());
        return 1;
    }

    std::vector<double> trie_scores;
    ScoreStats sc_stats{};
    std::vector<double> lat_ms;
    for (int iter = 0; iter < 10; ++iter) {
        if (!engine->score_candidates(nullptr, candidates, trie_scores, sc_stats, err)) {
            std::fprintf(stderr, "前缀树批量打分失败: %s\n", err.c_str());
            return 1;
        }
        lat_ms.push_back(sc_stats.elapsed_ms);
    }

    double max_abs_diff = 0.0;
    size_t max_diff_idx = 0;
    for (size_t i = 0; i < candidates.size(); ++i) {
        const double d = std::fabs(ref_scores[i] - trie_scores[i]);
        if (d > max_abs_diff) {
            max_abs_diff = d;
            max_diff_idx = i;
        }
    }

    std::vector<size_t> order(candidates.size());
    for (size_t i = 0; i < order.size(); ++i) order[i] = i;
    std::sort(order.begin(), order.end(), [&](size_t a, size_t b) {
        return trie_scores[a] > trie_scores[b];
    });

    std::printf("[能力 B] 30 候选前缀树批量打分 vs 逐条参考:\n");
    std::printf("  逐条参考耗时: %.2f ms | 前缀树 P50=%.2f ms, P95=%.2f ms (加速 %.2fx)\n",
                ref_ms,
                percentile(lat_ms, 0.50),
                percentile(lat_ms, 0.95),
                ref_ms / std::max(0.01, percentile(lat_ms, 0.50)));
    std::printf("  树压缩: total_tokens=%d -> unique_nodes=%d -> internal_decoded=%d, batches=%d\n",
                sc_stats.total_candidate_tokens,
                sc_stats.unique_trie_nodes,
                sc_stats.internal_decoded_nodes,
                sc_stats.decode_batches);
    std::printf("  最大数值偏差: %.6f nats (idx=%zu \"%s\": ref=%.4f vs trie=%.4f)\n",
                max_abs_diff,
                max_diff_idx,
                candidates[max_diff_idx].c_str(),
                ref_scores[max_diff_idx],
                trie_scores[max_diff_idx]);
    std::printf("  Top-5 神经排序结果:\n");
    for (size_t k = 0; k < 5 && k < order.size(); ++k) {
        const size_t idx = order[k];
        std::printf("    #%zu %-12s logP=%.4f\n", k + 1, candidates[idx].c_str(), trie_scores[idx]);
    }
    std::printf("\n");

    if (max_abs_diff > 0.15) {
        std::fprintf(stderr, "断言失败: 前缀树与逐条参考数值偏差过大 (%.6f nats)!\n", max_abs_diff);
        return 3;
    }

    // 3. 能力 C：拼音约束神经束搜索（`zheng'le'xie'qian`）。
    if (engine->has_constrained_vocab()) {
        std::vector<SyllablePathInput> paths(1);
        paths[0].syllables = {"zheng", "le", "xie", "qian"};
        paths[0].channel_log_prob = 0.0;
        paths[0].is_complete = true;

        std::vector<BeamCandidate> beams;
        BeamStats bstats{};
        std::vector<double> beam_lat;
        for (int iter = 0; iter < 5; ++iter) {
            if (!engine->decode_constrained(&ctx1, paths, 5, 4, beams, bstats, err)) {
                std::fprintf(stderr, "受限束搜索失败: %s\n", err.c_str());
                return 1;
            }
            beam_lat.push_back(bstats.elapsed_ms);
        }
        std::printf("[能力 C] 受限束搜索 (`%s` + `zheng'le'xie'qian`, B=5):\n", ctx1.c_str());
        std::printf("  P50=%.2f ms, P95=%.2f ms, decode_calls=%d, decoded_tokens=%d\n",
                    percentile(beam_lat, 0.50),
                    percentile(beam_lat, 0.95),
                    bstats.decode_calls,
                    bstats.decoded_tokens);
        for (size_t k = 0; k < beams.size(); ++k) {
            std::printf("    #%zu %-12s logP=%.4f (tokens=%d)\n",
                        k + 1, beams[k].text.c_str(), beams[k].log_prob, beams[k].token_count);
        }
        std::printf("\n");

        // 零声母多路径联合束搜索测试（`我准备明天去` + `xi'an` / `xian`）。
        const std::string ctx_xian = "我准备明天去";
        std::vector<SyllablePathInput> xian_paths(2);
        xian_paths[0].syllables = {"xian"};
        xian_paths[0].channel_log_prob = 0.0;
        xian_paths[1].syllables = {"xi", "an"};
        xian_paths[1].channel_log_prob = -0.6;
        if (engine->decode_constrained(&ctx_xian, xian_paths, 5, 4, beams, bstats, err)) {
            std::printf("[能力 C] 零声母多路径 (`%s` + `xian` / `xi'an`, %.2f ms):\n",
                        ctx_xian.c_str(), bstats.elapsed_ms);
            for (size_t k = 0; k < beams.size(); ++k) {
                std::printf("    #%zu %-12s logP=%.4f (path=%d)\n",
                            k + 1, beams[k].text.c_str(), beams[k].log_prob, beams[k].path_index);
            }
            std::printf("\n");
        }
    } else {
        std::printf("[能力 C] 未传入 --pinyin-db，跳过受限束搜索测试。\n\n");
    }

    // 4. C ABI 端到端冒烟验证（与 Python `ctypes` 共用入口）。
    std::string cfg_json = std::string("{\"model_path\":\"") + engine->model_path() + "\"";
    if (!cfg.pinyin_db_path.empty()) {
        cfg_json += ",\"pinyin_db_path\":\"" + cfg.pinyin_db_path + "\"";
    }
    cfg_json += "}";
    engine.reset();

    void *c_handle = shurufa_neural_create(cfg_json.data(), cfg_json.size());
    if (!c_handle) {
        std::fprintf(stderr, "C ABI shurufa_neural_create 失败\n");
        return 4;
    }
    const std::string req_json =
        "{\"session\":1,\"generation\":7,\"context\":\"这个兄弟 应该多少还是\","
        "\"syllable_paths\":[{\"syllables\":[\"zheng\",\"le\",\"xie\",\"qian\"],\"channel_log_prob\":0.0,\"is_complete\":true}],"
        "\"candidates\":[{\"text\":\"整了血铅\"},{\"text\":\"整了些钱\"},{\"text\":\"挣了些钱\"}],"
        "\"beam_width\":5,\"max_beam_results\":4,\"enable_constrained_decode\":true}";
    char *resp = shurufa_neural_process_request(c_handle, req_json.data(), req_json.size());
    if (!resp) {
        std::fprintf(stderr, "C ABI shurufa_neural_process_request 返回 NULL\n");
        shurufa_neural_destroy(c_handle);
        return 4;
    }
    std::printf("[C ABI] process_request 输出:\n%s\n", resp);
    shurufa_neural_string_free(resp);
    shurufa_neural_destroy(c_handle);
    return 0;
}
