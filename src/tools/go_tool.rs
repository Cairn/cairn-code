use super::process_runner::{self, with_cleanup, RunError, RunOptions};
use super::registry::Tool;
use super::workspace::Workspace;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Generous wall-clock cap so a hung `go` invocation cannot block the agent
/// forever. Long builds/tests normally finish well within this; the user can
/// also cancel sooner.
const GO_TIMEOUT: Duration = Duration::from_secs(600);
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 4_000;

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
        self.execute_with_cancel(input, &AtomicBool::new(false))
    }

    fn execute_with_cancel(&self, input: &str, cancel: &AtomicBool) -> Result<String, String> {
        let args = super::parse_command_args(input)?;

        let mut command = Command::new("go");
        command.args(&args).current_dir(self.workspace.root());

        let options = RunOptions {
            timeout: Some(GO_TIMEOUT),
            head_chars: HEAD_CHARS,
            tail_chars: TAIL_CHARS,
        };
        let result = match process_runner::run(command, &options, Some(cancel)) {
            Ok(result) => result,
            Err(error) => return Err(format_run_error(error)),
        };

        let body =
            super::bounded_command_output(result.stdout.as_bytes(), result.stderr.as_bytes());

        if !result.success {
            let mut error = format!("go exited with {}", result.code);
            if !body.is_empty() {
                error.push('\n');
                error.push_str(&body);
            }
            return Err(error);
        }

        Ok(body)
    }
}

fn format_run_error(error: RunError) -> String {
    match error {
        RunError::Spawn(message) => format!("go exec error: {message}"),
        RunError::TimedOut {
            after_ms,
            cleanup_error,
        } => with_cleanup(
            format!("go command timed out after {after_ms}ms"),
            &cleanup_error,
        ),
        RunError::Cancelled { cleanup_error } => {
            with_cleanup("go command cancelled".to_string(), &cleanup_error)
        }
        RunError::Wait {
            reason,
            cleanup_error,
        } => with_cleanup(format!("go exec error: {reason}"), &cleanup_error),
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
