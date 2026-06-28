use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

pub struct HttpResponse {
    pub body: String,
}

fn build_curl_args(req: &HttpRequest, include_status: bool) -> Vec<String> {
    let mut args = vec!["-sS".to_string()];

    if let Some(body) = &req.body {
        args.push("-X".to_string());
        args.push("POST".to_string());
        args.push("-d".to_string());
        args.push(body.clone());
    }

    if include_status {
        args.push("-w".to_string());
        args.push("\n%{http_code}\n".to_string());
    }

    for (name, value) in &req.headers {
        args.push("-H".to_string());
        args.push(format!("{name}: {value}"));
    }

    args.push(req.url.clone());
    args
}

fn run_curl(args: &[String]) -> Result<(u16, String), String> {
    let child = Command::new("curl")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

    let output = child.wait_with_output().map_err(|e| format!("curl failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl exited with {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim_end();

    // Parse status code from last line (added by -w "\n%{http_code}\n")
    let (body, status) = if let Some(last_nl) = trimmed.rfind('\n') {
        let code_str = trimmed[last_nl + 1..].trim();
        let st = code_str.parse::<u16>().unwrap_or(200);
        (trimmed[..last_nl].to_string(), st)
    } else {
        (trimmed.to_string(), 200)
    };

    Ok((status, body))
}

pub fn request(req: &HttpRequest) -> Result<HttpResponse, String> {
    let args = build_curl_args(req, true);
    let (_status, body) = run_curl(&args)?;
    Ok(HttpResponse { body })
}

pub fn request_streaming<F>(req: &HttpRequest, mut on_line: F) -> Result<(), String>
where
    F: FnMut(&str),
{
    let args = build_curl_args(req, false);

    let mut child = Command::new("curl")
        .args(&args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout from curl")?;
    let reader = BufReader::new(stdout);

    for line in reader.lines() {
        match line {
            Ok(l) => on_line(&l),
            Err(e) => return Err(format!("read error: {e}")),
        }
    }

    let status = child.wait().map_err(|e| format!("wait failed: {e}"))?;
    if !status.success() {
        return Err(format!("curl exited with {status}"));
    }

    Ok(())
}
