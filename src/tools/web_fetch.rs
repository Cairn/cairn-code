use std::process::{Command, Stdio};
use super::registry::Tool;

pub struct WebFetchTool;

impl Tool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }
    fn description(&self) -> &str { "Fetch content from a URL and extract text" }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let url = obj.get("url").and_then(|v| v.as_str()).ok_or("url required")?;

        let child = Command::new("curl")
            .args(["-sS", "-L", url])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl: {e}"))?;

        let output = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        let body = String::from_utf8_lossy(&output.stdout);

        let text = html_to_text(&body);
        Ok(text)
    }
}

fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_name = String::new();

    for c in html.chars() {
        if in_script || in_style {
            if c == '<' {
                tag_name.clear();
            } else if c == '>' {
                if tag_name == "/script" { in_script = false; }
                if tag_name == "/style" { in_style = false; }
            } else if tag_name.len() < 10 {
                tag_name.push(c);
            }
            continue;
        }

        match c {
            '<' => {
                in_tag = true;
                tag_name.clear();
            }
            '>' => {
                in_tag = false;
                let tn = tag_name.trim().to_lowercase();
                if tn == "script" || tn.starts_with("script ") { in_script = true; }
                if tn == "style" || tn.starts_with("style ") { in_style = true; }
                if tn == "br" || tn == "br/" || tn == "/p" || tn == "/div" || tn == "/tr" || tn == "/li" || tn == "/h1" || tn == "/h2" || tn == "/h3" || tn == "/h4" {
                    text.push('\n');
                }
            }
            _ => {
                if !in_tag {
                    text.push(c);
                } else {
                    tag_name.push(c);
                }
            }
        }
    }

    // Collapse whitespace: preserve single spaces, fold consecutive runs, strip leading/trailing per line
    let mut result = String::new();
    let mut prev_was_space = false;
    let mut at_line_start = true;
    for c in text.chars() {
        if c == ' ' || c == '\t' {
            if !prev_was_space && !at_line_start {
                result.push(' ');
                prev_was_space = true;
            }
        } else if c == '\n' {
            result.push('\n');
            prev_was_space = false;
            at_line_start = true;
        } else {
            result.push(c);
            prev_was_space = false;
            at_line_start = false;
        }
    }

    let result = result.trim().to_string();
    if result.len() > 10000 {
        format!("{}...\n[truncated at 10000 chars]", &result[..10000])
    } else {
        result
    }
}
