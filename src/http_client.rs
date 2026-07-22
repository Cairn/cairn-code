use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug)]
pub struct HttpResponse {
    pub body: String,
}

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF_MS: u64 = 500;
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const WATCHDOG_POLL: Duration = Duration::from_millis(200);

enum RequestError {
    /// A completed HTTP response with a non-2xx status.
    Status(u16, String),
    /// curl couldn't be spawned, the connection failed, or a similar
    /// transport-level problem occurred before/without a usable response.
    Transport(String),
}

impl RequestError {
    fn into_string(self) -> String {
        match self {
            RequestError::Status(status, body) => format!("HTTP {status}: {}", body.trim()),
            RequestError::Transport(msg) => format!("curl: {msg}"),
        }
    }
}

fn is_retriable_status(status: u16) -> bool {
    matches!(status, 429 | 503 | 529)
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(BASE_BACKOFF_MS * 2u64.saturating_pow(attempt.saturating_sub(1)))
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn debug_log_request(url: &str, body: &str) {
    let dir = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(dir).join(".config/cairn-code/debug_request.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("URL: {url}\n\nBody:\n{body}"));
}

fn spawn_curl(req: &HttpRequest) -> Result<Child, String> {
    if let Some(body) = &req.body {
        debug_log_request(&req.url, body);
    }

    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-i", "-X", "POST", &req.url]);
    for (k, v) in &req.headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    // Disable "Expect: 100-continue" so the response is a single header block,
    // which keeps status-line parsing simple for large request bodies.
    cmd.arg("-H").arg("Expect:");
    cmd.arg("--data-binary").arg("@-");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("curl: {e}"))?;

    let body = req.body.clone().unwrap_or_default();
    let mut stdin = child.stdin.take().ok_or("curl: no stdin")?;
    std::thread::spawn(move || {
        let _ = stdin.write_all(body.as_bytes());
    });

    Ok(child)
}

fn parse_status_line(line: &str) -> u16 {
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0)
}

fn request_once(req: &HttpRequest) -> Result<HttpResponse, RequestError> {
    let child = spawn_curl(req).map_err(RequestError::Transport)?;
    let output = child.wait_with_output().map_err(|e| RequestError::Transport(format!("{e}")))?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RequestError::Transport(stderr.trim().to_string()));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let split_at = raw
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))
        .unwrap_or(0);
    let status = raw
        .get(..split_at)
        .and_then(|h| h.lines().next())
        .map(parse_status_line)
        .unwrap_or(0);
    let body = raw.get(split_at..).unwrap_or(&raw).to_string();

    if status < 200 || status >= 300 {
        return Err(RequestError::Status(status, body));
    }

    Ok(HttpResponse { body })
}

/// Sends a non-streaming request, retrying transient failures (429/503/529
/// responses and transport-level errors) with exponential backoff.
pub fn request(req: &HttpRequest) -> Result<HttpResponse, String> {
    let mut attempt = 0;
    loop {
        match request_once(req) {
            Ok(resp) => return Ok(resp),
            Err(RequestError::Status(status, _)) if is_retriable_status(status) && attempt < MAX_RETRIES => {
                attempt += 1;
                std::thread::sleep(backoff_delay(attempt));
            }
            Err(RequestError::Transport(_)) if attempt < MAX_RETRIES => {
                attempt += 1;
                std::thread::sleep(backoff_delay(attempt));
            }
            Err(e) => return Err(e.into_string()),
        }
    }
}

enum StreamOutcome {
    Cancelled,
    IdleTimeout,
    Other(RequestError),
}

/// Runs one streaming attempt. `on_line` is called for every non-empty body
/// line of a 2xx response. Returns whether any line was emitted (needed by
/// the retry wrapper: once data has started flowing to the caller, retrying
/// from scratch would duplicate/confuse it, so retries only happen for
/// failures before the first emitted line).
fn request_streaming_attempt<F>(
    req: &HttpRequest,
    on_line: &mut F,
    cancel: Option<&AtomicBool>,
) -> Result<(), (StreamOutcome, bool)>
where
    F: FnMut(&str),
{
    let mut child = match spawn_curl(req) {
        Ok(c) => c,
        Err(e) => return Err((StreamOutcome::Other(RequestError::Transport(e)), false)),
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Err((StreamOutcome::Other(RequestError::Transport("no stdout".into())), false)),
    };
    let reader = BufReader::with_capacity(64 * 1024, stdout);

    let child = Mutex::new(child);
    let last_activity = AtomicU64::new(now_millis());
    let timed_out = AtomicBool::new(false);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    let mut emitted_any = false;
    let mut read_error: Option<String> = None;

    #[derive(PartialEq)]
    enum State {
        Status,
        Headers,
        Body,
    }

    let mut state = State::Status;
    let mut status: u16 = 0;
    let mut error_body = String::new();

    let child_ref = &child;
    let last_activity_ref = &last_activity;
    let timed_out_ref = &timed_out;

    std::thread::scope(|scope| {
        scope.spawn(move || loop {
            match stop_rx.recv_timeout(WATCHDOG_POLL) {
                Ok(()) => return,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    let cancelled = cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false);
                    let idle_for = now_millis().saturating_sub(last_activity_ref.load(Ordering::Relaxed));
                    if cancelled || idle_for >= STREAM_IDLE_TIMEOUT.as_millis() as u64 {
                        if !cancelled {
                            timed_out_ref.store(true, Ordering::Relaxed);
                        }
                        if let Ok(mut c) = child_ref.lock() {
                            let _ = c.kill();
                        }
                        return;
                    }
                }
            }
        });

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => { read_error = Some(e.to_string()); break; }
            };
            last_activity.store(now_millis(), Ordering::Relaxed);
            match state {
                State::Status => {
                    status = parse_status_line(&line);
                    state = State::Headers;
                }
                State::Headers => {
                    if line.is_empty() || line == "\r" {
                        state = State::Body;
                    }
                }
                State::Body => {
                    if (200..300).contains(&status) {
                        if !line.is_empty() {
                            emitted_any = true;
                            on_line(&line);
                        }
                    } else if !line.is_empty() {
                        error_body.push_str(&line);
                        error_body.push('\n');
                    }
                }
            }
        }

        let _ = stop_tx.send(());
    });

    let cancelled = cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false);
    if cancelled && !emitted_any {
        let _ = child.lock().map(|mut c| c.wait());
        return Err((StreamOutcome::Cancelled, emitted_any));
    }
    if timed_out.load(Ordering::Relaxed) && !emitted_any {
        let _ = child.lock().map(|mut c| c.wait());
        return Err((StreamOutcome::IdleTimeout, emitted_any));
    }

    let exit = match child.lock().unwrap().wait() {
        Ok(e) => e,
        Err(e) => return Err((StreamOutcome::Other(RequestError::Transport(e.to_string())), emitted_any)),
    };

    if let Some(e) = read_error {
        return Err((StreamOutcome::Other(RequestError::Transport(format!("read error: {e}"))), emitted_any));
    }

    if status == 0 {
        let mut stderr = String::new();
        if let Ok(mut c) = child.lock() {
            if let Some(mut e) = c.stderr.take() {
                let _ = e.read_to_string(&mut stderr);
            }
        }
        if !exit.success() {
            return Err((StreamOutcome::Other(RequestError::Transport(stderr.trim().to_string())), emitted_any));
        }
        return Err((StreamOutcome::Other(RequestError::Transport("no response".into())), emitted_any));
    }

    if status < 200 || status >= 300 {
        return Err((StreamOutcome::Other(RequestError::Status(status, error_body)), emitted_any));
    }

    Ok(())
}

/// Streams a request, calling `on_line` for each non-empty line of a 2xx
/// response body. A watchdog kills the underlying curl process if no data
/// arrives for 60s or if `cancel` is set, and transient failures that occur
/// before any line has been emitted are retried with backoff.
pub fn request_streaming_with_cancel<F>(
    req: &HttpRequest,
    mut on_line: F,
    cancel: Option<&AtomicBool>,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mut attempt = 0;
    loop {
        match request_streaming_attempt(req, &mut on_line, cancel) {
            Ok(()) => return Ok(()),
            Err((StreamOutcome::Cancelled, _)) => return Err("cancelled".into()),
            Err((StreamOutcome::IdleTimeout, _)) => return Err("stream idle timeout: no data received".into()),
            Err((StreamOutcome::Other(RequestError::Status(status, _)), false))
                if is_retriable_status(status) && attempt < MAX_RETRIES =>
            {
                attempt += 1;
                std::thread::sleep(backoff_delay(attempt));
            }
            Err((StreamOutcome::Other(RequestError::Transport(_)), false)) if attempt < MAX_RETRIES => {
                attempt += 1;
                std::thread::sleep(backoff_delay(attempt));
            }
            Err((StreamOutcome::Other(e), _)) => return Err(e.into_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serves each response in order, one per accepted connection, on a
    /// fresh loopback port. Used to make curl (a real subprocess) exercise
    /// the retry path against a real, if trivial, HTTP server.
    fn start_mock_server(responses: Vec<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for resp in responses {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn test_is_retriable_status() {
        assert!(is_retriable_status(429));
        assert!(is_retriable_status(503));
        assert!(is_retriable_status(529));
        assert!(!is_retriable_status(500));
        assert!(!is_retriable_status(404));
    }

    #[test]
    fn test_backoff_delay_increases_exponentially() {
        assert_eq!(backoff_delay(1), Duration::from_millis(500));
        assert_eq!(backoff_delay(2), Duration::from_millis(1000));
        assert_eq!(backoff_delay(3), Duration::from_millis(2000));
    }

    #[test]
    fn test_request_retries_on_503_then_succeeds() {
        let url = start_mock_server(vec![
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        ]);
        let req = HttpRequest { url, headers: vec![], body: Some("{}".into()) };
        let resp = request(&req).unwrap();
        assert_eq!(resp.body, "ok");
    }

    #[test]
    fn test_request_does_not_retry_on_400() {
        let url = start_mock_server(vec![
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 6\r\nConnection: close\r\n\r\nbadreq",
        ]);
        let req = HttpRequest { url, headers: vec![], body: Some("{}".into()) };
        let err = request(&req).unwrap_err();
        assert!(err.contains("400"), "expected error to mention status 400, got: {err}");
    }
}
