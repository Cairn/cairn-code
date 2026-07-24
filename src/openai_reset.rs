//! OpenAI ChatGPT subscription rate-limit reset credits.
//!
//! Reverse-engineered from the open-source Codex CLI
//! (`openai/codex` backend-client rate-limit reset paths):
//!
//! - List:  `GET  {base}/wham/rate-limit-reset-credits`
//! - Redeem: `POST {base}/wham/rate-limit-reset-credits/consume`
//!   body: `{ "redeem_request_id": "...", "credit_id"?: "..." }`
//!
//! Base URL defaults to `https://chatgpt.com/backend-api` (ChatGPT path style).
//! This only works with ChatGPT OAuth subscription tokens, not plain API keys.
//! Credentials are resolved from Codex `auth.json` (after `codex login`) or a
//! cairn-stored `oauth:openai` token that includes an access token.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::http_client::{self, HttpRequest};
use crate::json;
use crate::oauth;

const DEFAULT_CHATGPT_BACKEND: &str = "https://chatgpt.com/backend-api";

/// Where ChatGPT OAuth credentials were loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    /// `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`).
    CodexAuthJson,
    /// Cairn keyring entry `oauth:openai`.
    CairnOauth,
}

/// ChatGPT OAuth credentials used for WHAM / rate-limit-reset endpoints.
#[derive(Debug, Clone)]
pub struct ChatGptAuth {
    pub access_token: String,
    pub account_id: Option<String>,
    pub source: AuthSource,
}

/// One banked reset credit (referral / promo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetCredit {
    pub id: String,
    pub reset_type: String,
    pub status: String,
    pub granted_at: Option<String>,
    pub expires_at: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Payload from list rate-limit-reset-credits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetCreditsDetails {
    pub available_count: i64,
    pub credits: Vec<ResetCredit>,
}

/// Outcome of consuming a reset credit (Codex `ConsumeRateLimitResetCreditCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeOutcome {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeResult {
    pub outcome: ConsumeOutcome,
    pub windows_reset: i64,
}

/// Resolve ChatGPT OAuth credentials for rate-limit reset.
///
/// Order:
/// 1. Cairn `oauth:openai` keyring token (access_token required).
/// 2. Codex CLI `auth.json` tokens (ChatGPT subscription login).
///
/// Plain `OPENAI_API_KEY` / config API keys are intentionally ignored.
pub fn resolve_chatgpt_oauth() -> Result<ChatGptAuth, String> {
    if let Some(auth) = auth_from_cairn_oauth() {
        return Ok(auth);
    }
    if let Some(auth) = auth_from_codex_auth_json(&codex_auth_path()) {
        return Ok(auth);
    }
    Err(
        "no ChatGPT OAuth credentials found. Sign in with Codex CLI (`codex login`) \
         so ~/.codex/auth.json has tokens, or store an OpenAI OAuth access token in \
         the cairn keyring as oauth:openai. A plain OPENAI_API_KEY cannot redeem resets."
            .into(),
    )
}

fn auth_from_cairn_oauth() -> Option<ChatGptAuth> {
    let tok = oauth::load_token("openai")?;
    if tok.access_token.is_empty() {
        return None;
    }
    // Optional account_id may be stored alongside the standard oauth token fields.
    let account_id = oauth_account_id_from_keyring("openai");
    Some(ChatGptAuth {
        access_token: tok.access_token,
        account_id,
        source: AuthSource::CairnOauth,
    })
}

fn oauth_account_id_from_keyring(provider: &str) -> Option<String> {
    let raw = oauth::oauth_entry(provider).ok()?.get_password().ok()?;
    let val = json::parse(&raw).ok()?;
    let id = val
        .as_object()?
        .get("account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    Some(id)
}

/// `$CODEX_HOME/auth.json`, else `~/.codex/auth.json`.
pub fn codex_auth_path() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("auth.json");
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("auth.json")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Parse Codex `auth.json` for ChatGPT OAuth tokens.
pub fn auth_from_codex_auth_json(path: &Path) -> Option<ChatGptAuth> {
    let raw = fs::read_to_string(path).ok()?;
    parse_codex_auth_json(&raw)
}

/// Pure parser for unit tests (no filesystem).
pub fn parse_codex_auth_json(raw: &str) -> Option<ChatGptAuth> {
    let val = json::parse(raw).ok()?;
    let obj = val.as_object()?;
    let tokens = obj.get("tokens")?.as_object()?;
    let access = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let mut account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if account_id.is_none() {
        if let Some(id_token) = tokens.get("id_token").and_then(|v| v.as_str()) {
            account_id = chatgpt_account_id_from_jwt(id_token);
        }
    }
    Some(ChatGptAuth {
        access_token: access,
        account_id,
        source: AuthSource::CodexAuthJson,
    })
}

/// Best-effort extract of `chatgpt_account_id` from a JWT payload (no crypto verify).
pub fn chatgpt_account_id_from_jwt(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let _sig = parts.next()?;
    let bytes = base64url_decode(payload_b64)?;
    let text = String::from_utf8(bytes).ok()?;
    let val = json::parse(&text).ok()?;
    let obj = val.as_object()?;
    // Nested under https://api.openai.com/auth in ChatGPT id tokens.
    if let Some(auth) = obj
        .get("https://api.openai.com/auth")
        .and_then(|v| v.as_object())
    {
        if let Some(id) = auth
            .get("chatgpt_account_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return Some(id.to_string());
        }
    }
    obj.get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    // Pad to multiple of 4 for standard alphabet decode after url→std mapping.
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    base64_std_decode(&s)
}

fn base64_std_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 256] = &{
        let mut t = [0xffu8; 256];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            t[(b'a' + i) as usize] = 26 + i;
            i += 1;
        }
        i = 0;
        while i < 10 {
            t[(b'0' + i) as usize] = 52 + i;
            i += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        let v = TABLE[b as usize];
        if v == 0xff {
            return None;
        }
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn chatgpt_backend_base() -> String {
    std::env::var("CAIRN_CHATGPT_BACKEND")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_CHATGPT_BACKEND.to_string())
}

/// List URL for ChatGPT path style (matches Codex PathStyle::ChatGptApi).
pub fn rate_limit_reset_credits_url(base: &str) -> String {
    format!(
        "{}/wham/rate-limit-reset-credits",
        base.trim_end_matches('/')
    )
}

/// Consume URL for ChatGPT path style.
pub fn consume_rate_limit_reset_credit_url(base: &str) -> String {
    format!(
        "{}/wham/rate-limit-reset-credits/consume",
        base.trim_end_matches('/')
    )
}

fn auth_headers(auth: &ChatGptAuth) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "Authorization".into(),
            format!("Bearer {}", auth.access_token),
        ),
        ("Accept".into(), "application/json".into()),
        ("User-Agent".into(), "cairn-code".into()),
    ];
    if let Some(id) = auth.account_id.as_deref().filter(|s| !s.is_empty()) {
        headers.push(("ChatGPT-Account-Id".into(), id.to_string()));
    }
    headers
}

/// List available banked rate-limit reset credits for the signed-in account.
pub fn list_reset_credits(auth: &ChatGptAuth) -> Result<ResetCreditsDetails, String> {
    let url = rate_limit_reset_credits_url(&chatgpt_backend_base());
    let resp = http_client::request_get(&url, &auth_headers(auth))?;
    parse_reset_credits_details(&resp.body)
}

/// Parse list response JSON (Codex `RateLimitResetCreditsDetails` shape).
pub fn parse_reset_credits_details(body: &str) -> Result<ResetCreditsDetails, String> {
    let val = json::parse(body).map_err(|e| format!("reset credits response: {e}"))?;
    let obj = val
        .as_object()
        .ok_or("reset credits response not an object")?;
    let available_count = obj
        .get("available_count")
        .and_then(|v| v.as_u64())
        .map(|u| u as i64)
        .unwrap_or(0);
    let mut credits = Vec::new();
    if let Some(arr) = obj.get("credits").and_then(|v| v.as_array()) {
        for item in arr {
            let Some(c) = item.as_object() else {
                continue;
            };
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            credits.push(ResetCredit {
                id,
                reset_type: c
                    .get("reset_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                status: c
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                granted_at: c
                    .get("granted_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                expires_at: c
                    .get("expires_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                title: c
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                description: c
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }
    }
    Ok(ResetCreditsDetails {
        available_count,
        credits,
    })
}

/// Redeem one banked reset credit (Codex `consume_rate_limit_reset_credit`).
///
/// When `credit_id` is `None`, the backend picks the next available credit.
pub fn consume_reset_credit(
    auth: &ChatGptAuth,
    credit_id: Option<&str>,
) -> Result<ConsumeResult, String> {
    let url = consume_rate_limit_reset_credit_url(&chatgpt_backend_base());
    let redeem_request_id = new_redeem_request_id();
    let body = consume_request_body(&redeem_request_id, credit_id);
    let mut headers = auth_headers(auth);
    headers.push(("Content-Type".into(), "application/json".into()));
    let req = HttpRequest {
        url,
        headers,
        body: Some(body),
    };
    let resp = http_client::request(&req)?;
    parse_consume_result(&resp.body)
}

pub fn consume_request_body(redeem_request_id: &str, credit_id: Option<&str>) -> String {
    match credit_id {
        Some(id) if !id.is_empty() => format!(
            "{{\"redeem_request_id\":{},\"credit_id\":{}}}",
            json_string(redeem_request_id),
            json_string(id)
        ),
        _ => format!(
            "{{\"redeem_request_id\":{}}}",
            json_string(redeem_request_id)
        ),
    }
}

fn json_string(s: &str) -> String {
    // Minimal JSON string escape for API bodies we control.
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn parse_consume_result(body: &str) -> Result<ConsumeResult, String> {
    let val = json::parse(body).map_err(|e| format!("consume reset response: {e}"))?;
    let obj = val
        .as_object()
        .ok_or("consume reset response not an object")?;
    let code = obj
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let outcome = match code {
        "reset" => ConsumeOutcome::Reset,
        "nothing_to_reset" | "nothingToReset" => ConsumeOutcome::NothingToReset,
        "no_credit" | "noCredit" => ConsumeOutcome::NoCredit,
        "already_redeemed" | "alreadyRedeemed" => ConsumeOutcome::AlreadyRedeemed,
        _ => ConsumeOutcome::Unknown,
    };
    let windows_reset = obj
        .get("windows_reset")
        .and_then(|v| v.as_u64())
        .map(|u| u as i64)
        .unwrap_or(0);
    Ok(ConsumeResult {
        outcome,
        windows_reset,
    })
}

fn new_redeem_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("cairn-{pid:x}-{nanos:x}")
}

/// Format a human-readable summary of available credits.
pub fn format_credits_summary(details: &ResetCreditsDetails, source: AuthSource) -> String {
    let src = match source {
        AuthSource::CodexAuthJson => "Codex auth.json",
        AuthSource::CairnOauth => "cairn oauth:openai",
    };
    let mut lines = vec![
        format!("OpenAI ChatGPT rate-limit resets ({src})"),
        format!("Available banked resets: {}", details.available_count),
    ];
    let available: Vec<_> = details
        .credits
        .iter()
        .filter(|c| c.status == "available")
        .collect();
    if available.is_empty() {
        if details.available_count > 0 {
            lines.push(
                "Credits are available but detail rows were not returned; \
                 run `/reset apply` to redeem the next credit."
                    .into(),
            );
        } else {
            lines.push(
                "No banked resets right now. Resets come from ChatGPT Plus/Pro promos \
                 and referrals (usable for a limited time after grant)."
                    .into(),
            );
        }
    } else {
        lines.push("Credits:".into());
        for (i, c) in available.iter().enumerate() {
            let title = c
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Full reset");
            let exp = c
                .expires_at
                .as_deref()
                .map(|e| format!(" expires {e}"))
                .unwrap_or_default();
            lines.push(format!("  {}. {}  id={}{exp}", i + 1, title, c.id));
            if let Some(desc) = c
                .description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                lines.push(format!("     {desc}"));
            }
        }
    }
    lines.push(
        "Commands: `/reset` list · `/reset apply` redeem next · `/reset apply <credit_id>`".into(),
    );
    lines.join("\n")
}

pub fn format_consume_result(result: &ConsumeResult) -> String {
    match result.outcome {
        ConsumeOutcome::Reset => format!(
            "Rate limits reset successfully (windows_reset={}). You can continue using Codex/ChatGPT quota.",
            result.windows_reset
        ),
        ConsumeOutcome::NothingToReset => {
            "Nothing to reset: usage windows are already clear (no credit consumed, or no active limit)."
                .into()
        }
        ConsumeOutcome::NoCredit => {
            "No banked reset credit available to redeem.".into()
        }
        ConsumeOutcome::AlreadyRedeemed => {
            "That reset credit was already redeemed (idempotent replay).".into()
        }
        ConsumeOutcome::Unknown => format!(
            "Unexpected reset response (windows_reset={}). Check /reset for remaining credits.",
            result.windows_reset
        ),
    }
}

/// Run `/reset` subcommands: list (default) or apply.
pub fn run_reset_command(args: &[&str]) -> Result<String, String> {
    let auth = resolve_chatgpt_oauth()?;
    let sub = args.first().copied().unwrap_or("list");
    match sub {
        "list" | "status" | "show" => {
            let details = list_reset_credits(&auth)?;
            Ok(format_credits_summary(&details, auth.source))
        }
        "apply" | "consume" | "use" | "now" => {
            let credit_id = args.get(1).copied();
            // Prefer explicit id; else first available credit id when known.
            let credit_id = match credit_id {
                Some(id) => Some(id.to_string()),
                None => {
                    let details = list_reset_credits(&auth)?;
                    details
                        .credits
                        .iter()
                        .find(|c| c.status == "available")
                        .map(|c| c.id.clone())
                }
            };
            let result = consume_reset_credit(&auth, credit_id.as_deref())?;
            Ok(format_consume_result(&result))
        }
        other => Err(format!(
            "Unknown /reset subcommand '{other}'. Use /reset, /reset list, or /reset apply [credit_id]."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_match_codex_chatgpt_path_style() {
        assert_eq!(
            rate_limit_reset_credits_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits"
        );
        assert_eq!(
            consume_rate_limit_reset_credit_url("https://chatgpt.com/backend-api/"),
            "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume"
        );
    }

    #[test]
    fn consume_body_omits_credit_when_none() {
        let body = consume_request_body("redeem-123", None);
        assert_eq!(body, r#"{"redeem_request_id":"redeem-123"}"#);
        let body = consume_request_body("redeem-456", Some("credit-1"));
        assert_eq!(
            body,
            r#"{"redeem_request_id":"redeem-456","credit_id":"credit-1"}"#
        );
    }

    #[test]
    fn parse_list_and_consume_payloads() {
        let details = parse_reset_credits_details(
            r#"{
            "credits": [
              {
                "id": "credit-1",
                "reset_type": "codex_rate_limits",
                "status": "available",
                "granted_at": "2026-06-17T00:00:00Z",
                "expires_at": "2026-07-17T00:00:00Z",
                "title": "Full reset (Weekly + 5 hr)",
                "description": "Ready to redeem"
              }
            ],
            "available_count": 1
        }"#,
        )
        .unwrap();
        assert_eq!(details.available_count, 1);
        assert_eq!(details.credits.len(), 1);
        assert_eq!(details.credits[0].id, "credit-1");
        assert_eq!(details.credits[0].status, "available");

        let result = parse_consume_result(r#"{"code":"reset","windows_reset":2}"#).unwrap();
        assert_eq!(result.outcome, ConsumeOutcome::Reset);
        assert_eq!(result.windows_reset, 2);

        let result = parse_consume_result(r#"{"code":"no_credit","windows_reset":0}"#).unwrap();
        assert_eq!(result.outcome, ConsumeOutcome::NoCredit);
    }

    #[test]
    fn parse_codex_auth_json_tokens() {
        let raw = r#"{
            "tokens": {
                "id_token": "x",
                "access_token": "atok",
                "refresh_token": "rtok",
                "account_id": "acct-1"
            }
        }"#;
        let auth = parse_codex_auth_json(raw).unwrap();
        assert_eq!(auth.access_token, "atok");
        assert_eq!(auth.account_id.as_deref(), Some("acct-1"));
        assert_eq!(auth.source, AuthSource::CodexAuthJson);
    }

    #[test]
    fn jwt_account_id_from_nested_claim() {
        // header.payload.sig — payload is {"https://api.openai.com/auth":{"chatgpt_account_id":"ws-9"}}
        let payload =
            "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoid3MtOSJ9fQ";
        let jwt = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(chatgpt_account_id_from_jwt(&jwt).as_deref(), Some("ws-9"));
    }

    #[test]
    fn format_summary_mentions_apply() {
        let details = ResetCreditsDetails {
            available_count: 0,
            credits: vec![],
        };
        let s = format_credits_summary(&details, AuthSource::CodexAuthJson);
        assert!(s.contains("Available banked resets: 0"));
        assert!(s.contains("/reset apply"));
    }
}
