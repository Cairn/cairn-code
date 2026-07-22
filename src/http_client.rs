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
            RequestError::Status(status, body) => format_status_error(status, &body),
            RequestError::Transport(msg) => format_transport_error(&msg),
        }
    }
}

/// Pull a short human-readable detail out of a provider error body (JSON or plain text).
fn extract_error_detail(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(val) = crate::json::parse(trimmed) {
        if let Some(obj) = val.as_object() {
            // OpenAI / OpenRouter: {"error":{"message":"..."}}
            if let Some(err) = obj.get("error") {
                if let Some(msg) = err.get("message").and_then(|v| v.as_str()) {
                    return msg.trim().to_string();
                }
                if let Some(msg) = err.as_str() {
                    return msg.trim().to_string();
                }
            }
            // Anthropic-ish: {"type":"error","error":{"type":"...","message":"..."}}
            // Already handled above. Also bare {"message":"..."}.
            if let Some(msg) = obj.get("message").and_then(|v| v.as_str()) {
                return msg.trim().to_string();
            }
        }
    }
    // Collapse whitespace and cap length so huge HTML error pages stay readable.
    let flat: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 280 {
        let short: String = flat.chars().take(280).collect();
        format!("{short}…")
    } else {
        flat
    }
}

/// True when an error string looks like a context-window / prompt-too-long
/// failure from a common provider. Used for reactive compaction.
pub fn is_context_limit_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "context length",
        "context window",
        "context_length_exceeded",
        "maximum context",
        "context limit",
        "prompt is too long",
        "too many tokens",
        "reduce the length of the messages",
        "input is too long",
        // avoid bare "max_tokens" alone: it appears in normal request logs
        "max_tokens is too large",
        "exceeds the model's",
        "exceeds model",
    ]
    .iter()
    .any(|n| lower.contains(n))
}

fn looks_like_context_limit(detail: &str) -> bool {
    is_context_limit_error(detail)
}

fn format_status_error(status: u16, body: &str) -> String {
    let detail = crate::redact::redact_secrets(&extract_error_detail(body));
    let advice = if looks_like_context_limit(&detail) {
        "Prompt exceeds the model context window. Start a new session (/clear) or continue so compaction can shrink history."
    } else {
        match status {
            401 | 403 => "Authentication failed. Check your API key (env var or save one via /provider).",
            404 => "Not found. Check the model id and that your provider supports it.",
            402 => "Payment required or insufficient credits on this provider.",
            429 => "Rate limited by the provider. Wait and retry, or switch model/provider.",
            500 | 502 => "Provider server error. Retry shortly, or switch provider.",
            503 | 529 => "Provider overloaded or unavailable. Retry shortly, or switch provider.",
            _ => "",
        }
    };

    if advice.is_empty() && detail.is_empty() {
        format!("HTTP {status} from provider.")
    } else if advice.is_empty() {
        format!("HTTP {status}: {detail}")
    } else if detail.is_empty() {
        format!("HTTP {status}: {advice}")
    } else {
        format!("HTTP {status}: {advice} Provider said: {detail}")
    }
}

fn format_transport_error(msg: &str) -> String {
    let m = msg.trim();
    if m.is_empty() {
        return "Network error: could not reach the provider. Check connectivity and that curl is on PATH.".into();
    }
    let lower = m.to_ascii_lowercase();
    if lower.contains("could not resolve host") || lower.contains("name or service not known") {
        return format!("Network error: DNS lookup failed ({m}). Check connectivity.");
    }
    if lower.contains("connection refused") {
        return format!("Network error: connection refused ({m}). Is the provider running and reachable?");
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return format!("Network error: connection timed out ({m}). Retry, or check network/firewall.");
    }
    if lower.contains("failed to connect") || lower.contains("couldn't connect") {
        return format!("Network error: could not connect ({m}). Check connectivity and provider URL.");
    }
    // curl missing from PATH shows up as a spawn error.
    if lower.contains("the system cannot find the file")
        || lower.contains("no such file or directory")
        || lower.contains("program not found")
        || lower.contains("not found") && lower.contains("curl")
    {
        return format!("Network error: could not run curl ({m}). Install curl and ensure it is on PATH.");
    }
    format!("Network error: {m}")
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

/// Global opt-in for [`debug_log_request`]. Off by default (H-03): request
/// URLs, headers, and bodies previously landed on disk unconditionally on
/// every provider call, with only heuristic secret redaction. Set from
/// `Config::debug_log_requests` at startup, or via `CAIRN_DEBUG_HTTP=1`.
static DEBUG_LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enables or disables writing request metadata for troubleshooting. Call
/// once at startup from the loaded config; defaults to disabled otherwise.
pub fn set_debug_logging_enabled(enabled: bool) {
    DEBUG_LOGGING_ENABLED.store(enabled, Ordering::Relaxed);
}

fn debug_logging_enabled() -> bool {
    let env_value = std::env::var("CAIRN_DEBUG_HTTP").ok();
    debug_logging_enabled_for(
        DEBUG_LOGGING_ENABLED.load(Ordering::Relaxed),
        env_value.as_deref(),
    )
}

fn debug_logging_enabled_for(config_enabled: bool, env_value: Option<&str>) -> bool {
    config_enabled
        || env_value
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

/// When explicitly enabled, records request *metadata* only — a URL with
/// userinfo, query, and fragment removed, header names (never values), and body size — to
/// `~/.config/cairn-code/debug_request.json`. Header values and body content
/// (which can contain full prompts, source code, and credentials) are never
/// written, so there is nothing here for heuristic redaction to miss. The
/// file is overwritten (not appended) on every request, so it never
/// accumulates history beyond the most recent call; written atomically with
/// owner-only permissions where the OS supports it.
fn debug_log_request(req: &HttpRequest) {
    if !debug_logging_enabled() {
        return;
    }
    let dir = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(dir).join(".config/cairn-code/debug_request.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_atomic_private(&path, &debug_dump_content(req));
}

/// Pure formatting for [`debug_log_request`], split out so the "no header
/// values, no body content" contract is easy to test without touching disk.
fn debug_dump_content(req: &HttpRequest) -> String {
    let header_names = req.headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ");
    let body_bytes = req.body.as_ref().map(|b| b.len()).unwrap_or(0);
    format!(
        "timestamp_ms: {}\nurl: {}\nheader_names: {}\nbody_bytes: {}\n",
        now_millis(),
        sanitize_debug_url(&req.url),
        header_names,
        body_bytes,
    )
}

fn sanitize_debug_url(url: &str) -> String {
    let metadata_end = url.find(|c| c == '?' || c == '#').unwrap_or(url.len());
    let base = &url[..metadata_end];

    let Some(scheme_end) = base.find("://") else {
        return base.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = base[authority_start..]
        .find('/')
        .map(|offset| authority_start + offset)
        .unwrap_or(base.len());
    let authority = &base[authority_start..authority_end];
    let Some(userinfo_end) = authority.rfind('@') else {
        return base.to_string();
    };

    format!(
        "{}{}{}",
        &base[..authority_start],
        &authority[userinfo_end + 1..],
        &base[authority_end..],
    )
}

/// Writes `contents` to `path` via a same-directory temp file + rename, so a
/// reader never observes a partial write, and restricts the file to
/// owner-read/write where the OS supports Unix permission bits.
fn write_atomic_private(path: &std::path::Path, contents: &str) {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&tmp, contents).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn spawn_curl(req: &HttpRequest) -> Result<Child, String> {
    debug_log_request(req);

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

/// GET with optional headers. Used for catalog endpoints like `/v1/models`.
/// Retries the same transient failures as [`request`]. Caps wall time via curl `--max-time`.
pub fn request_get(url: &str, headers: &[(String, String)]) -> Result<HttpResponse, String> {
    let mut attempt = 0;
    loop {
        match request_get_once(url, headers) {
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

fn request_get_once(url: &str, headers: &[(String, String)]) -> Result<HttpResponse, RequestError> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-i", "-X", "GET", "--max-time", "12", url]);
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    cmd.arg("-H").arg("Expect:");
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| RequestError::Transport(format!("curl: {e}")))?;
    let output = child
        .wait_with_output()
        .map_err(|e| RequestError::Transport(format!("{e}")))?;
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
            Err((StreamOutcome::Cancelled, _)) => {
                return Err("Request cancelled.".into());
            }
            Err((StreamOutcome::IdleTimeout, _)) => {
                return Err(
                    "Stream idle timeout: no data from the provider for 60s. Retry, or check network/provider status."
                        .into(),
                );
            }
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

    #[test]
    fn test_status_error_auth_is_actionable() {
        let msg = format_status_error(
            401,
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
        );
        assert!(msg.contains("401"), "{msg}");
        assert!(msg.to_ascii_lowercase().contains("authentication") || msg.to_ascii_lowercase().contains("api key"), "{msg}");
        assert!(msg.contains("Incorrect API key"), "{msg}");
    }

    #[test]
    fn test_status_error_rate_limit_is_actionable() {
        let msg = format_status_error(429, r#"{"error":{"message":"Rate limit exceeded"}}"#);
        assert!(msg.contains("429"), "{msg}");
        assert!(msg.to_ascii_lowercase().contains("rate"), "{msg}");
    }

    #[test]
    fn test_status_error_context_limit_detected() {
        let msg = format_status_error(
            400,
            r#"{"error":{"message":"This model's maximum context length is 128000 tokens"}}"#,
        );
        assert!(msg.to_ascii_lowercase().contains("context"), "{msg}");
        assert!(is_context_limit_error(&msg), "{msg}");
    }

    #[test]
    fn test_transport_error_dns() {
        let msg = format_transport_error("Could not resolve host: api.example.com");
        assert!(msg.to_ascii_lowercase().contains("dns") || msg.to_ascii_lowercase().contains("network"), "{msg}");
    }

    /// H-03 regression: request logging must be off unless explicitly
    /// enabled, either via config (`set_debug_logging_enabled`) or the
    /// `CAIRN_DEBUG_HTTP` escape hatch.
    #[test]
    fn debug_logging_disabled_by_default() {
        assert!(!debug_logging_enabled_for(false, None));
    }

    #[test]
    fn debug_logging_enabled_via_config_flag() {
        assert!(debug_logging_enabled_for(true, None));
    }

    #[test]
    fn debug_logging_enabled_via_env_var() {
        assert!(debug_logging_enabled_for(false, Some("1")));
        assert!(debug_logging_enabled_for(false, Some("TRUE")));
        assert!(debug_logging_enabled_for(false, Some("True")));
        assert!(!debug_logging_enabled_for(false, Some("0")));
    }

    /// H-03: even when logging is enabled, the dump must never contain
    /// header values or body content — only metadata — so heuristic secret
    /// redaction has nothing left to fail to catch.
    #[test]
    fn debug_dump_never_contains_header_values_or_body() {
        let req = HttpRequest {
            url: "https://user:secret-url-password@api.example.com/v1/messages?api_key=secret-query#secret-fragment".into(),
            headers: vec![
                ("Authorization".into(), "Bearer sk-ant-supersecretvalue123456".into()),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Some(r#"{"prompt":"delete all my files","api_key":"sk-supersecret"}"#.into()),
        };
        let dump = debug_dump_content(&req);

        assert!(dump.contains("api.example.com"), "{dump}");
        assert!(dump.contains("/v1/messages"), "{dump}");
        assert!(!dump.contains("secret-url-password"), "leaked URL userinfo: {dump}");
        assert!(!dump.contains("secret-query"), "leaked URL query: {dump}");
        assert!(!dump.contains("secret-fragment"), "leaked URL fragment: {dump}");
        assert!(dump.contains("Authorization"), "header *names* are metadata: {dump}");
        assert!(dump.contains("Content-Type"), "{dump}");
        assert!(!dump.contains("sk-ant-supersecretvalue123456"), "leaked header value: {dump}");
        assert!(!dump.contains("sk-supersecret"), "leaked body secret: {dump}");
        assert!(!dump.contains("delete all my files"), "leaked prompt content: {dump}");
        assert!(dump.contains("body_bytes"), "{dump}");
    }

    #[test]
    fn sanitize_debug_url_handles_urls_without_credentials() {
        assert_eq!(
            sanitize_debug_url("https://api.example.com/v1/messages?debug=true#response"),
            "https://api.example.com/v1/messages"
        );
        assert_eq!(sanitize_debug_url("not-a-url?secret=value"), "not-a-url");
    }

    #[test]
    fn write_atomic_private_is_atomic_and_owner_only() {
        let dir = std::env::temp_dir().join(format!("cairn-http-client-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("debug_request.json");

        write_atomic_private(&path, "first");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        write_atomic_private(&path, "second");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "second",
            "second write should fully replace the first, not append or corrupt it"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "debug log must be owner-read/write only, got {mode:o}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
