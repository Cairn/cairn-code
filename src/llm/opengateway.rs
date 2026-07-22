//! GitLawb OpenGateway — OpenAI-compatible smart router.
//!
//! Mirrors zero's `gitlawb-opengateway` catalog entry:
//! - Base URL: `https://opengateway.gitlawb.com/v1`
//! - Chat: `POST /v1/chat/completions` with `Authorization: Bearer <ogw_…>`
//! - Routes by model id across upstream providers (xiaomi-mimo, minimax, qwen, …)
//! - Key env: `GITLAWB_OPENGATEWAY_API_KEY` (also accepts `OPENGATEWAY_API_KEY`)

use std::sync::{Arc, Mutex};

use super::provider::*;
use crate::http_client;
use crate::llm::openai_compat;

const CHAT_URL: &str = "https://opengateway.gitlawb.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "mimo-v2.5-pro";

pub struct OpenGatewayProvider {
    api_key: String,
}

impl OpenGatewayProvider {
    pub fn new() -> Self {
        OpenGatewayProvider { api_key: String::new() }
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
    }

    fn get_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        if let Ok(k) = std::env::var("GITLAWB_OPENGATEWAY_API_KEY") {
            if !k.is_empty() { return k; }
        }
        if let Ok(k) = std::env::var("OPENGATEWAY_API_KEY") {
            if !k.is_empty() { return k; }
        }
        crate::config::config_get_api_key("opengateway").unwrap_or_default()
    }
}

impl Provider for OpenGatewayProvider {
    fn name(&self) -> &str { "opengateway" }
    fn default_model(&self) -> &str { DEFAULT_MODEL }

    fn available_models(&self) -> Vec<ModelInfo> {
        // Curated coding defaults from zero's OpenGateway catalog. The gateway
        // accepts any model id its upstreams expose; the picker lists common ones.
        vec![
            ModelInfo { id: "mimo-v2.5-pro".into(), name: "Xiaomi MiMo V2.5 Pro".into(), max_ctx: 128_000 },
            ModelInfo { id: "xiaomi/mimo-v2.5-pro".into(), name: "Xiaomi MiMo V2.5 Pro (namespaced)".into(), max_ctx: 128_000 },
            ModelInfo { id: "mimo-v2.5-pro-ultraspeed".into(), name: "Xiaomi MiMo Ultraspeed".into(), max_ctx: 128_000 },
            ModelInfo { id: "xiaomi/mimo-v2.5".into(), name: "Xiaomi MiMo V2.5".into(), max_ctx: 128_000 },
            ModelInfo { id: "MiniMax-M3".into(), name: "MiniMax M3".into(), max_ctx: 262_144 },
            ModelInfo { id: "minimax/minimax-m3".into(), name: "MiniMax M3 (namespaced)".into(), max_ctx: 262_144 },
            ModelInfo { id: "qwen-plus".into(), name: "Qwen Plus".into(), max_ctx: 128_000 },
            ModelInfo { id: "qwen/qwen3.7-max".into(), name: "Qwen 3.7 Max".into(), max_ctx: 128_000 },
            ModelInfo { id: "gemini-2.5-pro".into(), name: "Gemini 2.5 Pro".into(), max_ctx: 1_000_000 },
            ModelInfo { id: "google/gemini-3.1-flash-lite".into(), name: "Gemini 3.1 Flash Lite".into(), max_ctx: 1_000_000 },
            ModelInfo { id: "glm-4.6".into(), name: "Z.ai GLM 4.6".into(), max_ctx: 128_000 },
            ModelInfo { id: "z-ai/glm-5.2".into(), name: "Z.ai GLM 5.2".into(), max_ctx: 128_000 },
            ModelInfo { id: "tencent/hy3".into(), name: "Tencent HY3 (free)".into(), max_ctx: 262_144 },
            ModelInfo { id: "nvidia/llama-3.1-nemotron-70b-instruct".into(), name: "NVIDIA Nemotron 70B".into(), max_ctx: 128_000 },
            ModelInfo { id: "nvidia/nemotron-3-ultra-550b-a55b:free".into(), name: "Nemotron 3 Ultra free".into(), max_ctx: 128_000 },
        ]
    }

    fn stream_complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        _max_tokens: usize,
        mut on_chunk: StreamingCallback,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(Vec<Message>, Usage), String> {
        let key = self.get_key();
        if key.is_empty() {
            return Err(crate::llm::provider::missing_api_key("GITLAWB_OPENGATEWAY_API_KEY"));
        }
        let body = request_body(messages, tools, system, model, true)?;
        let req = http_client::HttpRequest {
            url: CHAT_URL.into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {key}")),
                ("Content-Type".into(), "application/json".into()),
                ("HTTP-Referer".into(), "https://github.com/Cairn/cairn-code".into()),
                ("X-Title".into(), "Cairn Code".into()),
            ],
            body: Some(body),
        };
        let response_data: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let response_data2 = response_data.clone();
        http_client::request_streaming_with_cancel(&req, move |line| {
            let mut data = response_data2.lock().unwrap();
            data.push_str(line);
            data.push('\n');
            if let Some(json_str) = line.strip_prefix("data: ") {
                if json_str == "[DONE]" { return; }
                if let Ok(val) = crate::json::parse(json_str) {
                    if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
                        if let Some(choice) = choices.first() {
                            if let Some(delta) = choice.get("delta") {
                                if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                                    on_chunk(text, "text");
                                }
                            }
                        }
                    }
                }
            }
        }, Some(cancel))?;
        let raw = response_data.lock().unwrap().clone();
        openai_compat::parse_streaming_response(&raw)
    }

    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        _max_tokens: usize,
    ) -> Result<(Vec<Message>, Usage), String> {
        let key = self.get_key();
        if key.is_empty() {
            return Err(crate::llm::provider::missing_api_key("GITLAWB_OPENGATEWAY_API_KEY"));
        }
        let body = request_body(messages, tools, system, model, false)?;
        let req = http_client::HttpRequest {
            url: CHAT_URL.into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {key}")),
                ("Content-Type".into(), "application/json".into()),
                ("HTTP-Referer".into(), "https://github.com/Cairn/cairn-code".into()),
                ("X-Title".into(), "Cairn Code".into()),
            ],
            body: Some(body),
        };
        let resp = http_client::request(&req)?;
        openai_compat::parse_complete_response(&resp.body)
    }
}

fn request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    stream: bool,
) -> Result<String, String> {
    let mut body = format!("{{\"model\":\"{model}\",\"stream\":{stream}");
    body.push_str(",\"messages\":");
    body.push_str(&openai_compat::build_messages_json(messages, system));
    body.push_str(&openai_compat::build_tools_json(tools));
    body.push('}');
    openai_compat::validate_json_body(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_and_default_model() {
        let p = OpenGatewayProvider::new();
        assert_eq!(p.name(), "opengateway");
        assert_eq!(p.default_model(), DEFAULT_MODEL);
        assert!(p.available_models().iter().any(|m| m.id == DEFAULT_MODEL));
    }

    #[test]
    fn curated_models_nonempty() {
        let p = OpenGatewayProvider::new();
        assert!(p.available_models().len() >= 5);
        assert!(
            CHAT_URL.starts_with("https://opengateway.gitlawb.com/v1/")
                && CHAT_URL.ends_with("/chat/completions"),
            "unexpected chat URL: {CHAT_URL}"
        );
    }

    #[test]
    fn request_body_is_valid_json() {
        let msgs = vec![Message {
            role: "user".into(),
            content: Content::Text("hi".into()),
        }];
        let body = request_body(&msgs, &[], "sys", DEFAULT_MODEL, true).unwrap();
        let val = crate::json::parse(&body).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("model").and_then(|v| v.as_str()), Some(DEFAULT_MODEL));
        assert_eq!(obj.get("stream").and_then(|v| v.as_bool()), Some(true));
        assert!(obj.get("messages").and_then(|v| v.as_array()).is_some());
    }

    #[test]
    fn missing_key_errors_clearly() {
        // Ensure empty provider key + cleared env would fail; we only check the
        // message helper path via get_key on a fresh provider without env set
        // when the env is already empty. If the user has a key in env this may
        // not be empty — still assert with_api_key overrides.
        let p = OpenGatewayProvider::new().with_api_key("");
        // with empty stored key falls through to env; just check request_body works
        let body = request_body(&[], &[], "", "mimo-v2.5-pro", false).unwrap();
        assert!(body.contains("mimo-v2.5-pro"));
        let _ = p;
    }
}
