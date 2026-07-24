//! Shared subprocess runner: bounded output, an optional finite timeout, an
//! optional cooperative cancellation token, and process-tree termination.
//!
//! The `shell`, `git`, and `go` tools all run child processes that can hang,
//! spawn descendants, or produce unbounded output. This module centralizes the
//! safe way to do that: output is drained on background threads into a
//! head+tail bounded buffer (so a chatty process cannot exhaust memory or
//! deadlock on a full pipe), the caller can cap wall-clock time, and the whole
//! process group / job object is killed on timeout or cancellation so no
//! descendant keeps running after the tool returns.

use std::collections::VecDeque;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How the caller wants a command run.
pub struct RunOptions {
    /// Maximum wall-clock time before the process tree is killed. `None`
    /// leaves the command unbounded (caller relies on cancellation only).
    pub timeout: Option<Duration>,
    /// Characters kept from the start of each stream.
    pub head_chars: usize,
    /// Characters kept from the end of each stream.
    pub tail_chars: usize,
}

/// A completed process, with each stream already bounded to head+tail.
pub struct RunResult {
    pub code: i32,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Why a run did not complete normally. Messages are raw (no tool prefix) so
/// each caller can phrase them in its own voice.
#[derive(Debug)]
pub enum RunError {
    /// Failed to set up the process group / job object, spawn, or attach.
    Spawn(String),
    /// Exceeded the configured timeout; `after_ms` is that timeout.
    TimedOut {
        after_ms: u64,
        cleanup_error: Option<String>,
    },
    /// The cancellation token was set while the command was running.
    Cancelled { cleanup_error: Option<String> },
    /// Waiting on the child failed partway through.
    Wait {
        reason: String,
        cleanup_error: Option<String>,
    },
}

/// Append a process-cleanup failure detail to an error message, matching the
/// phrasing the shell tool has always used.
pub fn with_cleanup(base: String, cleanup_error: &Option<String>) -> String {
    match cleanup_error {
        Some(error) => format!("{base}; process cleanup failed: {error}"),
        None => base,
    }
}

/// Run `command` to completion (or until timeout/cancellation), returning
/// bounded output. Stdout/stderr are forced to pipes and the process is placed
/// in its own group/job so descendants can be killed together.
pub fn run(
    mut command: Command,
    options: &RunOptions,
    cancel: Option<&AtomicBool>,
) -> Result<RunResult, RunError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_process_group(&mut command);

    // Overflow (timeout near the end of the monotonic clock) degrades to
    // "no deadline" rather than erroring; cancellation still applies.
    let deadline = options.timeout.and_then(|d| Instant::now().checked_add(d));

    let process_tree = ProcessTree::new().map_err(RunError::Spawn)?;
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    if let Err(error) = process_tree.attach(&mut child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(RunError::Spawn(error));
    }

    let stdout_rx = drain_bounded(
        child.stdout.take().expect("stdout is piped"),
        options.head_chars,
        options.tail_chars,
    );
    let stderr_rx = drain_bounded(
        child.stderr.take().expect("stderr is piped"),
        options.head_chars,
        options.tail_chars,
    );

    let mut stdout = None;
    let mut stderr = None;

    let status = loop {
        receive_output(&stdout_rx, &mut stdout);
        receive_output(&stderr_rx, &mut stderr);

        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            let cleanup_error = process_tree.terminate(&mut child).err();
            if cleanup_error.is_none() {
                let _ = child.wait();
            }
            return Err(RunError::Cancelled { cleanup_error });
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let cleanup_error = process_tree.terminate(&mut child).err();
            if cleanup_error.is_none() {
                let _ = child.wait();
            }
            let after_ms = options
                .timeout
                .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
                .unwrap_or(0);
            return Err(RunError::TimedOut {
                after_ms,
                cleanup_error,
            });
        }

        // Keep the group leader unreaped until all pipe holders exit. Its PID
        // is also the Unix process-group ID and must not be reusable while
        // timeout/cancel cleanup may still signal the group.
        if stdout.is_some() && stderr.is_some() {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    let cleanup_error = process_tree.terminate(&mut child).err();
                    if cleanup_error.is_none() {
                        let _ = child.wait();
                    }
                    return Err(RunError::Wait {
                        reason: e.to_string(),
                        cleanup_error,
                    });
                }
            }
        }

        std::thread::sleep(Duration::from_millis(10));
    };

    Ok(RunResult {
        code: status.code().unwrap_or(-1),
        success: status.success(),
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
    })
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
                "could not create job object: {}",
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
                "could not assign process to job object: {}",
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
            "could not enumerate suspended process threads: {}",
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

    let thread_id =
        thread_id.ok_or_else(|| "could not find suspended process's primary thread".to_string())?;
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(format!(
            "could not open suspended process thread: {}",
            std::io::Error::last_os_error()
        ));
    }
    let resumed = unsafe { ResumeThread(thread) };
    unsafe {
        CloseHandle(thread);
    }
    if resumed != 1 {
        return Err(format!(
            "suspended process had unexpected resume count {resumed}"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_command(text: &str) -> Command {
        let mut command = Command::new(if cfg!(windows) { "powershell" } else { "bash" });
        if cfg!(windows) {
            command
                .arg("-Command")
                .arg(format!("Write-Output '{text}'"));
        } else {
            command.arg("-c").arg(format!("echo {text}"));
        }
        command
    }

    fn sleep_command(seconds: u64) -> Command {
        let mut command = Command::new(if cfg!(windows) { "powershell" } else { "bash" });
        if cfg!(windows) {
            command
                .arg("-Command")
                .arg(format!("Start-Sleep -Seconds {seconds}"));
        } else {
            command.arg("-c").arg(format!("sleep {seconds}"));
        }
        command
    }

    #[test]
    fn runs_to_completion_and_captures_stdout() {
        let result = run(
            echo_command("hello-runner"),
            &RunOptions {
                timeout: Some(Duration::from_secs(30)),
                head_chars: 6_000,
                tail_chars: 4_000,
            },
            None,
        )
        .expect("command should complete");
        assert!(result.success);
        assert_eq!(result.code, 0);
        assert!(result.stdout.contains("hello-runner"), "{}", result.stdout);
    }

    #[test]
    fn timeout_reports_and_kills() {
        let err = run(
            sleep_command(5),
            &RunOptions {
                timeout: Some(Duration::from_millis(200)),
                head_chars: 100,
                tail_chars: 100,
            },
            None,
        )
        .err()
        .expect("command should time out");
        match err {
            RunError::TimedOut { after_ms, .. } => assert_eq!(after_ms, 200),
            _ => panic!("expected TimedOut"),
        }
    }

    #[test]
    fn cancellation_stops_a_running_command() {
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                cancel.store(true, Ordering::Relaxed);
            });
            let err = run(
                sleep_command(30),
                &RunOptions {
                    timeout: None,
                    head_chars: 100,
                    tail_chars: 100,
                },
                Some(&cancel),
            )
            .err()
            .expect("command should be cancelled");
            assert!(matches!(err, RunError::Cancelled { .. }));
        });
    }
}
