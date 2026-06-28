use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

pub struct HttpResponse {
    pub body: String,
}

fn temp_body_path() -> PathBuf {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("cairn_body_{id}.json"))
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

pub fn request(req: &HttpRequest) -> Result<HttpResponse, String> {
    let mut args = vec!["-sS".to_string()];
    args.push("-w".to_string());
    args.push("\n%{http_code}\n".to_string());

    let temp_file = if let Some(body) = &req.body {
        let path = temp_body_path();
        std::fs::write(&path, body).map_err(|e| format!("write temp: {e}"))?;
        args.push("--data-binary".to_string());
        args.push(format!("@{}", path.to_string_lossy()));
        Some(path)
    } else {
        None
    };

    for (name, value) in &req.headers {
        args.push("-H".to_string());
        args.push(format!("{name}: {value}"));
    }

    args.push(req.url.clone());

    let child = Command::new("curl")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

    let output = child.wait_with_output().map_err(|e| format!("curl failed: {e}"))?;

    if let Some(path) = &temp_file {
        let _ = std::fs::remove_file(path);
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl exited with {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim_end();

    let (body, status) = if let Some(last_nl) = trimmed.rfind('\n') {
        let code_str = trimmed[last_nl + 1..].trim();
        let st = code_str.parse::<u16>().unwrap_or(200);
        (trimmed[..last_nl].to_string(), st)
    } else {
        (trimmed.to_string(), 200)
    };

    if status < 100 || status >= 300 {
        return Err(format!("HTTP {status}: {body}"));
    }

    Ok(HttpResponse { body })
}

pub fn request_streaming<F>(req: &HttpRequest, mut on_line: F) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mut args = vec!["-sS".to_string()];
    args.push("-w".to_string());
    args.push("\n%{http_code}\n".to_string());

    let temp_file = if let Some(body) = &req.body {
        let path = temp_body_path();
        std::fs::write(&path, body).map_err(|e| format!("write temp: {e}"))?;
        debug_log_request(&req.url, body);
        args.push("--data-binary".to_string());
        args.push(format!("@{}", path.to_string_lossy()));
        Some(path)
    } else {
        None
    };

    for (name, value) in &req.headers {
        args.push("-H".to_string());
        args.push(format!("{name}: {value}"));
    }

    args.push(req.url.clone());

    let mut child = Command::new("curl")
        .args(&args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout from curl")?;
    let reader = BufReader::new(stdout);

    let mut prev_line = String::new();
    let mut http_status: Option<u16> = None;
    let mut http_body = String::new();
    let mut last_non_empty = String::new();

    for line in reader.lines() {
        match line {
            Ok(l) => {
                if !prev_line.is_empty() {
                    on_line(&prev_line);
                    http_body.push_str(&prev_line);
                    http_body.push('\n');
                }
                prev_line = l.clone();
                if !l.trim().is_empty() {
                    last_non_empty = l;
                }
            }
            Err(e) => return Err(format!("read error: {e}")),
        }
    }

    if !prev_line.is_empty() {
        http_status = prev_line.trim().parse::<u16>().ok();
    } else if !last_non_empty.is_empty() {
        http_status = last_non_empty.trim().parse::<u16>().ok();
    }

    let _ = child.wait();

    if let Some(path) = &temp_file {
        let _ = std::fs::remove_file(path);
    }

    let status = http_status.unwrap_or(200);
    if status < 100 || status >= 300 {
        return Err(format!("HTTP {status}: {}", http_body.trim_end()));
    }

    Ok(())
}
