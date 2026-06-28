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
    agent_tx: Option<mpsc::Sender<String>>,
    cancel_flag: Option<Arc<AtomicBool>>,
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
            agent_tx: None,
            cancel_flag: None,
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

        let poll_ms = Duration::from_millis(100);
        let mut result = Ok(());

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
                    Err(e) => {
                        result = Err(format!("Event error: {e}"));
                        break 'outer;
                    }
                    _ => {}
                }
            } else {
                if let Ok(true) = ratatui::crossterm::event::poll(poll_ms) {
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
                            self.output_lines.push(OutputLine {
                                type_: "system".into(), content: "Cancelled.".into(),
                                tool_name: String::new(), duration: String::new(),
                            });
                        }
                    }
                }
                self.spinner_idx += 1;
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
            KeyCode::Esc => {
                if self.show_model_picker { self.show_model_picker = false; }
                true
            }
            KeyCode::Enter => {
                if self.show_model_picker {
                    if self.picker_sel < self.picker_models.len() {
                        self.model = self.picker_models[self.picker_sel].id.clone();
                        self.show_model_picker = false;
                        self.output_lines.push(OutputLine {
                            type_: "system".into(), content: format!("Model set to: {}", self.model),
                            tool_name: String::new(), duration: String::new(),
                        });
                    } else { self.show_model_picker = false; }
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
                if self.cursor > 0 && !self.show_model_picker {
                    self.input_buf.remove(self.cursor - 1);
                    self.cursor -= 1;
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
                if self.show_model_picker {
                    self.show_model_picker = false;
                } else if matches!(self.state, State::Running) {
                    if let Some(flag) = &self.cancel_flag {
                        flag.store(true, Ordering::Relaxed);
                    }
                    self.state = State::Idle;
                    self.flush_streaming();
                    self.output_lines.push(OutputLine {
                        type_: "system".into(), content: "Cancelled.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
                } else {
                    return false;
                }
                true
            }
            KeyCode::Char(ch) => {
                if !self.show_model_picker {
                    self.input_buf.insert(self.cursor, ch);
                    self.cursor += ch.len_utf8();
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
                self.output_lines.push(OutputLine {
                    type_: "system".into(), content: format!("Provider: {}", self.provider),
                    tool_name: String::new(), duration: String::new(),
                });
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

        // Prompt or model picker
        if self.show_model_picker {
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
}

fn terminal_height() -> Option<usize> {
    std::env::var("LINES").ok().and_then(|v| v.parse().ok()).or(Some(24))
}
