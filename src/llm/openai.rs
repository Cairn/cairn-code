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
        std::env::var("OPENAI_API_KEY").unwrap_or_default()
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
    ) -> Result<(Vec<Message>, Usage), String> {
        let key = self.get_key();
        if key.is_empty() { return Err("OPENAI_API_KEY not set".into()); }
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
        if key.is_empty() { return Err("OPENAI_API_KEY not set".into()); }
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
    let mut body = String::new();
    body.push_str(&format!("{{\"model\":\"{model}\",\"stream\":{stream}"));
    body.push_str(",\"messages\":[");
    let mut first = true;
    if !system.is_empty() {
        let escaped = escape_json_str(system);
        body.push_str(&format!("{{\"role\":\"system\",\"content\":\"{escaped}\"}}"));
        first = false;
    }
    for msg in messages.iter() {
        if !first { body.push(','); }
        first = false;
        match &msg.content {
            Content::Text(t) => {
                let escaped = escape_json_str(t);
                body.push_str(&format!(
                    "{{\"role\":\"{}\",\"content\":\"{escaped}\"}}",
                    msg.role
                ));
            }
            Content::ToolUse(tu) => {
                let args_escaped = tu.input.replace('\\', "\\\\").replace('"', "\\\"");
                body.push_str(&format!(
                    "{{\"role\":\"assistant\",\"tool_calls\":[{{\"id\":\"{}\",\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"arguments\":\"{}\"}}}}]}}",
                    tu.id, tu.name, args_escaped
                ));
            }
            Content::ToolResult(tr) => {
                let escaped = escape_json_str(&tr.content);
                body.push_str(&format!(
                    "{{\"role\":\"tool\",\"tool_call_id\":\"{}\",\"content\":\"{escaped}\"}}",
                    tr.tool_use_id
                ));
            }
            Content::Thinking(t) => {
                let escaped = escape_json_str(t);
                body.push_str(&format!(
                    "{{\"role\":\"assistant\",\"content\":\"{escaped}\"}}",
                ));
            }
        }
    }
    body.push(']');
    if !tools.is_empty() {
        body.push_str(",\"tools\":[");
        for (i, tool) in tools.iter().enumerate() {
            if i > 0 { body.push(','); }
            let name_esc = escape_json_str(&tool.name);
            let desc_esc = escape_json_str(&tool.description);
            body.push_str(&format!(
                "{{\"type\":\"function\",\"function\":{{\"name\":\"{name_esc}\",\"description\":\"{desc_esc}\",\"parameters\":{}}}}}",
                tool.input_schema
            ));
        }
        body.push(']');
    }
    body.push('}');
    if let Err(e) = crate::json::parse(&body) {
        let pos = e.pos;
        let start = pos.saturating_sub(20);
        let end = (pos + 20).min(body.len());
        let context = &body[start..end];
        let ch = body.as_bytes().get(pos).map(|&b| b as char).unwrap_or('?');
        return Err(format!("Invalid JSON body: {e}\nChar at pos {pos}: '{ch}' (0x{:02X})\nContext: ...{context}...", ch as u8));
    }
    Ok(body)
}

fn escape_json_str<'a>(s: &'a str) -> std::borrow::Cow<'a, str> {
    let bytes = s.as_bytes();
    let mut first_escape = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\\' || b == b'"' || b == b'\n' || b == b'\r' || b == b'\t' {
            first_escape = Some(i);
            break;
        }
    }

    let first_idx = match first_escape {
        Some(idx) => idx,
        None => return std::borrow::Cow::Borrowed(s),
    };

    let mut out = String::with_capacity(s.len() + s.len() / 4);
    out.push_str(&s[..first_idx]);

    for c in s[first_idx..].chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}

fn parse_openai_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages = Vec::new();
    let mut usage = Usage::default();
    let mut collected = String::new();
    let mut tool_calls: std::collections::HashMap<u64, (String, String, String)> = std::collections::HashMap::new();
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
                            if let Some(tc_arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                                for tc in tc_arr {
                                    let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let entry = tool_calls.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                                    if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                        if !id.is_empty() { entry.0 = id.to_string(); }
                                    }
                                    if let Some(func) = tc.get("function") {
                                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                            if !name.is_empty() { entry.1 = name.to_string(); }
                                        }
                                        if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                                            entry.2.push_str(args);
                                        }
                                    }
                                }
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
    if !collected.is_empty() || !tool_calls.is_empty() {
        let content = if !collected.is_empty() {
            Content::Text(collected)
        } else {
            let mut calls: Vec<_> = tool_calls.into_iter().collect();
            calls.sort_by_key(|(idx, _)| *idx);
            let tu = calls.into_iter().next().map(|(_, (id, name, args))| {
                super::provider::ToolUse { id, name, input: args }
            }).unwrap_or(super::provider::ToolUse {
                id: String::new(), name: String::new(), input: "{}".into(),
            });
            Content::ToolUse(tu)
        };
        messages.push(Message { role: "assistant".into(), content });
    }
    Ok((messages, usage))
}

fn parse_openai_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages = Vec::new();
    let mut usage = Usage::default();

    let val = json::parse(raw).map_err(|e| format!("Failed to parse response: {e}"))?;

    if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
        if let Some(choice) = choices.first() {
            if let Some(msg) = choice.get("message") {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("assistant").to_string();
                let content = if let Some(text) = msg.get("content").and_then(|v| v.as_str()) {
                    Content::Text(text.to_string())
                } else if let Some(tc_arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    let mut calls: Vec<_> = tc_arr.iter().enumerate().collect();
                    calls.sort_by_key(|(i, _)| *i);
                    let tu = calls.into_iter().next().map(|(_, tc)| {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let args = tc.get("function").and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("{}").to_string();
                        super::provider::ToolUse { id, name, input: args }
                    }).unwrap_or(super::provider::ToolUse {
                        id: String::new(), name: String::new(), input: "{}".into(),
                    });
                    Content::ToolUse(tu)
                } else {
                    Content::Text(String::new())
                };
                messages.push(Message { role, content });
            }
        }
    }
    if let Some(u) = val.get("usage") {
        usage.input_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        usage.output_tokens = u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    }
    Ok((messages, usage))
}
