use std::fs;
use std::path::PathBuf;
use super::registry::Tool;

pub struct MemoryTool;

impl Tool for MemoryTool {
    fn name(&self) -> &str { "memory" }
    fn description(&self) -> &str {
        "Store and retrieve cross-session information. Use for user preferences, project conventions, and important context."
    }
    fn needs_permission(&self) -> bool { false }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["save","recall","list","delete","search"]},"key":{"type":"string"},"content":{"type":"string"},"query":{"type":"string"}},"required":["action"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let action = obj.get("action").and_then(|v| v.as_str()).ok_or("action required")?;

        let dir = memory_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
        }

        match action {
            "save" => {
                let key = obj.get("key").and_then(|v| v.as_str()).ok_or("key required for save")?;
                let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let path = dir.join(format!("{key}.md"));
                let now = timestamp();
                let (created, existing_content) = if path.exists() {
                    if let Ok(existing) = fs::read_to_string(&path) {
                        let c = parse_frontmatter(&existing);
                        (c.0, c.1)
                    } else {
                        (now.clone(), String::new())
                    }
                } else {
                    (now.clone(), String::new())
                };
                let body = if content.is_empty() { &existing_content } else { content };
                let out = format!("---\nkey: {key}\ncreated_at: {created}\nupdated_at: {now}\n---\n\n{body}");
                fs::write(&path, &out).map_err(|e| format!("write: {e}"))?;
                Ok(format!("Saved memory '{}'", key))
            }
            "recall" => {
                let key = obj.get("key").and_then(|v| v.as_str()).ok_or("key required for recall")?;
                let path = dir.join(format!("{key}.md"));
                if !path.exists() {
                    return Err(format!("Memory '{}' not found", key));
                }
                let content = fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
                let (_, body) = parse_frontmatter(&content);
                Ok(format!("---\n{}\n{}", key, body.trim()))
            }
            "list" => {
                let query = obj.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let mut entries: Vec<String> = Vec::new();
                let read_dir = fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))?;
                for entry in read_dir {
                    let entry = entry.map_err(|e| format!("entry: {e}"))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(key) = name.strip_suffix(".md") {
                        if !query.is_empty() {
                            if let Ok(content) = fs::read_to_string(entry.path()) {
                                if !content.contains(query) { continue; }
                            } else { continue; }
                        }
                        entries.push(key.to_string());
                    }
                }
                if entries.is_empty() {
                    return Ok("No memories found.".to_string());
                }
                Ok(format!("Memories:\n{}", entries.join("\n")))
            }
            "delete" => {
                let key = obj.get("key").and_then(|v| v.as_str()).ok_or("key required for delete")?;
                let path = dir.join(format!("{key}.md"));
                if !path.exists() {
                    return Err(format!("Memory '{}' not found", key));
                }
                fs::remove_file(&path).map_err(|e| format!("delete: {e}"))?;
                Ok(format!("Deleted memory '{}'", key))
            }
            "search" => {
                let query = obj.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query.is_empty() { return Err("query required for search".into()); }
                let mut results: Vec<(String, String)> = Vec::new();
                let read_dir = fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))?;
                for entry in read_dir {
                    let entry = entry.map_err(|e| format!("entry: {e}"))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(key) = name.strip_suffix(".md") {
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            let (_, body) = parse_frontmatter(&content);
                            if body.contains(query) || key.contains(query) {
                                let preview: String = body.chars().take(200).collect();
                                results.push((key.to_string(), preview));
                            }
                        }
                    }
                }
                if results.is_empty() {
                    return Ok("No memories match query.".to_string());
                }
                let out: Vec<String> = results.iter().map(|(k, b)| format!("{k}: {b}")).collect();
                Ok(format!("Search results:\n{}", out.join("\n---\n")))
            }
            _ => Err(format!("Unknown action: {action}")),
        }
    }
}

fn memory_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/cairn-code/memory")
}

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    let nanos = dur.subsec_nanos();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let sec = time_secs % 60;
    let year = 1970 + (days as f64 / 365.25) as u64;
    let month = 1 + ((days as f64 / 30.44) as u64 % 12);
    let day = 1 + (days as u64 % 28);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{sec:02}.{nanos:06}Z")
}

fn parse_frontmatter(content: &str) -> (String, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (String::new(), trimmed.to_string());
    }
    let after_delim = trimmed.trim_start_matches("---").trim_start();
    if let Some(end) = after_delim.find("\n---") {
        let front = &after_delim[..end];
        let body = after_delim[end + 4..].trim_start().to_string();
        let mut created = String::new();
        for line in front.lines() {
            if let Some(val) = line.strip_prefix("created_at:") {
                created = val.trim().trim_matches('"').to_string();
            }
        }
        (created, body)
    } else {
        (String::new(), trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_basic() {
        let input = "---\nkey: test\ncreated_at: 2026-06-28T12:00:00Z\n---\n\nHello world";
        let (created, body) = parse_frontmatter(input);
        assert_eq!(created, "2026-06-28T12:00:00Z");
        assert_eq!(body.trim(), "Hello world");
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let (created, body) = parse_frontmatter("Just plain text");
        assert_eq!(created, "");
        assert_eq!(body, "Just plain text");
    }

    #[test]
    fn test_parse_frontmatter_missing_delim() {
        let input = "---\nkey: test\nno closing delim";
        let (_created, body) = parse_frontmatter(input);
        assert!(body.contains("key: test"));
    }

    #[test]
    fn test_parse_frontmatter_empty() {
        let (created, body) = parse_frontmatter("");
        assert_eq!(created, "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_tool_name_and_description() {
        let tool = MemoryTool;
        assert_eq!(tool.name(), "memory");
        assert!(tool.description().contains("cross-session"));
    }

    #[test]
    fn test_input_schema_is_valid_json() {
        let tool = MemoryTool;
        let schema = tool.input_schema();
        let parsed = crate::json::parse(&schema);
        assert!(parsed.is_ok(), "Schema should be valid JSON: {:?}", parsed.err());
        let obj = parsed.unwrap();
        let props = obj.get("properties").and_then(|v| v.as_object());
        assert!(props.is_some(), "Schema should have properties");
        assert!(props.unwrap().contains_key("action"), "Schema should have action property");
    }

    #[test]
    fn test_execute_unknown_action() {
        let tool = MemoryTool;
        let result = tool.execute(r#"{"action":"invalid"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown action"));
    }

    #[test]
    fn test_execute_missing_action() {
        let tool = MemoryTool;
        let result = tool.execute(r#"{}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_invalid_json() {
        let tool = MemoryTool;
        let result = tool.execute("not json");
        assert!(result.is_err());
    }
}
