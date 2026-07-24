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
/// Passed via `git commit --trailer` so it works with `-m`, `-F`, `-C`, etc.
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
         Commits automatically get a Co-Authored-By: cairn-code trailer via git --trailer."
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
            stdin: None,
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

/// Append `git commit --trailer <CO_AUTHOR_TRAILER>` for commit invocations.
///
/// Using `--trailer` (git 2.32+) is required: message sources `-m`, `-F`, `-C`,
/// and `-c` are mutually exclusive, so appending another `-m` breaks `-F`/`-C`/`-c`.
/// `--trailer` works with all of them and deduplicates via git's trailer rules.
fn with_co_author_trailer(mut args: Vec<String>) -> Vec<String> {
    if commit_subcommand_index(&args).is_none() {
        return args;
    }
    if already_has_cairn_trailer_flag(&args) {
        return args;
    }
    args.push("--trailer".into());
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
        i += 1;
    }
    None
}

fn already_has_cairn_trailer_flag(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--trailer" {
            if let Some(value) = args.get(i + 1) {
                if trailer_is_cairn_co_author(value) {
                    return true;
                }
            }
            i = i.saturating_add(2);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--trailer=") {
            if trailer_is_cairn_co_author(value) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn trailer_is_cairn_co_author(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.contains("co-authored-by:") && lower.contains("cairn-code")
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
                assert!(
                    e.contains("git exec") || e.contains("git exited"),
                    "unexpected error: {e}"
                );
            }
        }
    }

    #[test]
    fn injects_trailer_on_commit_with_message() {
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
                "--trailer".to_string(),
                CO_AUTHOR_TRAILER.to_string(),
            ]
        );
    }

    #[test]
    fn injects_trailer_after_global_git_options() {
        let args = with_co_author_trailer(vec![
            "-C".to_string(),
            "/tmp/repo".to_string(),
            "commit".to_string(),
            "--message".to_string(),
            "chore: note".to_string(),
        ]);
        assert_eq!(args[args.len() - 2], "--trailer");
        assert_eq!(args[args.len() - 1], CO_AUTHOR_TRAILER);
    }

    #[test]
    fn injects_trailer_with_file_message_without_extra_m() {
        // Must not append -m (git rejects -m together with -F).
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
                "--trailer".to_string(),
                CO_AUTHOR_TRAILER.to_string(),
            ]
        );
        assert!(!args.iter().any(|a| a == "-m"));
    }

    #[test]
    fn injects_trailer_with_reuse_message() {
        let args = with_co_author_trailer(vec![
            "commit".to_string(),
            "-C".to_string(),
            "HEAD".to_string(),
        ]);
        assert_eq!(args[args.len() - 2], "--trailer");
        assert_eq!(args[args.len() - 1], CO_AUTHOR_TRAILER);
        assert!(!args.iter().any(|a| a == "-m"));
    }

    #[test]
    fn does_not_duplicate_existing_trailer_flag() {
        let args = with_co_author_trailer(vec![
            "commit".to_string(),
            "-m".to_string(),
            "subject".to_string(),
            "--trailer".to_string(),
            CO_AUTHOR_TRAILER.to_string(),
        ]);
        assert_eq!(args.iter().filter(|a| a.as_str() == "--trailer").count(), 1);
    }

    #[test]
    fn leaves_non_commit_commands_alone() {
        let original = vec!["status".to_string(), "-sb".to_string()];
        assert_eq!(with_co_author_trailer(original.clone()), original);
    }

    #[test]
    fn real_commit_f_with_trailer_succeeds() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cairn-git-trailer-{nanos}"));
        std::fs::create_dir_all(&workspace).unwrap();
        let tool = GitTool::new(Workspace::new(&workspace).unwrap());

        tool.execute(r#"{"args":["init","--quiet"]}"#).unwrap();
        // Identity required for commit in empty temp repos.
        let _ = tool.execute(r#"{"args":["config","user.email","test@example.com"]}"#);
        let _ = tool.execute(r#"{"args":["config","user.name","test"]}"#);
        std::fs::write(workspace.join("a.txt"), "a\n").unwrap();
        tool.execute(r#"{"args":["add","a.txt"]}"#).unwrap();
        std::fs::write(workspace.join("msg.txt"), "feat: from file\n").unwrap();
        tool.execute(r#"{"args":["commit","-F","msg.txt"]}"#)
            .unwrap();
        let log = tool
            .execute(r#"{"args":["log","-1","--format=%B"]}"#)
            .unwrap();
        assert!(
            log.to_ascii_lowercase().contains("co-authored-by:") && log.contains("cairn-code"),
            "expected co-author trailer in commit message, got: {log}"
        );
        assert!(log.contains("feat: from file"), "{log}");

        drop(tool);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn real_commit_m_with_trailer_succeeds() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cairn-git-trailer-m-{nanos}"));
        std::fs::create_dir_all(&workspace).unwrap();
        let tool = GitTool::new(Workspace::new(&workspace).unwrap());

        tool.execute(r#"{"args":["init","--quiet"]}"#).unwrap();
        let _ = tool.execute(r#"{"args":["config","user.email","test@example.com"]}"#);
        let _ = tool.execute(r#"{"args":["config","user.name","test"]}"#);
        std::fs::write(workspace.join("b.txt"), "b\n").unwrap();
        tool.execute(r#"{"args":["add","b.txt"]}"#).unwrap();
        tool.execute(r#"{"args":["commit","-m","fix: via -m"]}"#)
            .unwrap();
        let log = tool
            .execute(r#"{"args":["log","-1","--format=%B"]}"#)
            .unwrap();
        assert!(log.contains("fix: via -m"), "{log}");
        assert!(
            log.to_ascii_lowercase().contains("co-authored-by:") && log.contains("cairn-code"),
            "{log}"
        );

        drop(tool);
        let _ = std::fs::remove_dir_all(workspace);
    }
}
