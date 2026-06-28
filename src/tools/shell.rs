use std::process::Command;
use super::registry::Tool;

pub struct ShellTool;

impl Tool for ShellTool {
    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str { "Execute a shell command" }
    fn needs_permission(&self) -> bool { true }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"}},"required":["command"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let cmd = obj.get("command").and_then(|v| v.as_str()).ok_or("command required")?;

        let shell = if cfg!(windows) { "powershell" } else { "bash" };
        let flag = if cfg!(windows) { "-Command" } else { "-c" };

        let output = Command::new(shell)
            .arg(flag)
            .arg(cmd)
            .output()
            .map_err(|e| format!("exec error: {e}"))?;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() { result.push('\n'); }
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if !output.status.success() && output.stdout.is_empty() {
            return Err(format!("exit code: {}", output.status.code().unwrap_or(-1)));
        }

        if result.len() > 5000 {
            result = result[..5000].to_string();
            result.push_str("\n... [truncated]");
        }

        Ok(result)
    }
}
