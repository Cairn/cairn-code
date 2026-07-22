use std::process::Command;
use super::registry::Tool;

pub struct GoTool;

impl Tool for GoTool {
    fn name(&self) -> &str { "go" }
    fn description(&self) -> &str { "Execute Go commands (build, test, vet, etc.)" }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"args":{"type":"string"}},"required":["args"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let args_str = obj.get("args").and_then(|v| v.as_str()).ok_or("args required")?;

        let args: Vec<&str> = args_str.split_whitespace().collect();

        let output = Command::new("go")
            .args(&args)
            .output()
            .map_err(|e| format!("go exec error: {e}"))?;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() { result.push('\n'); }
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if !output.status.success() {
            return Err(format!("go exited with {}", output.status.code().unwrap_or(-1)));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_args() {
        assert!(GoTool.execute("{}").is_err());
    }

    #[test]
    fn identity() {
        let t = GoTool;
        assert_eq!(t.name(), "go");
        assert!(t.needs_permission());
        assert!(crate::json::parse(&t.input_schema()).is_ok());
    }

    #[test]
    fn version_or_missing_binary() {
        match GoTool.execute(r#"{"args":"version"}"#) {
            Ok(out) => assert!(out.to_ascii_lowercase().contains("go"), "{out}"),
            Err(e) => assert!(
                e.contains("go exec") || e.contains("go exited"),
                "unexpected: {e}"
            ),
        }
    }
}
