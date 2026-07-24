use super::registry::Tool;
use super::workspace::Workspace;
use std::process::Command;

pub struct GoTool {
    workspace: Workspace,
}

impl GoTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for GoTool {
    fn name(&self) -> &str {
        "go"
    }
    fn description(&self) -> &str {
        "Execute Go commands in the workspace (build, test, vet, etc.)"
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"args":{"type":"array","items":{"type":"string"},"description":"Go arguments, one string per argument (no shell parsing)"}},"required":["args"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let args = super::parse_command_args(input)?;

        let output = Command::new("go")
            .args(&args)
            .current_dir(self.workspace.root())
            .output()
            .map_err(|e| format!("go exec error: {e}"))?;

        let result = super::bounded_command_output(&output.stdout, &output.stderr);

        if !output.status.success() {
            let mut error = format!("go exited with {}", output.status.code().unwrap_or(-1));
            if !result.is_empty() {
                error.push('\n');
                error.push_str(&result);
            }
            return Err(error);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> GoTool {
        GoTool::new(Workspace::current().unwrap())
    }

    #[test]
    fn requires_args() {
        assert!(tool().execute("{}").is_err());
        assert!(tool().execute(r#"{"args":"version"}"#).is_err());
        assert!(tool().execute(r#"{"args":["env",false]}"#).is_err());
    }

    #[test]
    fn identity() {
        let t = tool();
        assert_eq!(t.name(), "go");
        assert!(t.needs_permission());
        let schema = crate::json::parse(&t.input_schema()).unwrap();
        let args = schema
            .get("properties")
            .and_then(|value| value.get("args"))
            .unwrap();
        assert_eq!(
            args.get("type").and_then(|value| value.as_str()),
            Some("array")
        );
        assert_eq!(
            args.get("items")
                .and_then(|value| value.get("type"))
                .and_then(|value| value.as_str()),
            Some("string")
        );
    }

    #[test]
    fn version_or_missing_binary() {
        match tool().execute(r#"{"args":["version"]}"#) {
            Ok(out) => assert!(out.to_ascii_lowercase().contains("go"), "{out}"),
            Err(e) => assert!(
                e.contains("go exec") || e.contains("go exited"),
                "unexpected: {e}"
            ),
        }
    }
}
