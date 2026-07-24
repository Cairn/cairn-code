use super::provider::*;
use crate::http_client;

pub struct OpenRouterProvider {
    api_key: String,
}

impl OpenRouterProvider {
    pub fn new() -> Self {
        OpenRouterProvider {
            api_key: String::new(),
        }
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
    }

    fn get_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        if let Ok(k) = std::env::var("OPENROUTER_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        crate::config::config_get_api_key("openrouter").unwrap_or_default()
    }
}

impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }
    fn default_model(&self) -> &str {
        "openai/gpt-4o"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "openai/gpt-4o".into(),
                name: "GPT-4o".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "openai/gpt-4o-mini".into(),
                name: "GPT-4o Mini".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "openai/gpt-5-mini".into(),
                name: "GPT-5 Mini".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "openai/o3-mini".into(),
                name: "o3 Mini".into(),
                max_ctx: 200_000,
            },
            ModelInfo {
                id: "openai/o4-mini".into(),
                name: "o4 Mini".into(),
                max_ctx: 200_000,
            },
            ModelInfo {
                id: "anthropic/claude-sonnet-4-20250514".into(),
                name: "Claude Sonnet 4".into(),
                max_ctx: 200_000,
            },
            ModelInfo {
                id: "google/gemini-2.5-pro".into(),
                name: "Gemini 2.5 Pro".into(),
                max_ctx: 1_000_000,
            },
            ModelInfo {
                id: "google/gemini-2.5-flash".into(),
                name: "Gemini 2.5 Flash".into(),
                max_ctx: 1_000_000,
            },
            ModelInfo {
                id: "deepseek/deepseek-chat-v3-0324".into(),
                name: "DeepSeek V3".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "deepseek/deepseek-r1".into(),
                name: "DeepSeek R1".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "meta-llama/llama-4-scout".into(),
                name: "Llama 4 Scout".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "mistralai/mistral-large-24".into(),
                name: "Mistral Large".into(),
                max_ctx: 128_000,
            },
            ModelInfo {
                id: "qwen/qwq-32b".into(),
                name: "QwQ 32B".into(),
                max_ctx: 32_000,
            },
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
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(Vec<Message>, Usage), String> {
        let key = self.get_key();
        if key.is_empty() {
            return Err(crate::llm::provider::missing_api_key("OPENROUTER_API_KEY"));
        }
        let body = openrouter_request_body(messages, tools, system, model, true, max_tokens)?;
        do_stream_request(&key, body, &mut on_chunk, cancel)
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
        if key.is_empty() {
            return Err(crate::llm::provider::missing_api_key("OPENROUTER_API_KEY"));
        }
        let body = openrouter_request_body(messages, tools, system, model, false, max_tokens)?;
        do_complete_request(&key, body)
    }
}

fn do_stream_request(
    key: &str,
    body: String,
    on_chunk: &mut StreamingCallback,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(Vec<Message>, Usage), String> {
    let req = http_client::HttpRequest {
        url: "https://openrouter.ai/api/v1/chat/completions".into(),
        headers: vec![
            ("Authorization".into(), format!("Bearer {key}")),
            ("Content-Type".into(), "application/json".into()),
            (
                "HTTP-Referer".into(),
                "https://github.com/Cairn/cairn-code".into(),
            ),
            ("X-Title".into(), "Cairn Code".into()),
        ],
        body: Some(body),
    };
    let mut response = crate::llm::openai_compat::StreamingResponse::default();
    http_client::request_streaming_with_cancel(
        &req,
        |line| response.push_line(line, on_chunk, false),
        Some(cancel),
    )?;
    response.finish()
}

fn do_complete_request(key: &str, body: String) -> Result<(Vec<Message>, Usage), String> {
    let req = http_client::HttpRequest {
        url: "https://openrouter.ai/api/v1/chat/completions".into(),
        headers: vec![
            ("Authorization".into(), format!("Bearer {key}")),
            ("Content-Type".into(), "application/json".into()),
            (
                "HTTP-Referer".into(),
                "https://github.com/Cairn/cairn-code".into(),
            ),
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
    let model = crate::llm::openai_compat::escape_json_str(model);
    let mut body = format!("{{\"model\":\"{model}\",\"stream\":{stream}");
    body.push_str(&format!(",\"max_completion_tokens\":{max_tokens}"));
    body.push_str(",\"messages\":");
    body.push_str(&crate::llm::openai_compat::build_messages_json(
        messages, system,
    ));
    body.push_str(&crate::llm::openai_compat::build_tools_json(tools));
    body.push('}');
    crate::llm::openai_compat::validate_json_body(body)
}

fn parse_openrouter_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    crate::llm::openai_compat::parse_complete_response(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::default_registry;

    #[test]
    fn request_body_escapes_model_name() {
        let model = "model\",\"injected\":true,\"tail";
        let body = openrouter_request_body(&[], &[], "", model, false, 100).unwrap();
        let value = crate::json::parse(&body).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.get("model").and_then(|v| v.as_str()), Some(model));
        assert!(obj.get("injected").is_none());
    }

    #[test]
    fn test_all_tool_schemas_are_valid_json() {
        let reg = default_registry();
        for def in reg.definitions() {
            match crate::json::parse(&def.input_schema) {
                Ok(v) => {
                    let obj = v
                        .as_object()
                        .expect(&format!("{}: schema should be object", def.name));
                    assert!(
                        obj.get("type").is_some(),
                        "{}: schema missing 'type'",
                        def.name
                    );
                    assert!(
                        obj.get("properties").is_some(),
                        "{}: schema missing 'properties'",
                        def.name
                    );
                }
                Err(e) => panic!("{}: invalid schema JSON: {e}", def.name),
            }
        }
    }

    #[test]
    fn test_full_request_body_with_all_tools() {
        let reg = default_registry();
        let tools: Vec<ToolDefinition> = reg.definitions();
        let msgs = vec![Message {
            role: "user".into(),
            content: Content::Text("hello".into()),
        }];
        let body = openrouter_request_body(
            &msgs,
            &tools,
            "You are helpful.",
            "openai/gpt-4o",
            true,
            12_345,
        )
        .unwrap();
        match crate::json::parse(&body) {
            Ok(v) => {
                let obj = v.as_object().unwrap();
                assert_eq!(
                    obj.get("model").and_then(|v| v.as_str()),
                    Some("openai/gpt-4o")
                );
                assert_eq!(
                    obj.get("max_completion_tokens").and_then(|v| v.as_u64()),
                    Some(12_345)
                );
                assert!(obj.get("messages").and_then(|v| v.as_array()).is_some());
                let tools_arr = obj.get("tools").and_then(|v| v.as_array()).unwrap();
                assert_eq!(
                    tools_arr.len(),
                    crate::tools::registry::default_registry()
                        .definitions()
                        .len()
                );
                for (i, tool_val) in tools_arr.iter().enumerate() {
                    let tool_obj = tool_val.as_object().unwrap();
                    assert_eq!(
                        tool_obj.get("type").and_then(|v| v.as_str()),
                        Some("function")
                    );
                    let func = tool_obj.get("function").unwrap().as_object().unwrap();
                    assert!(func.get("name").is_some(), "tool {i} missing name");
                    assert!(
                        func.get("parameters").is_some(),
                        "tool {i} missing parameters"
                    );
                }
            }
            Err(e) => panic!("Full request body invalid: {e}"),
        }
    }
}
