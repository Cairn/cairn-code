use super::process_runner::{self, with_cleanup, RunError, RunOptions};
use super::registry::Tool;
use super::workspace::Workspace;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Generous wall-clock cap so a hung or runaway `git` (e.g. a stuck network
/// fetch or a credential prompt) cannot block the agent forever. Normal
/// commands finish well within this; the user can also cancel sooner.
const GIT_TIMEOUT: Duration = Duration::from_secs(600);
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 4_000;

pub struct GitTool {
    workspace: Workspace,
}

impl GitTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }
    fn description(&self) -> &str {
        "Execute Git commands in the workspace. Git can invoke aliases, hooks, helpers, and \
         config-defined commands, so treat it as shell-equivalent execution."
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"args":{"type":"array","items":{"type":"string"},"description":"Git arguments, one string per argument (no shell parsing)"}},"required":["args"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        self.execute_with_cancel(input, &AtomicBool::new(false))
    }

    fn execute_with_cancel(&self, input: &str, cancel: &AtomicBool) -> Result<String, String> {
        let args = super::parse_command_args(input)?;

        let mut command = Command::new("git");
        command.args(&args).current_dir(self.workspace.root());

        let options = RunOptions {
            timeout: Some(GIT_TIMEOUT),
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
            let mut error = format!("git exited with {}", result.code);
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
        RunError::Spawn(message) => format!("git exec error: {message}"),
        RunError::TimedOut {
            after_ms,
            cleanup_error,
        } => with_cleanup(
            format!("git command timed out after {after_ms}ms"),
            &cleanup_error,
        ),
        RunError::Cancelled { cleanup_error } => {
            with_cleanup("git command cancelled".to_string(), &cleanup_error)
        }
        RunError::Wait {
            reason,
            cleanup_error,
        } => with_cleanup(format!("git exec error: {reason}"), &cleanup_error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> GitTool {
        GitTool::new(Workspace::current().unwrap())
    }

    #[test]
    fn requires_args() {
        assert!(tool().execute("{}").is_err());
        assert!(tool().execute("not-json").is_err());
        assert!(tool().execute(r#"{"args":"status --short"}"#).is_err());
        assert!(tool().execute(r#"{"args":["status",1]}"#).is_err());
    }

    #[test]
    fn identity() {
        let t = tool();
        assert_eq!(t.name(), "git");
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
        assert!(t.description().contains("shell-equivalent"));
    }

    #[test]
    fn preserves_spaces_and_empty_arguments() {
        let out = tool()
            .execute(r#"{"args":["rev-parse","--sq-quote","path with spaces",""]}"#)
            .unwrap();
        assert_eq!(out.trim(), "'path with spaces' ''");
    }

    #[test]
    fn executes_from_the_configured_workspace() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cairn git workspace {nanos}"));
        std::fs::create_dir_all(&workspace).unwrap();
        let tool = GitTool::new(Workspace::new(&workspace).unwrap());

        tool.execute(r#"{"args":["init","--quiet"]}"#).unwrap();
        assert!(workspace.join(".git").is_dir());

        drop(tool);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn rev_parse_works_in_repo() {
        match tool().execute(r#"{"args":["rev-parse","--is-inside-work-tree"]}"#) {
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
