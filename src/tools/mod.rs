pub mod file_edit;
pub mod file_history;
pub mod file_read;
pub mod file_undo;
pub mod file_write;
pub mod git_tool;
pub mod glob_tool;
pub mod go_tool;
pub mod grep_tool;
pub mod memory;
pub mod powershell_tool;
pub mod process_runner;
pub mod python_tool;
pub mod registry;
pub mod shell;
pub mod skill_tool;
pub mod todo;
pub mod web_fetch;
pub mod web_search;
pub mod workspace;

/// Keep command output useful to the model without returning an unbounded
/// response. Preserve both the beginning (the primary diagnostic) and the end
/// (summaries and final errors).
const MAX_COMMAND_OUTPUT_CHARS: usize = 12_000;
const COMMAND_OUTPUT_HEAD_CHARS: usize = 6_000;
const COMMAND_OUTPUT_TAIL_CHARS: usize = 4_000;

pub(super) fn parse_command_args(input: &str) -> Result<Vec<String>, String> {
    let value = crate::json::parse(input).map_err(|error| format!("invalid input: {error}"))?;
    let object = value.as_object().ok_or("expected object")?;
    object
        .get("args")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "args must be an array of strings".to_string())?
        .iter()
        .map(|arg| {
            arg.as_str()
                .map(String::from)
                .ok_or_else(|| "args must contain only strings".to_string())
        })
        .collect()
}

pub(super) fn bounded_command_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let mut output = String::new();

    if !stdout.is_empty() {
        output.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&stderr);
    }

    truncate_head_tail(
        &output,
        MAX_COMMAND_OUTPUT_CHARS,
        COMMAND_OUTPUT_HEAD_CHARS,
        COMMAND_OUTPUT_TAIL_CHARS,
    )
}

fn truncate_head_tail(s: &str, max: usize, head: usize, tail: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }

    let head = head.min(chars.len());
    let tail = tail.min(chars.len().saturating_sub(head));
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    let omitted = chars.len() - head - tail;
    format!("{start}\n... [{omitted} chars truncated] ...\n{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_args_preserve_spaces_quotes_and_empty_values() {
        let args = parse_command_args(r#"{"args":["status","path with spaces","\"quoted\"",""]}"#)
            .unwrap();

        assert_eq!(args, ["status", "path with spaces", "\"quoted\"", ""]);
    }

    #[test]
    fn command_output_is_bounded_and_keeps_head_and_tail() {
        let stdout = format!("HEAD{}", "x".repeat(20_000));
        let stderr = "TAIL";
        let output = bounded_command_output(stdout.as_bytes(), stderr.as_bytes());

        assert!(output.starts_with("HEAD"));
        assert!(output.ends_with("TAIL"));
        assert!(output.contains("chars truncated"));
        assert!(output.chars().count() <= MAX_COMMAND_OUTPUT_CHARS);
    }
}
