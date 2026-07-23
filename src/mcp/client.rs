//! Stdio MCP client: initialize, tools/list, tools/call.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::McpServerConfig;
use crate::json::{self, JsonValue};

const PROTOCOL_VERSION: &str = "2024-11-05";
const INIT_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    pub input_schema: String,
}

struct Pending {
    tx: Sender<Result<JsonValue, String>>,
}

pub struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, Pending>>>,
    next_id: AtomicU64,
    server_name: String,
    /// Reader thread join handle.
    _reader: Option<thread::JoinHandle<()>>,
}

impl McpClient {
    pub fn connect(server_name: &str, cfg: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        // Avoid flashing a console window on Windows for GUI-less MCP servers.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn MCP server {server_name:?} ({}): {e}", cfg.command))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("MCP {server_name}: no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("MCP {server_name}: no stdout"))?;
        let stderr = child.stderr.take();

        let pending: Arc<Mutex<HashMap<u64, Pending>>> = Arc::new(Mutex::new(HashMap::new()));
        let pending_r = pending.clone();
        let server_label = server_name.to_string();

        let reader = thread::spawn(move || {
            read_loop(stdout, pending_r, &server_label);
            // Drain stderr into nowhere (or log later); keep process from blocking.
            if let Some(mut err) = stderr {
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
            }
        });

        let mut client = McpClient {
            child,
            stdin: Some(stdin),
            pending,
            next_id: AtomicU64::new(1),
            server_name: server_name.to_string(),
            _reader: Some(reader),
        };

        client.handshake()?;
        Ok(client)
    }

    fn handshake(&mut self) -> Result<(), String> {
        let params = JsonValue::Object(HashMap::from([
            (
                "protocolVersion".into(),
                JsonValue::String(PROTOCOL_VERSION.into()),
            ),
            ("capabilities".into(), JsonValue::Object(HashMap::new())),
            (
                "clientInfo".into(),
                JsonValue::Object(HashMap::from([
                    ("name".into(), JsonValue::String("cairn-code".into())),
                    (
                        "version".into(),
                        JsonValue::String(env!("CARGO_PKG_VERSION").into()),
                    ),
                ])),
            ),
        ]));
        let _ = self.request("initialize", params, INIT_TIMEOUT)?;
        self.notify(
            "notifications/initialized",
            JsonValue::Object(HashMap::new()),
        )?;
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<RemoteTool>, String> {
        let result = self.request(
            "tools/list",
            JsonValue::Object(HashMap::new()),
            INIT_TIMEOUT,
        )?;
        let tools_val = result
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("MCP {}: tools/list missing tools array", self.server_name))?;
        let mut out = Vec::new();
        for t in tools_val {
            let Some(obj) = t.as_object() else { continue };
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let description = obj
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let schema = obj
                .get("inputSchema")
                .map(|v| v.to_string())
                .unwrap_or_else(|| r#"{"type":"object","properties":{}}"#.into());
            out.push(RemoteTool {
                name,
                description,
                input_schema: schema,
            });
        }
        Ok(out)
    }

    pub fn call_tool(&mut self, name: &str, arguments_json: &str) -> Result<String, String> {
        let args_val = if arguments_json.trim().is_empty() {
            JsonValue::Object(HashMap::new())
        } else {
            json::parse(arguments_json).map_err(|e| format!("invalid tool arguments JSON: {e}"))?
        };
        let params = JsonValue::Object(HashMap::from([
            ("name".into(), JsonValue::String(name.into())),
            ("arguments".into(), args_val),
        ]));
        let result = self.request("tools/call", params, CALL_TIMEOUT)?;
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let text = flatten_content(result.get("content"));
        if is_error {
            Err(if text.is_empty() {
                format!("MCP tool {name} returned isError")
            } else {
                text
            })
        } else {
            Ok(if text.is_empty() {
                "(empty MCP result)".into()
            } else {
                text
            })
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn request(
        &mut self,
        method: &str,
        params: JsonValue,
        timeout: Duration,
    ) -> Result<JsonValue, String> {
        let id = self.next_id();
        let (tx, rx) = mpsc::channel();
        {
            let mut map = self.pending.lock().map_err(|e| e.to_string())?;
            map.insert(id, Pending { tx });
        }
        let msg = JsonValue::Object(HashMap::from([
            ("jsonrpc".into(), JsonValue::String("2.0".into())),
            ("id".into(), JsonValue::Number(id as f64)),
            ("method".into(), JsonValue::String(method.into())),
            ("params".into(), params),
        ]));
        if let Err(e) = self.write_message(&msg) {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&id);
            }
            return Err(e);
        }
        let response = wait_response(&rx, timeout, &self.server_name, method);
        if response.is_err() {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&id);
            }
        }
        response
    }

    fn notify(&mut self, method: &str, params: JsonValue) -> Result<(), String> {
        let msg = JsonValue::Object(HashMap::from([
            ("jsonrpc".into(), JsonValue::String("2.0".into())),
            ("method".into(), JsonValue::String(method.into())),
            ("params".into(), params),
        ]));
        self.write_message(&msg)
    }

    fn write_message(&mut self, msg: &JsonValue) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| format!("MCP {}: stdin closed", self.server_name))?;
        let line = format!("{msg}\n");
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("MCP {}: write: {e}", self.server_name))?;
        stdin
            .flush()
            .map_err(|e| format!("MCP {}: flush: {e}", self.server_name))?;
        Ok(())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Close stdin so the server sees EOF and can exit cleanly.
        self.stdin.take();
        if wait_for_exit(&mut self.child, SHUTDOWN_GRACE) {
            return;
        }

        // Never block indefinitely on a server that ignores EOF.
        let _ = self.child.kill();
        let _ = wait_for_exit(&mut self.child, SHUTDOWN_GRACE);
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
    false
}

fn wait_response(
    rx: &Receiver<Result<JsonValue, String>>,
    timeout: Duration,
    server: &str,
    method: &str,
) -> Result<JsonValue, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("MCP {server}: {method} timed out"));
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(e)) => return Err(e),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!("MCP {server}: reader disconnected during {method}"));
            }
        }
    }
}

fn read_loop<R: Read>(stdout: R, pending: Arc<Mutex<HashMap<u64, Pending>>>, server: &str) {
    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    loop {
        buf.clear();
        // Prefer NDJSON lines; also accept Content-Length framing.
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let header_trim = header.trim();
        if header_trim.is_empty() {
            continue;
        }
        let payload = if let Some(rest) = header_trim
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
        {
            let len: usize = rest.trim().parse().unwrap_or(0);
            // consume remaining headers until blank line
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 {
                    break;
                }
                if h.trim().is_empty() {
                    break;
                }
            }
            let mut body = vec![0u8; len];
            if reader.read_exact(&mut body).is_err() {
                break;
            }
            String::from_utf8_lossy(&body).into_owned()
        } else {
            header_trim.to_string()
        };

        let Ok(val) = json::parse(&payload) else {
            continue;
        };
        // Response with id
        if let Some(id_v) = val.get("id") {
            let id = match id_v {
                JsonValue::Number(n) => *n as u64,
                JsonValue::String(s) => s.parse().unwrap_or(0),
                _ => 0,
            };
            let result = if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("MCP error");
                Err(format!("MCP {server}: {msg}"))
            } else if let Some(r) = val.get("result") {
                Ok(r.clone())
            } else {
                Err(format!("MCP {server}: response missing result"))
            };
            if let Ok(mut map) = pending.lock() {
                if let Some(p) = map.remove(&id) {
                    let _ = p.tx.send(result);
                }
            }
        }
        // notifications ignored
    }
    // Fail all pending
    if let Ok(mut map) = pending.lock() {
        for (_, p) in map.drain() {
            let _ = p.tx.send(Err(format!("MCP {server}: connection closed")));
        }
    }
}

fn flatten_content(content: Option<&JsonValue>) -> String {
    let Some(c) = content else {
        return String::new();
    };
    if let Some(s) = c.as_str() {
        return s.to_string();
    }
    let Some(arr) = c.as_array() else {
        return c.to_string();
    };
    let mut parts = Vec::new();
    for item in arr {
        if let Some(obj) = item.as_object() {
            let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("text");
            if ty == "text" {
                if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                    parts.push(t.to_string());
                }
            } else if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                parts.push(format!("[{ty}] {t}"));
            }
        } else if let Some(s) = item.as_str() {
            parts.push(s.to_string());
        }
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_response(
        id: u64,
    ) -> (
        Arc<Mutex<HashMap<u64, Pending>>>,
        Receiver<Result<JsonValue, String>>,
    ) {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        pending.lock().unwrap().insert(id, Pending { tx });
        (pending, rx)
    }

    #[test]
    fn flatten_text_blocks() {
        let raw = r#"{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}"#;
        let v = json::parse(raw).unwrap();
        assert_eq!(flatten_content(v.get("content")), "hello\nworld");
    }

    #[test]
    fn read_loop_correlates_ndjson_response() {
        let (pending, rx) = pending_response(7);
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n";

        read_loop(&input[..], pending.clone(), "test");

        let result = wait_response(&rx, Duration::from_secs(1), "test", "tools/list").unwrap();
        assert_eq!(result.get("ok").and_then(JsonValue::as_bool), Some(true));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn read_loop_correlates_content_length_response() {
        let (pending, rx) = pending_response(9);
        let body = r#"{"jsonrpc":"2.0","id":"9","result":"done"}"#;
        let input = format!("Content-Length: {}\r\n\r\n{body}", body.len());

        read_loop(input.as_bytes(), pending, "test");

        let result = wait_response(&rx, Duration::from_secs(1), "test", "tools/call").unwrap();
        assert_eq!(result.as_str(), Some("done"));
    }

    #[test]
    fn wait_response_reports_timeout_and_disconnect() {
        let (tx, rx) = mpsc::channel();
        let timeout = wait_response(&rx, Duration::from_millis(1), "slow", "initialize")
            .unwrap_err();
        assert!(timeout.contains("timed out"), "{timeout}");

        drop(tx);
        let disconnected =
            wait_response(&rx, Duration::from_secs(1), "gone", "tools/list").unwrap_err();
        assert!(disconnected.contains("reader disconnected"), "{disconnected}");
    }
}
