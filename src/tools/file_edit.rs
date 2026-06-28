use std::fs;
use super::registry::Tool;

pub struct FileEditTool;

impl Tool for FileEditTool {
    fn name(&self) -> &str { "file_edit" }
    fn description(&self) -> &str { "Find and replace text in a file" }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["file_path","old_string","new_string"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let file_path = obj.get("file_path").and_then(|v| v.as_str()).ok_or("file_path required")?;
        let old_string = obj.get("old_string").and_then(|v| v.as_str()).ok_or("old_string required")?;
        let new_string = obj.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        let replace_all = obj.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);

        let content = fs::read_to_string(file_path).map_err(|e| format!("read error: {e}"))?;

        if !content.contains(old_string) {
            return Err(format!("old_string not found in {file_path}"));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            match content.find(old_string) {
                Some(pos) => {
                    let mut result = content[..pos].to_string();
                    result.push_str(new_string);
                    result.push_str(&content[pos + old_string.len()..]);
                    result
                }
                None => return Err("old_string not found".into()),
            }
        };

        let count = if replace_all {
            content.matches(old_string).count()
        } else {
            1
        };

        fs::write(file_path, &new_content).map_err(|e| format!("write error: {e}"))?;
        Ok(format!("Applied {count} edit(s) to {file_path}"))
    }
}
