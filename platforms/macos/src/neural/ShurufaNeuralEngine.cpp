#include "ShurufaNeuralEngine.h"
#include "ShurufaNeuralJson.h"

#include <llama.h>
#include <sqlite3.h>
#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#include <dispatch/dispatch.h>
#endif
#ifdef __aarch64__
#include <arm_neon.h>
#endif

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <limits>
#include <memory>
#include <mutex>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace shurufa::neural {

namespace {

using Clock = std::chrono::steady_clock;

double elapsed_ms_since(Clock::time_point start) {
    return std::chrono::duration<double, std::milli>(Clock::now() - start).count();
}

double env_double_or(const char *name, double fallback) {
    const char *raw = std::getenv(name);
    if (!raw || raw[0] == '\0') return fallback;
    char *end = nullptr;
    const double val = std::strtod(raw, &end);
    if (end == raw || !std::isfinite(val)) return fallback;
    return val;
}

bool env_flag_enabled(const char *name) {
    const char *raw = std::getenv(name);
    return raw && (raw[0] == '1' || raw[0] == 't' || raw[0] == 'T');
}

// 通过 C++17 SFINAE 兼容不同 build 的 `llama_context_params` 字段演进（如 `kv_unified`、`flash_attn_type` / `flash_attn`）。
template <typename T, typename = void>
struct HasKvUnified : std::false_type {};
template <typename T>
struct HasKvUnified<T, std::void_t<decltype(std::declval<T &>().kv_unified)>> : std::true_type {};

template <typename T, typename = void>
struct HasFlashAttnBool : std::false_type {};
template <typename T>
struct HasFlashAttnBool<T, std::void_t<decltype(std::declval<T &>().flash_attn)>> : std::true_type {};

template <typename T, typename = void>
struct HasFlashAttnType : std::false_type {};
template <typename T>
struct HasFlashAttnType<T, std::void_t<decltype(std::declval<T &>().flash_attn_type)>> : std::true_type {};

template <typename T, typename = void>
struct HasNoPerf : std::false_type {};
template <typename T>
struct HasNoPerf<T, std::void_t<decltype(std::declval<T &>().no_perf)>> : std::true_type {};

template <typename Params>
void configure_context_extensions(Params &cparams, int32_t flash_attn) {
    if constexpr (HasKvUnified<Params>::value) {
        cparams.kv_unified = true;
    }
    if constexpr (HasNoPerf<Params>::value) {
        cparams.no_perf = true;
    }
    if constexpr (HasFlashAttnType<Params>::value) {
        // 默认保留 `LLAMA_FLASH_ATTN_TYPE_AUTO`，仅在显式关闭时写 0（DISABLED）。
        if (flash_attn == 0) {
            cparams.flash_attn_type = static_cast<decltype(cparams.flash_attn_type)>(0);
        }
    } else if constexpr (HasFlashAttnBool<Params>::value) {
        cparams.flash_attn = (flash_attn != 0);
    }
}

void silent_llama_log_callback(enum ggml_log_level level, const char *text, void *user_data) {
    (void)user_data;
    const char *verbose = std::getenv("SHURUFA_NEURAL_VERBOSE");
    if (verbose && verbose[0] == '1') {
        if (text) std::fputs(text, stderr);
        return;
    }
    if (level == GGML_LOG_LEVEL_ERROR && text) {
        std::fputs(text, stderr);
    }
}

void ensure_llama_backend_initialized() {
    static std::once_flag once;
    std::call_once(once, []() {
        llama_log_set(silent_llama_log_callback, nullptr);
        llama_backend_init();
    });
}

char *dup_c_string(const std::string &s) {
    char *out = static_cast<char *>(std::malloc(s.size() + 1));
    if (!out) return nullptr;
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return out;
}

bool is_cjk_hanzi(char32_t cp) {
    return (cp >= 0x4E00u && cp <= 0x9FFFu) ||
           (cp >= 0x3400u && cp <= 0x4DBFu) ||
           cp == 0x3007u;
}

int32_t count_utf8_codepoints(const char *data, size_t len) {
    int32_t count = 0;
    for (size_t i = 0; i < len; ++i) {
        if ((static_cast<unsigned char>(data[i]) & 0xC0u) != 0x80u) {
            ++count;
        }
    }
    return count;
}

int32_t count_utf8_codepoints(const std::string &s) {
    return count_utf8_codepoints(s.data(), s.size());
}

/// 按 Unicode 标量字符区间 `[start_cp, end_cp)` 安全切片 UTF-8 字符串。
std::string utf8_char_slice(const std::string &s, int32_t start_cp, int32_t end_cp) {
    if (start_cp >= end_cp || s.empty()) return {};
    size_t start_byte = s.size();
    size_t end_byte = s.size();
    int32_t cp_idx = 0;
    for (size_t i = 0; i < s.size(); ++i) {
        if ((static_cast<unsigned char>(s[i]) & 0xC0u) != 0x80u) {
            if (cp_idx == start_cp) start_byte = i;
            if (cp_idx == end_cp) {
                end_byte = i;
                break;
            }
            ++cp_idx;
        }
    }
    if (start_byte >= s.size() || start_byte >= end_byte) return {};
    return s.substr(start_byte, end_byte - start_byte);
}

/// 严格解码 UTF-8，要求字节流完整合法且全部为 1..max_chars 个纯汉字（自动过滤残缺 UTF-8 字节碎片）。
bool decode_strict_utf8_hanzi(const char *data, size_t len, size_t max_chars, std::vector<char32_t> &out) {
    out.clear();
    if (!data || len == 0) return false;
    size_t i = 0;
    while (i < len) {
        const unsigned char b0 = static_cast<unsigned char>(data[i]);
        char32_t cp = 0;
        size_t need = 0;
        if (b0 < 0x80u) {
            return false;  // ASCII 非汉字直接跳过
        } else if ((b0 & 0xE0u) == 0xC0u) {
            need = 2;
            cp = b0 & 0x1Fu;
        } else if ((b0 & 0xF0u) == 0xE0u) {
            need = 3;
            cp = b0 & 0x0Fu;
        } else if ((b0 & 0xF8u) == 0xF0u) {
            need = 4;
            cp = b0 & 0x07u;
        } else {
            return false;
        }
        if (i + need > len) return false;
        for (size_t k = 1; k < need; ++k) {
            const unsigned char bk = static_cast<unsigned char>(data[i + k]);
            if ((bk & 0xC0u) != 0x80u) return false;
            cp = (cp << 6) | (bk & 0x3Fu);
        }
        // 拒绝过长编码和 UTF-16 代理区。
        if ((need == 2 && cp < 0x80u) ||
            (need == 3 && cp < 0x800u) ||
            (need == 4 && (cp < 0x10000u || cp > 0x10FFFFu)) ||
            (cp >= 0xD800u && cp <= 0xDFFFu)) {
            return false;
        }
        if (!is_cjk_hanzi(cp)) return false;
        out.push_back(cp);
        if (out.size() > max_chars) return false;
        i += need;
    }
    return !out.empty();
}

std::vector<std::string> split_syllables(const std::string &joined) {
    std::vector<std::string> parts;
    std::string cur;
    for (char c : joined) {
        if (c == '\'' || c == ' ' || c == '\t') {
            if (!cur.empty()) {
                parts.push_back(std::move(cur));
                cur.clear();
            }
        } else if (c >= 'A' && c <= 'Z') {
            cur.push_back(static_cast<char>(c - 'A' + 'a'));
        } else if (c >= 'a' && c <= 'z') {
            cur.push_back(c);
        }
    }
    if (!cur.empty()) parts.push_back(std::move(cur));
    return parts;
}

/// 数值稳定且带 ARM NEON 16-路向量化加速与尾部截断的 `log_sum_exp`：
/// 对 `x < max_val - 20.0f` 的项 `exp(x - max) < 2.06e-9`，跳过 96%+ 无效 `exp` 调用，与标量版逐比特等价。
double fast_log_sum_exp(const float *logits, int32_t n_vocab) {
    if (!logits || n_vocab <= 0) return 0.0;
    float max_val = logits[0];
    int32_t i = 0;
#ifdef __aarch64__
    if (n_vocab >= 16) {
        float32x4_t vmax0 = vld1q_f32(logits);
        float32x4_t vmax1 = vld1q_f32(logits + 4);
        float32x4_t vmax2 = vld1q_f32(logits + 8);
        float32x4_t vmax3 = vld1q_f32(logits + 12);
        i = 16;
        const int32_t vec_end = n_vocab & ~15;
        for (; i < vec_end; i += 16) {
            vmax0 = vmaxq_f32(vmax0, vld1q_f32(logits + i));
            vmax1 = vmaxq_f32(vmax1, vld1q_f32(logits + i + 4));
            vmax2 = vmaxq_f32(vmax2, vld1q_f32(logits + i + 8));
            vmax3 = vmaxq_f32(vmax3, vld1q_f32(logits + i + 12));
        }
        vmax0 = vmaxq_f32(vmaxq_f32(vmax0, vmax1), vmaxq_f32(vmax2, vmax3));
        max_val = vmaxvq_f32(vmax0);
    }
#endif
    for (; i < n_vocab; ++i) {
        const float v = logits[i];
        if (v > max_val) max_val = v;
    }
    const float cutoff = max_val - 20.0f;
    double sum_exp = 0.0;
    int32_t j = 0;
#ifdef __aarch64__
    if (n_vocab >= 16) {
        const float32x4_t vcut = vdupq_n_f32(cutoff);
        const int32_t vec_end = n_vocab & ~15;
        for (; j < vec_end; j += 16) {
            const uint32x4_t m0 = vcgeq_f32(vld1q_f32(logits + j), vcut);
            const uint32x4_t m1 = vcgeq_f32(vld1q_f32(logits + j + 4), vcut);
            const uint32x4_t m2 = vcgeq_f32(vld1q_f32(logits + j + 8), vcut);
            const uint32x4_t m3 = vcgeq_f32(vld1q_f32(logits + j + 12), vcut);
            const uint32x4_t any = vorrq_u32(vorrq_u32(m0, m1), vorrq_u32(m2, m3));
            if (vmaxvq_u32(any) == 0u) continue;
            for (int32_t k = j; k < j + 16; ++k) {
                const float v = logits[k];
                if (v >= cutoff) {
                    sum_exp += static_cast<double>(std::exp(v - max_val));
                }
            }
        }
    }
#endif
    for (; j < n_vocab; ++j) {
        const float v = logits[j];
        if (v >= cutoff) {
            sum_exp += static_cast<double>(std::exp(v - max_val));
        }
    }
    return static_cast<double>(max_val) + std::log(sum_exp);
}

struct ParallelLogZCtx {
    const float *const *logits_ptrs = nullptr;
    double *log_zs = nullptr;
    int32_t n_vocab = 0;
};

#ifdef __APPLE__
void parallel_log_z_worker(void *raw_ctx, size_t idx) {
    auto *ctx = static_cast<ParallelLogZCtx *>(raw_ctx);
    const float *ptr = ctx->logits_ptrs[idx];
    ctx->log_zs[idx] = ptr ? fast_log_sum_exp(ptr, ctx->n_vocab) : 0.0;
}
#endif

/// 对多组全词表 logits 并行计算 `log_sum_exp`（M3 Max 多核加速 3–4x，结果与单线程逐比特一致）。
void compute_log_zs_batch(
    const std::vector<const float *> &logits_ptrs,
    int32_t n_vocab,
    std::vector<double> &out_log_zs) {
    const size_t n = logits_ptrs.size();
    out_log_zs.assign(n, 0.0);
    if (n == 0) return;
#ifdef __APPLE__
    if (n >= 2) {
        ParallelLogZCtx work{};
        work.logits_ptrs = logits_ptrs.data();
        work.log_zs = out_log_zs.data();
        work.n_vocab = n_vocab;
        dispatch_apply_f(
            n,
            dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0),
            &work,
            parallel_log_z_worker);
        return;
    }
#endif
    for (size_t i = 0; i < n; ++i) {
        out_log_zs[i] = logits_ptrs[i] ? fast_log_sum_exp(logits_ptrs[i], n_vocab) : 0.0;
    }
}

#ifdef __APPLE__
/// 通过 macOS CoreFoundation ICU `Traditional-Simplified` 转换检测纯繁体字形，返回对应的简体标量；
/// 若本就是简体或非单字映射，返回原码点。仅在启动建索引时调用一次（总耗时 < 3 ms）。
char32_t simplified_counterpart(char32_t cp) {
    if (cp > 0xFFFFu) return cp;
    const UniChar uc = static_cast<UniChar>(cp);
    CFMutableStringRef ms = CFStringCreateMutable(kCFAllocatorDefault, 4);
    if (!ms) return cp;
    CFStringAppendCharacters(ms, &uc, 1);
    char32_t result = cp;
    if (CFStringTransform(ms, nullptr, CFSTR("Traditional-Simplified"), false) &&
        CFStringGetLength(ms) == 1) {
        result = static_cast<char32_t>(CFStringGetCharacterAtIndex(ms, 0));
    }
    CFRelease(ms);
    return result;
}
#endif

void compute_full_log_softmax(const float *logits, int32_t n_vocab, std::vector<float> &out) {
    out.resize(static_cast<size_t>(n_vocab));
    const float log_z = static_cast<float>(fast_log_sum_exp(logits, n_vocab));
    for (int32_t i = 0; i < n_vocab; ++i) {
        out[static_cast<size_t>(i)] = logits[i] - log_z;
    }
}

void batch_clear(llama_batch &batch) {
    batch.n_tokens = 0;
}

void batch_add(
    llama_batch &batch,
    llama_token token,
    llama_pos pos,
    const std::vector<llama_seq_id> &seq_ids,
    bool need_logits) {
    const int32_t idx = batch.n_tokens;
    batch.token[idx] = token;
    batch.pos[idx] = pos;
    batch.n_seq_id[idx] = static_cast<int32_t>(seq_ids.size());
    for (size_t i = 0; i < seq_ids.size(); ++i) {
        batch.seq_id[idx][i] = seq_ids[i];
    }
    batch.logits[idx] = need_logits ? 1 : 0;
    batch.n_tokens = idx + 1;
}

void batch_add_single_seq(
    llama_batch &batch,
    llama_token token,
    llama_pos pos,
    llama_seq_id seq_id,
    bool need_logits) {
    const int32_t idx = batch.n_tokens;
    batch.token[idx] = token;
    batch.pos[idx] = pos;
    batch.n_seq_id[idx] = 1;
    batch.seq_id[idx][0] = seq_id;
    batch.logits[idx] = need_logits ? 1 : 0;
    batch.n_tokens = idx + 1;
}

/// 单条候选的细粒度惊奇度诊断（零额外 GPU 推理开销，直接复用前缀树每步边对数概率）。
struct CandidateScoreDiag {
    double total_lp = -1e9;
    double avg_char_lp = -1e9;
    double min_edge_lp = -1e9;
    int32_t worst_char_start = 0;
    int32_t worst_char_end = 0;
    int32_t char_count = 0;
    int32_t token_count = 0;
};

}  // namespace

struct NeuralEngine::Impl {
    std::mutex mutex;
    std::string model_path;
    llama_model *model = nullptr;
    const llama_vocab *vocab = nullptr;
    llama_context *ctx = nullptr;
    llama_batch batch{};
    bool batch_initialized = false;

    int32_t n_ctx = 2048;
    int32_t n_batch = 512;
    int32_t n_seq_max = 20;
    int32_t n_vocab = 0;
    int32_t max_ctx_tokens = 512;
    llama_token seed_token = 0;
    bool model_adds_bos = false;

    // 固定注意力锚点与用户输入起始前缀（常驻 `seq_0` 头部并被 LCP 100% 复用，
    // 彻底消除 Qwen3 等无 BOS 模型在空上文或 1-token 上文时 pos=0 退化倒置问题）。
    // 空上文时额外在起始前缀后附中性中文分句符 `，`，消除 Instruct 模型句首助手提示偏置。
    std::vector<llama_token> prompt_prefix_tokens;
    std::vector<llama_token> empty_prompt_tokens;
    // 中文 IM 聊天常省略句间逗号，缓存单个中文逗号 token 供前缀树单遍贝叶斯边际化复用。
    llama_token comma_token = -1;

    // `seq_id = 0` 持久化上文状态与末尾 token 的精确 log-softmax 缓存。
    std::vector<llama_token> cached_ctx_tokens;
    std::vector<float> cached_ctx_log_probs;
    bool cached_ctx_valid = false;
    bool cached_ctx_was_empty = true;
    bool cached_ctx_ends_with_han = false;

    // 能力 C：受限束搜索倒排索引（`音节串 -> 合法纯汉字 token 列表`）及每个 token 的纯汉字 UTF-8 文本。
    bool constrained_vocab_ready = false;
    VocabIndexStats vocab_stats{};
    std::unordered_map<std::string, std::vector<llama_token>> syllable_to_tokens;
    std::vector<std::string> token_hanzi_text;
    std::vector<int32_t> token_char_lens;

    ~Impl() {
        if (batch_initialized) {
            llama_batch_free(batch);
            batch_initialized = false;
        }
        if (ctx) {
            llama_free(ctx);
            ctx = nullptr;
        }
        if (model) {
            llama_model_free(model);
            model = nullptr;
        }
    }

    bool tokenize_raw(const std::string &text, std::vector<llama_token> &out) const {
        out.clear();
        if (text.empty()) return true;
        int32_t cap = static_cast<int32_t>(text.size()) + 16;
        out.resize(static_cast<size_t>(cap));
        int32_t n = llama_tokenize(
            vocab,
            text.data(),
            static_cast<int32_t>(text.size()),
            out.data(),
            cap,
            false,
            false);
        if (n < 0) {
            cap = -n;
            out.resize(static_cast<size_t>(cap));
            n = llama_tokenize(
                vocab,
                text.data(),
                static_cast<int32_t>(text.size()),
                out.data(),
                cap,
                false,
                false);
        }
        if (n < 0) {
            out.clear();
            return false;
        }
        out.resize(static_cast<size_t>(n));
        return true;
    }

    llama_token resolve_seed_token() const {
        const llama_token bos = llama_vocab_bos(vocab);
        if (bos != LLAMA_TOKEN_NULL) return bos;
        // Qwen3 常见 `<|endoftext|>` 为文档起始分隔符。
        llama_token special = LLAMA_TOKEN_NULL;
        const char *eot_str = "<|endoftext|>";
        if (llama_tokenize(vocab, eot_str, static_cast<int32_t>(std::strlen(eot_str)), &special, 1, false, true) == 1 &&
            special != LLAMA_TOKEN_NULL) {
            return special;
        }
        const llama_token eos = llama_vocab_eos(vocab);
        if (eos != LLAMA_TOKEN_NULL) return eos;
        llama_token nl = 0;
        if (llama_tokenize(vocab, "\n", 1, &nl, 1, false, false) == 1) return nl;
        return 0;
    }

    /// 构造固定常驻前缀 `prompt_prefix_tokens`：
    /// 1. 仅当模型元数据声明了对话模板（Instruct/Chat 模型）且词表包含 `<|im_start|>` 时使用
    ///    `<|im_start|>user\n`（3 tokens），使其稳定处于「自然用户输入文本」分布；
    /// 2. 纯预训练基座模型（无 chat_template，如 `Qwen3-0.6B-Base`、`Qwen2.5-0.5B`）或非 ChatML 模型
    ///    使用 `[seed_token, '\n']`（2 tokens）作为文档起始注意力锚点，杜绝 pos=0 退化与模板污染。
    std::vector<llama_token> resolve_prompt_prefix() const {
        const char *chat_tmpl = model ? llama_model_chat_template(model, nullptr) : nullptr;
        const bool has_chat_template = (chat_tmpl != nullptr && chat_tmpl[0] != '\0');
        llama_token im_start = LLAMA_TOKEN_NULL;
        const char *tag = "<|im_start|>";
        if (has_chat_template &&
            llama_tokenize(vocab, tag, static_cast<int32_t>(std::strlen(tag)), &im_start, 1, false, true) == 1 &&
            im_start != LLAMA_TOKEN_NULL) {
            const char *chat_header = "<|im_start|>user\n";
            std::vector<llama_token> toks(16);
            const int32_t n = llama_tokenize(
                vocab,
                chat_header,
                static_cast<int32_t>(std::strlen(chat_header)),
                toks.data(),
                static_cast<int32_t>(toks.size()),
                false,
                true);
            if (n >= 2 && n <= static_cast<int32_t>(toks.size()) && toks[0] == im_start) {
                toks.resize(static_cast<size_t>(n));
                return toks;
            }
        }
        std::vector<llama_token> fallback;
        fallback.push_back(seed_token);
        llama_token nl = LLAMA_TOKEN_NULL;
        if (llama_tokenize(vocab, "\n", 1, &nl, 1, false, false) == 1 && nl != LLAMA_TOKEN_NULL) {
            fallback.push_back(nl);
        }
        return fallback;
    }

    /// 构造空上文专用起始前缀：在 `prompt_prefix_tokens` 后追加中性中文分句符 `，`，
    /// 将空上文下的模型分布从“AI 指令开头”拉回自然中文语句续写分布（非空上文保持原样不加逗号）。
    std::vector<llama_token> resolve_empty_prompt_prefix(const std::vector<llama_token> &base_prefix) const {
        std::vector<llama_token> out = base_prefix;
        std::vector<llama_token> comma_toks;
        if (tokenize_raw("，", comma_toks) && !comma_toks.empty()) {
            out.insert(out.end(), comma_toks.begin(), comma_toks.end());
        }
        return out;
    }

    int32_t token_utf8_chars(llama_token tid) const {
        if (tid < 0 || tid >= n_vocab) return 1;
        const size_t idx = static_cast<size_t>(tid);
        if (idx < token_char_lens.size() && token_char_lens[idx] > 0) {
            return token_char_lens[idx];
        }
        char buf[128];
        const int32_t plen = llama_token_to_piece(vocab, tid, buf, static_cast<int32_t>(sizeof(buf)), 0, false);
        if (plen <= 0 || plen >= static_cast<int32_t>(sizeof(buf))) return 1;
        return std::max<int32_t>(1, count_utf8_codepoints(buf, static_cast<size_t>(plen)));
    }

    bool build_constrained_vocab_index(const std::string &pinyin_db_path, const std::string &pinyin_tsv_path) {
        const auto start = Clock::now();
        std::unordered_map<char32_t, std::vector<std::pair<int64_t, std::string>>> raw_map;

        // 1. 只读读取主词库 `tbl_1_*` 单字表（与 `crates/engine/src/format.rs` 契约一致）。
        if (!pinyin_db_path.empty() && std::filesystem::exists(pinyin_db_path)) {
            sqlite3 *db = nullptr;
            if (sqlite3_open_v2(pinyin_db_path.c_str(), &db, SQLITE_OPEN_READONLY, nullptr) == SQLITE_OK && db) {
                const char *initials = "abcdefghjklmnopqrstwxyz";
                std::vector<char32_t> cps;
                for (const char *p = initials; *p != '\0'; ++p) {
                    char sql[128];
                    std::snprintf(sql, sizeof(sql), "SELECT key, value, weight FROM tbl_1_%c", *p);
                    sqlite3_stmt *stmt = nullptr;
                    if (sqlite3_prepare_v2(db, sql, -1, &stmt, nullptr) != SQLITE_OK) {
                        continue;
                    }
                    while (sqlite3_step(stmt) == SQLITE_ROW) {
                        const unsigned char *k_txt = sqlite3_column_text(stmt, 0);
                        const unsigned char *v_txt = sqlite3_column_text(stmt, 1);
                        const int64_t weight = sqlite3_column_int64(stmt, 2);
                        if (!k_txt || !v_txt) continue;
                        const char *v_str = reinterpret_cast<const char *>(v_txt);
                        if (!decode_strict_utf8_hanzi(v_str, std::strlen(v_str), 1, cps) || cps.size() != 1) {
                            continue;
                        }
                        auto sylls = split_syllables(reinterpret_cast<const char *>(k_txt));
                        if (sylls.size() != 1) continue;
                        raw_map[cps[0]].emplace_back(weight, std::move(sylls[0]));
                    }
                    sqlite3_finalize(stmt);
                }
                sqlite3_close(db);
            } else if (db) {
                sqlite3_close(db);
            }
        }

        // 2. 可选 TSV 补充单字拼音表（每行 `汉字\t音节1,音节2`）。
        if (!pinyin_tsv_path.empty() && std::filesystem::exists(pinyin_tsv_path)) {
            std::ifstream in(pinyin_tsv_path);
            std::string line;
            std::vector<char32_t> cps;
            while (std::getline(in, line)) {
                const size_t tab = line.find('\t');
                if (tab == std::string::npos || tab == 0) continue;
                if (!decode_strict_utf8_hanzi(line.data(), tab, 1, cps) || cps.size() != 1) continue;
                std::string rest = line.substr(tab + 1);
                size_t pos = 0;
                int64_t synthetic_weight = 10000;
                while (pos < rest.size()) {
                    size_t comma = rest.find(',', pos);
                    if (comma == std::string::npos) comma = rest.size();
                    auto sylls = split_syllables(rest.substr(pos, comma - pos));
                    if (sylls.size() == 1) {
                        raw_map[cps[0]].emplace_back(synthetic_weight, std::move(sylls[0]));
                        if (synthetic_weight > 1) --synthetic_weight;
                    }
                    pos = comma + 1;
                }
            }
        }

        if (raw_map.empty()) {
            constrained_vocab_ready = false;
            return false;
        }

        // 统计每个汉字的最高读音权重，用于剔除生僻古音/零权读音及纯繁体异体字。
        std::unordered_map<char32_t, int64_t> char_max_weight;
        char_max_weight.reserve(raw_map.size());
        for (const auto &[cp, entries] : raw_map) {
            int64_t best = 0;
            for (const auto &[w, s] : entries) {
                (void)s;
                if (w > best) best = w;
            }
            char_max_weight.emplace(cp, best);
        }

        // 每个汉字的读音按词频降序去重，过滤低权古音与纯繁体异体字。
        std::unordered_map<char32_t, std::vector<std::string>> hanzi_to_pinyins;
        hanzi_to_pinyins.reserve(raw_map.size());
        for (auto &[cp, entries] : raw_map) {
            const int64_t max_w = char_max_weight[cp];
            if (max_w < 100) continue;
#ifdef __APPLE__
            const char32_t simp = simplified_counterpart(cp);
            if (simp != cp) {
                auto sit = char_max_weight.find(simp);
                if (sit != char_max_weight.end() && sit->second > max_w * 4) {
                    continue;
                }
            }
#endif
            std::sort(entries.begin(), entries.end(), [](const auto &a, const auto &b) {
                return a.first > b.first;
            });
            // 过滤低于本字最高读音权重 2.0% 的生僻古音（保留现代常用多音字，通用剔除 <2% 古音异读）。
            const int64_t reading_floor = std::max<int64_t>(100, static_cast<int64_t>(static_cast<double>(max_w) * 0.02));
            std::vector<std::string> unique_sylls;
            for (auto &[w, s] : entries) {
                if (w < reading_floor && !unique_sylls.empty()) continue;
                if (w < 100) continue;
                if (std::find(unique_sylls.begin(), unique_sylls.end(), s) == unique_sylls.end()) {
                    unique_sylls.push_back(std::move(s));
                }
            }
            if (!unique_sylls.empty()) {
                hanzi_to_pinyins.emplace(cp, std::move(unique_sylls));
            }
        }

        token_hanzi_text.assign(static_cast<size_t>(n_vocab), std::string{});
        token_char_lens.assign(static_cast<size_t>(n_vocab), 0);
        syllable_to_tokens.clear();
        syllable_to_tokens.reserve(65536);

        char piece_buf[128];
        std::vector<char32_t> cps;
        int32_t indexed_tokens = 0;

        for (llama_token tid = 0; tid < n_vocab; ++tid) {
            if (llama_vocab_is_control(vocab, tid)) continue;
            const int32_t plen = llama_token_to_piece(
                vocab, tid, piece_buf, static_cast<int32_t>(sizeof(piece_buf)), 0, false);
            if (plen <= 0 || plen >= static_cast<int32_t>(sizeof(piece_buf))) continue;
            token_char_lens[static_cast<size_t>(tid)] =
                std::max<int32_t>(1, count_utf8_codepoints(piece_buf, static_cast<size_t>(plen)));
            if (!decode_strict_utf8_hanzi(piece_buf, static_cast<size_t>(plen), 4, cps)) continue;

            std::vector<const std::vector<std::string> *> per_char;
            per_char.reserve(cps.size());
            bool all_found = true;
            for (char32_t cp : cps) {
                auto it = hanzi_to_pinyins.find(cp);
                if (it == hanzi_to_pinyins.end()) {
                    all_found = false;
                    break;
                }
                per_char.push_back(&it->second);
            }
            if (!all_found) continue;

            token_hanzi_text[static_cast<size_t>(tid)].assign(piece_buf, static_cast<size_t>(plen));
            ++indexed_tokens;

            std::vector<std::string> combos;
            combos.emplace_back();
            for (size_t c_idx = 0; c_idx < per_char.size(); ++c_idx) {
                const auto &readings = *per_char[c_idx];
                std::vector<std::string> next_combos;
                next_combos.reserve(std::min<size_t>(16, combos.size() * readings.size()));
                for (const std::string &prefix : combos) {
                    for (const std::string &r : readings) {
                        if (next_combos.size() >= 16) break;
                        std::string joined = prefix;
                        if (!joined.empty()) joined.push_back('\'');
                        joined += r;
                        next_combos.push_back(std::move(joined));
                    }
                    if (next_combos.size() >= 16) break;
                }
                combos = std::move(next_combos);
            }
            for (const std::string &key : combos) {
                auto &vec = syllable_to_tokens[key];
                if (vec.empty() || vec.back() != tid) {
                    vec.push_back(tid);
                }
            }
        }

        vocab_stats.hanzi_entries = static_cast<int32_t>(hanzi_to_pinyins.size());
        vocab_stats.indexed_tokens = indexed_tokens;
        vocab_stats.syllable_keys = static_cast<int32_t>(syllable_to_tokens.size());
        vocab_stats.build_ms = elapsed_ms_since(start);
        constrained_vocab_ready = (indexed_tokens > 0);
        return constrained_vocab_ready;
    }

    /// 内部无锁版上文预热（调用方须已持有 `mutex`）。
    /// 非空上文前置 `prompt_prefix_tokens`（`<|im_start|>user\n`）；空上文前置 `empty_prompt_tokens`（追加中性 `，`）。
    /// 两者共享 `prompt_prefix_tokens` 头部并被 LCP 100% 零开销复用。
    bool prefill_context_locked(const std::string &context_utf8, PrefillStats &stats, std::string &error_out) {
        const auto start = Clock::now();
        stats = PrefillStats{};

        const bool is_empty_context = context_utf8.empty();
        std::vector<llama_token> target = is_empty_context ? empty_prompt_tokens : prompt_prefix_tokens;
        if (target.empty()) {
            target.push_back(seed_token);
        }
        const size_t prefix_len = target.size();

        if (!is_empty_context) {
            std::vector<llama_token> ctx_toks;
            if (!tokenize_raw(context_utf8, ctx_toks)) {
                error_out = "上文分词失败";
                return false;
            }
            const size_t max_user_toks = (static_cast<size_t>(max_ctx_tokens) > prefix_len)
                ? (static_cast<size_t>(max_ctx_tokens) - prefix_len)
                : 0;
            if (ctx_toks.size() > max_user_toks) {
                const size_t drop = ctx_toks.size() - max_user_toks;
                ctx_toks.erase(ctx_toks.begin(), ctx_toks.begin() + static_cast<std::ptrdiff_t>(drop));
            }
            target.insert(target.end(), ctx_toks.begin(), ctx_toks.end());
        }

        stats.total_tokens = static_cast<int32_t>(target.size());

        // 判断 UTF-8 上文是否直接以汉字结尾（无句间标点或空格），用于决定是否启用隐式分句逗号边际化。
        auto ends_with_han_utf8 = [](const std::string &s) -> bool {
            if (s.empty()) return false;
            size_t i = s.size();
            while (i > 0 && (static_cast<unsigned char>(s[i - 1]) & 0xC0u) == 0x80u) {
                --i;
            }
            if (i == 0) return false;
            const size_t start_idx = i - 1;
            const unsigned char b0 = static_cast<unsigned char>(s[start_idx]);
            uint32_t cp = 0;
            if ((b0 & 0xF0u) == 0xE0u && start_idx + 2 < s.size()) {
                cp = ((b0 & 0x0Fu) << 12)
                    | ((static_cast<unsigned char>(s[start_idx + 1]) & 0x3Fu) << 6)
                    | (static_cast<unsigned char>(s[start_idx + 2]) & 0x3Fu);
            } else if ((b0 & 0xF8u) == 0xF0u && start_idx + 3 < s.size()) {
                cp = ((b0 & 0x07u) << 18)
                    | ((static_cast<unsigned char>(s[start_idx + 1]) & 0x3Fu) << 12)
                    | ((static_cast<unsigned char>(s[start_idx + 2]) & 0x3Fu) << 6)
                    | (static_cast<unsigned char>(s[start_idx + 3]) & 0x3Fu);
            } else {
                return false;
            }
            return (cp >= 0x4E00u && cp <= 0x9FFFu) || (cp >= 0x3400u && cp <= 0x4DBFu)
                || (cp >= 0x20000u && cp <= 0x2A6DFu);
        };
        const bool ctx_ends_with_han = !is_empty_context && ends_with_han_utf8(context_utf8);

        // 快速路径：完全相同上文（含连续空上文或同句连续按键）0.01 ms 复用。
        if (cached_ctx_valid && cached_ctx_tokens == target) {
            cached_ctx_was_empty = is_empty_context;
            cached_ctx_ends_with_han = ctx_ends_with_han;
            stats.reused_tokens = stats.total_tokens;
            stats.decoded_tokens = 0;
            stats.elapsed_ms = elapsed_ms_since(start);
            return true;
        }

        llama_memory_t mem = llama_get_memory(ctx);

        size_t lcp = 0;
        if (cached_ctx_valid) {
            const size_t limit = std::min(cached_ctx_tokens.size(), target.size());
            while (lcp < limit && cached_ctx_tokens[lcp] == target[lcp]) {
                ++lcp;
            }
            // 若新上文是旧上文的真前缀，需重算新上文最后一个 token 以刷新末尾 logits 缓存。
            if (lcp == target.size() && lcp > 0) {
                --lcp;
            }
        }

        if (lcp == 0) {
            llama_memory_seq_rm(mem, 0, -1, -1);
        } else if (lcp < cached_ctx_tokens.size()) {
            llama_memory_seq_rm(mem, 0, static_cast<llama_pos>(lcp), -1);
        }

        // 增量解码 `target[lcp .. target.size())`，仅在最后一个 token 打开 logits 开关。
        const size_t total = target.size();
        size_t cursor = lcp;
        while (cursor < total) {
            const size_t chunk = std::min<size_t>(static_cast<size_t>(n_batch), total - cursor);
            batch_clear(batch);
            for (size_t i = 0; i < chunk; ++i) {
                const size_t pos = cursor + i;
                const bool is_last = (pos + 1 == total);
                batch_add_single_seq(batch, target[pos], static_cast<llama_pos>(pos), 0, is_last);
            }
            const int32_t rc = llama_decode(ctx, batch);
            if (rc != 0) {
                cached_ctx_valid = false;
                cached_ctx_tokens.clear();
                llama_memory_seq_rm(mem, 0, -1, -1);
                error_out = "llama_decode 上文预热失败，返回码=" + std::to_string(rc);
                return false;
            }
            cursor += chunk;
        }

        const float *last_logits = llama_get_logits_ith(ctx, batch.n_tokens - 1);
        if (!last_logits) {
            cached_ctx_valid = false;
            error_out = "获取上文末尾 logits 失败";
            return false;
        }
        compute_full_log_softmax(last_logits, n_vocab, cached_ctx_log_probs);

        cached_ctx_tokens = std::move(target);
        cached_ctx_valid = true;
        cached_ctx_was_empty = is_empty_context;
        cached_ctx_ends_with_han = ctx_ends_with_han;

        stats.reused_tokens = static_cast<int32_t>(lcp);
        stats.decoded_tokens = static_cast<int32_t>(total - lcp);
        stats.elapsed_ms = elapsed_ms_since(start);
        return true;
    }

    struct TrieNode {
        llama_token token = 0;
        int32_t parent = -1;
        int32_t depth = -1;
        std::vector<std::pair<llama_token, int32_t>> children;
        double edge_log_prob = 0.0;
        double cum_log_prob = 0.0;
        bool scored = false;
        bool is_internal = false;
        int32_t branch_begin = 0;
        int32_t branch_end = 0;
    };

    /// 内部无锁版前缀树共享 KV 批量打分，可选输出每条候选的细粒度惊奇度诊断（零额外 GPU 推理）。
    /// 当 `marginalize_latent_comma == true` 且非空上文直接以汉字结尾时，在同一棵前缀树内并入单节点
    /// 隐式分句逗号分支，按全概率公式对「直接续写 / IM 省略逗号起新分句」做精确贝叶斯边际化：
    /// `P(Y | C) = P_LM(Y | C) + alpha_omit * P_LM("，" | C) * P_LM(Y | C + "，")`。
    bool score_candidates_with_diag_locked(
        const std::string *context_utf8,
        const std::vector<std::string> &candidates,
        std::vector<double> &out_log_probs,
        std::vector<CandidateScoreDiag> *out_diags,
        ScoreStats &stats,
        std::string &error_out,
        bool marginalize_latent_comma = false) {
        const auto start = Clock::now();
        stats = ScoreStats{};
        stats.candidate_count = static_cast<int32_t>(candidates.size());
        out_log_probs.assign(candidates.size(), -1e9);
        if (out_diags) {
            out_diags->assign(candidates.size(), CandidateScoreDiag{});
        }

        if (context_utf8 != nullptr) {
            PrefillStats pf{};
            if (!prefill_context_locked(*context_utf8, pf, error_out)) return false;
        } else if (!cached_ctx_valid) {
            PrefillStats pf{};
            if (!prefill_context_locked("", pf, error_out)) return false;
        }

        if (candidates.empty()) {
            stats.elapsed_ms = elapsed_ms_since(start);
            return true;
        }

        const int32_t ctx_len = static_cast<int32_t>(cached_ctx_tokens.size());
        const int32_t max_cand_len = std::max<int32_t>(1, n_ctx - ctx_len - 3);
        // 仅当模型自身对上文末尾出现分句逗号赋予不可忽略概率（`P_LM("，" | C) >= e^-4.5 ≈ 1.11%`）时
        // 才在同一前缀树内展开隐式逗号子树；句中连续语境处 `P_LM("，" | C)` 极低，零开销直接跳过。
        constexpr double kLatentCommaTriggerMinLogProb = -4.5;
        const bool use_latent_comma = marginalize_latent_comma
            && !cached_ctx_was_empty
            && cached_ctx_ends_with_han
            && comma_token >= 0
            && comma_token < n_vocab
            && static_cast<size_t>(comma_token) < cached_ctx_log_probs.size()
            && static_cast<double>(cached_ctx_log_probs[static_cast<size_t>(comma_token)]) >= kLatentCommaTriggerMinLogProb
            && max_cand_len >= 2;

        // 1. 构造 Token 前缀字典树（根节点 0 代表已预热上文末尾）。
        std::vector<TrieNode> trie;
        trie.reserve(candidates.size() * (use_latent_comma ? 6 : 4) + 2);
        trie.push_back(TrieNode{});
        trie[0].depth = -1;
        trie[0].scored = true;

        auto insert_child = [&](int32_t parent_id, llama_token tok, int32_t depth) -> int32_t {
            for (const auto &ch : trie[static_cast<size_t>(parent_id)].children) {
                if (ch.first == tok) {
                    return ch.second;
                }
            }
            const int32_t next_id = static_cast<int32_t>(trie.size());
            TrieNode node{};
            node.token = tok;
            node.parent = parent_id;
            node.depth = depth;
            trie.push_back(std::move(node));
            trie[static_cast<size_t>(parent_id)].children.emplace_back(tok, next_id);
            return next_id;
        };

        std::vector<int32_t> cand_end_node(candidates.size(), -1);
        std::vector<int32_t> cand_comma_end_node(candidates.size(), -1);
        std::vector<std::vector<llama_token>> cand_tokens(candidates.size());
        int32_t total_cand_tokens = 0;

        for (size_t i = 0; i < candidates.size(); ++i) {
            if (candidates[i].empty()) continue;
            auto &toks = cand_tokens[i];
            if (!tokenize_raw(candidates[i], toks) || toks.empty()) continue;
            if (static_cast<int32_t>(toks.size()) > max_cand_len) {
                toks.resize(static_cast<size_t>(max_cand_len));
            }
            total_cand_tokens += static_cast<int32_t>(toks.size());

            int32_t cur = 0;
            for (size_t d = 0; d < toks.size(); ++d) {
                cur = insert_child(cur, toks[d], static_cast<int32_t>(d));
            }
            cand_end_node[i] = cur;
        }

        // 在不撑破单批序列上限（`n_seq_max - 1`）的前提下挂载共享隐式逗号子树：
        // - 单 token 候选（绝大多数中文常用双字词）在逗号节点下直接成为叶子，零额外序列与零额外解码 token，全量纳入；
        // - 多 token 候选按 Tier 0 优先级在剩余单批序列预算内纳入，杜绝触发二次 Metal 分块前向。
        int32_t comma_node_id = -1;
        if (use_latent_comma) {
            int32_t direct_internal_leaves = 0;
            for (size_t id = 1; id < trie.size(); ++id) {
                if (trie[id].children.empty()) continue;
                bool has_internal_child = false;
                for (const auto &ch : trie[id].children) {
                    if (!trie[static_cast<size_t>(ch.second)].children.empty()) {
                        has_internal_child = true;
                        break;
                    }
                }
                if (!has_internal_child) {
                    ++direct_internal_leaves;
                }
            }
            const int32_t max_chunk_seqs = std::max<int32_t>(1, n_seq_max - 1);
            const int32_t comma_branch_budget = std::max<int32_t>(0, max_chunk_seqs - direct_internal_leaves);
            if (comma_branch_budget >= 1) {
                comma_node_id = insert_child(0, comma_token, 0);
                // 仅当存在至少一条候选时逗号节点本身占 1 个初始内部叶子；每新增一条独立多 token 前缀再消耗 1 预算。
                int32_t multi_tok_added = 0;
                const int32_t max_multi_tok_comma = std::max<int32_t>(0, comma_branch_budget);
                for (size_t i = 0; i < candidates.size(); ++i) {
                    const auto &toks = cand_tokens[i];
                    if (toks.empty()) continue;
                    const size_t c_len = std::min<size_t>(toks.size(), static_cast<size_t>(max_cand_len - 1));
                    if (c_len == 0) continue;
                    if (c_len > 1) {
                        if (multi_tok_added >= max_multi_tok_comma) continue;
                        ++multi_tok_added;
                    }
                    int32_t c_cur = comma_node_id;
                    for (size_t d = 0; d < c_len; ++d) {
                        c_cur = insert_child(c_cur, toks[d], static_cast<int32_t>(d + 1));
                    }
                    cand_comma_end_node[i] = c_cur;
                }
            }
        }

        stats.unique_trie_nodes = static_cast<int32_t>(trie.size()) - 1;
        stats.total_candidate_tokens = total_cand_tokens;

        // 2. 所有 depth == 0 首 token 直接查 `cached_ctx_log_probs`，零 GPU 开销。
        for (const auto &ch : trie[0].children) {
            TrieNode &node = trie[static_cast<size_t>(ch.second)];
            if (node.token >= 0 && node.token < n_vocab) {
                node.edge_log_prob = static_cast<double>(cached_ctx_log_probs[static_cast<size_t>(node.token)]);
            } else {
                node.edge_log_prob = -30.0;
            }
            node.scored = true;
        }

        // 3. 标记需要解码的内部节点（仅有子节点的非根节点需要解码，叶子节点完全免解码）。
        int32_t internal_count = 0;
        for (size_t id = 1; id < trie.size(); ++id) {
            if (!trie[id].children.empty()) {
                trie[id].is_internal = true;
                ++internal_count;
            }
        }
        stats.internal_decoded_nodes = internal_count;

        if (internal_count > 0) {
            int32_t next_branch = 0;
            auto assign_branches = [&](auto &&self, int32_t node_id) -> void {
                TrieNode &node = trie[static_cast<size_t>(node_id)];
                node.branch_begin = next_branch;
                bool has_internal_child = false;
                for (const auto &ch : node.children) {
                    if (trie[static_cast<size_t>(ch.second)].is_internal) {
                        has_internal_child = true;
                        self(self, ch.second);
                    }
                }
                if (!has_internal_child) {
                    ++next_branch;
                }
                node.branch_end = next_branch;
            };
            for (const auto &ch : trie[0].children) {
                if (trie[static_cast<size_t>(ch.second)].is_internal) {
                    assign_branches(assign_branches, ch.second);
                }
            }

            const int32_t total_branches = next_branch;
            const int32_t max_chunk_seqs = std::max<int32_t>(1, n_seq_max - 1);
            llama_memory_t mem = llama_get_memory(ctx);

            std::vector<uint8_t> node_children_scored(trie.size(), 0);
            std::vector<llama_seq_id> seq_ids_buf;
            seq_ids_buf.reserve(static_cast<size_t>(max_chunk_seqs));

            for (int32_t b_start = 0; b_start < total_branches; b_start += max_chunk_seqs) {
                const int32_t b_end = std::min(total_branches, b_start + max_chunk_seqs);
                const int32_t chunk_seqs = b_end - b_start;

                for (int32_t s = 1; s <= chunk_seqs; ++s) {
                    llama_memory_seq_cp(mem, 0, s, -1, -1);
                }

                std::vector<int32_t> active_nodes;
                active_nodes.reserve(static_cast<size_t>(internal_count));
                for (size_t id = 1; id < trie.size(); ++id) {
                    const TrieNode &node = trie[id];
                    if (node.is_internal && node.branch_end > b_start && node.branch_begin < b_end) {
                        active_nodes.push_back(static_cast<int32_t>(id));
                    }
                }
                std::sort(active_nodes.begin(), active_nodes.end(), [&](int32_t a, int32_t b) {
                    if (trie[static_cast<size_t>(a)].depth != trie[static_cast<size_t>(b)].depth) {
                        return trie[static_cast<size_t>(a)].depth < trie[static_cast<size_t>(b)].depth;
                    }
                    return a < b;
                });

                size_t offset = 0;
                bool decode_ok = true;
                while (offset < active_nodes.size()) {
                    const size_t take = std::min<size_t>(static_cast<size_t>(n_batch), active_nodes.size() - offset);
                    batch_clear(batch);
                    std::vector<std::pair<int32_t, int32_t>> logit_slots;
                    logit_slots.reserve(take);

                    for (size_t j = 0; j < take; ++j) {
                        const int32_t node_id = active_nodes[offset + j];
                        const TrieNode &node = trie[static_cast<size_t>(node_id)];
                        seq_ids_buf.clear();
                        const int32_t s_lo = std::max(node.branch_begin, b_start);
                        const int32_t s_hi = std::min(node.branch_end, b_end);
                        for (int32_t b = s_lo; b < s_hi; ++b) {
                            seq_ids_buf.push_back(static_cast<llama_seq_id>((b - b_start) + 1));
                        }
                        const bool need_logits = (node_children_scored[static_cast<size_t>(node_id)] == 0);
                        const int32_t batch_idx = batch.n_tokens;
                        batch_add(
                            batch,
                            node.token,
                            static_cast<llama_pos>(ctx_len + node.depth),
                            seq_ids_buf,
                            need_logits);
                        if (need_logits) {
                            logit_slots.emplace_back(batch_idx, node_id);
                        }
                    }

                    const int32_t rc = llama_decode(ctx, batch);
                    ++stats.decode_batches;
                    if (rc != 0) {
                        error_out = "llama_decode 前缀树批量打分失败，返回码=" + std::to_string(rc);
                        decode_ok = false;
                        break;
                    }

                    // 收集本批所有待算归一化常数的 logits 指针，利用 M3 Max 多核并行算 `log_sum_exp`。
                    std::vector<const float *> logits_ptrs(logit_slots.size(), nullptr);
                    for (size_t k = 0; k < logit_slots.size(); ++k) {
                        logits_ptrs[k] = llama_get_logits_ith(ctx, logit_slots[k].first);
                        if (!logits_ptrs[k]) {
                            error_out = "获取前缀树节点 logits 失败";
                            decode_ok = false;
                            break;
                        }
                    }
                    if (!decode_ok) break;

                    std::vector<double> log_zs;
                    compute_log_zs_batch(logits_ptrs, n_vocab, log_zs);

                    for (size_t k = 0; k < logit_slots.size(); ++k) {
                        const int32_t node_id = logit_slots[k].second;
                        const float *logits = logits_ptrs[k];
                        const double log_z = log_zs[k];
                        const TrieNode &parent = trie[static_cast<size_t>(node_id)];
                        for (const auto &ch : parent.children) {
                            TrieNode &child = trie[static_cast<size_t>(ch.second)];
                            if (ch.first >= 0 && ch.first < n_vocab) {
                                child.edge_log_prob = static_cast<double>(logits[ch.first]) - log_z;
                            } else {
                                child.edge_log_prob = -30.0;
                            }
                            child.scored = true;
                        }
                        node_children_scored[static_cast<size_t>(node_id)] = 1;
                    }
                    offset += take;
                }

                for (int32_t s = 1; s <= chunk_seqs; ++s) {
                    llama_memory_seq_rm(mem, s, -1, -1);
                }
                if (!decode_ok) return false;
            }
        }

        // 4. 自顶向下累加联合条件对数概率 `log P_LM(Y | C)`。
        for (size_t id = 1; id < trie.size(); ++id) {
            TrieNode &node = trie[id];
            const double parent_cum = (node.parent > 0) ? trie[static_cast<size_t>(node.parent)].cum_log_prob : 0.0;
            node.cum_log_prob = parent_cum + node.edge_log_prob;
        }

        // 中文 IM 聊天省略句间逗号的先验系数 `alpha_omit = 0.70`（`log(0.70) ≈ -0.3567`）。
        // 仅当模型自身判断该处适合分句（`P_LM("，" | C)` 足够高）时，逗号分支才显著抬升新分句首词；
        // 短语内部连续语境下 `P_LM("，" | C)` 极低，直连分支自然主导，零误伤。
        constexpr double kLatentCommaOmitLogPrior = -0.35667494393873245;

        std::vector<int32_t> path_nodes;
        for (size_t i = 0; i < candidates.size(); ++i) {
            const int32_t end_id = cand_end_node[i];
            if (end_id <= 0 || !trie[static_cast<size_t>(end_id)].scored) continue;
            const double lp_direct = trie[static_cast<size_t>(end_id)].cum_log_prob;
            double total_lp = lp_direct;
            bool comma_won = false;
            const int32_t c_end = use_latent_comma ? cand_comma_end_node[i] : -1;
            if (c_end > 0 && trie[static_cast<size_t>(c_end)].scored) {
                const double lp_via_comma = kLatentCommaOmitLogPrior + trie[static_cast<size_t>(c_end)].cum_log_prob;
                const double hi = std::max(lp_direct, lp_via_comma);
                const double lo = std::min(lp_direct, lp_via_comma);
                total_lp = hi + std::log1p(std::exp(lo - hi));
                comma_won = (lp_via_comma > lp_direct);
            }
            out_log_probs[i] = total_lp;

            if (out_diags) {
                CandidateScoreDiag diag{};
                diag.total_lp = total_lp;
                diag.char_count = std::max<int32_t>(1, count_utf8_codepoints(candidates[i]));
                diag.avg_char_lp = total_lp / static_cast<double>(diag.char_count);

                path_nodes.clear();
                const int32_t stop_parent = comma_won ? comma_node_id : 0;
                int32_t cur = comma_won ? c_end : end_id;
                while (cur > 0 && cur != stop_parent) {
                    path_nodes.push_back(cur);
                    cur = trie[static_cast<size_t>(cur)].parent;
                }
                std::reverse(path_nodes.begin(), path_nodes.end());
                diag.token_count = static_cast<int32_t>(path_nodes.size());

                // 计算最差单步惊奇度与异常字区间：
                // - 有真实用户上文且直连时检查每一步 `t >= 0`；
                // - 空上文或由隐式分句逗号主导时，对 `t == 0` 给予温和 +1.5 nat 的分句首词熵裕度。
                double worst_metric = 0.0;
                bool has_metric = false;
                int32_t char_cursor = 0;
                for (size_t t = 0; t < path_nodes.size(); ++t) {
                    const TrieNode &nd = trie[static_cast<size_t>(path_nodes[t])];
                    const int32_t tok_chars = token_utf8_chars(nd.token);
                    double metric = nd.edge_log_prob / std::sqrt(static_cast<double>(std::max<int32_t>(1, tok_chars)));
                    if ((cached_ctx_was_empty || comma_won) && t == 0) {
                        metric += 1.5;
                    }
                    if (!has_metric || metric < worst_metric) {
                        worst_metric = metric;
                        has_metric = true;
                        diag.worst_char_start = std::min(char_cursor, diag.char_count - 1);
                        diag.worst_char_end = std::min(char_cursor + tok_chars, diag.char_count);
                    }
                    char_cursor += tok_chars;
                }
                diag.min_edge_lp = has_metric ? worst_metric : diag.avg_char_lp;
                (*out_diags)[i] = diag;
            }
        }

        stats.elapsed_ms = elapsed_ms_since(start);
        return true;
    }

    bool score_candidates_locked(
        const std::string *context_utf8,
        const std::vector<std::string> &candidates,
        std::vector<double> &out_log_probs,
        ScoreStats &stats,
        std::string &error_out) {
        return score_candidates_with_diag_locked(context_utf8, candidates, out_log_probs, nullptr, stats, error_out);
    }

    /// 逐条独立序列精确参考打分（仅微基准测试核对前缀树批处理数值等价性）。
    bool score_candidates_sequential_locked(
        const std::string *context_utf8,
        const std::vector<std::string> &candidates,
        std::vector<double> &out_log_probs,
        double &elapsed_ms_out,
        std::string &error_out) {
        const auto start = Clock::now();
        out_log_probs.assign(candidates.size(), -1e9);

        if (context_utf8 != nullptr) {
            PrefillStats pf{};
            if (!prefill_context_locked(*context_utf8, pf, error_out)) return false;
        } else if (!cached_ctx_valid) {
            PrefillStats pf{};
            if (!prefill_context_locked("", pf, error_out)) return false;
        }

        llama_memory_t mem = llama_get_memory(ctx);
        const int32_t ctx_len = static_cast<int32_t>(cached_ctx_tokens.size());
        std::vector<llama_token> toks;

        for (size_t i = 0; i < candidates.size(); ++i) {
            if (candidates[i].empty()) continue;
            if (!tokenize_raw(candidates[i], toks) || toks.empty()) continue;

            double sum = static_cast<double>(cached_ctx_log_probs[static_cast<size_t>(toks[0])]);
            if (toks.size() > 1) {
                llama_memory_seq_cp(mem, 0, 1, -1, -1);
                batch_clear(batch);
                for (size_t d = 0; d + 1 < toks.size(); ++d) {
                    batch_add_single_seq(batch, toks[d], static_cast<llama_pos>(ctx_len + static_cast<int32_t>(d)), 1, true);
                }
                const int32_t rc = llama_decode(ctx, batch);
                if (rc != 0) {
                    llama_memory_seq_rm(mem, 1, -1, -1);
                    error_out = "参考打分 llama_decode 失败";
                    return false;
                }
                for (size_t d = 0; d + 1 < toks.size(); ++d) {
                    const float *logits = llama_get_logits_ith(ctx, static_cast<int32_t>(d));
                    const double log_z = fast_log_sum_exp(logits, n_vocab);
                    sum += static_cast<double>(logits[toks[d + 1]]) - log_z;
                }
                llama_memory_seq_rm(mem, 1, -1, -1);
            }
            out_log_probs[i] = sum;
        }

        elapsed_ms_out = elapsed_ms_since(start);
        return true;
    }

    struct PendingBeamHyp {
        llama_seq_id parent_seq = 0;
        llama_token last_token = 0;
        int32_t token_count = 0;
        double score = 0.0;
        double lm_log_prob = 0.0;
        std::string text;
    };

    /// 能力 C：按已消费音节数同步的 Batched Constrained Beam Search。
    bool decode_constrained_locked(
        const std::string *context_utf8,
        const std::vector<SyllablePathInput> &paths,
        int32_t beam_width,
        int32_t max_results,
        size_t max_paths,
        std::vector<BeamCandidate> &out_results,
        BeamStats &stats,
        std::string &error_out) {
        const auto start = Clock::now();
        out_results.clear();
        stats = BeamStats{};

        if (context_utf8 != nullptr) {
            PrefillStats pf{};
            if (!prefill_context_locked(*context_utf8, pf, error_out)) return false;
        } else if (!cached_ctx_valid) {
            PrefillStats pf{};
            if (!prefill_context_locked("", pf, error_out)) return false;
        }

        if (!constrained_vocab_ready || paths.empty()) {
            stats.elapsed_ms = elapsed_ms_since(start);
            return true;
        }

        beam_width = std::clamp(beam_width, 2, 8);
        max_results = std::clamp(max_results, 1, 16);

        llama_memory_t mem = llama_get_memory(ctx);
        const int32_t ctx_len = static_cast<int32_t>(cached_ctx_tokens.size());
        const int32_t max_temp_seqs = std::max<int32_t>(1, n_seq_max - 1);

        std::unordered_map<std::string, BeamCandidate> best_by_text;

        const size_t path_limit = std::min<size_t>(paths.size(), std::max<size_t>(1, max_paths));
        for (size_t path_idx = 0; path_idx < path_limit; ++path_idx) {
            const auto &path = paths[path_idx];
            if (!path.is_complete || path.syllables.empty() || path.syllables.size() > 24) {
                continue;
            }
            ++stats.path_count;
            const size_t n = path.syllables.size();

            std::vector<std::vector<std::string>> span_keys(n);
            for (size_t p = 0; p < n; ++p) {
                const size_t max_span = std::min<size_t>(4, n - p);
                span_keys[p].resize(max_span);
                std::string acc;
                for (size_t s = 0; s < max_span; ++s) {
                    if (s > 0) acc.push_back('\'');
                    acc += path.syllables[p + s];
                    span_keys[p][s] = acc;
                }
            }

            std::vector<llama_seq_id> free_seqs;
            free_seqs.reserve(static_cast<size_t>(max_temp_seqs));
            for (int32_t s = max_temp_seqs; s >= 1; --s) {
                free_seqs.push_back(static_cast<llama_seq_id>(s));
            }
            std::vector<int32_t> seq_refs(static_cast<size_t>(n_seq_max), 0);

            auto release_seq = [&](llama_seq_id sid) {
                if (sid <= 0 || sid >= n_seq_max) return;
                if (seq_refs[static_cast<size_t>(sid)] > 0) {
                    --seq_refs[static_cast<size_t>(sid)];
                    if (seq_refs[static_cast<size_t>(sid)] == 0) {
                        llama_memory_seq_rm(mem, sid, -1, -1);
                        free_seqs.push_back(sid);
                    }
                }
            };

            auto cleanup_all_temp_seqs = [&]() {
                for (int32_t s = 1; s < n_seq_max; ++s) {
                    llama_memory_seq_rm(mem, static_cast<llama_seq_id>(s), -1, -1);
                    seq_refs[static_cast<size_t>(s)] = 0;
                }
            };

            std::vector<std::vector<PendingBeamHyp>> buckets(n + 1);

            auto expand_from_log_probs = [&](
                size_t p,
                llama_seq_id parent_seq,
                int32_t parent_toks,
                double parent_score,
                double parent_lm,
                const std::string &parent_text,
                const float *logits_or_log_probs,
                bool already_log_probs,
                double log_z) {
                const size_t max_span = std::min<size_t>(4, n - p);
                // 每个跨度保留更宽的局部单步候选（纯 CPU 筛选、零额外 GPU 解码开销），
                // 避免高频同音单字在看到下一音节前过早挤掉合理搭配的首字（如 `把碗洗了`、`过去蛮远`、`首字耗时`）。
                const size_t per_span_keep = std::max<size_t>(static_cast<size_t>(beam_width) * 3, 10);
                std::vector<std::pair<double, llama_token>> top_toks;
                for (size_t s = 0; s < max_span; ++s) {
                    auto it = syllable_to_tokens.find(span_keys[p][s]);
                    if (it == syllable_to_tokens.end() || it->second.empty()) continue;
                    const auto &cands = it->second;
                    top_toks.clear();
                    top_toks.reserve(cands.size());
                    for (llama_token tid : cands) {
                        const double lp = already_log_probs
                                              ? static_cast<double>(logits_or_log_probs[static_cast<size_t>(tid)])
                                              : (static_cast<double>(logits_or_log_probs[static_cast<size_t>(tid)]) - log_z);
                        top_toks.emplace_back(lp, tid);
                    }
                    if (top_toks.size() > per_span_keep) {
                        std::partial_sort(
                            top_toks.begin(),
                            top_toks.begin() + static_cast<std::ptrdiff_t>(per_span_keep),
                            top_toks.end(),
                            [](const auto &a, const auto &b) { return a.first > b.first; });
                        top_toks.resize(per_span_keep);
                    }
                    const size_t next_p = p + s + 1;
                    for (const auto &[lp, tid] : top_toks) {
                        PendingBeamHyp child{};
                        child.parent_seq = parent_seq;
                        child.last_token = tid;
                        child.token_count = parent_toks + 1;
                        child.score = parent_score + lp;
                        child.lm_log_prob = parent_lm + lp;
                        child.text = parent_text + token_hanzi_text[static_cast<size_t>(tid)];
                        if (parent_seq > 0 && next_p < n) {
                            ++seq_refs[static_cast<size_t>(parent_seq)];
                        }
                        buckets[next_p].push_back(std::move(child));
                    }
                }
            };

            expand_from_log_probs(
                0,
                0,
                0,
                path.channel_log_prob,
                0.0,
                "",
                cached_ctx_log_probs.data(),
                true,
                0.0);

            bool ok = true;
            for (size_t p = 1; p < n; ++p) {
                auto &bucket = buckets[p];
                if (bucket.empty()) continue;

                std::sort(bucket.begin(), bucket.end(), [](const PendingBeamHyp &a, const PendingBeamHyp &b) {
                    return a.score > b.score;
                });

                std::vector<PendingBeamHyp> survivors;
                survivors.reserve(static_cast<size_t>(beam_width));
                std::unordered_set<std::string> seen_text;
                for (auto &hyp : bucket) {
                    if (static_cast<int32_t>(survivors.size()) < beam_width &&
                        !free_seqs.empty() &&
                        static_cast<int32_t>(survivors.size()) < static_cast<int32_t>(free_seqs.size()) &&
                        seen_text.insert(hyp.text).second) {
                        survivors.push_back(std::move(hyp));
                    } else {
                        release_seq(hyp.parent_seq);
                    }
                }
                bucket.clear();
                if (survivors.empty()) continue;

                std::vector<llama_seq_id> assigned_seqs(survivors.size(), 0);
                batch_clear(batch);
                for (size_t i = 0; i < survivors.size(); ++i) {
                    const llama_seq_id sid = free_seqs.back();
                    free_seqs.pop_back();
                    assigned_seqs[i] = sid;
                    seq_refs[static_cast<size_t>(sid)] = 1;

                    const PendingBeamHyp &hyp = survivors[i];
                    const llama_pos parent_end = static_cast<llama_pos>(ctx_len + hyp.token_count - 1);
                    llama_memory_seq_cp(mem, hyp.parent_seq, sid, 0, parent_end);
                    batch_add_single_seq(batch, hyp.last_token, parent_end, sid, true);
                }

                for (const auto &hyp : survivors) {
                    release_seq(hyp.parent_seq);
                }

                const int32_t rc = llama_decode(ctx, batch);
                ++stats.decode_calls;
                stats.decoded_tokens += batch.n_tokens;
                if (rc != 0) {
                    cleanup_all_temp_seqs();
                    error_out = "受限束搜索 llama_decode 失败，返回码=" + std::to_string(rc);
                    ok = false;
                    break;
                }

                std::vector<const float *> surv_logits(survivors.size(), nullptr);
                for (size_t i = 0; i < survivors.size(); ++i) {
                    surv_logits[i] = llama_get_logits_ith(ctx, static_cast<int32_t>(i));
                }
                std::vector<double> surv_log_zs;
                compute_log_zs_batch(surv_logits, n_vocab, surv_log_zs);

                for (size_t i = 0; i < survivors.size(); ++i) {
                    if (!surv_logits[i]) continue;
                    const PendingBeamHyp &hyp = survivors[i];
                    expand_from_log_probs(
                        p,
                        assigned_seqs[i],
                        hyp.token_count,
                        hyp.score,
                        hyp.lm_log_prob,
                        hyp.text,
                        surv_logits[i],
                        false,
                        surv_log_zs[i]);
                }

                for (llama_seq_id sid : assigned_seqs) {
                    release_seq(sid);
                }
            }

            for (const auto &done : buckets[n]) {
                auto it = best_by_text.find(done.text);
                if (it == best_by_text.end() || done.lm_log_prob > it->second.log_prob) {
                    BeamCandidate bc{};
                    bc.text = done.text;
                    bc.log_prob = done.lm_log_prob;
                    bc.path_index = static_cast<int32_t>(path_idx);
                    bc.token_count = done.token_count;
                    best_by_text[done.text] = std::move(bc);
                }
            }

            cleanup_all_temp_seqs();
            if (!ok) return false;
        }

        out_results.reserve(best_by_text.size());
        for (auto &[txt, cand] : best_by_text) {
            (void)txt;
            out_results.push_back(std::move(cand));
        }
        std::sort(out_results.begin(), out_results.end(), [](const BeamCandidate &a, const BeamCandidate &b) {
            return a.log_prob > b.log_prob;
        });
        if (static_cast<int32_t>(out_results.size()) > max_results) {
            out_results.resize(static_cast<size_t>(max_results));
        }

        stats.result_count = static_cast<int32_t>(out_results.size());
        stats.elapsed_ms = elapsed_ms_since(start);
        return true;
    }

    /// 长句（`n > 8`）局部异常窗口受限束搜索：
    /// 仅针对最差惊奇度 token 附近至多 8 音节子串运行受限束搜索，临时将正常前段挂入 `seq_0` 后立刻无痕恢复，
    /// 并将重拼后的完整长句送入前缀树精准重算整句联合对数概率，使长句救回耗时稳定控制在 20 ms 左右。
    bool decode_anomaly_window_locked(
        const SyllablePathInput &primary_path,
        const std::string &anchor_text,
        const CandidateScoreDiag &anchor_diag,
        int32_t beam_width,
        int32_t max_results,
        std::vector<BeamCandidate> &out_results,
        BeamStats &beam_stats,
        std::string &error_out) {
        const auto start = Clock::now();
        out_results.clear();
        beam_stats = BeamStats{};

        const int32_t n = static_cast<int32_t>(primary_path.syllables.size());
        if (n <= 5 || anchor_diag.char_count != n || !primary_path.is_complete) {
            return true;
        }

        // 围绕最差惊奇度字符定位宽度 <= 5 的紧凑局部窗口 `[win_start, win_end)`（3–4 步完成，~18–25 ms）。
        constexpr int32_t kMaxWindow = 5;
        int32_t win_start = std::max<int32_t>(0, anchor_diag.worst_char_start - 1);
        int32_t win_end = std::min<int32_t>(n, std::max(anchor_diag.worst_char_end + 2, win_start + 4));
        if (win_end - win_start > kMaxWindow) {
            const int32_t center = (anchor_diag.worst_char_start + anchor_diag.worst_char_end) / 2;
            win_start = std::clamp(center - kMaxWindow / 2, 0, n - kMaxWindow);
            win_end = win_start + kMaxWindow;
        }
        if (win_end <= win_start) return true;

        const std::string prefix_han = utf8_char_slice(anchor_text, 0, win_start);
        const std::string suffix_han = utf8_char_slice(anchor_text, win_end, n);

        SyllablePathInput win_path{};
        win_path.syllables.assign(
            primary_path.syllables.begin() + win_start,
            primary_path.syllables.begin() + win_end);
        win_path.channel_log_prob = 0.0;
        win_path.is_complete = true;
        std::vector<SyllablePathInput> win_paths;
        win_paths.push_back(std::move(win_path));

        // 备份当前 `seq_0` 状态；若前段非空则临时增量追加到 `seq_0` 末尾，束搜索完立即裁剪恢复。
        const size_t orig_ctx_len = cached_ctx_tokens.size();
        auto orig_tokens = cached_ctx_tokens;
        auto orig_log_probs = cached_ctx_log_probs;
        const bool orig_was_empty = cached_ctx_was_empty;
        llama_memory_t mem = llama_get_memory(ctx);
        bool appended_prefix = false;

        auto restore_seq0 = [&]() {
            if (appended_prefix) {
                llama_memory_seq_rm(mem, 0, static_cast<llama_pos>(orig_ctx_len), -1);
                cached_ctx_tokens = std::move(orig_tokens);
                cached_ctx_log_probs = std::move(orig_log_probs);
                cached_ctx_was_empty = orig_was_empty;
                appended_prefix = false;
            }
        };

        if (!prefix_han.empty()) {
            std::vector<llama_token> ptoks;
            if (!tokenize_raw(prefix_han, ptoks)) {
                return true;
            }
            if (!ptoks.empty()) {
                if (static_cast<int32_t>(orig_ctx_len + ptoks.size()) >= n_ctx - 16) {
                    return true;
                }
                size_t cursor = 0;
                while (cursor < ptoks.size()) {
                    const size_t chunk = std::min<size_t>(static_cast<size_t>(n_batch), ptoks.size() - cursor);
                    batch_clear(batch);
                    for (size_t i = 0; i < chunk; ++i) {
                        const size_t pos = orig_ctx_len + cursor + i;
                        const bool is_last = (cursor + i + 1 == ptoks.size());
                        batch_add_single_seq(batch, ptoks[cursor + i], static_cast<llama_pos>(pos), 0, is_last);
                    }
                    const int32_t rc = llama_decode(ctx, batch);
                    ++beam_stats.decode_calls;
                    beam_stats.decoded_tokens += batch.n_tokens;
                    if (rc != 0) {
                        llama_memory_seq_rm(mem, 0, static_cast<llama_pos>(orig_ctx_len), -1);
                        return true;
                    }
                    cursor += chunk;
                }
                const float *last_logits = llama_get_logits_ith(ctx, batch.n_tokens - 1);
                if (!last_logits) {
                    llama_memory_seq_rm(mem, 0, static_cast<llama_pos>(orig_ctx_len), -1);
                    return true;
                }
                compute_full_log_softmax(last_logits, n_vocab, cached_ctx_log_probs);
                cached_ctx_tokens.insert(cached_ctx_tokens.end(), ptoks.begin(), ptoks.end());
                cached_ctx_was_empty = false;
                appended_prefix = true;
            }
        }

        std::vector<BeamCandidate> win_results;
        BeamStats sub_stats{};
        const bool ok = decode_constrained_locked(
            nullptr, win_paths, beam_width, max_results, 1, win_results, sub_stats, error_out);
        restore_seq0();
        beam_stats.path_count = sub_stats.path_count;
        beam_stats.decode_calls += sub_stats.decode_calls;
        beam_stats.decoded_tokens += sub_stats.decoded_tokens;
        if (!ok || win_results.empty()) {
            beam_stats.elapsed_ms = elapsed_ms_since(start);
            return ok;
        }

        // 重拼为完整长句并在恢复后的原始 `seq_0` 上用前缀树精准重算整句联合对数概率。
        std::vector<std::string> full_texts;
        std::unordered_set<std::string> seen;
        seen.insert(anchor_text);
        for (const auto &wr : win_results) {
            std::string full = prefix_han + wr.text + suffix_han;
            if (count_utf8_codepoints(full) == n && seen.insert(full).second) {
                full_texts.push_back(std::move(full));
            }
        }
        if (full_texts.empty()) {
            beam_stats.elapsed_ms = elapsed_ms_since(start);
            return true;
        }

        std::vector<double> full_scores;
        ScoreStats rescore_stats{};
        if (!score_candidates_with_diag_locked(nullptr, full_texts, full_scores, nullptr, rescore_stats, error_out, true)) {
            beam_stats.elapsed_ms = elapsed_ms_since(start);
            return false;
        }
        beam_stats.decode_calls += rescore_stats.decode_batches;

        out_results.reserve(full_texts.size());
        for (size_t i = 0; i < full_texts.size(); ++i) {
            BeamCandidate bc{};
            bc.text = std::move(full_texts[i]);
            bc.log_prob = full_scores[i];
            bc.path_index = 0;
            bc.token_count = static_cast<int32_t>(n);
            out_results.push_back(std::move(bc));
        }
        std::sort(out_results.begin(), out_results.end(), [](const BeamCandidate &a, const BeamCandidate &b) {
            return a.log_prob > b.log_prob;
        });
        beam_stats.result_count = static_cast<int32_t>(out_results.size());
        beam_stats.elapsed_ms = elapsed_ms_since(start);
        return true;
    }

    /// 一步合一请求处理（持有 `mutex` 全程串行）：
    /// 1. 同步含固定 `<|im_start|>user\n` 前缀的上文 KV-Cache；
    /// 2. 仅对去重等跨度候选（短中句上限 16 条，超长句上限 14 条）执行前缀树共享 KV 批量精准打分并提取惊奇度诊断；
    /// 3. 信息论自信度门控：仅当 Tier 0 最佳候选平均每字对数概率或最差单步惊奇度异常时触发受限束搜索（长句自动切局部异常窗口）。
    JsonValue process_request_locked(const JsonValue &req) {
        const auto total_start = Clock::now();
        const int64_t session = req.get_i64("session", 0);
        const int64_t generation = req.get_i64("generation", 0);

        auto make_err = [](const std::string &msg) {
            JsonValue obj = JsonValue::make_object();
            obj.set("ok", JsonValue::make_bool(false));
            obj.set("error", JsonValue::make_string(msg));
            return obj;
        };

        PrefillStats pf_stats{};
        std::string err;
        if (const JsonValue *cv = req.find("context"); cv && cv->is_string()) {
            if (!prefill_context_locked(cv->str_val, pf_stats, err)) {
                return make_err(err);
            }
        } else if (!cached_ctx_valid) {
            if (!prefill_context_locked("", pf_stats, err)) {
                return make_err(err);
            }
        }

        // 仅对去重且等跨度候选执行批量打分，完整保留混淆网络 1-Span 扰动槽位。
        std::vector<SyllablePathInput> parsed_paths;
        {
            const JsonValue *parr = req.find("syllable_paths");
            if (parr && parr->is_array()) {
                for (const auto &item : parr->arr_val) {
                    SyllablePathInput p{};
                    if (item.is_string()) {
                        p.syllables = split_syllables(item.str_val);
                        p.is_complete = !p.syllables.empty();
                    } else if (item.is_object()) {
                        const JsonValue *s_val = item.find("syllables");
                        if (s_val && s_val->is_array()) {
                            for (const auto &el : s_val->arr_val) {
                                if (el.is_string() && !el.str_val.empty()) {
                                    p.syllables.push_back(el.str_val);
                                }
                            }
                        } else if (s_val && s_val->is_string()) {
                            p.syllables = split_syllables(s_val->str_val);
                        } else {
                            const std::string joined = item.get_string("segmentation");
                            if (!joined.empty()) p.syllables = split_syllables(joined);
                        }
                        p.channel_log_prob = item.get_double("channel_log_prob", 0.0);
                        p.is_complete = item.get_bool("is_complete", !p.syllables.empty());
                    }
                    if (!p.syllables.empty()) parsed_paths.push_back(std::move(p));
                }
            } else {
                const std::string seg = req.get_string("segmentation");
                if (!seg.empty()) {
                    SyllablePathInput p{};
                    p.syllables = split_syllables(seg);
                    p.is_complete = !p.syllables.empty();
                    if (!p.syllables.empty()) parsed_paths.push_back(std::move(p));
                }
            }
        }
        const size_t primary_n = parsed_paths.empty() ? 0 : parsed_paths[0].syllables.size();
        // 按句长自适应控制前缀树上限：短中句满额收容 16 条（承接全部 1-Span 混淆网络扰动），
        // 14+ 音节超长句保留 14 条以兼顾长句混淆网络召回与低延迟窗口。
        const size_t cand_cap = (primary_n >= 14) ? 14 : 16;

        std::vector<std::string> unique_cands;
        std::unordered_set<std::string> seen_cands;
        if (const JsonValue *arr = req.find("candidates"); arr && arr->is_array()) {
            unique_cands.reserve(std::min<size_t>(arr->arr_val.size(), cand_cap));
            for (const auto &item : arr->arr_val) {
                if (unique_cands.size() >= cand_cap) break;
                std::string txt;
                if (item.is_string()) {
                    txt = item.str_val;
                } else if (item.is_object()) {
                    if (const JsonValue *ex = item.find("exact_span"); ex && ex->is_bool() && !ex->bool_val) {
                        continue;
                    }
                    txt = item.get_string("text");
                }
                if (!txt.empty() && seen_cands.insert(txt).second) {
                    unique_cands.push_back(std::move(txt));
                }
            }
        }

        std::vector<double> scores;
        std::vector<CandidateScoreDiag> diags;
        ScoreStats score_stats{};
        if (!unique_cands.empty()) {
            if (!score_candidates_with_diag_locked(nullptr, unique_cands, scores, &diags, score_stats, err, true)) {
                return make_err(err);
            }
        }

        // 评估 Tier 0 候选池最高自然度（按每字平均对数概率选最佳锚点，同时检查最差单步惊奇度）。
        int32_t best_idx = -1;
        double best_avg_char_lp = -1e9;
        double best_min_edge_lp = -1e9;
        for (size_t i = 0; i < diags.size(); ++i) {
            const auto &d = diags[i];
            if (best_idx < 0 || d.avg_char_lp > best_avg_char_lp ||
                (d.avg_char_lp == best_avg_char_lp && d.min_edge_lp > best_min_edge_lp)) {
                best_idx = static_cast<int32_t>(i);
                best_avg_char_lp = d.avg_char_lp;
                best_min_edge_lp = d.min_edge_lp;
            }
        }

        // 合取 + 极值惊奇度信息论门控（配合 1-Span 混淆网络扰动将束搜索触发率压至低位）：
        // 仅当无候选、或 `(best_avg < -4.75 && best_min_edge < -9.8)`、或 `best_min_edge < -12.5`、
        // 或 4 字以上整句平均极低 `best_avg < -5.5` 时触发束救回。
        const double avg_thresh = env_double_or("SHURUFA_BEAM_AVG_LP_THRESH", -4.75);
        const double min_edge_thresh = env_double_or("SHURUFA_BEAM_MIN_EDGE_LP_THRESH", -9.8);
        const double extreme_edge_thresh = env_double_or("SHURUFA_BEAM_EXTREME_EDGE_LP_THRESH", -12.5);
        const double extreme_avg_thresh = env_double_or("SHURUFA_BEAM_EXTREME_AVG_LP_THRESH", -5.5);
        const bool force_beam = env_flag_enabled("SHURUFA_BEAM_ALWAYS");
        const bool enable_beam = req.get_bool("enable_constrained_decode", true);

        bool beam_ran = false;
        bool beam_windowed = false;
        std::vector<BeamCandidate> beam_results;
        BeamStats beam_stats{};

        const bool trigger_beam = force_beam ||
            (best_idx < 0) ||
            (best_avg_char_lp < avg_thresh && best_min_edge_lp < min_edge_thresh) ||
            (best_min_edge_lp < extreme_edge_thresh) ||
            (primary_n >= 4 && best_avg_char_lp < extreme_avg_thresh);

        if (enable_beam && constrained_vocab_ready && trigger_beam &&
            !parsed_paths.empty() && parsed_paths[0].is_complete && primary_n >= 2) {
            const int32_t beam_width = std::clamp(req.get_i32("beam_width", 4), 2, 4);
            const int32_t max_results = std::clamp(
                req.get_i32("max_beam_results", req.get_i32("max_results", 3)), 1, 4);

            if (primary_n <= 5) {
                beam_ran = true;
                if (!decode_constrained_locked(
                        nullptr, parsed_paths, beam_width, max_results, 1, beam_results, beam_stats, err)) {
                    return make_err(err);
                }
            } else if (best_idx >= 0 &&
                       diags[static_cast<size_t>(best_idx)].char_count == static_cast<int32_t>(primary_n)) {
                beam_ran = true;
                beam_windowed = true;
                if (!decode_anomaly_window_locked(
                        parsed_paths[0],
                        unique_cands[static_cast<size_t>(best_idx)],
                        diags[static_cast<size_t>(best_idx)],
                        beam_width,
                        max_results,
                        beam_results,
                        beam_stats,
                        err)) {
                    return make_err(err);
                }
            } else if (primary_n <= 8) {
                beam_ran = true;
                if (!decode_constrained_locked(
                        nullptr, parsed_paths, beam_width, max_results, 1, beam_results, beam_stats, err)) {
                    return make_err(err);
                }
            }
        }

        JsonValue out = JsonValue::make_object();
        out.set("ok", JsonValue::make_bool(true));
        out.set("session", JsonValue::make_int(session));
        out.set("generation", JsonValue::make_int(generation));

        JsonValue cand_scores = JsonValue::make_array();
        for (size_t i = 0; i < unique_cands.size(); ++i) {
            JsonValue item = JsonValue::make_object();
            item.set("text", JsonValue::make_string(unique_cands[i]));
            item.set("neural_log_prob", JsonValue::make_number(scores[i]));
            cand_scores.push(std::move(item));
        }
        out.set("candidate_scores", std::move(cand_scores));

        JsonValue beam_arr = JsonValue::make_array();
        for (const auto &bc : beam_results) {
            JsonValue item = JsonValue::make_object();
            item.set("text", JsonValue::make_string(bc.text));
            item.set("neural_log_prob", JsonValue::make_number(bc.log_prob));
            item.set("path_index", JsonValue::make_int(bc.path_index));
            item.set("token_count", JsonValue::make_int(bc.token_count));
            beam_arr.push(std::move(item));
        }
        out.set("beam_candidates", std::move(beam_arr));

        JsonValue st = JsonValue::make_object();
        st.set("prefill_ms", JsonValue::make_number(pf_stats.elapsed_ms));
        st.set("reused_ctx_tokens", JsonValue::make_int(pf_stats.reused_tokens));
        st.set("decoded_ctx_tokens", JsonValue::make_int(pf_stats.decoded_tokens));
        st.set("prompt_prefix_tokens", JsonValue::make_int(static_cast<int32_t>(prompt_prefix_tokens.size())));
        st.set("score_ms", JsonValue::make_number(score_stats.elapsed_ms));
        st.set("unique_trie_nodes", JsonValue::make_int(score_stats.unique_trie_nodes));
        st.set("internal_decoded_nodes", JsonValue::make_int(score_stats.internal_decoded_nodes));
        st.set("beam_ran", JsonValue::make_bool(beam_ran));
        st.set("beam_windowed", JsonValue::make_bool(beam_windowed));
        st.set("best_avg_char_lp", JsonValue::make_number(best_idx >= 0 ? best_avg_char_lp : -99.0));
        st.set("best_min_edge_lp", JsonValue::make_number(best_idx >= 0 ? best_min_edge_lp : -99.0));
        st.set("beam_ms", JsonValue::make_number(beam_stats.elapsed_ms));
        st.set("beam_decode_calls", JsonValue::make_int(beam_stats.decode_calls));
        st.set("total_ms", JsonValue::make_number(elapsed_ms_since(total_start)));
        out.set("stats", std::move(st));
        return out;
    }
};

std::string NeuralEngine::default_model_dir() {
    const char *env_dir = std::getenv("SHURUFA_NEURAL_MODEL_DIR");
    if (env_dir && env_dir[0] != '\0') return std::string(env_dir);
    const char *home = std::getenv("HOME");
    if (!home || home[0] == '\0') return "";
    return std::string(home) + "/Library/Application Support/Shurufa/resource-packs/neural-lm";
}

std::string NeuralEngine::discover_model_in_dir(const std::string &dir) {
    if (dir.empty()) return "";
    std::error_code ec;
    if (!std::filesystem::exists(dir, ec) || !std::filesystem::is_directory(dir, ec)) return "";
    std::vector<std::string> found;
    for (const auto &entry : std::filesystem::directory_iterator(dir, ec)) {
        if (ec) break;
        if (!entry.is_regular_file(ec)) continue;
        const auto ext = entry.path().extension().string();
        if (ext == ".gguf" || ext == ".GGUF") {
            std::string lower_name = entry.path().filename().string();
            for (char &ch : lower_name) {
                if (ch >= 'A' && ch <= 'Z') ch = static_cast<char>(ch - 'A' + 'a');
            }
            if (lower_name.find("mmproj") != std::string::npos) continue;
            found.push_back(entry.path().string());
        }
    }
    if (found.empty()) return "";
    std::sort(found.begin(), found.end());
    return found.front();
}

NeuralEngine::NeuralEngine(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}
NeuralEngine::~NeuralEngine() = default;

std::unique_ptr<NeuralEngine> NeuralEngine::create(const EngineConfig &config, std::string &error_out) {
    ensure_llama_backend_initialized();

    std::string resolved_path = config.model_path;
    if (resolved_path.empty()) {
        const char *env_model = std::getenv("SHURUFA_NEURAL_MODEL_PATH");
        if (env_model && env_model[0] != '\0') {
            resolved_path = env_model;
        }
    }
    if (resolved_path.empty()) {
        const std::string dir = config.model_dir.empty() ? default_model_dir() : config.model_dir;
        resolved_path = discover_model_in_dir(dir);
    }
    if (resolved_path.empty() || !std::filesystem::exists(resolved_path)) {
        error_out = "未找到可用的 GGUF 神经语言模型文件";
        return nullptr;
    }

    auto impl = std::make_unique<Impl>();
    impl->model_path = resolved_path;
    impl->n_ctx = config.n_ctx > 0 ? std::max<int32_t>(512, config.n_ctx) : 1024;
    impl->n_batch = config.n_batch > 0 ? std::clamp<int32_t>(config.n_batch, 64, impl->n_ctx) : 512;
    impl->n_seq_max = config.n_seq_max > 0 ? std::clamp<int32_t>(config.n_seq_max, 4, 64) : 20;
    impl->max_ctx_tokens = std::min<int32_t>(512, impl->n_ctx / 2);

    llama_model_params mparams = llama_model_default_params();
    mparams.n_gpu_layers = config.n_gpu_layers >= 0 ? config.n_gpu_layers : 99;

    impl->model = llama_model_load_from_file(resolved_path.c_str(), mparams);
    if (!impl->model) {
        error_out = "加载 GGUF 模型失败: " + resolved_path;
        return nullptr;
    }
    impl->vocab = llama_model_get_vocab(impl->model);
    if (!impl->vocab) {
        error_out = "读取模型词表失败";
        return nullptr;
    }
    impl->n_vocab = llama_vocab_n_tokens(impl->vocab);
    impl->model_adds_bos = llama_vocab_get_add_bos(impl->vocab);
    impl->seed_token = impl->resolve_seed_token();
    impl->prompt_prefix_tokens = impl->resolve_prompt_prefix();
    impl->empty_prompt_tokens = impl->resolve_empty_prompt_prefix(impl->prompt_prefix_tokens);
    {
        std::vector<llama_token> comma_toks;
        if (impl->tokenize_raw("，", comma_toks) && comma_toks.size() == 1) {
            impl->comma_token = comma_toks[0];
        }
    }

    llama_context_params cparams = llama_context_default_params();
    cparams.n_ctx = static_cast<uint32_t>(impl->n_ctx);
    cparams.n_batch = static_cast<uint32_t>(impl->n_batch);
    cparams.n_ubatch = static_cast<uint32_t>(impl->n_batch);
    cparams.n_seq_max = static_cast<uint32_t>(impl->n_seq_max);
    const int32_t threads = config.n_threads > 0 ? config.n_threads : 4;
    cparams.n_threads = threads;
    cparams.n_threads_batch = threads;
    configure_context_extensions(cparams, config.flash_attn);

    impl->ctx = llama_init_from_model(impl->model, cparams);
    if (!impl->ctx) {
        error_out = "初始化 llama_context 失败";
        return nullptr;
    }

    impl->batch = llama_batch_init(impl->n_batch, 0, impl->n_seq_max);
    impl->batch_initialized = true;

    if (!config.pinyin_db_path.empty() || !config.pinyin_tsv_path.empty()) {
        impl->build_constrained_vocab_index(config.pinyin_db_path, config.pinyin_tsv_path);
    }

    // 预热固定起始前缀（并缓存空上文精确 log_probs），确保 Metal 着色器首次编译完成且杜绝 pos=0 退化。
    PrefillStats warmup_stats{};
    std::string warmup_err;
    if (!impl->prefill_context_locked("", warmup_stats, warmup_err)) {
        error_out = "初始化空上文预热失败: " + warmup_err;
        return nullptr;
    }

    return std::unique_ptr<NeuralEngine>(new NeuralEngine(std::move(impl)));
}

const std::string &NeuralEngine::model_path() const { return impl_->model_path; }
bool NeuralEngine::has_constrained_vocab() const { return impl_->constrained_vocab_ready; }
VocabIndexStats NeuralEngine::vocab_index_stats() const { return impl_->vocab_stats; }

bool NeuralEngine::prefill_context(const std::string &context_utf8, PrefillStats &stats_out, std::string &error_out) {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return impl_->prefill_context_locked(context_utf8, stats_out, error_out);
}

bool NeuralEngine::score_candidates(
    const std::string *context_utf8,
    const std::vector<std::string> &candidates_utf8,
    std::vector<double> &out_log_probs,
    ScoreStats &stats_out,
    std::string &error_out) {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return impl_->score_candidates_locked(context_utf8, candidates_utf8, out_log_probs, stats_out, error_out);
}

bool NeuralEngine::score_candidates_sequential_reference(
    const std::string *context_utf8,
    const std::vector<std::string> &candidates_utf8,
    std::vector<double> &out_log_probs,
    double &elapsed_ms_out,
    std::string &error_out) {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return impl_->score_candidates_sequential_locked(context_utf8, candidates_utf8, out_log_probs, elapsed_ms_out, error_out);
}

bool NeuralEngine::decode_constrained(
    const std::string *context_utf8,
    const std::vector<SyllablePathInput> &paths,
    int32_t beam_width,
    int32_t max_results,
    std::vector<BeamCandidate> &out_results,
    BeamStats &stats_out,
    std::string &error_out) {
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return impl_->decode_constrained_locked(context_utf8, paths, beam_width, max_results, 4, out_results, stats_out, error_out);
}

std::string NeuralEngine::process_request_raw_json(const char *req_json, size_t len) {
    JsonValue req;
    if (!parse_json(req_json, len, req) || !req.is_object()) {
        JsonValue err = JsonValue::make_object();
        err.set("ok", JsonValue::make_bool(false));
        err.set("error", JsonValue::make_string("process_request 请求 JSON 非法"));
        return dump_json(err);
    }
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return dump_json(impl_->process_request_locked(req));
}

namespace {

JsonValue make_error_json(const std::string &msg) {
    JsonValue obj = JsonValue::make_object();
    obj.set("ok", JsonValue::make_bool(false));
    obj.set("error", JsonValue::make_string(msg));
    return obj;
}

void parse_candidates_array(const JsonValue *arr, std::vector<std::string> &out) {
    out.clear();
    if (!arr || !arr->is_array()) return;
    out.reserve(arr->arr_val.size());
    for (const auto &item : arr->arr_val) {
        if (item.is_string()) {
            out.push_back(item.str_val);
        } else if (item.is_object()) {
            out.push_back(item.get_string("text"));
        } else {
            out.emplace_back();
        }
    }
}

void parse_syllable_paths(const JsonValue &root, std::vector<SyllablePathInput> &out) {
    out.clear();
    const JsonValue *arr = root.find("syllable_paths");
    if (arr && arr->is_array()) {
        for (const auto &item : arr->arr_val) {
            SyllablePathInput p{};
            if (item.is_string()) {
                p.syllables = split_syllables(item.str_val);
                p.is_complete = !p.syllables.empty();
            } else if (item.is_object()) {
                const JsonValue *s_val = item.find("syllables");
                if (s_val && s_val->is_array()) {
                    for (const auto &el : s_val->arr_val) {
                        if (el.is_string() && !el.str_val.empty()) {
                            p.syllables.push_back(el.str_val);
                        }
                    }
                } else if (s_val && s_val->is_string()) {
                    p.syllables = split_syllables(s_val->str_val);
                } else {
                    const std::string joined = item.get_string("segmentation");
                    if (!joined.empty()) p.syllables = split_syllables(joined);
                }
                p.channel_log_prob = item.get_double("channel_log_prob", 0.0);
                p.is_complete = item.get_bool("is_complete", !p.syllables.empty());
            }
            if (!p.syllables.empty()) out.push_back(std::move(p));
        }
        return;
    }
    const std::string seg = root.get_string("segmentation");
    if (!seg.empty()) {
        SyllablePathInput p{};
        p.syllables = split_syllables(seg);
        p.is_complete = !p.syllables.empty();
        if (!p.syllables.empty()) out.push_back(std::move(p));
    }
}

}  // namespace

}  // namespace shurufa::neural

using namespace shurufa::neural;

extern "C" {

SHURUFA_NEURAL_API char *shurufa_neural_discover_model_path(const char *model_dir, size_t len) {
    std::string dir = (model_dir && len > 0) ? std::string(model_dir, len) : NeuralEngine::default_model_dir();
    std::string found = NeuralEngine::discover_model_in_dir(dir);
    if (found.empty()) return nullptr;
    return dup_c_string(found);
}

SHURUFA_NEURAL_API void *shurufa_neural_create(const char *cfg_json, size_t len) {
    EngineConfig cfg{};
    if (cfg_json && len > 0) {
        JsonValue j;
        if (!parse_json(cfg_json, len, j) || !j.is_object()) {
            return nullptr;
        }
        cfg.model_path = j.get_string("model_path");
        cfg.model_dir = j.get_string("model_dir");
        cfg.pinyin_db_path = j.get_string("pinyin_db_path");
        cfg.pinyin_tsv_path = j.get_string("pinyin_tsv_path");
        cfg.n_ctx = j.get_i32("n_ctx", 1024);
        cfg.n_batch = j.get_i32("n_batch", 512);
        cfg.n_seq_max = j.get_i32("n_seq_max", 20);
        cfg.n_gpu_layers = j.get_i32("n_gpu_layers", 99);
        cfg.n_threads = j.get_i32("n_threads", 4);
        cfg.flash_attn = j.get_i32("flash_attn", 1);
    } else {
        cfg.n_ctx = 1024;
    }

    std::string err;
    auto engine = NeuralEngine::create(cfg, err);
    if (!engine) {
        if (!err.empty()) {
            std::fprintf(stderr, "[shurufa_neural] create failed: %s\n", err.c_str());
        }
        return nullptr;
    }
    return engine.release();
}

SHURUFA_NEURAL_API void shurufa_neural_destroy(void *handle) {
    if (!handle) return;
    delete static_cast<NeuralEngine *>(handle);
}

SHURUFA_NEURAL_API void shurufa_neural_string_free(char *ptr) {
    std::free(ptr);
}

SHURUFA_NEURAL_API char *shurufa_neural_info(void *handle) {
    if (!handle) return dup_c_string(dump_json(make_error_json("无效的引擎句柄")));
    auto *engine = static_cast<NeuralEngine *>(handle);
    const auto vs = engine->vocab_index_stats();
    JsonValue out = JsonValue::make_object();
    out.set("ok", JsonValue::make_bool(true));
    out.set("model_path", JsonValue::make_string(engine->model_path()));
    out.set("has_constrained_vocab", JsonValue::make_bool(engine->has_constrained_vocab()));
    out.set("hanzi_entries", JsonValue::make_int(vs.hanzi_entries));
    out.set("indexed_tokens", JsonValue::make_int(vs.indexed_tokens));
    out.set("syllable_keys", JsonValue::make_int(vs.syllable_keys));
    out.set("vocab_build_ms", JsonValue::make_number(vs.build_ms));
    return dup_c_string(dump_json(out));
}

SHURUFA_NEURAL_API char *shurufa_neural_prefill_context(void *handle, const char *ctx_utf8, size_t len) {
    if (!handle) return dup_c_string(dump_json(make_error_json("无效的引擎句柄")));
    auto *engine = static_cast<NeuralEngine *>(handle);
    std::string ctx_str = (ctx_utf8 && len > 0) ? std::string(ctx_utf8, len) : std::string{};
    PrefillStats stats{};
    std::string err;
    if (!engine->prefill_context(ctx_str, stats, err)) {
        return dup_c_string(dump_json(make_error_json(err)));
    }
    JsonValue out = JsonValue::make_object();
    out.set("ok", JsonValue::make_bool(true));
    out.set("total_tokens", JsonValue::make_int(stats.total_tokens));
    out.set("reused_tokens", JsonValue::make_int(stats.reused_tokens));
    out.set("decoded_tokens", JsonValue::make_int(stats.decoded_tokens));
    out.set("elapsed_ms", JsonValue::make_number(stats.elapsed_ms));
    return dup_c_string(dump_json(out));
}

SHURUFA_NEURAL_API char *shurufa_neural_score_candidates(void *handle, const char *req_json, size_t len) {
    if (!handle) return dup_c_string(dump_json(make_error_json("无效的引擎句柄")));
    JsonValue req;
    if (!parse_json(req_json, len, req) || !req.is_object()) {
        return dup_c_string(dump_json(make_error_json("score_candidates 请求 JSON 非法")));
    }

    auto *engine = static_cast<NeuralEngine *>(handle);
    std::string ctx_str;
    const std::string *ctx_ptr = nullptr;
    if (const JsonValue *cv = req.find("context"); cv && cv->is_string()) {
        ctx_str = cv->str_val;
        ctx_ptr = &ctx_str;
    }

    std::vector<std::string> candidates;
    parse_candidates_array(req.find("candidates"), candidates);

    std::vector<double> scores;
    ScoreStats stats{};
    std::string err;
    if (!engine->score_candidates(ctx_ptr, candidates, scores, stats, err)) {
        return dup_c_string(dump_json(make_error_json(err)));
    }

    JsonValue out = JsonValue::make_object();
    out.set("ok", JsonValue::make_bool(true));
    JsonValue score_arr = JsonValue::make_array();
    JsonValue cand_scores = JsonValue::make_array();
    for (size_t i = 0; i < candidates.size(); ++i) {
        score_arr.push(JsonValue::make_number(scores[i]));
        JsonValue item = JsonValue::make_object();
        item.set("text", JsonValue::make_string(candidates[i]));
        item.set("neural_log_prob", JsonValue::make_number(scores[i]));
        cand_scores.push(std::move(item));
    }
    out.set("scores", std::move(score_arr));
    out.set("candidate_scores", std::move(cand_scores));

    JsonValue st = JsonValue::make_object();
    st.set("candidate_count", JsonValue::make_int(stats.candidate_count));
    st.set("unique_trie_nodes", JsonValue::make_int(stats.unique_trie_nodes));
    st.set("internal_decoded_nodes", JsonValue::make_int(stats.internal_decoded_nodes));
    st.set("total_candidate_tokens", JsonValue::make_int(stats.total_candidate_tokens));
    st.set("decode_batches", JsonValue::make_int(stats.decode_batches));
    st.set("elapsed_ms", JsonValue::make_number(stats.elapsed_ms));
    out.set("stats", std::move(st));
    return dup_c_string(dump_json(out));
}

SHURUFA_NEURAL_API char *shurufa_neural_decode_constrained(void *handle, const char *req_json, size_t len) {
    if (!handle) return dup_c_string(dump_json(make_error_json("无效的引擎句柄")));
    JsonValue req;
    if (!parse_json(req_json, len, req) || !req.is_object()) {
        return dup_c_string(dump_json(make_error_json("decode_constrained 请求 JSON 非法")));
    }

    auto *engine = static_cast<NeuralEngine *>(handle);
    std::string ctx_str;
    const std::string *ctx_ptr = nullptr;
    if (const JsonValue *cv = req.find("context"); cv && cv->is_string()) {
        ctx_str = cv->str_val;
        ctx_ptr = &ctx_str;
    }

    std::vector<SyllablePathInput> paths;
    parse_syllable_paths(req, paths);
    const int32_t beam_width = req.get_i32("beam_width", 4);
    const int32_t max_results = req.get_i32("max_beam_results", req.get_i32("max_results", 4));

    std::vector<BeamCandidate> results;
    BeamStats stats{};
    std::string err;
    if (!engine->decode_constrained(ctx_ptr, paths, beam_width, max_results, results, stats, err)) {
        return dup_c_string(dump_json(make_error_json(err)));
    }

    JsonValue out = JsonValue::make_object();
    out.set("ok", JsonValue::make_bool(true));
    JsonValue arr = JsonValue::make_array();
    for (const auto &bc : results) {
        JsonValue item = JsonValue::make_object();
        item.set("text", JsonValue::make_string(bc.text));
        item.set("neural_log_prob", JsonValue::make_number(bc.log_prob));
        item.set("path_index", JsonValue::make_int(bc.path_index));
        item.set("token_count", JsonValue::make_int(bc.token_count));
        arr.push(std::move(item));
    }
    out.set("beam_candidates", std::move(arr));

    JsonValue st = JsonValue::make_object();
    st.set("path_count", JsonValue::make_int(stats.path_count));
    st.set("result_count", JsonValue::make_int(stats.result_count));
    st.set("decode_calls", JsonValue::make_int(stats.decode_calls));
    st.set("decoded_tokens", JsonValue::make_int(stats.decoded_tokens));
    st.set("elapsed_ms", JsonValue::make_number(stats.elapsed_ms));
    out.set("stats", std::move(st));
    return dup_c_string(dump_json(out));
}

SHURUFA_NEURAL_API char *shurufa_neural_process_request(void *handle, const char *req_json, size_t len) {
    if (!handle) return dup_c_string(dump_json(make_error_json("无效的引擎句柄")));
    auto *engine = static_cast<NeuralEngine *>(handle);
    return dup_c_string(engine->process_request_raw_json(req_json, len));
}

}  // extern "C"
