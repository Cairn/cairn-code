use crate::llm::ToolDefinition;

pub trait Tool: Send {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> String;
    fn needs_permission(&self) -> bool;
    fn needs_permission_for(&self, _input: &str) -> bool {
        self.needs_permission()
    }
    fn permission_key(&self, _input: &str) -> String {
        self.name().to_string()
    }
    fn execute(&self, input: &str) -> Result<String, String>;
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.input_schema(),
        }
    }
}

pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    #[allow(dead_code)]
    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

pub fn default_registry() -> Registry {
    let mut r = Registry::new();
    let workspace = crate::tools::workspace::Workspace::current()
        .expect("current directory must be a readable workspace");
    r.register(Box::new(crate::tools::file_read::FileReadTool::new(
        workspace.clone(),
    )));
    r.register(Box::new(crate::tools::file_write::FileWriteTool::new(
        workspace.clone(),
    )));
    r.register(Box::new(crate::tools::file_edit::FileEditTool::new(
        workspace.clone(),
    )));
    r.register(Box::new(crate::tools::file_undo::FileUndoTool));
    r.register(Box::new(crate::tools::shell::ShellTool));
    r.register(Box::new(crate::tools::powershell_tool::PowerShellTool));
    r.register(Box::new(crate::tools::python_tool::PythonTool));
    r.register(Box::new(crate::tools::go_tool::GoTool));
    r.register(Box::new(crate::tools::git_tool::GitTool));
    r.register(Box::new(crate::tools::glob_tool::GlobTool::new(
        workspace.clone(),
    )));
    r.register(Box::new(crate::tools::grep_tool::GrepTool::new(workspace)));
    r.register(Box::new(crate::tools::web_search::WebSearchTool));
    r.register(Box::new(crate::tools::web_fetch::WebFetchTool));
    r.register(Box::new(crate::tools::todo::TodoTool));
    r.register(Box::new(crate::tools::memory::MemoryTool));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_has_expected_tools() {
        let r = default_registry();
        assert_eq!(r.len(), 15);
        for name in [
            "file_read",
            "file_write",
            "file_edit",
            "file_undo",
            "shell",
            "powershell",
            "python",
            "go",
            "git",
            "glob",
            "grep",
            "web_search",
            "web_fetch",
            "todo_write",
            "memory",
        ] {
            assert!(r.get(name).is_some(), "missing tool {name}");
        }
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn definitions_are_valid_json_schemas() {
        let r = default_registry();
        for def in r.definitions() {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
            let parsed = crate::json::parse(&def.input_schema);
            assert!(parsed.is_ok(), "{} schema: {:?}", def.name, parsed.err());
        }
    }

    #[test]
    fn permission_flags_match_expectations() {
        let r = default_registry();
        assert!(!r.get("file_read").unwrap().needs_permission());
        assert!(!r.get("glob").unwrap().needs_permission());
        assert!(!r.get("grep").unwrap().needs_permission());
        assert!(r.get("shell").unwrap().needs_permission());
        assert!(r.get("powershell").unwrap().needs_permission());
        assert!(r.get("python").unwrap().needs_permission());
        assert!(r.get("file_write").unwrap().needs_permission());
        assert!(r.get("git").unwrap().needs_permission());
        assert!(r.get("web_fetch").unwrap().needs_permission());
    }

    #[test]
    fn empty_registry() {
        let r = Registry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.names().is_empty());
    }
}
