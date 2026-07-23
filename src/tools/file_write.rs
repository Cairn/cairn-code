use super::registry::Tool;
use std::fs;

pub struct FileWriteTool;

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

        let resolved = super::workspace::resolve_in_workspace(file_path)?;

        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }

        super::file_history::record_before_write(resolved.clone(), file_path)?;
        fs::write(&resolved, content).map_err(|e| format!("write error: {e}"))?;
        Ok(format!("Written {} bytes to {}", content.len(), file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_escape_is_rejected() {
        let tool = FileWriteTool;
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
        let tool = FileWriteTool;
        let input = format!(r#"{{"file_path":"{path}","content":"hello"}}"#);
        tool.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "hello");
        let _ = fs::remove_file(path);
    }
}
