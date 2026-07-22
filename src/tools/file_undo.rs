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

    #[test]
    fn undo_restores_file_edit() {
        crate::tools::file_history::clear();
        // Stay inside the workspace (cwd) so resolve_in_workspace accepts the path.
        // Unique subdir avoids parallel test collisions; do not touch process cwd.
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
    fn undo_empty_stack_errors_clearly() {
        crate::tools::file_history::clear();
        let err = FileUndoTool.execute("{}").unwrap_err();
        assert!(err.to_ascii_lowercase().contains("nothing to undo"), "got: {err}");
    }
}
