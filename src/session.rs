#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::llm::Message;

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

pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:016x}", nanos)
}

pub fn save(sessions_dir: &str, session: &Session) -> Result<(), String> {
    let dir = PathBuf::from(sessions_dir);
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;

    let path = dir.join(&session.id);
    let json = session_to_json(session);
    fs::write(&path, json).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

pub fn load(sessions_dir: &str, id: &str) -> Result<Session, String> {
    let path = PathBuf::from(sessions_dir).join(id);
    let content = fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
    session_from_json(&content)
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
        if path.is_file() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(s) = session_from_json(&content) {
                    sessions.push(SessionSummary {
                        id: s.id,
                        model: s.model,
                        msg_count: s.messages.len(),
                        updated_at: s.updated_at,
                        summary: s.messages.first().and_then(|m| match &m.content {
                            crate::llm::Content::Text(t) => Some(t.clone()),
                            _ => None,
                        }).unwrap_or_default(),
                    });
                }
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

fn session_to_json(s: &Session) -> String {
    let mut json = String::new();
    json.push_str(&format!("{{\"id\":\"{}\",\"model\":\"{}\",\"provider\":\"{}\",", s.id, s.model, s.provider));
    json.push_str(&format!("\"tokens_in\":{},\"tokens_out\":{},", s.tokens_in, s.tokens_out));
    json.push_str(&format!("\"created_at\":{},\"updated_at\":{},", s.created_at, s.updated_at));
    json.push_str("\"messages\":[");
    for (i, msg) in s.messages.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push_str(&message_to_json(msg));
    }
    json.push_str("]}");
    json
}

fn message_to_json(msg: &Message) -> String {
    let content = match &msg.content {
        crate::llm::Content::Text(t) => format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"")),
        crate::llm::Content::ToolUse(tu) => {
            format!("{{\"type\":\"tool_use\",\"name\":\"{}\",\"input\":{}}}", tu.name, tu.input)
        }
        crate::llm::Content::ToolResult(tr) => {
            format!("{{\"type\":\"tool_result\",\"tool_use_id\":\"{}\",\"content\":\"{}\"}}",
                tr.tool_use_id, tr.content.replace('\\', "\\\\").replace('"', "\\\""))
        }
        crate::llm::Content::Thinking(t) => {
            format!("{{\"type\":\"thinking\",\"thinking\":\"{}\"}}", t.replace('\\', "\\\\").replace('"', "\\\""))
        }
    };
    format!("{{\"role\":\"{}\",\"content\":{}}}", msg.role, content)
}

fn session_from_json(json_str: &str) -> Result<Session, String> {
    let val = crate::json::parse(json_str).map_err(|e| e.to_string())?;
    let obj = val.as_object().ok_or("not an object")?;

    let id = obj.get("id").and_then(|v| v.as_str()).ok_or("no id")?.to_string();
    let model = obj.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let provider = obj.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let tokens_in = obj.get("tokens_in").and_then(|v| v.as_u64()).unwrap_or(0);
    let tokens_out = obj.get("tokens_out").and_then(|v| v.as_u64()).unwrap_or(0);
    let created_at = obj.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0);
    let updated_at = obj.get("updated_at").and_then(|v| v.as_u64()).unwrap_or(0);

    let messages = if let Some(arr) = obj.get("messages").and_then(|v| v.as_array()) {
        arr.iter().filter_map(|v| message_from_json(v)).collect()
    } else {
        Vec::new()
    };

    Ok(Session { id, model, provider, messages, tokens_in, tokens_out, created_at, updated_at })
}

fn message_from_json(val: &crate::json::JsonValue) -> Option<Message> {
    let obj = val.as_object()?;
    let role = obj.get("role")?.as_str()?.to_string();
    let content_val = obj.get("content")?;

    let content = if let Some(s) = content_val.as_str() {
        crate::llm::Content::Text(s.to_string())
    } else if let Some(o) = content_val.as_object() {
        let type_str = o.get("type")?.as_str()?;
        match type_str {
            "tool_use" => {
                let name = o.get("name")?.as_str()?.to_string();
                let input = crate::json::serialize(o.get("input")?);
                crate::llm::Content::ToolUse(crate::llm::ToolUse { name, input, id: String::new() })
            }
            "tool_result" => {
                let tool_use_id = o.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let content = o.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                crate::llm::Content::ToolResult(crate::llm::ToolResult { tool_use_id, content })
            }
            "thinking" => {
                let thinking = o.get("thinking").and_then(|v| v.as_str()).unwrap_or("").to_string();
                crate::llm::Content::Thinking(thinking)
            }
            _ => crate::llm::Content::Text(crate::json::serialize(content_val)),
        }
    } else {
        return None;
    };

    Some(Message { role, content })
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
        let dir = std::env::temp_dir().join("cairn-test-session");
        let dir_str = dir.to_string_lossy().to_string();
        fs::create_dir_all(&dir).unwrap();

        let msgs = vec![
            Message { role: "user".into(), content: crate::llm::Content::Text("hello".into()) },
            Message { role: "assistant".into(), content: crate::llm::Content::Text("hi".into()) },
        ];

        let session = Session {
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

        // Cleanup
        let _ = fs::remove_file(dir.join(&test_id));
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
}
