use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use super::registry::Tool;

pub struct ShellTool;

impl Tool for ShellTool {
    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str { "Execute a shell command" }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"}},"required":["command"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let cmd = obj.get("command").and_then(|v| v.as_str()).ok_or("command required")?;
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

        let output = child.wait_with_output().map_err(|e| format!("exec error: {e}"))?;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() { result.push('\n'); }
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if !output.status.success() && output.stdout.is_empty() {
            return Err(format!("exit code: {}", output.status.code().unwrap_or(-1)));
        }

        if result.len() > 5000 {
            result = result[..5000].to_string();
            result.push_str("\n... [truncated]");
        }

        Ok(result)
    }
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
    }

    #[test]
    fn test_generous_timeout_does_not_interrupt_fast_command() {
        let tool = ShellTool;
        let input = r#"{"command":"echo hi","timeout":30000}"#;
        let out = tool.execute(input).unwrap();
        assert!(out.contains("hi"), "unexpected output: {out}");
    }
}
