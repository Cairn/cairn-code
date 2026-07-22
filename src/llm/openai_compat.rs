use std::collections::HashMap;
use super::provider::*;
use crate::json;

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
        body.push_str(&format!("{{\"role\":\"system\",\"content\":\"{escaped}\"}}"));
        first = false;
    }
    for msg in messages.iter() {
        if !first { body.push(','); }
        first = false;
        match &msg.content {
            Content::Text(t) => {
                let escaped = escape_json_str(t);
                body.push_str(&format!("{{\"role\":\"{}\",\"content\":\"{escaped}\"}}", msg.role));
            }
            Content::ToolUse(tu) => {
                let args_escaped = escape_json_str(&tu.input);
                body.push_str(&format!(
                    "{{\"role\":\"assistant\",\"content\":null,\"tool_calls\":[{{\"id\":\"{}\",\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"arguments\":\"{}\"}}}}]}}",
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
                body.push_str(&format!("{{\"role\":\"assistant\",\"content\":\"{escaped}\"}}"));
            }
        }
    }
    body.push(']');
    body
}

/// Returns an empty string when there are no tools, otherwise a
/// `,"tools":[...]` fragment ready to be appended to a request body.
pub fn build_tools_json(tools: &[ToolDefinition]) -> String {
    if tools.is_empty() { return String::new(); }
    let mut body = String::from(",\"tools\":[");
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

pub fn parse_streaming_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages = Vec::new();
    let mut usage = Usage::default();
    let mut collected = String::new();
    let mut tool_calls: HashMap<u64, (String, String, String)> = HashMap::new();
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
    if !collected.is_empty() {
        messages.push(Message { role: "assistant".into(), content: Content::Text(collected) });
    }
    if !tool_calls.is_empty() {
        let mut calls: Vec<_> = tool_calls.into_iter().collect();
        calls.sort_by_key(|(idx, _)| *idx);
        for (_, (id, name, args)) in calls {
            let input = if args.is_empty() { "{}".to_string() } else { args };
            messages.push(Message { role: "assistant".into(), content: Content::ToolUse(ToolUse { id, name, input }) });
        }
    }
    Ok((messages, usage))
}

pub fn parse_complete_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages = Vec::new();
    let mut usage = Usage::default();

    let val = json::parse(raw).map_err(|e| format!("Failed to parse response: {e}"))?;

    if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
        if let Some(choice) = choices.first() {
            if let Some(msg) = choice.get("message") {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("assistant").to_string();
                if let Some(text) = msg.get("content").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        messages.push(Message { role: role.clone(), content: Content::Text(text.to_string()) });
                    }
                }
                if let Some(tc_arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tc_arr {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let args = tc.get("function").and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("{}").to_string();
                        messages.push(Message { role: role.clone(), content: Content::ToolUse(ToolUse { id, name, input: args }) });
                    }
                }
                if messages.is_empty() {
                    messages.push(Message { role, content: Content::Text(String::new()) });
                }
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
    fn test_build_messages_json_tool_result_is_role_tool_not_content_block() {
        let msgs = vec![Message {
            role: "user".into(),
            content: Content::ToolResult(ToolResult { tool_use_id: "call_1".into(), content: "line1\nline2".into() }),
        }];
        let body = build_messages_json(&msgs, "");
        let v = crate::json::parse(&body).unwrap();
        let arr = v.as_array().unwrap();
        let obj = arr[0].as_object().unwrap();
        assert_eq!(obj.get("role").and_then(|v| v.as_str()), Some("tool"));
        assert_eq!(obj.get("tool_call_id").and_then(|v| v.as_str()), Some("call_1"));
        assert_eq!(obj.get("content").and_then(|v| v.as_str()), Some("line1\nline2"));
    }

    #[test]
    fn test_build_messages_json_tool_use_has_tool_calls_field() {
        let msgs = vec![Message {
            role: "assistant".into(),
            content: Content::ToolUse(ToolUse { id: "call_1".into(), name: "glob".into(), input: "{\"pattern\":\"*.rs\"}".into() }),
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
    fn test_escape_handles_newlines() {
        let escaped = escape_json_str("line1\nline2\ttabbed\r");
        assert_eq!(escaped, "line1\\nline2\\ttabbed\\r");
    }
}
