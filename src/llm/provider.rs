use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: String,
}

impl ToolUse {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("tool call is missing an id".into());
        }
        if self.name.trim().is_empty() {
            return Err("tool call is missing a name".into());
        }
        let input = crate::json::parse(&self.input)
            .map_err(|e| format!("tool call '{}' has invalid JSON arguments: {e}", self.name))?;
        if input.as_object().is_none() {
            return Err(format!(
                "tool call '{}' arguments must be a JSON object",
                self.name
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum Content {
    Text(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    Thinking(String),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: Content,
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: String,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub max_ctx: u64,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_create: u64,
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_create: 0,
        }
    }
}

impl Usage {
    pub fn _add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read += other.cache_read;
        self.cache_create += other.cache_create;
    }
}

pub type StreamingCallback = Box<dyn FnMut(&str, &str) + Send>;

pub trait Provider: Send {
    fn name(&self) -> &str;
    fn default_model(&self) -> &str;
    fn available_models(&self) -> Vec<ModelInfo>;
    fn stream_complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        max_tokens: usize,
        on_chunk: StreamingCallback,
        cancel: &AtomicBool,
    ) -> Result<(Vec<Message>, Usage), String>;
    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        model: &str,
        max_tokens: usize,
    ) -> Result<(Vec<Message>, Usage), String>;
}

/// Actionable message when a provider requires an API key that is missing.
pub fn missing_api_key(env_var: &str) -> String {
    format!(
        "{env_var} is not set. Export it in your shell, or save a key via the provider picker (/provider)."
    )
}

pub fn default_providers() -> HashMap<String, Box<dyn Provider>> {
    let mut map: HashMap<String, Box<dyn Provider>> = HashMap::new();
    map.insert(
        "anthropic".into(),
        Box::new(crate::llm::anthropic::AnthropicProvider::new()),
    );
    map.insert(
        "openai".into(),
        Box::new(crate::llm::openai::OpenAIProvider::new()),
    );
    map.insert(
        "openrouter".into(),
        Box::new(crate::llm::openrouter::OpenRouterProvider::new()),
    );
    map.insert(
        "opengateway".into(),
        Box::new(crate::llm::opengateway::OpenGatewayProvider::new()),
    );
    map.insert("xai".into(), Box::new(crate::llm::xai::XaiProvider::new()));
    map.insert(
        "ollama".into(),
        Box::new(crate::llm::ollama::OllamaProvider::new()),
    );
    map
}
