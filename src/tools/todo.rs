use super::registry::Tool;
use std::fs;
use std::path::PathBuf;

pub struct TodoTool;

impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo_write"
    }
    fn description(&self) -> &str {
        "Manage a task/todo list"
    }
    fn needs_permission(&self) -> bool {
        false
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string"},"status":{"type":"string"},"priority":{"type":"string"}}}}},"required":["todos"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let todos = obj
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or("todos required")?;

        let path = PathBuf::from(".cairn/todos.json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }

        let pretty = crate::json::serialize(&val);
        fs::write(&path, &pretty).map_err(|e| format!("write: {e}"))?;
        Ok(format!(
            "Saved {} todo item(s) to .cairn/todos.json",
            todos.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_todos_file() {
        // TodoTool always writes relative `.cairn/todos.json` under the process
        // cwd. Use a unique nested path check by running from a temp workspace
        // only after snapshotting and carefully restoring cwd (serialized step).
        // Prefer not racing other tests: write then verify under cwd, then clean up.
        let tool = TodoTool;
        let marker = format!(
            "ship-it-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let input = format!(
            r#"{{"todos":[{{"content":"{marker}","status":"pending","priority":"high"}}]}}"#
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("1 todo"), "{out}");
        let raw = fs::read_to_string(".cairn/todos.json").unwrap();
        assert!(raw.contains(&marker), "{raw}");
        // Leave the file; later runs overwrite. Avoid set_current_dir (races workspace tests).
    }

    #[test]
    fn requires_todos_array() {
        assert!(TodoTool.execute("{}").is_err());
        assert!(TodoTool.execute(r#"{"todos":"nope"}"#).is_err());
    }
}
