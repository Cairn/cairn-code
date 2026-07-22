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
        let path = "target/cairn_file_undo_edit.txt";
        fs::write(path, "original").unwrap();

        let edit = FileEditTool;
        let input = format!(r#"{{"file_path":"{path}","old_string":"original","new_string":"changed"}}"#);
        edit.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "changed");

        FileUndoTool.execute("{}").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "original");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn undo_removes_newly_created_file() {
        crate::tools::file_history::clear();
        let path = "target/cairn_file_undo_create.txt";
        let _ = fs::remove_file(path);

        let write = FileWriteTool;
        let input = format!(r#"{{"file_path":"{path}","content":"brand new"}}"#);
        write.execute(&input).unwrap();
        assert!(std::path::Path::new(path).exists());

        FileUndoTool.execute("{}").unwrap();
        assert!(!std::path::Path::new(path).exists());
    }

    #[test]
    fn undo_empty_stack_errors_clearly() {
        crate::tools::file_history::clear();
        let err = FileUndoTool.execute("{}").unwrap_err();
        assert!(err.to_ascii_lowercase().contains("nothing to undo"), "got: {err}");
    }
}
