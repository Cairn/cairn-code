use std::io::{BufRead, BufReader};
use std::sync::OnceLock;

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

pub struct HttpResponse {
    pub body: String,
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(ureq::Agent::new_with_defaults)
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
    let mut builder = agent().post(&req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    let resp = if let Some(body) = &req.body {
        debug_log_request(&req.url, body);
        builder.send(body.as_bytes())
    } else {
        builder.send_empty()
    }
    .map_err(|e| format!("{e}"))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        let body = resp.into_body().read_to_string().unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }

    let body = resp.into_body().read_to_string()
        .map_err(|e| format!("read body: {e}"))?;
    Ok(HttpResponse { body })
}

pub fn request_streaming<F>(req: &HttpRequest, mut on_line: F) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mut builder = agent().post(&req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    if let Some(body) = &req.body {
        debug_log_request(&req.url, body);
    }

    let resp = if let Some(body) = &req.body {
        builder.send(body.as_bytes())
    } else {
        builder.send_empty()
    }
    .map_err(|e| format!("{e}"))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        let body = resp.into_body().read_to_string().unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }

    let reader = BufReader::with_capacity(64 * 1024, resp.into_body().into_reader());
    for line in reader.lines() {
        match line {
            Ok(l) => {
                if !l.is_empty() {
                    on_line(&l);
                }
            }
            Err(e) => return Err(format!("read error: {e}")),
        }
    }

    Ok(())
}
