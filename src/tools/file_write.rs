use super::registry::Tool;

pub struct FileWriteTool;

impl Tool for FileWriteTool {
    fn name(&self) -> &str { "file_write" }
    fn description(&self) -> &str { "Create or overwrite a file" }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let file_path = obj.get("file_path").and_then(|v| v.as_str()).ok_or("file_path required")?;
        let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");

        let secured = super::workspace::acquire(file_path, true, false)?;
        let snapshot = secured.previous.clone();
        super::workspace::atomic_replace(&secured, content)?;
        super::file_history::record_snapshot(secured.relative, file_path, snapshot);
        Ok(format!("Written {} bytes to {}", content.len(), file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_path(label: &str) -> String {
        format!("target/cairn_{label}_{}_{}", std::process::id(), TEST_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn test_workspace_escape_is_rejected() {
        let tool = FileWriteTool;
        let input = r#"{"file_path":"../outside_cairn_write_test.txt","content":"x"}"#;
        let err = tool.execute(input).unwrap_err();
        assert!(err.contains("outside the workspace"), "unexpected error: {err}");
    }

    #[test]
    fn test_workspace_escape_after_nonexistent_prefix_is_rejected() {
        let prefix = unique_path("missing_write_prefix");
        let victim = format!(
            "outside_cairn_write_{}_{}.txt",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let outside = std::env::current_dir().unwrap().parent().unwrap().join(&victim);
        assert!(!outside.exists(), "unique outside victim unexpectedly exists");
        let tool = FileWriteTool;
        let input = format!(r#"{{"file_path":"{prefix}/../../../{victim}","content":"x"}}"#);
        let err = tool.execute(&input).unwrap_err();
        assert!(err.contains("outside the workspace"), "unexpected error: {err}");
        assert!(!outside.exists());
        assert!(!std::path::Path::new(&prefix).exists());
    }

    #[test]
    fn test_write_creates_file_with_content() {
        let path = unique_path("file_write");
        let tool = FileWriteTool;
        let input = format!(r#"{{"file_path":"{path}","content":"hello"}}"#);
        tool.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_write_creates_missing_nested_parents() {
        let base = unique_path("nested_write");
        let path = format!("{base}/one/two/file.txt");
        let input = format!(r#"{{"file_path":"{path}","content":"nested"}}"#);
        FileWriteTool.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "nested");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn test_final_symlink_cannot_modify_outside_file() {
        let link = unique_path("write_link");
        let outside = std::env::temp_dir().join(format!("cairn-outside-write-{}-{}", std::process::id(), TEST_COUNTER.fetch_add(1, Ordering::Relaxed)));
        fs::write(&outside, "original").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        if let Err(e) = std::os::windows::fs::symlink_file(&outside, &link) {
            if e.kind() == std::io::ErrorKind::PermissionDenied { fs::remove_file(outside).unwrap(); return; }
            panic!("symlink creation failed: {e}");
        }
        let input = format!(r#"{{"file_path":"{link}","content":"changed"}}"#);
        assert!(FileWriteTool.execute(&input).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
        fs::remove_file(link).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_dangling_directory_symlink_cannot_escape() {
        use std::os::unix::fs::symlink;
        let prefix = unique_path("dangling_write");
        let outside = std::env::temp_dir().join(format!("cairn-outside-{}-{}", std::process::id(), TEST_COUNTER.fetch_add(1, Ordering::Relaxed)));
        symlink(&outside, &prefix).unwrap();
        let input = format!(r#"{{"file_path":"{prefix}/created.txt","content":"x"}}"#);
        assert!(FileWriteTool.execute(&input).is_err());
        assert!(!outside.join("created.txt").exists());
        assert!(!outside.exists());
        fs::remove_file(prefix).unwrap();
    }

    #[test]
    fn test_directory_symlink_cannot_escape() {
        let link = unique_path("write_dir_link");
        let outside = std::env::temp_dir().join(format!(
            "cairn-outside-dir-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        if let Err(e) = std::os::windows::fs::symlink_dir(&outside, &link) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                fs::remove_dir(outside).unwrap();
                return;
            }
            panic!("directory symlink creation failed: {e}");
        }
        let input = format!(r#"{{"file_path":"{link}/created.txt","content":"x"}}"#);
        assert!(FileWriteTool.execute(&input).is_err());
        assert!(!outside.join("created.txt").exists());
        #[cfg(unix)]
        fs::remove_file(link).unwrap();
        #[cfg(windows)]
        fs::remove_dir(link).unwrap();
        fs::remove_dir(outside).unwrap();
    }
}
