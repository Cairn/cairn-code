use std::fs;
use super::registry::Tool;

pub struct FileReadTool;

impl Tool for FileReadTool {
    fn name(&self) -> &str { "file_read" }
    fn description(&self) -> &str { "Read a file with optional offset and limit" }
    fn needs_permission(&self) -> bool { false }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["file_path"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let file_path = obj.get("file_path").and_then(|v| v.as_str()).ok_or("file_path required")?;
        let offset = obj.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = obj.get("limit").and_then(|v| v.as_u64());

        let content = fs::read_to_string(file_path).map_err(|e| format!("read error: {e}"))?;
        let lines: Vec<&str> = content.lines().collect();

        let start = offset.min(lines.len());
        let end = match limit {
            Some(l) => (start + l as usize).min(lines.len()),
            None => lines.len(),
        };

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate().skip(start).take(end - start) {
            result.push_str(&format!("{}:{}\n", i + 1, line));
        }
        let start_display = start + 1;
        let total = lines.len();
        result.push_str(&format!("\n{file_path}:{start_display} (showing lines {start_display}-{end_display} of {total})", file_path=file_path, start_display=start_display, end_display=end, total=total));
        Ok(result)
    }
}
