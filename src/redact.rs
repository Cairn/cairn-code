//! Secret scrubbing for logs, error text, and other user-visible sinks.
//! Pure Rust (no regex crate): token and key-shaped substrings are replaced
//! with a fixed placeholder so dumps stay useful without leaking credentials.

const REDACTED: &str = "[REDACTED]";

/// Returns a copy of `input` with common secret shapes replaced.
pub fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(consumed) = match_secret_at(bytes, i) {
            out.push_str(REDACTED);
            i += consumed;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// True when a header name is expected to carry credentials.
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization"
        || lower == "x-api-key"
        || lower == "api-key"
        || lower == "x-auth-token"
        || lower == "proxy-authorization"
}

fn match_secret_at(bytes: &[u8], i: usize) -> Option<usize> {
    let rest = &bytes[i..];
    let s = std::str::from_utf8(rest).ok()?;

    for prefix in ["Bearer ", "bearer ", "Basic ", "basic "] {
        if let Some(after) = s.strip_prefix(prefix) {
            let n = token_len(after.as_bytes());
            if n >= 8 {
                return Some(prefix.len() + n);
            }
        }
    }

    for prefix in ["sk-ant-", "sk-or-", "sk-"] {
        if let Some(after) = s.strip_prefix(prefix) {
            let n = key_body_len(after.as_bytes());
            let min = if prefix == "sk-" { 16 } else { 8 };
            if n >= min {
                return Some(prefix.len() + n);
            }
        }
    }

    for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"] {
        if let Some(after) = s.strip_prefix(prefix) {
            let n = key_body_len(after.as_bytes());
            if n >= 12 {
                return Some(prefix.len() + n);
            }
        }
    }

    // xox?- Slack-style tokens
    if s.len() > 5 && s.as_bytes().starts_with(b"xox") && s.as_bytes()[4] == b'-' {
        let n = key_body_len(&s.as_bytes()[5..]);
        if n >= 8 {
            return Some(5 + n);
        }
    }

    // ENV_NAME=value where name looks secret
    if let Some((name, value)) = split_env_assign(s) {
        if name_looks_secret(name) {
            let n = token_len(value.as_bytes());
            if n >= 4 {
                return Some(name.len() + 1 + n);
            }
        }
    }

    // "api_key": "...." and similar JSON string values
    if let Some(n) = match_json_secret_field(s) {
        return Some(n);
    }

    None
}

fn match_json_secret_field(s: &str) -> Option<usize> {
    const KEYS: &[&str] = &[
        "\"api_key\"",
        "\"apiKey\"",
        "\"access_token\"",
        "\"accessToken\"",
        "\"authorization\"",
        "\"password\"",
        "\"secret\"",
    ];
    for key in KEYS {
        if !s.starts_with(key) {
            continue;
        }
        let mut pos = key.len();
        let b = s.as_bytes();
        while pos < b.len() && b[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= b.len() || b[pos] != b':' {
            continue;
        }
        pos += 1;
        while pos < b.len() && b[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= b.len() || b[pos] != b'"' {
            continue;
        }
        pos += 1; // open quote
        let start_val = pos;
        while pos < b.len() && b[pos] != b'"' {
            // skip escaped quote
            if b[pos] == b'\\' && pos + 1 < b.len() {
                pos += 2;
                continue;
            }
            pos += 1;
        }
        if pos >= b.len() {
            continue;
        }
        let val_len = pos - start_val;
        if val_len >= 4 {
            return Some(pos + 1); // include closing quote
        }
    }
    None
}

fn token_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| {
            b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'.' | b'+' | b'/' | b'=')
        })
        .count()
}

fn key_body_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_'))
        .count()
}

fn split_env_assign(s: &str) -> Option<(&str, &str)> {
    let eq = s.find('=')?;
    let name = &s[..eq];
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    if !name.contains('_') && name != name.to_ascii_uppercase() {
        return None;
    }
    Some((name, &s[eq + 1..]))
}

fn name_looks_secret(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    u.contains("API_KEY")
        || u.contains("ACCESS_TOKEN")
        || u.contains("AUTH_TOKEN")
        || u.ends_with("_TOKEN")
        || u.ends_with("_SECRET")
        || u.ends_with("_PASSWORD")
        || u == "PASSWORD"
        || u == "SECRET"
        || u == "TOKEN"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_style_key() {
        let s = "key is sk-abcdefghijklmnopqrstuvwxyz123456 keep";
        let r = redact_secrets(s);
        assert!(!r.contains("sk-abcd"), "{r}");
        assert!(r.contains(REDACTED), "{r}");
        assert!(r.contains("keep"), "{r}");
    }

    #[test]
    fn redacts_anthropic_key() {
        let s = "ANTHROPIC: sk-ant-api03-abcdefghijklmnopqrstuv";
        let r = redact_secrets(s);
        assert!(!r.contains("sk-ant-api03"), "{r}");
        assert!(r.contains(REDACTED), "{r}");
    }

    #[test]
    fn redacts_bearer_token() {
        let s = "Authorization: Bearer abcdefghijklmnop1234";
        let r = redact_secrets(s);
        assert!(!r.contains("abcdefghijklmnop"), "{r}");
        assert!(r.contains(REDACTED), "{r}");
    }

    #[test]
    fn redacts_env_assignment() {
        let s = "OPENAI_API_KEY=sk-notthislongkeyvalue1234567890";
        let r = redact_secrets(s);
        assert!(!r.contains("sk-notthislongkeyvalue"), "{r}");
        assert!(r.contains(REDACTED), "{r}");
    }

    #[test]
    fn redacts_json_api_key_field() {
        let s = r#"{"api_key":"supersecretvalue123","ok":true}"#;
        let r = redact_secrets(s);
        assert!(!r.contains("supersecretvalue"), "{r}");
        assert!(r.contains(REDACTED), "{r}");
        assert!(r.contains("ok"), "{r}");
    }

    #[test]
    fn leaves_normal_text() {
        let s = "read Cargo.toml and fix the build";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn sensitive_headers() {
        assert!(is_sensitive_header("Authorization"));
        assert!(is_sensitive_header("x-api-key"));
        assert!(!is_sensitive_header("Content-Type"));
    }
}
