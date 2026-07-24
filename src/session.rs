#![allow(dead_code)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::llm::Message;

/// Shared agent transcript for autosave / resume (full tool history, not just TUI text).
#[derive(Default, Clone)]
pub struct LiveSnapshot {
    pub messages: Vec<Message>,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

pub type LiveMirror = Arc<Mutex<LiveSnapshot>>;

pub fn new_live_mirror() -> LiveMirror {
    Arc::new(Mutex::new(LiveSnapshot::default()))
}

pub struct Session {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub messages: Vec<Message>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

static ID_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Clock resolution on some CI runners is coarser than back-to-back calls,
    // so fold in a process-local counter to guarantee uniqueness even when
    // two calls land on the same nanosecond.
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}{:04x}", nanos, seq & 0xffff)
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.len() < 8
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid session id".into());
    }
    Ok(())
}

pub fn save(sessions_dir: &str, session: &Session) -> Result<(), String> {
    validate_id(&session.id)?;
    let dir = PathBuf::from(sessions_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(&dir).map_err(|e| format!("mkdir: {e}"))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("set session directory permissions: {e}"))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;

    let path = dir.join(&session.id);
    let temp_path = dir.join(format!(".{}.{}.tmp", session.id, new_id()));
    let json = session_to_json(session)?;
    let write_result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp_path)
            .map_err(|e| format!("create temporary session: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("set temporary session permissions: {e}"))?;
        }
        file.write_all(json.as_bytes())
            .map_err(|e| format!("write temporary session: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync temporary session: {e}"))?;
        drop(file);
        fs::rename(&temp_path, &path).map_err(|e| format!("replace session: {e}"))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result?;
    Ok(())
}

pub fn load(sessions_dir: &str, id: &str) -> Result<Session, String> {
    validate_id(id)?;
    let path = PathBuf::from(sessions_dir).join(id);
    let content = fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
    let mut session = session_from_json(&content)?;
    session.id = id.to_string();
    Ok(session)
}

/// Deletes a saved session file. `id` must be an exact session id (no path
/// separators). Use [`resolve_id`] first when the user only typed a prefix.
pub fn delete(sessions_dir: &str, id: &str) -> Result<(), String> {
    validate_id(id)?;
    let path = PathBuf::from(sessions_dir).join(id);
    if !path.is_file() {
        return Err(format!("session not found: {id}"));
    }
    fs::remove_file(&path).map_err(|e| format!("delete: {e}"))
}

/// Resolve a full session id from an exact id or unique prefix (as shown in
/// `/sessions`, which displays the first 8 hex characters).
pub fn resolve_id(sessions_dir: &str, query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("session id required".into());
    }
    if !q
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid session id".into());
    }
    let sessions = list(sessions_dir)?;
    let matches: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| s.id == q || s.id.starts_with(q))
        .collect();
    match matches.as_slice() {
        [] => Err(format!("session not found: {q}")),
        [one] => Ok(one.id.clone()),
        many => {
            let ids: Vec<&str> = many.iter().map(|s| &s.id[..s.id.len().min(8)]).collect();
            Err(format!(
                "ambiguous session id '{q}' matches {}: {}",
                many.len(),
                ids.join(", ")
            ))
        }
    }
}

pub fn list(sessions_dir: &str) -> Result<Vec<SessionSummary>, String> {
    let dir = PathBuf::from(sessions_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("readdir: {e}"))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let path = entry.path();
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !path.is_file() || validate_id(&id).is_err() {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(s) = session_from_json(&content) {
                sessions.push(SessionSummary {
                    id,
                    model: s.model,
                    msg_count: s.messages.len(),
                    updated_at: s.updated_at,
                    summary: s
                        .messages
                        .first()
                        .and_then(|m| match &m.content {
                            crate::llm::Content::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                });
            }
        }
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

pub struct SessionSummary {
    pub id: String,
    pub model: String,
    pub msg_count: usize,
    pub updated_at: u64,
    pub summary: String,
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
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn session_to_json(s: &Session) -> Result<String, String> {
    let mut json = String::new();
    json.push_str(&format!(
        "{{\"id\":\"{}\",\"model\":\"{}\",\"provider\":\"{}\",",
        json_escape(&s.id),
        json_escape(&s.model),
        json_escape(&s.provider)
    ));
    json.push_str(&format!(
        "\"tokens_in\":{},\"tokens_out\":{},",
        s.tokens_in, s.tokens_out
    ));
    json.push_str(&format!(
        "\"created_at\":{},\"updated_at\":{},",
        s.created_at, s.updated_at
    ));
    json.push_str("\"messages\":[");
    for (i, msg) in s.messages.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&message_to_json(msg)?);
    }
    json.push_str("]}");
    Ok(json)
}

fn json_value_to_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("serializing a JSON value cannot fail")
}

fn message_to_json(msg: &Message) -> Result<String, String> {
    let content = match &msg.content {
        crate::llm::Content::Text(t) => format!("\"{}\"", json_escape(t)),
        crate::llm::Content::ToolUse(tu) => {
            let input = if tu.input.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&tu.input)
                    .map_err(|e| format!("invalid tool input for '{}': {e}", tu.name))?
            };
            format!(
                "{{\"type\":\"tool_use\",\"id\":\"{}\",\"name\":\"{}\",\"input\":{}}}",
                json_escape(&tu.id),
                json_escape(&tu.name),
                json_value_to_json(&input)
            )
        }
        crate::llm::Content::ToolResult(tr) => {
            format!(
                "{{\"type\":\"tool_result\",\"tool_use_id\":\"{}\",\"content\":\"{}\"}}",
                json_escape(&tr.tool_use_id),
                json_escape(&tr.content)
            )
        }
        crate::llm::Content::Thinking(t) => {
            format!(
                "{{\"type\":\"thinking\",\"thinking\":\"{}\"}}",
                json_escape(t)
            )
        }
    };
    Ok(format!(
        "{{\"role\":\"{}\",\"content\":{}}}",
        json_escape(&msg.role),
        content
    ))
}

fn session_from_json(json_str: &str) -> Result<Session, String> {
    let val: serde_json::Value = serde_json::from_str(json_str).map_err(|e| e.to_string())?;
    let obj = val.as_object().ok_or("not an object")?;

    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("no id")?
        .to_string();
    validate_id(&id)?;
    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let provider = obj
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tokens_in = obj.get("tokens_in").and_then(|v| v.as_u64()).unwrap_or(0);
    let tokens_out = obj.get("tokens_out").and_then(|v| v.as_u64()).unwrap_or(0);
    let created_at = obj.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0);
    let updated_at = obj.get("updated_at").and_then(|v| v.as_u64()).unwrap_or(0);

    let messages = obj
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or("messages must be an array")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            message_from_json(value).map_err(|e| format!("invalid message {index}: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Session {
        id,
        model,
        provider,
        messages,
        tokens_in,
        tokens_out,
        created_at,
        updated_at,
    })
}

fn message_from_json(val: &serde_json::Value) -> Result<Message, String> {
    let obj = val.as_object().ok_or("not an object")?;
    let role = obj
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or("no role")?
        .to_string();
    let content_val = obj.get("content").ok_or("no content")?;

    let content = if let Some(s) = content_val.as_str() {
        crate::llm::Content::Text(s.to_string())
    } else if let Some(o) = content_val.as_object() {
        let type_str = o
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or("no content type")?;
        match type_str {
            "tool_use" => {
                let name = o
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("no tool name")?
                    .to_string();
                let id = o
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("no tool id")?
                    .to_string();
                let input = o
                    .get("input")
                    .map(json_value_to_json)
                    .ok_or("no tool input")?;
                crate::llm::Content::ToolUse(crate::llm::ToolUse { name, input, id })
            }
            "tool_result" => {
                let tool_use_id = o
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .ok_or("no tool result id")?
                    .to_string();
                let content = o
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or("no tool result content")?
                    .to_string();
                crate::llm::Content::ToolResult(crate::llm::ToolResult {
                    tool_use_id,
                    content,
                })
            }
            "thinking" => {
                let thinking = o
                    .get("thinking")
                    .and_then(|v| v.as_str())
                    .ok_or("no thinking content")?
                    .to_string();
                crate::llm::Content::Thinking(thinking)
            }
            _ => return Err(format!("unknown content type: {type_str}")),
        }
    } else {
        return Err("invalid content".into());
    };

    Ok(Message { role, content })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_id_not_empty() {
        let id = new_id();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_new_id_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_save_and_list_and_load() {
        let test_id = format!("test-{}", new_id());
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let dir_str = dir.to_string_lossy().to_string();

        let msgs = vec![
            Message {
                role: "user".into(),
                content: crate::llm::Content::Text("hello".into()),
            },
            Message {
                role: "assistant".into(),
                content: crate::llm::Content::Text("hi".into()),
            },
        ];

        let mut session = Session {
            id: test_id.clone(),
            messages: msgs,
            model: "claude-sonnet-4".into(),
            provider: "anthropic".into(),
            tokens_in: 10,
            tokens_out: 20,
            created_at: 0,
            updated_at: 0,
        };

        let result = save(&dir_str, &session);
        assert!(result.is_ok(), "save failed: {:?}", result);

        let summaries = list(&dir_str).unwrap();
        assert!(!summaries.is_empty());
        assert!(summaries.iter().any(|s| s.id == test_id));

        let loaded = load(&dir_str, &test_id).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.model, "claude-sonnet-4");
        assert_eq!(loaded.tokens_in, 10);
        assert_eq!(loaded.tokens_out, 20);

        session.messages.push(Message {
            role: "user".into(),
            content: crate::llm::Content::Text("follow-up".into()),
        });
        session.updated_at = 1;
        save(&dir_str, &session).unwrap();

        let loaded = load(&dir_str, &test_id).unwrap();
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.updated_at, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_save_enforces_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sessions");
        let dir_str = dir.to_string_lossy().to_string();
        let test_id = format!("test-{}", new_id());
        let mut session = Session {
            id: test_id.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: crate::llm::Content::Text("first".into()),
            }],
            model: "mock".into(),
            provider: "mock".into(),
            tokens_in: 1,
            tokens_out: 2,
            created_at: 3,
            updated_at: 4,
        };

        save(&dir_str, &session).unwrap();
        let path = dir.join(&test_id);
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        session.messages[0].content = crate::llm::Content::Text("replacement".into());
        save(&dir_str, &session).unwrap();

        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = load(&dir_str, &test_id).unwrap();
        assert!(matches!(
            &loaded.messages[0].content,
            crate::llm::Content::Text(text) if text == "replacement"
        ));
    }

    #[test]
    fn test_list_nonexistent_dir() {
        let summaries = list("/nonexistent/path/xyz123").unwrap();
        assert!(summaries.is_empty());
    }

    #[test]
    fn test_load_nonexistent() {
        let result = load("/nonexistent/path/xyz123", "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_removes_session_and_resolve_prefix() {
        let test_id = format!("test-{}", new_id());
        let dir = std::env::temp_dir().join(format!("cairn-test-session-del-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(&dir).unwrap();

        let session = Session {
            id: test_id.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: crate::llm::Content::Text("hello".into()),
            }],
            model: "mock".into(),
            provider: "mock".into(),
            tokens_in: 1,
            tokens_out: 1,
            created_at: 0,
            updated_at: 0,
        };
        save(&dir_str, &session).unwrap();
        assert!(dir.join(&test_id).is_file());

        let prefix = &test_id[..8];
        let resolved = resolve_id(&dir_str, prefix).unwrap();
        assert_eq!(resolved, test_id);

        delete(&dir_str, &resolved).unwrap();
        assert!(!dir.join(&test_id).is_file());
        assert!(load(&dir_str, &test_id).is_err());
        assert!(delete(&dir_str, &test_id).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_rejects_path_traversal() {
        let err = delete(".", "../Cargo.toml").unwrap_err();
        assert!(err.contains("invalid"), "got: {err}");
        let err = resolve_id(".", "..\\foo").unwrap_err();
        assert!(err.contains("invalid"), "got: {err}");

        let session = Session {
            id: "../escaped".into(),
            messages: Vec::new(),
            model: "mock".into(),
            provider: "mock".into(),
            tokens_in: 0,
            tokens_out: 0,
            created_at: 0,
            updated_at: 0,
        };
        assert!(save(".", &session).unwrap_err().contains("invalid"));
        let error = match load(".", "../Cargo.toml") {
            Ok(_) => panic!("path traversal should be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("invalid"));
    }

    #[test]
    fn test_list_and_load_derive_id_from_filename() {
        let dir = std::env::temp_dir().join(format!("cairn-test-session-id-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("actual-id"),
            r#"{"id":"other-id","model":"mock","provider":"mock","messages":[]}"#,
        )
        .unwrap();

        let listed = list(&dir_str).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "actual-id");
        assert_eq!(load(&dir_str, "actual-id").unwrap().id, "actual-id");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_surfaces_malformed_messages() {
        let dir = std::env::temp_dir().join(format!("cairn-test-session-bad-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(&dir).unwrap();
        let malformed = [
            r#"{"id":"bad-message","messages":{}}"#,
            r#"{"id":"bad-message","messages":[{"role":"user"}]}"#,
            r#"{"id":"bad-message","messages":[{"role":"assistant","content":{"type":"tool_use","name":"shell","input":{}}}]}"#,
            r#"{"id":"bad-message","messages":[{"role":"assistant","content":{"type":"tool_use","id":"call-1","name":"shell"}}]}"#,
            r#"{"id":"bad-message","messages":[{"role":"user","content":{"type":"tool_result","content":"ok"}}]}"#,
            r#"{"id":"bad-message","messages":[{"role":"assistant","content":{"type":"thinking"}}]}"#,
            r#"{"id":"bad-message","messages":[{"role":"assistant","content":{"type":"future"}}]}"#,
        ];

        for content in malformed {
            fs::write(dir.join("bad-message"), content).unwrap();
            assert!(
                load(&dir_str, "bad-message").is_err(),
                "accepted: {content}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_roundtrip_multiline_and_quotes() {
        let test_id = format!("test-{}", new_id());
        let dir = std::env::temp_dir().join(format!("cairn-test-session-ml-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(&dir).unwrap();

        let body = "line1\nline2\twith\ttabs\nand \"quotes\" and \\slashes";
        let session = Session {
            id: test_id.clone(),
            messages: vec![
                Message {
                    role: "user".into(),
                    content: crate::llm::Content::Text(body.into()),
                },
                Message {
                    role: "assistant".into(),
                    content: crate::llm::Content::Text("ok\nnext".into()),
                },
            ],
            model: "m".into(),
            provider: "p".into(),
            tokens_in: 1,
            tokens_out: 2,
            created_at: 10,
            updated_at: 20,
        };
        save(&dir_str, &session).unwrap();
        let loaded = load(&dir_str, &test_id).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        match &loaded.messages[0].content {
            crate::llm::Content::Text(t) => assert_eq!(t, body),
            _ => panic!("expected text"),
        }
        match &loaded.messages[1].content {
            crate::llm::Content::Text(t) => assert_eq!(t, "ok\nnext"),
            _ => panic!("expected text"),
        }
        // list must surface the file (corrupt JSON used to drop sessions silently)
        let listed = list(&dir_str).unwrap();
        assert!(listed.iter().any(|s| s.id == test_id));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_mirror_roundtrip() {
        let mirror = new_live_mirror();
        {
            let mut g = mirror.lock().unwrap();
            g.messages.push(Message {
                role: "user".into(),
                content: crate::llm::Content::Text("hi".into()),
            });
            g.tokens_in = 3;
            g.tokens_out = 7;
        }
        let g = mirror.lock().unwrap();
        assert_eq!(g.messages.len(), 1);
        assert_eq!(g.tokens_in, 3);
        assert_eq!(g.tokens_out, 7);
    }

    #[test]
    fn test_roundtrip_tool_use_and_result() {
        let test_id = format!("test-{}", new_id());
        let dir = std::env::temp_dir().join(format!("cairn-test-session-tools-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(&dir).unwrap();

        let session = Session {
            id: test_id.clone(),
            messages: vec![
                Message {
                    role: "user".into(),
                    content: crate::llm::Content::Text("run tests".into()),
                },
                Message {
                    role: "assistant".into(),
                    content: crate::llm::Content::ToolUse(crate::llm::ToolUse {
                        id: "call_1".into(),
                        name: "shell".into(),
                        input: r#"{"command":"cargo test"}"#.into(),
                    }),
                },
                Message {
                    role: "user".into(),
                    content: crate::llm::Content::ToolResult(crate::llm::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "ok\n147 passed".into(),
                    }),
                },
            ],
            model: "grok-4.5:high".into(),
            provider: "xai".into(),
            tokens_in: 9,
            tokens_out: 3,
            created_at: 1,
            updated_at: 2,
        };
        save(&dir_str, &session).unwrap();
        let loaded = load(&dir_str, &test_id).unwrap();
        assert_eq!(loaded.messages.len(), 3);
        match &loaded.messages[1].content {
            crate::llm::Content::ToolUse(tu) => {
                assert_eq!(tu.name, "shell");
                assert_eq!(tu.id, "call_1");
                assert!(tu.input.contains("cargo test"));
            }
            _ => panic!("expected tool_use"),
        }
        match &loaded.messages[2].content {
            crate::llm::Content::ToolResult(tr) => {
                assert_eq!(tr.tool_use_id, "call_1");
                assert!(tr.content.contains("147 passed"));
            }
            _ => panic!("expected tool_result"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tool_input_is_parsed_and_reserialized() {
        let test_id = format!("test-{}", new_id());
        let dir = std::env::temp_dir().join(format!("cairn-test-session-json-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        let input = r#" { "text": "a\"b", "nested": [true, null], "big": 9007199254740993 } "#;
        let session = Session {
            id: test_id.clone(),
            messages: vec![Message {
                role: "assistant".into(),
                content: crate::llm::Content::ToolUse(crate::llm::ToolUse {
                    id: "call-1".into(),
                    name: "example".into(),
                    input: input.into(),
                }),
            }],
            model: "mock".into(),
            provider: "mock".into(),
            tokens_in: 0,
            tokens_out: 0,
            created_at: 0,
            updated_at: 0,
        };

        save(&dir_str, &session).unwrap();
        let loaded = load(&dir_str, &test_id).unwrap();
        let crate::llm::Content::ToolUse(tool_use) = &loaded.messages[0].content else {
            panic!("expected tool use");
        };
        assert!(tool_use.input.contains("9007199254740993"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&tool_use.input).unwrap(),
            serde_json::from_str::<serde_json::Value>(input).unwrap()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalid_tool_input_does_not_replace_saved_session() {
        let test_id = format!("test-{}", new_id());
        let dir = std::env::temp_dir().join(format!("cairn-test-session-atomic-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        let mut session = Session {
            id: test_id.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: crate::llm::Content::Text("original".into()),
            }],
            model: "mock".into(),
            provider: "mock".into(),
            tokens_in: 0,
            tokens_out: 0,
            created_at: 0,
            updated_at: 0,
        };
        save(&dir_str, &session).unwrap();

        session.messages = vec![Message {
            role: "user".into(),
            content: crate::llm::Content::Text("replacement".into()),
        }];
        save(&dir_str, &session).unwrap();

        session.messages = vec![Message {
            role: "assistant".into(),
            content: crate::llm::Content::ToolUse(crate::llm::ToolUse {
                id: "call-1".into(),
                name: "example".into(),
                input: r#"{"valid":true} trailing"#.into(),
            }),
        }];
        assert!(save(&dir_str, &session).is_err());

        let loaded = load(&dir_str, &test_id).unwrap();
        assert!(matches!(
            &loaded.messages[0].content,
            crate::llm::Content::Text(text) if text == "replacement"
        ));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_failed_atomic_replace_removes_temporary_file() {
        let test_id = format!("test-{}", new_id());
        let dir = std::env::temp_dir().join(format!("cairn-test-session-cleanup-{}", new_id()));
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(dir.join(&test_id)).unwrap();
        let session = Session {
            id: test_id,
            messages: Vec::new(),
            model: "mock".into(),
            provider: "mock".into(),
            tokens_in: 0,
            tokens_out: 0,
            created_at: 0,
            updated_at: 0,
        };

        assert!(save(&dir_str, &session).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
