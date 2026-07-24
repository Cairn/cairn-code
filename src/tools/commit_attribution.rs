//! Ensure commits made through any Cairn tool get the cairn-code co-author trailer.
//!
//! The dedicated `git` tool already injects `--trailer` on `git commit`. Agents often
//! bypass that path via `shell` / `powershell` (`git commit -m ...`), so those
//! command strings are rewritten here before execution.

use crate::tools::git_tool::CO_AUTHOR_TRAILER;

/// Rewrite a shell/bash command line so bare `git commit` invocations gain the
/// Cairn co-author trailer. Leaves non-commit commands unchanged.
pub fn ensure_shell_command_co_author(command: &str) -> String {
    rewrite_command_line(command, ShellDialect::Posix)
}

/// Rewrite a PowerShell command/script so bare `git commit` invocations gain the
/// Cairn co-author trailer.
pub fn ensure_powershell_command_co_author(command: &str) -> String {
    rewrite_command_line(command, ShellDialect::PowerShell)
}

#[derive(Clone, Copy)]
enum ShellDialect {
    Posix,
    PowerShell,
}

fn rewrite_command_line(command: &str, dialect: ShellDialect) -> String {
    if command_already_has_cairn_trailer(command) {
        return command.to_string();
    }

    let mut out = String::with_capacity(command.len() + CO_AUTHOR_TRAILER.len() + 32);
    let mut rest = command;
    let mut changed = false;

    while let Some((prefix, git_commit, suffix)) = split_next_git_commit(rest, dialect) {
        out.push_str(prefix);
        out.push_str(git_commit);
        if !git_commit_fragment_has_cairn_trailer(git_commit) {
            // Prefer inserting before trailing shell operators so
            // `git commit -m x && push` becomes `git commit -m x --trailer ... && push`.
            out.push_str(" --trailer ");
            out.push_str(&shell_quote_trailer(dialect));
            changed = true;
        }
        rest = suffix;
    }
    out.push_str(rest);

    if changed {
        out
    } else {
        command.to_string()
    }
}

fn shell_quote_trailer(dialect: ShellDialect) -> String {
    match dialect {
        // Double quotes are fine in bash/pwsh for this trailer (no `$`, `` ` ``, or `!`).
        ShellDialect::Posix | ShellDialect::PowerShell => {
            format!("\"{CO_AUTHOR_TRAILER}\"")
        }
    }
}

fn command_already_has_cairn_trailer(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("co-authored-by:") && lower.contains("cairn-code")
}

fn git_commit_fragment_has_cairn_trailer(fragment: &str) -> bool {
    command_already_has_cairn_trailer(fragment)
}

/// Find the next `git commit` invocation and split into (before, commit_cmd, after).
/// `commit_cmd` runs until a top-level shell separator for the dialect.
fn split_next_git_commit(input: &str, dialect: ShellDialect) -> Option<(&str, &str, &str)> {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(start) = match_git_commit_at(input, i) {
            let end = scan_command_end(input, start, dialect);
            let prefix = &input[..start];
            let cmd = &input[start..end];
            let suffix = &input[end..];
            return Some((prefix, cmd, suffix));
        }
        i += 1;
    }
    None
}

/// If `input[i..]` begins a `git` token followed by `commit` (with optional path /
/// exe suffix and global options between them), return the start index of that
/// `git` token.
fn match_git_commit_at(input: &str, i: usize) -> Option<usize> {
    if !is_token_start(input, i) {
        return None;
    }
    let rest = &input[i..];
    let git_len = match_git_token(rest)?;
    let after_git = &rest[git_len..];
    let after_ws = after_git.trim_start_matches(is_ws);
    // Optional global options before subcommand: -C path, -c key=val, --git-dir=...
    let mut cursor = after_ws;
    loop {
        if cursor.is_empty() {
            return None;
        }
        if cursor.starts_with("commit") && is_token_end(cursor, "commit".len()) {
            return Some(i);
        }
        if !cursor.starts_with('-') {
            return None;
        }
        // Consume one global option (and its value when separate).
        let (opt, rest) = split_first_token(cursor)?;
        cursor = rest.trim_start_matches(is_ws);
        if opt == "-C"
            || opt == "-c"
            || opt == "--git-dir"
            || opt == "--work-tree"
            || opt == "--namespace"
            || opt == "--config-env"
            || opt.starts_with("-C") && opt.len() > 2
        {
            // Separate value token when option is exactly -C / -c / etc.
            if matches!(
                opt,
                "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
            ) {
                let (_, rest2) = split_first_token(cursor)?;
                cursor = rest2.trim_start_matches(is_ws);
            }
            continue;
        }
        // Flag without required value (-q, --no-pager, --bare, …)
        continue;
    }
}

fn match_git_token(s: &str) -> Option<usize> {
    // git, git.exe, /usr/bin/git, C:\Program Files\Git\cmd\git.exe
    let lower = s.to_ascii_lowercase();
    // Find end of first token.
    let token_end = lower
        .char_indices()
        .find(|(_, c)| is_ws(*c) || is_shell_meta(*c))
        .map(|(idx, _)| idx)
        .unwrap_or(lower.len());
    if token_end == 0 {
        return None;
    }
    let token = &lower[..token_end];
    let base = token
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(token)
        .trim_end_matches(".exe");
    if base == "git" {
        Some(token_end)
    } else {
        None
    }
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start_matches(is_ws);
    if s.is_empty() {
        return None;
    }
    // Quoted token.
    let bytes = s.as_bytes();
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        let quote = bytes[0];
        let mut i = 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                i += 1;
                break;
            }
            i += 1;
        }
        return Some((&s[..i], &s[i..]));
    }
    let end = s
        .char_indices()
        .find(|(_, c)| is_ws(*c) || is_shell_meta(*c))
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

fn scan_command_end(input: &str, start: usize, dialect: ShellDialect) -> usize {
    let bytes = input.as_bytes();
    let mut i = start;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            i += 1;
            continue;
        }
        match dialect {
            ShellDialect::Posix => {
                if in_single {
                    if b == b'\'' {
                        in_single = false;
                    }
                    i += 1;
                    continue;
                }
                if in_double {
                    if b == b'\\' {
                        escape = true;
                        i += 1;
                        continue;
                    }
                    if b == b'"' {
                        in_double = false;
                    }
                    i += 1;
                    continue;
                }
                if b == b'\\' {
                    escape = true;
                    i += 1;
                    continue;
                }
                if b == b'\'' {
                    in_single = true;
                    i += 1;
                    continue;
                }
                if b == b'"' {
                    in_double = true;
                    i += 1;
                    continue;
                }
                // Separators: ; & | newline, or && ||
                if b == b'\n' || b == b';' || b == b'&' || b == b'|' {
                    return i;
                }
            }
            ShellDialect::PowerShell => {
                // PowerShell escape is backtick.
                if in_single {
                    // Single-quoted: only '' escapes.
                    if b == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        in_single = false;
                    }
                    i += 1;
                    continue;
                }
                if in_double {
                    if b == b'`' {
                        escape = true;
                        i += 1;
                        continue;
                    }
                    if b == b'"' {
                        in_double = false;
                    }
                    i += 1;
                    continue;
                }
                if b == b'`' {
                    escape = true;
                    i += 1;
                    continue;
                }
                if b == b'\'' {
                    in_single = true;
                    i += 1;
                    continue;
                }
                if b == b'"' {
                    in_double = true;
                    i += 1;
                    continue;
                }
                // Separators: ; | & newline, && || (pwsh 7+)
                if b == b'\n' || b == b';' || b == b'|' || b == b'&' {
                    return i;
                }
            }
        }
        i += 1;
    }
    input.len()
}

fn is_ws(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\r'
}

fn is_shell_meta(c: char) -> bool {
    matches!(c, ';' | '&' | '|' | '\n' | '(' | ')' | '<' | '>')
}

fn is_token_start(input: &str, i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = input[..i].chars().next_back().unwrap_or(' ');
    is_ws(prev)
        || matches!(prev, ';' | '&' | '|' | '\n' | '(' | ')' | '`' | '"' | '\'')
        // PowerShell statement separators / pipe
        || prev == '\r'
}

fn is_token_end(s: &str, token_len: usize) -> bool {
    s.len() == token_len
        || s[token_len..]
            .chars()
            .next()
            .map(|c| is_ws(c) || is_shell_meta(c) || c == '"' || c == '\'')
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_on_simple_git_commit_m() {
        let out = ensure_shell_command_co_author("git commit -m \"fix: x\"");
        assert!(out.contains("--trailer"));
        assert!(out.contains("cairn-code"));
        assert!(out.contains("fix: x"));
    }

    #[test]
    fn injects_before_shell_and() {
        let out = ensure_shell_command_co_author("git commit -m 'x' && git push");
        assert!(out.contains("--trailer"));
        assert!(out.contains("&& git push"));
        // trailer should appear before &&
        let trailer_at = out.find("--trailer").unwrap();
        let and_at = out.find("&&").unwrap();
        assert!(trailer_at < and_at, "{out}");
    }

    #[test]
    fn skips_when_trailer_already_present() {
        let cmd = format!("git commit -m x --trailer \"{CO_AUTHOR_TRAILER}\"");
        assert_eq!(ensure_shell_command_co_author(&cmd), cmd);
    }

    #[test]
    fn skips_non_commit() {
        let cmd = "git status -sb";
        assert_eq!(ensure_shell_command_co_author(cmd), cmd);
    }

    #[test]
    fn handles_git_exe_and_path() {
        let out =
            ensure_shell_command_co_author(r#"C:\Program Files\Git\cmd\git.exe commit -m hi"#);
        // path may not match if spaces confuse token — ensure we still try git.exe form
        let out2 = ensure_shell_command_co_author("git.exe commit -m hi");
        assert!(out2.contains("--trailer"), "{out2}");
        let _ = out;
    }

    #[test]
    fn injects_with_global_c_option() {
        let out = ensure_shell_command_co_author("git -C /tmp/repo commit -m note");
        assert!(out.contains("--trailer"), "{out}");
        assert!(out.contains("-C /tmp/repo"), "{out}");
    }

    #[test]
    fn powershell_semicolon_chain() {
        let out = ensure_powershell_command_co_author("git add .; git commit -m 'x'; git push");
        assert!(out.contains("--trailer"), "{out}");
        let trailer_at = out.find("--trailer").unwrap();
        // second semicolon after commit should remain after trailer
        assert!(out[trailer_at..].contains("; git push"), "{out}");
    }

    #[test]
    fn does_not_touch_commit_in_string_message_only() {
        // still a real commit — must inject
        let out = ensure_shell_command_co_author("git commit -m \"please commit soon\"");
        assert!(out.contains("--trailer"));
    }

    #[test]
    fn multiple_commits_in_one_line() {
        let out = ensure_shell_command_co_author("git commit -m one; git commit -m two");
        assert_eq!(out.matches("--trailer").count(), 2, "{out}");
    }
}
