use super::registry::Tool;
use super::workspace::Workspace;

pub struct FileWriteTool {
    workspace: Workspace,
}

impl FileWriteTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }
    fn description(&self) -> &str {
        "Create or overwrite a file"
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let file_path = obj
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or("file_path required")?;
        let content = obj.get("content").and_then(|v| v.as_str()).unwrap_or("");

        let relative = self.workspace.relative_path(file_path)?;
        self.workspace.create_parent_dirs(&relative)?;
        super::file_history::record_before_write(&self.workspace, relative.clone(), file_path)?;
        self.workspace
            .dir()
            .write(&relative, content)
            .map_err(|e| format!("write error: {}", self.workspace.access_error(&relative, e)))?;
        Ok(format!("Written {} bytes to {}", content.len(), file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tool() -> FileWriteTool {
        FileWriteTool::new(Workspace::current().unwrap())
    }

    #[test]
    fn test_workspace_escape_is_rejected() {
        let tool = tool();
        let input = r#"{"file_path":"../outside_cairn_write_test.txt","content":"x"}"#;
        let err = tool.execute(input).unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_write_creates_file_with_content() {
        let path = "target/cairn_file_write_test.txt";
        let tool = tool();
        let input = format!(r#"{{"file_path":"{path}","content":"hello"}}"#);
        tool.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "hello");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_missing_prefix_escape_is_rejected() {
        let tool = tool();
        let input = r#"{"file_path":"missing/../../outside_cairn_write_test.txt","content":"x"}"#;
        let err = tool.execute(input).unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_directory_link_escape_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "cairn-file-write-link-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let link = workspace.join("escape");
        assert!(
            create_dir_link(&outside, &link),
            "failed to create test link"
        );

        let tool = FileWriteTool::new(Workspace::new(&workspace).unwrap());
        let err = tool
            .execute(r#"{"file_path":"escape/escaped.txt","content":"x"}"#)
            .unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
        assert!(!outside.join("escaped.txt").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}
