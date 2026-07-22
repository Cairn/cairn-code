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
    PermissionRequest(String, String),
    Compacted(usize),
    Done,
}

/// Above this fraction of a model's context window, compact proactively
/// before sending the next turn.
const COMPACT_THRESHOLD: f64 = 0.7;
/// Number of trailing messages compaction always keeps verbatim (the actual
/// kept tail may be a bit longer, since the split walks left to an assistant
/// boundary so the suffix never starts mid tool-call).
const KEEP_RECENT_MESSAGES: usize = 8;
/// Don't bother compacting for a handful of messages.
const MIN_MESSAGES_TO_COMPACT: usize = KEEP_RECENT_MESSAGES + 2;
const SUMMARY_MAX_TOKENS: usize = 1024;

const SUMMARY_SYSTEM_PROMPT: &str = "You are compacting an in-progress coding session's history so it fits in a smaller context window. Summarize the conversation below concisely but completely: preserve file paths touched, decisions made, and any outstanding/unfinished tasks. Write plain prose, not a transcript. Do not use tools.";

pub struct Agent {
    provider: Box<dyn llm::Provider>,
    model: String,
    messages: Vec<llm::Message>,
    tools: Registry,
    config: Config,
    usage: Usage,
    last_input_tokens: u64,
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
            last_input_tokens: 0,
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
        self.last_input_tokens = 0;
        Ok(())
    }

    pub fn set_state(&mut self, messages: Vec<llm::Message>, usage: Usage) {
        self.messages = messages;
        self.usage = usage;
        self.last_input_tokens = 0;
    }

    #[allow(dead_code)]
    pub fn messages(&self) -> &[llm::Message] {
        &self.messages
    }

    #[allow(dead_code)]
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    fn should_proactively_compact(&self) -> bool {
        if self.last_input_tokens == 0 || self.messages.len() < MIN_MESSAGES_TO_COMPACT {
            return false;
        }
        let max_ctx = self.provider.available_models().iter()
            .find(|m| m.id == self.model)
            .map(|m| m.max_ctx);
        match max_ctx {
            Some(max_ctx) if max_ctx > 0 => {
                self.last_input_tokens as f64 >= max_ctx as f64 * COMPACT_THRESHOLD
            }
            _ => false,
        }
    }

    /// Summarizes the oldest messages (up to a safe split point) into one
    /// message, keeping the most recent turns verbatim. Returns the number
    /// of original messages folded into the summary, or 0 if it skipped
    /// (no safe split point, too few messages, or the summarization call
    /// itself failed).
    fn compact_history(&mut self, tx: &mpsc::Sender<AgentEvent>) -> usize {
        let Some(split) = find_safe_split_point(&self.messages) else { return 0; };

        let transcript = render_transcript(&self.messages[..split]);
        let request = vec![llm::Message { role: "user".into(), content: llm::Content::Text(transcript) }];

        let (summary_msgs, summary_usage) = match self.provider.complete(
            &request, &[], SUMMARY_SYSTEM_PROMPT, &self.model, SUMMARY_MAX_TOKENS,
        ) {
            Ok(r) => r,
            Err(_) => return 0,
        };

        let summary = summary_msgs.iter().find_map(|m| match &m.content {
            llm::Content::Text(t) if !t.is_empty() => Some(t.clone()),
            _ => None,
        });
        let Some(summary) = summary else { return 0; };

        let mut compacted = vec![llm::Message {
            role: "user".into(),
            content: llm::Content::Text(format!("[Earlier conversation summary]\n{summary}")),
        }];
        compacted.extend_from_slice(&self.messages[split..]);
        self.messages = compacted;

        // Reset so the next turn re-measures against the shrunk history instead
        // of immediately re-triggering on the pre-compact input token count.
        self.last_input_tokens = 0;

        self.usage.input_tokens += summary_usage.input_tokens;
        self.usage.output_tokens += summary_usage.output_tokens;
        let _ = tx.send(AgentEvent::TurnEnd(llm::Usage {
            input_tokens: summary_usage.input_tokens,
            output_tokens: summary_usage.output_tokens,
            cache_read: summary_usage.cache_read,
            cache_create: summary_usage.cache_create,
        }));
        let _ = tx.send(AgentEvent::Compacted(split));

        split
    }

    pub fn run(&mut self, input: &str, tx: mpsc::Sender<AgentEvent>, cancel: &AtomicBool, perm_rx: &mpsc::Receiver<String>) -> Result<(), String> {
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

                if self.should_proactively_compact() {
                    self.compact_history(&tx);
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
                    cancel,
                ).map_err(|e| format!("LLM error: {e}"))?;

                if cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }

                self.usage.input_tokens += usage.input_tokens;
                self.usage.output_tokens += usage.output_tokens;
                self.usage.cache_read += usage.cache_read;
                self.usage.cache_create += usage.cache_create;
                self.last_input_tokens = usage.input_tokens;

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
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }

                    let _ = tx.send(AgentEvent::ToolUse(tu.name.clone(), tu.input.clone()));

                    let tool = self.tools.get(&tu.name);
                    let wants_permission = tool.map(|t| t.needs_permission()).unwrap_or(false);
                    let needs_ask = wants_permission || self.config.ask.iter().any(|t| t == &tu.name);
                    let always_allowed = self.config.auto_allow.iter().any(|t| t == &tu.name);
                    let denied = self.config.is_tool_denied(&tu.name);

                    let result = if denied {
                        Err(format!("Tool '{}' is denied by config", tu.name))
                    } else if needs_ask && !always_allowed {
                        let _ = tx.send(AgentEvent::PermissionRequest(tu.name.clone(), tu.input.clone()));
                        let response = loop {
                            match perm_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                                Ok(resp) => break resp,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    if cancel.load(Ordering::Relaxed) {
                                        break "deny".to_string();
                                    }
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break "deny".to_string(),
                            }
                        };
                        match response.as_str() {
                            "always_allow" => {
                                self.config.auto_allow.push(tu.name.clone());
                                let _ = crate::config::save_full_config(&self.config);
                                self.tools.get(&tu.name).map(|t| t.execute(&tu.input)).unwrap_or(Err(format!("Unknown tool: {}", tu.name)))
                            }
                            "allow" => {
                                self.tools.get(&tu.name).map(|t| t.execute(&tu.input)).unwrap_or(Err(format!("Unknown tool: {}", tu.name)))
                            }
                            _ => Err(format!("Permission denied by user for tool '{}'", tu.name)),
                        }
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

/// Index at which to split history for compaction: `messages[..split]` is
/// summarized, `messages[split..]` is kept. Walks the naive keep-window start
/// leftward so the preserved suffix begins on an assistant message (providers
/// reject a dangling tool_result without its preceding tool_use, and a
/// user-led suffix would create consecutive user turns after the injected
/// summary). Returns `None` when there is nothing safe to fold.
fn find_safe_split_point(messages: &[llm::Message]) -> Option<usize> {
    if messages.len() < MIN_MESSAGES_TO_COMPACT {
        return None;
    }
    let mut split = messages.len() - KEEP_RECENT_MESSAGES;
    while split > 0 && messages[split].role != "assistant" {
        split -= 1;
    }
    if split == 0 {
        return None;
    }
    Some(split)
}

/// Flatten messages into plain text for the summarizer.
fn render_transcript(messages: &[llm::Message]) -> String {
    messages.iter().map(|m| match &m.content {
        llm::Content::Text(t) => format!("{}: {t}", m.role),
        llm::Content::Thinking(t) => format!("{} [thinking]: {t}", m.role),
        llm::Content::ToolUse(tu) => format!("assistant [tool call]: {}({})", tu.name, tu.input),
        llm::Content::ToolResult(tr) => format!("tool result: {}", tr.content),
    }).collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Content, Message, ToolResult, ToolUse};

    fn text(role: &str, body: &str) -> Message {
        Message { role: role.into(), content: Content::Text(body.into()) }
    }

    fn tool_use(name: &str, input: &str) -> Message {
        Message {
            role: "assistant".into(),
            content: Content::ToolUse(ToolUse {
                id: "1".into(),
                name: name.into(),
                input: input.into(),
            }),
        }
    }

    fn tool_result(content: &str) -> Message {
        Message {
            role: "user".into(),
            content: Content::ToolResult(ToolResult {
                tool_use_id: "1".into(),
                content: content.into(),
            }),
        }
    }

    #[test]
    fn find_safe_split_skips_short_histories() {
        let msgs = vec![text("user", "a"), text("assistant", "b")];
        assert!(find_safe_split_point(&msgs).is_none());
    }

    #[test]
    fn find_safe_split_keeps_recent_and_starts_on_assistant() {
        // 12 messages: u a u a u a u a u a u a
        let mut msgs = Vec::new();
        for i in 0..6 {
            msgs.push(text("user", &format!("u{i}")));
            msgs.push(text("assistant", &format!("a{i}")));
        }
        let split = find_safe_split_point(&msgs).expect("should split");
        // Naive keep window is last 8; walk left to assistant if needed.
        assert!(split <= msgs.len() - KEEP_RECENT_MESSAGES);
        assert_eq!(msgs[split].role, "assistant");
        assert!(msgs.len() - split >= KEEP_RECENT_MESSAGES);
    }

    #[test]
    fn find_safe_split_does_not_start_on_tool_result() {
        // Build enough messages, ending the keep window on a tool_result.
        let mut msgs = vec![
            text("user", "start"),
            text("assistant", "ok"),
            text("user", "do work"),
            tool_use("read", r#"{"path":"a"}"#),
            tool_result("file a"),
            text("assistant", "done a"),
            text("user", "more"),
            tool_use("read", r#"{"path":"b"}"#),
            tool_result("file b"),
            text("assistant", "done b"),
            text("user", "again"),
            text("assistant", "final"),
        ];
        // Pad if under min
        while msgs.len() < MIN_MESSAGES_TO_COMPACT {
            msgs.insert(0, text("user", "pad"));
            msgs.insert(1, text("assistant", "pad"));
        }
        let split = find_safe_split_point(&msgs).expect("should split");
        assert_eq!(msgs[split].role, "assistant");
        assert!(!matches!(msgs[split].content, Content::ToolResult(_)));
    }

    #[test]
    fn render_transcript_covers_content_kinds() {
        let msgs = vec![
            text("user", "hello"),
            text("assistant", "hi"),
            tool_use("grep", r#"{"q":"x"}"#),
            tool_result("matches"),
        ];
        let t = render_transcript(&msgs);
        assert!(t.contains("user: hello"));
        assert!(t.contains("assistant: hi"));
        assert!(t.contains("assistant [tool call]: grep("));
        assert!(t.contains("tool result: matches"));
    }
}

