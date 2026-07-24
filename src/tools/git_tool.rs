use super::registry::Tool;
use std::process::Command;

pub struct GitTool;

impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }
    fn description(&self) -> &str {
        "Execute Git commands"
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"args":{"type":"string"}},"required":["args"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let args_str = obj
            .get("args")
            .and_then(|v| v.as_str())
            .ok_or("args required")?;

        let args: Vec<&str> = args_str.split_whitespace().collect();

        let output = Command::new("git")
            .args(&args)
            .output()
            .map_err(|e| format!("git exec error: {e}"))?;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if !output.status.success() {
            return Err(format!(
                "git exited with {}",
                output.status.code().unwrap_or(-1)
            ));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_args() {
        assert!(GitTool.execute("{}").is_err());
        assert!(GitTool.execute("not-json").is_err());
    }

    #[test]
    fn identity() {
        let t = GitTool;
        assert_eq!(t.name(), "git");
        assert!(t.needs_permission());
        assert!(crate::json::parse(&t.input_schema()).is_ok());
    }

    #[test]
    fn rev_parse_works_in_repo() {
        // Only assert success when cwd is a git repo (this project's tree).
        let tool = GitTool;
        match tool.execute(r#"{"args":"rev-parse --is-inside-work-tree"}"#) {
            Ok(out) => assert!(out.trim().contains("true") || out.contains("true"), "{out}"),
            Err(e) => {
                // Allow missing git binary in restricted CI.
                assert!(
                    e.contains("git exec") || e.contains("git exited"),
                    "unexpected error: {e}"
                );
            }
        }
    }
}
