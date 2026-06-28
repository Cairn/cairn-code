use crate::llm::ToolDefinition;

pub trait Tool: Send {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> String;
    #[allow(dead_code)]
    fn needs_permission(&self) -> bool;
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
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
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
}

pub fn default_registry() -> Registry {
    let mut r = Registry::new();
    r.register(Box::new(crate::tools::file_read::FileReadTool));
    r.register(Box::new(crate::tools::file_write::FileWriteTool));
    r.register(Box::new(crate::tools::file_edit::FileEditTool));
    r.register(Box::new(crate::tools::shell::ShellTool));
    r.register(Box::new(crate::tools::go_tool::GoTool));
    r.register(Box::new(crate::tools::git_tool::GitTool));
    r.register(Box::new(crate::tools::glob_tool::GlobTool));
    r.register(Box::new(crate::tools::grep_tool::GrepTool));
    r.register(Box::new(crate::tools::web_search::WebSearchTool));
    r.register(Box::new(crate::tools::web_fetch::WebFetchTool));
    r.register(Box::new(crate::tools::todo::TodoTool));
    r.register(Box::new(crate::tools::memory::MemoryTool));
    r
}
