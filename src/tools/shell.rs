use super::registry::Tool;
use std::collections::VecDeque;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
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
        let deadline = timeout_ms
            .map(|ms| {
                Instant::now()
                    .checked_add(Duration::from_millis(ms))
                    .ok_or_else(|| format!("timeout too large: {ms}ms"))
            })
            .transpose()?;

        let shell = if cfg!(windows) { "powershell" } else { "bash" };
        let flag = if cfg!(windows) { "-Command" } else { "-c" };

        let mut command = Command::new(shell);
        command
            .arg(flag)
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_process_group(&mut command);

        let process_tree = ProcessTree::new()?;
        let mut child = command.spawn().map_err(|e| format!("exec error: {e}"))?;
        if let Err(error) = process_tree.attach(&mut child) {
            return match child.kill() {
                Ok(()) => {
                    let _ = child.wait();
                    Err(error)
                }
                Err(cleanup_error) => {
                    Err(format!("{error}; process cleanup failed: {cleanup_error}"))
                }
            };
        }

        let stdout_rx = drain_bounded(
            child.stdout.take().expect("stdout is piped"),
            HEAD_CHARS,
            TAIL_CHARS,
        );
        let stderr_rx = drain_bounded(
            child.stderr.take().expect("stderr is piped"),
            HEAD_CHARS,
            TAIL_CHARS,
        );

        let mut stdout = None;
        let mut stderr = None;

        let status = loop {
            receive_output(&stdout_rx, &mut stdout);
            receive_output(&stderr_rx, &mut stderr);

            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                let cleanup_error = process_tree.terminate(&mut child).err();
                if cleanup_error.is_none() {
                    let _ = child.wait();
                }
                let mut error = format!(
                    "command timed out after {}ms",
                    timeout_ms.expect("deadline requires timeout")
                );
                if let Some(cleanup_error) = cleanup_error {
                    error.push_str(&format!("; process cleanup failed: {cleanup_error}"));
                }
                return Err(error);
            }

            // Keep the group leader unreaped until all pipe holders exit. Its
            // PID is also the Unix process-group ID and must not be reusable
            // while timeout cleanup may still signal the group.
            if stdout.is_some() && stderr.is_some() {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => {}
                    Err(e) => {
                        let cleanup_error = process_tree.terminate(&mut child).err();
                        if cleanup_error.is_none() {
                            let _ = child.wait();
                        }
                        return Err(match cleanup_error {
                            Some(cleanup_error) => {
                                format!("exec error: {e}; process cleanup failed: {cleanup_error}")
                            }
                            None => format!("exec error: {e}"),
                        });
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(10));
        };

        let code = status.code().unwrap_or(-1);
        let ok = status.success();

        let stdout = normalize_cli_output(&stdout.unwrap_or_default());
        let stderr = normalize_cli_output(&stderr.unwrap_or_default());

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

fn receive_output(rx: &Receiver<String>, output: &mut Option<String>) {
    if output.is_some() {
        return;
    }
    match rx.try_recv() {
        Ok(value) => *output = Some(value),
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => *output = Some(String::new()),
    }
}

struct BoundedCollector {
    head: String,
    head_len: usize,
    head_max: usize,
    tail: VecDeque<char>,
    tail_max: usize,
    total_chars: usize,
}

impl BoundedCollector {
    fn new(head_max: usize, tail_max: usize) -> Self {
        Self {
            head: String::new(),
            head_len: 0,
            head_max,
            tail: VecDeque::new(),
            tail_max,
            total_chars: 0,
        }
    }

    fn push_str(&mut self, value: &str) {
        for character in value.chars() {
            self.total_chars += 1;
            if self.head_len < self.head_max {
                self.head.push(character);
                self.head_len += 1;
            } else {
                if self.tail.len() >= self.tail_max {
                    self.tail.pop_front();
                }
                self.tail.push_back(character);
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
            format!("{}\n... [{omitted} chars truncated] ...\n{tail}", self.head)
        }
    }
}

fn drain_bounded<R: Read + Send + 'static>(
    mut reader: R,
    head_max: usize,
    tail_max: usize,
) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut collector = BoundedCollector::new(head_max, tail_max);
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => collector.push_str(&String::from_utf8_lossy(&buffer[..read])),
            }
        }
        let _ = tx.send(collector.finish());
    });
    rx
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
struct ProcessTree;

#[cfg(unix)]
impl ProcessTree {
    fn new() -> Result<Self, String> {
        Ok(Self)
    }

    fn attach(&self, _child: &mut std::process::Child) -> Result<(), String> {
        Ok(())
    }

    fn terminate(&self, child: &mut std::process::Child) -> Result<(), String> {
        // The child's PID is also the process-group ID configured before spawn.
        if unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) } == 0 {
            return Ok(());
        }
        let group_error = std::io::Error::last_os_error();
        if group_error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        match child.kill() {
            Ok(()) => Err(group_error.to_string()),
            Err(child_error) => Err(format!(
                "{group_error}; child kill also failed: {child_error}"
            )),
        }
    }
}

#[cfg(windows)]
struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessTree {
    fn new() -> Result<Self, String> {
        use windows_sys::Win32::System::JobObjects::CreateJobObjectW;

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(format!(
                "exec error: could not create job object: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(job))
    }

    fn attach(&self, child: &mut std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let job = self.0;
        if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } == 0 {
            return Err(format!(
                "exec error: could not assign process to job object: {}",
                std::io::Error::last_os_error()
            ));
        }
        resume_process(child.id())
    }

    fn terminate(&self, child: &mut std::process::Child) -> Result<(), String> {
        if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) } != 0 {
            return Ok(());
        }
        let job_error = std::io::Error::last_os_error();
        child
            .kill()
            .map_err(|child_error| format!("{job_error}; child kill also failed: {child_error}"))?;
        Err(job_error.to_string())
    }
}

#[cfg(windows)]
fn resume_process(process_id: u32) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "exec error: could not enumerate suspended process threads: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // A CREATE_SUSPENDED process has only its primary thread: it cannot run
    // and create another before assignment to the job and this snapshot.
    let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    let mut thread_id = None;
    while found {
        if entry.th32OwnerProcessID == process_id {
            thread_id = Some(entry.th32ThreadID);
            break;
        }
        found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }

    let thread_id = thread_id.ok_or_else(|| {
        "exec error: could not find suspended process's primary thread".to_string()
    })?;
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(format!(
            "exec error: could not open suspended process thread: {}",
            std::io::Error::last_os_error()
        ));
    }
    let resumed = unsafe { ResumeThread(thread) };
    unsafe {
        CloseHandle(thread);
    }
    if resumed != 1 {
        return Err(format!(
            "exec error: suspended process had unexpected resume count {resumed}"
        ));
    }
    Ok(())
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct ProcessTree;

#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    fn new() -> Result<Self, String> {
        Ok(Self)
    }

    fn attach(&self, _child: &mut std::process::Child) -> Result<(), String> {
        Ok(())
    }

    fn terminate(&self, child: &mut std::process::Child) -> Result<(), String> {
        child.kill().map_err(|error| error.to_string())
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
    fn test_large_stdout_and_stderr_do_not_deadlock() {
        let tool = ShellTool;
        let cmd = if cfg!(windows) {
            "1..20000 | ForEach-Object { Write-Output ('stdout padding ' + $_); [Console]::Error.WriteLine(('stderr padding ' + $_)) }"
        } else {
            "for i in $(seq 1 20000); do echo stdout-padding-$i; echo stderr-padding-$i >&2; done"
        };
        let input = format!(r#"{{"command":"{cmd}","timeout":20000}}"#);
        let out = tool.execute(&input).unwrap();
        assert!(
            out.contains("stdout padding 1") || out.contains("stdout-padding-1"),
            "lost stdout: {out}"
        );
        assert!(
            out.contains("stderr padding 20000") || out.contains("stderr-padding-20000"),
            "lost stderr: {out}"
        );
        assert!(out.contains("(exit code 0)"), "missing exit footer: {out}");
        assert!(out.chars().count() <= MAX_OUTPUT_CHARS + 100);
    }

    #[cfg(unix)]
    #[test]
    fn test_timeout_kills_descendants() {
        let marker = std::env::temp_dir().join(format!(
            "cairn-shell-descendant-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Let the shell exit immediately. The descendant keeps the inherited
        // pipes open, so timeout handling must still kill its process group.
        let command = format!("(sleep 1; printf survived > '{}') &", marker.display());
        let input = serde_json::json!({ "command": command, "timeout": 200 }).to_string();

        let err = ShellTool.execute(&input).unwrap_err();
        assert!(err.contains("timed out"), "unexpected error: {err}");
        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "descendant survived timeout and wrote {}",
            marker.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_timeout_kills_descendants() {
        let marker = std::env::temp_dir().join(format!(
            "cairn-shell-descendant-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let marker = marker.display().to_string().replace('\'', "''");
        let command = format!(
            "Start-Process powershell -ArgumentList @('-NoProfile','-Command','Start-Sleep -Milliseconds 1000; Set-Content -LiteralPath ''{marker}'' -Value survived'); Start-Sleep -Seconds 5"
        );
        let input = serde_json::json!({ "command": command, "timeout": 200 }).to_string();

        let err = ShellTool.execute(&input).unwrap_err();
        assert!(err.contains("timed out"), "unexpected error: {err}");
        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !std::path::Path::new(&marker).exists(),
            "descendant survived timeout and wrote {marker}"
        );
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
