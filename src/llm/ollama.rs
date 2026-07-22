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
        parse_ollama_complete_response(&resp.body)
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

fn parse_ollama_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name_and_default_model() {
        let p = OllamaProvider::new();
        assert_eq!(p.name(), "ollama");
        assert_eq!(p.default_model(), "llama3.2");
    }

    #[test]
    fn test_parse_complete_response_collects_text() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","content":"pong"}}],"usage":{"prompt_tokens":10,"completion_tokens":1}}"#;
        let (msgs, usage) = parse_ollama_complete_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::Text(t) => assert_eq!(t, "pong"),
            _ => panic!("expected Text content"),
        }
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 1);
    }

    #[test]
    fn test_parse_complete_response_tool_call() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call_1","function":{"name":"glob","arguments":"{\"pattern\":\"*.rs\"}"}}]}}]}"#;
        let (msgs, _usage) = parse_ollama_complete_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::ToolUse(tu) => {
                assert_eq!(tu.id, "call_1");
                assert_eq!(tu.name, "glob");
            }
            _ => panic!("expected ToolUse content"),
        }
    }

    #[test]
    fn test_parse_response_streaming_text() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\ndata: [DONE]\n";
        let (msgs, _usage) = parse_ollama_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::Text(t) => assert_eq!(t, "hello world"),
            _ => panic!("expected Text content"),
        }
    }

    #[test]
    fn test_parse_response_streaming_tool_call() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"glob\",\"arguments\":\"\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"pattern\\\":\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"*.rs\\\"}\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        let (msgs, _usage) = parse_ollama_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::ToolUse(tu) => {
                assert_eq!(tu.id, "call_1");
                assert_eq!(tu.name, "glob");
                assert_eq!(tu.input, r#"{"pattern":"*.rs"}"#);
            }
            _ => panic!("expected ToolUse content"),
        }
    }
}
