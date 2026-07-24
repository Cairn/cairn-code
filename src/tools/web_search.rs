use super::process_runner::{self, ByteLimitedRunError};
use super::registry::Tool;
use std::process::Command;

const MAX_RESPONSE_BYTES: usize = 1_000_000;
const MAX_STDERR_BYTES: usize = 16 * 1024;

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

        let mut command = Command::new("curl");
        command.args([
            "-q",
            "-sS",
            "-L",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            &url,
        ]);
        let html = run_search_command(command)?;
        let html = String::from_utf8_lossy(&html);

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

fn run_search_command(command: Command) -> Result<Vec<u8>, String> {
    let output =
        process_runner::run_with_byte_limits(command, None, MAX_RESPONSE_BYTES, MAX_STDERR_BYTES)
            .map_err(format_search_process_error)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(format!(
            "curl: {}",
            if detail.is_empty() {
                "request failed"
            } else {
                detail
            }
        ));
    }
    Ok(output.stdout)
}

fn format_search_process_error(error: ByteLimitedRunError) -> String {
    match error {
        ByteLimitedRunError::Spawn(reason) => format!("curl: {reason}"),
        ByteLimitedRunError::StdoutLimit {
            limit,
            cleanup_error,
        } => process_runner::with_cleanup(
            format!("web_search: response exceeds {limit} bytes"),
            &cleanup_error,
        ),
        ByteLimitedRunError::StderrLimit {
            limit,
            cleanup_error,
        } => process_runner::with_cleanup(
            format!("curl: stderr exceeds {limit} bytes"),
            &cleanup_error,
        ),
        ByteLimitedRunError::Read {
            stream,
            reason,
            cleanup_error,
        } => process_runner::with_cleanup(
            format!("curl: failed reading {stream}: {reason}"),
            &cleanup_error,
        ),
        ByteLimitedRunError::Wait(reason) => format!("curl: wait failed: {reason}"),
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
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            byte => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                result.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            result.push(b' ');
        } else {
            result.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_command(stderr: bool, bytes: usize) -> Command {
        let mut command = Command::new(if cfg!(windows) { "powershell" } else { "bash" });
        if cfg!(windows) {
            let stream = if stderr { "Error" } else { "Out" };
            command
                .arg("-Command")
                .arg(format!("[Console]::{stream}.Write(('x' * {bytes}))"));
        } else {
            let redirect = if stderr { " >&2" } else { "" };
            command
                .arg("-c")
                .arg(format!("head -c {bytes} /dev/zero{redirect}"));
        }
        command
    }

    #[test]
    fn search_command_accepts_normal_response() {
        let response = run_search_command(output_command(false, 128)).unwrap();
        assert_eq!(response.len(), 128);
    }

    #[test]
    fn search_command_rejects_fast_oversized_stdout() {
        let error = run_search_command(output_command(false, MAX_RESPONSE_BYTES + 1)).unwrap_err();
        assert!(error.contains("response exceeds"), "{error}");
    }

    #[test]
    fn search_command_rejects_fast_oversized_stderr() {
        let response = run_search_command(output_command(false, 1)).unwrap();
        assert_eq!(response.len(), 1);

        let error = run_search_command(output_command(true, MAX_STDERR_BYTES + 1)).unwrap_err();
        assert!(error.contains("stderr exceeds"), "{error}");
    }

    #[test]
    fn url_codec_preserves_accented_cjk_and_emoji_text() {
        let text = "café 世界 🙂";
        let encoded = "caf%C3%A9+%E4%B8%96%E7%95%8C+%F0%9F%99%82";

        assert_eq!(urlencode(text), encoded);
        assert_eq!(url_decode(encoded), text);
        assert_eq!(url_decode("100% 🙂"), "100% 🙂");
    }

    #[test]
    fn result_parser_decodes_unicode_redirect_urls() {
        let html = r#"<a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fcaf%C3%A9%2F%E4%B8%96%E7%95%8C%2F%F0%9F%99%82">Unicode</a>"#;

        let results = parse_ddg_results(html);
        assert_eq!(results[0].1, "https://example.com/café/世界/🙂");
    }
}
