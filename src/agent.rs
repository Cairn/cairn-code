use std::fs;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::Config;
use crate::llm::Usage;
use crate::llm;
use crate::tools::registry::Registry;

#[allow(dead_code)]
pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ToolUse(String, String),
    ToolResult(String, String, String),
    Error(String),
    TurnEnd(llm::Usage),
    Done,
}

pub struct Agent {
    provider: Box<dyn llm::Provider>,
    model: String,
    messages: Vec<llm::Message>,
    tools: Registry,
    config: Config,
    usage: Usage,
}

impl Agent {
    pub fn new(
        provider: Box<dyn llm::Provider>,
        model: String,
        tools: Registry,
        config: Config,
    ) -> Self {
        Agent {
            provider,
            model,
            messages: Vec::new(),
            tools,
            config,
            usage: Usage::default(),
        }
    }

    #[allow(dead_code)]
    pub fn model(&self) -> &str { &self.model }
    #[allow(dead_code)]
    pub fn provider_name(&self) -> &str { self.provider.name() }
    #[allow(dead_code)]
    pub fn available_models(&self) -> Vec<llm::ModelInfo> { self.provider.available_models() }

    pub fn switch_provider(&mut self, provider_name: &str, model: &str) -> Result<(), String> {
        let mut providers = crate::llm::provider::default_providers();
        let provider = providers.remove(provider_name).ok_or_else(|| format!("Unknown provider: {provider_name}"))?;
        self.provider = provider;
        self.model = model.to_string();
        self.messages.clear();
        self.usage = Usage::default();
        Ok(())
    }

    pub fn run(&mut self, input: &str, tx: mpsc::Sender<AgentEvent>, cancel: &AtomicBool) -> Result<(), String> {
        self.messages.push(llm::Message {
            role: "user".into(),
            content: llm::Content::Text(input.to_string()),
        });

        let system = load_system_prompt(&self.config.system_prompt_file);

        let result = (|| -> Result<(), String> {
            for _turn in 0..self.config.max_turns {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }

                let tool_defs = self.tools.definitions();

                let tx_clone = tx.clone();
                let (new_msgs, usage) = self.provider.stream_complete(
                    &self.messages,
                    &tool_defs,
                    &system,
                    &self.model,
                    self.config.max_tokens,
                    Box::new(move |chunk, chunk_type| {
                        let _ = tx_clone.send(match chunk_type {
                            "thinking" => AgentEvent::Thinking(chunk.to_string()),
                            _ => AgentEvent::Text(chunk.to_string()),
                        });
                    }),
                ).map_err(|e| format!("LLM error: {e}"))?;

                self.usage.input_tokens += usage.input_tokens;
                self.usage.output_tokens += usage.output_tokens;
                self.usage.cache_read += usage.cache_read;
                self.usage.cache_create += usage.cache_create;

                let _ = tx.send(AgentEvent::TurnEnd(llm::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read: usage.cache_read,
                    cache_create: usage.cache_create,
                }));

                let tool_uses: Vec<llm::ToolUse> = new_msgs.iter()
                    .filter_map(|m| match &m.content {
                        llm::Content::ToolUse(tu) => Some(tu.clone()),
                        _ => None,
                    })
                    .collect();

                for msg in &new_msgs {
                    self.messages.push(msg.clone());
                }

                if tool_uses.is_empty() {
                    break;
                }

                for tu in &tool_uses {
                    let _ = tx.send(AgentEvent::ToolUse(tu.name.clone(), tu.input.clone()));

                    let needs_ask = self.config.ask.iter().any(|t| t == &tu.name);
                    let denied = self.config.is_tool_denied(&tu.name);

                    let result = if denied {
                        Err(format!("Tool '{}' is denied by config", tu.name))
                    } else if needs_ask {
                        Err(format!("Tool '{}' requires permission — not yet implemented in REPL", tu.name))
                    } else {
                        match self.tools.get(&tu.name) {
                            Some(tool) => tool.execute(&tu.input),
                            None => Err(format!("Unknown tool: {}", tu.name)),
                        }
                    };

                    let result_str = match &result {
                        Ok(s) => s.clone(),
                        Err(e) => format!("Error: {e}"),
                    };

                    let _ = tx.send(AgentEvent::ToolResult(
                        tu.name.clone(),
                        tu.input.clone(),
                        result_str.clone(),
                    ));

                    self.messages.push(llm::Message {
                        role: "user".into(),
                        content: llm::Content::ToolResult(llm::ToolResult {
                            tool_use_id: tu.id.clone(),
                            content: result_str,
                        }),
                    });
                }
            }
            Ok(())
        })();

        if let Err(ref e) = result {
            let _ = tx.send(AgentEvent::Error(e.clone()));
        }
        let _ = tx.send(AgentEvent::Done);
        result
    }

    pub fn run_simple(&mut self, input: &str) -> Result<String, String> {
        self.messages.push(llm::Message {
            role: "user".into(),
            content: llm::Content::Text(input.to_string()),
        });

        let system = load_system_prompt(&self.config.system_prompt_file);
        let tool_defs = self.tools.definitions();

        let (new_msgs, usage) = self.provider.complete(
            &self.messages,
            &tool_defs,
            &system,
            &self.model,
            self.config.max_tokens,
        )?;

        self.usage.input_tokens += usage.input_tokens;
        self.usage.output_tokens += usage.output_tokens;

        let mut output = String::new();
        for msg in &new_msgs {
            self.messages.push(msg.clone());
            match &msg.content {
                llm::Content::Text(t) => output.push_str(t),
                llm::Content::ToolUse(tu) => {
                    let result = match self.tools.get(&tu.name) {
                        Some(tool) => tool.execute(&tu.input).unwrap_or_else(|e| format!("Error: {e}")),
                        None => format!("Error: unknown tool {}", tu.name),
                    };
                    output.push_str(&format!("\n[{}({})]: {}", tu.name, tu.input, result));
                }
                _ => {}
            }
        }

        Ok(output)
    }
}

fn load_system_prompt(path: &str) -> String {
    if let Ok(content) = fs::read_to_string(path) {
        content
    } else {
        String::new()
    }
}
