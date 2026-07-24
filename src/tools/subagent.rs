//! External harness subagents: run Claude Code, AGY, Grok Build, Zero, or a
//! user-configured CLI headlessly and return bounded stdout/stderr.
//!
//! This is not in-process subagents (worktrees, personas). The child is a
//! separate binary already on PATH.

use super::process_runner::{self, with_cleanup, RunError, RunOptions};
use super::registry::Tool;
use crate::config::{HarnessConfig, HarnessPromptMode, SubagentConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const MAX_OUTPUT_CHARS: usize = 12_000;
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 4_000;
const DEFAULT_TIMEOUT_MS: u64 = 600_000;

pub struct SubagentTool {
    config: SubagentConfig,
}

impl SubagentTool {
    pub fn new(config: SubagentConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &SubagentConfig {
        &self.config
    }
}

impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Run a headless external coding harness (claude, agy, grok, zero, or a \
         config-defined name) with a full task prompt. Use to delegate research \
         or implementation to another agent CLI. Requires permission. Returns \
         bounded stdout/stderr and the exit code. Does not stream the child TUI."
    }

    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"harness":{"type":"string","description":"Harness name: claude, agy, grok, zero, or a custom name from config.subagents.harnesses"},"prompt":{"type":"string","description":"Full task for the child harness"},"timeout_ms":{"type":"integer","description":"Wall-clock timeout in milliseconds (default 600000)"},"cwd":{"type":"string","description":"Working directory (default: current workspace)"},"extra_args":{"type":"array","items":{"type":"string"},"description":"Extra argv appended after the harness template args"}},"required":["harness","prompt"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        self.execute_with_cancel(input, &AtomicBool::new(false))
    }

    fn execute_with_cancel(&self, input: &str, cancel: &AtomicBool) -> Result<String, String> {
        if !self.config.is_enabled() {
            return Err(
                "Subagents are disabled (config subagents.enabled=false or CAIRN_SUBAGENTS=0)."
                    .into(),
            );
        }
        let req = parse_request(input)?;
        let harness = resolve_harness(&self.config, &req.harness)?;
        run_harness(
            &req.harness,
            &harness,
            &req.prompt,
            req.timeout_ms
                .or(harness.timeout_ms)
                .unwrap_or(self.config.default_timeout_ms),
            req.cwd.as_deref(),
            &req.extra_args,
            Some(cancel),
        )
    }
}

#[derive(Debug)]
struct RunRequest {
    harness: String,
    prompt: String,
    timeout_ms: Option<u64>,
    cwd: Option<String>,
    extra_args: Vec<String>,
}

fn parse_request(input: &str) -> Result<RunRequest, String> {
    let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
    let obj = val.as_object().ok_or("expected object")?;
    let harness = obj
        .get("harness")
        .and_then(|v| v.as_str())
        .ok_or("harness required")?
        .trim()
        .to_string();
    if harness.is_empty() {
        return Err("harness must be non-empty".into());
    }
    let prompt = obj
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or("prompt required")?
        .to_string();
    if prompt.trim().is_empty() {
        return Err("prompt must be non-empty".into());
    }
    let timeout_ms = obj.get("timeout_ms").and_then(|v| v.as_u64());
    let cwd = obj
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let extra_args = obj
        .get("extra_args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(RunRequest {
        harness,
        prompt,
        timeout_ms,
        cwd,
        extra_args,
    })
}

/// Builtin headless templates (config overrides replace these by name).
pub fn builtin_harnesses() -> Vec<(String, HarnessConfig)> {
    vec![
        (
            "claude".into(),
            HarnessConfig {
                command: "claude".into(),
                // -p/--print: non-interactive one-shot. Do not default
                // --dangerously-skip-permissions.
                args: vec!["-p".into()],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: None,
            },
        ),
        (
            "agy".into(),
            HarnessConfig {
                command: "agy".into(),
                args: vec!["-p".into()],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: None,
            },
        ),
        (
            "grok".into(),
            HarnessConfig {
                command: "grok".into(),
                // -p / --single: single-turn headless prompt.
                args: vec!["-p".into()],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: None,
            },
        ),
        (
            "zero".into(),
            HarnessConfig {
                command: "zero".into(),
                args: vec!["exec".into()],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: None,
            },
        ),
    ]
}

pub fn resolve_harness(cfg: &SubagentConfig, name: &str) -> Result<HarnessConfig, String> {
    let key = name.trim();
    if key.is_empty() {
        return Err("harness name is empty".into());
    }
    // Case-insensitive match on config keys and builtins.
    if let Some((_, h)) = cfg
        .harnesses
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
    {
        return Ok(h.clone());
    }
    if let Some((_, h)) = builtin_harnesses()
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
    {
        return Ok(h);
    }
    let mut names: Vec<String> = builtin_harnesses()
        .into_iter()
        .map(|(n, _)| n)
        .chain(cfg.harnesses.keys().cloned())
        .collect();
    names.sort();
    names.dedup();
    Err(format!(
        "Unknown harness {name:?}. Available: {}. Add custom harnesses under config.subagents.harnesses.",
        names.join(", ")
    ))
}

/// Merged list for `/subagent list`: builtins first, then config-only names.
pub fn list_harnesses(cfg: &SubagentConfig) -> Vec<(String, HarnessConfig, &'static str)> {
    let mut out: Vec<(String, HarnessConfig, &'static str)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, h) in builtin_harnesses() {
        let source = if cfg.harnesses.keys().any(|k| k.eq_ignore_ascii_case(&name)) {
            "config"
        } else {
            "builtin"
        };
        let harness = if source == "config" {
            resolve_harness(cfg, &name).unwrap_or(h)
        } else {
            h
        };
        seen.insert(name.to_ascii_lowercase());
        out.push((name, harness, source));
    }
    let mut extra: Vec<_> = cfg.harnesses.iter().collect();
    extra.sort_by(|a, b| a.0.cmp(b.0));
    for (name, h) in extra {
        if seen.insert(name.to_ascii_lowercase()) {
            out.push((name.clone(), h.clone(), "config"));
        }
    }
    out
}

pub fn binary_on_path(command: &str) -> bool {
    which_bin(command).is_some()
}

fn which_bin(command: &str) -> Option<PathBuf> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }
    let path = Path::new(cmd);
    if path.is_absolute() || cmd.contains('/') || cmd.contains('\\') {
        return path.is_file().then(|| path.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let with_exe = dir.join(format!("{cmd}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
        }
    }
    None
}

pub fn run_harness(
    name: &str,
    harness: &HarnessConfig,
    prompt: &str,
    timeout_ms: u64,
    cwd: Option<&str>,
    extra_args: &[String],
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("prompt must be non-empty".into());
    }
    if which_bin(&harness.command).is_none() {
        return Err(format!(
            "Harness binary {:?} not found on PATH (or is not a file). Install it or set config.subagents.harnesses.{name}.command.",
            harness.command
        ));
    }

    let mut command = Command::new(&harness.command);
    command.args(&harness.args);
    command.args(extra_args);

    let mut stdin_bytes = None;
    match harness.prompt {
        HarnessPromptMode::Arg => {
            command.arg(prompt);
        }
        HarnessPromptMode::Stdin => {
            stdin_bytes = Some(prompt.as_bytes().to_vec());
        }
    }

    if let Some(dir) = cwd {
        let path = PathBuf::from(dir);
        if !path.is_dir() {
            return Err(format!("cwd is not a directory: {dir}"));
        }
        command.current_dir(path);
    }

    let timeout = if timeout_ms == 0 {
        Duration::from_millis(DEFAULT_TIMEOUT_MS)
    } else {
        Duration::from_millis(timeout_ms)
    };
    let options = RunOptions {
        timeout: Some(timeout),
        head_chars: HEAD_CHARS,
        tail_chars: TAIL_CHARS,
        stdin: stdin_bytes,
    };

    let started = Instant::now();
    let result = match process_runner::run(command, &options, cancel) {
        Ok(r) => r,
        Err(error) => return Err(format_run_error(error, timeout_ms)),
    };
    let elapsed = started.elapsed();

    let stdout = result.stdout.trim_end().to_string();
    let stderr = result.stderr.trim_end().to_string();
    let mut body = format!(
        "Subagent harness={name} exit={} duration={:.1}s binary={}\n",
        result.code,
        elapsed.as_secs_f64(),
        harness.command
    );
    if !stdout.is_empty() {
        body.push_str("--- stdout ---\n");
        body.push_str(&stdout);
        body.push('\n');
    }
    if !stderr.is_empty() {
        body.push_str("--- stderr ---\n");
        body.push_str(&stderr);
        body.push('\n');
    }
    if stdout.is_empty() && stderr.is_empty() {
        body.push_str("(no output)\n");
    }

    let body = truncate_head_tail(&body, MAX_OUTPUT_CHARS, HEAD_CHARS, TAIL_CHARS);
    if result.success {
        Ok(body)
    } else {
        Err(format!(
            "{body}Harness exited with code {}.",
            result.code
        ))
    }
}

fn format_run_error(error: RunError, timeout_ms: u64) -> String {
    match error {
        RunError::Spawn(e) => format!("Failed to start harness: {e}"),
        RunError::TimedOut {
            after_ms,
            cleanup_error,
        } => with_cleanup(
            format!(
                "Harness timed out after {}ms (limit {}ms).",
                after_ms,
                if timeout_ms == 0 {
                    DEFAULT_TIMEOUT_MS
                } else {
                    timeout_ms
                }
            ),
            &cleanup_error,
        ),
        RunError::Cancelled { cleanup_error } => {
            with_cleanup("Harness cancelled.".into(), &cleanup_error)
        }
        RunError::Wait {
            reason,
            cleanup_error,
        } => with_cleanup(format!("Harness wait failed: {reason}"), &cleanup_error),
    }
}

fn truncate_head_tail(s: &str, max: usize, head: usize, tail: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let head = head.min(chars.len());
    let tail = tail.min(chars.len().saturating_sub(head));
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    let omitted = chars.len() - head - tail;
    format!("{start}\n... [{omitted} chars truncated] ...\n{end}")
}

/// Format `/subagent list` text.
pub fn format_list(cfg: &SubagentConfig) -> String {
    if !cfg.is_enabled() {
        return "Subagents are disabled (config subagents.enabled=false or CAIRN_SUBAGENTS=0)."
            .into();
    }
    let rows = list_harnesses(cfg);
    let mut out = String::from("External harness subagents:\n");
    out.push_str("  name     binary     on PATH  source\n");
    for (name, h, source) in rows {
        let on_path = if binary_on_path(&h.command) {
            "yes"
        } else {
            "no"
        };
        out.push_str(&format!(
            "  {:<8} {:<10} {:<8} {source}\n",
            name, h.command, on_path
        ));
    }
    out.push_str(
        "\nUsage: /subagent <harness> <prompt…>\n\
         Tool:  subagent {\"harness\":\"claude\",\"prompt\":\"…\"}\n\
         Config: subagents.harnesses.<name> = { command, args, prompt }\n\
         Note: headless only; child permission UIs can hang until timeout.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SubagentConfig;
    use std::collections::HashMap;

    #[test]
    fn builtins_resolve() {
        let cfg = SubagentConfig::default();
        for name in ["claude", "agy", "grok", "zero", "CLAUDE"] {
            let h = resolve_harness(&cfg, name).unwrap();
            assert!(!h.command.is_empty(), "{name}");
        }
    }

    #[test]
    fn config_override_wins() {
        let mut cfg = SubagentConfig::default();
        cfg.harnesses.insert(
            "claude".into(),
            HarnessConfig {
                command: "my-claude".into(),
                args: vec!["--print".into()],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: Some(1_000),
            },
        );
        let h = resolve_harness(&cfg, "claude").unwrap();
        assert_eq!(h.command, "my-claude");
        assert_eq!(h.args, vec!["--print"]);
        assert_eq!(h.timeout_ms, Some(1_000));
    }

    #[test]
    fn custom_harness_from_config() {
        let mut cfg = SubagentConfig::default();
        cfg.harnesses.insert(
            "reviewer".into(),
            HarnessConfig {
                command: "claude".into(),
                args: vec!["-p".into(), "--agent".into(), "reviewer".into()],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: None,
            },
        );
        let h = resolve_harness(&cfg, "reviewer").unwrap();
        assert_eq!(h.args.len(), 3);
    }

    #[test]
    fn unknown_harness_lists_available() {
        let cfg = SubagentConfig::default();
        let err = resolve_harness(&cfg, "nope").unwrap_err();
        assert!(err.contains("claude"));
        assert!(err.contains("Unknown"));
    }

    #[test]
    fn parse_rejects_empty_prompt() {
        let err = parse_request(r#"{"harness":"claude","prompt":"  "}"#).unwrap_err();
        assert!(err.contains("prompt"));
    }

    #[test]
    fn parse_accepts_extra_args() {
        let r = parse_request(
            r#"{"harness":"claude","prompt":"hi","extra_args":["--model","x"],"timeout_ms":1000}"#,
        )
        .unwrap();
        assert_eq!(r.extra_args, vec!["--model", "x"]);
        assert_eq!(r.timeout_ms, Some(1000));
    }

    #[test]
    fn run_echo_harness_via_config() {
        // Use a tiny custom harness that is always on PATH: `echo`.
        let mut cfg = SubagentConfig::default();
        cfg.harnesses.insert(
            "echo".into(),
            HarnessConfig {
                command: "echo".into(),
                args: vec![],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: Some(5_000),
            },
        );
        let tool = SubagentTool::new(cfg);
        let out = tool
            .execute(r#"{"harness":"echo","prompt":"hello-subagent"}"#)
            .unwrap();
        assert!(out.contains("hello-subagent"), "{out}");
        assert!(out.contains("harness=echo"), "{out}");
        assert!(out.contains("exit=0"), "{out}");
    }

    #[test]
    fn run_stdin_harness() {
        let mut cfg = SubagentConfig::default();
        // `cat` echoes stdin.
        cfg.harnesses.insert(
            "cat".into(),
            HarnessConfig {
                command: "cat".into(),
                args: vec![],
                prompt: HarnessPromptMode::Stdin,
                timeout_ms: Some(5_000),
            },
        );
        let tool = SubagentTool::new(cfg);
        let out = tool
            .execute(r#"{"harness":"cat","prompt":"from-stdin"}"#)
            .unwrap();
        assert!(out.contains("from-stdin"), "{out}");
    }

    #[test]
    fn missing_binary_errors_cleanly() {
        let mut cfg = SubagentConfig::default();
        cfg.harnesses.insert(
            "ghost".into(),
            HarnessConfig {
                command: "cairn-definitely-not-a-real-binary-xyz".into(),
                args: vec![],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: Some(1_000),
            },
        );
        let tool = SubagentTool::new(cfg);
        let err = tool
            .execute(r#"{"harness":"ghost","prompt":"hi"}"#)
            .unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn disabled_tool_errors() {
        let mut cfg = SubagentConfig::default();
        cfg.enabled = false;
        let tool = SubagentTool::new(cfg);
        let err = tool
            .execute(r#"{"harness":"claude","prompt":"hi"}"#)
            .unwrap_err();
        assert!(err.contains("disabled"), "{err}");
    }

    #[test]
    fn format_list_mentions_usage() {
        let text = format_list(&SubagentConfig::default());
        assert!(text.contains("claude"));
        assert!(text.contains("/subagent"));
    }

    #[test]
    fn subagent_config_from_json() {
        let raw = r#"{
            "enabled": true,
            "default_timeout_ms": 120000,
            "harnesses": {
                "reviewer": {
                    "command": "claude",
                    "args": ["-p", "--agent", "reviewer"],
                    "prompt": "stdin",
                    "timeout_ms": 900000
                }
            }
        }"#;
        let val = crate::json::parse(raw).unwrap();
        let obj = val.as_object().unwrap();
        let cfg = SubagentConfig::from_json_obj(obj);
        assert_eq!(cfg.default_timeout_ms, 120_000);
        let h = cfg.harnesses.get("reviewer").unwrap();
        assert_eq!(h.prompt, HarnessPromptMode::Stdin);
        assert_eq!(h.timeout_ms, Some(900_000));
    }

    #[test]
    fn list_merges_custom_names() {
        let mut cfg = SubagentConfig::default();
        cfg.harnesses.insert(
            "reviewer".into(),
            HarnessConfig {
                command: "claude".into(),
                args: vec![],
                prompt: HarnessPromptMode::Arg,
                timeout_ms: None,
            },
        );
        let names: Vec<_> = list_harnesses(&cfg).into_iter().map(|(n, _, _)| n).collect();
        assert!(names.contains(&"claude".into()));
        assert!(names.contains(&"reviewer".into()));
    }

    // Silence unused import warning in some toolchains.
    #[allow(dead_code)]
    fn _hashmap_typecheck() {
        let _: HashMap<String, HarnessConfig> = HashMap::new();
    }
}
