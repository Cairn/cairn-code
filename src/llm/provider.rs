use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: String,
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

pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_create: u64,
}

impl Default for Usage {
    fn default() -> Self {
        Usage { input_tokens: 0, output_tokens: 0, cache_read: 0, cache_create: 0 }
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

pub fn default_providers() -> HashMap<String, Box<dyn Provider>> {
    let mut map: HashMap<String, Box<dyn Provider>> = HashMap::new();
    map.insert("anthropic".into(), Box::new(crate::llm::anthropic::AnthropicProvider::new()));
    map.insert("openai".into(), Box::new(crate::llm::openai::OpenAIProvider::new()));
    let opencode = match crate::config::config_get_api_key("opencode") {
        Some(key) => crate::llm::opencode::OpenCodeProvider::new().with_api_key(&key),
        None => crate::llm::opencode::OpenCodeProvider::new(),
    };
    map.insert("opencode".into(), Box::new(opencode));
    let openrouter = match crate::config::config_get_api_key("openrouter") {
        Some(key) => crate::llm::openrouter::OpenRouterProvider::new().with_api_key(&key),
        None => crate::llm::openrouter::OpenRouterProvider::new(),
    };
    map.insert("openrouter".into(), Box::new(openrouter));
    map.insert("ollama".into(), Box::new(crate::llm::ollama::OllamaProvider::new()));
    map
}
