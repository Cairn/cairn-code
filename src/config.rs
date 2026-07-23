use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub struct Config {
    pub default_provider: String,
    pub default_model: String,
    pub max_turns: usize,
    pub max_tokens: usize,
    pub system_prompt_file: String,
    pub auto_allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
    pub api_keys: HashMap<String, String>,
    /// TUI color theme name (dark themes only). Default: "dark".
    pub theme: String,
    /// When true, stream and keep full model thinking in the transcript.
    /// When false (default, Claude Code-style), only a short "Thought for …" line is shown.
    pub show_thinking: bool,
    /// When true, show grayed ready-to-send idle prompts in the empty composer.
    /// Default off; enable with `/suggestions on`.
    pub show_suggestions: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_provider: "anthropic".to_string(),
            default_model: "claude-sonnet-4-20250514".to_string(),
            max_turns: 100,
            max_tokens: 8192,
            system_prompt_file: "CAIRN.md".to_string(),
            auto_allow: vec!["file_read".into(), "glob".into(), "grep".into()],
            ask: vec!["file_write".into(), "shell".into(), "file_edit".into()],
            deny: Vec::new(),
            api_keys: HashMap::new(),
            theme: "dark".to_string(),
            show_thinking: false,
            show_suggestions: false,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        migrate_plaintext_keys_in_file(&config_path());

        let paths = [dirs_config_path(), PathBuf::from(".cairn/config.json")];

        for path in &paths {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(path) {
                    if let Ok(cfg) = parse_config(&content) {
                        return cfg;
                    }
                }
            }
        }

        Config::default()
    }

    pub fn is_tool_denied(&self, name: &str) -> bool {
        self.deny.iter().any(|t| t == name)
    }

    pub fn _get_api_key(&self, provider: &str) -> Option<String> {
        // Check config file first, then env vars
        if let Some(key) = self.api_keys.get(provider) {
            return Some(key.clone());
        }
        env_key_for(provider)
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/cairn-code/config.json")
}

/// Name of the environment variable that holds the API key for the given provider.
pub fn env_var_name(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "opengateway" => Some("GITLAWB_OPENGATEWAY_API_KEY"),
        "xai" => Some("XAI_API_KEY"),
        _ => None,
    }
}

/// Look up an API key from the environment for the given provider.
/// Returns the env-var value (empty string if unset/filtered).
pub fn env_key_for(provider: &str) -> Option<String> {
    if let Some(name) = env_var_name(provider) {
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    // OpenGateway also accepts the shorter alias used by some setups.
    if provider == "opengateway" {
        return std::env::var("OPENGATEWAY_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
    }
    None
}

fn dirs_config_path() -> PathBuf {
    config_path()
}

fn keyring_entry(provider: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("cairn-code", provider).map_err(|e| e.to_string())
}

fn keyring_set(provider: &str, key: &str) -> Result<(), String> {
    keyring_entry(provider)?
        .set_password(key)
        .map_err(|e| e.to_string())
}

fn keyring_get(provider: &str) -> Option<String> {
    match keyring_entry(provider).ok()?.get_password() {
        Ok(pw) if !pw.is_empty() => Some(pw),
        _ => None,
    }
}

fn keyring_delete(provider: &str) -> Result<bool, String> {
    match keyring_entry(provider)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// One-time migration: API keys used to be stored as plaintext in the config
/// file's `api_keys` map. Move any that are still there into the OS keyring
/// and strip them from the file.
fn migrate_plaintext_keys_in_file(path: &std::path::Path) {
    use crate::json::JsonValue;
    if !path.exists() {
        return;
    }
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let Ok(val) = crate::json::parse(&content) else {
        return;
    };
    let Some(mut obj) = val.as_object().cloned() else {
        return;
    };
    let Some(keys) = obj.get("api_keys").and_then(|v| v.as_object()).cloned() else {
        return;
    };
    if keys.is_empty() {
        return;
    }

    let mut migrated_any = false;
    for (provider, v) in &keys {
        if let Some(key) = v.as_str() {
            if !key.is_empty() && keyring_set(provider, key).is_ok() {
                migrated_any = true;
            }
        }
    }

    if migrated_any {
        obj.remove("api_keys");
        let output = crate::json::serialize(&JsonValue::Object(obj));
        let _ = fs::write(path, &output);
    }
}

pub fn sessions_dir() -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config/cairn-code/sessions")
        .to_string_lossy()
        .to_string()
}

#[cfg_attr(test, allow(dead_code))]
pub fn save_config(provider: &str, model: &str, api_key: Option<&str>) -> Result<(), String> {
    use crate::json::JsonValue;
    let path = config_path();
    let mut obj: std::collections::HashMap<String, JsonValue> = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        crate::json::parse(&content)
            .map_err(|e| e.to_string())?
            .as_object()
            .cloned()
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    obj.insert(
        "default_provider".into(),
        JsonValue::String(provider.into()),
    );
    obj.insert("default_model".into(), JsonValue::String(model.into()));
    // API keys are never written to the config file; they live in the OS keyring.
    obj.remove("api_keys");

    if let Some(key) = api_key {
        keyring_set(provider, key)?;
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let output = crate::json::serialize(&JsonValue::Object(obj));
    std::fs::write(&path, &output).map_err(|e| e.to_string())
}

/// Persist a provider credential without changing the selected provider or model.
pub fn save_api_key(provider: &str, api_key: &str) -> Result<(), String> {
    keyring_set(provider, api_key)
}

/// Persist only the TUI theme preference, leaving other config keys intact.
pub fn save_theme(theme: &str) -> Result<(), String> {
    use crate::json::JsonValue;
    let path = config_path();
    let mut obj: std::collections::HashMap<String, JsonValue> = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        crate::json::parse(&content)
            .map_err(|e| e.to_string())?
            .as_object()
            .cloned()
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    obj.insert("theme".into(), JsonValue::String(theme.into()));
    obj.remove("api_keys");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let output = crate::json::serialize(&JsonValue::Object(obj));
    std::fs::write(&path, &output).map_err(|e| e.to_string())
}

/// Persist the show-thinking preference without rewriting other keys.
pub fn save_show_thinking(show: bool) -> Result<(), String> {
    save_bool_pref("show_thinking", show)
}

/// Persist the idle-suggestions preference without rewriting other keys.
pub fn save_show_suggestions(show: bool) -> Result<(), String> {
    save_bool_pref("show_suggestions", show)
}

fn save_bool_pref(key: &str, value: bool) -> Result<(), String> {
    use crate::json::JsonValue;
    let path = config_path();
    let mut obj: std::collections::HashMap<String, JsonValue> = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        crate::json::parse(&content)
            .map_err(|e| e.to_string())?
            .as_object()
            .cloned()
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    obj.insert(key.into(), JsonValue::Bool(value));
    obj.remove("api_keys");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let output = crate::json::serialize(&JsonValue::Object(obj));
    std::fs::write(&path, &output).map_err(|e| e.to_string())
}

pub fn save_full_config(cfg: &Config) -> Result<(), String> {
    use crate::json::JsonValue;
    let path = config_path();
    let mut obj: std::collections::HashMap<String, JsonValue> = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        crate::json::parse(&content)
            .map_err(|e| e.to_string())?
            .as_object()
            .cloned()
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    obj.insert(
        "default_provider".into(),
        JsonValue::String(cfg.default_provider.clone()),
    );
    obj.insert(
        "default_model".into(),
        JsonValue::String(cfg.default_model.clone()),
    );
    obj.insert("theme".into(), JsonValue::String(cfg.theme.clone()));
    obj.insert("show_thinking".into(), JsonValue::Bool(cfg.show_thinking));
    obj.insert(
        "show_suggestions".into(),
        JsonValue::Bool(cfg.show_suggestions),
    );

    let perms = JsonValue::Object(std::collections::HashMap::from([
        (
            "auto_allow".into(),
            JsonValue::Array(
                cfg.auto_allow
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        ),
        (
            "ask".into(),
            JsonValue::Array(
                cfg.ask
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        ),
        (
            "deny".into(),
            JsonValue::Array(
                cfg.deny
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        ),
    ]));
    obj.insert("permissions".into(), perms);
    // API keys are never written to the config file; they live in the OS keyring.
    obj.remove("api_keys");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let output = crate::json::serialize(&JsonValue::Object(obj));
    std::fs::write(&path, &output).map_err(|e| e.to_string())
}

pub fn config_get_api_key(provider: &str) -> Option<String> {
    keyring_get(provider)
}

pub fn config_has_api_key(provider: &str) -> bool {
    config_get_api_key(provider).is_some()
}

/// True when this provider needs a cloud API key (not local ollama).
pub fn provider_requires_api_key(provider: &str) -> bool {
    env_var_name(provider).is_some()
}

/// True when the provider needs credentials and none are available yet
/// (API key, env, or OAuth token).
pub fn needs_credential(provider: &str) -> bool {
    if provider == "ollama" {
        return false;
    }
    provider_requires_api_key(provider) && !has_usable_credential(provider)
}

/// True when a usable key or OAuth token is available from the keyring or environment.
pub fn has_usable_credential(provider: &str) -> bool {
    if config_has_api_key(provider) || env_key_for(provider).is_some() {
        return true;
    }
    // xAI (and future OAuth providers): valid device-code login counts as signed in.
    if crate::oauth::supports_oauth(provider) {
        return crate::oauth::access_token(provider).is_some();
    }
    false
}

/// Apply a key to the process environment for the given provider (so the
/// current agent process can use it without restart).
pub fn apply_key_to_env(provider: &str, key: &str) {
    if let Some(var) = env_var_name(provider) {
        std::env::set_var(var, key);
    }
    if provider == "opengateway" {
        std::env::set_var("OPENGATEWAY_API_KEY", key);
    }
}

/// Load any keyring-stored keys into the environment when the env var is unset.
pub fn hydrate_env_from_keyring() {
    for provider in ["anthropic", "openai", "openrouter", "opengateway", "xai"] {
        if env_key_for(provider).is_some() {
            continue;
        }
        if let Some(key) = config_get_api_key(provider) {
            apply_key_to_env(provider, &key);
            continue;
        }
        // OAuth access token for providers that support device login (xAI).
        if crate::oauth::supports_oauth(provider) {
            if let Some(tok) = crate::oauth::access_token(provider) {
                apply_key_to_env(provider, &tok);
            }
        }
    }
}

/// Mask a secret for on-screen input: bullets for all but the last `reveal`
/// characters (still fully masked when shorter than `reveal`).
pub fn mask_secret_display(value: &str, reveal: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= reveal {
        return "•".repeat(chars.len());
    }
    let mask_n = chars.len() - reveal;
    let mut out = "•".repeat(mask_n);
    out.extend(chars[mask_n..].iter().copied());
    out
}

/// Remove the saved API key for `provider` from the OS keyring.
/// Returns Ok(true) if a key was removed, Ok(false) if none was stored.
pub fn remove_api_key(provider: &str) -> Result<bool, String> {
    keyring_delete(provider)
}

fn parse_config(content: &str) -> Result<Config, String> {
    let val = crate::json::parse(content).map_err(|e| e.to_string())?;
    let obj = val.as_object().ok_or("config must be an object")?;

    let mut cfg = Config::default();

    if let Some(v) = obj.get("default_provider").and_then(|v| v.as_str()) {
        cfg.default_provider = v.to_string();
    }
    if let Some(v) = obj.get("default_model").and_then(|v| v.as_str()) {
        cfg.default_model = v.to_string();
    }
    if let Some(v) = obj.get("max_turns").and_then(|v| v.as_u64()) {
        cfg.max_turns = v as usize;
    }
    if let Some(v) = obj.get("max_tokens").and_then(|v| v.as_u64()) {
        cfg.max_tokens = v as usize;
    }
    if let Some(v) = obj.get("system_prompt_file").and_then(|v| v.as_str()) {
        cfg.system_prompt_file = v.to_string();
    }
    if let Some(v) = obj.get("theme").and_then(|v| v.as_str()) {
        cfg.theme = v.to_string();
    }
    if let Some(v) = obj.get("show_thinking").and_then(|v| v.as_bool()) {
        cfg.show_thinking = v;
    }
    if let Some(v) = obj.get("show_suggestions").and_then(|v| v.as_bool()) {
        cfg.show_suggestions = v;
    }

    if let Some(perms) = obj.get("permissions").and_then(|v| v.as_object()) {
        if let Some(arr) = perms.get("auto_allow").and_then(|v| v.as_array()) {
            cfg.auto_allow = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(arr) = perms.get("ask").and_then(|v| v.as_array()) {
            cfg.ask = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(arr) = perms.get("deny").and_then(|v| v.as_array()) {
            cfg.deny = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }

    if let Some(keys) = obj.get("api_keys").and_then(|v| v.as_object()) {
        for (k, v) in keys {
            if let Some(s) = v.as_str() {
                cfg.api_keys.insert(k.clone(), s.to_string());
            }
        }
    }

    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.default_provider, "anthropic");
        assert_eq!(cfg.auto_allow.len(), 3);
        assert!(cfg.ask.contains(&"shell".to_string()));
        assert!(cfg.deny.is_empty());
        assert!(
            !cfg.show_thinking,
            "thinking hidden by default (Claude Code-style)"
        );
        assert!(!cfg.show_suggestions, "idle suggestions off by default");
    }

    #[test]
    fn test_parse_show_thinking() {
        let cfg = parse_config(r#"{"show_thinking": true}"#).unwrap();
        assert!(cfg.show_thinking);
        let cfg = parse_config(r#"{"show_thinking": false}"#).unwrap();
        assert!(!cfg.show_thinking);
    }

    #[test]
    fn test_parse_show_suggestions() {
        let cfg = parse_config(r#"{"show_suggestions": true}"#).unwrap();
        assert!(cfg.show_suggestions);
        let cfg = parse_config(r#"{"show_suggestions": false}"#).unwrap();
        assert!(!cfg.show_suggestions);
    }

    #[test]
    fn test_is_tool_denied() {
        let mut cfg = Config::default();
        cfg.deny.push("shell".into());
        assert!(cfg.is_tool_denied("shell"));
        assert!(!cfg.is_tool_denied("file_read"));
    }

    #[test]
    fn test_parse_full_config() {
        let input = r#"{
            "default_provider": "openai",
            "default_model": "gpt-4o",
            "max_turns": 50,
            "max_tokens": 4096,
            "system_prompt_file": "CUSTOM.md",
            "permissions": {
                "auto_allow": ["file_read"],
                "ask": ["shell"],
                "deny": ["file_write"]
            },
            "api_keys": {
                "openai": "sk-test123"
            }
        }"#;
        let cfg = parse_config(input).unwrap();
        assert_eq!(cfg.default_provider, "openai");
        assert_eq!(cfg.default_model, "gpt-4o");
        assert_eq!(cfg.max_turns, 50);
        assert_eq!(cfg.max_tokens, 4096);
        assert_eq!(cfg.system_prompt_file, "CUSTOM.md");
        assert_eq!(cfg.auto_allow, vec!["file_read".to_string()]);
        assert_eq!(cfg.ask, vec!["shell".to_string()]);
        assert_eq!(cfg.deny, vec!["file_write".to_string()]);
        assert_eq!(cfg.api_keys.get("openai"), Some(&"sk-test123".to_string()));
    }

    #[test]
    fn test_parse_minimal_config() {
        let input = r#"{"default_provider":"test","default_model":"m"}"#;
        let cfg = parse_config(input).unwrap();
        assert_eq!(cfg.default_provider, "test");
        assert_eq!(cfg.default_model, "m");
        assert!(cfg.auto_allow.contains(&"file_read".to_string()));
    }

    #[test]
    fn test_roundtrip_config() {
        let mut cfg = Config::default();
        cfg.default_provider = "openai".into();
        cfg.default_model = "gpt-4o".into();
        cfg.auto_allow = vec!["file_read".into(), "glob".into()];
        cfg.ask = vec!["shell".into()];
        cfg.deny = vec!["file_write".into()];
        cfg.api_keys.insert("openai".into(), "sk-abc".into());

        let output = crate::json::serialize(&crate::json::JsonValue::Object(
            std::collections::HashMap::from([
                (
                    "default_provider".into(),
                    crate::json::JsonValue::String(cfg.default_provider.clone()),
                ),
                (
                    "default_model".into(),
                    crate::json::JsonValue::String(cfg.default_model.clone()),
                ),
                (
                    "permissions".into(),
                    crate::json::JsonValue::Object(std::collections::HashMap::from([
                        (
                            "auto_allow".into(),
                            crate::json::JsonValue::Array(
                                cfg.auto_allow
                                    .iter()
                                    .map(|s| crate::json::JsonValue::String(s.clone()))
                                    .collect(),
                            ),
                        ),
                        (
                            "ask".into(),
                            crate::json::JsonValue::Array(
                                cfg.ask
                                    .iter()
                                    .map(|s| crate::json::JsonValue::String(s.clone()))
                                    .collect(),
                            ),
                        ),
                        (
                            "deny".into(),
                            crate::json::JsonValue::Array(
                                cfg.deny
                                    .iter()
                                    .map(|s| crate::json::JsonValue::String(s.clone()))
                                    .collect(),
                            ),
                        ),
                    ])),
                ),
            ]),
        ));

        let parsed = parse_config(&output).unwrap();
        assert_eq!(parsed.default_provider, "openai");
        assert_eq!(parsed.auto_allow, vec!["file_read", "glob"]);
    }

    #[test]
    fn test_config_path() {
        let path = config_path();
        assert!(path.to_string_lossy().contains("cairn-code"));
    }

    #[test]
    fn test_env_key_for_known_providers() {
        // env_key_for must return None when the env var is unset, for each known provider.
        // We can't easily unset env vars, so we just check the function returns a String
        // (or None) for the well-known names without panicking.
        for p in ["anthropic", "openai", "openrouter", "opengateway"].iter() {
            let _ = env_key_for(p);
        }
        // Unknown provider => None
        assert!(env_key_for("nonsense_provider_xyz").is_none());
    }

    #[test]
    fn test_provider_requires_api_key() {
        assert!(provider_requires_api_key("anthropic"));
        assert!(provider_requires_api_key("openai"));
        assert!(provider_requires_api_key("openrouter"));
        assert!(provider_requires_api_key("opengateway"));
        assert!(provider_requires_api_key("xai"));
        assert!(!provider_requires_api_key("ollama"));
    }

    #[test]
    fn test_needs_credential_ollama_never() {
        assert!(!needs_credential("ollama"));
    }

    #[test]
    fn test_apply_key_to_env_and_mask() {
        apply_key_to_env("openai", "sk-test-apply-env-key");
        assert_eq!(
            std::env::var("OPENAI_API_KEY").ok().as_deref(),
            Some("sk-test-apply-env-key")
        );
        std::env::remove_var("OPENAI_API_KEY");
        assert_eq!(mask_secret_display("abcdefghij", 4), "••••••ghij");
    }

    #[test]
    fn test_mask_secret_display_shows_last_four() {
        assert_eq!(mask_secret_display("", 4), "");
        assert_eq!(mask_secret_display("abcd", 4), "••••");
        assert_eq!(mask_secret_display("abcdefghij", 4), "••••••ghij");
        assert_eq!(
            mask_secret_display("sk-ant-secretvalue99", 4),
            "••••••••••••••••ue99"
        );
    }

    #[test]
    fn test_keyring_set_get_delete_roundtrip() {
        // Distinctly-named test provider so this can never collide with a
        // real stored credential.
        let provider = "cairn-code-test-provider-roundtrip";
        // CI runners often have no secret-service / keychain backend.
        if keyring_set(provider, "sk-roundtrip-test").is_err() {
            eprintln!("skipping keyring roundtrip: no usable OS keyring backend");
            return;
        }
        assert_eq!(keyring_get(provider), Some("sk-roundtrip-test".to_string()));
        assert_eq!(keyring_delete(provider), Ok(true));
        assert_eq!(keyring_get(provider), None);
        // Deleting again is a no-op, not an error.
        assert_eq!(keyring_delete(provider), Ok(false));
    }

    #[test]
    fn test_migrate_plaintext_keys_moves_to_keyring_and_strips_file() {
        let provider = "cairn-code-test-provider-migrate";
        let tmp = std::env::temp_dir().join(format!("cairn-test-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg_path = tmp.join("config.json");
        std::fs::write(&cfg_path, format!(
            r#"{{"default_provider":"openrouter","default_model":"m","api_keys":{{"{provider}":"sk-migrate-test"}}}}"#
        )).unwrap();

        // Probe keyring first so headless CI without a backend does not fail.
        if keyring_set(provider, "probe").is_err() {
            eprintln!("skipping keyring migration test: no usable OS keyring backend");
            let _ = std::fs::remove_file(&cfg_path);
            return;
        }
        let _ = keyring_delete(provider);

        migrate_plaintext_keys_in_file(&cfg_path);

        // The key moved into the keyring...
        assert_eq!(keyring_get(provider), Some("sk-migrate-test".to_string()));
        // ...and the file no longer carries it, while other settings survive.
        let content = std::fs::read_to_string(&cfg_path).unwrap();
        let parsed = crate::json::parse(&content).unwrap();
        let obj = parsed.as_object().unwrap();
        assert!(
            obj.get("api_keys").is_none(),
            "api_keys must be stripped from the file after migration"
        );
        assert_eq!(
            obj.get("default_provider").and_then(|v| v.as_str()),
            Some("openrouter")
        );

        let _ = keyring_delete(provider);
        let _ = std::fs::remove_file(&cfg_path);
    }
}
