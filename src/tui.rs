use std::io::stdout;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ratatui::{
    Frame,
    crossterm::{
        event::{
            DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
            MouseEventKind,
        },
        execute,
    },
    layout::{Constraint, Direction, Layout, Position},
    style::Modifier,
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use crate::llm;
use crate::session;
use crate::agent::AgentEvent;
use crate::theme::{self, Theme};

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
    /// After successful OAuth or API key entry from the provider picker, open the model list.
    /// Auth comes first so live model catalogs (xAI, Anthropic, …) can load.
    pending_model_after_auth: bool,
    /// Provider name to capture an API key for (e.g. "openrouter", "opengateway").
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
    /// After an LLM/provider failure: offer Switch model / Switch provider / Dismiss.
    show_recovery_prompt: bool,
    recovery_selection: usize,
    theme: Theme,
    show_theme_picker: bool,
    theme_picker_list: Vec<Theme>,
    theme_picker_sel: usize,
    /// Theme name before opening the picker (restored on Esc).
    theme_before_picker: Option<String>,
    /// Claude Code-style exit: first Ctrl+C on empty idle prompt arms this;
    /// second Ctrl+C exits. Disarmed by any other key or action.
    ctrl_c_exit_armed: bool,
    /// Transcript vertical offset (videre-style `rowoff`): first visible wrapped
    /// line of the body. When `transcript_follow` is true, view sticks to bottom.
    transcript_rowoff: usize,
    /// When true, keep the transcript pinned to the latest content (auto-scroll).
    transcript_follow: bool,
    /// Last body pane height / content height from render (for page sizes).
    last_body_h: usize,
    last_body_wrapped: usize,
    /// Active session id for autosave / resume (None until first save or resume).
    current_session_id: Option<String>,
    /// created_at for the active session (preserved across autosaves).
    session_created_at: u64,
    /// Full agent transcript (tools included) for session files.
    live_mirror: Option<session::LiveMirror>,
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
            pending_model_after_auth: false,
            api_key_target: None,
            show_command_picker: false,
            cmd_picker_list: vec![
                "/auth".into(), "/clear".into(), "/compact".into(), "/cost".into(), "/delete".into(),
                "/exit".into(), "/help".into(), "/model".into(), "/provider".into(), "/quit".into(),
                "/q".into(), "/resume".into(), "/save".into(), "/sessions".into(), "/theme".into(),
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
            show_recovery_prompt: false,
            recovery_selection: 0,
            theme: theme::default_theme(),
            show_theme_picker: false,
            theme_picker_list: theme::all_themes(),
            theme_picker_sel: 0,
            theme_before_picker: None,
            ctrl_c_exit_armed: false,
            transcript_rowoff: 0,
            transcript_follow: true,
            last_body_h: 0,
            last_body_wrapped: 0,
            current_session_id: None,
            session_created_at: 0,
            live_mirror: None,
        }
    }

    pub fn set_live_mirror(&mut self, mirror: session::LiveMirror) {
        self.live_mirror = Some(mirror);
    }

    pub fn set_theme_name(&mut self, name: &str) {
        self.theme = theme::lookup(name);
        self.theme_picker_list = theme::all_themes();
        self.theme_picker_sel = self.theme_picker_list.iter()
            .position(|t| t.name == self.theme.name)
            .unwrap_or(0);
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
        // Mouse wheel scroll works on Windows / macOS / Linux terminals that
        // support it (same idea as videre's cross-platform input layer).
        let _ = execute!(stdout(), EnableMouseCapture);

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
                            let is_llm = e.starts_with("LLM error:");
                            self.output_lines.push(OutputLine {
                                type_: "error".into(),
                                content: crate::redact::redact_secrets(&e),
                                tool_name: String::new(),
                                duration: String::new(),
                            });
                            // Offer a manual model/provider switch after LLM failures only
                            // (not for compact/session errors). Never silent multi-provider fallback.
                            if is_llm {
                                self.show_recovery_prompt = true;
                                self.recovery_selection = 0;
                            }
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
                            // Autosave after each finished turn (not only manual /save).
                            self.autosave_session(false);
                            if self.pending_model_after_auth {
                                if crate::config::has_usable_credential(&self.provider) {
                                    // Signed in — now pick a model (live catalog available).
                                    self.pending_model_after_auth = false;
                                    self.output_lines.push(OutputLine {
                                        type_: "system".into(),
                                        content: format!(
                                            "Signed in to {}. Choose a model.",
                                            self.provider
                                        ),
                                        tool_name: String::new(),
                                        duration: String::new(),
                                    });
                                    self.open_model_picker();
                                } else {
                                    // OAuth failed; keep pending so API key still continues to model picker.
                                    self.output_lines.push(OutputLine {
                                        type_: "system".into(),
                                        content: format!(
                                            "OAuth for {} did not complete. Paste an API key below, or run `/auth login {}` again.",
                                            self.provider, self.provider
                                        ),
                                        tool_name: String::new(),
                                        duration: String::new(),
                                    });
                                    self.begin_api_key_prompt(&self.provider.clone());
                                }
                            }
                        }
                    }
                }
                if got_event { self.dirty = true; }
            }

            if matches!(self.state, State::Idle) {
                match ratatui::crossterm::event::read() {
                    Ok(Event::Key(key)) => {
                        if !self.handle_key(key) {
                            break 'outer;
                        } else {
                            self.dirty = true;
                        }
                    }
                    Ok(Event::Mouse(m)) => {
                        self.handle_mouse(m.kind);
                        self.dirty = true;
                    }
                    Ok(Event::Resize(_, _)) => {
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
                    match ratatui::crossterm::event::read() {
                        Ok(Event::Key(key)) => {
                            if key.kind == KeyEventKind::Press {
                                self.handle_key(key);
                                self.dirty = true;
                            }
                        }
                        Ok(Event::Mouse(m)) => {
                            self.handle_mouse(m.kind);
                            self.dirty = true;
                        }
                        Ok(Event::Resize(_, _)) => {
                            needs_rebuild = true;
                            self.dirty = true;
                        }
                        _ => {}
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

        // Persist conversation on clean exit so the last session is not lost.
        self.autosave_session(false);
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        result
    }

    fn handle_mouse(&mut self, kind: MouseEventKind) {
        // Ignore mouse while pickers own the chrome (wheel should not fight them).
        if self.show_model_picker
            || self.show_provider_picker
            || self.show_theme_picker
            || self.show_session_picker
            || self.show_command_picker
        {
            return;
        }
        match kind {
            MouseEventKind::ScrollUp => self.scroll_transcript(-3),
            MouseEventKind::ScrollDown => self.scroll_transcript(3),
            _ => {}
        }
    }

    /// Scroll the transcript by `delta` wrapped lines (negative = up / older).
    /// Mirrors videre's rowoff model: free scroll until the bottom, then re-follow.
    fn scroll_transcript(&mut self, delta: isize) {
        let max_off = self
            .last_body_wrapped
            .saturating_sub(self.last_body_h.max(1));
        if max_off == 0 {
            self.transcript_rowoff = 0;
            self.transcript_follow = true;
            return;
        }
        let cur = if self.transcript_follow {
            max_off
        } else {
            self.transcript_rowoff.min(max_off)
        };
        let next = if delta < 0 {
            cur.saturating_sub((-delta) as usize)
        } else {
            cur.saturating_add(delta as usize).min(max_off)
        };
        self.transcript_rowoff = next;
        self.transcript_follow = next >= max_off;
    }

    fn scroll_page(&mut self, down: bool) {
        let page = self.last_body_h.max(1).saturating_sub(1);
        self.scroll_transcript(if down { page as isize } else { -(page as isize) });
    }

    fn scroll_half_page(&mut self, down: bool) {
        // videre: half = screen_rows / 2
        let half = (self.last_body_h.max(1) / 2).max(1);
        self.scroll_transcript(if down { half as isize } else { -(half as isize) });
    }

    fn handle_key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return true;
        }

        let is_ctrl_c = matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
            && key.modifiers.contains(KeyModifiers::CONTROL);
        // Claude Code: any key other than Ctrl+C disarms the "press again to exit" arm.
        if !is_ctrl_c {
            self.ctrl_c_exit_armed = false;
        }
        if is_ctrl_c {
            return self.handle_ctrl_c();
        }

        // Transcript scroll (videre-style Page / Ctrl-U/D + arrows with Ctrl).
        // Skip when a picker owns navigation keys.
        let picker_nav = self.show_model_picker
            || self.show_provider_picker
            || self.show_theme_picker
            || self.show_session_picker
            || self.show_command_picker
            || self.show_permission_prompt
            || self.confirm_remove_provider.is_some();
        if !picker_nav {
            match key.code {
                KeyCode::PageUp => {
                    self.scroll_page(false);
                    return true;
                }
                KeyCode::PageDown => {
                    self.scroll_page(true);
                    return true;
                }
                KeyCode::Char('u') | KeyCode::Char('U')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.scroll_half_page(false);
                    return true;
                }
                KeyCode::Char('d') | KeyCode::Char('D')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.scroll_half_page(true);
                    return true;
                }
                KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.scroll_transcript(-1);
                    return true;
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.scroll_transcript(1);
                    return true;
                }
                KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.transcript_follow = false;
                    self.transcript_rowoff = 0;
                    return true;
                }
                KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.transcript_follow = true;
                    return true;
                }
                _ => {}
            }
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

        if self.show_recovery_prompt {
            match key.code {
                KeyCode::Left => {
                    self.recovery_selection = self.recovery_selection.saturating_sub(1);
                }
                KeyCode::Right => {
                    if self.recovery_selection < 2 {
                        self.recovery_selection += 1;
                    }
                }
                KeyCode::Char('m') | KeyCode::Char('M') => {
                    self.show_recovery_prompt = false;
                    self.open_model_picker();
                }
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.show_recovery_prompt = false;
                    self.open_provider_picker();
                }
                KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => {
                    self.show_recovery_prompt = false;
                }
                KeyCode::Enter => {
                    let sel = self.recovery_selection;
                    self.show_recovery_prompt = false;
                    match sel {
                        0 => self.open_model_picker(),
                        1 => self.open_provider_picker(),
                        _ => {}
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
                            .map(|n| crate::config::has_usable_credential(n)).collect();
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
                if self.show_theme_picker {
                    if self.theme_picker_sel > 0 {
                        self.theme_picker_sel -= 1;
                        self.theme = self.theme_picker_list[self.theme_picker_sel].clone();
                    }
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
                if self.show_theme_picker {
                    if self.theme_picker_sel + 1 < self.theme_picker_list.len() {
                        self.theme_picker_sel += 1;
                        self.theme = self.theme_picker_list[self.theme_picker_sel].clone();
                    }
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
                // Slash-command completion: apply selected (or sole) match and
                // keep the picker open for the next argument when needed.
                if self.input_buf.starts_with('/') {
                    self.update_cmd_picker();
                }
                if self.show_command_picker && !self.cmd_picker_filtered.is_empty() {
                    let cmd = self.cmd_picker_filtered[self.cmd_picker_sel].clone();
                    self.apply_slash_completion(&cmd);
                }
                true
            }
            KeyCode::Esc => {
                if self.awaiting_api_key {
                    self.awaiting_api_key = false;
                    self.pending_model_after_auth = false;
                    self.api_key_target = None;
                    self.input_buf.clear();
                    self.cursor = 0;
                    self.output_lines.push(OutputLine {
                        type_: "system".into(),
                        content: "API key entry cancelled.".into(),
                        tool_name: String::new(),
                        duration: String::new(),
                    });
                } else if self.show_command_picker { self.show_command_picker = false; }
                else if self.show_provider_picker { self.show_provider_picker = false; }
                else if self.show_model_picker { self.show_model_picker = false; }
                else if self.show_session_picker {
                    self.show_session_picker = false;
                    self.session_picker_delete = false;
                }
                else if self.show_theme_picker {
                    self.show_theme_picker = false;
                    if let Some(prev) = self.theme_before_picker.take() {
                        self.theme = theme::lookup(&prev);
                    }
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
                            // Provisional default until the user picks from the model list.
                            self.model = if name == "openrouter" {
                                "gpt-5-mini".to_string()
                            } else {
                                default_model
                            };
                            self.show_provider_picker = false;
                            // Auth first (browser OAuth or API key), then model list.
                            // Live catalogs need credentials; model-before-login was backwards.
                            if crate::config::needs_credential(&name) {
                                self.pending_model_after_auth = true;
                                if crate::oauth::supports_oauth(&name) {
                                    self.begin_oauth_login(&name, true);
                                } else {
                                    self.begin_api_key_prompt(&name);
                                }
                            } else {
                                self.open_model_picker();
                            }
                        } else {
                            self.show_provider_picker = false;
                        }
                    } else {
                        self.show_provider_picker = false;
                    }
                    return true;
                }
                if self.show_model_picker {
                    if self.picker_sel < self.picker_models.len() {
                        self.model = self.picker_models[self.picker_sel].id.clone();
                        self.show_model_picker = false;
                        self.finish_provider_model_selection(None);
                    } else {
                        self.show_model_picker = false;
                    }
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

                if self.show_theme_picker {
                    if self.theme_picker_sel < self.theme_picker_list.len() {
                        self.theme = self.theme_picker_list[self.theme_picker_sel].clone();
                        let name = self.theme.name.to_string();
                        let label = self.theme.label.to_string();
                        let _ = crate::config::save_theme(&name);
                        self.output_lines.push(OutputLine {
                            type_: "system".into(),
                            content: format!("Theme set to: {label} ({name})"),
                            tool_name: String::new(),
                            duration: String::new(),
                        });
                    }
                    self.show_theme_picker = false;
                    self.theme_before_picker = None;
                    return true;
                }

                if self.awaiting_api_key {
                    let key = self.input_buf.trim().to_string();
                    self.input_buf.clear();
                    self.cursor = 0;
                    if key.is_empty() {
                        return true;
                    }
                    self.awaiting_api_key = false;
                    let target = self.api_key_target.take().unwrap_or_else(|| self.provider.clone());
                    if self.pending_model_after_auth {
                        // Provider switch path: save key, then open model list (live catalog).
                        self.pending_model_after_auth = false;
                        self.provider = target.clone();
                        crate::config::apply_key_to_env(&target, &key);
                        let _ = crate::config::save_config(&target, &self.model, Some(&key));
                        let tail = crate::config::mask_secret_display(&key, 4);
                        self.output_lines.push(OutputLine {
                            type_: "system".into(),
                            content: format!(
                                "API key saved for {target} ({tail}). Choose a model."
                            ),
                            tool_name: String::new(),
                            duration: String::new(),
                        });
                        self.open_model_picker();
                    } else {
                        // `/auth key` or similar: save and apply without forcing the model picker.
                        self.finish_provider_model_selection(Some((target, key)));
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
                self.show_recovery_prompt = false;
                // New turn: pin transcript to bottom (follow latest output).
                self.transcript_follow = true;
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
            KeyCode::Char(ch) => {
                // Allow typing into the API-key prompt and the normal input
                // (but not while a list picker is focused).
                if self.awaiting_api_key
                    || (!self.show_model_picker
                        && !self.show_provider_picker
                        && !self.show_theme_picker
                        && !self.show_session_picker)
                {
                    self.input_buf.insert(self.cursor, ch);
                    self.cursor += ch.len_utf8();
                    if !self.awaiting_api_key {
                        self.update_cmd_picker();
                    }
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
                // Finish the previous session file first, then open a fresh id.
                self.autosave_session(false);
                self.output_lines.clear();
                self.streaming_text.clear();
                self.stream_thinking.clear();
                self.current_session_id = None;
                self.session_created_at = 0;
                self.total_usage = llm::Usage::default();
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
                    self.open_model_picker();
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
                if parts.len() > 1 {
                    let name = parts[1].to_ascii_lowercase();
                    let providers = crate::llm::default_providers();
                    if let Some(p) = providers.get(&name) {
                        self.provider = name.clone();
                        self.model = p.default_model().to_string();
                        let _ = crate::config::save_config(&self.provider, &self.model, None);
                        if crate::config::needs_credential(&name) {
                            self.pending_model_after_auth = true;
                            if crate::oauth::supports_oauth(&name) {
                                self.begin_oauth_login(&name, true);
                            } else {
                                self.begin_api_key_prompt(&name);
                            }
                        } else {
                            self.output_lines.push(OutputLine {
                                type_: "system".into(),
                                content: format!(
                                    "Provider set to: {}\nModel set to: {}",
                                    self.provider, self.model
                                ),
                                tool_name: String::new(),
                                duration: String::new(),
                            });
                            if let Some(tx) = &self.agent_tx {
                                let _ = tx.send(format!("__switch__:{}:{}", self.provider, self.model));
                            }
                            self.open_model_picker();
                        }
                    } else {
                        self.output_lines.push(OutputLine {
                            type_: "system".into(),
                            content: format!("Unknown provider '{name}'. Use /provider to pick from the list."),
                            tool_name: String::new(),
                            duration: String::new(),
                        });
                    }
                } else {
                    self.open_provider_picker();
                }
            }
            "/help" => {
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: "Commands: /auth /clear /compact /cost /delete /exit /help /model /provider /resume /save /sessions /theme\nTab completes slash commands (auth, model, theme, provider, session ids)\nAfter an LLM error: Switch model (m) · Switch provider (p) · Dismiss (d/Esc)\nScroll: mouse wheel · PgUp/PgDn · Ctrl+U/D (half page) · Ctrl+Home/End\n/provider xai — browser OAuth; /auth login xai; /auth key xai for API key".into(),
                    tool_name: String::new(), duration: String::new(),
                });
            }
            "/auth" => {
                let sub = parts.get(1).copied().unwrap_or("status");
                match sub {
                    "login" => {
                        let provider = parts.get(2).copied().unwrap_or("xai").to_ascii_lowercase();
                        self.begin_oauth_login(&provider, false);
                    }
                    "key" => {
                        // Escape hatch: paste API key instead of OAuth (xAI / others).
                        let provider = parts.get(2).copied().unwrap_or("xai").to_ascii_lowercase();
                        if crate::config::provider_requires_api_key(&provider) {
                            self.begin_api_key_prompt(&provider);
                        } else {
                            self.output_lines.push(OutputLine {
                                type_: "system".into(),
                                content: format!("Provider '{provider}' does not use a cloud API key."),
                                tool_name: String::new(), duration: String::new(),
                            });
                        }
                    }
                    "logout" => {
                        let provider = parts.get(2).copied().unwrap_or("xai").to_ascii_lowercase();
                        if let Some(tx) = &self.agent_tx {
                            self.state = State::Running;
                            let _ = tx.send(format!("__auth_logout__:{provider}"));
                        }
                    }
                    "status" | _ => {
                        if let Some(tx) = &self.agent_tx {
                            self.state = State::Running;
                            let _ = tx.send("__auth_status__".into());
                        }
                    }
                }
            }
            "/theme" => {
                if parts.get(1) == Some(&"list") {
                    let names = theme::theme_names().join(", ");
                    self.output_lines.push(OutputLine {
                        type_: "system".into(),
                        content: format!("Active theme: {}\nThemes: {names}", self.theme.name),
                        tool_name: String::new(),
                        duration: String::new(),
                    });
                } else if parts.len() > 1 {
                    let name = parts[1..].join("-");
                    let t = theme::lookup(&name);
                    let applied = t.name.to_string();
                    let label = t.label.to_string();
                    self.theme = t;
                    let _ = crate::config::save_theme(&applied);
                    self.output_lines.push(OutputLine {
                        type_: "system".into(),
                        content: format!("Theme set to: {label} ({applied})"),
                        tool_name: String::new(),
                        duration: String::new(),
                    });
                } else {
                    self.open_theme_picker();
                }
            }
            "/compact" => {
                if !matches!(self.state, State::Idle) {
                    self.output_lines.push(OutputLine {
                        type_: "system".into(),
                        content: "Wait for the current turn to finish before compacting.".into(),
                        tool_name: String::new(), duration: String::new(),
                    });
                } else if let Some(tx) = &self.agent_tx {
                    self.state = State::Running;
                    let _ = tx.send("__compact__".into());
                }
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

    fn open_model_picker(&mut self) {
        let providers = crate::llm::default_providers();
        if let Some(p) = providers.get(&self.provider) {
            self.picker_models = p.available_models();
        }
        self.show_model_picker = true;
        self.picker_sel = self.picker_models.iter().position(|m| m.id == self.model).unwrap_or(0);
        let vh = self.picker_visible_height();
        self.picker_scrl = self.picker_sel.saturating_sub(vh.saturating_sub(1));
    }

    fn open_provider_picker(&mut self) {
        let providers = crate::llm::default_providers();
        let mut names: Vec<String> = providers.into_keys().collect();
        names.sort();
        // Current provider first, matching render order so the
        // selection index always points at the displayed row.
        names.sort_by_key(|n| usize::from(*n != self.provider));
        self.provider_picker_keys = names.iter()
            .map(|n| crate::config::has_usable_credential(n)).collect();
        self.provider_picker_list = names;
        self.provider_picker_sel = 0;
        self.show_provider_picker = true;
    }

    /// Claude Code Ctrl+C: interrupt running work; clear prompt when idle with
    /// text; on empty idle prompt, arm exit then quit on a second press.
    /// Returns `false` to exit the TUI.
    fn handle_ctrl_c(&mut self) -> bool {
        // Close overlays / cancel key entry first (same spirit as Esc).
        if self.awaiting_api_key {
            self.awaiting_api_key = false;
            self.pending_model_after_auth = false;
            self.api_key_target = None;
            self.input_buf.clear();
            self.cursor = 0;
            self.ctrl_c_exit_armed = false;
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: "API key entry cancelled.".into(),
                tool_name: String::new(),
                duration: String::new(),
            });
            return true;
        }
        if self.confirm_remove_provider.is_some() {
            self.confirm_remove_provider = None;
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_command_picker {
            self.show_command_picker = false;
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_provider_picker {
            self.show_provider_picker = false;
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_model_picker {
            self.show_model_picker = false;
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_session_picker {
            self.show_session_picker = false;
            self.session_picker_delete = false;
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_theme_picker {
            self.show_theme_picker = false;
            if let Some(prev) = self.theme_before_picker.take() {
                self.theme = theme::lookup(&prev);
            }
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_recovery_prompt {
            self.show_recovery_prompt = false;
            self.ctrl_c_exit_armed = false;
            return true;
        }
        if self.show_permission_prompt {
            self.show_permission_prompt = false;
            if let Some(tx) = &self.perm_tx {
                let _ = tx.send("deny".to_string());
            }
            self.ctrl_c_exit_armed = false;
            return true;
        }

        // Interrupt a running agent turn.
        if matches!(self.state, State::Running) {
            if let Some(flag) = &self.cancel_flag {
                flag.store(true, Ordering::Relaxed);
            }
            // Tell the agent loop as well (if it listens for "cancel").
            if let Some(tx) = &self.agent_tx {
                let _ = tx.send("cancel".into());
            }
            self.state = State::Idle;
            self.flush_streaming();
            self.ctrl_c_exit_armed = false;
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: "Interrupted.".into(),
                tool_name: String::new(),
                duration: String::new(),
            });
            return true;
        }

        // Idle: clear non-empty prompt (first action).
        if !self.input_buf.is_empty() {
            self.input_buf.clear();
            self.cursor = 0;
            self.show_command_picker = false;
            self.cmd_picker_filtered.clear();
            self.ctrl_c_exit_armed = false;
            return true;
        }

        // Empty idle prompt: second Ctrl+C exits; first arms confirmation.
        if self.ctrl_c_exit_armed {
            return false;
        }
        self.ctrl_c_exit_armed = true;
        self.output_lines.push(OutputLine {
            type_: "system".into(),
            content: "Press Ctrl+C again to exit".into(),
            tool_name: String::new(),
            duration: String::new(),
        });
        true
    }

    fn open_theme_picker(&mut self) {
        self.theme_before_picker = Some(self.theme.name.to_string());
        self.theme_picker_list = theme::all_themes();
        self.theme_picker_sel = self.theme_picker_list.iter()
            .position(|t| t.name == self.theme.name)
            .unwrap_or(0);
        // Live-preview current selection immediately
        if let Some(t) = self.theme_picker_list.get(self.theme_picker_sel) {
            self.theme = t.clone();
        }
        self.show_theme_picker = true;
    }

    fn begin_api_key_prompt(&mut self, provider: &str) {
        self.awaiting_api_key = true;
        self.api_key_target = Some(provider.to_string());
        self.input_buf.clear();
        self.cursor = 0;
        let env = crate::config::env_var_name(provider).unwrap_or("API_KEY");
        let oauth_hint = if crate::oauth::supports_oauth(provider) {
            " Prefer browser login: Esc, then `/auth login xai`."
        } else {
            ""
        };
        self.output_lines.push(OutputLine {
            type_: "system".into(),
            content: format!(
                "Enter API key for {provider} (saved to OS keyring, env {env}). Input is masked.{oauth_hint}"
            ),
            tool_name: String::new(),
            duration: String::new(),
        });
    }

    /// Start device-code OAuth (browser) for a provider, like zero / Grok Build.
    /// When `then_model_picker` is true (provider path), successful login opens
    /// the model list; on failure, falls back to API key paste then model list.
    fn begin_oauth_login(&mut self, provider: &str, then_model_picker: bool) {
        if !crate::oauth::supports_oauth(provider) {
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: format!(
                    "OAuth login is not implemented for '{provider}'. Supported: xai. Use an API key via /auth key {provider}."
                ),
                tool_name: String::new(),
                duration: String::new(),
            });
            return;
        }
        if !matches!(self.state, State::Idle) {
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: "Wait for the current turn to finish before logging in.".into(),
                tool_name: String::new(),
                duration: String::new(),
            });
            return;
        }
        let Some(tx) = &self.agent_tx else {
            self.output_lines.push(OutputLine {
                type_: "error".into(),
                content: "Agent channel not ready; cannot start OAuth.".into(),
                tool_name: String::new(),
                duration: String::new(),
            });
            return;
        };
        self.pending_model_after_auth = then_model_picker;
        self.state = State::Running;
        self.output_lines.push(OutputLine {
            type_: "system".into(),
            content: "Starting xAI browser OAuth (device code)… A browser window should open. Approve the code shown next, or open the URL manually.".into(),
            tool_name: String::new(),
            duration: String::new(),
        });
        let _ = tx.send(format!("__auth_login__:{provider}"));
    }

    /// Finish provider/model selection, optionally saving a freshly entered key.
    fn finish_provider_model_selection(&mut self, new_key: Option<(String, String)>) {
        if let Some((provider, key)) = new_key {
            crate::config::apply_key_to_env(&provider, &key);
            let _ = crate::config::save_config(&provider, &self.model, Some(&key));
            self.provider = provider;
            let tail = crate::config::mask_secret_display(&key, 4);
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: format!(
                    "API key saved for {} ({}). Provider set to: {}\nModel set to: {}",
                    self.provider, tail, self.provider, self.model
                ),
                tool_name: String::new(),
                duration: String::new(),
            });
        } else {
            let _ = crate::config::save_config(&self.provider, &self.model, None);
            self.output_lines.push(OutputLine {
                type_: "system".into(),
                content: format!("Provider set to: {}\nModel set to: {}", self.provider, self.model),
                tool_name: String::new(),
                duration: String::new(),
            });
        }
        if let Some(tx) = &self.agent_tx {
            let _ = tx.send(format!("__switch__:{}:{}", self.provider, self.model));
        }
    }

    /// Prefer the agent's full transcript (tools included); fall back to TUI lines.
    fn session_snapshot(&self) -> (Vec<llm::Message>, u64, u64) {
        if let Some(mirror) = &self.live_mirror {
            if let Ok(g) = mirror.lock() {
                if !g.messages.is_empty() {
                    return (g.messages.clone(), g.tokens_in, g.tokens_out);
                }
            }
        }
        let messages = self
            .output_lines
            .iter()
            .filter_map(|l| {
                if l.type_ == "user" {
                    Some(llm::Message {
                        role: "user".into(),
                        content: llm::Content::Text(l.content.clone()),
                    })
                } else if l.type_ == "text" {
                    Some(llm::Message {
                        role: "assistant".into(),
                        content: llm::Content::Text(l.content.clone()),
                    })
                } else if l.type_ == "tool_use" {
                    Some(llm::Message {
                        role: "assistant".into(),
                        content: llm::Content::ToolUse(llm::ToolUse {
                            id: String::new(),
                            name: l.tool_name.clone(),
                            input: l.content.clone(),
                        }),
                    })
                } else if l.type_ == "tool_result" {
                    Some(llm::Message {
                        role: "user".into(),
                        content: llm::Content::ToolResult(llm::ToolResult {
                            tool_use_id: String::new(),
                            content: l.content.clone(),
                        }),
                    })
                } else {
                    None
                }
            })
            .collect();
        (
            messages,
            self.total_usage.input_tokens,
            self.total_usage.output_tokens,
        )
    }

    /// Save (or update) the current session. When `announce` is true, print a
    /// system line (manual `/save`). Autosave stays quiet unless it fails.
    fn autosave_session(&mut self, announce: bool) {
        let (messages, tokens_in, tokens_out) = self.session_snapshot();
        if messages.is_empty() {
            if announce {
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: "Nothing to save — no conversation yet.".into(),
                    tool_name: String::new(),
                    duration: String::new(),
                });
            }
            return;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let id = self
            .current_session_id
            .clone()
            .unwrap_or_else(session::new_id);
        let created_at = if self.session_created_at > 0 {
            self.session_created_at
        } else {
            now
        };
        let msg_count = messages.len();
        let sess = session::Session {
            id: id.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            messages,
            tokens_in,
            tokens_out,
            created_at,
            updated_at: now,
        };
        match session::save(&self.sessions_dir(), &sess) {
            Ok(()) => {
                self.current_session_id = Some(id.clone());
                self.session_created_at = created_at;
                if announce {
                    let short = if id.len() >= 8 { &id[..8] } else { id.as_str() };
                    self.output_lines.push(OutputLine {
                        type_: "system".into(),
                        content: format!(
                            "Session saved: {short} ({msg_count} msgs) → {}",
                            self.sessions_dir()
                        ),
                        tool_name: String::new(),
                        duration: String::new(),
                    });
                }
            }
            Err(e) => {
                // Always surface write failures (including silent autosave).
                self.output_lines.push(OutputLine {
                    type_: "error".into(),
                    content: format!("Failed to save session: {e}"),
                    tool_name: String::new(),
                    duration: String::new(),
                });
            }
        }
    }

    fn save_session(&mut self) {
        self.autosave_session(true);
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
                // Rebuild TUI transcript including tool calls/results for continuity.
                let mut lines = Vec::new();
                for msg in &sess.messages {
                    match &msg.content {
                        llm::Content::Text(t) => {
                            lines.push(OutputLine {
                                type_: if msg.role == "user" {
                                    "user".into()
                                } else {
                                    "text".into()
                                },
                                content: t.clone(),
                                tool_name: String::new(),
                                duration: String::new(),
                            });
                        }
                        llm::Content::Thinking(t) => {
                            lines.push(OutputLine {
                                type_: "system".into(),
                                content: format!("(thinking) {t}"),
                                tool_name: String::new(),
                                duration: String::new(),
                            });
                        }
                        llm::Content::ToolUse(tu) => {
                            lines.push(OutputLine {
                                type_: "tool_use".into(),
                                content: tu.input.clone(),
                                tool_name: tu.name.clone(),
                                duration: String::new(),
                            });
                        }
                        llm::Content::ToolResult(tr) => {
                            lines.push(OutputLine {
                                type_: "tool_result".into(),
                                content: tr.content.clone(),
                                tool_name: "tool".into(),
                                duration: String::new(),
                            });
                        }
                    }
                }
                self.output_lines = lines;
                self.total_usage = llm::Usage {
                    input_tokens: sess.tokens_in,
                    output_tokens: sess.tokens_out,
                    cache_read: 0,
                    cache_create: 0,
                };
                // Seed the live mirror so the next autosave keeps full history.
                if let Some(mirror) = &self.live_mirror {
                    if let Ok(mut g) = mirror.lock() {
                        g.messages = sess.messages.clone();
                        g.tokens_in = sess.tokens_in;
                        g.tokens_out = sess.tokens_out;
                    }
                }
                self.model = sess.model.clone();
                self.provider = sess.provider.clone();
                self.current_session_id = Some(sess.id.clone());
                self.session_created_at = if sess.created_at > 0 {
                    sess.created_at
                } else {
                    sess.updated_at
                };

                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(format!("__switch__:{}:{}", sess.provider, sess.model));
                    let _ = tx.send(format!("__load_session__:{}", sess.id));
                }
                let short = if sess.id.len() >= 8 {
                    &sess.id[..8]
                } else {
                    sess.id.as_str()
                };
                self.output_lines.push(OutputLine {
                    type_: "system".into(),
                    content: format!(
                        "Resumed session {short} (model: {}, messages: {})",
                        sess.model,
                        sess.messages.len()
                    ),
                    tool_name: String::new(),
                    duration: String::new(),
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

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        let dim = self.theme.muted;
        let bright = self.theme.accent;
        let bold_dim = self.theme.faintest;
        let orange = self.theme.accent;
        let white = self.theme.ink;
        let red = self.theme.red;
        let green = self.theme.green;
        let orange_fg = self.theme.accent_fg;
        let selected = self.theme.selected;

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
                    // Use a different marker than the live composer (❯) so past turns
                    // are not mistaken for a second prompt.
                    lines.push(Line::from(vec![
                        Span::styled("› ", orange),
                        Span::raw(&line.content),
                    ]));
                    lines.push(Line::from(""));
                }
                "text" => {
                    lines.extend(crate::markdown::render(&line.content));
                }
                "tool_use" => {
                    lines.push(Line::from(vec![
                        Span::styled("● ", white),
                        Span::raw(&line.tool_name),
                    ]));
                    // Multi-line args (pretty JSON) need separate Lines; a single
                    // Line with embedded \n does not wrap as real newlines in ratatui.
                    let arg = line.content.trim();
                    if !arg.is_empty() {
                        let shown = if arg.chars().count() > 240 {
                            let head: String = arg.chars().take(240).collect();
                            format!("{head}…")
                        } else {
                            arg.to_string()
                        };
                        for part in shown.split('\n') {
                            lines.push(Line::from(vec![
                                Span::styled(format!("  {part}"), dim),
                            ]));
                        }
                    }
                }
                "tool_result" => {
                    let is_err = line.content.starts_with("Error:")
                        || line.content.contains("exit code")
                            && !line.content.contains("(exit code 0)");
                    let color = if is_err { red } else { green };
                    let dur = if line.duration.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", line.duration)
                    };
                    lines.push(Line::from(vec![
                        Span::styled("● ", color),
                        Span::styled(format!("{}{dur}", line.tool_name), dim),
                    ]));
                    // Head+tail so long command output stays readable and keeps the footer.
                    let display = truncate_display(&line.content, 80, 40);
                    for part in display.split('\n') {
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {part}"), dim),
                        ]));
                    }
                }
                "error" => {
                    for (i, part) in line.content.split('\n').enumerate() {
                        if i == 0 {
                            lines.push(Line::from(vec![Span::styled(format!("● {part}"), red)]));
                        } else {
                            lines.push(Line::from(vec![Span::styled(format!("  {part}"), red)]));
                        }
                    }
                }
                "system" => {
                    for part in line.content.split('\n') {
                        lines.push(Line::from(vec![Span::styled(part, dim)]));
                    }
                }
                _ => {
                    for part in line.content.split('\n') {
                        lines.push(Line::from(Span::raw(part)));
                    }
                }
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

        // Composer / pickers live in a fixed bottom chrome region so typing never
        // steals viewport rows from the transcript above (single-scroll layout used to
        // push the last LLM line off-screen as soon as the prompt grew).
        let mut chrome: Vec<Line> = Vec::new();
        // Cursor: (x offset within chrome width, logical line index in chrome).
        let mut cursor_pos: Option<(u16, usize)> = None;

        if self.show_command_picker {
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            chrome.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::styled("▋", orange_fg),
                Span::raw(after),
            ]));
            for (i, cmd) in self.cmd_picker_filtered.iter().enumerate() {
                let is_sel = i == self.cmd_picker_sel;
                let prefix = if is_sel { "▸ " } else { "  " };
                chrome.push(Line::from(vec![
                    Span::styled(format!("{prefix}{cmd}"), if is_sel { selected } else { dim }),
                ]));
            }
            cursor_pos = Some((display_width("❯ ") as u16 + display_width(before) as u16, 0));
        } else if let Some(name) = &self.confirm_remove_provider {
            chrome.push(Line::from(vec![
                Span::styled(format!("Remove saved API key for '{name}'?"), white),
            ]));
            chrome.push(Line::from(vec![
                Span::styled("This only deletes the key from the config file.", dim),
            ]));
            chrome.push(Line::from(""));
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
            chrome.push(Line::from(option_spans));
            chrome.push(Line::from(vec![
                Span::styled("(← → navigate  Enter confirm  Esc cancel)", dim),
            ]));
        } else if self.show_provider_picker {
            chrome.push(Line::from(vec![
                Span::styled("── Provider ", orange),
                Span::styled("(↑↓ navigate  Enter select  Del remove key  Esc cancel) ──", bold_dim),
            ]));
            for (i, name) in self.provider_picker_list.iter().enumerate() {
                let is_sel = i == self.provider_picker_sel;
                let is_cur = *name == self.provider;
                let cur_mark = if is_cur { "  (current)" } else { "" };
                let key_mark = if self.provider_picker_keys.get(i).copied().unwrap_or(false) {
                    "  [signed in]"
                } else if crate::oauth::supports_oauth(name) {
                    "  [browser login]"
                } else {
                    ""
                };
                let prefix = if is_sel { "▸ " } else { "  " };
                chrome.push(Line::from(vec![
                    Span::styled(format!("{prefix}{name}{key_mark}{cur_mark}"), if is_sel { selected } else { dim }),
                ]));
            }
        } else if self.show_model_picker {
            let visible = self.picker_visible_height();
            let end = (self.picker_scrl + visible).min(self.picker_models.len());
            let num = self.picker_models.len();

            chrome.push(Line::from(vec![
                Span::styled("── Model ", orange),
                Span::styled("(↑↓ navigate  Enter select  Esc cancel) ──", bold_dim),
            ]));
            if num > visible {
                chrome.push(Line::from(vec![Span::styled(format!("  … {}/{}  ↑↓ scroll", self.picker_sel + 1, num), dim)]));
            }
            for i in self.picker_scrl..end {
                let m = &self.picker_models[i];
                let is_sel = i == self.picker_sel;
                let is_cur = m.id == self.model;
                let ctx = if m.max_ctx > 0 { format!(" ({}K context)", m.max_ctx / 1000) } else { String::new() };
                let check = if is_cur { "  ✓" } else { "" };
                let prefix = if is_sel { "▸ " } else { "  " };
                chrome.push(Line::from(vec![
                    Span::styled(format!("{prefix}{}  {}{ctx}{check}", m.name, m.id), if is_sel { selected } else { dim }),
                ]));
            }
        } else if self.show_theme_picker {
            chrome.push(Line::from(vec![
                Span::styled("── Theme ", orange),
                Span::styled("(↑↓ live-preview  Enter apply  Esc cancel) ──", bold_dim),
            ]));
            for (i, t) in self.theme_picker_list.iter().enumerate() {
                let is_sel = i == self.theme_picker_sel;
                let is_cur = t.name == self.theme.name;
                let cur_mark = if is_cur { "  ✓" } else { "" };
                let prefix = if is_sel { "▸ " } else { "  " };
                chrome.push(Line::from(vec![
                    Span::styled(format!("{prefix}{} ({}){cur_mark}", t.label, t.name), if is_sel { selected } else { dim }),
                ]));
            }
        } else if self.show_session_picker {
            let visible = 10usize;
            let end = (self.picker_session_scrl + visible).min(self.picker_sessions.len());
            let num = self.picker_sessions.len();

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
            chrome.push(Line::from(vec![
                Span::styled(title, orange),
                Span::styled(hint, bold_dim),
            ]));
            if num > visible {
                chrome.push(Line::from(vec![Span::styled(format!("  … {}/{}  ↑↓ scroll", self.picker_session_sel + 1, num), dim)]));
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
                chrome.push(Line::from(vec![
                    Span::styled(format!("{prefix}{}  {}  {} msgs  {time_str}", &s.id[..8], s.model, s.msg_count), if is_sel { selected } else { dim }),
                ]));
                if !summary.is_empty() && is_sel {
                    chrome.push(Line::from(vec![
                        Span::styled(format!("   {summary}"), if is_sel { selected } else { dim }),
                    ]));
                }
            }
        } else if self.show_permission_prompt {
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            let prompt_line_idx = chrome.len();
            chrome.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::styled("▋", orange_fg),
                Span::raw(after),
            ]));
            chrome.push(Line::from(""));
            chrome.push(Line::from(vec![
                Span::styled(format!("Tool '{}' wants to run:", self.perm_tool_name), white),
            ]));
            if !self.perm_tool_input.is_empty() {
                chrome.push(Line::from(vec![
                    Span::styled(format!("  {}", self.perm_tool_input), dim),
                ]));
            }
            chrome.push(Line::from(""));
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
            chrome.push(Line::from(option_spans));
            chrome.push(Line::from(vec![
                Span::styled("(← → navigate  Enter confirm  Esc deny)", dim),
            ]));
            cursor_pos = Some((display_width("❯ ") as u16 + display_width(before) as u16, prompt_line_idx));
        } else if self.show_recovery_prompt {
            chrome.push(Line::from(vec![
                Span::styled(
                    format!("LLM failed ({}/{}). Switch and retry your request:", self.provider, self.model),
                    white,
                ),
            ]));
            chrome.push(Line::from(""));
            let options = ["Switch model (m)", "Switch provider (p)", "Dismiss (d)"];
            let mut option_spans = Vec::new();
            for (i, opt) in options.iter().enumerate() {
                if i > 0 { option_spans.push(Span::raw("  ")); }
                let is_sel = i == self.recovery_selection;
                let open = if is_sel { "[" } else { " " };
                let close = if is_sel { "]" } else { " " };
                option_spans.push(Span::styled(
                    format!("{open}{opt}{close}"),
                    if is_sel { orange_fg.add_modifier(Modifier::BOLD) } else { dim },
                ));
            }
            chrome.push(Line::from(option_spans));
            chrome.push(Line::from(vec![
                Span::styled("(← → navigate  Enter confirm  Esc dismiss)", dim),
            ]));
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            let prompt_line_idx = chrome.len();
            chrome.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::styled("▋", orange_fg),
                Span::raw(after),
            ]));
            cursor_pos = Some((display_width("❯ ") as u16 + display_width(before) as u16, prompt_line_idx));
        } else if self.awaiting_api_key {
            let target = self.api_key_target.as_deref().unwrap_or(&self.provider);
            let env_hint = crate::config::env_var_name(target).unwrap_or("API_KEY");
            let label = format!("{target} API key ({env_hint}) > ");
            let cursor_chars = self.input_buf[..self.cursor.min(self.input_buf.len())]
                .chars()
                .count();
            let masked = crate::config::mask_secret_display(&self.input_buf, 4);
            let masked_chars: Vec<char> = masked.chars().collect();
            let before: String = masked_chars.iter().take(cursor_chars).collect();
            let after: String = masked_chars.iter().skip(cursor_chars).collect();
            chrome.push(Line::from(vec![
                Span::styled(format!("{label}{before}"), orange_fg),
                Span::styled("▋", orange_fg),
                Span::styled(after, orange_fg),
            ]));
            chrome.push(Line::from(vec![
                Span::styled(
                    "Hidden as you type (last 4 characters shown). Enter to save  ·  Esc to cancel",
                    dim,
                ),
            ]));
            cursor_pos = Some((
                display_width(&label) as u16 + display_width(&before) as u16,
                0,
            ));
        } else {
            let cursor = self.cursor.min(self.input_buf.len());
            let (before, after) = self.input_buf.split_at(cursor);
            chrome.push(Line::from(vec![
                Span::styled("❯ ", orange_fg),
                Span::raw(before),
                Span::raw(after),
            ]));
            cursor_pos = Some((display_width("❯ ") as u16 + display_width(before) as u16, 0));
        }

        let width = area.width as usize;
        let body_wrapped = total_wrapped(&lines, width);
        let chrome_wrapped = total_wrapped(&chrome, width).max(1);
        // Keep room for transcript; cap chrome so pickers cannot hide all output.
        let max_chrome = (area.height as usize)
            .saturating_sub(3)
            .min((area.height as usize).saturating_mul(2) / 3)
            .max(1);
        let chrome_h = chrome_wrapped.min(max_chrome) as u16;
        let chrome_scroll = chrome_wrapped.saturating_sub(chrome_h as usize);
        let term_h = area.height as usize;

        // Short chats: sit the composer directly under the transcript (top of the
        // window), not glued to the bottom with a huge empty gap that looks like a
        // second orphaned prompt.
        // Long chats: pin chrome to the bottom and scroll the transcript above.
        let fits = body_wrapped.saturating_add(chrome_h as usize) <= term_h;
        let (body_area, chrome_area) = if fits {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(body_wrapped.max(1) as u16),
                    Constraint::Length(chrome_h),
                    Constraint::Min(0),
                ])
                .split(area);
            (chunks[0], chunks[1])
        } else {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(chrome_h),
                ])
                .split(area);
            (chunks[0], chunks[1])
        };

        // videre-style rowoff: free scroll when not following; pin to bottom when following.
        let body_h = body_area.height as usize;
        let max_off = body_wrapped.saturating_sub(body_h.max(1));
        self.last_body_h = body_h;
        self.last_body_wrapped = body_wrapped;
        let body_scroll = if fits {
            0
        } else if self.transcript_follow {
            self.transcript_rowoff = max_off;
            max_off
        } else {
            let off = self.transcript_rowoff.min(max_off);
            self.transcript_rowoff = off;
            if off >= max_off {
                self.transcript_follow = true;
            }
            off
        };

        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((body_scroll as u16, 0)),
            body_area,
        );
        f.render_widget(
            Paragraph::new(Text::from(chrome.clone()))
                .wrap(Wrap { trim: false })
                .scroll((chrome_scroll as u16, 0)),
            chrome_area,
        );

        // Scroll position hint when not pinned to bottom (videre shows %).
        if !fits && !self.transcript_follow && max_off > 0 {
            let pct = (body_scroll * 100) / max_off;
            let hint = format!(" ↑ {pct}% · PgUp/PgDn · wheel · Ctrl+U/D ");
            let hx = body_area
                .x
                .saturating_add(body_area.width.saturating_sub(hint.len() as u16 + 1));
            let hy = body_area.y;
            if body_area.width > 8 {
                f.render_widget(
                    Paragraph::new(Span::styled(hint, dim)),
                    ratatui::layout::Rect {
                        x: hx,
                        y: hy,
                        width: body_area
                            .width
                            .saturating_sub(hx.saturating_sub(body_area.x))
                            .min(40),
                        height: 1,
                    },
                );
            }
        }

        if let Some((x_off, line_idx)) = cursor_pos {
            let line_idx = line_idx.min(chrome.len().saturating_sub(1));
            let wrapped_before = total_wrapped(&chrome[..line_idx], width);
            let y = (chrome_area.y as usize + wrapped_before).saturating_sub(chrome_scroll) as u16;
            let y = y.min(chrome_area.y.saturating_add(chrome_area.height.saturating_sub(1)));
            let x = chrome_area.x.saturating_add(x_off.min(chrome_area.width.saturating_sub(1)));
            f.set_cursor_position(Position { x, y });
        }
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
        if self.awaiting_api_key || !self.input_buf.starts_with('/') {
            self.show_command_picker = false;
            self.cmd_picker_filtered.clear();
            return;
        }

        let models: Vec<String> = {
            let providers = crate::llm::default_providers();
            providers
                .get(&self.provider)
                .map(|p| p.available_models().into_iter().map(|m| m.id).collect())
                .unwrap_or_default()
        };
        let mut provider_names: Vec<String> = crate::llm::default_providers().into_keys().collect();
        provider_names.sort();
        let themes: Vec<String> = theme::theme_names().iter().map(|s| (*s).to_string()).collect();
        let sessions: Vec<String> = session::list(&self.sessions_dir())
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                if s.id.len() >= 8 {
                    s.id[..8].to_string()
                } else {
                    s.id
                }
            })
            .collect();

        self.cmd_picker_filtered = slash_completions(
            &self.input_buf,
            &self.cmd_picker_list,
            &models,
            &provider_names,
            &themes,
            &sessions,
        );
        if self.cmd_picker_sel >= self.cmd_picker_filtered.len() {
            self.cmd_picker_sel = self.cmd_picker_filtered.len().saturating_sub(1);
        }
        self.show_command_picker = !self.cmd_picker_filtered.is_empty();
    }

    /// Insert a completion into the composer. Adds a trailing space when more
    /// arguments are expected so the next Tab level can open immediately.
    fn apply_slash_completion(&mut self, completion: &str) {
        let text = if completion_wants_trailing_space(completion) {
            format!("{completion} ")
        } else {
            completion.to_string()
        };
        self.input_buf = text;
        self.cursor = self.input_buf.len();
        self.update_cmd_picker();
    }
}

/// Commands that still need another argument after Tab.
fn completion_wants_trailing_space(completion: &str) -> bool {
    let c = completion.trim_end();
    matches!(
        c,
        "/auth"
            | "/auth login"
            | "/auth logout"
            | "/auth key"
            | "/theme"
            | "/model"
            | "/provider"
            | "/resume"
            | "/delete"
    )
}

/// Contextual slash-command completions for the composer.
///
/// Supports base commands, `/auth` subcommands + providers, `/theme` names,
/// `/model` ids for the active provider, `/provider` names, and short session
/// ids for `/resume` / `/delete`.
pub(crate) fn slash_completions(
    input: &str,
    base_commands: &[String],
    models: &[String],
    providers: &[String],
    themes: &[String],
    session_ids: &[String],
) -> Vec<String> {
    if !input.starts_with('/') {
        return Vec::new();
    }
    let ends_with_space = input.ends_with(' ') || input.ends_with('\t');
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() {
        return base_commands.to_vec();
    }

    let cmd = parts[0].to_ascii_lowercase();

    // Still typing the root command: `/mo` → `/model`
    if parts.len() == 1 && !ends_with_space {
        return base_commands
            .iter()
            .filter(|c| c.to_ascii_lowercase().starts_with(&cmd))
            .cloned()
            .collect();
    }

    let prefix_match = |candidates: &[String], typed: &str, format: &dyn Fn(&str) -> String| {
        let typed = typed.to_ascii_lowercase();
        let mut out: Vec<String> = candidates
            .iter()
            .filter(|c| c.to_ascii_lowercase().starts_with(&typed))
            .map(|c| format(c))
            .collect();
        out.sort();
        out.dedup();
        out
    };

    match cmd.as_str() {
        "/auth" => {
            let subs = ["login", "logout", "status", "key"];
            if parts.len() == 1 && ends_with_space {
                return subs.iter().map(|s| format!("/auth {s}")).collect();
            }
            if parts.len() == 2 && !ends_with_space {
                let p = parts[1].to_ascii_lowercase();
                return subs
                    .iter()
                    .filter(|s| s.starts_with(&p))
                    .map(|s| format!("/auth {s}"))
                    .collect();
            }
            if parts.len() >= 2 {
                let action = parts[1].to_ascii_lowercase();
                if matches!(action.as_str(), "login" | "logout" | "key") {
                    if parts.len() == 2 && ends_with_space {
                        return providers
                            .iter()
                            .map(|p| format!("/auth {action} {p}"))
                            .collect();
                    }
                    if parts.len() == 3 && !ends_with_space {
                        return prefix_match(providers, parts[2], &|p| {
                            format!("/auth {action} {p}")
                        });
                    }
                }
            }
            Vec::new()
        }
        "/theme" => {
            if parts.len() == 1 && ends_with_space {
                let mut v: Vec<String> = themes.iter().map(|t| format!("/theme {t}")).collect();
                v.insert(0, "/theme list".into());
                return v;
            }
            if parts.len() == 2 && !ends_with_space {
                let mut v = prefix_match(themes, parts[1], &|t| format!("/theme {t}"));
                if "list".starts_with(&parts[1].to_ascii_lowercase()) {
                    v.insert(0, "/theme list".into());
                }
                v.dedup();
                return v;
            }
            Vec::new()
        }
        "/model" => {
            if parts.len() == 1 && ends_with_space {
                return models.iter().map(|m| format!("/model {m}")).collect();
            }
            if parts.len() >= 2 && !ends_with_space {
                let typed = parts[1..].join(" ");
                return prefix_match(models, &typed, &|m| format!("/model {m}"));
            }
            Vec::new()
        }
        "/provider" => {
            if parts.len() == 1 && ends_with_space {
                return providers.iter().map(|p| format!("/provider {p}")).collect();
            }
            if parts.len() == 2 && !ends_with_space {
                return prefix_match(providers, parts[1], &|p| format!("/provider {p}"));
            }
            Vec::new()
        }
        "/resume" | "/delete" => {
            if parts.len() == 1 && ends_with_space {
                return session_ids
                    .iter()
                    .map(|id| format!("{cmd} {id}"))
                    .collect();
            }
            if parts.len() == 2 && !ends_with_space {
                return prefix_match(session_ids, parts[1], &|id| format!("{cmd} {id}"));
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    fn base() -> Vec<String> {
        vec![
            "/auth".into(),
            "/clear".into(),
            "/help".into(),
            "/model".into(),
            "/provider".into(),
            "/resume".into(),
            "/delete".into(),
            "/theme".into(),
        ]
    }

    #[test]
    fn completes_root_command_prefix() {
        let c = slash_completions("/mo", &base(), &[], &[], &[], &[]);
        assert_eq!(c, vec!["/model".to_string()]);
    }

    #[test]
    fn completes_auth_subcommands() {
        let c = slash_completions("/auth ", &base(), &[], &[], &[], &[]);
        assert!(c.iter().any(|x| x == "/auth login"));
        assert!(c.iter().any(|x| x == "/auth status"));
        let c = slash_completions("/auth lo", &base(), &[], &[], &[], &[]);
        assert_eq!(c, vec!["/auth login".to_string(), "/auth logout".to_string()]);
    }

    #[test]
    fn completes_auth_login_provider() {
        let providers = vec!["anthropic".into(), "xai".into()];
        let c = slash_completions("/auth login ", &base(), &[], &providers, &[], &[]);
        assert!(c.iter().any(|x| x == "/auth login xai"));
        let c = slash_completions("/auth login x", &base(), &[], &providers, &[], &[]);
        assert_eq!(c, vec!["/auth login xai".to_string()]);
    }

    #[test]
    fn completes_models_and_themes() {
        let models = vec!["grok-4.5:high".into(), "grok-4.3".into()];
        let c = slash_completions("/model grok-4.5", &base(), &models, &[], &[], &[]);
        assert_eq!(c, vec!["/model grok-4.5:high".to_string()]);
        let themes = vec!["dark".into(), "dune".into()];
        let c = slash_completions("/theme d", &base(), &[], &[], &themes, &[]);
        assert!(c.iter().any(|x| x == "/theme dark"));
        assert!(c.iter().any(|x| x == "/theme dune"));
    }

    #[test]
    fn completes_session_ids_for_resume() {
        let sessions = vec!["abcdef12".into(), "abcdef99".into(), "deadbeef".into()];
        let c = slash_completions("/resume abc", &base(), &[], &[], &[], &sessions);
        assert_eq!(
            c,
            vec![
                "/resume abcdef12".to_string(),
                "/resume abcdef99".to_string()
            ]
        );
    }

    #[test]
    fn trailing_space_helpers() {
        assert!(completion_wants_trailing_space("/auth login"));
        assert!(!completion_wants_trailing_space("/auth login xai"));
        assert!(!completion_wants_trailing_space("/help"));
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
    // `ts` is absolute unix seconds; show relative age.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now.saturating_sub(ts);
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h ago")
    } else if hours > 0 {
        format!("{hours}h {mins}m ago")
    } else if mins > 0 {
        format!("{mins}m ago")
    } else {
        "just now".into()
    }
}

fn total_wrapped(lines: &[Line], width: usize) -> usize {
    let w = width.max(1);
    lines.iter().map(|l| {
        let line_w: usize = l.spans.iter().map(|s| display_width(&s.content)).sum();
        if line_w == 0 { 1 } else { (line_w + w - 1) / w }
    }).sum()
}

/// Limit on-screen tool output by line count (head + tail) so long shell
/// dumps stay readable and keep the trailing summary / exit code.
fn truncate_display(s: &str, head_lines: usize, tail_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= head_lines + tail_lines {
        return s.to_string();
    }
    let mut out = String::new();
    for line in &lines[..head_lines] {
        out.push_str(line);
        out.push('\n');
    }
    let omitted = lines.len() - head_lines - tail_lines;
    out.push_str(&format!("  … ({omitted} lines omitted) …\n"));
    for line in &lines[lines.len() - tail_lines..] {
        out.push_str(line);
        out.push('\n');
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}


