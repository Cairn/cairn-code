use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ratatui::{
    Frame,
    layout::Position,
    widgets::{Paragraph, Wrap},
    style::{Style, Color, Modifier},
    text::{Span, Line, Text},
};

use crate::llm;
use crate::session;
use crate::agent::AgentEvent;

pub struct OutputLine {
    pub type_: String,
    pub content: String,
    pub tool_name: String,
    pub duration: String,
}

enum State {
    Idle,
    Running,
}

const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Tui {
    output_lines: Vec<OutputLine>,
    input_buf: String,
    cursor: usize,
    history: Vec<String>,
    hist_idx: usize,
    spinner_idx: usize,
    streaming_text: String,
    stream_thinking: String,
    state: State,
    total_usage: llm::Usage,
    version: String,
    model: String,
    provider: String,
    work_dir: String,
    show_model_picker: bool,
    picker_models: Vec<llm::ModelInfo>,
    picker_sel: usize,
    picker_scrl: usize,
    show_provider_picker: bool,
    provider_picker_list: Vec<String>,
    provider_picker_sel: usize,
    /// Whether each provider in provider_picker_list has an API key saved in the config file.
    provider_picker_keys: Vec<bool>,
    /// When Some, a confirmation prompt to remove this provider's saved API key is shown.
    confirm_remove_provider: Option<String>,
    confirm_remove_sel: usize,
    awaiting_api_key: bool,
    pending_openrouter_setup: bool,
    /// Provider name to capture an API key for (e.g. "openrouter", "opencode", "openai").
    /// When Some, the awaiting_api_key flow stores the key under this provider.
    api_key_target: Option<String>,
    show_command_picker: bool,
    cmd_picker_list: Vec<String>,
    cmd_picker_filtered: Vec<String>,
    cmd_picker_sel: usize,
    show_session_picker: bool,
    /// When true, Enter on the session picker deletes instead of resuming.
    session_picker_delete: bool,
    picker_sessions: Vec<session::SessionSummary>,
    picker_session_sel: usize,
    picker_session_scrl: usize,
    agent_tx: Option<mpsc::Sender<String>>,
    perm_tx: Option<mpsc::Sender<String>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    dirty: bool,
    show_permission_prompt: bool,
    perm_tool_name: String,
    perm_tool_input: String,
    perm_selection: usize,
}

impl Tui {
    pub fn new(version: &str, model: &str, provider: &str, work_dir: &str) -> Self {
        Tui {
            output_lines: Vec::new(),
            input_buf: String::new(),
            cursor: 0,
            history: Vec::new(),
            hist_idx: 0,
            spinner_idx: 0,
            streaming_text: String::new(),
            stream_thinking: String::new(),
            state: State::Idle,
            total_usage: llm::Usage::default(),
            version: version.to_string(),
            model: model.to_string(),
            provider: provider.to_string(),
            work_dir: work_dir.to_string(),
            show_model_picker: false,
            picker_models: Vec::new(),
            picker_sel: 0,
            picker_scrl: 0,
            show_provider_picker: false,
            provider_picker_list: Vec::new(),
            provider_picker_sel: 0,
            provider_picker_keys: Vec::new(),
            confirm_remove_provider: None,
            confirm_remove_sel: 0,
            awaiting_api_key: false,
            pending_openrouter_setup: false,
            api_key_target: None,
            show_command_picker: false,
            cmd_picker_list: vec![
                "/clear".into(), "/cost".into(), "/delete".into(), "/exit".into(), "/help".into(),
                "/model".into(), "/provider".into(), "/quit".into(), "/q".into(),
                "/resume".into(), "/save".into(), "/sessions".into(),
            ],
            cmd_picker_filtered: Vec::new(),
            cmd_picker_sel: 0,
            show_session_picker: false,
            session_picker_delete: false,
            picker_sessions: Vec::new(),
            picker_session_sel: 0,
            picker_session_scrl: 0,
            agent_tx: None,
            perm_tx: None,
            cancel_flag: None,
            dirty: false,
            show_permission_prompt: false,
            perm_tool_name: String::new(),
            perm_tool_input: String::new(),
            perm_selection: 0,
        }
    }

    pub fn set_agent_tx(&mut self, tx: mpsc::Sender<String>) {
        self.agent_tx = Some(tx);
    }

    pub fn set_perm_tx(&mut self, tx: mpsc::Sender<String>) {
        self.perm_tx = Some(tx);
    }

    pub fn set_cancel_flag(&mut self, flag: Arc<AtomicBool>) {
        self.cancel_flag = Some(flag);
    }

    pub fn run(&mut self, rx: mpsc::Receiver<AgentEvent>) -> Result<(), String> {
        let mut terminal = ratatui::init();
        terminal.clear().map_err(|e| e.to_string())?;

        let mut result = Ok(());
        let mut last_spinner_update = std::time::Instant::now();
        let mut last_draw = std::time::Instant::now();
        const MIN_FRAME: std::time::Duration = std::time::Duration::from_micros(1_000_000 / 240);
        let mut needs_rebuild = false;
        self.dirty = true;

        'outer: loop {
            if matches!(self.state, State::Running) {
                let mut got_event = false;
                while let Ok(event) = rx.try_recv() {
                    got_event = true;
                    match event {
                        AgentEvent::Text(t) => { self.streaming_text.push_str(&t); }
                        AgentEvent::Thinking(t) => { self.stream_thinking.push_str(&t); }
                        AgentEvent::ToolUse(name, input) => {
                            self.flush_streaming();
                            self.output_lines.push(OutputLine {
                                type_: "tool_use".into(), content: input, tool_name: name, duration: String::new(),
                            });
                        }
                        AgentEvent::ToolResult(name, _inp, out) => {
                            self.output_lines.push(OutputLine {
                                type_: "tool_result".into(), content: out, tool_name: name, duration: String::new(),
                            });
                        }
                        AgentEvent::Error(e) => {
                            self.flush_streaming();
                            self.output_lines.push(OutputLine {
                                type_: "error".into(), content: e, tool_name: String::new(), duration: String::new(),
                            });
                        }
                        AgentEvent::PermissionRequest(name, input) => {
                            self.flush_streaming();
                            self.show_permission_prompt = true;
                            self.perm_tool_name = name;
                            self.perm_tool_input = input;
                            self.perm_selection = 0;
                        }
                        AgentEvent::TurnEnd(u) => {
                            self.total_usage.input_tokens += u.input_tokens;
                            self.total_usage.output_tokens += u.output_tokens;
                        }
                        AgentEvent::Compacted(n) => {
                            self.flush_streaming();
                            self.output_lines.push(OutputLine {
                                type_: "system".into(),
                                content: format!("Compacted {n} earlier messages into a summary."),
                                tool_name: String::new(),
                                duration: String::new(),
                            });
                        }
                        AgentEvent::Done => {
                            self.flush_streaming();
                            self.state = State::Idle;
                        }
                    }
                }
                if got_event { self.dirty = true; }
            }

            if matches!(self.state, State::Idle) {
                match ratatui::crossterm::event::read() {
                    Ok(ratatui::crossterm::event::Event::Key(key)) => {
                        if !self.handle_key(key) {
                            break 'outer;
                        } else {
                            self.dirty = true;
                        }
                    }
                    Ok(ratatui::crossterm::event::Event::Resize(_, _)) => {
                        needs_rebuild = true;
                        self.dirty = true;
                    }
                    Err(e) => {
                        result = Err(format!("Event error: {e}"));
                        break 'outer;
                    }
                    _ => {}
                }
            } else {
                let event_avail = ratatui::crossterm::event::poll(Duration::from_millis(1)).unwrap_or(false);
                if event_avail {
                    if let Ok(ratatui::crossterm::event::Event::Key(key)) = ratatui::crossterm::event::read() {
                        if key.kind == ratatui::crossterm::event::KeyEventKind::Press {
                            self.handle_key(key);
                            self.dirty = true;
                        }
                    }
                }
                if last_spinner_update.elapsed() >= Duration::from_millis(80) {
                    self.spinner_idx = self.spinner_idx.wrapping_add(1);
                    last_spinner_update = std::time::Instant::now();
                    self.dirty = true;
                }
            }

            if needs_rebuild {
                let _ = terminal.clear();
                needs_rebuild = false;
            }

            if self.dirty {
                if let Some(sleep) = MIN_FRAME.checked_sub(last_draw.elapsed()) {
                    std::thread::sleep(sleep);
                }
                if let Err(e) = terminal.draw(|f| self.render(f)) {
                    result = Err(format!("Render error: {e}"));
                    break 'outer;
                }
                last_draw = std::time::Instant::now();
                self.dirty = false;
            }
        }

        ratatui::restore();
        result
    }

    fn handle_key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> bool {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers, KeyEventKind};

        if key.kind != KeyEventKind::Press {
            return true;
        }

        if self.show_permission_prompt {
            match key.code {
                KeyCode::Left => {
                    self.perm_selection = self.perm_selection.saturating_sub(1);
                }
                KeyCode::Right => {
                    if self.perm_selection < 2 { self.perm_selection += 1; }
                }
                KeyCode::Enter => {
                    let selected = match self.perm_selection {
                        0 => "allow",
                        1 => "always_allow",
                        _ => "deny",
                    };
                    self.show_permission_prompt = false;
                    if let Some(tx) = &self.perm_tx {
                        let _ = tx.send(selected.to_string());
                    }
                }
                KeyCode::Esc => {
                    self.show_permission_prompt = false;
                    if let Some(tx) = &self.perm_tx {
                        let _ = tx.send("deny".to_string());
                    }
                }
                _ => {}
            }
            return true;
        }

        if let Some(name) = self.confirm_remove_provider.clone() {
            match key.code {
                KeyCode::Left => { self.confirm_remove_sel = 0; }
                KeyCode::Right => { self.confirm_remove_sel = 1; }
                KeyCode::Esc => { self.confirm_remove_provider = None; }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.confirm_remove_provider = None;
                }
                KeyCode::Enter => {
                    let remove = self.confirm_remove_sel == 1;
                    self.confirm_remove_provider = None;
                    if remove {
                        let (type_, content) = match crate::config::remove_api_key(&name) {
                            Ok(true) => {
                                let env_note = match crate::config::env_key_for(&name) {
                                    Some(_) => format!(" Note: {} is still set in the environment and will still be used.",
                                        crate::config::env_var_name(&name).unwrap_or("its environment variable")),
                                    None => String::new(),
                                };
                                ("system", format!("Removed saved API key for {name}.{env_note}"))
                            }
                            Ok(false) => ("system", format!("No saved API key for {name}.")),
                            Err(e) => ("error", format!("Failed to remove API key for {name}: {e}")),
                        };
                        self.output_lines.push(OutputLine {
                            type_: type_.into(), content,
                            tool_name: String::new(), duration: String::new(),
                        });
                        self.provider_picker_keys = self.provider_picker_list.iter()
                            .map(|n| crate::config::config_has_api_key(n)).collect();
                    }
                }
                _ => {}
            }
            return true;
        }

        match key.code {
            KeyCode::Up => {
                if self.show_session_picker {
                    if self.picker_session_sel > 0 { self.picker_session_sel -= 1; }
                    return true;
                }
                if self.show_command_picker {
                    if self.cmd_picker_sel > 0 { self.cmd_picker_sel -= 1; }
                    return true;
                }
                if self.show_provider_picker {
                    if self.provider_picker_sel > 0 { self.provider_picker_sel -= 1; }
                    return true;
                }
                if self.show_model_picker {
                    if self.picker_sel > 0 { self.picker_sel -= 1; }
                    return true;
                }
                if self.hist_idx > 0 {
                    self.hist_idx -= 1;
                    self.input_buf = self.history[self.hist_idx].clone();
                    self.cursor = self.input_buf.len();
                }
                true
            }
            KeyCode::Down => {
                if self.show_session_picker {
                    if self.picker_session_sel + 1 < self.picker_sessions.len() { self.picker_session_sel += 1; }
                    return true;
                }
                if self.show_command_picker {
                    if self.cmd_picker_sel + 1 < self.cmd_picker_filtered.len() { self.cmd_picker_sel += 1; }
                    return true;
                }
                if self.show_provider_picker {
                    if self.provider_picker_sel + 1 < self.provider_picker_list.len() { self.provider_picker_sel += 1; }
                    return true;
                }
                if self.show_model_picker {
                    if self.picker_sel + 1 < self.picker_models.len() {
                        self.picker_sel += 1;
                        let vh = self.picker_visible_height();
                        if self.picker_sel >= self.picker_scrl + vh { self.picker_scrl = self.picker_sel - vh + 1; }
                    }
                    return true;
                }
                if self.hist_idx < self.history.len().saturating_sub(1) {
                    self.hist_idx += 1;
                    self.input_buf = self.history[self.hist_idx].clone();
                } else {
                    self.hist_idx = self.history.len();
                    self.input_buf.clear();
                }
                self.cursor = self.input_buf.len();
                true
            }
            KeyCode::Left => {
                if self.cursor > 0 { self.cursor -= 1; }
                true
            }
            KeyCode::Right => {
                if self.cursor < self.input_buf.len() { self.cursor += 1; }
                true
            }
            KeyCode::Tab => {
                if self.show_command_picker && !self.cmd_picker_filtered.is_empty() {
                    let cmd = &self.cmd_picker_filtered[self.cmd_picker_sel];
                    self.input_buf = cmd.clone();
                    self.cursor = cmd.len();
                    self.show_command_picker = false;
                    self.cmd_picker_filtered.clear();
                }
                true
            }
            KeyCode::Esc => {
                if self.awaiting_api_key {
                    self.awaiting_api_key = false;
                    self.pending_openrouter_setup = false;
                    self.input_buf.clear();
                    self.cursor = 0;
                } else if self.show_command_picker { self.show_command_picker = false; }
                else if self.show_provider_picker { self.show_provider_picker = false; }
                else if self.show_model_picker { self.show_model_picker = false; }
                else if self.show_session_picker {
                    self.show_session_picker = false;
                    self.session_picker_delete = false;
                }
                else if matches!(self.state, State::Running) {
                    if let Some(flag) = &self.cancel_flag {
                        flag.store(true, Ordering::Relaxed);
                    }
                    self.state = State::Idle;
                    self.flush_streaming();
                    self.output_lines.push(OutputLine {
                        type_: "system".into(), content: "Cancelled.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
                }
                true
            }
            KeyCode::Enter => {
                if self.show_command_picker && !self.cmd_picker_filtered.is_empty() {
                    let cmd = self.cmd_picker_filtered[self.cmd_picker_sel].clone();
                    self.show_command_picker = false;
                    self.cmd_picker_filtered.clear();
                    self.input_buf.clear();
                    self.cursor = 0;
                    self.handle_command(&cmd);
                    return true;
                }
                if self.show_provider_picker {
                    if self.provider_picker_sel < self.provider_picker_list.len() {
                        let name = self.provider_picker_list[self.provider_picker_sel].clone();
                        let providers = crate::llm::default_providers();
                        if let Some(p) = providers.get(&name) {
                            let default_model = p.default_model().to_string();
                            self.provider = name.clone();
                            self.picker_models = p.available_models();
                            if name == "openrouter" {
                                self.model = "gpt-5-mini".to_string();
                                self.show_provider_picker = false;
                                self.show_model_picker = true;
                                self.picker_sel = self.picker_models.iter().position(|m| m.id == "gpt-5-mini").unwrap_or(0);
                                self.picker_scrl = 0;
                                self.pending_openrouter_setup = true;
                            } else {
                                self.model = default_model;
                                self.show_provider_picker = false;
                        let _ = crate::config::save_config(&self.provider, &self.model, None);
                        self.output_lines.push(OutputLine {
                            type_: "system".into(), content: format!("Provider set to: {}", self.provider),
                            tool_name: String::new(), duration: String::new(),
                        });
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(format!("__switch__:{name}:{}", self.model));
                        }
                            }
                        } else { self.show_provider_picker = false; }
                    } else { self.show_provider_picker = false; }
                    return true;
                }
                if self.show_model_picker {
                    if self.picker_sel < self.picker_models.len() {
                        self.model = self.picker_models[self.picker_sel].id.clone();
                        self.show_model_picker = false;
                        if self.pending_openrouter_setup {
                            self.pending_openrouter_setup = false;
                            if std::env::var("OPENROUTER_API_KEY").is_ok() || crate::config::config_has_api_key("openrouter") {
                                let _ = crate::config::save_config(&self.provider, &self.model, None);
                                self.output_lines.push(OutputLine {
                                    type_: "system".into(), content: format!("Provider set to: {}\nModel set to: {}", self.provider, self.model),
                                    tool_name: String::new(), duration: String::new(),
                                });
                                if let Some(tx) = &self.agent_tx {
                                    let _ = tx.send(format!("__switch__:openrouter:{}", self.model));
                                }
                            } else {
                                self.awaiting_api_key = true;
                                self.input_buf.clear();
                                self.cursor = 0;
                                self.output_lines.push(OutputLine {
                                    type_: "system".into(), content: "Enter your OpenRouter API key:".into(),
                                    tool_name: String::new(), duration: String::new(),
                                });
                            }
                        } else {
                            let _ = crate::config::save_config(&self.provider, &self.model, None);
                            self.output_lines.push(OutputLine {
                                type_: "system".into(), content: format!("Model set to: {}", self.model),
                                tool_name: String::new(), duration: String::new(),
                            });
                        }
                    } else { self.show_model_picker = false; }
                    return true;
                }

                if self.show_session_picker {
                    if self.picker_session_sel < self.picker_sessions.len() {
                        let id = self.picker_sessions[self.picker_session_sel].id.clone();
                        let deleting = self.session_picker_delete;
                        self.show_session_picker = false;
                        self.session_picker_delete = false;
                        if deleting {
                            self.delete_session(&id);
                        } else {
                            self.resume_session(&id);
                        }
                    } else {
                        self.show_session_picker = false;
                        self.session_picker_delete = false;
                    }
                    return true;
                }

                if self.awaiting_api_key {
                    let key = self.input_buf.trim().to_string();
                    self.input_buf.clear(); self.cursor = 0;
                    if key.is_empty() { return true; }
                    self.awaiting_api_key = false;
                    std::env::set_var("OPENROUTER_API_KEY", &key);
                    let _ = crate::config::save_config(&self.provider, &self.model, Some(&key));
                    self.output_lines.push(OutputLine {
                        type_: "system".into(), content: "OpenRouter API key set.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
                    if let Some(tx) = &self.agent_tx {
                        let _ = tx.send(format!("__switch__:openrouter:{}", self.model));
                    }
                    return true;
                }

                if !matches!(self.state, State::Idle) { return true; }

                let input = self.input_buf.trim().to_string();
                self.input_buf.clear(); self.cursor = 0;
                if input.is_empty() { return true; }

                if input.starts_with('/') { self.handle_command(&input); return true; }

                self.history.push(input.clone());
                self.hist_idx = self.history.len();
                self.output_lines.push(OutputLine {
                    type_: "user".into(), content: input.clone(),
                    tool_name: String::new(), duration: String::new(),
                });
                self.state = State::Running;
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(input);
                }
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 && !self.show_model_picker && !self.show_provider_picker {
                    self.input_buf.remove(self.cursor - 1);
                    self.cursor -= 1;
                    self.update_cmd_picker();
                }
                true
            }
            KeyCode::Delete => {
                if self.show_provider_picker {
                    if self.provider_picker_sel < self.provider_picker_list.len() {
                        let name = self.provider_picker_list[self.provider_picker_sel].clone();
                        if crate::config::config_has_api_key(&name) {
                            self.confirm_remove_provider = Some(name);
                            self.confirm_remove_sel = 0;
                        } else {
                            self.output_lines.push(OutputLine {
                                type_: "system".into(),
                                content: format!("No saved API key for {name} in the config file."),
                                tool_name: String::new(), duration: String::new(),
                            });
                        }
                    }
                    return true;
                }
                if self.cursor < self.input_buf.len() && !self.show_model_picker {
                    self.input_buf.remove(self.cursor);
                }
                true
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.show_command_picker {
                    self.show_command_picker = false;
                } else if self.show_provider_picker {
                    self.show_provider_picker = false;
                } else if self.show_model_picker {
                    self.show_model_picker = false;
                } else {
                    self.input_buf.clear();
                    self.cursor = 0;
                }
                true
            }
            KeyCode::Char(ch) => {
                if !self.show_model_picker && !self.show_provider_picker && !self.awaiting_api_key {
                    self.input_buf.insert(self.cursor, ch);
                    self.cursor += ch.len_utf8();
                    self.update_cmd_picker();
                }
                true
            }
            _ => true,
        }
    }

    fn handle_command(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() { return; }

        match parts[0] {
            "/clear" => {
                self.output_lines.clear();
                self.streaming_text.clear();
                self.stream_thinking.clear();
            }
            "/model" => {
                if parts.len() > 1 {
                    self.model = parts[1..].join(" ");
                    let _ = crate::config::save_config(&self.provider, &self.model, None);
                    self.output_lines.push(OutputLine {
                        type_: "system".into(), content: format!("Model set to: {}", self.model),
                        tool_name: String::new(), duration: String::new(),
                    });
                } else {
                    self.show_model_picker = true;
                    self.picker_sel = 0;
                    self.picker_scrl = 0;
                }
            }
            "/cost" => {
                let est = crate::cost::estimate_cost(&self.model, &self.total_usage);
                let cost_str = crate::cost::format_cost(est);
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: format!("Tokens: {} in, {} out  •  {}\nModel: {}\nEstimated cost: {}",
                        self.total_usage.input_tokens, self.total_usage.output_tokens,
                        self.total_usage.cache_read + self.total_usage.cache_create,
                        self.model, cost_str),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            "/provider" => {
                let providers = crate::llm::default_providers();
                let mut names: Vec<String> = providers.into_keys().collect();
                names.sort();
                // Current provider first, matching render order so the
                // selection index always points at the displayed row.
                names.sort_by_key(|n| usize::from(*n != self.provider));
                self.provider_picker_keys = names.iter()
                    .map(|n| crate::config::config_has_api_key(n)).collect();
                self.provider_picker_list = names;
                self.provider_picker_sel = 0;
                self.show_provider_picker = true;
            }
            "/help" => {
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: "Commands: /clear /cost /delete /exit /help /model /provider /resume /save /sessions".into(),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            "/save" => {
                self.save_session();
            }
            "/sessions" => {
                self.list_sessions();
            }
            "/delete" => {
                if parts.len() > 1 {
                    let query = parts[1..].join(" ");
                    match session::resolve_id(&self.sessions_dir(), &query) {
                        Ok(id) => self.delete_session(&id),
                        Err(e) => {
                            self.output_lines.push(OutputLine {
                                type_: "error".into(),
                                content: e,
                                tool_name: String::new(), duration: String::new(),
                            });
                        }
                    }
                } else {
                    let sessions = session::list(&self.sessions_dir()).unwrap_or_default();
                    if sessions.is_empty() {
                        self.output_lines.push(OutputLine {
                            type_: "system".into(),
                            content: "No saved sessions to delete.".into(),
                            tool_name: String::new(), duration: String::new(),
                        });
                    } else {
                        self.show_session_picker = true;
                        self.session_picker_delete = true;
                        self.picker_sessions = sessions;
                        self.picker_session_sel = 0;
                        self.picker_session_scrl = 0;
                    }
                }
            }
            "/resume" => {
                let sessions = session::list(&self.sessions_dir()).unwrap_or_default();
                if sessions.is_empty() {
                    self.output_lines.push(OutputLine {
                        type_: "system".into(),
                        content: "No saved sessions. Use /save to save the current conversation first.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
                } else {
                    self.show_session_picker = true;
                    self.session_picker_delete = false;
                    self.picker_sessions = sessions;
                    self.picker_session_sel = 0;
                    self.picker_session_scrl = 0;
                }
            }
            "/exit" | "/quit" | "/q" => {
                std::process::exit(0);
            }
            _ => {
                self.output_lines.push(OutputLine {
                    type_: "error".into(),
                    content: format!("Unknown command: {} (type /help)", parts[0]),
                    tool_name: String::new(), duration: String::new(),
                });
            }
        }
    }

    fn sessions_dir(&self) -> String {
        crate::config::sessions_dir()
    }

    fn save_session(&mut self) {
        let messages = self.output_lines.iter().filter_map(|l| {
            if l.type_ == "user" {
                Some(llm::Message { role: "user".into(), content: llm::Content::Text(l.content.clone()) })
            } else if l.type_ == "text" {
                Some(llm::Message { role: "assistant".into(), content: llm::Content::Text(l.content.clone()) })
            } else {
                None
            }
        }).collect::<Vec<_>>();

        if messages.is_empty() {
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: "Nothing to save — no conversation yet.".into(),
                tool_name: String::new(), duration: String::new(),
            });
            return;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        let sess = session::Session {
            id: session::new_id(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            messages,
            tokens_in: self.total_usage.input_tokens,
            tokens_out: self.total_usage.output_tokens,
            created_at: now,
            updated_at: now,
        };
        match session::save(&self.sessions_dir(), &sess) {
            Ok(()) => {
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: format!("Session saved: {} ({} msgs)", &sess.id[..8], self.output_lines.len()),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            Err(e) => {
                self.output_lines.push(OutputLine {
                    type_: "error".into(),
                    content: format!("Failed to save session: {e}"),
                    tool_name: String::new(), duration: String::new(),
                });
            }
        }
    }

    fn list_sessions(&mut self) {
        let sessions = session::list(&self.sessions_dir()).unwrap_or_default();
        if sessions.is_empty() {
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: "No saved sessions.".into(),
                tool_name: String::new(), duration: String::new(),
            });
            return;
        }
        let mut msg = String::from("Saved sessions:\n");
        for s in &sessions {
            let time_str = format_timestamp(s.updated_at);
            let summary = if s.summary.len() > 60 {
                format!("{}…", &s.summary[..60])
            } else {
                s.summary.clone()
            };
            msg.push_str(&format!("  {}  {}  {} msgs  {}\n", &s.id[..8], s.model, s.msg_count, time_str));
            if !summary.is_empty() {
                msg.push_str(&format!("    {summary}\n"));
            }
        }
        self.output_lines.push(OutputLine {
            type_: "system".into(),
            content: msg.trim_end().to_string(),
            tool_name: String::new(), duration: String::new(),
        });
    }

    fn delete_session(&mut self, id: &str) {
        let short = if id.len() >= 8 { &id[..8] } else { id };
        match session::delete(&self.sessions_dir(), id) {
            Ok(()) => {
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: format!("Deleted session {short}."),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            Err(e) => {
                self.output_lines.push(OutputLine {
                    type_: "error".into(),
                    content: format!("Failed to delete session: {e}"),
                    tool_name: String::new(), duration: String::new(),
                });
            }
        }
    }

    fn resume_session(&mut self, id: &str) {
        match session::load(&self.sessions_dir(), id) {
            Ok(sess) => {
                let mut lines = Vec::new();
                for msg in &sess.messages {
                    let content = match &msg.content {
                        llm::Content::Text(t) => t.clone(),
                        llm::Content::Thinking(t) => t.clone(),
                        _ => continue,
                    };
                    lines.push(OutputLine {
                        type_: if msg.role == "user" { "user".into() } else { "text".into() },
                        content,
                        tool_name: String::new(), duration: String::new(),
                    });
                }
                self.output_lines = lines;
                self.total_usage = llm::Usage {
                    input_tokens: sess.tokens_in,
                    output_tokens: sess.tokens_out,
                    cache_read: 0,
                    cache_create: 0,
                };
                self.model = sess.model.clone();
                self.provider = sess.provider.clone();

                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(format!("__switch__:{}:{}", sess.provider, sess.model));
                    let _ = tx.send(format!("__load_session__:{}", sess.id));
                }
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: format!("Resumed session {} (model: {}, messages: {})", &sess.id[..8], sess.model, sess.messages.len()),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            Err(e) => {
                self.output_lines.push(OutputLine {
                    type_: "error".into(),
                    content: format!("Failed to load session: {e}"),
                    tool_name: String::new(), duration: String::new(),
                });
            }
        }
    }

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        let dim = Style::new().fg(Color::Indexed(245));
        let bright = Style::new().fg(Color::Indexed(215)).add_modifier(Modifier::BOLD);
        let bold_dim = Style::new().fg(Color::Indexed(240));
        let orange = Style::new().fg(Color::Indexed(215)).add_modifier(Modifier::BOLD);
        let white = Style::new().fg(Color::Indexed(252));
        let red = Style::new().fg(Color::Indexed(196));
        let green = Style::new().fg(Color::Indexed(78));
        let orange_fg = Style::new().fg(Color::Indexed(215));

        let mut lines: Vec<Line> = Vec::new();

        // Banner
        let w = area.width.saturating_sub(2) as usize;
        let pw = w.min(58);
        let pad = |s: &str| {
            let dw = display_width(s);
            if dw < pw {
                format!("{}{}", s, " ".repeat(pw - dw))
            } else {
                let mut out = String::with_capacity(pw);
                let mut w_used = 0;
                for c in s.chars() {
                    let cw = char_width(c);
                    if w_used + cw > pw { break; }
                    out.push(c);
                    w_used += cw;
                }
                if w_used < pw { out.push_str(&" ".repeat(pw - w_used)); }
                out
            }
        };
        let sp = " ".repeat((area.width as usize).saturating_sub(pw + 4));

        lines.push(Line::from(Span::styled(format!("╭{}╮", "─".repeat(pw)), dim)));
        lines.push(Line::from(vec![
            Span::styled("│", dim),
            Span::styled(pad(&format!("  ⚡ Cairn Code v{}", self.version)), bright),
            Span::styled(format!("│{sp}"), dim),
        ]));
        lines.push(Line::from(vec![
            Span::styled("│", dim),
            Span::styled(pad("  open terminal coding agent"), dim),
            Span::styled(format!("│{sp}"), dim),
        ]));
        lines.push(Line::from(Span::styled(format!("├{}┤", "─".repeat(pw)), dim)));
        lines.push(Line::from(vec![
            Span::styled("│", dim),
            Span::styled(pad(&format!("  Model   {} / {}", self.provider, self.model)), dim),
            Span::styled(format!("│{sp}"), dim),
        ]));
        lines.push(Line::from(vec![
            Span::styled("│", dim),
            Span::styled(pad(&format!("  Path    {}", self.work_dir)), dim),
            Span::styled(format!("│{sp}"), dim),
        ]));
        lines.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(pw)), dim)));

        // Output
        for line in &self.output_lines {
            match line.type_.as_str() {
                "user" => {
                    lines.push(Line::from(vec![Span::styled("❯ ", orange), Span::raw(&line.content)]));
                    lines.push(Line::from(""));
                }
                "text" => {
                    lines.extend(crate::markdown::render(&line.content));
                }
                "tool_use" => {
                    let inner = if line.content.len() > 80 { format!("\n  {}", line.content) } else { format!("({})", line.content) };
                    lines.push(Line::from(vec![
                        Span::styled("● ", white), Span::raw(&line.tool_name), Span::styled(inner, dim),
                    ]));
                }
                "tool_result" => {
                    let content = if line.content.len() > 500 { format!("{}... [truncated]", &line.content[..500]) } else { line.content.clone() };
                    let is_err = line.content.starts_with("Error:");
                    let color = if is_err { red } else { green };
                    let dur = if line.duration.is_empty() { String::new() } else { format!(" ({})", line.duration) };
                    lines.push(Line::from(vec![Span::styled("● ", color), Span::styled(format!("{}{dur}", line.tool_name), dim)]));
                    lines.push(Line::from(vec![Span::styled(format!("  {content}"), dim)]));
                }
                "error" => lines.push(Line::from(vec![Span::styled(format!("● {}", line.content), red)])),
                "system" => lines.push(Line::from(vec![Span::styled(&line.content, dim)])),
                _ => lines.push(Line::from(Span::raw(&line.content))),
            }
        }

        // Streaming
        if !self.stream_thinking.is_empty() {
            lines.push(Line::from(vec![Span::styled("── Thinking ──", bold_dim)]));
            for part in self.stream_thinking.split('\n') {
                lines.push(Line::from(vec![Span::styled(part, dim)]));
            }
        }
        if !self.streaming_text.is_empty() {
            lines.extend(crate::markdown::render(&self.streaming_text));
        }
        if matches!(self.state, State::Running) && self.streaming_text.is_empty() {
            let spin = SPINNER_CHARS[self.spinner_idx % SPINNER_CHARS.len()];
            lines.push(Line::from(format!("{spin} Thinking…")));
        }

        // Usage
        if self.total_usage.input_tokens > 0 {
            lines.push(Line::from(vec![
                Span::styled(format!("\nTokens: {} in, {} out  •  {}", self.total_usage.input_tokens, self.total_usage.output_tokens, self.model), dim),
            ]));
        }

        // Prompt or pickers
        let mut cursor_pos: Option<(u16, usize)> = None;
        if self.show_command_picker {
            let bg_orange = Style::new().bg(Color::Indexed(215)).fg(Color::Indexed(230));
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            lines.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::styled("▋", orange_fg),
                Span::raw(after),
            ]));
            for (i, cmd) in self.cmd_picker_filtered.iter().enumerate() {
                let is_sel = i == self.cmd_picker_sel;
                let prefix = if is_sel { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix}{cmd}"), if is_sel { bg_orange } else { dim }),
                ]));
            }
        } else if let Some(name) = &self.confirm_remove_provider {
            lines.push(Line::from(vec![
                Span::styled(format!("Remove saved API key for '{name}'?"), white),
            ]));
            lines.push(Line::from(vec![
                Span::styled("This only deletes the key from the config file.", dim),
            ]));
            lines.push(Line::from(""));
            let options = ["Cancel", "Remove"];
            let mut option_spans = Vec::new();
            for (i, opt) in options.iter().enumerate() {
                if i > 0 { option_spans.push(Span::raw("  ")); }
                let is_sel = i == self.confirm_remove_sel;
                let open = if is_sel { "[" } else { " " };
                let close = if is_sel { "]" } else { " " };
                option_spans.push(Span::styled(
                    format!("{open}{opt}{close}"),
                    if is_sel { orange_fg.add_modifier(Modifier::BOLD) } else { dim },
                ));
            }
            lines.push(Line::from(option_spans));
            lines.push(Line::from(vec![
                Span::styled("(← → navigate  Enter confirm  Esc cancel)", dim),
            ]));
        } else if self.show_provider_picker {
            let bg_orange = Style::new().bg(Color::Indexed(215)).fg(Color::Indexed(230));

            lines.push(Line::from(vec![
                Span::styled("── Provider ", orange_fg.add_modifier(Modifier::BOLD)),
                Span::styled("(↑↓ navigate  Enter select  Del remove key  Esc cancel) ──", bold_dim),
            ]));
            for (i, name) in self.provider_picker_list.iter().enumerate() {
                let is_sel = i == self.provider_picker_sel;
                let is_cur = *name == self.provider;
                let cur_mark = if is_cur { "  (current)" } else { "" };
                let key_mark = if self.provider_picker_keys.get(i).copied().unwrap_or(false) { "  [key saved]" } else { "" };
                let prefix = if is_sel { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix}{name}{key_mark}{cur_mark}"), if is_sel { bg_orange } else { dim }),
                ]));
            }
        } else if self.show_model_picker {
            let visible = self.picker_visible_height();
            let end = (self.picker_scrl + visible).min(self.picker_models.len());
            let num = self.picker_models.len();
            let bg_orange = Style::new().bg(Color::Indexed(215)).fg(Color::Indexed(230));
            let bold_dim = Style::new().fg(Color::Indexed(240));

            lines.push(Line::from(vec![
                Span::styled("── Model ", orange_fg.add_modifier(Modifier::BOLD)),
                Span::styled("(↑↓ navigate  Enter select  Esc cancel) ──", bold_dim),
            ]));
            if num > visible {
                lines.push(Line::from(vec![Span::styled(format!("  … {}/{}  ↑↓ scroll", self.picker_sel + 1, num), dim)]));
            }
            for i in self.picker_scrl..end {
                let m = &self.picker_models[i];
                let is_sel = i == self.picker_sel;
                let is_cur = m.id == self.model;
                let ctx = if m.max_ctx > 0 { format!(" ({}K context)", m.max_ctx / 1000) } else { String::new() };
                let check = if is_cur { "  ✓" } else { "" };
                let prefix = if is_sel { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix}{}  {}{ctx}{check}", m.name, m.id), if is_sel { bg_orange } else { dim }),
                ]));
            }
        } else if self.show_session_picker {
            let visible = 10usize;
            let end = (self.picker_session_scrl + visible).min(self.picker_sessions.len());
            let num = self.picker_sessions.len();
            let bg_orange = Style::new().bg(Color::Indexed(215)).fg(Color::Indexed(230));

            let title = if self.session_picker_delete {
                "── Delete Session "
            } else {
                "── Resume Session "
            };
            let hint = if self.session_picker_delete {
                "(↑↓ navigate  Enter delete  Esc cancel) ──"
            } else {
                "(↑↓ navigate  Enter select  Esc cancel) ──"
            };
            lines.push(Line::from(vec![
                Span::styled(title, orange_fg.add_modifier(Modifier::BOLD)),
                Span::styled(hint, bold_dim),
            ]));
            if num > visible {
                lines.push(Line::from(vec![Span::styled(format!("  … {}/{}  ↑↓ scroll", self.picker_session_sel + 1, num), dim)]));
            }
            for i in self.picker_session_scrl..end {
                let s = &self.picker_sessions[i];
                let is_sel = i == self.picker_session_sel;
                let prefix = if is_sel { "▸ " } else { "  " };
                let summary = if s.summary.len() > 50 {
                    format!("{}…", &s.summary[..50])
                } else {
                    s.summary.clone()
                };
                let time_str = format_timestamp(s.updated_at);
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix}{}  {}  {} msgs  {time_str}", &s.id[..8], s.model, s.msg_count), if is_sel { bg_orange } else { dim }),
                ]));
                if !summary.is_empty() && is_sel {
                    lines.push(Line::from(vec![
                        Span::styled(format!("   {summary}"), if is_sel { bg_orange } else { dim }),
                    ]));
                }
            }
        } else if self.show_permission_prompt {
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            let prompt_line_idx = lines.len();
            lines.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::styled("▋", orange_fg),
                Span::raw(after),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(format!("Tool '{}' wants to run:", self.perm_tool_name), white),
            ]));
            if !self.perm_tool_input.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}", self.perm_tool_input), dim),
                ]));
            }
            lines.push(Line::from(""));
            let options = ["Allow", "Always Allow", "Deny"];
            let mut option_spans = Vec::new();
            for (i, opt) in options.iter().enumerate() {
                if i > 0 { option_spans.push(Span::raw("  ")); }
                let is_sel = i == self.perm_selection;
                let open = if is_sel { "[" } else { " " };
                let close = if is_sel { "]" } else { " " };
                option_spans.push(Span::styled(
                    format!("{open}{opt}{close}"),
                    if is_sel { orange_fg.add_modifier(Modifier::BOLD) } else { dim },
                ));
            }
            lines.push(Line::from(option_spans));
            lines.push(Line::from(vec![
                Span::styled("(← → navigate  Enter confirm  Esc deny)", dim),
            ]));
            cursor_pos = Some((area.x + display_width("❯ ") as u16 + display_width(before) as u16, prompt_line_idx));
        } else if self.awaiting_api_key {
            let cursor = self.cursor.min(self.input_buf.len());
            let nchars = self.input_buf[..cursor].chars().count();
            let ntotal = self.input_buf.chars().count();
            let masked: Vec<char> = (0..ntotal).map(|_| '•').collect();
            let before: String = masked.iter().take(nchars).collect();
            let after: String = masked.iter().skip(nchars).collect();
            lines.push(Line::from(vec![
                Span::styled(format!("OpenRouter API key: {before}{after}"), orange_fg),
            ]));
            cursor_pos = Some((area.x + display_width("OpenRouter API key: ") as u16 + display_width(&before) as u16, lines.len() - 1));
        } else {
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            lines.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::raw(after),
            ]));
            cursor_pos = Some((area.x + display_width("❯ ") as u16 + display_width(before) as u16, lines.len() - 1));
        }

        let scroll = total_wrapped(&lines, area.width as usize).saturating_sub(area.height as usize);

        if let Some((x, line_idx)) = cursor_pos {
            let wrapped_before = total_wrapped(&lines[..line_idx], area.width as usize);
            let y = (area.y as usize + wrapped_before).saturating_sub(scroll) as u16;
            f.set_cursor_position(Position { x, y });
        }

        f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }).scroll((scroll as u16, 0)), area);
    }

    fn flush_streaming(&mut self) {
        if !self.streaming_text.is_empty() || !self.stream_thinking.is_empty() {
            if !self.streaming_text.is_empty() {
                self.output_lines.push(OutputLine {
                    type_: "text".into(), content: self.streaming_text.clone(),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            self.streaming_text.clear();
            self.stream_thinking.clear();
        }
    }

    fn picker_visible_height(&self) -> usize {
        let h = terminal_height().unwrap_or(24).saturating_sub(10);
        if h < 3 { 3 } else { h.min(self.picker_models.len()) }
    }

    pub fn set_picker_models(&mut self, models: Vec<llm::ModelInfo>) {
        self.picker_models = models;
    }

    pub fn add_output_line(&mut self, line: OutputLine) {
        self.output_lines.push(line);
    }

    fn update_cmd_picker(&mut self) {
        if self.input_buf.starts_with('/') && !self.input_buf.is_empty() {
            let filter = self.input_buf[1..].to_lowercase();
            self.cmd_picker_filtered = self.cmd_picker_list.iter()
                .filter(|c| c[1..].to_lowercase().starts_with(&filter))
                .cloned()
                .collect();
            if self.cmd_picker_sel >= self.cmd_picker_filtered.len() {
                self.cmd_picker_sel = self.cmd_picker_filtered.len().saturating_sub(1);
            }
            self.show_command_picker = !self.cmd_picker_filtered.is_empty();
        } else {
            self.show_command_picker = false;
            self.cmd_picker_filtered.clear();
        }
    }
}

fn char_width(c: char) -> usize {
    let cp = c as u32;
    if cp < 0x1100 { 1 }
    else if cp <= 0x115F { 2 }
    else if cp >= 0x2329 && cp <= 0x232A { 2 }
    else if cp >= 0x2E80 && cp <= 0x303E { 2 }
    else if cp >= 0x3040 && cp <= 0x3096 { 2 }
    else if cp >= 0x3099 && cp <= 0x30FF { 2 }
    else if cp >= 0x3105 && cp <= 0x312F { 2 }
    else if cp >= 0x3131 && cp <= 0x318E { 2 }
    else if cp >= 0x3190 && cp <= 0x31E3 { 2 }
    else if cp >= 0x31F0 && cp <= 0x321E { 2 }
    else if cp >= 0x3220 && cp <= 0x3247 { 2 }
    else if cp >= 0x3250 && cp <= 0x4DBF { 2 }
    else if cp >= 0x4E00 && cp <= 0xA4CF { 2 }
    else if cp >= 0xA960 && cp <= 0xA97C { 2 }
    else if cp >= 0xAC00 && cp <= 0xD7A3 { 2 }
    else if cp >= 0xF900 && cp <= 0xFAFF { 2 }
    else if cp >= 0xFE10 && cp <= 0xFE19 { 2 }
    else if cp >= 0xFE30 && cp <= 0xFE6F { 2 }
    else if cp >= 0xFF01 && cp <= 0xFF60 { 2 }
    else if cp >= 0xFFE0 && cp <= 0xFFE6 { 2 }
    else if cp >= 0x1B000 && cp <= 0x1B0FF { 2 }
    else if cp >= 0x1B100 && cp <= 0x1B12F { 2 }
    else if cp >= 0x1F200 && cp <= 0x1F2FF { 2 }
    else if cp >= 0x20000 && cp <= 0x2FFFD { 2 }
    else if cp >= 0x30000 && cp <= 0x3FFFD { 2 }
    else if cp >= 0x2600 && cp <= 0x26FF { 2 }
    else { 1 }
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn terminal_height() -> Option<usize> {
    std::env::var("LINES").ok().and_then(|v| v.parse().ok()).or(Some(24))
}

fn format_timestamp(ts: u64) -> String {
    let secs = ts as i64;
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h ago")
    } else if hours > 0 {
        format!("{hours}h {mins}m ago")
    } else {
        format!("{mins}m ago")
    }
}

fn total_wrapped(lines: &[Line], width: usize) -> usize {
    let w = width.max(1);
    lines.iter().map(|l| {
        let line_w: usize = l.spans.iter().map(|s| display_width(&s.content)).sum();
        if line_w == 0 { 1 } else { (line_w + w - 1) / w }
    }).sum()
}


