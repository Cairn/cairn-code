//! External harness subagents: run Claude Code, AGY, Grok Build, Zero, or a
//! user-configured CLI headlessly and return bounded stdout/stderr.
//!
//! The child is a separate binary on PATH (not an in-process Cairn session).
//! By default each run gets a **git worktree** under `.cairn/worktrees/` so
//! the harness edits an isolated branch instead of the parent working tree.

use super::process_runner::{self, with_cleanup, RunError, RunOptions};
use super::registry::Tool;
use crate::config::{HarnessConfig, HarnessPromptMode, SubagentConfig, SubagentIsolation};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
         config-defined name) with a full task prompt. Default isolation is a \
         git worktree under .cairn/worktrees/ so the child edits a separate \
         branch. Set isolation to \"none\" to use the parent tree (or pass cwd). \
         Requires permission. Returns bounded stdout/stderr, exit code, and \
         worktree path when applicable. Does not stream the child TUI."
    }

    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"harness":{"type":"string","description":"Harness name: claude, agy, grok, zero, or a custom name from config.subagents.harnesses"},"prompt":{"type":"string","description":"Full task for the child harness"},"isolation":{"type":"string","description":"none | worktree (default from config, usually worktree)"},"timeout_ms":{"type":"integer","description":"Wall-clock timeout in milliseconds (default 600000)"},"cwd":{"type":"string","description":"Any directory this process can access; mutually exclusive with isolation=worktree. Not confined to the workspace root."},"extra_args":{"type":"array","items":{"type":"string"},"description":"Extra argv after the harness template. Fully model-controlled after permission (can pass flags like --dangerously-skip-permissions if the harness accepts them)"}},"required":["harness","prompt"]}"#.into()
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
        let req = parse_request(input, self.config.default_isolation)?;
        let harness = resolve_harness(&self.config, &req.harness)?;
        run_subagent(
            &req.harness,
            &harness,
            &req.prompt,
            req.timeout_ms
                .or(harness.timeout_ms)
                .unwrap_or(self.config.default_timeout_ms),
            req.isolation,
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
    isolation: SubagentIsolation,
    cwd: Option<String>,
    extra_args: Vec<String>,
}

fn parse_request(
    input: &str,
    default_isolation: SubagentIsolation,
) -> Result<RunRequest, String> {
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
    let isolation = match obj.get("isolation").and_then(|v| v.as_str()) {
        Some(s) => SubagentIsolation::parse(s)
            .ok_or_else(|| format!("invalid isolation {s:?}; use none or worktree"))?,
        None => default_isolation,
    };
    let cwd = obj
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if isolation == SubagentIsolation::Worktree && cwd.is_some() {
        return Err(
            "isolation=worktree is mutually exclusive with cwd; omit cwd or set isolation=none"
                .into(),
        );
    }
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
        isolation,
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

/// Created worktree for an isolated subagent run (left on disk for review).
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub repo_root: PathBuf,
}

static WORKTREE_SEQ: AtomicU64 = AtomicU64::new(1);

fn unique_worktree_id() -> String {
    let n = WORKTREE_SEQ.fetch_add(1, Ordering::Relaxed);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs:x}-{n:x}")
}

fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    let options = RunOptions {
        timeout: Some(Duration::from_secs(120)),
        head_chars: HEAD_CHARS,
        tail_chars: TAIL_CHARS,
        stdin: None,
    };
    let result = process_runner::run(command, &options, None).map_err(|e| match e {
        RunError::Spawn(s) => format!("git failed to start: {s}"),
        RunError::TimedOut { .. } => "git timed out".into(),
        RunError::Cancelled { .. } => "git cancelled".into(),
        RunError::Wait { reason, .. } => format!("git wait failed: {reason}"),
    })?;
    if !result.success {
        let mut msg = format!("git {} failed (exit {})", args.join(" "), result.code);
        let body = result.stderr.trim();
        if body.is_empty() {
            let out = result.stdout.trim();
            if !out.is_empty() {
                msg.push_str(": ");
                msg.push_str(out);
            }
        } else {
            msg.push_str(": ");
            msg.push_str(body);
        }
        return Err(msg);
    }
    Ok(result.stdout.trim().to_string())
}

/// Create an isolated git worktree under `<repo>/.cairn/worktrees/<id>`.
pub fn create_worktree(base_cwd: &Path) -> Result<WorktreeInfo, String> {
    let repo_root = git_capture(base_cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|e| {
            format!(
                "isolation=worktree requires a git repository ({e}). \
                 Use isolation=none or run from a git checkout."
            )
        })?;
    let repo_root = PathBuf::from(repo_root);
    let id = unique_worktree_id();
    let branch = format!("cairn/subagent-{id}");
    let path = repo_root.join(".cairn").join("worktrees").join(&id);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        // Keep worktree contents out of the main index if someone stages .cairn.
        let gi = parent.join(".gitignore");
        if !gi.exists() {
            let _ = fs::write(&gi, "*\n!.gitignore\n");
        }
    }

    // New branch at current HEAD so the child can commit without touching main.
    git_capture(
        &repo_root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            "HEAD",
        ],
    )?;

    Ok(WorktreeInfo {
        path,
        branch,
        repo_root,
    })
}

/// High-level entry: optional worktree isolation, then harness run.
pub fn run_subagent(
    name: &str,
    harness: &HarnessConfig,
    prompt: &str,
    timeout_ms: u64,
    isolation: SubagentIsolation,
    cwd: Option<&str>,
    extra_args: &[String],
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    if isolation == SubagentIsolation::Worktree && cwd.is_some() {
        return Err(
            "isolation=worktree is mutually exclusive with cwd; omit cwd or set isolation=none"
                .into(),
        );
    }

    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let worktree = match isolation {
        SubagentIsolation::None => None,
        SubagentIsolation::Worktree => Some(create_worktree(&base)?),
    };
    let run_cwd = match (&worktree, cwd) {
        (Some(wt), _) => Some(wt.path.to_string_lossy().into_owned()),
        (None, Some(c)) => Some(c.to_string()),
        (None, None) => None,
    };

    let mut body = match run_harness(
        name,
        harness,
        prompt,
        timeout_ms,
        run_cwd.as_deref(),
        extra_args,
        cancel,
    ) {
        Ok(s) => s,
        Err(e) => {
            if let Some(wt) = &worktree {
                return Err(format!(
                    "{e}\n\nisolation=worktree path={} branch={}\n\
                     Worktree was kept for inspection. Remove with:\n\
                       git -C {} worktree remove {}\n\
                       git -C {} branch -D {}",
                    wt.path.display(),
                    wt.branch,
                    wt.repo_root.display(),
                    wt.path.display(),
                    wt.repo_root.display(),
                    wt.branch
                ));
            }
            return Err(e);
        }
    };

    if let Some(wt) = worktree {
        let header = format!(
            "isolation=worktree path={} branch={}\n\
             Child edits are on this branch only. Review, then merge or discard:\n\
               git -C {} worktree list\n\
               git -C {} worktree remove {}\n\
               git -C {} branch -D {}\n\n",
            wt.path.display(),
            wt.branch,
            wt.repo_root.display(),
            wt.repo_root.display(),
            wt.path.display(),
            wt.repo_root.display(),
            wt.branch
        );
        body = format!("{header}{body}");
    } else {
        body = format!("isolation=none\n{body}");
    }
    Ok(body)
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
    out.push_str(&format!(
        "\nDefault isolation: {} (git worktree under .cairn/worktrees/)\n\
         Usage: /subagent [worktree|none] <harness> <prompt…>\n\
         Tool:  subagent {{\"harness\":\"claude\",\"prompt\":\"…\",\"isolation\":\"worktree\"}}\n\
         Config: subagents.default_isolation, subagents.harnesses.<name>\n\
         Note: headless only; child permission UIs can hang until timeout.\n\
         Worktrees are kept after the run for review (not auto-removed).",
        cfg.default_isolation.as_str()
    ));
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
        let err = parse_request(
            r#"{"harness":"claude","prompt":"  "}"#,
            SubagentIsolation::Worktree,
        )
        .unwrap_err();
        assert!(err.contains("prompt"));
    }

    #[test]
    fn parse_accepts_extra_args() {
        let r = parse_request(
            r#"{"harness":"claude","prompt":"hi","extra_args":["--model","x"],"timeout_ms":1000}"#,
            SubagentIsolation::Worktree,
        )
        .unwrap();
        assert_eq!(r.extra_args, vec!["--model", "x"]);
        assert_eq!(r.timeout_ms, Some(1000));
        assert_eq!(r.isolation, SubagentIsolation::Worktree);
    }

    #[test]
    fn parse_isolation_and_cwd_mutex() {
        let err = parse_request(
            r#"{"harness":"claude","prompt":"hi","isolation":"worktree","cwd":"/tmp"}"#,
            SubagentIsolation::None,
        )
        .unwrap_err();
        assert!(err.contains("mutually exclusive"), "{err}");
        let r = parse_request(
            r#"{"harness":"claude","prompt":"hi","isolation":"none"}"#,
            SubagentIsolation::Worktree,
        )
        .unwrap();
        assert_eq!(r.isolation, SubagentIsolation::None);
    }

    #[test]
    fn create_worktree_in_temp_repo() {
        let root = std::env::temp_dir().join(format!(
            "cairn-wt-test-{}-{}",
            std::process::id(),
            unique_worktree_id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git_capture(&root, &["init"]).unwrap();
        git_capture(&root, &["config", "user.email", "test@example.com"]).unwrap();
        git_capture(&root, &["config", "user.name", "test"]).unwrap();
        fs::write(root.join("README"), "hi").unwrap();
        git_capture(&root, &["add", "README"]).unwrap();
        git_capture(&root, &["commit", "-m", "init"]).unwrap();

        let wt = create_worktree(&root).unwrap();
        assert!(wt.path.is_dir(), "{:?}", wt.path);
        assert!(wt.branch.starts_with("cairn/subagent-"));
        assert!(wt.path.join("README").is_file());

        // Cleanup
        let _ = git_capture(
            &root,
            &[
                "worktree",
                "remove",
                "--force",
                &wt.path.to_string_lossy(),
            ],
        );
        let _ = git_capture(&root, &["branch", "-D", &wt.branch]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn run_subagent_none_isolation_echo() {
        let mut cfg = SubagentConfig::default();
        cfg.default_isolation = SubagentIsolation::None;
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
            .execute(r#"{"harness":"echo","prompt":"iso-none","isolation":"none"}"#)
            .unwrap();
        assert!(out.contains("iso-none"), "{out}");
        assert!(out.contains("isolation=none"), "{out}");
    }

    #[test]
    fn run_echo_harness_via_config() {
        // Use a tiny custom harness that is always on PATH: `echo`.
        // isolation=none so unit tests do not create real repo worktrees.
        let mut cfg = SubagentConfig::default();
        cfg.default_isolation = SubagentIsolation::None;
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
            .execute(r#"{"harness":"echo","prompt":"hello-subagent","isolation":"none"}"#)
            .unwrap();
        assert!(out.contains("hello-subagent"), "{out}");
        assert!(out.contains("harness=echo"), "{out}");
        assert!(out.contains("exit=0"), "{out}");
    }

    #[test]
    fn run_stdin_harness() {
        let mut cfg = SubagentConfig::default();
        cfg.default_isolation = SubagentIsolation::None;
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
            .execute(r#"{"harness":"cat","prompt":"from-stdin","isolation":"none"}"#)
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
