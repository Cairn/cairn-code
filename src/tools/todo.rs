use std::fs;
use std::path::PathBuf;
use super::registry::Tool;

pub struct TodoTool;

impl Tool for TodoTool {
    fn name(&self) -> &str { "todo_write" }
    fn description(&self) -> &str { "Manage a task/todo list" }
    fn needs_permission(&self) -> bool { false }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string"},"status":{"type":"string"},"priority":{"type":"string"}}}}},"required":["todos"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let todos = obj.get("todos").and_then(|v| v.as_array()).ok_or("todos required")?;

        let path = PathBuf::from(".cairn/todos.json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }

        let pretty = crate::json::serialize(&val);
        fs::write(&path, &pretty).map_err(|e| format!("write: {e}"))?;
        Ok(format!("Saved {} todo item(s) to .cairn/todos.json", todos.len()))
    }
}
