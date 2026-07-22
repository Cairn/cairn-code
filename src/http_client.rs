use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

pub struct HttpResponse {
    pub body: String,
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

pub fn request(req: &HttpRequest) -> Result<HttpResponse, String> {
    let child = spawn_curl(req)?;
    let output = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl: {}", stderr.trim()));
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
        return Err(format!("HTTP {status}: {}", body.trim()));
    }

    Ok(HttpResponse { body })
}

pub fn request_streaming<F>(req: &HttpRequest, mut on_line: F) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mut child = spawn_curl(req)?;
    let stdout = child.stdout.take().ok_or("curl: no stdout")?;
    let reader = BufReader::with_capacity(64 * 1024, stdout);

    #[derive(PartialEq)]
    enum State {
        Status,
        Headers,
        Body,
    }

    let mut state = State::Status;
    let mut status: u16 = 0;
    let mut error_body = String::new();

    for line in reader.lines() {
        let line = line.map_err(|e| format!("read error: {e}"))?;
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
                        on_line(&line);
                    }
                } else if !line.is_empty() {
                    error_body.push_str(&line);
                    error_body.push('\n');
                }
            }
        }
    }

    let exit = child.wait().map_err(|e| format!("curl: {e}"))?;

    if status == 0 {
        let mut stderr = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        if !exit.success() {
            return Err(format!("curl: {}", stderr.trim()));
        }
        return Err("curl: no response".into());
    }

    if status < 200 || status >= 300 {
        return Err(format!("HTTP {status}: {}", error_body.trim()));
    }

    Ok(())
}
