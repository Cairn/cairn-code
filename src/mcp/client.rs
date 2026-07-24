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
/// Upper bound on a single inbound frame (an NDJSON line or a Content-Length
/// body) so a peer cannot force an unbounded allocation.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// Grace period Drop waits for a server to exit after stdin EOF before it
/// force-kills and reaps the child.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

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
    /// Stderr-draining thread join handle.
    _stderr: Option<thread::JoinHandle<()>>,
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

        // Drain stderr on its own thread so a chatty server that fills its
        // stderr pipe cannot block before it answers on stdout.
        let stderr_drainer = stderr.map(|mut err| {
            thread::spawn(move || {
                let mut sink = Vec::new();
                let _ = err.read_to_end(&mut sink);
            })
        });

        let reader = thread::spawn(move || {
            read_loop(stdout, pending_r, &server_label);
        });

        let mut client = McpClient {
            child,
            stdin: Some(stdin),
            pending,
            next_id: AtomicU64::new(1),
            server_name: server_name.to_string(),
            _reader: Some(reader),
            _stderr: stderr_drainer,
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
        // On any failure path, drop the pending entry so a timed-out or
        // never-sent request cannot leak in the map forever.
        if let Err(e) = self.write_message(&msg) {
            self.forget_pending(id);
            return Err(e);
        }
        let result = wait_response(&rx, timeout, &self.server_name, method);
        if result.is_err() {
            self.forget_pending(id);
        }
        result
    }

    fn forget_pending(&self, id: u64) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(&id);
        }
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
        // Bounded graceful shutdown: poll for a clean exit for a short grace
        // period, then force-kill so a non-cooperative server cannot hang us.
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                _ => break,
            }
        }
        let _ = self.child.kill();
        // Reap the child so we never leave a zombie behind.
        let _ = self.child.wait();
    }
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
    loop {
        // Prefer NDJSON lines; also accept Content-Length framing. Cap every
        // read so a peer cannot force an unbounded line allocation.
        let mut header = String::new();
        match (&mut reader)
            .take(MAX_FRAME_BYTES as u64)
            .read_line(&mut header)
        {
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
            // Refuse a peer-selected allocation larger than our frame cap.
            if len > MAX_FRAME_BYTES {
                break;
            }
            // consume remaining headers until blank line
            loop {
                let mut h = String::new();
                if (&mut reader)
                    .take(MAX_FRAME_BYTES as u64)
                    .read_line(&mut h)
                    .unwrap_or(0)
                    == 0
                {
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
    use std::io::Cursor;

    #[test]
    fn flatten_text_blocks() {
        let raw = r#"{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}"#;
        let v = json::parse(raw).unwrap();
        assert_eq!(flatten_content(v.get("content")), "hello\nworld");
    }

    #[test]
    fn read_loop_ndjson_framing() {
        let payload = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n";
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        pending.lock().unwrap().insert(1, Pending { tx });
        read_loop(Cursor::new(payload), pending, "test");
        let res = rx.recv().unwrap();
        assert!(res.is_ok());
    }

    #[test]
    fn read_loop_content_length_framing() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"status\":\"ok\"}}";
        let payload = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        pending.lock().unwrap().insert(2, Pending { tx });
        read_loop(Cursor::new(payload), pending, "test");
        let res = rx.recv().unwrap();
        assert!(res.is_ok());
    }

    #[test]
    fn read_loop_rejects_oversized_content_length() {
        // A peer-selected Content-Length above the cap must not be allocated;
        // the loop breaks and any pending request is failed.
        let payload = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        pending.lock().unwrap().insert(9, Pending { tx });
        read_loop(Cursor::new(payload), pending, "test");
        let res = rx.recv().unwrap();
        assert!(res.is_err());
    }

    #[test]
    fn wait_response_timeout_and_disconnect() {
        let (tx, rx) = mpsc::channel();
        assert!(wait_response(&rx, Duration::from_millis(50), "test", "test").is_err());
        drop(tx);
        assert!(wait_response(&rx, Duration::from_secs(1), "test", "test").is_err());
    }
}
