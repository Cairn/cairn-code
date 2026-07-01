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

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
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
    parse_openrouter_complete_response(&resp.body)
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

fn escape_json_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn parse_openrouter_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
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

fn parse_openrouter_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::default_registry;

    #[test]
    fn test_all_tool_schemas_are_valid_json() {
        let reg = default_registry();
        for def in reg.definitions() {
            match crate::json::parse(&def.input_schema) {
                Ok(v) => {
                    let obj = v.as_object().expect(&format!("{}: schema should be object", def.name));
                    assert!(obj.get("type").is_some(), "{}: schema missing 'type'", def.name);
                    assert!(obj.get("properties").is_some(), "{}: schema missing 'properties'", def.name);
                }
                Err(e) => panic!("{}: invalid schema JSON: {e}", def.name),
            }
        }
    }

    #[test]
    fn test_full_request_body_with_all_tools() {
        let reg = default_registry();
        let tools: Vec<ToolDefinition> = reg.definitions();
        let msgs = vec![Message { role: "user".into(), content: Content::Text("hello".into()) }];
        let body = openrouter_request_body(&msgs, &tools, "You are helpful.", "openai/gpt-4o", true, 4096).unwrap();
        match crate::json::parse(&body) {
            Ok(v) => {
                let obj = v.as_object().unwrap();
                assert_eq!(obj.get("model").and_then(|v| v.as_str()), Some("openai/gpt-4o"));
                assert!(obj.get("messages").and_then(|v| v.as_array()).is_some());
                let tools_arr = obj.get("tools").and_then(|v| v.as_array()).unwrap();
                assert_eq!(tools_arr.len(), 12);
                for (i, tool_val) in tools_arr.iter().enumerate() {
                    let tool_obj = tool_val.as_object().unwrap();
                    assert_eq!(tool_obj.get("type").and_then(|v| v.as_str()), Some("function"));
                    let func = tool_obj.get("function").unwrap().as_object().unwrap();
                    assert!(func.get("name").is_some(), "tool {i} missing name");
                    assert!(func.get("parameters").is_some(), "tool {i} missing parameters");
                }
            }
            Err(e) => panic!("Full request body invalid: {e}"),
        }
    }
}
