use super::registry::Tool;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct MemoryTool;
static MEMORY_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        "Store and retrieve cross-session information. Use for user preferences, project conventions, and important context."
    }
    fn needs_permission(&self) -> bool {
        false
    }
    fn needs_permission_for(&self, input: &str) -> bool {
        crate::json::parse(input)
            .ok()
            .map(|value| {
                matches!(
                    value.get("action").and_then(|action| action.as_str()),
                    Some("save" | "delete")
                )
            })
            .unwrap_or(false)
    }
    fn permission_key(&self, input: &str) -> String {
        let action = crate::json::parse(input)
            .ok()
            .and_then(|value| value.get("action")?.as_str().map(str::to_owned));
        match action.as_deref() {
            Some(action @ ("save" | "delete")) => format!("memory:{action}"),
            _ => self.name().to_string(),
        }
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["save","recall","list","delete","search"]},"key":{"type":"string"},"content":{"type":"string"},"query":{"type":"string"}},"required":["action"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let action = obj
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("action required")?;

        match action {
            "save" => {
                let key = obj
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or("key required for save")?;
                let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let root =
                    open_memory_dir(true).map_err(|e| format!("open memory directory: {e}"))?;
                save_memory(&root, key, content)?;
                Ok(format!("Saved memory '{}'", key))
            }
            "recall" => {
                let key = obj
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or("key required for recall")?;
                let Some(root) = open_existing_memory_dir()? else {
                    return Err(format!("Memory '{}' not found", key));
                };
                let content = read_memory(&root, key)?;
                let (_, body) = parse_frontmatter(&content);
                Ok(format!("---\n{}\n{}", key, body.trim()))
            }
            "list" => {
                let query = obj.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let Some(root) = open_existing_memory_dir()? else {
                    return Ok("No memories found.".to_string());
                };
                let mut entries: Vec<String> = Vec::new();
                for entry in root.entries().map_err(|e| format!("read dir: {e}"))? {
                    let entry = entry.map_err(|e| format!("entry: {e}"))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(key) = name.strip_suffix(".md") {
                        if memory_file_name(key).is_err() {
                            continue;
                        }
                        if query.is_empty() {
                            if validate_memory_file(&root, &name, key).is_err() {
                                continue;
                            }
                        } else {
                            if let Ok(content) = read_memory(&root, key) {
                                if !content.contains(query) {
                                    continue;
                                }
                            } else {
                                continue;
                            }
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
                let key = obj
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or("key required for delete")?;
                let Some(root) = open_existing_memory_dir()? else {
                    return Err(format!("Memory '{}' not found", key));
                };
                let name = memory_file_name(key)?;
                reject_symlink(&root, &name, key)?;
                root.remove_file(&name).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        format!("Memory '{}' not found", key)
                    } else {
                        format!("delete: {e}")
                    }
                })?;
                Ok(format!("Deleted memory '{}'", key))
            }
            "search" => {
                let query = obj.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query.is_empty() {
                    return Err("query required for search".into());
                }
                let Some(root) = open_existing_memory_dir()? else {
                    return Ok("No memories match query.".to_string());
                };
                let mut results: Vec<(String, String)> = Vec::new();
                for entry in root.entries().map_err(|e| format!("read dir: {e}"))? {
                    let entry = entry.map_err(|e| format!("entry: {e}"))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(key) = name.strip_suffix(".md") {
                        if memory_file_name(key).is_err() {
                            continue;
                        }
                        if let Ok(content) = read_memory(&root, key) {
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

fn memory_home() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| "HOME or USERPROFILE is required for memory storage".to_string())
}

fn validate_memory_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("memory key must contain only ASCII letters, numbers, '-' or '_'".into());
    }
    Ok(())
}

fn memory_file_name(key: &str) -> Result<String, String> {
    validate_memory_key(key)?;
    Ok(format!("{key}.md"))
}

fn open_memory_dir(create: bool) -> std::io::Result<Dir> {
    let home =
        memory_home().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    open_memory_dir_at(&home, create)
}

fn open_memory_dir_at(home: &Path, create: bool) -> std::io::Result<Dir> {
    let mut current = Dir::open_ambient_dir(home, ambient_authority())?;
    for component in [".config", "cairn-code", "memory"] {
        if create {
            match current.create_dir(component) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        current = current.open_dir_nofollow(component)?;
    }
    Ok(current)
}

fn open_existing_memory_dir() -> Result<Option<Dir>, String> {
    match open_memory_dir(false) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("open memory directory: {error}")),
    }
}

fn save_memory(root: &Dir, key: &str, content: &str) -> Result<(), String> {
    let name = memory_file_name(key)?;
    let now = timestamp();
    let (created, existing_content) = match read_memory_file(root, &name, key) {
        Ok(existing) => parse_frontmatter(&existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (now.clone(), String::new()),
        Err(error) => return Err(format!("read existing memory: {error}")),
    };
    let body = if content.is_empty() {
        &existing_content
    } else {
        content
    };
    let output =
        format!("---\nkey: {key}\ncreated_at: {created}\nupdated_at: {now}\n---\n\n{body}");
    write_memory_file_atomic(root, &name, output.as_bytes()).map_err(|e| format!("write: {e}"))
}

fn write_memory_file_atomic(root: &Dir, name: &str, contents: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    for _ in 0..16 {
        let sequence = MEMORY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_name = format!(".memory-{}-{sequence}.tmp", std::process::id());
        let mut file = match open_memory_file(root, &temp_name, &temp_name, &options) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };

        let result = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            drop(file);
            root.rename(&temp_name, root, name)
        })();
        if result.is_err() {
            let _ = root.remove_file(&temp_name);
        }
        return result;
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a temporary memory file",
    ))
}

fn reject_symlink(root: &Dir, name: &str, key: &str) -> Result<(), String> {
    match root.symlink_metadata(name) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("refusing to follow symlink for memory '{key}'"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect memory path: {error}")),
    }
    Ok(())
}

fn open_memory_file(
    root: &Dir,
    name: &str,
    key: &str,
    options: &OpenOptions,
) -> std::io::Result<File> {
    let mut options = options.clone();
    options.follow(FollowSymlinks::No);
    root.open_with(name, &options).map_err(|error| {
        if root
            .symlink_metadata(name)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("refusing to follow symlink for memory '{key}'"),
            )
        } else {
            error
        }
    })
}

fn read_memory_file(root: &Dir, name: &str, key: &str) -> std::io::Result<String> {
    let mut options = OpenOptions::new();
    options.read(true);
    let mut file = open_memory_file(root, name, key, &options)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

fn validate_memory_file(root: &Dir, name: &str, key: &str) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.read(true);
    open_memory_file(root, name, key, &options).map(|_| ())
}

fn read_memory(root: &Dir, key: &str) -> Result<String, String> {
    let name = memory_file_name(key)?;
    read_memory_file(root, &name, key).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("Memory '{}' not found", key)
        } else {
            format!("read: {e}")
        }
    })
}

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
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
    use std::fs;

    fn temp_path(label: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cairn-memory-{label}-{nanos}"))
    }

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
    fn test_mutating_actions_need_permission() {
        let tool = MemoryTool;
        assert!(tool.needs_permission_for(r#"{"action":"save","key":"test"}"#));
        assert!(tool.needs_permission_for(r#"{"action":"delete","key":"test"}"#));
        assert_eq!(
            tool.permission_key(r#"{"action":"save","key":"test"}"#),
            "memory:save"
        );
        assert_eq!(
            tool.permission_key(r#"{"action":"delete","key":"test"}"#),
            "memory:delete"
        );
        assert!(!tool.needs_permission_for(r#"{"action":"recall","key":"test"}"#));
        assert_eq!(
            tool.permission_key(r#"{"action":"recall","key":"test"}"#),
            "memory"
        );
        assert!(!tool.needs_permission_for(r#"{"action":"list"}"#));
        assert!(!tool.needs_permission_for("invalid"));
    }

    #[test]
    fn test_memory_file_name_rejects_unsafe_keys() {
        for key in [
            "",
            "../secret",
            "..\\secret",
            "nested/key",
            "nested\\key",
            ".",
            "two words",
        ] {
            assert!(memory_file_name(key).is_err(), "accepted unsafe key: {key}");
        }
        assert_eq!(memory_file_name("safe-key_123").unwrap(), "safe-key_123.md");
    }

    #[cfg(unix)]
    #[test]
    fn test_memory_path_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let base = temp_path("symlink-test");
        let outside = base.join("outside.md");
        fs::create_dir_all(&base).unwrap();
        let root = open_memory_dir_at(&base, true).unwrap();
        fs::write(&outside, "secret").unwrap();
        symlink(&outside, base.join(".config/cairn-code/memory/linked.md")).unwrap();

        let error = read_memory(&root, "linked").unwrap_err();
        assert!(error.contains("symlink"), "unexpected error: {error}");

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn test_directory_capability_blocks_symlink_swap() {
        use std::os::unix::fs::symlink;

        let base = temp_path("symlink-swap-test");
        let outside = base.join("outside.md");
        fs::create_dir_all(&base).unwrap();
        let root = open_memory_dir_at(&base, true).unwrap();
        fs::write(&outside, "secret").unwrap();

        reject_symlink(&root, "linked.md", "linked").unwrap();
        symlink(&outside, base.join(".config/cairn-code/memory/linked.md")).unwrap();

        assert!(read_memory_file(&root, "linked.md", "linked").is_err());

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn test_memory_root_rejects_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let base = temp_path("parent-symlink-test");
        let home = base.join("home");
        let outside = base.join("outside");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(outside.join("cairn-code/memory")).unwrap();
        fs::write(
            outside.join("cairn-code/memory/sentinel.md"),
            "outside secret",
        )
        .unwrap();
        symlink(&outside, home.join(".config")).unwrap();

        assert!(open_memory_dir_at(&home, false).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("cairn-code/memory/sentinel.md")).unwrap(),
            "outside secret"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(windows)]
    #[test]
    fn test_memory_root_rejects_junctioned_parent() {
        use std::process::Command;

        let base = temp_path("junction-test");
        let home = base.join("home");
        let outside = base.join("outside");
        let junction = home.join(".config");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(outside.join("cairn-code/memory")).unwrap();
        fs::write(
            outside.join("cairn-code/memory/sentinel.md"),
            "outside secret",
        )
        .unwrap();

        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(status.success(), "failed to create test junction");

        assert!(open_memory_dir_at(&home, false).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("cairn-code/memory/sentinel.md")).unwrap(),
            "outside secret"
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_save_replaces_hard_link_without_modifying_outside_file() {
        let base = temp_path("hard-link-test");
        let outside = base.join("outside.md");
        let memory_path = base.join(".config/cairn-code/memory/linked.md");
        fs::create_dir_all(&base).unwrap();
        let root = open_memory_dir_at(&base, true).unwrap();
        fs::write(&outside, "outside content").unwrap();
        fs::hard_link(&outside, &memory_path).unwrap();

        save_memory(&root, "linked", "new memory").unwrap();

        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside content");
        assert!(fs::read_to_string(&memory_path)
            .unwrap()
            .contains("new memory"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_save_read_error_preserves_existing_file() {
        let base = temp_path("read-error-test");
        let memory_path = base.join(".config/cairn-code/memory/binary.md");
        fs::create_dir_all(&base).unwrap();
        let root = open_memory_dir_at(&base, true).unwrap();
        let original = [0xff, 0xfe, 0xfd];
        fs::write(&memory_path, original).unwrap();

        let error = save_memory(&root, "binary", "replacement").unwrap_err();

        assert!(
            error.contains("read existing memory"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read(&memory_path).unwrap(), original);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_input_schema_is_valid_json() {
        let tool = MemoryTool;
        let schema = tool.input_schema();
        let parsed = crate::json::parse(&schema);
        assert!(
            parsed.is_ok(),
            "Schema should be valid JSON: {:?}",
            parsed.err()
        );
        let obj = parsed.unwrap();
        let props = obj.get("properties").and_then(|v| v.as_object());
        assert!(props.is_some(), "Schema should have properties");
        assert!(
            props.unwrap().contains_key("action"),
            "Schema should have action property"
        );
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
