use std::fs;
use std::path::Path;
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

        if let Some(parent) = Path::new(file_path).parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }

        fs::write(file_path, content).map_err(|e| format!("write error: {e}"))?;
        Ok(format!("Written {} bytes to {}", content.len(), file_path))
    }
}
