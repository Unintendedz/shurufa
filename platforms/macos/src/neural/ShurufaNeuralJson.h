#ifndef SHURUFA_NEURAL_JSON_H
#define SHURUFA_NEURAL_JSON_H

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <map>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace shurufa::neural {

/// 轻量自包含 RFC-8259 JSON 值类型（零第三方依赖，完整保留 UTF-8 与代理对解码）。
struct JsonValue {
    enum class Kind { Null, Bool, Number, String, Array, Object };

    Kind kind = Kind::Null;
    bool bool_val = false;
    double num_val = 0.0;
    std::string str_val;
    std::vector<JsonValue> arr_val;
    std::vector<std::pair<std::string, JsonValue>> obj_val;

    static JsonValue make_null() { return JsonValue{}; }
    static JsonValue make_bool(bool v) {
        JsonValue j;
        j.kind = Kind::Bool;
        j.bool_val = v;
        return j;
    }
    static JsonValue make_number(double v) {
        JsonValue j;
        j.kind = Kind::Number;
        j.num_val = v;
        return j;
    }
    static JsonValue make_int(int64_t v) { return make_number(static_cast<double>(v)); }
    static JsonValue make_string(std::string v) {
        JsonValue j;
        j.kind = Kind::String;
        j.str_val = std::move(v);
        return j;
    }
    static JsonValue make_array() {
        JsonValue j;
        j.kind = Kind::Array;
        return j;
    }
    static JsonValue make_object() {
        JsonValue j;
        j.kind = Kind::Object;
        return j;
    }

    bool is_null() const { return kind == Kind::Null; }
    bool is_bool() const { return kind == Kind::Bool; }
    bool is_number() const { return kind == Kind::Number; }
    bool is_string() const { return kind == Kind::String; }
    bool is_array() const { return kind == Kind::Array; }
    bool is_object() const { return kind == Kind::Object; }

    void push(JsonValue v) { arr_val.push_back(std::move(v)); }

    void set(std::string key, JsonValue v) {
        for (auto &kv : obj_val) {
            if (kv.first == key) {
                kv.second = std::move(v);
                return;
            }
        }
        obj_val.emplace_back(std::move(key), std::move(v));
    }

    const JsonValue *find(std::string_view key) const {
        if (kind != Kind::Object) return nullptr;
        for (const auto &kv : obj_val) {
            if (kv.first == key) return &kv.second;
        }
        return nullptr;
    }

    std::string get_string(std::string_view key, const std::string &fallback = "") const {
        const JsonValue *v = find(key);
        return (v && v->is_string()) ? v->str_val : fallback;
    }

    bool get_bool(std::string_view key, bool fallback) const {
        const JsonValue *v = find(key);
        return (v && v->is_bool()) ? v->bool_val : fallback;
    }

    double get_double(std::string_view key, double fallback) const {
        const JsonValue *v = find(key);
        return (v && v->is_number()) ? v->num_val : fallback;
    }

    int64_t get_i64(std::string_view key, int64_t fallback) const {
        const JsonValue *v = find(key);
        return (v && v->is_number() && std::isfinite(v->num_val))
                   ? static_cast<int64_t>(std::llround(v->num_val))
                   : fallback;
    }

    int32_t get_i32(std::string_view key, int32_t fallback) const {
        return static_cast<int32_t>(get_i64(key, fallback));
    }
};

namespace detail {

inline void append_utf8(std::string &out, uint32_t cp) {
    if (cp <= 0x7Fu) {
        out.push_back(static_cast<char>(cp));
    } else if (cp <= 0x7FFu) {
        out.push_back(static_cast<char>(0xC0u | (cp >> 6)));
        out.push_back(static_cast<char>(0x80u | (cp & 0x3Fu)));
    } else if (cp <= 0xFFFFu) {
        out.push_back(static_cast<char>(0xE0u | (cp >> 12)));
        out.push_back(static_cast<char>(0x80u | ((cp >> 6) & 0x3Fu)));
        out.push_back(static_cast<char>(0x80u | (cp & 0x3Fu)));
    } else if (cp <= 0x10FFFFu) {
        out.push_back(static_cast<char>(0xF0u | (cp >> 18)));
        out.push_back(static_cast<char>(0x80u | ((cp >> 12) & 0x3Fu)));
        out.push_back(static_cast<char>(0x80u | ((cp >> 6) & 0x3Fu)));
        out.push_back(static_cast<char>(0x80u | (cp & 0x3Fu)));
    }
}

struct JsonParser {
    const char *data = nullptr;
    size_t len = 0;
    size_t pos = 0;
    int depth = 0;

    void skip_ws() {
        while (pos < len) {
            const char c = data[pos];
            if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
                ++pos;
            } else {
                break;
            }
        }
    }

    bool parse_hex4(uint32_t &out) {
        if (pos + 4 > len) return false;
        uint32_t val = 0;
        for (int i = 0; i < 4; ++i) {
            const char c = data[pos++];
            val <<= 4;
            if (c >= '0' && c <= '9') {
                val |= static_cast<uint32_t>(c - '0');
            } else if (c >= 'a' && c <= 'f') {
                val |= static_cast<uint32_t>(c - 'a' + 10);
            } else if (c >= 'A' && c <= 'F') {
                val |= static_cast<uint32_t>(c - 'A' + 10);
            } else {
                return false;
            }
        }
        out = val;
        return true;
    }

    bool parse_string(std::string &out) {
        if (pos >= len || data[pos] != '"') return false;
        ++pos;
        out.clear();
        while (pos < len) {
            const char c = data[pos++];
            if (c == '"') return true;
            if (c == '\\') {
                if (pos >= len) return false;
                const char esc = data[pos++];
                switch (esc) {
                    case '"': out.push_back('"'); break;
                    case '\\': out.push_back('\\'); break;
                    case '/': out.push_back('/'); break;
                    case 'b': out.push_back('\b'); break;
                    case 'f': out.push_back('\f'); break;
                    case 'n': out.push_back('\n'); break;
                    case 'r': out.push_back('\r'); break;
                    case 't': out.push_back('\t'); break;
                    case 'u': {
                        uint32_t u1 = 0;
                        if (!parse_hex4(u1)) return false;
                        if (u1 >= 0xD800u && u1 <= 0xDBFFu) {
                            if (pos + 2 > len || data[pos] != '\\' || data[pos + 1] != 'u') {
                                return false;
                            }
                            pos += 2;
                            uint32_t u2 = 0;
                            if (!parse_hex4(u2) || u2 < 0xDC00u || u2 > 0xDFFFu) return false;
                            const uint32_t cp = 0x10000u + (((u1 - 0xD800u) << 10) | (u2 - 0xDC00u));
                            append_utf8(out, cp);
                        } else {
                            append_utf8(out, u1);
                        }
                        break;
                    }
                    default:
                        return false;
                }
            } else if (static_cast<unsigned char>(c) < 0x20u) {
                return false;
            } else {
                out.push_back(c);
            }
        }
        return false;
    }

    bool parse_number(double &out) {
        const size_t start = pos;
        if (pos < len && data[pos] == '-') ++pos;
        if (pos >= len) return false;
        if (data[pos] == '0') {
            ++pos;
        } else if (data[pos] >= '1' && data[pos] <= '9') {
            while (pos < len && data[pos] >= '0' && data[pos] <= '9') ++pos;
        } else {
            return false;
        }
        if (pos < len && data[pos] == '.') {
            ++pos;
            if (pos >= len || data[pos] < '0' || data[pos] > '9') return false;
            while (pos < len && data[pos] >= '0' && data[pos] <= '9') ++pos;
        }
        if (pos < len && (data[pos] == 'e' || data[pos] == 'E')) {
            ++pos;
            if (pos < len && (data[pos] == '+' || data[pos] == '-')) ++pos;
            if (pos >= len || data[pos] < '0' || data[pos] > '9') return false;
            while (pos < len && data[pos] >= '0' && data[pos] <= '9') ++pos;
        }
        std::string buf(data + start, pos - start);
        char *endp = nullptr;
        out = std::strtod(buf.c_str(), &endp);
        return endp == buf.c_str() + buf.size() && std::isfinite(out);
    }

    bool parse_value(JsonValue &out) {
        if (++depth > 64) return false;
        skip_ws();
        if (pos >= len) {
            --depth;
            return false;
        }
        const char c = data[pos];
        bool ok = false;
        if (c == 'n') {
            if (pos + 4 <= len && std::string_view(data + pos, 4) == "null") {
                pos += 4;
                out = JsonValue::make_null();
                ok = true;
            }
        } else if (c == 't') {
            if (pos + 4 <= len && std::string_view(data + pos, 4) == "true") {
                pos += 4;
                out = JsonValue::make_bool(true);
                ok = true;
            }
        } else if (c == 'f') {
            if (pos + 5 <= len && std::string_view(data + pos, 5) == "false") {
                pos += 5;
                out = JsonValue::make_bool(false);
                ok = true;
            }
        } else if (c == '"') {
            std::string s;
            ok = parse_string(s);
            if (ok) out = JsonValue::make_string(std::move(s));
        } else if (c == '[') {
            ++pos;
            out = JsonValue::make_array();
            skip_ws();
            if (pos < len && data[pos] == ']') {
                ++pos;
                ok = true;
            } else {
                ok = true;
                while (true) {
                    JsonValue elem;
                    if (!parse_value(elem)) {
                        ok = false;
                        break;
                    }
                    out.arr_val.push_back(std::move(elem));
                    skip_ws();
                    if (pos < len && data[pos] == ',') {
                        ++pos;
                        continue;
                    }
                    if (pos < len && data[pos] == ']') {
                        ++pos;
                        break;
                    }
                    ok = false;
                    break;
                }
            }
        } else if (c == '{') {
            ++pos;
            out = JsonValue::make_object();
            skip_ws();
            if (pos < len && data[pos] == '}') {
                ++pos;
                ok = true;
            } else {
                ok = true;
                while (true) {
                    skip_ws();
                    std::string key;
                    if (!parse_string(key)) {
                        ok = false;
                        break;
                    }
                    skip_ws();
                    if (pos >= len || data[pos] != ':') {
                        ok = false;
                        break;
                    }
                    ++pos;
                    JsonValue val;
                    if (!parse_value(val)) {
                        ok = false;
                        break;
                    }
                    out.obj_val.emplace_back(std::move(key), std::move(val));
                    skip_ws();
                    if (pos < len && data[pos] == ',') {
                        ++pos;
                        continue;
                    }
                    if (pos < len && data[pos] == '}') {
                        ++pos;
                        break;
                    }
                    ok = false;
                    break;
                }
            }
        } else if (c == '-' || (c >= '0' && c <= '9')) {
            double num = 0.0;
            ok = parse_number(num);
            if (ok) out = JsonValue::make_number(num);
        }
        --depth;
        return ok;
    }
};

inline void serialize_string(const std::string &s, std::string &out) {
    out.push_back('"');
    for (unsigned char c : s) {
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\b': out += "\\b"; break;
            case '\f': out += "\\f"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (c < 0x20u) {
                    char buf[8];
                    std::snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned>(c));
                    out += buf;
                } else {
                    out.push_back(static_cast<char>(c));
                }
                break;
        }
    }
    out.push_back('"');
}

inline void serialize_value(const JsonValue &v, std::string &out) {
    switch (v.kind) {
        case JsonValue::Kind::Null:
            out += "null";
            break;
        case JsonValue::Kind::Bool:
            out += v.bool_val ? "true" : "false";
            break;
        case JsonValue::Kind::Number: {
            if (!std::isfinite(v.num_val)) {
                out += "null";
                break;
            }
            const double rounded = std::round(v.num_val);
            if (std::fabs(v.num_val - rounded) < 1e-12 &&
                std::fabs(rounded) < 9007199254740991.0) {
                char buf[32];
                std::snprintf(buf, sizeof(buf), "%lld", static_cast<long long>(rounded));
                out += buf;
            } else {
                char buf[64];
                std::snprintf(buf, sizeof(buf), "%.9g", v.num_val);
                out += buf;
            }
            break;
        }
        case JsonValue::Kind::String:
            serialize_string(v.str_val, out);
            break;
        case JsonValue::Kind::Array:
            out.push_back('[');
            for (size_t i = 0; i < v.arr_val.size(); ++i) {
                if (i > 0) out.push_back(',');
                serialize_value(v.arr_val[i], out);
            }
            out.push_back(']');
            break;
        case JsonValue::Kind::Object:
            out.push_back('{');
            for (size_t i = 0; i < v.obj_val.size(); ++i) {
                if (i > 0) out.push_back(',');
                serialize_string(v.obj_val[i].first, out);
                out.push_back(':');
                serialize_value(v.obj_val[i].second, out);
            }
            out.push_back('}');
            break;
    }
}

}  // namespace detail

inline bool parse_json(const char *data, size_t len, JsonValue &out) {
    if (!data || len == 0) return false;
    detail::JsonParser parser{data, len, 0, 0};
    if (!parser.parse_value(out)) return false;
    parser.skip_ws();
    return parser.pos == len;
}

inline std::string dump_json(const JsonValue &v) {
    std::string out;
    out.reserve(256);
    detail::serialize_value(v, out);
    return out;
}

}  // namespace shurufa::neural

#endif  // SHURUFA_NEURAL_JSON_H
