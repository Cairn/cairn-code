use std::sync::{Arc, Mutex};
use super::provider::*;
use crate::http_client;
use crate::json;

pub struct OpenRouterProvider {
    api_key: String,
}

impl OpenRouterProvider {
    pub fn new() -> Self {
        OpenRouterProvider { api_key: String::new() }
    }

    fn get_key(&self) -> String {
        if !self.api_key.is_empty() { return self.api_key.clone(); }
        std::env::var("OPENROUTER_API_KEY").unwrap_or_default()
    }
}

impl Provider for OpenRouterProvider {
    fn name(&self) -> &str { "openrouter" }
    fn default_model(&self) -> &str { "openai/gpt-4o" }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "openai/gpt-4o".into(), name: "GPT-4o".into(), max_ctx: 128_000 },
            ModelInfo { id: "openai/gpt-4o-mini".into(), name: "GPT-4o Mini".into(), max_ctx: 128_000 },
            ModelInfo { id: "openai/gpt-5-mini".into(), name: "GPT-5 Mini".into(), max_ctx: 128_000 },
            ModelInfo { id: "openai/o3-mini".into(), name: "o3 Mini".into(), max_ctx: 200_000 },
            ModelInfo { id: "openai/o4-mini".into(), name: "o4 Mini".into(), max_ctx: 200_000 },
            ModelInfo { id: "anthropic/claude-sonnet-4-20250514".into(), name: "Claude Sonnet 4".into(), max_ctx: 200_000 },
            ModelInfo { id: "google/gemini-2.5-pro".into(), name: "Gemini 2.5 Pro".into(), max_ctx: 1_000_000 },
            ModelInfo { id: "google/gemini-2.5-flash".into(), name: "Gemini 2.5 Flash".into(), max_ctx: 1_000_000 },
            ModelInfo { id: "deepseek/deepseek-chat-v3-0324".into(), name: "DeepSeek V3".into(), max_ctx: 128_000 },
            ModelInfo { id: "deepseek/deepseek-r1".into(), name: "DeepSeek R1".into(), max_ctx: 128_000 },
            ModelInfo { id: "meta-llama/llama-4-scout".into(), name: "Llama 4 Scout".into(), max_ctx: 128_000 },
            ModelInfo { id: "mistralai/mistral-large-24".into(), name: "Mistral Large".into(), max_ctx: 128_000 },
            ModelInfo { id: "qwen/qwq-32b".into(), name: "QwQ 32B".into(), max_ctx: 32_000 },
        ]
    }

    fn stream_complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        max_tokens: usize,
        mut on_chunk: StreamingCallback,
    ) -> Result<(Vec<Message>, Usage), String> {
        let key = self.get_key();
        if key.is_empty() { return Err("OPENROUTER_API_KEY not set".into()); }

        let mut mt = if max_tokens == 0 || max_tokens > 4096 {
            4096
        } else {
            max_tokens
        };

        loop {
            let body = openrouter_request_body(messages, tools, system, model, true, mt)?;
            let result = do_stream_request(&key, body, &mut on_chunk);
            match result {
                Err(e) => {
                    if let Some(affordable) = parse_openrouter_402(&e) {
                        if affordable < mt {
                            mt = affordable;
                            continue;
                        }
                    }
                    return Err(e);
                }
                Ok(r) => return Ok(r),
            }
        }
    }

    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        max_tokens: usize,
    ) -> Result<(Vec<Message>, Usage), String> {
        let key = self.get_key();
        if key.is_empty() { return Err("OPENROUTER_API_KEY not set".into()); }

        let mut mt = if max_tokens == 0 || max_tokens > 4096 {
            4096
        } else {
            max_tokens
        };

        loop {
            let body = openrouter_request_body(messages, tools, system, model, false, mt)?;
            let result = do_complete_request(&key, body);
            match result {
                Err(e) => {
                    if let Some(affordable) = parse_openrouter_402(&e) {
                        if affordable < mt {
                            mt = affordable;
                            continue;
                        }
                    }
                    return Err(e);
                }
                Ok(r) => return Ok(r),
            }
        }
    }
}

fn do_stream_request(
    key: &str,
    body: String,
    on_chunk: &mut StreamingCallback,
) -> Result<(Vec<Message>, Usage), String> {
    let req = http_client::HttpRequest {
        url: "https://openrouter.ai/api/v1/chat/completions".into(),
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
    parse_openrouter_response(&raw)
}

fn do_complete_request(key: &str, body: String) -> Result<(Vec<Message>, Usage), String> {
    let req = http_client::HttpRequest {
        url: "https://openrouter.ai/api/v1/chat/completions".into(),
        headers: vec![
            ("Authorization".into(), format!("Bearer {key}")),
            ("Content-Type".into(), "application/json".into()),
            ("HTTP-Referer".into(), "https://github.com/Cairn/cairn-code".into()),
            ("X-Title".into(), "Cairn Code".into()),
        ],
        body: Some(body),
    };
    let resp = http_client::request(&req)?;
    parse_openrouter_response(&resp.body)
}

fn openrouter_request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    stream: bool,
    max_tokens: usize,
) -> Result<String, String> {
    let mut body = String::new();
    body.push_str(&format!("{{\"model\":\"{model}\",\"stream\":{stream}"));
    body.push_str(&format!(",\"max_completion_tokens\":{max_tokens}"));
    body.push_str(",\"messages\":[");
    if !system.is_empty() {
        let escaped = system.replace('\\', "\\\\").replace('"', "\\\"");
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

fn parse_openrouter_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
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

fn parse_openrouter_402(err: &str) -> Option<usize> {
    let lower = err.to_lowercase();
    if lower.contains("fewer max_tokens") || lower.contains("fewer max tokens") || lower.contains("fewer max_completion_tokens") {
        return Some(1024);
    }
    // "requested up to N tokens, but can only afford M"
    if let Some(pos) = err.find("but can only afford ") {
        let after = &err[pos + "but can only afford ".len()..];
        let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(affordable) = num.parse::<usize>() {
            return Some(affordable.min(4096).max(1));
        }
    }
    None
}
