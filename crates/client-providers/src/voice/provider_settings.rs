//! 语音设置页的接口预检与换服务字段补丁；不读写偏好、不联网，也不记录凭据。

use super::provider_catalog::{
    polish_provider_preset, VoiceProviderPreset, ASR_PROVIDERS, LOCAL_POLISH_PROVIDERS,
    POLISH_PROVIDERS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 设置页允许留空使用默认地址；复用运行时 URL 解析，并保留移动宿主的协议和字符限制。
pub fn endpoint_issue(endpoint: &str, websocket: bool) -> Option<&'static str> {
    let endpoint = endpoint.trim();
    let prefix = if websocket { "wss://" } else { "https://" };
    if endpoint.is_empty()
        || (endpoint.starts_with(prefix)
            && !endpoint.contains('@')
            && super::provider::valid_mobile_voice_endpoint(endpoint, websocket))
    {
        return None;
    }
    Some(if websocket {
        "豆包的接口地址要以 wss:// 开头"
    } else {
        "接口地址要以 https:// 开头，且不能带用户名或密码"
    })
}

/// 校验语音润色服务接口地址；仅允许留空（或纯空格）表示沿用默认预设或由本机托管服务分配地址，
/// 非空时直接复用 `crate::ai::endpoint::validate`（放行 `https://` 以及本机/私网/Tailscale `http://`，拒绝公网明文 `http://` 与任何控制字符）。
pub fn polish_endpoint_issue(endpoint: &str) -> Option<&'static str> {
    // 仅去掉普通空格判断空串，禁止用 `.trim()` 吞掉 `\n`/`\r`/`\t` 等控制字符而掩盖 `ai::endpoint::validate` 的拒绝语义。
    if endpoint.trim_matches(' ').is_empty() || crate::ai::endpoint::validate(endpoint).is_ok() {
        return None;
    }
    Some(crate::ai::endpoint::hint(endpoint))
}

/// 判断给定润色服务与接口地址是否必须提供 API Token。
///
/// - `local`（Shurufa 托管本地 `llama-server`，使用内存态随机密钥）与 `ollama` 免填 Token；
/// - 任意经 `ai::endpoint::validate` 校验通过且指向本机/私网/Tailscale 主机的地址统一免填 Token（与 macOS 设置页及 `polish.rs` 保持跨层一致）；
/// - 其余公网云端服务必填 Token。
pub fn polish_requires_token(provider: &str, endpoint: &str) -> bool {
    let provider = provider.trim();
    if provider.eq_ignore_ascii_case("local") || provider.eq_ignore_ascii_case("ollama") {
        return false;
    }
    if crate::ai::endpoint::validate(endpoint).is_ok_and(|url| {
        url.host_str()
            .is_some_and(crate::ai::endpoint::is_local_network_host)
    }) {
        return false;
    }
    true
}

#[derive(Clone, Copy, Deserialize)]
pub enum ProviderService {
    #[serde(rename = "voice.asr")]
    Asr,
    #[serde(rename = "voice.polish")]
    Polish,
}

/// 默认替换出厂值并保留自定义字段；清空策略用于不沿用旧服务地址的宿主。
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFieldPolicy {
    #[default]
    FillDefaults,
    Clear,
}

/// 切换前的编辑值；字段名不带 asr/polish 前缀，由宿主映射对应设置。
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ProviderFields {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub token: String,
    pub tokens: BTreeMap<String, String>,
}

/// 只包含需要写回的字段；省略 endpoint/model 表示保留用户原值，空串表示清空。
#[derive(Serialize)]
pub struct ProviderUpdate {
    pub provider: String,
    pub token: String,
    pub tokens: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'static str>,
}

/// 保存原服务的令牌并恢复新服务的分槽，按显式策略生成地址和模型补丁。
pub fn provider_update(
    service: ProviderService,
    provider: &str,
    current: &ProviderFields,
    field_policy: ProviderFieldPolicy,
) -> ProviderUpdate {
    let mut tokens = current.tokens.clone();
    if !current.provider.is_empty() {
        if current.token.is_empty() {
            tokens.remove(&current.provider);
        } else {
            tokens.insert(current.provider.clone(), current.token.clone());
        }
    }
    let offline_asr =
        matches!(service, ProviderService::Asr) && matches!(provider, "system" | "local");
    let managed_local_polish = matches!(service, ProviderService::Polish) && provider == "local";
    let clears_token = offline_asr || managed_local_polish;
    if clears_token {
        tokens.remove(provider);
    }
    let token = if clears_token {
        String::new()
    } else {
        tokens.get(provider).cloned().unwrap_or_default()
    };
    let tables: &[&[VoiceProviderPreset]] = match service {
        ProviderService::Asr => &[ASR_PROVIDERS],
        ProviderService::Polish => &[POLISH_PROVIDERS, LOCAL_POLISH_PROVIDERS],
    };
    let defaults = if offline_asr {
        Some(("", ""))
    } else {
        match service {
            ProviderService::Asr => ASR_PROVIDERS
                .iter()
                .find(|preset| preset.id == provider)
                .map(|preset| (preset.endpoint, preset.model)),
            ProviderService::Polish => {
                polish_provider_preset(provider).map(|preset| (preset.endpoint, preset.model))
            }
        }
    };
    let (endpoint, model) = match field_policy {
        ProviderFieldPolicy::Clear if provider != current.provider => (Some(""), Some("")),
        ProviderFieldPolicy::Clear => (None, None),
        ProviderFieldPolicy::FillDefaults => (
            defaults.and_then(|(endpoint, _)| {
                fill_if_default(&current.endpoint, endpoint, tables, |preset| {
                    preset.endpoint
                })
            }),
            defaults.and_then(|(_, model)| {
                fill_if_default(&current.model, model, tables, |preset| preset.model)
            }),
        ),
    };
    ProviderUpdate {
        provider: provider.to_owned(),
        token,
        tokens,
        endpoint,
        model,
    }
}

fn fill_if_default(
    current: &str,
    next: &'static str,
    tables: &[&[VoiceProviderPreset]],
    field: impl Fn(&VoiceProviderPreset) -> &'static str,
) -> Option<&'static str> {
    let value = msime_client_base::text::trim_ui_whitespace(current);
    (value.is_empty()
        || tables.iter().flat_map(|table| table.iter()).any(|preset| {
            let preset_value = field(preset);
            !preset_value.is_empty() && preset_value == value
        }))
    .then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct EndpointContract {
        cases: Vec<EndpointCase>,
    }

    #[derive(Deserialize)]
    struct EndpointCase {
        endpoint: String,
        result: String,
    }

    #[test]
    fn endpoint_issue_preserves_existing_https_and_wss_contract() {
        assert_eq!(endpoint_issue("", false), None);
        assert_eq!(endpoint_issue("   ", true), None);
        assert_eq!(
            endpoint_issue("https://api.example.test/v1/audio/transcriptions", false),
            None
        );
        assert_eq!(
            endpoint_issue("wss://speech.example.test/v3/sauc", true),
            None
        );
        // 既有移动/识别接口契约保持不变：不放行明文 http:// 与嵌入凭据。
        assert!(endpoint_issue("http://127.0.0.1:8080/v1", false).is_some());
        assert!(endpoint_issue("https://user:pass@api.example.test/v1", false).is_some());
        assert!(endpoint_issue("https://speech.example.test/v3/sauc", true).is_some());
    }

    #[test]
    fn polish_endpoint_issue_matches_shared_ai_endpoint_contract() {
        assert_eq!(polish_endpoint_issue(""), None);
        assert_eq!(polish_endpoint_issue("   "), None);

        let contract: EndpointContract = serde_json::from_str(include_str!(
            "../../../../shared/contracts/ai-endpoint/cases.json"
        ))
        .unwrap();
        assert!(contract.cases.len() > 40);
        for case in &contract.cases {
            // 共享契约里的纯空串/空格用例对应 ai::endpoint::validate 拒空；设置页留空表示使用默认地址，而控制字符（如 \n/\t）必须照常拒绝。
            if case.endpoint.trim_matches(' ').is_empty() {
                assert_eq!(polish_endpoint_issue(&case.endpoint), None);
                continue;
            }
            let issue = polish_endpoint_issue(&case.endpoint);
            match case.result.as_str() {
                "allowed" => assert_eq!(issue, None, "{}", case.endpoint),
                "cleartext_public" => assert!(
                    issue.is_some_and(|msg| msg.starts_with("http:// 只能用于本机或局域网地址")),
                    "{}: {:?}",
                    case.endpoint,
                    issue
                ),
                "invalid" => assert!(
                    issue.is_some_and(|msg| msg.starts_with("请填写完整的接口地址")),
                    "{}: {:?}",
                    case.endpoint,
                    issue
                ),
                other => panic!("未知的契约预期: {other}"),
            }
        }
    }

    #[test]
    fn polish_endpoint_issue_allows_loopback_private_and_tailscale_http() {
        for allowed in [
            "https://api.example.test/v1/chat/completions",
            "http://127.0.0.1:11434/v1/chat/completions",
            "http://localhost:8080/v1/chat/completions",
            "http://[::1]:8080/v1/chat/completions",
            "http://192.168.1.20:11434/v1/chat/completions",
            "http://10.0.0.5:8080/v1/chat/completions",
            "http://172.16.0.9:8080/v1/chat/completions",
            "http://100.64.0.7:8080/v1/chat/completions",
            "http://studio.local:11434/v1/chat/completions",
        ] {
            assert_eq!(polish_endpoint_issue(allowed), None, "{allowed}");
        }
        for rejected in [
            "http://api.example.test/v1/chat/completions",
            "http://8.8.8.8/v1/chat/completions",
            "http://0x7f000001:8080/v1/chat/completions",
            "https://user:pass@api.example.test/v1/chat/completions",
            "https://api.example.test/v1/chat/completions#frag",
            "http://localhost:1234/v1\n",
            "http://localhost:1234/v1\r\n",
            "http://localhost:1234/v1\t",
            "\n",
            "file:///tmp/model.gguf",
        ] {
            assert!(polish_endpoint_issue(rejected).is_some(), "{rejected}");
        }
    }

    #[test]
    fn polish_requires_token_exempts_local_ollama_and_local_custom() {
        for (provider, endpoint) in [
            ("local", ""),
            ("LOCAL", "http://127.0.0.1:8080/v1/chat/completions"),
            ("ollama", ""),
            ("Ollama", "http://127.0.0.1:11434/v1/chat/completions"),
            ("custom", "http://127.0.0.1:8080/v1/chat/completions"),
            ("custom", "http://localhost:11434/v1/chat/completions"),
            ("custom", "http://[::1]:8080/v1/chat/completions"),
            ("custom", "http://192.168.1.20:8080/v1/chat/completions"),
            ("custom", "http://10.0.0.5:8080/v1/chat/completions"),
            ("custom", "http://100.64.0.7:8080/v1/chat/completions"),
            ("custom", "http://studio.local:8080/v1/chat/completions"),
            ("custom", "https://127.0.0.1:8443/v1/chat/completions"),
            // macOS 设置页默认 provider 为 deepseek 时将端点改为本机/私网地址也应免填 Token。
            ("deepseek", "http://127.0.0.1:11434/v1/chat/completions"),
            ("openai", "http://192.168.1.20:8080/v1/chat/completions"),
        ] {
            assert!(
                !polish_requires_token(provider, endpoint),
                "{provider} @ {endpoint}"
            );
        }

        for (provider, endpoint) in [
            ("deepseek", "https://api.deepseek.com/chat/completions"),
            ("openai", "https://api.openai.com/v1/chat/completions"),
            (
                "siliconflow",
                "https://api.siliconflow.cn/v1/chat/completions",
            ),
            ("custom", "https://api.example.test/v1/chat/completions"),
            ("custom", "http://api.example.test/v1/chat/completions"),
            ("custom", "http://localhost:1234/v1\n"),
            ("custom", ""),
        ] {
            assert!(
                polish_requires_token(provider, endpoint),
                "{provider} @ {endpoint}"
            );
        }
    }

    #[test]
    fn provider_update_round_trips_local_and_ollama_polish_presets() {
        let deepseek = ProviderFields {
            provider: "deepseek".into(),
            endpoint: "https://api.deepseek.com/chat/completions".into(),
            model: "deepseek-v4-flash".into(),
            token: "synthetic-deepseek-token".into(),
            tokens: BTreeMap::new(),
        };
        let to_local = provider_update(
            ProviderService::Polish,
            "local",
            &deepseek,
            ProviderFieldPolicy::FillDefaults,
        );
        assert_eq!(to_local.provider, "local");
        assert!(to_local.token.is_empty());
        assert_eq!(
            to_local.tokens.get("deepseek").map(String::as_str),
            Some("synthetic-deepseek-token")
        );
        assert!(!to_local.tokens.contains_key("local"));
        assert_eq!(to_local.endpoint, Some(""));
        assert_eq!(to_local.model, Some("local"));

        let local_fields = ProviderFields {
            provider: to_local.provider,
            endpoint: to_local.endpoint.unwrap().into(),
            model: to_local.model.unwrap().into(),
            token: to_local.token,
            tokens: to_local.tokens,
        };
        let to_ollama = provider_update(
            ProviderService::Polish,
            "ollama",
            &local_fields,
            ProviderFieldPolicy::FillDefaults,
        );
        assert_eq!(to_ollama.provider, "ollama");
        assert!(to_ollama.token.is_empty());
        assert_eq!(
            to_ollama.endpoint,
            Some("http://127.0.0.1:11434/v1/chat/completions")
        );
        assert_eq!(to_ollama.model, Some("qwen3.5:9b"));

        let ollama_fields = ProviderFields {
            provider: to_ollama.provider,
            endpoint: to_ollama.endpoint.unwrap().into(),
            model: to_ollama.model.unwrap().into(),
            token: to_ollama.token,
            tokens: to_ollama.tokens,
        };
        let back_to_deepseek = provider_update(
            ProviderService::Polish,
            "deepseek",
            &ollama_fields,
            ProviderFieldPolicy::FillDefaults,
        );
        assert_eq!(back_to_deepseek.token, "synthetic-deepseek-token");
        assert_eq!(
            back_to_deepseek.endpoint,
            Some("https://api.deepseek.com/chat/completions")
        );
        assert_eq!(back_to_deepseek.model, Some("deepseek-v4-flash"));
    }
}
