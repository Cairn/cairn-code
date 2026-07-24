use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

static CONFIG_TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    /// Optional override for the skills root (else CAIRN_SKILLS_DIR / default path).
    pub skills_dir: Option<String>,
    /// MCP stdio servers (external tools).
    pub mcp: crate::mcp::McpConfig,
    /// When true, write request metadata (sanitized URL, header names, body
    /// size — never header values, body content, or secrets) to
    /// `~/.config/cairn-code/debug_request.json` for troubleshooting.
    /// Off by default (H-03); can also be enabled with `CAIRN_DEBUG_HTTP=1`.
    pub debug_log_requests: bool,
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
            skills_dir: None,
            mcp: crate::mcp::McpConfig::default(),
            debug_log_requests: false,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let Some(user_path) = config_path() else {
            let mut cfg = Config::default();
            cfg.system_prompt_file.clear();
            return Ok(cfg);
        };
        load_for_workspace(&user_path, &workspace)
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

pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));

    config_path_from_home(home.as_deref())
}

fn config_path_from_home(home: Option<&OsStr>) -> Option<PathBuf> {
    let home = Path::new(home?);
    home.is_absolute()
        .then(|| home.join(".config/cairn-code/config.json"))
}

fn config_path_or_err() -> Result<PathBuf, String> {
    config_path().ok_or_else(|| "user config directory is unavailable or not absolute".to_string())
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

fn load_for_workspace(user_path: &Path, workspace: &Path) -> Result<Config, String> {
    migrate_plaintext_keys_in_file(user_path)?;

    let user_content = read_optional_config(user_path)?;
    let mut cfg = match user_content.as_deref() {
        Some(content) => parse_config(content)
            .map_err(|error| format!("failed to parse config {}: {error}", user_path.display()))?,
        None => Config::default(),
    };
    let user_selected_prompt = user_content
        .as_deref()
        .and_then(|content| crate::json::parse(content).ok())
        .and_then(|value| value.as_object().cloned())
        .and_then(|obj| {
            obj.get("system_prompt_file")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });

    // The default CAIRN.md belongs to the repository, not the user. Do not load
    // it until the user has explicitly trusted this workspace.
    cfg.system_prompt_file = user_selected_prompt
        .as_deref()
        .and_then(|prompt| resolve_user_prompt(user_path, prompt))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();

    let trusted = user_content
        .as_deref()
        .is_some_and(|content| workspace_is_trusted(content, workspace));
    if !trusted {
        return Ok(cfg);
    }

    let project_path = workspace.join(".cairn/config.json");
    let Some(project_content) = read_optional_config(&project_path)? else {
        if user_selected_prompt.is_none() {
            if let Some(path) = resolve_workspace_prompt(workspace, "CAIRN.md") {
                cfg.system_prompt_file = path.to_string_lossy().into_owned();
            }
        }
        return Ok(cfg);
    };

    let project_value = crate::json::parse(&project_content)
        .map_err(|error| format!("failed to parse config {}: {error}", project_path.display()))?;
    let project = project_value.as_object().ok_or_else(|| {
        format!(
            "failed to parse config {}: config must be an object",
            project_path.display()
        )
    })?;

    apply_project_preferences(&mut cfg, project);

    if let Some(prompt) = project.get("system_prompt_file").and_then(|v| v.as_str()) {
        if let Some(path) = resolve_workspace_prompt(workspace, prompt) {
            cfg.system_prompt_file = path.to_string_lossy().into_owned();
        }
    } else if user_selected_prompt.is_none() {
        if let Some(path) = resolve_workspace_prompt(workspace, "CAIRN.md") {
            cfg.system_prompt_file = path.to_string_lossy().into_owned();
        }
    }

    Ok(cfg)
}

fn read_optional_config(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read config {}: {error}", path.display())),
    }
}

fn resolve_user_prompt(user_path: &Path, prompt: &str) -> Option<PathBuf> {
    let prompt = Path::new(prompt);
    let candidate = if prompt.is_absolute() {
        prompt.to_path_buf()
    } else {
        user_path.parent()?.join(prompt)
    };
    let candidate = fs::canonicalize(candidate).ok()?;
    candidate.is_file().then_some(candidate)
}

fn apply_project_preferences(cfg: &mut Config, project: &HashMap<String, crate::json::JsonValue>) {
    if let Some(v) = project.get("default_provider").and_then(|v| v.as_str()) {
        cfg.default_provider = v.to_string();
    }
    if let Some(v) = project.get("default_model").and_then(|v| v.as_str()) {
        cfg.default_model = v.to_string();
    }
    if let Some(v) = project.get("max_turns").and_then(|v| v.as_u64()) {
        cfg.max_turns = v as usize;
    }
    if let Some(v) = project.get("max_tokens").and_then(|v| v.as_u64()) {
        cfg.max_tokens = v as usize;
    }
    if let Some(v) = project.get("theme").and_then(|v| v.as_str()) {
        cfg.theme = v.to_string();
    }
    if let Some(v) = project.get("show_thinking").and_then(|v| v.as_bool()) {
        cfg.show_thinking = v;
    }
    if let Some(v) = project.get("show_suggestions").and_then(|v| v.as_bool()) {
        cfg.show_suggestions = v;
    }
}

fn workspace_is_trusted(user_content: &str, workspace: &Path) -> bool {
    let Ok(workspace) = fs::canonicalize(workspace) else {
        return false;
    };
    let Ok(value) = crate::json::parse(user_content) else {
        return false;
    };
    let Some(entries) = value
        .as_object()
        .and_then(|obj| obj.get("trusted_workspaces"))
        .and_then(|v| v.as_array())
    else {
        return false;
    };

    entries.iter().filter_map(|v| v.as_str()).any(|entry| {
        let path = Path::new(entry);
        path.is_absolute()
            && fs::canonicalize(path).is_ok_and(|trusted_path| trusted_path == workspace)
    })
}

fn resolve_workspace_prompt(workspace: &Path, prompt: &str) -> Option<PathBuf> {
    let workspace = fs::canonicalize(workspace).ok()?;
    let prompt = Path::new(prompt);
    let candidate = if prompt.is_absolute() {
        prompt.to_path_buf()
    } else {
        workspace.join(prompt)
    };
    let candidate = fs::canonicalize(candidate).ok()?;
    (candidate.is_file() && candidate.starts_with(&workspace)).then_some(candidate)
}

#[cfg(not(test))]
fn keyring_entry(provider: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("cairn-code", provider).map_err(|e| e.to_string())
}

#[cfg(test)]
fn keyring_entry(provider: &str) -> Result<keyring_core::Entry, String> {
    use std::sync::OnceLock;

    static MOCK_KEYRING: OnceLock<()> = OnceLock::new();
    MOCK_KEYRING.get_or_init(|| {
        let store = keyring_core::mock::Store::new().expect("create mock keyring store");
        keyring_core::set_default_store(store);
    });
    keyring_core::Entry::new("cairn-code", provider).map_err(|e| e.to_string())
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
fn migrate_plaintext_keys_in_file(path: &std::path::Path) -> Result<(), String> {
    use crate::json::JsonValue;
    if !path.exists() {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let Ok(val) = crate::json::parse(&content) else {
        return Ok(());
    };
    let Some(mut obj) = val.as_object().cloned() else {
        return Ok(());
    };
    let Some(keys) = obj.get("api_keys").and_then(|v| v.as_object()).cloned() else {
        return Ok(());
    };
    if keys.is_empty() {
        return Ok(());
    }

    // Only drop keys that were actually written to the keyring; keep any
    // whose migration failed so they aren't lost and can be retried later.
    let mut remaining = keys.clone();
    let mut migrated_any = false;
    for (provider, v) in &keys {
        if let Some(key) = v.as_str() {
            if !key.is_empty() && keyring_set(provider, key).is_ok() {
                remaining.remove(provider);
                migrated_any = true;
            }
        }
    }

    if migrated_any {
        if remaining.is_empty() {
            obj.remove("api_keys");
        } else {
            obj.insert("api_keys".into(), JsonValue::Object(remaining));
        }
        write_config_object(path, obj)?;
    }
    Ok(())
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
    let path = config_path_or_err()?;
    let mut obj = load_config_object(&path)?;

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

    write_config_object(&path, obj)
}

/// Persist a provider credential without changing the selected provider or model.
pub fn save_api_key(provider: &str, api_key: &str) -> Result<(), String> {
    keyring_set(provider, api_key)
}

/// Persist only the TUI theme preference, leaving other config keys intact.
pub fn save_theme(theme: &str) -> Result<(), String> {
    use crate::json::JsonValue;
    let path = config_path_or_err()?;
    let mut obj = load_config_object(&path)?;
    obj.insert("theme".into(), JsonValue::String(theme.into()));
    obj.remove("api_keys");
    write_config_object(&path, obj)
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
    let path = config_path_or_err()?;
    let mut obj = load_config_object(&path)?;
    obj.insert(key.into(), JsonValue::Bool(value));
    obj.remove("api_keys");
    write_config_object(&path, obj)
}

pub fn save_permissions(cfg: &Config) -> Result<(), String> {
    let path = config_path_or_err()?;
    save_permissions_to_path(&path, cfg)
}

fn save_permissions_to_path(path: &Path, cfg: &Config) -> Result<(), String> {
    use crate::json::JsonValue;
    let mut obj = load_config_object(path)?;

    let perms = JsonValue::Object(HashMap::from([
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
    obj.remove("api_keys");
    obj.remove("api_keys");

    write_config_object(path, obj)
}

fn load_config_object(path: &Path) -> Result<HashMap<String, crate::json::JsonValue>, String> {
    let Some(content) = read_optional_config(path)? else {
        return Ok(HashMap::new());
    };
    crate::json::parse(&content)
        .map_err(|error| format!("failed to parse config {}: {error}", path.display()))?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            format!(
                "failed to parse config {}: config must be an object",
                path.display()
            )
        })
}

fn write_config_object(
    path: &Path,
    obj: HashMap<String, crate::json::JsonValue>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create config directory {}: {error}",
            parent.display()
        )
    })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("invalid config path {}", path.display()))?;

    #[cfg(unix)]
    let parent_directory = fs::File::open(parent).map_err(|error| {
        format!(
            "failed to open config directory {}: {error}",
            parent.display()
        )
    })?;

    let output = crate::json::serialize(&crate::json::JsonValue::Object(obj));
    let (temp_path, mut temp_file) = loop {
        let sequence = CONFIG_TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(".{}-{sequence}.tmp", std::process::id()));
        let temp_path = parent.join(temp_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp_path) {
            Ok(file) => break (temp_path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create temporary config file {}: {error}",
                    temp_path.display()
                ));
            }
        }
    };

    let result = (|| -> Result<(), String> {
        temp_file.write_all(output.as_bytes()).map_err(|error| {
            format!(
                "failed to write temporary config file {}: {error}",
                temp_path.display()
            )
        })?;
        temp_file.sync_all().map_err(|error| {
            format!(
                "failed to sync temporary config file {}: {error}",
                temp_path.display()
            )
        })?;
        drop(temp_file);
        fs::rename(&temp_path, path)
            .map_err(|error| format!("failed to replace config {}: {error}", path.display()))?;
        #[cfg(unix)]
        parent_directory.sync_all().map_err(|error| {
            format!(
                "failed to sync config directory {}: {error}",
                parent.display()
            )
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
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

/// Load any keyring-stored API keys into the environment when the env var is unset.
/// OAuth tokens stay in the OAuth keyring so providers use the refresh-aware accessor.
pub fn hydrate_env_from_keyring() {
    for provider in ["anthropic", "openai", "openrouter", "opengateway", "xai"] {
        if env_key_for(provider).is_some() {
            continue;
        }
        if let Some(key) = config_get_api_key(provider) {
            apply_key_to_env(provider, &key);
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
    if let Some(v) = obj.get("skills_dir").and_then(|v| v.as_str()) {
        let t = v.trim();
        if !t.is_empty() {
            cfg.skills_dir = Some(t.to_string());
        }
    }
    if let Some(mcp_obj) = obj.get("mcp").and_then(|v| v.as_object()) {
        cfg.mcp = crate::mcp::McpConfig::from_json_obj(mcp_obj);
    } else if let Some(servers) = obj.get("mcpServers").and_then(|v| v.as_object()) {
        // Claude Code / zero-style top-level alias.
        let mut wrap = std::collections::HashMap::new();
        wrap.insert(
            "servers".into(),
            crate::json::JsonValue::Object(servers.clone()),
        );
        cfg.mcp = crate::mcp::McpConfig::from_json_obj(&wrap);
    }
    if let Some(v) = obj.get("debug_log_requests").and_then(|v| v.as_bool()) {
        cfg.debug_log_requests = v;
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEMP_DIR: AtomicUsize = AtomicUsize::new(0);

    fn temp_test_dir(name: &str) -> PathBuf {
        let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("cairn-config-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn user_config_with_trust(workspace: &Path) -> String {
        let workspace = workspace.to_string_lossy().replace('\\', "\\\\");
        format!(
            r#"{{
                "trusted_workspaces": ["{workspace}"],
                "permissions": {{
                    "auto_allow": ["file_read"],
                    "ask": ["shell"],
                    "deny": ["git"]
                }}
            }}"#
        )
    }

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
        assert!(
            !cfg.debug_log_requests,
            "request debug logging off by default (H-03)"
        );
    }

    #[test]
    fn test_parse_debug_log_requests() {
        let cfg = parse_config(r#"{"debug_log_requests": true}"#).unwrap();
        assert!(cfg.debug_log_requests);
        let cfg = parse_config(r#"{"debug_log_requests": false}"#).unwrap();
        assert!(!cfg.debug_log_requests);
        // Absent entirely -> stays off.
        let cfg = parse_config(r#"{}"#).unwrap();
        assert!(!cfg.debug_log_requests);
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
    fn untrusted_project_cannot_override_user_preferences() {
        let root = temp_test_dir("untrusted");
        let workspace = root.join("workspace");
        fs::create_dir_all(workspace.join(".cairn")).unwrap();
        let user_path = root.join("user-config.json");
        let user_prompt = root.join("user-prompt.md");
        fs::write(&user_prompt, "user-owned prompt").unwrap();
        fs::write(
            &user_path,
            r#"{
                "default_provider": "ollama",
                "default_model": "user-model",
                "max_turns": 7,
                "max_tokens": 2048,
                "theme": "user-theme",
                "show_thinking": false,
                "show_suggestions": false,
                "system_prompt_file": "user-prompt.md",
                "permissions": {
                    "auto_allow": ["file_read"],
                    "ask": ["shell"],
                    "deny": ["git"]
                }
            }"#,
        )
        .unwrap();
        fs::write(
            workspace.join(".cairn/config.json"),
            r#"{
                "default_provider": "openai",
                "default_model": "project-model",
                "max_turns": 999,
                "max_tokens": 1234,
                "theme": "project-theme",
                "show_thinking": true,
                "show_suggestions": true,
                "system_prompt_file": "../outside.md",
                "permissions": {
                    "auto_allow": ["shell", "file_write"],
                    "ask": [],
                    "deny": []
                },
                "api_keys": {"openai": "repository-secret"},
                "trusted_workspaces": ["."]
            }"#,
        )
        .unwrap();
        fs::write(root.join("outside.md"), "untrusted prompt").unwrap();
        fs::write(workspace.join("user-prompt.md"), "repository prompt").unwrap();

        let cfg = load_for_workspace(&user_path, &workspace).unwrap();

        assert_eq!(cfg.default_provider, "ollama");
        assert_eq!(cfg.default_model, "user-model");
        assert_eq!(cfg.max_turns, 7);
        assert_eq!(cfg.max_tokens, 2048);
        assert_eq!(cfg.theme, "user-theme");
        assert!(!cfg.show_thinking);
        assert!(!cfg.show_suggestions);
        assert_eq!(cfg.auto_allow, vec!["file_read"]);
        assert_eq!(cfg.ask, vec!["shell"]);
        assert_eq!(cfg.deny, vec!["git"]);
        assert!(cfg.api_keys.is_empty());
        assert_eq!(
            PathBuf::from(cfg.system_prompt_file),
            fs::canonicalize(user_prompt).unwrap()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trusted_project_can_select_prompt_only_inside_workspace() {
        let root = temp_test_dir("trusted-prompt");
        let workspace = root.join("workspace");
        fs::create_dir_all(workspace.join(".cairn")).unwrap();
        let user_path = root.join("user-config.json");
        fs::write(&user_path, user_config_with_trust(&workspace)).unwrap();
        fs::create_dir_all(workspace.join("prompts")).unwrap();
        let prompt = workspace.join("prompts/project.md");
        fs::write(&prompt, "trusted prompt").unwrap();
        fs::write(
            workspace.join(".cairn/config.json"),
            r#"{
                "default_model": "project-model",
                "max_tokens": 1234,
                "show_thinking": true,
                "system_prompt_file": "prompts/project.md",
                "permissions": {"auto_allow": ["shell"]}
            }"#,
        )
        .unwrap();

        let cfg = load_for_workspace(&user_path, &workspace).unwrap();

        assert_eq!(
            PathBuf::from(cfg.system_prompt_file),
            fs::canonicalize(prompt).unwrap()
        );
        assert_eq!(cfg.default_model, "project-model");
        assert_eq!(cfg.max_tokens, 1234);
        assert!(cfg.show_thinking);
        assert_eq!(cfg.auto_allow, vec!["file_read"]);

        fs::write(root.join("outside.md"), "outside prompt").unwrap();
        fs::write(
            workspace.join(".cairn/config.json"),
            r#"{"system_prompt_file": "../outside.md"}"#,
        )
        .unwrap();

        let cfg = load_for_workspace(&user_path, &workspace).unwrap();
        assert!(cfg.system_prompt_file.is_empty());

        let outside = fs::canonicalize(root.join("outside.md")).unwrap();
        let outside_json = outside.to_string_lossy().replace('\\', "\\\\");
        fs::write(
            workspace.join(".cairn/config.json"),
            format!(r#"{{"system_prompt_file": "{outside_json}"}}"#),
        )
        .unwrap();

        let cfg = load_for_workspace(&user_path, &workspace).unwrap();
        assert!(cfg.system_prompt_file.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_user_config_reports_path_and_parse_error() {
        let root = temp_test_dir("malformed-user");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let user_path = root.join("config.json");
        fs::write(&user_path, "{not json").unwrap();

        let error = load_for_workspace(&user_path, &workspace)
            .err()
            .expect("malformed user config should fail");

        assert!(error.contains(&user_path.display().to_string()), "{error}");
        assert!(error.contains("failed to parse config"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_trusted_project_config_reports_path_and_parse_error() {
        let root = temp_test_dir("malformed-project");
        let workspace = root.join("workspace");
        fs::create_dir_all(workspace.join(".cairn")).unwrap();
        let user_path = root.join("config.json");
        fs::write(&user_path, user_config_with_trust(&workspace)).unwrap();
        let project_path = workspace.join(".cairn/config.json");
        fs::write(&project_path, "[]").unwrap();

        let error = load_for_workspace(&user_path, &workspace)
            .err()
            .expect("malformed project config should fail");

        assert!(
            error.contains(&project_path.display().to_string()),
            "{error}"
        );
        assert!(error.contains("config must be an object"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_user_config_reports_path_and_read_error() {
        let root = temp_test_dir("unreadable-user");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let user_path = root.join("config.json");
        fs::create_dir_all(&user_path).unwrap();

        let error = load_for_workspace(&user_path, &workspace)
            .err()
            .expect("unreadable user config should fail");

        assert!(error.contains(&user_path.display().to_string()), "{error}");
        assert!(error.contains("failed to read config"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_trust_requires_exact_absolute_canonical_path() {
        let root = temp_test_dir("trust-path");
        let workspace = root.join("workspace");
        let child = workspace.join("child");
        fs::create_dir_all(&child).unwrap();

        let relative = r#"{"trusted_workspaces":["workspace"]}"#;
        assert!(!workspace_is_trusted(relative, &workspace));

        let parent_trusted = user_config_with_trust(&workspace);
        assert!(workspace_is_trusted(&parent_trusted, &workspace));
        assert!(!workspace_is_trusted(&parent_trusted, &child));

        fs::remove_dir_all(root).unwrap();
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
        let path = config_path().expect("test environment should have an absolute home directory");
        assert!(path.to_string_lossy().contains("cairn-code"));
    }

    #[test]
    fn config_path_requires_an_absolute_home() {
        assert!(config_path_from_home(None).is_none());
        assert!(config_path_from_home(Some(OsStr::new("."))).is_none());

        let absolute = std::env::temp_dir();
        assert_eq!(
            config_path_from_home(Some(absolute.as_os_str())),
            Some(absolute.join(".config/cairn-code/config.json"))
        );
    }

    #[test]
    fn saving_permissions_preserves_other_user_settings() {
        let root = temp_test_dir("save-permissions");
        let path = root.join("config.json");
        fs::write(
            &path,
            r#"{
                "default_provider": "openai",
                "default_model": "user-model",
                "max_turns": 50,
                "max_tokens": 4096,
                "system_prompt_file": "user-prompt.md",
                "theme": "user-theme",
                "show_thinking": false,
                "show_suggestions": true,
                "trusted_workspaces": ["C:/trusted"],
                "permissions": {"auto_allow": ["file_read"]}
            }"#,
        )
        .unwrap();

        let mut cfg = Config::default();
        cfg.default_provider = "project-provider".into();
        cfg.default_model = "project-model".into();
        cfg.max_turns = 999;
        cfg.max_tokens = 1234;
        cfg.system_prompt_file = "project-prompt.md".into();
        cfg.theme = "project-theme".into();
        cfg.show_thinking = true;
        cfg.show_suggestions = false;
        cfg.auto_allow = vec!["file_read".into(), "shell".into()];
        cfg.ask = vec!["file_write".into()];
        cfg.deny = vec!["git".into()];

        save_permissions_to_path(&path, &cfg).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let value = crate::json::parse(&content).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(
            obj.get("default_provider").and_then(|v| v.as_str()),
            Some("openai")
        );
        assert_eq!(
            obj.get("default_model").and_then(|v| v.as_str()),
            Some("user-model")
        );
        assert_eq!(obj.get("max_turns").and_then(|v| v.as_u64()), Some(50));
        assert_eq!(obj.get("max_tokens").and_then(|v| v.as_u64()), Some(4096));
        assert_eq!(
            obj.get("system_prompt_file").and_then(|v| v.as_str()),
            Some("user-prompt.md")
        );
        assert_eq!(
            obj.get("theme").and_then(|v| v.as_str()),
            Some("user-theme")
        );
        assert_eq!(
            obj.get("show_thinking").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            obj.get("show_suggestions").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(obj.get("trusted_workspaces").is_some());
        let permissions = obj.get("permissions").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            permissions
                .get("auto_allow")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            permissions
                .get("deny")
                .and_then(|v| v.as_array())
                .and_then(|values| values.first())
                .and_then(|v| v.as_str()),
            Some("git")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saving_rejects_non_object_config_without_overwriting_it() {
        let root = temp_test_dir("save-non-object");
        let path = root.join("config.json");
        fs::write(&path, "[]").unwrap();

        let error = save_permissions_to_path(&path, &Config::default()).unwrap_err();

        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("config must be an object"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[]");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_writes_replace_the_file_atomically_and_privately() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        fs::write(&path, r#"{"generation":"original"}"#).unwrap();

        #[cfg(unix)]
        let original_inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&path).unwrap().ino()
        };

        write_config_object(
            &path,
            HashMap::from([(
                "generation".into(),
                crate::json::JsonValue::String("replacement".into()),
            )]),
        )
        .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let parsed = crate::json::parse(&content).unwrap();
        assert_eq!(
            parsed
                .as_object()
                .and_then(|obj| obj.get("generation"))
                .and_then(|value| value.as_str()),
            Some("replacement")
        );
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = fs::metadata(&path).unwrap();
            assert_ne!(
                metadata.ino(),
                original_inode,
                "config update must publish a new file rather than truncate in place"
            );
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn failed_config_replace_removes_the_temporary_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("sentinel"), "keep").unwrap();

        let error = write_config_object(&path, HashMap::new()).unwrap_err();

        assert!(error.contains("failed to replace config"), "{error}");
        assert_eq!(fs::read_to_string(path.join("sentinel")).unwrap(), "keep");
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "failed replacement must not leave a temporary config file"
        );
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
        let _lock = crate::test_support::ENV_LOCK.lock().unwrap();
        let _env = crate::test_support::EnvGuard::capture("OPENAI_API_KEY");
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
        let provider = "cairn-code-test-provider-roundtrip";
        keyring_set(provider, "sk-roundtrip-test").unwrap();
        assert_eq!(keyring_get(provider), Some("sk-roundtrip-test".to_string()));
        assert_eq!(keyring_delete(provider), Ok(true));
        assert_eq!(keyring_get(provider), None);
        // Deleting again is a no-op, not an error.
        assert_eq!(keyring_delete(provider), Ok(false));
    }

    #[test]
    fn test_migrate_plaintext_keys_moves_to_keyring_and_strips_file() {
        let provider = "cairn-code-test-provider-migrate";
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.json");
        std::fs::write(&cfg_path, format!(
            r#"{{"default_provider":"openrouter","default_model":"m","api_keys":{{"{provider}":"sk-migrate-test"}}}}"#
        )).unwrap();

        migrate_plaintext_keys_in_file(&cfg_path).unwrap();

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
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_key_migration_propagates_config_write_failures() {
        use std::os::unix::fs::PermissionsExt;

        let provider = "cairn-code-test-provider-migrate-write-fail";
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let cfg_path = tmp.path().join("config.json");
        let original = format!(
            r#"{{"default_provider":"openrouter","api_keys":{{"{provider}":"sk-write-fail"}}}}"#
        );
        fs::write(&cfg_path, &original).unwrap();

        let original_permissions = fs::metadata(tmp.path()).unwrap().permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_mode(original_permissions.mode() & !0o222);
        fs::set_permissions(tmp.path(), read_only_permissions).unwrap();

        // Elevated test environments may bypass mode bits. Skip the failure
        // assertion there rather than depending on a particular user id.
        let probe = tmp.path().join("permission-probe");
        if fs::write(&probe, "probe").is_ok() {
            fs::remove_file(probe).unwrap();
            fs::set_permissions(tmp.path(), original_permissions).unwrap();
            return;
        }

        let result = load_for_workspace(&cfg_path, &workspace);
        fs::set_permissions(tmp.path(), original_permissions).unwrap();

        let error = result.err().expect("migration config write should fail");
        assert!(
            error.contains("failed to create temporary config file"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&cfg_path).unwrap(), original);
        assert_eq!(keyring_get(provider), Some("sk-write-fail".to_string()));
        let _ = keyring_delete(provider);
    }

    #[test]
    fn test_migrate_plaintext_keys_retains_entries_that_fail_to_migrate() {
        let good_provider = "cairn-code-test-provider-migrate-ok";
        let bad_provider = "cairn-code-test-provider-migrate-fail";
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.json");
        std::fs::write(
            &cfg_path,
            format!(
                r#"{{"default_provider":"openrouter","default_model":"m","api_keys":{{"{good_provider}":"sk-good","{bad_provider}":"sk-bad"}}}}"#
            ),
        )
        .unwrap();

        // Force the keyring write for `bad_provider` to fail so the
        // migration only partially succeeds.
        let entry = keyring_entry(bad_provider).unwrap();
        let mock: &keyring_core::mock::Cred = entry.as_any().downcast_ref().unwrap();
        mock.set_error(keyring_core::Error::NoStorageAccess(Box::new(
            std::io::Error::other("mock storage failure"),
        )));

        migrate_plaintext_keys_in_file(&cfg_path).unwrap();

        // The good provider's key made it into the keyring...
        assert_eq!(keyring_get(good_provider), Some("sk-good".to_string()));
        // ...but the bad provider's key was never migrated (the keyring
        // has nothing for it)...
        assert_eq!(keyring_get(bad_provider), None);

        // ...and crucially, it must still be present in the file so it
        // isn't lost forever; only the successfully migrated key is
        // stripped out.
        let content = std::fs::read_to_string(&cfg_path).unwrap();
        let parsed = crate::json::parse(&content).unwrap();
        let obj = parsed.as_object().unwrap();
        let remaining_keys = obj
            .get("api_keys")
            .and_then(|v| v.as_object())
            .expect("api_keys must be retained when a migration fails");
        assert_eq!(remaining_keys.len(), 1);
        assert_eq!(
            remaining_keys.get(bad_provider).and_then(|v| v.as_str()),
            Some("sk-bad")
        );
        assert!(remaining_keys.get(good_provider).is_none());

        let _ = keyring_delete(good_provider);
    }
}
