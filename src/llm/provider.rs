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

/// Image bytes already base64-encoded for provider APIs (no data: prefix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBlock {
    pub media_type: String,
    pub data_base64: String,
}

/// Multimodal user turn: optional text plus zero or more images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserBlocks {
    pub text: String,
    pub images: Vec<ImageBlock>,
}

impl UserBlocks {
    pub fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.images.is_empty()
    }

    /// Short label for TUI transcript lines (not the full base64 payload).
    pub fn display_label(&self) -> String {
        let n = self.images.len();
        let t = self.text.trim();
        match (t.is_empty(), n) {
            (true, 0) => String::new(),
            (true, 1) => "[image]".into(),
            (true, n) => format!("[{n} images]"),
            (false, 0) => t.to_string(),
            (false, 1) => format!("{t}\n[image]"),
            (false, n) => format!("{t}\n[{n} images]"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Content {
    Text(String),
    /// User message that may include pasted clipboard images.
    User(UserBlocks),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    Thinking(String),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: Content,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Content::Text(text.into()),
        }
    }

    pub fn user_blocks(blocks: UserBlocks) -> Self {
        if blocks.images.is_empty() {
            Self::user_text(blocks.text)
        } else {
            Self {
                role: "user".into(),
                content: Content::User(blocks),
            }
        }
    }
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
