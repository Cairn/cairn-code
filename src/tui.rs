use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ratatui::{
    Frame,
    widgets::Paragraph,
    style::{Style, Color, Modifier},
    text::{Span, Line, Text},
};

use crate::llm;
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
    awaiting_api_key: bool,
    pending_openrouter_setup: bool,
    show_command_picker: bool,
    cmd_picker_list: Vec<String>,
    cmd_picker_filtered: Vec<String>,
    cmd_picker_sel: usize,
    agent_tx: Option<mpsc::Sender<String>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    cancel_pressed_at: Option<std::time::Instant>,
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
            awaiting_api_key: false,
            pending_openrouter_setup: false,
            show_command_picker: false,
            cmd_picker_list: vec!["/clear".into(), "/cost".into(), "/exit".into(), "/help".into(), "/model".into(), "/provider".into(), "/quit".into(), "/q".into()],
            cmd_picker_filtered: Vec::new(),
            cmd_picker_sel: 0,
            agent_tx: None,
            cancel_flag: None,
            cancel_pressed_at: None,
        }
    }

    pub fn set_agent_tx(&mut self, tx: mpsc::Sender<String>) {
        self.agent_tx = Some(tx);
    }

    pub fn set_cancel_flag(&mut self, flag: Arc<AtomicBool>) {
        self.cancel_flag = Some(flag);
    }

    pub fn run(&mut self, rx: mpsc::Receiver<AgentEvent>) -> Result<(), String> {
        let mut terminal = ratatui::init();
        terminal.clear().map_err(|e| e.to_string())?;

        let mut result = Ok(());
        let mut last_spinner_update = std::time::Instant::now();
        let mut needs_rebuild = false;

        'outer: loop {
            if matches!(self.state, State::Running) {
                while let Ok(event) = rx.try_recv() {
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
                        AgentEvent::TurnEnd(u) => {
                            self.total_usage.input_tokens += u.input_tokens;
                            self.total_usage.output_tokens += u.output_tokens;
                        }
                        AgentEvent::Done => {
                            self.flush_streaming();
                            self.state = State::Idle;
                        }
                    }
                }
            }

            if matches!(self.state, State::Idle) {
                match ratatui::crossterm::event::read() {
                    Ok(ratatui::crossterm::event::Event::Key(key)) => {
                        if !self.handle_key(key) {
                            break 'outer;
                        }
                    }
                    Ok(ratatui::crossterm::event::Event::Resize(_, _)) => {
                        needs_rebuild = true;
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
                        if key.kind == ratatui::crossterm::event::KeyEventKind::Press
                            && key.code == ratatui::crossterm::event::KeyCode::Char('c')
                            && key.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
                        {
                            if let Some(flag) = &self.cancel_flag {
                                flag.store(true, Ordering::Relaxed);
                            }
                            self.state = State::Idle;
                            self.flush_streaming();
                            self.cancel_pressed_at = Some(std::time::Instant::now());
                            self.output_lines.push(OutputLine {
                                type_: "system".into(), content: "Cancelled. Press Ctrl+C again or type /exit to quit.".into(),
                                tool_name: String::new(), duration: String::new(),
                            });
                        }
                    }
                }
                if last_spinner_update.elapsed() >= Duration::from_millis(80) {
                    self.spinner_idx = self.spinner_idx.wrapping_add(1);
                    last_spinner_update = std::time::Instant::now();
                }
            }

            if needs_rebuild {
                let _ = terminal.clear();
                needs_rebuild = false;
            }

            if let Err(e) = terminal.draw(|f| self.render(f)) {
                result = Err(format!("Render error: {e}"));
                break 'outer;
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

        match key.code {
            KeyCode::Up => {
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
                } else if matches!(self.state, State::Running) {
                    if let Some(flag) = &self.cancel_flag {
                        flag.store(true, Ordering::Relaxed);
                    }
                    self.state = State::Idle;
                    self.flush_streaming();
                    self.cancel_pressed_at = Some(std::time::Instant::now());
                    self.output_lines.push(OutputLine {
                        type_: "system".into(), content: "Cancelled. Press Ctrl+C again or type /exit to quit.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
                } else if self.cancel_pressed_at.map_or(false, |t| t.elapsed() < std::time::Duration::from_secs(2)) {
                    return false;
                } else {
                    self.output_lines.push(OutputLine {
                        type_: "system".into(), content: "Press Ctrl+C again or type /exit to quit.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
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
                self.provider_picker_list = names;
                self.provider_picker_sel = self.provider_picker_list.iter().position(|n| n == &self.provider).unwrap_or(0);
                self.show_provider_picker = true;
            }
            "/help" => {
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: "Commands: /clear /model /cost /provider /help /exit".into(),
                    tool_name: String::new(), duration: String::new(),
                });
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
            let b = s.len();
            if b < pw { format!("{}{}", s, " ".repeat(pw - b)) }
            else { s.chars().take(pw).collect::<String>() }
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
                "text" => lines.push(Line::from(Span::raw(&line.content))),
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
            lines.push(Line::from(vec![Span::styled(&self.stream_thinking, dim)]));
        }
        if !self.streaming_text.is_empty() {
            lines.push(Line::from(Span::raw(&self.streaming_text)));
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
        } else if self.show_provider_picker {
            let bg_orange = Style::new().bg(Color::Indexed(215)).fg(Color::Indexed(230));
            let mut names: Vec<String> = self.provider_picker_list.clone();
            let current = self.provider.clone();
            names.sort_by(|a, b| {
                let ac = if *a == current { 0 } else { 1 };
                let bc = if *b == current { 0 } else { 1 };
                ac.cmp(&bc)
            });

            lines.push(Line::from(vec![
                Span::styled("── Provider ", orange_fg.add_modifier(Modifier::BOLD)),
                Span::styled("(↑↓ navigate  Enter select  Esc cancel) ──", bold_dim),
            ]));
            for (i, name) in names.iter().enumerate() {
                let is_sel = i == self.provider_picker_sel;
                let is_cur = *name == current;
                let check = if is_cur { "  ✓" } else { "" };
                let prefix = if is_sel { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix}{}{check}", name), if is_sel { bg_orange } else { dim }),
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
        } else if self.awaiting_api_key {
            let cursor = self.cursor.min(self.input_buf.len());
            let nchars = self.input_buf[..cursor].chars().count();
            let ntotal = self.input_buf.chars().count();
            let masked: Vec<char> = (0..ntotal).map(|_| '•').collect();
            let before: String = masked.iter().take(nchars).collect();
            let after: String = masked.iter().skip(nchars).collect();
            lines.push(Line::from(vec![
                Span::styled(format!("OpenRouter API key: {before}▋{after}"), orange_fg),
            ]));
        } else {
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            lines.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::styled("▋", orange_fg),
                Span::raw(after),
            ]));
        }

        let scroll = lines.len().saturating_sub(area.height as usize);
        f.render_widget(Paragraph::new(Text::from(lines)).scroll((scroll as u16, 0)), area);
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

fn terminal_height() -> Option<usize> {
    std::env::var("LINES").ok().and_then(|v| v.parse().ok()).or(Some(24))
}


