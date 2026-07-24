use super::provider::*;
use crate::json;
use std::collections::HashMap;

/// Shared parsing/serialization for OpenAI-compatible chat-completions APIs
/// (openai, ollama, openrouter, and opengateway all speak this dialect).

pub fn escape_json_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub fn build_messages_json(messages: &[Message], system: &str) -> String {
    let mut body = String::from("[");
    let mut first = true;
    if !system.is_empty() {
        let escaped = escape_json_str(system);
        body.push_str(&format!(
            "{{\"role\":\"system\",\"content\":\"{escaped}\"}}"
        ));
        first = false;
    }
    let mut index = 0;
    while index < messages.len() {
        let msg = &messages[index];
        if !first {
            body.push(',');
        }
        first = false;
        match &msg.content {
            Content::Text(t) => {
                let escaped = escape_json_str(t);
                body.push_str(&format!(
                    "{{\"role\":\"{}\",\"content\":\"{escaped}\"}}",
                    msg.role
                ));
            }
            Content::ToolUse(_) => {
                // Providers return parallel calls as consecutive ToolUse
                // messages. OpenAI-compatible APIs require those calls in one
                // assistant message before any corresponding tool results.
                body.push_str("{\"role\":\"assistant\",\"content\":null,\"tool_calls\":[");
                let mut first_call = true;
                while index < messages.len() {
                    let Content::ToolUse(tu) = &messages[index].content else {
                        break;
                    };
                    if !first_call {
                        body.push(',');
                    }
                    first_call = false;
                    let id = escape_json_str(&tu.id);
                    let name = escape_json_str(&tu.name);
                    let args = escape_json_str(&tu.input);
                    body.push_str(&format!(
                        "{{\"id\":\"{id}\",\"type\":\"function\",\"function\":{{\"name\":\"{name}\",\"arguments\":\"{args}\"}}}}"
                    ));
                    index += 1;
                }
                body.push_str("]}");
                continue;
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
                    "{{\"role\":\"assistant\",\"content\":\"{escaped}\"}}"
                ));
            }
        }
        index += 1;
    }
    body.push(']');
    body
}

/// Returns an empty string when there are no tools, otherwise a
/// `,"tools":[...]` fragment ready to be appended to a request body.
pub fn build_tools_json(tools: &[ToolDefinition]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let mut body = String::from(",\"tools\":[");
    for (i, tool) in tools.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        let name_esc = escape_json_str(&tool.name);
        let desc_esc = escape_json_str(&tool.description);
        body.push_str(&format!(
            "{{\"type\":\"function\",\"function\":{{\"name\":\"{name_esc}\",\"description\":\"{desc_esc}\",\"parameters\":{}}}}}",
            tool.input_schema
        ));
    }
    body.push(']');
    body
}

/// Sanity-checks a hand-built request body and returns a diagnostic error
/// (with byte offset and surrounding context) if it isn't valid JSON.
pub fn validate_json_body(body: String) -> Result<String, String> {
    if let Err(e) = json::parse(&body) {
        let pos = e.pos;
        let start = pos.saturating_sub(20);
        let end = (pos + 20).min(body.len());
        let context = &body[start..end];
        let ch = body.as_bytes().get(pos).map(|&b| b as char).unwrap_or('?');
        return Err(format!("Invalid JSON body: {e}\nChar at pos {pos}: '{ch}' (0x{:02X})\nContext: ...{context}...", ch as u8));
    }
    Ok(body)
}

/// Incremental reducer for OpenAI-compatible SSE data lines. It retains only
/// state that becomes part of the returned messages/usage, rather than a
/// second copy of the complete wire transcript.
#[derive(Default)]
pub struct StreamingResponse {
    usage: Usage,
    collected: String,
    tool_calls: HashMap<u64, (String, String, String)>,
    saw_done: bool,
    lines_seen: usize,
    error: Option<String>,
}

impl StreamingResponse {
    pub fn push_line<F>(&mut self, line: &str, on_chunk: &mut F, emit_reasoning: bool)
    where
        F: FnMut(&str, &str),
    {
        self.lines_seen = self.lines_seen.saturating_add(1);
        if self.error.is_some() || self.saw_done {
            return;
        }
        if let Err(error) = self.process_line(line, on_chunk, emit_reasoning) {
            self.error = Some(error);
        }
    }

    fn process_line<F>(
        &mut self,
        line: &str,
        on_chunk: &mut F,
        emit_reasoning: bool,
    ) -> Result<(), String>
    where
        F: FnMut(&str, &str),
    {
        let Some(data) = line.strip_prefix("data: ") else {
            return Ok(());
        };
        if data == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }

        let val = json::parse(data).map_err(|error| {
            format!(
                "Malformed OpenAI-compatible stream event on line {}: {error}",
                self.lines_seen
            )
        })?;
        if val.as_object().is_none() {
            return Err(format!(
                "Malformed OpenAI-compatible stream event on line {}: expected a JSON object",
                self.lines_seen
            ));
        }
        if let Some(error) = val.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("provider returned an error event");
            return Err(format!("OpenAI-compatible API stream error: {message}"));
        }
        if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
            if let Some(choice) = choices.first() {
                if let Some(delta) = choice.get("delta") {
                    if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                        self.collected.push_str(text);
                        on_chunk(text, "text");
                    }
                    if emit_reasoning {
                        if let Some(text) = delta.get("reasoning_content").and_then(|v| v.as_str())
                        {
                            if !text.is_empty() {
                                on_chunk(text, "thinking");
                            }
                        }
                    }
                    if let Some(tc_arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                        for tc in tc_arr {
                            let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                            let entry = self
                                .tool_calls
                                .entry(idx)
                                .or_insert_with(|| (String::new(), String::new(), String::new()));
                            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                if !id.is_empty() {
                                    entry.0 = id.to_string();
                                }
                            }
                            if let Some(func) = tc.get("function") {
                                if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                    if !name.is_empty() {
                                        entry.1 = name.to_string();
                                    }
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
        if let Some(usage) = val.get("usage") {
            self.usage.input_tokens = usage
                .get("prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            self.usage.output_tokens = usage
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(Vec<Message>, Usage), String> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.saw_done {
            return Err(
                "Incomplete OpenAI-compatible stream: missing [DONE] completion marker".into(),
            );
        }

        let mut messages = Vec::new();
        if !self.collected.is_empty() {
            messages.push(Message {
                role: "assistant".into(),
                content: Content::Text(self.collected),
            });
        }
        if !self.tool_calls.is_empty() {
            let mut calls: Vec<_> = self.tool_calls.into_iter().collect();
            calls.sort_by_key(|(idx, _)| *idx);
            for (_, (id, name, args)) in calls {
                let tool_use = ToolUse {
                    id,
                    name,
                    input: args,
                };
                tool_use
                    .validate()
                    .map_err(|error| format!("Invalid OpenAI-compatible stream: {error}"))?;
                messages.push(Message {
                    role: "assistant".into(),
                    content: Content::ToolUse(tool_use),
                });
            }
        }
        Ok((messages, self.usage))
    }
}

pub fn parse_streaming_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut response = StreamingResponse::default();
    let mut ignore_chunk = |_: &str, _: &str| {};
    for line in raw.lines() {
        response.push_line(line, &mut ignore_chunk, false);
    }
    response.finish()
}

pub fn parse_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages = Vec::new();
    let mut usage = Usage::default();

    let val = json::parse(raw).map_err(|e| format!("Failed to parse response: {e}"))?;

    if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
        if let Some(choice) = choices.first() {
            if let Some(msg) = choice.get("message") {
                let role = msg
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("assistant")
                    .to_string();
                if let Some(text) = msg.get("content").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        messages.push(Message {
                            role: role.clone(),
                            content: Content::Text(text.to_string()),
                        });
                    }
                }
                if let Some(tc_arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tc_arr {
                        let tool_use = ToolUse {
                            id: tc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            name: tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input: tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        };
                        tool_use
                            .validate()
                            .map_err(|e| format!("Invalid OpenAI-compatible response: {e}"))?;
                        messages.push(Message {
                            role: role.clone(),
                            content: Content::ToolUse(tool_use),
                        });
                    }
                }
                if messages.is_empty() {
                    messages.push(Message {
                        role,
                        content: Content::Text(String::new()),
                    });
                }
            }
        }
    }
    if let Some(u) = val.get("usage") {
        usage.input_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        usage.output_tokens = u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }
    Ok((messages, usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_multiple_tool_calls_all_survive() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"glob\",\"arguments\":\"{}\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"grep\",\"arguments\":\"{}\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        let (msgs, _usage) = parse_streaming_response(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        match &msgs[0].content {
            Content::ToolUse(tu) => assert_eq!(tu.name, "glob"),
            _ => panic!("expected ToolUse"),
        }
        match &msgs[1].content {
            Content::ToolUse(tu) => assert_eq!(tu.name, "grep"),
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_streaming_text_and_tool_calls_both_survive() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"looking...\"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"glob\",\"arguments\":\"{}\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        let (msgs, _usage) = parse_streaming_response(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        match &msgs[0].content {
            Content::Text(t) => assert_eq!(t, "looking..."),
            _ => panic!("expected Text"),
        }
        match &msgs[1].content {
            Content::ToolUse(tu) => assert_eq!(tu.name, "glob"),
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_streaming_rejects_malformed_event() {
        let raw = "data: not-json\ndata: [DONE]\n";
        let error = parse_streaming_response(raw).unwrap_err();

        assert!(error.contains("Malformed OpenAI-compatible stream event"));
    }

    #[test]
    fn test_streaming_surfaces_provider_error_event() {
        let raw = "data: {\"error\":{\"message\":\"rate limited\"}}\n";
        let error = parse_streaming_response(raw).unwrap_err();

        assert!(error.contains("rate limited"), "{error}");
    }

    #[test]
    fn test_streaming_rejects_truncated_event_at_eof() {
        let raw = r#"data: {"choices":[{"delta":{"content":"partial"}}]
"#;
        let error = parse_streaming_response(raw).unwrap_err();

        assert!(error.contains("Malformed OpenAI-compatible stream event"));
    }

    #[test]
    fn test_streaming_requires_done_marker() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n";
        let error = parse_streaming_response(raw).unwrap_err();

        assert!(error.contains("missing [DONE] completion marker"));
    }

    #[test]
    fn test_streaming_rejects_tool_calls_missing_id_or_name() {
        let missing_id = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"glob\",\"arguments\":\"{}\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        let missing_name = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"arguments\":\"{}\"}}]}}]}\n",
            "data: [DONE]\n",
        );

        let id_error = parse_streaming_response(missing_id).unwrap_err();
        assert!(id_error.contains("missing an id"), "{id_error}");
        let name_error = parse_streaming_response(missing_name).unwrap_err();
        assert!(name_error.contains("missing a name"), "{name_error}");
    }

    #[test]
    fn test_streaming_rejects_tool_call_missing_arguments() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"glob\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        let error = parse_streaming_response(raw).unwrap_err();

        assert!(error.contains("invalid JSON arguments"), "{error}");
    }

    #[test]
    fn test_streaming_rejects_invalid_tool_arguments() {
        let raw = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"glob","arguments":"{\"pattern\":"}}]}}]}
data: [DONE]
"#;
        let error = parse_streaming_response(raw).unwrap_err();

        assert!(error.contains("invalid JSON arguments"), "{error}");
    }

    #[test]
    fn test_complete_response_multiple_tool_calls_all_survive() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[
            {"id":"call_1","function":{"name":"glob","arguments":"{\"pattern\":\"*.rs\"}"}},
            {"id":"call_2","function":{"name":"grep","arguments":"{\"pattern\":\"foo\"}"}}
        ]}}]}"#;
        let (msgs, _usage) = parse_complete_response(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        match &msgs[0].content {
            Content::ToolUse(tu) => assert_eq!(tu.id, "call_1"),
            _ => panic!("expected ToolUse"),
        }
        match &msgs[1].content {
            Content::ToolUse(tu) => assert_eq!(tu.id, "call_2"),
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_complete_response_rejects_invalid_tool_call() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[
            {"id":"call_1","function":{"name":"glob","arguments":"not-json"}}
        ]}}]}"#;
        let error = parse_complete_response(raw).unwrap_err();

        assert!(error.contains("invalid JSON arguments"), "{error}");
    }

    #[test]
    fn test_build_messages_json_tool_result_is_role_tool_not_content_block() {
        let msgs = vec![Message {
            role: "user".into(),
            content: Content::ToolResult(ToolResult {
                tool_use_id: "call_1".into(),
                content: "line1\nline2".into(),
            }),
        }];
        let body = build_messages_json(&msgs, "");
        let v = crate::json::parse(&body).unwrap();
        let arr = v.as_array().unwrap();
        let obj = arr[0].as_object().unwrap();
        assert_eq!(obj.get("role").and_then(|v| v.as_str()), Some("tool"));
        assert_eq!(
            obj.get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(
            obj.get("content").and_then(|v| v.as_str()),
            Some("line1\nline2")
        );
    }

    #[test]
    fn test_build_messages_json_tool_use_has_tool_calls_field() {
        let msgs = vec![Message {
            role: "assistant".into(),
            content: Content::ToolUse(ToolUse {
                id: "call_1".into(),
                name: "glob".into(),
                input: "{\"pattern\":\"*.rs\"}".into(),
            }),
        }];
        let body = build_messages_json(&msgs, "");
        let v = crate::json::parse(&body).unwrap();
        let arr = v.as_array().unwrap();
        let obj = arr[0].as_object().unwrap();
        assert_eq!(obj.get("role").and_then(|v| v.as_str()), Some("assistant"));
        let tool_calls = obj.get("tool_calls").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tool_calls.len(), 1);
    }

    #[test]
    fn test_build_messages_json_coalesces_parallel_tool_calls() {
        let msgs = vec![
            Message {
                role: "assistant".into(),
                content: Content::ToolUse(ToolUse {
                    id: "call_1".into(),
                    name: "glob".into(),
                    input: "{}".into(),
                }),
            },
            Message {
                role: "assistant".into(),
                content: Content::ToolUse(ToolUse {
                    id: "call_2".into(),
                    name: "grep".into(),
                    input: "{}".into(),
                }),
            },
            Message {
                role: "user".into(),
                content: Content::ToolResult(ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "a.rs".into(),
                }),
            },
            Message {
                role: "user".into(),
                content: Content::ToolResult(ToolResult {
                    tool_use_id: "call_2".into(),
                    content: "match".into(),
                }),
            },
        ];

        let parsed = crate::json::parse(&build_messages_json(&msgs, "")).unwrap();
        let messages = parsed.as_array().unwrap();
        assert_eq!(
            messages.len(),
            3,
            "one assistant turn plus two tool results"
        );
        let calls = messages[0]
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            messages[1].get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(
            messages[2].get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_2")
        );
    }

    #[test]
    fn test_escape_handles_newlines() {
        let escaped = escape_json_str("line1\nline2\ttabbed\r");
        assert_eq!(escaped, "line1\\nline2\\ttabbed\\r");
    }
}
