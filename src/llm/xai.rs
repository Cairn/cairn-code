//! xAI (Grok) provider — OpenAI-compatible API at api.x.ai/v1.
//! Credentials: `XAI_API_KEY` / keyring, or OAuth access token from `/auth login xai`.
//!
//! Model picker prefers a live `GET /v1/models` catalog when credentials exist,
//! falling back to a curated list. Models that support reasoning effort are
//! listed as `id:effort` (e.g. `grok-4.5:high`) so effort is pickable and saved.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::provider::*;
use crate::http_client;
use crate::llm::openai_compat;

const CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
const MODELS_URL: &str = "https://api.x.ai/v1/models";
const DEFAULT_MODEL: &str = "grok-4.5:high";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

static MODELS_CACHE: OnceLock<Mutex<Option<(Instant, Vec<ModelInfo>)>>> = OnceLock::new();

pub struct XaiProvider {
    api_key: String,
}

impl XaiProvider {
    pub fn new() -> Self {
        XaiProvider {
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
        if let Ok(k) = std::env::var("XAI_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        if let Some(k) = crate::config::config_get_api_key("xai") {
            return k;
        }
        if let Some(tok) = crate::oauth::access_token("xai") {
            return tok;
        }
        String::new()
    }
}

impl Provider for XaiProvider {
    fn name(&self) -> &str {
        "xai"
    }
    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        let key = self.get_key();
        if key.is_empty() {
            return expand_reasoning_rows(curated_models());
        }
        if let Some(cached) = cached_models() {
            return cached;
        }
        match fetch_remote_models(&key) {
            Ok(ids) if !ids.is_empty() => {
                let models = expand_reasoning_rows(ids_to_model_info(&ids));
                store_cache(models.clone());
                models
            }
            _ => expand_reasoning_rows(curated_models()),
        }
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
        let key = self.get_key();
        if key.is_empty() {
            return Err(
                "xAI credentials missing. Run `/auth login xai` (OAuth device code) or set XAI_API_KEY / paste a key via /provider."
                    .into(),
            );
        }
        let body = request_body(messages, tools, system, model, true)?;
        let req = http_client::HttpRequest {
            url: CHAT_URL.into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {key}")),
                ("Content-Type".into(), "application/json".into()),
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
                    if let Ok(val) = crate::json::parse(json_str) {
                        if let Some(choices) = val.get("choices").and_then(|v| v.as_array()) {
                            if let Some(choice) = choices.first() {
                                if let Some(delta) = choice.get("delta") {
                                    if let Some(text) =
                                        delta.get("content").and_then(|v| v.as_str())
                                    {
                                        on_chunk(text, "text");
                                    }
                                    // Optional reasoning summary stream (when present)
                                    if let Some(text) =
                                        delta.get("reasoning_content").and_then(|v| v.as_str())
                                    {
                                        if !text.is_empty() {
                                            on_chunk(text, "thinking");
                                        }
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
        openai_compat::parse_streaming_response(&raw)
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
        if key.is_empty() {
            return Err(
                "xAI credentials missing. Run `/auth login xai` (OAuth device code) or set XAI_API_KEY / paste a key via /provider."
                    .into(),
            );
        }
        let body = request_body(messages, tools, system, model, false)?;
        let req = http_client::HttpRequest {
            url: CHAT_URL.into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {key}")),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Some(body),
        };
        let resp = http_client::request(&req)?;
        openai_compat::parse_complete_response(&resp.body)
    }
}

/// Split picker id `grok-4.5:high` → (`grok-4.5`, Some(`high`)).
pub fn parse_model_spec(model: &str) -> (String, Option<String>) {
    let model = model.trim();
    if let Some((base, effort)) = model.rsplit_once(':') {
        let effort = effort.trim().to_ascii_lowercase();
        if is_effort_token(&effort) && !base.is_empty() {
            return (base.to_string(), Some(effort));
        }
    }
    (model.to_string(), None)
}

fn is_effort_token(s: &str) -> bool {
    matches!(s, "low" | "medium" | "high" | "xhigh" | "none")
}

fn request_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    stream: bool,
) -> Result<String, String> {
    let (base_model, effort) = parse_model_spec(model);
    let model_esc = openai_compat::escape_json_str(&base_model);
    let mut body = format!("{{\"model\":\"{model_esc}\",\"stream\":{stream}");
    if let Some(effort) = effort {
        // Chat Completions: top-level reasoning_effort (xAI / OpenAI-style).
        let e = openai_compat::escape_json_str(&effort);
        body.push_str(&format!(",\"reasoning_effort\":\"{e}\""));
    } else if model_supports_effort(&base_model) {
        // API default for grok-4.5 is high; still send it so behavior is explicit.
        body.push_str(",\"reasoning_effort\":\"high\"");
    }
    body.push_str(",\"messages\":");
    body.push_str(&openai_compat::build_messages_json(messages, system));
    body.push_str(&openai_compat::build_tools_json(tools));
    body.push('}');
    openai_compat::validate_json_body(body)
}

fn curated_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "grok-4.5".into(),
            name: "Grok 4.5".into(),
            max_ctx: 500_000,
        },
        ModelInfo {
            id: "grok-4.3".into(),
            name: "Grok 4.3".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "grok-4.20-0309-reasoning".into(),
            name: "Grok 4.20 Reasoning".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "grok-4.20-0309-non-reasoning".into(),
            name: "Grok 4.20 Non-reasoning".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "grok-4.20-multi-agent-0309".into(),
            name: "Grok 4.20 Multi-agent".into(),
            max_ctx: 1_000_000,
        },
        ModelInfo {
            id: "grok-build-0.1".into(),
            name: "Grok Build 0.1".into(),
            max_ctx: 256_000,
        },
        ModelInfo {
            id: "grok-4".into(),
            name: "Grok 4".into(),
            max_ctx: 256_000,
        },
        ModelInfo {
            id: "grok-code-fast-1".into(),
            name: "Grok Code Fast".into(),
            max_ctx: 256_000,
        },
        ModelInfo {
            id: "grok-3".into(),
            name: "Grok 3".into(),
            max_ctx: 131_072,
        },
        ModelInfo {
            id: "grok-3-mini".into(),
            name: "Grok 3 Mini".into(),
            max_ctx: 131_072,
        },
    ]
}

fn context_for_id(id: &str) -> u64 {
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("grok-4.5") {
        500_000
    } else if lower.starts_with("grok-4.3")
        || lower.starts_with("grok-4.20")
        || lower.contains("4.20")
    {
        1_000_000
    } else if lower.starts_with("grok-build") {
        256_000
    } else if lower.starts_with("grok-4") || lower.contains("code-fast") {
        256_000
    } else if lower.starts_with("grok-3") {
        131_072
    } else {
        131_072
    }
}

fn display_name_for_id(id: &str) -> String {
    // Prefer curated pretty names when we know them.
    for m in curated_models() {
        if m.id.eq_ignore_ascii_case(id) {
            return m.name;
        }
    }
    // grok-4.5-preview → Grok 4.5 Preview
    let mut parts = Vec::new();
    for part in id.split(|c: char| c == '-' || c == '_') {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        let head = chars
            .next()
            .map(|c| c.to_ascii_uppercase())
            .into_iter()
            .collect::<String>();
        parts.push(format!("{head}{}", chars.as_str()));
    }
    if parts.is_empty() {
        id.to_string()
    } else {
        parts.join(" ")
    }
}

fn is_chat_model_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    if !lower.starts_with("grok") {
        return false;
    }
    let skip = [
        "imagine",
        "video",
        "image",
        "voice",
        "tts",
        "stt",
        "embedding",
        "realtime",
        "speech",
        "audio",
    ];
    !skip.iter().any(|s| lower.contains(s))
}

fn model_supports_effort(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.starts_with("grok-4.5") || lower.contains("multi-agent")
}

fn efforts_for(id: &str) -> Option<&'static [&'static str]> {
    let lower = id.to_ascii_lowercase();
    // Strongest effort first: high → medium → low (xhigh above high for multi-agent).
    if lower.starts_with("grok-4.5") {
        return Some(&["high", "medium", "low"]);
    }
    if lower.contains("multi-agent") {
        return Some(&["xhigh", "high", "medium", "low"]);
    }
    None
}

fn effort_rank(effort: Option<&str>) -> u8 {
    match effort {
        Some("xhigh") => 0,
        Some("high") => 1,
        Some("medium") => 2,
        Some("low") => 3,
        Some("none") => 4,
        Some(_) => 5,
        None => 6,
    }
}

fn expand_reasoning_rows(base: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let mut out = Vec::new();
    for m in base {
        if let Some(efforts) = efforts_for(&m.id) {
            for e in efforts {
                out.push(ModelInfo {
                    id: format!("{}:{e}", m.id),
                    name: format!("{} · {e}", m.name),
                    max_ctx: m.max_ctx,
                });
            }
        } else {
            out.push(m);
        }
    }
    // Flagship family first, then effort high→medium→low (never alpha on "high"/"low").
    out.sort_by(|a, b| {
        let (base_a, effort_a) = parse_model_spec(&a.id);
        let (base_b, effort_b) = parse_model_spec(&b.id);
        rank_model(&base_a)
            .cmp(&rank_model(&base_b))
            .then_with(|| base_a.cmp(&base_b))
            .then_with(|| effort_rank(effort_a.as_deref()).cmp(&effort_rank(effort_b.as_deref())))
    });
    out
}

fn rank_model(id: &str) -> u8 {
    let base = id.to_ascii_lowercase();
    if base.starts_with("grok-4.5") {
        0
    } else if base.starts_with("grok-4.3") {
        1
    } else if base.contains("multi-agent") {
        2
    } else if base.starts_with("grok-4.20") {
        3
    } else if base.starts_with("grok-build") {
        4
    } else if base.starts_with("grok-4") {
        5
    } else if base.contains("code") {
        6
    } else {
        10
    }
}

fn ids_to_model_info(ids: &[String]) -> Vec<ModelInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() || !is_chat_model_id(id) {
            continue;
        }
        let key = id.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        out.push(ModelInfo {
            id: id.to_string(),
            name: display_name_for_id(id),
            max_ctx: context_for_id(id),
        });
    }
    // Ensure curated flagship models appear even if the catalog is sparse.
    for m in curated_models() {
        let key = m.id.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(m);
        }
    }
    out
}

fn fetch_remote_models(api_key: &str) -> Result<Vec<String>, String> {
    let headers = vec![
        ("Authorization".into(), format!("Bearer {api_key}")),
        ("Accept".into(), "application/json".into()),
    ];
    let resp = http_client::request_get(MODELS_URL, &headers)?;
    parse_models_response(&resp.body)
}

fn parse_models_response(body: &str) -> Result<Vec<String>, String> {
    let val = crate::json::parse(body).map_err(|e| format!("models list: {e}"))?;
    let arr = val
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "models list: missing data array".to_string())?;
    let mut ids = Vec::new();
    for item in arr {
        if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity() {
        let p = XaiProvider::new();
        assert_eq!(p.name(), "xai");
        assert_eq!(p.default_model(), "grok-4.5:high");
        let models = p.available_models();
        assert!(models.iter().any(|m| m.id.starts_with("grok-4.5")));
        assert!(models.iter().any(|m| m.id == "grok-4.5:high"));
        assert!(models.iter().any(|m| m.id == "grok-4.5:low"));
    }

    #[test]
    fn parse_model_spec_effort() {
        assert_eq!(
            parse_model_spec("grok-4.5:high"),
            ("grok-4.5".into(), Some("high".into()))
        );
        assert_eq!(parse_model_spec("grok-4.3"), ("grok-4.3".into(), None));
        assert_eq!(
            parse_model_spec("grok-4.20-multi-agent-0309:xhigh"),
            ("grok-4.20-multi-agent-0309".into(), Some("xhigh".into()))
        );
        // Colon that is not an effort token stays intact
        assert_eq!(parse_model_spec("foo:bar"), ("foo:bar".into(), None));
    }

    #[test]
    fn request_body_includes_reasoning_effort() {
        let body = request_body(&[], &[], "sys", "grok-4.5:medium", false).unwrap();
        assert!(body.contains("\"model\":\"grok-4.5\""));
        assert!(body.contains("\"reasoning_effort\":\"medium\""));
        assert!(!body.contains("grok-4.5:medium"));
    }

    #[test]
    fn request_body_default_effort_for_45() {
        let body = request_body(&[], &[], "sys", "grok-4.5", true).unwrap();
        assert!(body.contains("\"reasoning_effort\":\"high\""));
    }

    #[test]
    fn request_body_no_effort_for_plain_models() {
        let body = request_body(&[], &[], "sys", "grok-3", false).unwrap();
        assert!(!body.contains("reasoning_effort"));
    }

    #[test]
    fn parse_models_json() {
        let body = r#"{"data":[{"id":"grok-4.5"},{"id":"grok-imagine-image"},{"id":"grok-4.3"}]}"#;
        let ids = parse_models_response(body).unwrap();
        assert_eq!(ids, vec!["grok-4.5", "grok-imagine-image", "grok-4.3"]);
        let infos = ids_to_model_info(&ids);
        assert!(infos.iter().any(|m| m.id == "grok-4.5"));
        assert!(infos.iter().any(|m| m.id == "grok-4.3"));
        assert!(!infos.iter().any(|m| m.id.contains("imagine")));
    }

    #[test]
    fn expand_multi_agent_has_xhigh() {
        let rows = expand_reasoning_rows(vec![ModelInfo {
            id: "grok-4.20-multi-agent-0309".into(),
            name: "Multi".into(),
            max_ctx: 1_000_000,
        }]);
        assert!(rows.iter().any(|m| m.id.ends_with(":xhigh")));
    }

    #[test]
    fn effort_rows_ordered_high_medium_low() {
        let rows = expand_reasoning_rows(vec![
            ModelInfo {
                id: "grok-4.5".into(),
                name: "Grok 4.5".into(),
                max_ctx: 500_000,
            },
            ModelInfo {
                id: "grok-4.20-multi-agent-0309".into(),
                name: "Multi".into(),
                max_ctx: 1_000_000,
            },
        ]);
        let g45: Vec<_> = rows
            .iter()
            .filter(|m| m.id.starts_with("grok-4.5:"))
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            g45,
            vec!["grok-4.5:high", "grok-4.5:medium", "grok-4.5:low"]
        );

        let ma: Vec<_> = rows
            .iter()
            .filter(|m| m.id.contains("multi-agent"))
            .map(|m| m.id.rsplit_once(':').unwrap().1)
            .collect();
        assert_eq!(ma, vec!["xhigh", "high", "medium", "low"]);
    }

    #[test]
    fn is_chat_filters_media() {
        assert!(is_chat_model_id("grok-4.5"));
        assert!(!is_chat_model_id("grok-imagine-video"));
        assert!(!is_chat_model_id("gpt-4o"));
    }
}
