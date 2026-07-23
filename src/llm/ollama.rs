use super::provider::*;
use crate::http_client;
use crate::json;
use std::sync::{Arc, Mutex};

pub struct OllamaProvider {
    base_url: String,
}

impl OllamaProvider {
    pub fn new() -> Self {
        OllamaProvider {
            base_url: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
        }
    }

    fn chat_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }
}

impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }
    fn default_model(&self) -> &str {
        "llama3.2"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "llama3.2".into(),
                name: "Llama 3.2".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "llama3.1".into(),
                name: "Llama 3.1".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "codellama".into(),
                name: "Code Llama".into(),
                max_ctx: 16_000,
            },
            ModelInfo {
                id: "mistral".into(),
                name: "Mistral".into(),
                max_ctx: 32_000,
            },
            ModelInfo {
                id: "mixtral".into(),
                name: "Mixtral".into(),
                max_ctx: 32_000,
            },
            ModelInfo {
                id: "deepseek-coder".into(),
                name: "DeepSeek Coder".into(),
                max_ctx: 16_000,
            },
            ModelInfo {
                id: "qwen2.5".into(),
                name: "Qwen 2.5".into(),
                max_ctx: 32_000,
            },
            ModelInfo {
                id: "phi4".into(),
                name: "Phi-4".into(),
                max_ctx: 16_000,
            },
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
        let body = ollama_request_body(messages, tools, system, model, true)?;
        let req = http_client::HttpRequest {
            url: self.chat_url(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(body),
        };
        let response_data: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let response_data2 = response_data.clone();
        http_client::request_streaming_with_cancel(
            &req,
            move |line| {
                let mut data = response_data2.lock().unwrap();
                data.push_str(line);
                data.push('\n');
                if let Some(json_str) = line.strip_prefix("data: ") {
                    if json_str == "[DONE]" {
                        return;
                    }
                    if let Ok(val) = json::parse(json_str) {
                        if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
                            if let Some(choice) = choices.first() {
                                if let Some(delta) = choice.get("delta") {
                                    if let Some(text) =
                                        delta.get("content").and_then(|v| v.as_str())
                                    {
                                        on_chunk(text, "text");
                                    }
                                }
                            }
                        }
                    }
                }
            },
            Some(cancel),
        )?;
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
            headers: vec![("Content-Type".into(), "application/json".into())],
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
    let mut body = format!("{{\"model\":\"{model}\",\"stream\":{stream}");
    body.push_str(",\"messages\":");
    body.push_str(&crate::llm::openai_compat::build_messages_json(
        messages, system,
    ));
    body.push_str(&crate::llm::openai_compat::build_tools_json(tools));
    body.push('}');
    crate::llm::openai_compat::validate_json_body(body)
}

fn parse_ollama_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    crate::llm::openai_compat::parse_streaming_response(raw)
}

fn parse_ollama_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    crate::llm::openai_compat::parse_complete_response(raw)
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
