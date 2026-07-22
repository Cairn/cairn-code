//! Anthropic (Claude) provider.
//!
//! Model picker prefers a live `GET /v1/models` catalog when an API key is
//! available (5-minute cache), falling back to a curated list.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::provider::*;
use crate::http_client;
use crate::json;

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const MODELS_URL: &str = "https://api.anthropic.com/v1/models";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-5";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

static MODELS_CACHE: OnceLock<Mutex<Option<(Instant, Vec<ModelInfo>)>>> = OnceLock::new();

pub struct AnthropicProvider {
    api_key: String,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        AnthropicProvider {
            api_key: String::new(),
        }
    }

    fn get_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        crate::config::config_get_api_key("anthropic").unwrap_or_default()
    }
}

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }
    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        let key = self.get_key();
        if key.is_empty() {
            return curated_models();
        }
        if let Some(cached) = cached_models() {
            return cached;
        }
        match fetch_remote_models(&key) {
            Ok(models) if !models.is_empty() => {
                store_cache(models.clone());
                models
            }
            _ => curated_models(),
        }
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
            return Err(missing_api_key("ANTHROPIC_API_KEY"));
        }
        let body = build_request_body(messages, tools, system, model, max_tokens, true)?;
        let req = http_client::HttpRequest {
            url: MESSAGES_URL.into(),
            headers: vec![
                ("x-api-key".into(), key),
                ("anthropic-version".into(), API_VERSION.into()),
                ("content-type".into(), "application/json".into()),
            ],
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
                        if let Some(obj) = val.as_object() {
                            match obj.get("type").and_then(|v| v.as_str()) {
                                Some("content_block_delta") => {
                                    if let Some(delta) = obj.get("delta") {
                                        if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                            on_chunk(text, "text");
                                        }
                                        if let Some(text) =
                                            delta.get("thinking").and_then(|v| v.as_str())
                                        {
                                            on_chunk(text, "thinking");
                                        }
                                    }
                                }
                                Some("content_block_start") => {
                                    if let Some(block) = obj.get("content_block") {
                                        if let Some(text) =
                                            block.get("text").and_then(|v| v.as_str())
                                        {
                                            on_chunk(text, "text");
                                        }
                                        if let Some(text) =
                                            block.get("thinking").and_then(|v| v.as_str())
                                        {
                                            on_chunk(text, "thinking");
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            },
            Some(cancel),
        )?;
        let raw = response_data.lock().unwrap().clone();
        parse_anthropic_response(&raw)
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
            return Err(missing_api_key("ANTHROPIC_API_KEY"));
        }
        let body = build_request_body(messages, tools, system, model, max_tokens, false)?;
        let req = http_client::HttpRequest {
            url: MESSAGES_URL.into(),
            headers: vec![
                ("x-api-key".into(), key),
                ("anthropic-version".into(), API_VERSION.into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: Some(body),
        };
        let resp = http_client::request(&req)?;
        parse_anthropic_response(&resp.body)
    }
}

fn curated_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-sonnet-5".into(),
            name: "Claude Sonnet 5".into(),
            max_ctx: 200_000,
        },
        ModelInfo {
            id: "claude-opus-4-8".into(),
            name: "Claude Opus 4.8".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "claude-haiku-4-5".into(),
            name: "Claude Haiku 4.5".into(),
            max_ctx: 200_000,
        },
        ModelInfo {
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "claude-opus-4-7".into(),
            name: "Claude Opus 4.7".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "claude-sonnet-4-20250514".into(),
            name: "Claude Sonnet 4".into(),
            max_ctx: 200_000,
        },
        ModelInfo {
            id: "claude-opus-4-20250514".into(),
            name: "Claude Opus 4".into(),
            max_ctx: 200_000,
        },
    ]
}

fn models_cache() -> &'static Mutex<Option<(Instant, Vec<ModelInfo>)>> {
    MODELS_CACHE.get_or_init(|| Mutex::new(None))
}

fn cached_models() -> Option<Vec<ModelInfo>> {
    let guard = models_cache().lock().ok()?;
    let (at, models) = guard.as_ref()?;
    if at.elapsed() > CACHE_TTL {
        return None;
    }
    Some(models.clone())
}

fn store_cache(models: Vec<ModelInfo>) {
    if let Ok(mut guard) = models_cache().lock() {
        *guard = Some((Instant::now(), models));
    }
}

fn anthropic_headers(api_key: &str) -> Vec<(String, String)> {
    vec![
        ("x-api-key".into(), api_key.to_string()),
        ("anthropic-version".into(), API_VERSION.into()),
        ("Accept".into(), "application/json".into()),
    ]
}

fn fetch_remote_models(api_key: &str) -> Result<Vec<ModelInfo>, String> {
    let headers = anthropic_headers(api_key);
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    // Cap pages so a weird API response cannot hang the picker.
    for _ in 0..20 {
        let url = match &after {
            Some(id) => format!("{MODELS_URL}?limit=100&after_id={id}"),
            None => format!("{MODELS_URL}?limit=100"),
        };
        let resp = http_client::request_get(&url, &headers)?;
        let page = parse_models_page(&resp.body)?;
        out.extend(page.models);
        if !page.has_more {
            break;
        }
        let Some(last) = page.last_id else {
            break;
        };
        if last.is_empty() {
            break;
        }
        after = Some(last);
    }
    if out.is_empty() {
        return Err("models list empty".into());
    }
    // API already returns newest first; keep that order.
    Ok(out)
}

struct ModelsPage {
    models: Vec<ModelInfo>,
    has_more: bool,
    last_id: Option<String>,
}

fn parse_models_page(body: &str) -> Result<ModelsPage, String> {
    let val = json::parse(body).map_err(|e| format!("models list: {e}"))?;
    let arr = val
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "models list: missing data array".to_string())?;
    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in arr {
        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || !seen.insert(id.to_ascii_lowercase()) {
            continue;
        }
        // Skip non-chat / non-Claude entries if any appear.
        let lower = id.to_ascii_lowercase();
        if !lower.starts_with("claude") {
            continue;
        }
        let name = item
            .get("display_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| display_name_fallback(id));
        let max_ctx = item
            .get("max_input_tokens")
            .and_then(|v| v.as_u64())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| context_for_id(id));
        models.push(ModelInfo {
            id: id.to_string(),
            name,
            max_ctx,
        });
    }
    let has_more = val.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false);
    let last_id = val
        .get("last_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| models.last().map(|m| m.id.clone()));
    Ok(ModelsPage {
        models,
        has_more,
        last_id,
    })
}

fn display_name_fallback(id: &str) -> String {
    // claude-opus-4-8 → Claude Opus 4.8 (best-effort)
    let rest = id
        .strip_prefix("claude-")
        .unwrap_or(id)
        .replace('-', " ");
    let mut out = String::from("Claude");
    for word in rest.split_whitespace() {
        out.push(' ');
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            out.push(c.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn context_for_id(id: &str) -> u64 {
    let lower = id.to_ascii_lowercase();
    // Newer Opus / Sonnet 4.6+ often ship with 1M context; fall back to 200k.
    if lower.contains("opus-4-") || lower.contains("sonnet-4-6") || lower.contains("sonnet-5") {
        1_000_000
    } else if lower.contains("haiku") {
        200_000
    } else {
        200_000
    }
}

fn build_request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    max_tokens: usize,
    stream: bool,
) -> Result<String, String> {
    let mut body = String::new();
    body.push_str(&format!(
        "{{\"model\":\"{model}\",\"max_tokens\":{max_tokens},\"stream\":{stream}"
    ));
    if !system.is_empty() {
        let escaped = system
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        body.push_str(&format!(",\"system\":\"{escaped}\""));
    }
    body.push_str(",\"messages\":[");
    for (i, msg) in messages.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
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
            if i > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                "{{\"name\":\"{}\",\"description\":\"{}\",\"input_schema\":{}}}",
                tool.name, tool.description, tool.input_schema
            ));
        }
        body.push(']');
    }
    body.push('}');
    Ok(body)
}

fn parse_anthropic_response(raw: &str) -> Result<(Vec<Message>, Usage), String> {
    let mut messages: Vec<Message> = Vec::new();
    let mut usage = Usage::default();
    let mut current_tool_use: Option<ToolUse> = None;
    let mut tool_input_accum = String::new();
    let mut text_accum = String::new();
    for line in raw.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            if let Ok(val) = json::parse(data) {
                if let Some(obj) = val.as_object() {
                    match obj.get("type").and_then(|v| v.as_str()) {
                        Some("content_block_start") => {
                            if let Some(block) = obj.get("content_block") {
                                match block.get("type").and_then(|v| v.as_str()) {
                                    Some("text") => {
                                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                            text_accum = t.to_string();
                                        }
                                    }
                                    Some("tool_use") => {
                                        if !text_accum.is_empty() {
                                            messages.push(Message {
                                                role: "assistant".into(),
                                                content: Content::Text(text_accum.clone()),
                                            });
                                            text_accum.clear();
                                        }
                                        tool_input_accum.clear();
                                        current_tool_use = Some(ToolUse {
                                            name: block
                                                .get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            id: block
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            // In real streaming responses content_block_start's `input` is
                                            // always `{}` — the actual arguments arrive as input_json_delta
                                            // fragments below and get assembled at content_block_stop.
                                            input: block
                                                .get("input")
                                                .map(|v| json::serialize(v))
                                                .unwrap_or_default(),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some("content_block_delta") => {
                            if let Some(delta) = obj.get("delta") {
                                if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                                    text_accum.push_str(t);
                                }
                                if delta.get("type").and_then(|v| v.as_str())
                                    == Some("input_json_delta")
                                {
                                    if let Some(partial) =
                                        delta.get("partial_json").and_then(|v| v.as_str())
                                    {
                                        tool_input_accum.push_str(partial);
                                    }
                                }
                            }
                        }
                        Some("content_block_stop") => {
                            if let Some(mut tu) = current_tool_use.take() {
                                if !tool_input_accum.is_empty() {
                                    tu.input = tool_input_accum.clone();
                                } else if tu.input.is_empty() {
                                    tu.input = "{}".to_string();
                                }
                                tool_input_accum.clear();
                                messages.push(Message {
                                    role: "assistant".into(),
                                    content: Content::ToolUse(tu),
                                });
                            }
                        }
                        Some("message_start") | Some("message_delta") => {
                            if let Some(u) = obj.get("usage") {
                                usage.input_tokens +=
                                    u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                usage.output_tokens +=
                                    u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if !text_accum.is_empty() {
        messages.push(Message {
            role: "assistant".into(),
            content: Content::Text(text_accum),
        });
    }
    Ok((messages, usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity_and_fallback_catalog() {
        let p = AnthropicProvider::new();
        assert_eq!(p.name(), "anthropic");
        assert_eq!(p.default_model(), "claude-sonnet-5");
        let models = p.available_models();
        assert!(models.iter().any(|m| m.id.contains("sonnet")));
        assert!(models.len() >= 3);
    }

    #[test]
    fn parse_models_page_basic() {
        let body = r#"{
            "data": [
                {"id":"claude-opus-4-8","display_name":"Claude Opus 4.8","max_input_tokens":1000000,"type":"model"},
                {"id":"claude-sonnet-5","display_name":"Claude Sonnet 5","max_input_tokens":200000,"type":"model"},
                {"id":"not-claude","display_name":"Skip","type":"model"}
            ],
            "has_more": false,
            "last_id": "claude-sonnet-5"
        }"#;
        let page = parse_models_page(body).unwrap();
        assert_eq!(page.models.len(), 2);
        assert_eq!(page.models[0].id, "claude-opus-4-8");
        assert_eq!(page.models[0].name, "Claude Opus 4.8");
        assert_eq!(page.models[0].max_ctx, 1_000_000);
        assert_eq!(page.models[1].id, "claude-sonnet-5");
        assert!(!page.has_more);
    }

    #[test]
    fn parse_models_page_missing_max_uses_fallback() {
        let body = r#"{"data":[{"id":"claude-haiku-4-5","display_name":"Haiku","type":"model"}],"has_more":false}"#;
        let page = parse_models_page(body).unwrap();
        assert_eq!(page.models[0].max_ctx, 200_000);
    }

    #[test]
    fn test_streaming_tool_use_reconstructs_args_from_input_json_delta() {
        // Mirrors what Anthropic actually sends: content_block_start's `input`
        // is `{}`, then the real arguments arrive as input_json_delta fragments.
        let raw = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"glob\",\"input\":{}}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pattern\\\":\"}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"*.rs\\\"}\"}}\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
        );
        let (msgs, _usage) = parse_anthropic_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::ToolUse(tu) => {
                assert_eq!(tu.id, "toolu_1");
                assert_eq!(tu.name, "glob");
                assert_eq!(tu.input, r#"{"pattern":"*.rs"}"#);
            }
            _ => panic!("expected ToolUse content"),
        }
    }

    #[test]
    fn test_streaming_multiple_tool_uses_each_get_own_args() {
        let raw = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"glob\",\"input\":{}}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_2\",\"name\":\"grep\",\"input\":{}}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pattern\\\":\\\"foo\\\"}\"}}\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n",
        );
        let (msgs, _usage) = parse_anthropic_response(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        match &msgs[0].content {
            Content::ToolUse(tu) => assert_eq!(tu.input, "{}"),
            _ => panic!("expected ToolUse content"),
        }
        match &msgs[1].content {
            Content::ToolUse(tu) => assert_eq!(tu.input, r#"{"pattern":"foo"}"#),
            _ => panic!("expected ToolUse content"),
        }
    }

    #[test]
    fn test_non_streaming_tool_use_still_uses_content_block_input() {
        // Non-streaming responses put the full input directly on content_block_start
        // with no deltas at all — must still work.
        let raw = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"glob\",\"input\":{\"pattern\":\"*.rs\"}}}\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n";
        let (msgs, _usage) = parse_anthropic_response(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            Content::ToolUse(tu) => assert_eq!(tu.input, r#"{"pattern":"*.rs"}"#),
            _ => panic!("expected ToolUse content"),
        }
    }
}
