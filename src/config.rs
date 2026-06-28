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
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let paths = [
            dirs_config_path(),
            PathBuf::from(".cairn/config.json"),
        ];

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
        match provider {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            _ => None,
        }
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/cairn-code/config.json")
}

fn dirs_config_path() -> PathBuf {
    config_path()
}

pub fn save_config(provider: &str, model: &str, api_key: Option<&str>) -> Result<(), String> {
    use crate::json::JsonValue;
    let path = config_path();
    let mut obj: std::collections::HashMap<String, JsonValue> = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        crate::json::parse(&content).map_err(|e| e.to_string())?.as_object().cloned().unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    obj.insert("default_provider".into(), JsonValue::String(provider.into()));
    obj.insert("default_model".into(), JsonValue::String(model.into()));

    if let Some(key) = api_key {
        let mut keys = obj.get("api_keys").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        keys.insert("openrouter".into(), JsonValue::String(key.into()));
        obj.insert("api_keys".into(), JsonValue::Object(keys));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let output = crate::json::serialize(&JsonValue::Object(obj));
    std::fs::write(&path, &output).map_err(|e| e.to_string())
}

pub fn config_has_api_key(provider: &str) -> bool {
    let path = config_path();
    if !path.exists() { return false; }
    let content = std::fs::read_to_string(&path).ok()?;
    let val = crate::json::parse(&content).ok()?;
    let obj = val.as_object()?;
    let keys = obj.get("api_keys")?.as_object()?;
    keys.get(provider).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).is_some()
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

    if let Some(perms) = obj.get("permissions").and_then(|v| v.as_object()) {
        if let Some(arr) = perms.get("auto_allow").and_then(|v| v.as_array()) {
            cfg.auto_allow = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if let Some(arr) = perms.get("ask").and_then(|v| v.as_array()) {
            cfg.ask = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if let Some(arr) = perms.get("deny").and_then(|v| v.as_array()) {
            cfg.deny = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
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
