use std::sync::{Arc, Mutex};
use super::provider::*;
use crate::http_client;
use crate::json;

pub struct OllamaProvider {
    base_url: String,
}

impl OllamaProvider {
    pub fn new() -> Self {
        OllamaProvider {
            base_url: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into()),
        }
    }

    fn chat_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

impl Provider for OllamaProvider {
    fn name(&self) -> &str { "ollama" }
    fn default_model(&self) -> &str { "llama3.2" }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "llama3.2".into(), name: "Llama 3.2".into(), max_ctx: 128_000 },
            ModelInfo { id: "llama3.1".into(), name: "Llama 3.1".into(), max_ctx: 128_000 },
            ModelInfo { id: "codellama".into(), name: "Code Llama".into(), max_ctx: 16_000 },
            ModelInfo { id: "mistral".into(), name: "Mistral".into(), max_ctx: 32_000 },
            ModelInfo { id: "mixtral".into(), name: "Mixtral".into(), max_ctx: 32_000 },
            ModelInfo { id: "deepseek-coder".into(), name: "DeepSeek Coder".into(), max_ctx: 16_000 },
            ModelInfo { id: "qwen2.5".into(), name: "Qwen 2.5".into(), max_ctx: 32_000 },
            ModelInfo { id: "phi4".into(), name: "Phi-4".into(), max_ctx: 16_000 },
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
    ) -> Result<(Vec<Message>, Usage), String> {
        let body = ollama_request_body(messages, tools, system, model, true)?;
        let req = http_client::HttpRequest {
            url: self.chat_url(),
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Some(body),
        };
        let response_data: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let response_data2 = response_data.clone();
        http_client::request_streaming(&req, move |line| {
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
        })?;
        let raw = response_data.lock().unwrap().clone();
        parse_ollama_response(&raw)
    }

    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        _max_tokens: usize,
    ) -> Result<(Vec<Message>, Usage), String> {
        let body = ollama_request_body(messages, tools, system, model, false)?;
        let req = http_client::HttpRequest {
            url: self.chat_url(),
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Some(body),
        };
        let resp = http_client::request(&req)?;
        parse_ollama_response(&resp.body)
    }
}

fn ollama_request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    stream: bool,
) -> Result<String, String> {
    let mut body = String::new();
    body.push_str(&format!("{{\"model\":\"{model}\",\"stream\":{stream}"));
    body.push_str(",\"messages\":[");
    if !system.is_empty() {
        let escaped = system.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        body.push_str(&format!("{{\"role\":\"system\",\"content\":\"{escaped}\"}},"));
    }
    for (i, msg) in messages.iter().enumerate() {
        if i > 0 { body.push(','); }
        body.push_str(&format!("{{\"role\":\"{}\",\"content\":", msg.role));
        match &msg.content {
            Content::Text(t) => {
                let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
                body.push_str(&format!("\"{escaped}\""));
            }
            Content::ToolUse(tu) => {
                body.push_str(&format!(
                    "[{{\"type\":\"tool_use\",\"id\":\"{}\",\"name\":\"{}\",\"input\":{}}}]",
                    tu.id, tu.name, tu.input
                ));
            }
            Content::ToolResult(tr) => {
                let escaped = tr.content.replace('\\', "\\\\").replace('"', "\\\"");
                body.push_str(&format!(
                    "[{{\"type\":\"tool_result\",\"tool_use_id\":\"{}\",\"content\":\"{escaped}\"}}]",
                    tr.tool_use_id
                ));
            }
            Content::Thinking(t) => {
                let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
                body.push_str(&format!("\"{escaped}\""));
            }
        }
        body.push('}');
    }
    body.push(']');
    if !tools.is_empty() {
        body.push_str(",\"tools\":[");
        for (i, tool) in tools.iter().enumerate() {
            if i > 0 { body.push(','); }
            body.push_str(&format!(
                "{{\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"description\":\"{}\",\"parameters\":{}}}}}",
                tool.name, tool.description, tool.input_schema
            ));
        }
        body.push(']');
    }
    body.push('}');
    Ok(body)
}

fn parse_ollama_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages = Vec::new();
    let mut usage = Usage::default();
    let mut collected = String::new();
    for line in raw.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" { continue; }
            if let Ok(val) = json::parse(data) {
                if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
                    if let Some(choice) = choices.first() {
                        if let Some(delta) = choice.get("delta") {
                            if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                                collected.push_str(text);
                            }
                        }
                    }
                }
                if let Some(u) = val.get("usage") {
                    usage.input_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    usage.output_tokens = u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                }
            }
        }
    }
    if !collected.is_empty() {
        messages.push(Message { role: "assistant".into(), content: Content::Text(collected) });
    }
    Ok((messages, usage))
}
