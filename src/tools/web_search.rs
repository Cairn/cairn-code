use super::registry::Tool;
use std::process::{Command, Stdio};

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web using DuckDuckGo"
    }
    fn needs_permission(&self) -> bool {
        false
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let query = obj
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("query required")?;

        let url = format!("https://lite.duckduckgo.com/lite/?q={}", urlencode(query));

        let child = Command::new("curl")
            .args(["-sS", "-L", &url])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl: {e}"))?;

        let output = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        let html = String::from_utf8_lossy(&output.stdout);

        let results = parse_ddg_results(&html);
        if results.is_empty() {
            return Ok("No results found.".into());
        }

        let mut buf = String::new();
        for (i, (title, url, snippet)) in results.iter().enumerate() {
            buf.push_str(&format!(
                "{}. {}\n   URL: {}\n   {}\n\n",
                i + 1,
                title,
                url,
                snippet
            ));
        }
        buf.push_str(&format!("{} results returned.", results.len()));
        Ok(buf)
    }
}

fn parse_ddg_results(html: &str) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    let mut pos = 0;

    while let Some(start) = html[pos..].find("<a rel=\"nofollow\" href=") {
        let start_abs = pos + start;

        let href_start = html[start_abs..].find("href=\"").map(|i| start_abs + i + 6);
        let href_end = href_start.and_then(|i| html[i..].find('"').map(|j| i + j));
        let href = href_end
            .map(|i| &html[href_start.unwrap()..i])
            .unwrap_or("");

        let title_start = html[start_abs..].find('>').map(|i| start_abs + i + 1);
        let title_end = title_start.and_then(|i| html[i..].find("</a>").map(|j| i + j));
        let title = title_end
            .map(|i| &html[title_start.unwrap()..i])
            .unwrap_or("");
        let title = strip_html(title);

        let remaining = &html[start_abs..];
        let snip_start = remaining
            .find("class='result-snippet'")
            .and_then(|i| remaining[i..].find('>').map(|j| start_abs + i + j + 1));
        let snip_end = snip_start.and_then(|i| html[i..].find("</td>").map(|j| i + j));
        let snippet = snip_end
            .map(|i| &html[snip_start.unwrap()..i])
            .unwrap_or("");
        let snippet = strip_html(snippet);

        // Resolve redirect URL
        let resolved = if href.contains("uddg=") {
            if let Some(encoded) = href.split("uddg=").nth(1) {
                let decoded = url_decode(encoded.split('&').next().unwrap_or(encoded));
                decoded
            } else {
                href.to_string()
            }
        } else if !href.starts_with("http") {
            format!("https://{href}")
        } else {
            href.to_string()
        };

        if !title.is_empty() {
            results.push((title, resolved, snippet));
        }

        pos = start_abs + 10;
        if results.len() >= 5 {
            break;
        }
    }

    results
}

fn strip_html(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ => {
                if !in_tag {
                    result.push(c);
                }
            }
        }
    }
    result.trim().to_string()
}

fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => '+'.to_string(),
            c => format!("%{:02X}", c as u8),
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(code) = u8::from_str_radix(&hex, 16) {
                result.push(code as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
