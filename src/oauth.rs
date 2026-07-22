//! OAuth helpers for provider login (xAI / Grok device-code flow, like zero).
//!
//! Tokens are stored in the OS keyring under `oauth:<provider>` as JSON and
//! never written to the config file. Device-code (RFC 8628) is used so login
//! works in SSH/headless sessions without a local browser callback.

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::json;

const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_DEVICE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const XAI_SCOPES: &str = "openid profile email offline_access grok-cli:access api:access";

#[derive(Debug, Clone)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_at: u64, // unix seconds; 0 = unknown
}

#[derive(Debug, Clone)]
pub struct DeviceAuth {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub interval_secs: u64,
    pub expires_at: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Whether to use the baked-in public Grok-CLI OAuth client id (same idea as zero).
/// Defaults to **on** so `/auth login xai` and the provider picker work without env setup.
/// Opt out with `CAIRN_OAUTH_ALLOW_PRESETS=0` (or `ZERO_OAUTH_ALLOW_PRESETS=0`) and set your own client id.
fn presets_allowed() -> bool {
    match std::env::var("CAIRN_OAUTH_ALLOW_PRESETS")
        .or_else(|_| std::env::var("ZERO_OAUTH_ALLOW_PRESETS"))
    {
        Ok(v) => !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

/// Providers that support browser / device-code OAuth (no API key paste required).
pub fn supports_oauth(provider: &str) -> bool {
    matches!(provider.trim().to_ascii_lowercase().as_str(), "xai")
}

/// xAI client id: env override, or baked-in public Grok-CLI client when presets allowed.
pub fn xai_client_id() -> Result<String, String> {
    if let Ok(id) = std::env::var("CAIRN_OAUTH_XAI_CLIENT_ID")
        .or_else(|_| std::env::var("ZERO_OAUTH_XAI_CLIENT_ID"))
    {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    if presets_allowed() {
        return Ok(XAI_CLIENT_ID.into());
    }
    Err(
        "xAI OAuth needs a client id. Public Grok-CLI client is disabled (CAIRN_OAUTH_ALLOW_PRESETS=0). Set CAIRN_OAUTH_XAI_CLIENT_ID, or re-enable presets."
            .into(),
    )
}

fn form_post(url: &str, body: &str) -> Result<(u16, String), String> {
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "-i",
        "-X",
        "POST",
        url,
        "-H",
        "Content-Type: application/x-www-form-urlencoded",
        "-H",
        "Accept: application/json",
        "-H",
        "Expect:",
        "--data-binary",
        "@-",
    ]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("curl: {e}"))?;
    let body = body.to_string();
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(body.as_bytes());
    }
    let output = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let raw = String::from_utf8_lossy(&output.stdout);
    let split_at = raw
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))
        .unwrap_or(0);
    let status = raw
        .get(..split_at)
        .and_then(|h| h.lines().next())
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = raw.get(split_at..).unwrap_or("").to_string();
    Ok((status, body))
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                urlencoding_minimal(k),
                urlencoding_minimal(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn request_xai_device_code() -> Result<DeviceAuth, String> {
    let client_id = xai_client_id()?;
    let body = form_encode(&[
        ("client_id", &client_id),
        ("scope", XAI_SCOPES),
    ]);
    let (status, resp) = form_post(XAI_DEVICE_URL, &body)?;
    let val = json::parse(&resp).map_err(|e| format!("oauth device response: {e}"))?;
    let obj = val.as_object().ok_or("oauth device response not an object")?;
    if status < 200 || status >= 300 {
        let err = obj
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(format!("oauth device authorization failed: {err} (HTTP {status})"));
    }
    let device_code = obj
        .get("device_code")
        .and_then(|v| v.as_str())
        .ok_or("missing device_code")?
        .to_string();
    let user_code = obj
        .get("user_code")
        .and_then(|v| v.as_str())
        .ok_or("missing user_code")?
        .to_string();
    let verification_uri = obj
        .get("verification_uri")
        .and_then(|v| v.as_str())
        .unwrap_or("https://auth.x.ai/activate")
        .to_string();
    let verification_uri_complete = obj
        .get("verification_uri_complete")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let interval = obj
        .get("interval")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .max(1);
    let expires_in = obj
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(600);
    Ok(DeviceAuth {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        interval_secs: interval,
        expires_at: now_unix() + expires_in,
    })
}

pub fn poll_xai_device_token(auth: &DeviceAuth) -> Result<Token, String> {
    let client_id = xai_client_id()?;
    let mut interval = Duration::from_secs(auth.interval_secs.max(1));
    loop {
        if now_unix() >= auth.expires_at {
            return Err("oauth: device code expired before authorization".into());
        }
        thread::sleep(interval);
        if now_unix() >= auth.expires_at {
            return Err("oauth: device code expired before authorization".into());
        }
        let body = form_encode(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", &auth.device_code),
            ("client_id", &client_id),
        ]);
        let (status, resp) = form_post(XAI_TOKEN_URL, &body)?;
        let val = json::parse(&resp).map_err(|e| format!("oauth token response: {e}"))?;
        let obj = val.as_object().ok_or("oauth token response not an object")?;
        if let Some(access) = obj.get("access_token").and_then(|v| v.as_str()) {
            if !access.is_empty() {
                let expires_in = obj.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(0);
                return Ok(Token {
                    access_token: access.to_string(),
                    refresh_token: obj
                        .get("refresh_token")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    token_type: obj
                        .get("token_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Bearer")
                        .to_string(),
                    expires_at: if expires_in > 0 {
                        now_unix() + expires_in
                    } else {
                        0
                    },
                });
            }
        }
        let err = obj.get("error").and_then(|v| v.as_str()).unwrap_or("");
        match err {
            "authorization_pending" => {}
            "slow_down" => {
                interval += Duration::from_secs(5);
            }
            "expired_token" => {
                return Err("oauth: device code expired before authorization".into());
            }
            "access_denied" => {
                return Err("oauth: authorization denied by the user".into());
            }
            "" => {
                return Err(format!("oauth: device token poll HTTP {status}"));
            }
            other => {
                return Err(format!("oauth: device token error {other:?}"));
            }
        }
    }
}

fn keyring_user(provider: &str) -> String {
    format!("oauth:{}", provider.trim().to_ascii_lowercase())
}

pub fn save_token(provider: &str, token: &Token) -> Result<(), String> {
    let json = format!(
        "{{\"access_token\":\"{}\",\"refresh_token\":\"{}\",\"token_type\":\"{}\",\"expires_at\":{}}}",
        json_escape(&token.access_token),
        json_escape(&token.refresh_token),
        json_escape(&token.token_type),
        token.expires_at
    );
    keyring::Entry::new("cairn-code", &keyring_user(provider))
        .map_err(|e| e.to_string())?
        .set_password(&json)
        .map_err(|e| e.to_string())
}

pub fn load_token(provider: &str) -> Option<Token> {
    let raw = keyring::Entry::new("cairn-code", &keyring_user(provider))
        .ok()?
        .get_password()
        .ok()?;
    let val = json::parse(&raw).ok()?;
    let obj = val.as_object()?;
    let access = obj.get("access_token")?.as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some(Token {
        access_token: access,
        refresh_token: obj
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        token_type: obj
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        expires_at: obj.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

pub fn delete_token(provider: &str) -> Result<bool, String> {
    let entry = keyring::Entry::new("cairn-code", &keyring_user(provider)).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

pub fn access_token(provider: &str) -> Option<String> {
    let tok = load_token(provider)?;
    if tok.expires_at > 0 && tok.expires_at <= now_unix() + 60 {
        // expired or about to expire; refresh not implemented yet
        return None;
    }
    Some(tok.access_token)
}

pub fn status_line(provider: &str) -> String {
    match load_token(provider) {
        Some(t) => {
            let exp = if t.expires_at == 0 {
                "no expiry".into()
            } else if t.expires_at <= now_unix() {
                "expired".into()
            } else {
                format!("expires in {}s", t.expires_at.saturating_sub(now_unix()))
            };
            format!(
                "{provider}: OAuth login present ({exp}, refresh={})",
                if t.refresh_token.is_empty() { "no" } else { "yes" }
            )
        }
        None => format!("{provider}: no OAuth login"),
    }
}

/// Best-effort open a URL in the default browser (Windows / macOS / Linux).
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encode_basic() {
        let s = form_encode(&[("client_id", "abc"), ("scope", "a b")]);
        assert!(s.contains("client_id=abc"));
        assert!(s.contains("scope=a+b") || s.contains("scope=a%20b"));
    }

    #[test]
    fn keyring_user_normalized() {
        assert_eq!(keyring_user("xAI"), "oauth:xai");
    }
}
