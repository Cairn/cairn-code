//! Native PowerShell tool. Prefer this on Windows instead of shelling through
//! bash/git-bash style commands. Resolves `pwsh` first, then Windows PowerShell 5.1.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::registry::Tool;

const MAX_OUTPUT_CHARS: usize = 12_000;
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 4_000;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct PowerShellTool;

impl Tool for PowerShellTool {
    fn name(&self) -> &str {
        "powershell"
    }

    fn description(&self) -> &str {
        "Run a PowerShell command natively (pwsh if available, else Windows PowerShell). \
         Use PowerShell syntax (Select-Object, Get-ChildItem, Select-String, etc.) — not bash. \
         Prefer this over shell for Windows-native work. Check the exit code footer."
    }

    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"command":{"type":"string","description":"PowerShell command or script block text"},"timeout":{"type":"integer","description":"Timeout in milliseconds (default 120000)"}},"required":["command"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let command = obj
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("command required")?;
        let timeout_ms = obj
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        let (bin, flag) = resolve_powershell()?;
        let mut cmd = Command::new(&bin);
        // -NoProfile keeps startup fast and avoids user-profile side effects.
        cmd.arg("-NoProfile")
            .arg("-NonInteractive")
            .arg(flag)
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("powershell exec error ({bin}): {e}"))?;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!("powershell timed out after {timeout_ms}ms"));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("powershell exec error: {e}")),
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("powershell exec error: {e}"))?;
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
            if !stdout.is_empty() {
                body.push_str("--- stderr ---\n");
            }
            body.push_str(&stderr);
        }

        let body = truncate_head_tail(&body, MAX_OUTPUT_CHARS, HEAD_CHARS, TAIL_CHARS);
        let mut result = body;
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("(exit code {code})"));

        if ok {
            Ok(result)
        } else {
            Err(format!("exit code {code}\n{result}"))
        }
    }
}

/// Prefer PowerShell 7+ (`pwsh`), then Windows PowerShell 5.1 (`powershell`).
fn resolve_powershell() -> Result<(String, &'static str), String> {
    if which_ok("pwsh") {
        // pwsh uses -Command for one-liners the same way.
        return Ok(("pwsh".into(), "-Command"));
    }
    if cfg!(windows) && which_ok("powershell") {
        return Ok(("powershell".into(), "-Command"));
    }
    if cfg!(windows) {
        Err(
            "no PowerShell on PATH (tried pwsh, powershell). Install PowerShell 7 or use Windows PowerShell."
                .into(),
        )
    } else {
        Err(
            "no pwsh on PATH. Install PowerShell 7 (https://aka.ms/powershell) to use the powershell tool on this OS."
                .into(),
        )
    }
}

fn which_ok(bin: &str) -> bool {
    // --version works for both pwsh and Windows powershell 5.1
    let mut c = Command::new(bin);
    if bin == "powershell" {
        c.args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"]);
    } else {
        c.arg("--version");
    }
    c.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn normalize_cli_output(s: &str) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
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

    #[test]
    fn identity() {
        let t = PowerShellTool;
        assert_eq!(t.name(), "powershell");
        assert!(t.needs_permission());
        assert!(crate::json::parse(&t.input_schema()).is_ok());
    }

    #[test]
    fn requires_command() {
        assert!(PowerShellTool.execute("{}").is_err());
    }

    #[test]
    fn echo_or_missing_binary() {
        let input = r#"{"command":"Write-Output 'hi-from-ps'"}"#;
        match PowerShellTool.execute(input) {
            Ok(out) => {
                assert!(out.to_ascii_lowercase().contains("hi-from-ps"), "{out}");
                assert!(out.contains("(exit code 0)"), "{out}");
            }
            Err(e) => {
                assert!(
                    e.contains("no PowerShell")
                        || e.contains("no pwsh")
                        || e.contains("powershell exec")
                        || e.contains("exit code"),
                    "unexpected: {e}"
                );
            }
        }
    }

    #[test]
    fn timeout_kills_long_sleep() {
        if resolve_powershell().is_err() {
            return;
        }
        let input = r#"{"command":"Start-Sleep -Seconds 5","timeout":300}"#;
        let err = PowerShellTool.execute(input).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }

    #[test]
    fn resolve_finds_something_on_windows() {
        if !cfg!(windows) {
            return;
        }
        let (bin, _) = resolve_powershell().expect("Windows should have powershell");
        assert!(bin == "pwsh" || bin == "powershell", "{bin}");
    }
}
