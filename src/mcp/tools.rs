//! Bridge MCP remote tools into cairn's `Tool` registry.

use std::sync::{Arc, Mutex};

use super::client::McpClient;
use super::{mcp_tool_name, McpConfig, McpServerConfig};
use crate::tools::registry::{Registry, Tool};

/// Owns live MCP clients for the process lifetime.
pub struct McpRuntime {
    pub clients: Vec<Arc<Mutex<McpClient>>>,
    pub warnings: Vec<String>,
    pub tool_names: Vec<String>,
}

impl McpRuntime {
    pub fn empty() -> Self {
        McpRuntime {
            clients: Vec::new(),
            warnings: Vec::new(),
            tool_names: Vec::new(),
        }
    }
}

struct McpTool {
    name: String,
    description: String,
    input_schema: String,
    remote_name: String,
    client: Arc<Mutex<McpClient>>,
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> String {
        self.input_schema.clone()
    }

    fn needs_permission(&self) -> bool {
        // External servers may have side effects; require confirmation by default.
        true
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let mut guard = self
            .client
            .lock()
            .map_err(|e| format!("MCP client lock: {e}"))?;
        guard.call_tool(&self.remote_name, input)
    }
}

/// Connect configured servers, list tools, register into `registry`.
/// Failed servers are skipped with a warning (session still starts).
pub fn register_mcp_tools(registry: &mut Registry, cfg: &McpConfig) -> McpRuntime {
    let mut runtime = McpRuntime::empty();
    let mut servers: Vec<(String, McpServerConfig)> = cfg
        .servers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    servers.sort_by(|a, b| a.0.cmp(&b.0));

    for (server_name, scfg) in servers {
        if scfg.disabled {
            continue;
        }
        match McpClient::connect(&server_name, &scfg) {
            Ok(mut client) => {
                let tools = match client.list_tools() {
                    Ok(t) => t,
                    Err(e) => {
                        runtime
                            .warnings
                            .push(format!("MCP server {server_name:?}: tools/list failed: {e}"));
                        continue;
                    }
                };
                let client = Arc::new(Mutex::new(client));
                runtime.clients.push(client.clone());
                for remote in tools {
                    let display = mcp_tool_name(&server_name, &remote.name);
                    if registry.get(&display).is_some() {
                        runtime.warnings.push(format!(
                            "MCP tool {display} conflicts with an existing tool; skipped"
                        ));
                        continue;
                    }
                    let desc = if remote.description.is_empty() {
                        format!("MCP tool {}/{}", server_name, remote.name)
                    } else {
                        format!("[MCP:{server_name}] {}", remote.description)
                    };
                    registry.register(Box::new(McpTool {
                        name: display.clone(),
                        description: desc,
                        input_schema: remote.input_schema,
                        remote_name: remote.name,
                        client: client.clone(),
                    }));
                    runtime.tool_names.push(display);
                }
            }
            Err(e) => {
                runtime
                    .warnings
                    .push(format!("MCP server {server_name:?} failed to start: {e}"));
            }
        }
    }
    runtime
}


