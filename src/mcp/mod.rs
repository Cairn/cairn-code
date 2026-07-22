//! Minimal MCP client (stdio JSON-RPC) for external tools.
//!
//! v1: stdio servers only, tools/list + tools/call, failed servers skipped.

mod client;
mod tools;

pub use tools::{register_mcp_tools, McpRuntime};

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub disabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct McpConfig {
    pub servers: HashMap<String, McpServerConfig>,
}

impl McpConfig {
    pub fn from_json_obj(obj: &HashMap<String, crate::json::JsonValue>) -> Self {
        let mut cfg = McpConfig::default();
        // Accept either { "servers": { ... } } or a bare map of server name → config
        // (also accept nested under key "mcp" already unwrapped by caller).
        let servers_val = obj
            .get("servers")
            .cloned()
            .unwrap_or_else(|| crate::json::JsonValue::Object(obj.clone()));
        let Some(servers) = servers_val.as_object() else {
            return cfg;
        };
        for (name, v) in servers {
            // Skip non-object entries (e.g. if bare map mixed with other keys)
            let Some(sobj) = v.as_object() else {
                continue;
            };
            let command = sobj
                .get("command")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if command.is_empty() {
                continue;
            }
            let args = sobj
                .get("args")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let mut env = HashMap::new();
            if let Some(eobj) = sobj.get("env").and_then(|x| x.as_object()) {
                for (k, v) in eobj {
                    if let Some(s) = v.as_str() {
                        env.insert(k.clone(), s.to_string());
                    }
                }
            }
            let disabled = sobj
                .get("disabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            // type: only stdio supported; ignore http/sse for v1
            if let Some(ty) = sobj.get("type").and_then(|x| x.as_str()) {
                let ty = ty.to_ascii_lowercase();
                if ty != "stdio" && ty != "std" && !ty.is_empty() {
                    continue;
                }
            }
            cfg.servers.insert(
                name.clone(),
                McpServerConfig {
                    command,
                    args,
                    env,
                    disabled,
                },
            );
        }
        cfg
    }
}

/// Sanitize a name fragment for tool ids: `[a-zA-Z0-9_]` only.
pub fn sanitize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if c == '-' || c == '.' || c == ' ' {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("tool");
    }
    out
}

pub fn mcp_tool_name(server: &str, tool: &str) -> String {
    format!("mcp_{}_{}", sanitize_name(server), sanitize_name(tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn parse_servers_map() {
        let raw = r#"{
            "servers": {
                "docs": {
                    "command": "npx",
                    "args": ["-y", "foo"],
                    "env": {"A": "1"}
                },
                "off": { "command": "x", "disabled": true }
            }
        }"#;
        let v = json::parse(raw).unwrap();
        let cfg = McpConfig::from_json_obj(v.as_object().unwrap());
        assert_eq!(cfg.servers.len(), 2);
        assert_eq!(cfg.servers["docs"].command, "npx");
        assert_eq!(cfg.servers["docs"].args, vec!["-y", "foo"]);
        assert_eq!(cfg.servers["docs"].env.get("A").map(|s| s.as_str()), Some("1"));
        assert!(cfg.servers["off"].disabled);
    }

    #[test]
    fn tool_name_sanitized() {
        assert_eq!(mcp_tool_name("my-server", "list.files"), "mcp_my_server_list_files");
    }
}
