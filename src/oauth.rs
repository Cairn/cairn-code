//! OAuth helpers for provider login (xAI / Grok device-code flow, like zero).
//!
//! Tokens are stored in the OS keyring under `oauth:<provider>` as JSON and
//! never written to the config file. Device-code (RFC 8628) is used so login
//! works in SSH/headless sessions without a local browser callback.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::json;

/// Serializes refresh attempts so concurrent API calls do not thrash the token endpoint.
static REFRESH_LOCK: Mutex<()> = Mutex::new(());

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
        Ok(v) => !matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
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

/// POST a form body to an OAuth endpoint. Delegates to the hardened HTTP
/// helper so credential-bearing requests ignore the user's `.curlrc` (`-q`),
/// carry connect/total timeouts, are size-bounded, and validate curl's exit
/// status. Returns the raw `(status, body)` because the device-code and
/// refresh flows must read error bodies (e.g. `authorization_pending`).
fn form_post(url: &str, body: &str) -> Result<(u16, String), String> {
    crate::http_client::form_post(url, body)
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding_minimal(k), urlencoding_minimal(v)))
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
    let body = form_encode(&[("client_id", &client_id), ("scope", XAI_SCOPES)]);
    let (status, resp) = form_post(XAI_DEVICE_URL, &body)?;
    let val = json::parse(&resp).map_err(|e| format!("oauth device response: {e}"))?;
    let obj = val
        .as_object()
        .ok_or("oauth device response not an object")?;
    if status < 200 || status >= 300 {
        let err = obj
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(format!(
            "oauth device authorization failed: {err} (HTTP {status})"
        ));
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

pub fn poll_xai_device_token(auth: &DeviceAuth, cancelled: &AtomicBool) -> Result<Token, String> {
    let client_id = xai_client_id()?;
    let mut interval = Duration::from_secs(auth.interval_secs.max(1));
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("oauth: cancelled".into());
        }
        if now_unix() >= auth.expires_at {
            return Err("oauth: device code expired before authorization".into());
        }
        let wait_started = std::time::Instant::now();
        while wait_started.elapsed() < interval {
            if cancelled.load(Ordering::Relaxed) {
                return Err("oauth: cancelled".into());
            }
            let remaining = interval.saturating_sub(wait_started.elapsed());
            thread::sleep(Duration::from_millis(100).min(remaining));
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("oauth: cancelled".into());
        }
        if now_unix() >= auth.expires_at {
            return Err("oauth: device code expired before authorization".into());
        }
        let body = form_encode(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", &auth.device_code),
            ("client_id", &client_id),
        ]);
        let (status, resp) = form_post(XAI_TOKEN_URL, &body)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("oauth: cancelled".into());
        }
        let val = json::parse(&resp).map_err(|e| format!("oauth token response: {e}"))?;
        let obj = val
            .as_object()
            .ok_or("oauth token response not an object")?;
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

/// Single entry point for OAuth credential access.
///
/// The test build deliberately does **not** go through `keyring::Entry`. That
/// type is `keyring::v1::Entry`, and its `new` installs the platform
/// credential store process-wide on first call:
///
/// ```ignore
/// if SET_CREDENTIAL_STORE.compare_exchange(false, true, ..) == Ok(false) {
///     set_credential_store()?   // keyring_core::set_default_store(<platform store>)
/// }
/// ```
///
/// `keyring_core` keeps one default store per process, so that call replaces
/// the mock store the config tests install — and it can land *after* the mock
/// is in place, because it is driven by whichever test first reaches
/// `XaiProvider::get_key` (which checks `has_token` before anything else).
/// The config keyring tests then read and write through the real OS credential
/// store and see none of their own writes. Opening `keyring_core::Entry`
/// directly under `cfg(test)` keeps the platform store from ever being
/// installed. Mirrors the split already used by `config::keyring_entry`.
#[cfg(not(test))]
pub(crate) fn oauth_entry(provider: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("cairn-code", &keyring_user(provider)).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) fn oauth_entry(provider: &str) -> Result<keyring_core::Entry, String> {
    crate::config::init_test_keyring();
    keyring_core::Entry::new("cairn-code", &keyring_user(provider)).map_err(|e| e.to_string())
}

pub fn save_token(provider: &str, token: &Token) -> Result<(), String> {
    let json = format!(
        "{{\"access_token\":\"{}\",\"refresh_token\":\"{}\",\"token_type\":\"{}\",\"expires_at\":{}}}",
        json_escape(&token.access_token),
        json_escape(&token.refresh_token),
        json_escape(&token.token_type),
        token.expires_at
    );
    oauth_entry(provider)?
        .set_password(&json)
        .map_err(|e| e.to_string())
}

pub fn load_token(provider: &str) -> Option<Token> {
    let raw = oauth_entry(provider).ok()?.get_password().ok()?;
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

pub fn has_token(provider: &str) -> bool {
    load_token(provider).is_some()
}

pub fn delete_token(provider: &str) -> Result<bool, String> {
    let _guard = REFRESH_LOCK
        .lock()
        .map_err(|_| "oauth: credential lock poisoned".to_string())?;
    let token = load_token(provider);
    let entry = oauth_entry(provider)?;
    let result = match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e.to_string()),
    };
    if let Some(token) = token {
        clear_matching_oauth_env(provider, &token.access_token);
    }
    result
}

/// Remove a process environment credential only when it came from OAuth.
/// This cleans up tokens mirrored by older code without deleting a genuine API key.
fn clear_matching_oauth_env(provider: &str, access_token: &str) {
    let Some(var) = crate::config::env_var_name(provider) else {
        return;
    };
    if std::env::var(var).ok().as_deref() == Some(access_token) {
        std::env::remove_var(var);
    }
}

/// True when the access token is missing an expiry, or still valid for >60s.
fn access_still_fresh(tok: &Token) -> bool {
    tok.expires_at == 0 || tok.expires_at > now_unix() + 60
}

/// Refresh an xAI access token using a refresh_token grant (RFC 6749).
pub fn refresh_xai_token(refresh_token: &str) -> Result<Token, String> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return Err("oauth: no refresh token".into());
    }
    let client_id = xai_client_id()?;
    let body = form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", &client_id),
    ]);
    let (status, resp) = form_post(XAI_TOKEN_URL, &body)?;
    if status < 200 || status >= 300 {
        let err = json::parse(&resp)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .and_then(|o| {
                let code = o.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                let desc = o
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Some(if desc.is_empty() {
                    format!("oauth refresh failed: {code} (HTTP {status})")
                } else {
                    format!("oauth refresh failed: {code} ({desc}) (HTTP {status})")
                })
            })
            .unwrap_or_else(|| format!("oauth refresh failed (HTTP {status})"));
        return Err(err);
    }
    // Some IdPs omit a new refresh_token; keep the previous one.
    token_from_json_response(&resp, refresh_token)
}

/// Return a usable access token, refreshing via refresh_token when near expiry.
pub fn access_token(provider: &str) -> Option<String> {
    let provider = provider.trim().to_ascii_lowercase();
    let tok = load_token(&provider)?;
    if access_still_fresh(&tok) {
        return Some(tok.access_token);
    }
    if provider != "xai" || tok.refresh_token.is_empty() {
        return None;
    }
    // Serialize refresh; re-check keyring after lock in case another thread won.
    let _guard = REFRESH_LOCK.lock().ok()?;
    let tok = load_token(&provider)?;
    if access_still_fresh(&tok) {
        return Some(tok.access_token);
    }
    if tok.refresh_token.is_empty() {
        return None;
    }
    match refresh_xai_token(&tok.refresh_token) {
        Ok(new_tok) => persist_refreshed_token(&provider, &tok, new_tok, save_token).ok(),
        Err(_) => None,
    }
}

fn persist_refreshed_token<F>(
    provider: &str,
    previous: &Token,
    mut refreshed: Token,
    persist: F,
) -> Result<String, String>
where
    F: FnOnce(&str, &Token) -> Result<(), String>,
{
    if refreshed.refresh_token.is_empty() {
        refreshed.refresh_token = previous.refresh_token.clone();
    }
    let access_token = refreshed.access_token.clone();
    persist(provider, &refreshed)?;
    clear_matching_oauth_env(provider, &previous.access_token);
    Ok(access_token)
}

pub fn status_line(provider: &str) -> String {
    let provider = provider.trim().to_ascii_lowercase();
    match load_token(&provider) {
        Some(t) => {
            let exp = if t.expires_at == 0 {
                "no expiry".into()
            } else if t.expires_at <= now_unix() {
                if t.refresh_token.is_empty() {
                    "expired".into()
                } else {
                    "expired (refresh available)".into()
                }
            } else {
                format!("expires in {}s", t.expires_at.saturating_sub(now_unix()))
            };
            format!(
                "{provider}: OAuth login present ({exp}, refresh={})",
                if t.refresh_token.is_empty() {
                    "no"
                } else {
                    "yes"
                }
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
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Parse a successful OAuth token JSON body into a [`Token`] (shared by device + refresh).
pub fn token_from_json_response(resp: &str, fallback_refresh: &str) -> Result<Token, String> {
    let val = json::parse(resp).map_err(|e| format!("oauth token response: {e}"))?;
    let obj = val
        .as_object()
        .ok_or("oauth token response not an object")?;
    let access = obj
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("oauth: missing access_token")?;
    let expires_in = obj.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(0);
    let refresh = obj
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_refresh);
    Ok(Token {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Guards the `cfg` split on [`oauth_entry`].
    ///
    /// Opening a `keyring::Entry` anywhere in a test build installs the
    /// platform credential store for the whole process, replacing the mock and
    /// making the config keyring tests read the real OS store — which fails
    /// intermittently, only under the full suite, and only depending on which
    /// test reached the keyring first. Asserting the credential is a mock here
    /// catches a regression at its source rather than as a flake elsewhere.
    #[test]
    fn oauth_entry_opens_mock_credentials_in_test_builds() {
        let entry = oauth_entry("mock-store-regression-probe").unwrap();
        assert!(
            entry
                .as_any()
                .downcast_ref::<keyring_core::mock::Cred>()
                .is_some(),
            "oauth_entry must not open keyring::Entry in test builds: doing so \
             installs the platform store process-wide and clobbers the mock"
        );
    }

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

    #[test]
    fn token_from_json_keeps_fallback_refresh() {
        let body = r#"{"access_token":"new-access","expires_in":3600,"token_type":"Bearer"}"#;
        let tok = token_from_json_response(body, "old-refresh").unwrap();
        assert_eq!(tok.access_token, "new-access");
        assert_eq!(tok.refresh_token, "old-refresh");
        assert!(tok.expires_at > now_unix());
    }

    #[test]
    fn token_from_json_prefers_new_refresh() {
        let body = r#"{"access_token":"a","refresh_token":"r2","expires_in":10}"#;
        let tok = token_from_json_response(body, "r1").unwrap();
        assert_eq!(tok.refresh_token, "r2");
    }

    #[test]
    fn supports_oauth_only_xai() {
        assert!(supports_oauth("xai"));
        assert!(supports_oauth("XAI"));
        assert!(!supports_oauth("anthropic"));
        assert!(!supports_oauth("openai"));
    }

    #[test]
    fn access_still_fresh_logic() {
        let mut t = Token {
            access_token: "x".into(),
            refresh_token: "y".into(),
            token_type: "Bearer".into(),
            expires_at: 0,
        };
        assert!(access_still_fresh(&t));
        t.expires_at = now_unix() + 120;
        assert!(access_still_fresh(&t));
        t.expires_at = now_unix() + 10;
        assert!(!access_still_fresh(&t));
    }

    #[test]
    fn refreshed_token_is_persisted() {
        let previous = Token {
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            token_type: "Bearer".into(),
            expires_at: 1,
        };
        let refreshed = Token {
            access_token: "new-access".into(),
            refresh_token: String::new(),
            token_type: "Bearer".into(),
            expires_at: now_unix() + 3600,
        };
        let saved = RefCell::new(None);

        let access = persist_refreshed_token("xai", &previous, refreshed, |provider, token| {
            *saved.borrow_mut() = Some((provider.to_string(), token.clone()));
            Ok(())
        })
        .unwrap();

        assert_eq!(access, "new-access");
        let saved = saved.into_inner().unwrap();
        assert_eq!(saved.0, "xai");
        assert_eq!(saved.1.access_token, "new-access");
        assert_eq!(saved.1.refresh_token, "old-refresh");
    }

    #[test]
    fn refreshed_token_is_not_used_when_persistence_fails() {
        let previous = Token {
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            token_type: "Bearer".into(),
            expires_at: 1,
        };
        let refreshed = Token {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            token_type: "Bearer".into(),
            expires_at: now_unix() + 3600,
        };

        let result = persist_refreshed_token("xai", &previous, refreshed, |_, _| {
            Err("keyring unavailable".into())
        });

        assert!(result.is_err());
    }

    #[test]
    fn logout_clears_only_matching_oauth_environment_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var_os("XAI_API_KEY");

        std::env::set_var("XAI_API_KEY", "oauth-access");
        clear_matching_oauth_env("xai", "oauth-access");
        assert!(std::env::var_os("XAI_API_KEY").is_none());

        std::env::set_var("XAI_API_KEY", "old-oauth-access");
        let previous = Token {
            access_token: "old-oauth-access".into(),
            refresh_token: "old-refresh".into(),
            token_type: "Bearer".into(),
            expires_at: 1,
        };
        let refreshed = Token {
            access_token: "new-oauth-access".into(),
            refresh_token: "new-refresh".into(),
            token_type: "Bearer".into(),
            expires_at: now_unix() + 3600,
        };
        persist_refreshed_token("xai", &previous, refreshed, |_, _| Ok(())).unwrap();
        assert!(std::env::var_os("XAI_API_KEY").is_none());

        std::env::set_var("XAI_API_KEY", "user-api-key");
        clear_matching_oauth_env("xai", "oauth-access");
        assert_eq!(std::env::var("XAI_API_KEY").unwrap(), "user-api-key");

        match original {
            Some(value) => std::env::set_var("XAI_API_KEY", value),
            None => std::env::remove_var("XAI_API_KEY"),
        }
    }
}
