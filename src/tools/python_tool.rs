//! Thin `python` tool: run a snippet (`code`) or a script file (`file` + `args`).
//! Same idea as `shell` / `go` — spawn an interpreter on PATH, return stdout/stderr.

use std::collections::VecDeque;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::registry::Tool;
use super::workspace;

/// Max chars returned to the model (head + tail).
const MAX_OUTPUT_CHARS: usize = 12_000;
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 4_000;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

pub struct PythonTool;

impl Tool for PythonTool {
    fn name(&self) -> &str {
        "python"
    }

    fn description(&self) -> &str {
        "Run Python via the system interpreter (python3 / python / py -3). \
         Pass either `code` (inline snippet) or `file` (script path under the workspace) \
         with optional `args` and `timeout` (ms). Prefer this over shell for Python so \
         quoting stays simple. Check the exit code footer in the result."
    }

    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"code":{"type":"string","description":"Inline Python source to run with python -c"},"file":{"type":"string","description":"Path to a .py script under the workspace"},"args":{"type":"array","items":{"type":"string"},"description":"Arguments passed to the script (file mode only)"},"timeout":{"type":"integer","description":"Timeout in milliseconds (default 60000)"}},"additionalProperties":false}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;

        let code = obj.get("code").and_then(|v| v.as_str());
        let file = obj.get("file").and_then(|v| v.as_str());
        let timeout_ms = obj
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        if code.is_some() && file.is_some() {
            return Err("pass either code or file, not both".into());
        }
        if code.is_none() && file.is_none() {
            return Err("code or file required".into());
        }

        let args_list: Vec<String> = match obj.get("args") {
            Some(value) => value
                .as_array()
                .ok_or("args must be an array of strings")?
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .map(String::from)
                        .ok_or("args must contain only strings")
                })
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };

        if !args_list.is_empty() && file.is_none() {
            return Err("args only apply with file (not code)".into());
        }

        let deadline = Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .ok_or_else(|| format!("timeout too large: {timeout_ms}ms"))?;

        let (bin, prefix_args) = resolve_python()?;
        let mut cmd = Command::new(&bin);
        cmd.args(&prefix_args);

        if let Some(src) = code {
            cmd.arg("-c").arg(src);
        } else if let Some(path) = file {
            let abs =
                workspace::resolve_in_workspace(path).map_err(|e| format!("file path: {e}"))?;
            if !abs.is_file() {
                return Err(format!("file not found: {}", abs.display()));
            }
            cmd.arg(&abs);
            cmd.args(&args_list);
        }

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Quiet child console on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("python exec error ({bin}): {e}"))?;

        // Drain both streams while Python runs. Waiting for exit first can
        // deadlock once either OS pipe fills, causing a false timeout.
        let stdout_handle = drain_bounded(
            child.stdout.take().expect("stdout is piped"),
            HEAD_CHARS,
            TAIL_CHARS,
        );
        let stderr_handle = drain_bounded(
            child.stderr.take().expect("stderr is piped"),
            HEAD_CHARS,
            TAIL_CHARS,
        );

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        // Descendants can inherit the pipes, so joining the
                        // drain threads here would defeat the timeout.
                        return Err(format!("python timed out after {timeout_ms}ms"));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("python exec error: {e}")),
            }
        };
        let code_n = status.code().unwrap_or(-1);
        let ok = status.success();

        let stdout = normalize_output(&stdout_handle.join().unwrap_or_default());
        let stderr = normalize_output(&stderr_handle.join().unwrap_or_default());

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
        result.push_str(&format!("(exit code {code_n})"));

        if ok {
            Ok(result)
        } else {
            Err(format!("exit code {code_n}\n{result}"))
        }
    }
}

/// Pick a Python interpreter: `python3`, then `python`, then Windows `py -3`.
fn resolve_python() -> Result<(String, Vec<String>), String> {
    if which_ok("python3") {
        return Ok(("python3".into(), Vec::new()));
    }
    if which_ok("python") {
        return Ok(("python".into(), Vec::new()));
    }
    if cfg!(windows) && which_ok("py") {
        return Ok(("py".into(), vec!["-3".into()]));
    }
    Err(
        "no Python interpreter on PATH (tried python3, python, py -3). Install Python or add it to PATH."
            .into(),
    )
}

fn which_ok(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct BoundedCollector {
    head: String,
    head_len: usize,
    tail: VecDeque<char>,
    total_chars: usize,
    head_max: usize,
    tail_max: usize,
}

impl BoundedCollector {
    fn new(head_max: usize, tail_max: usize) -> Self {
        Self {
            head: String::new(),
            head_len: 0,
            tail: VecDeque::new(),
            total_chars: 0,
            head_max,
            tail_max,
        }
    }

    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.total_chars += 1;
            if self.head_len < self.head_max {
                self.head.push(c);
                self.head_len += 1;
            } else {
                if self.tail.len() >= self.tail_max {
                    self.tail.pop_front();
                }
                self.tail.push_back(c);
            }
        }
    }

    fn finish(self) -> String {
        let tail_len = self.tail.len();
        let tail: String = self.tail.into_iter().collect();
        if self.total_chars <= self.head_len + tail_len {
            format!("{}{tail}", self.head)
        } else {
            let omitted = self.total_chars - self.head_len - tail_len;
            format!("{}\n… ({omitted} chars omitted) …\n{tail}", self.head)
        }
    }
}

fn drain_bounded<R: Read + Send + 'static>(
    mut reader: R,
    head_max: usize,
    tail_max: usize,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut collector = BoundedCollector::new(head_max, tail_max);
        let mut buf = [0u8; 8192];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => push_utf8(&mut collector, &mut pending, &buf[..n]),
            }
        }
        if !pending.is_empty() {
            collector.push_str(&String::from_utf8_lossy(&pending));
        }
        collector.finish()
    })
}

fn push_utf8(collector: &mut BoundedCollector, pending: &mut Vec<u8>, bytes: &[u8]) {
    pending.extend_from_slice(bytes);
    loop {
        let (valid_up_to, error_len) = match std::str::from_utf8(pending) {
            Ok(valid) => {
                collector.push_str(valid);
                pending.clear();
                return;
            }
            Err(error) => (error.valid_up_to(), error.error_len()),
        };

        if valid_up_to > 0 {
            let valid = std::str::from_utf8(&pending[..valid_up_to])
                .expect("valid_up_to always ends at valid UTF-8");
            collector.push_str(valid);
            pending.drain(..valid_up_to);
        }

        let Some(error_len) = error_len else {
            return;
        };
        collector.push_str("\u{fffd}");
        pending.drain(..error_len);
    }
}

fn normalize_output(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn truncate_head_tail(s: &str, max_total: usize, head: usize, tail: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_total {
        return s.to_string();
    }
    let head_n = head.min(chars.len());
    let tail_n = tail.min(chars.len().saturating_sub(head_n));
    let head_part: String = chars[..head_n].iter().collect();
    let tail_part: String = chars[chars.len() - tail_n..].iter().collect();
    let omitted = chars.len() - head_n - tail_n;
    format!("{head_part}\n… ({omitted} chars omitted) …\n{tail_part}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn identity() {
        let t = PythonTool;
        assert_eq!(t.name(), "python");
        assert!(t.needs_permission());
        assert!(crate::json::parse(&t.input_schema()).is_ok());
    }

    #[test]
    fn requires_code_or_file() {
        assert!(PythonTool.execute("{}").is_err());
        assert!(PythonTool
            .execute(r#"{"code":"x=1","file":"a.py"}"#)
            .is_err());
    }

    #[test]
    fn args_without_file_rejected() {
        let err = PythonTool
            .execute(r#"{"code":"print(1)","args":["x"]}"#)
            .unwrap_err();
        assert!(err.contains("args"), "{err}");
    }

    #[test]
    fn invalid_args_are_rejected() {
        let err = PythonTool
            .execute(r#"{"file":"script.py","args":[1]}"#)
            .unwrap_err();
        assert!(err.contains("only strings"), "{err}");
    }

    #[test]
    fn runs_inline_or_reports_missing_python() {
        match PythonTool.execute(r#"{"code":"print(1+1)"}"#) {
            Ok(out) => {
                assert!(out.contains('2'), "{out}");
                assert!(out.contains("exit code 0"), "{out}");
            }
            Err(e) => {
                assert!(
                    e.contains("no Python") || e.contains("python exec") || e.contains("exit code"),
                    "unexpected: {e}"
                );
            }
        }
    }

    #[test]
    fn large_output_does_not_deadlock() {
        if resolve_python().is_err() {
            return;
        }
        let input = r#"{"code":"print('x' * 200000)","timeout":20000}"#;
        let out = PythonTool.execute(input).unwrap();
        assert!(out.contains("omitted"), "output was not truncated");
        assert!(out.contains("exit code 0"), "{out}");
    }

    #[test]
    fn truncation_preserves_multibyte_characters() {
        let out = truncate_head_tail(&"€".repeat(200), 100, 31, 31);
        assert!(out.starts_with(&"€".repeat(31)));
        assert!(out.ends_with(&"€".repeat(31)));
        assert!(out.contains("138 chars omitted"), "{out}");
    }

    #[test]
    fn draining_preserves_multibyte_characters_across_chunks() {
        let mut bytes = vec![b'x'; 8191];
        bytes.extend_from_slice("€tail".as_bytes());
        let out = drain_bounded(std::io::Cursor::new(bytes), 10_000, 100)
            .join()
            .unwrap();
        assert!(out.ends_with("€tail"));
        assert!(!out.contains('\u{fffd}'), "{out}");
    }

    #[test]
    fn runs_file_in_workspace_or_missing_python() {
        // Write under the repo cwd so resolve_in_workspace accepts it (no chdir).
        let name = format!("cairn_py_test_{}.py", std::process::id());
        fs::write(&name, "import sys\nprint(sys.argv[1])\n").unwrap();
        let input = format!(r#"{{"file":"{name}","args":["ok"]}}"#);
        let result = PythonTool.execute(&input);
        let _ = fs::remove_file(&name);
        match result {
            Ok(out) => {
                assert!(out.contains("ok"), "{out}");
                assert!(out.contains("exit code 0"), "{out}");
            }
            Err(e) => {
                assert!(
                    e.contains("no Python") || e.contains("python exec") || e.contains("exit code"),
                    "unexpected: {e}"
                );
            }
        }
    }

    #[test]
    fn resolve_python_returns_something_or_clear_error() {
        match resolve_python() {
            Ok((bin, _)) => assert!(!bin.is_empty()),
            Err(e) => assert!(e.contains("no Python"), "{e}"),
        }
    }
}
