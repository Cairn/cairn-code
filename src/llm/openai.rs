use std::sync::{Arc, Mutex};
use super::provider::*;
use crate::http_client;
use crate::json;

pub struct OpenAIProvider {
    api_key: String,
}

impl OpenAIProvider {
    pub fn new() -> Self {
        OpenAIProvider { api_key: String::new() }
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
    }

    fn get_key(&self) -> String {
        if !self.api_key.is_empty() { return self.api_key.clone(); }
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            if !k.is_empty() { return k; }
        }
        crate::config::config_get_api_key("openai").unwrap_or_default()
    }
}

impl Provider for OpenAIProvider {
    fn name(&self) -> &str { "openai" }
    fn default_model(&self) -> &str { "gpt-4o" }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "gpt-4o".into(), name: "GPT-4o".into(), max_ctx: 128_000 },
            ModelInfo { id: "gpt-4o-mini".into(), name: "GPT-4o Mini".into(), max_ctx: 128_000 },
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
        if key.is_empty() { return Err(crate::llm::provider::missing_api_key("OPENAI_API_KEY")); }
        let body = openai_request_body(messages, tools, system, model, true)?;
        let req = http_client::HttpRequest {
            url: "https://api.openai.com/v1/chat/completions".into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {key}")),
                ("Content-Type".into(), "application/json".into()),
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
                if let Ok(val) = json::parse(json_str) {
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
        parse_openai_response(&raw)
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
        if key.is_empty() { return Err(crate::llm::provider::missing_api_key("OPENAI_API_KEY")); }
        let body = openai_request_body(messages, tools, system, model, false)?;
        let req = http_client::HttpRequest {
            url: "https://api.openai.com/v1/chat/completions".into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {key}")),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Some(body),
        };
        let resp = http_client::request(&req)?;
        parse_openai_complete_response(&resp.body)
    }
}

fn openai_request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    stream: bool,
) -> Result<String, String> {
    let mut body = format!("{{\"model\":\"{model}\",\"stream\":{stream}");
    body.push_str(",\"messages\":");
    body.push_str(&crate::llm::openai_compat::build_messages_json(messages, system));
    body.push_str(&crate::llm::openai_compat::build_tools_json(tools));
    body.push('}');
    crate::llm::openai_compat::validate_json_body(body)
}

fn parse_openai_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    crate::llm::openai_compat::parse_streaming_response(raw)
}

fn parse_openai_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    crate::llm::openai_compat::parse_complete_response(raw)
}
