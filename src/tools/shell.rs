use super::registry::Tool;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Max chars returned to the model. Prefer head+tail so summaries
/// (e.g. `cargo test` "147 passed") survive even when the middle is huge.
const MAX_OUTPUT_CHARS: usize = 12_000;
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 4_000;

pub struct ShellTool;

impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }
    fn description(&self) -> &str {
        "Execute a shell command. On Windows this uses PowerShell (-Command); \
         on Unix it uses bash (-c). For intentional PowerShell work on Windows, \
         prefer the dedicated `powershell` tool. Always check the exit code footer."
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"}},"required":["command"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let cmd = obj
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("command required")?;
        let timeout_ms = obj.get("timeout").and_then(|v| v.as_u64());

        let shell = if cfg!(windows) { "powershell" } else { "bash" };
        let flag = if cfg!(windows) { "-Command" } else { "-c" };

        let mut child = Command::new(shell)
            .arg(flag)
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("exec error: {e}"))?;

        if let Some(ms) = timeout_ms {
            let deadline = Instant::now() + Duration::from_millis(ms);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(format!("command timed out after {ms}ms"));
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => return Err(format!("exec error: {e}")),
                }
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("exec error: {e}"))?;
        let code = output.status.code().unwrap_or(-1);
        let ok = output.status.success();

        let stdout = normalize_cli_output(&String::from_utf8_lossy(&output.stdout));
        let stderr = normalize_cli_output(&String::from_utf8_lossy(&output.stderr));

        let mut body = String::new();
        if !stdout.is_empty() {
            body.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !body.is_empty() && !body.ends_with('\n') {
                body.push('\n');
            }
            // Label stderr so the model can tell streams apart.
            if !stdout.is_empty() {
                body.push_str("--- stderr ---\n");
            }
            body.push_str(&stderr);
        }

        let body = truncate_head_tail(&body, MAX_OUTPUT_CHARS, HEAD_CHARS, TAIL_CHARS);

        // Always surface exit code. Non-zero used to return a bare "exit code: N"
        // when stdout was empty (e.g. missing `tail` on Windows), and when stdout
        // was non-empty the failure was silent — both confuse the agent.
        let mut result = body;
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("(exit code {code})"));

        if ok {
            Ok(result)
        } else {
            // Prefix so the TUI marks it red; keep full body so the model can recover.
            Err(format!("exit code {code}\n{result}"))
        }
    }
}

/// Turn CR progress rewrites (`cargo`'s `\r`) into real newlines and normalize CRLF.
fn normalize_cli_output(s: &str) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    // Collapse huge runs of blank lines from progress spam.
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0usize;
    for line in s.split('\n') {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Keep the beginning and end of large output so status lines at the bottom
/// (test summaries, build results) are not chopped off.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sleep_command(seconds: u64) -> String {
        if cfg!(windows) {
            format!("Start-Sleep -Seconds {seconds}")
        } else {
            format!("sleep {seconds}")
        }
    }

    #[test]
    fn test_timeout_kills_long_running_command() {
        let tool = ShellTool;
        let cmd = sleep_command(5);
        let input = format!(r#"{{"command":"{cmd}","timeout":300}}"#);
        let err = tool.execute(&input).unwrap_err();
        assert!(err.contains("timed out"), "unexpected error: {err}");
    }

    #[test]
    fn test_no_timeout_runs_normally() {
        let tool = ShellTool;
        let cmd = if cfg!(windows) { "echo hi" } else { "echo hi" };
        let input = format!(r#"{{"command":"{cmd}"}}"#);
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("hi"), "unexpected output: {out}");
        assert!(out.contains("(exit code 0)"), "missing exit footer: {out}");
    }

    #[test]
    fn test_generous_timeout_does_not_interrupt_fast_command() {
        let tool = ShellTool;
        let input = r#"{"command":"echo hi","timeout":30000}"#;
        let out = tool.execute(input).unwrap();
        assert!(out.contains("hi"), "unexpected output: {out}");
    }

    #[test]
    fn normalize_turns_cr_into_newlines() {
        let s = normalize_cli_output("a\rb\r\nc");
        assert!(s.contains('\n'));
        assert!(!s.contains('\r'));
        assert!(s.contains('a') && s.contains('b') && s.contains('c'));
    }

    #[test]
    fn truncate_keeps_head_and_tail() {
        let s = format!("{}{}{}", "H".repeat(100), "M".repeat(5000), "T".repeat(100));
        let out = truncate_head_tail(&s, 500, 100, 100);
        assert!(out.starts_with(&"H".repeat(100)));
        assert!(out.ends_with(&"T".repeat(100)));
        assert!(out.contains("truncated"));
        assert!(!out.contains(&"M".repeat(50)));
    }

    #[test]
    fn failed_command_includes_body_not_bare_code() {
        let tool = ShellTool;
        let cmd = if cfg!(windows) {
            "Write-Output 'visible-fail-body'; exit 7"
        } else {
            "echo visible-fail-body; exit 7"
        };
        let input = format!(r#"{{"command":"{cmd}"}}"#);
        let err = tool.execute(&input).unwrap_err();
        assert!(
            err.contains("visible-fail-body"),
            "lost stdout on failure: {err}"
        );
        assert!(err.contains("exit code"), "missing exit code: {err}");
    }
}
