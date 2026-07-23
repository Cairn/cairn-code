use super::registry::Tool;

/// Restores the most recent `file_edit` / `file_write` in this process.
pub struct FileUndoTool;

impl Tool for FileUndoTool {
    fn name(&self) -> &str { "file_undo" }
    fn description(&self) -> &str {
        "Undo the most recent file_edit or file_write in this session, restoring the previous file contents (or deleting a newly created file)"
    }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{},"required":[]}"#.into()
    }

    fn execute(&self, _input: &str) -> Result<String, String> {
        super::file_history::undo_last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::file_edit::FileEditTool;
    use crate::tools::file_write::FileWriteTool;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn undo_restores_file_edit() {
        crate::tools::file_history::clear();
        // A unique workspace-relative subdirectory avoids parallel collisions.
        let rel = format!(
            "target/cairn_undo_edit_{}/f.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        if let Some(parent) = std::path::Path::new(&rel).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&rel, "original").unwrap();

        let edit = FileEditTool;
        let input = format!(
            r#"{{"file_path":"{rel}","old_string":"original","new_string":"changed"}}"#
        );
        edit.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(&rel).unwrap(), "changed");

        FileUndoTool.execute("{}").unwrap();
        assert_eq!(fs::read_to_string(&rel).unwrap(), "original");
        if let Some(parent) = std::path::Path::new(&rel).parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn undo_removes_newly_created_file() {
        crate::tools::file_history::clear();
        let rel = format!(
            "target/cairn_undo_create_{}/f.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        if let Some(parent) = std::path::Path::new(&rel).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let _ = fs::remove_file(&rel);

        let write = FileWriteTool;
        let input = format!(r#"{{"file_path":"{rel}","content":"brand new"}}"#);
        write.execute(&input).unwrap();
        assert!(std::path::Path::new(&rel).exists());

        FileUndoTool.execute("{}").unwrap();
        assert!(!std::path::Path::new(&rel).exists());
        if let Some(parent) = std::path::Path::new(&rel).parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn undo_restores_preexisting_empty_file() {
        crate::tools::file_history::clear();
        let rel = format!("target/cairn_undo_empty_{}/f.txt", std::process::id());
        fs::create_dir_all(std::path::Path::new(&rel).parent().unwrap()).unwrap();
        fs::write(&rel, "").unwrap();
        let input = format!(r#"{{"file_path":"{rel}","content":"changed"}}"#);
        FileWriteTool.execute(&input).unwrap();
        FileUndoTool.execute("{}").unwrap();
        assert!(std::path::Path::new(&rel).is_file());
        assert_eq!(fs::read_to_string(&rel).unwrap(), "");
        fs::remove_dir_all(std::path::Path::new(&rel).parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_undo_cannot_follow_symlink_and_remains_retryable() {
        crate::tools::file_history::clear();
        let sequence = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let rel = format!("target/cairn_undo_link_{}_{sequence}.txt", std::process::id());
        let outside = std::env::temp_dir().join(format!(
            "cairn-undo-outside-{}-{sequence}.txt",
            std::process::id()
        ));
        assert!(!outside.exists(), "unique outside victim unexpectedly exists");
        fs::write(&outside, "outside").unwrap();

        let input = format!(r#"{{"file_path":"{rel}","content":"created"}}"#);
        FileWriteTool.execute(&input).unwrap();
        fs::remove_file(&rel).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &rel).unwrap();
        #[cfg(windows)]
        if let Err(e) = std::os::windows::fs::symlink_file(&outside, &rel) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                fs::remove_file(outside).unwrap();
                return;
            }
            panic!("symlink creation failed: {e}");
        }

        assert!(FileUndoTool.execute("{}").is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");

        fs::remove_file(&rel).unwrap();
        FileUndoTool.execute("{}").unwrap();
        assert!(!std::path::Path::new(&rel).exists());
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn undo_empty_stack_errors_clearly() {
        crate::tools::file_history::clear();
        let err = FileUndoTool.execute("{}").unwrap_err();
        assert!(err.to_ascii_lowercase().contains("nothing to undo"), "got: {err}");
    }
}
