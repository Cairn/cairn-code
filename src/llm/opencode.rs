use std::sync::{Arc, Mutex};
use super::provider::*;
use crate::http_client;
use crate::json;

pub struct OpenCodeProvider {
    api_key: String,
}

impl OpenCodeProvider {
    pub fn new() -> Self {
        OpenCodeProvider { api_key: String::new() }
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
    }

    fn get_key(&self) -> String {
        if !self.api_key.is_empty() { return self.api_key.clone(); }
        std::env::var("OPENCODE_API_KEY").unwrap_or_default()
    }
}

impl Provider for OpenCodeProvider {
    fn name(&self) -> &str { "opencode" }
    fn default_model(&self) -> &str { "deepseek-v4-flash-free" }
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "deepseek-v4-flash-free".into(), name: "DeepSeek V4 Flash".into(), max_ctx: 128_000 },
            ModelInfo { id: "big-pickle".into(), name: "Big Pickle".into(), max_ctx: 128_000 },
            ModelInfo { id: "mimo-v2.5-free".into(), name: "Mimo v2.5".into(), max_ctx: 128_000 },
            ModelInfo { id: "north-mini-code-free".into(), name: "North Mini Code".into(), max_ctx: 128_000 },
            ModelInfo { id: "nemotron-3-ultra-free".into(), name: "Nemotron 3 Ultra".into(), max_ctx: 128_000 },
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
        let body = build_request_body(messages, tools, system, model, true)?;
        let mut headers: Vec<(String, String)> = vec![("Content-Type".into(), "application/json".into())];
        let key = self.get_key();
        if !key.is_empty() {
            headers.push(("Authorization".into(), format!("Bearer {key}")));
        }
        let req = http_client::HttpRequest {
            url: "https://opencode.ai/zen/v1/chat/completions".into(),
            headers,
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
        parse_opencode_response(&raw)
    }
    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        _max_tokens: usize,
    ) -> Result<(Vec<Message>, Usage), String> {
        let body = build_request_body(messages, tools, system, model, false)?;
        let mut headers: Vec<(String, String)> = vec![("Content-Type".into(), "application/json".into())];
        let key = self.get_key();
        if !key.is_empty() {
            headers.push(("Authorization".into(), format!("Bearer {key}")));
        }
        let req = http_client::HttpRequest {
            url: "https://opencode.ai/zen/v1/chat/completions".into(),
            headers,
            body: Some(body),
        };
        let resp = http_client::request(&req)?;
        parse_opencode_response(&resp.body)
    }
}

fn build_request_body(
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

fn parse_opencode_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name_and_default_model() {
        let p = OpenCodeProvider::new();
        assert_eq!(p.name(), "opencode");
        assert_eq!(p.default_model(), "deepseek-v4-flash-free");
    }

    #[test]
    fn test_available_models_nonempty() {
        let p = OpenCodeProvider::new();
        let models = p.available_models();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id == "deepseek-v4-flash-free"));
    }

    #[test]
    fn test_with_api_key_constructor() {
        let _p = OpenCodeProvider::new().with_api_key("sk-test");
        // If this compiles, with_api_key is wired up.
    }

    #[test]
    fn test_request_body_shape() {
        let msgs = vec![Message { role: "user".into(), content: Content::Text("hi".into()) }];
        let body = build_request_body(&msgs, &[], "sys", "deepseek-v4-flash-free", true).unwrap();
        let v = crate::json::parse(&body).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("model").and_then(|v| v.as_str()), Some("deepseek-v4-flash-free"));
        // stream is serialized as a JSON bool, not a string
        assert_eq!(obj.get("stream").map(|v| v.as_bool().unwrap_or(false)), Some(true));
        let msgs_arr = obj.get("messages").and_then(|v| v.as_array()).unwrap();
        // system + user
        assert_eq!(msgs_arr.len(), 2);
    }

    #[test]
    fn test_request_body_no_system() {
        let msgs = vec![Message { role: "user".into(), content: Content::Text("hi".into()) }];
        let body = build_request_body(&msgs, &[], "", "m", false).unwrap();
        let v = crate::json::parse(&body).unwrap();
        let msgs_arr = v.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(msgs_arr.len(), 1);
    }

    #[test]
    fn test_parse_response_collects_text() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\ndata: [DONE]\n";
        let (msgs, _usage) = parse_opencode_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::Text(t) => assert_eq!(t, "hello world"),
            _ => panic!("expected Text content"),
        }
    }

    #[test]
    fn test_parse_response_handles_usage() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":5}}\n\ndata: [DONE]\n";
        let (_msgs, usage) = parse_opencode_response(raw).unwrap();
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_response_empty() {
        let raw = "";
        let (msgs, usage) = parse_opencode_response(raw).unwrap();
        assert!(msgs.is_empty());
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }
}
