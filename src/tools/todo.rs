use super::registry::Tool;
use super::workspace::{self, Workspace};

pub struct TodoTool {
    workspace: Workspace,
}

impl TodoTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

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

        let pretty = crate::json::serialize(&val);
        let path = self
            .workspace
            .relative_path(".cairn/todos.json")?
            .to_string_lossy()
            .into_owned();
        let target = workspace::acquire_in(&self.workspace, &path, true, false)?;
        workspace::atomic_replace(&target, &pretty)?;
        Ok(format!(
            "Saved {} todo item(s) to .cairn/todos.json",
            todos.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn writes_todos_file() {
        let root = tempfile::tempdir().unwrap();
        let tool = TodoTool::new(Workspace::new(root.path()).unwrap());
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
        let raw = fs::read_to_string(root.path().join(".cairn/todos.json")).unwrap();
        assert!(raw.contains(&marker), "{raw}");
    }

    #[test]
    fn requires_todos_array() {
        let root = tempfile::tempdir().unwrap();
        let tool = TodoTool::new(Workspace::new(root.path()).unwrap());
        assert!(tool.execute("{}").is_err());
        assert!(tool.execute(r#"{"todos":"nope"}"#).is_err());
    }
}
