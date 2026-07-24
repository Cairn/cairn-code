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

/// GitHub co-author trailer for commits created through Cairn Code.
/// Format matches Git / GitHub `Co-authored-by` (case-insensitive).
pub(crate) const CO_AUTHOR_TRAILER: &str =
    "Co-Authored-By: cairn-code <282421612+cairn-code@users.noreply.github.com>";

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
         config-defined commands, so treat it as shell-equivalent execution. \
         Commits with a message automatically get a Co-Authored-By: cairn-code trailer."
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
        let args = with_co_author_trailer(super::parse_command_args(input)?);

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

/// Append the Cairn Code co-author trailer to `git commit` when a message is
/// provided (`-m` / `--message`). Skips if the trailer is already present, or
/// when there is no explicit message (editor / `--no-edit` amend).
fn with_co_author_trailer(mut args: Vec<String>) -> Vec<String> {
    let Some(commit_idx) = commit_subcommand_index(&args) else {
        return args;
    };
    let commit_args = &args[commit_idx + 1..];
    if commit_message_args_contain_co_author(commit_args) {
        return args;
    }
    if !commit_has_explicit_message(commit_args) {
        return args;
    }
    // Extra `-m` becomes another paragraph in the commit message, which is the
    // usual place for Git trailers (blank line separator is added by git).
    args.push("-m".into());
    args.push(CO_AUTHOR_TRAILER.into());
    args
}

/// Index of the `commit` subcommand after optional `git` global options.
fn commit_subcommand_index(args: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "commit" {
            return Some(i);
        }
        // First non-option is the subcommand (after optional globals).
        if !arg.starts_with('-') {
            return None;
        }
        // Global options that take a separate value.
        if matches!(
            arg,
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
        ) {
            i = i.saturating_add(2);
            continue;
        }
        // Combined form: --git-dir=..., -c core.foo=bar, etc.
        i += 1;
    }
    None
}

fn commit_has_explicit_message(commit_args: &[String]) -> bool {
    let mut i = 0;
    while i < commit_args.len() {
        let arg = commit_args[i].as_str();
        match arg {
            "-m" | "--message" | "-F" | "--file" | "-C" | "--reuse-message" | "-c"
            | "--reedit-message" => return true,
            s if s.starts_with("--message=")
                || s.starts_with("--file=")
                || s.starts_with("--reuse-message=")
                || s.starts_with("--reedit-message=") =>
            {
                return true;
            }
            _ => i += 1,
        }
    }
    false
}

fn commit_message_args_contain_co_author(commit_args: &[String]) -> bool {
    let mut i = 0;
    while i < commit_args.len() {
        let arg = commit_args[i].as_str();
        if arg == "-m" || arg == "--message" {
            if let Some(msg) = commit_args.get(i + 1) {
                if message_has_cairn_co_author(msg) {
                    return true;
                }
            }
            i = i.saturating_add(2);
            continue;
        }
        if let Some(msg) = arg.strip_prefix("--message=") {
            if message_has_cairn_co_author(msg) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn message_has_cairn_co_author(message: &str) -> bool {
    message.lines().any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        lower.starts_with("co-authored-by:") && lower.contains("cairn-code")
    })
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
        assert!(t.description().contains("Co-Authored-By"));
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

    #[test]
    fn injects_co_author_on_commit_with_message() {
        let args = with_co_author_trailer(vec![
            "commit".to_string(),
            "-m".to_string(),
            "fix: something".to_string(),
        ]);
        assert_eq!(
            args,
            vec![
                "commit".to_string(),
                "-m".to_string(),
                "fix: something".to_string(),
                "-m".to_string(),
                CO_AUTHOR_TRAILER.to_string(),
            ]
        );
    }

    #[test]
    fn injects_co_author_after_global_git_options() {
        let args = with_co_author_trailer(vec![
            "-C".to_string(),
            "/tmp/repo".to_string(),
            "commit".to_string(),
            "--message".to_string(),
            "chore: note".to_string(),
        ]);
        assert_eq!(args[args.len() - 2], "-m");
        assert_eq!(args[args.len() - 1], CO_AUTHOR_TRAILER);
    }

    #[test]
    fn does_not_duplicate_existing_cairn_co_author() {
        let message = format!("subject\n\n{CO_AUTHOR_TRAILER}");
        let args = with_co_author_trailer(vec![
            "commit".to_string(),
            "-m".to_string(),
            message.clone(),
        ]);
        // No extra `-m` trailer arg when the message already co-authors cairn-code.
        assert_eq!(
            args,
            vec!["commit".to_string(), "-m".to_string(), message.clone()]
        );
        assert!(message_has_cairn_co_author(&message));
        assert_eq!(args.iter().filter(|a| *a == CO_AUTHOR_TRAILER).count(), 0);
    }

    #[test]
    fn leaves_non_commit_commands_alone() {
        let original = vec!["status".to_string(), "-sb".to_string()];
        assert_eq!(with_co_author_trailer(original.clone()), original);
    }

    #[test]
    fn leaves_commit_without_message_alone() {
        // Editor / --no-edit amend paths: do not invent a message of only the trailer.
        let original = vec![
            "commit".to_string(),
            "--amend".to_string(),
            "--no-edit".to_string(),
        ];
        assert_eq!(with_co_author_trailer(original.clone()), original);
    }

    #[test]
    fn global_c_path_alone_does_not_count_as_commit_message() {
        // `git -C /repo commit --no-edit` must not treat global -C as --reuse-message.
        let original = vec![
            "-C".to_string(),
            "/tmp/repo".to_string(),
            "commit".to_string(),
            "--amend".to_string(),
            "--no-edit".to_string(),
        ];
        assert_eq!(with_co_author_trailer(original.clone()), original);
    }

    #[test]
    fn recognizes_equals_message_form() {
        let args =
            with_co_author_trailer(vec!["commit".to_string(), "--message=hello".to_string()]);
        assert_eq!(
            args,
            vec![
                "commit".to_string(),
                "--message=hello".to_string(),
                "-m".to_string(),
                CO_AUTHOR_TRAILER.to_string(),
            ]
        );
    }

    #[test]
    fn commit_with_file_message_still_gets_trailer() {
        let args = with_co_author_trailer(vec![
            "commit".to_string(),
            "-F".to_string(),
            "msg.txt".to_string(),
        ]);
        assert_eq!(
            args,
            vec![
                "commit".to_string(),
                "-F".to_string(),
                "msg.txt".to_string(),
                "-m".to_string(),
                CO_AUTHOR_TRAILER.to_string(),
            ]
        );
    }
}
