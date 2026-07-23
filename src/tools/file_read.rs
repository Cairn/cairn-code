use super::registry::Tool;
use super::workspace::Workspace;
#[cfg(test)]
use std::fs;

pub struct FileReadTool {
    workspace: Workspace,
}

impl FileReadTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }
    fn description(&self) -> &str {
        "Read a file with optional offset and limit"
    }
    fn needs_permission(&self) -> bool {
        false
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["file_path"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let file_path = obj
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or("file_path required")?;
        let offset = obj.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = obj.get("limit").and_then(|v| v.as_u64());

        let relative = self
            .workspace
            .relative_path(file_path)
            .map_err(|e| format!("read error: {e}"))?;
        let content = self
            .workspace
            .dir()
            .read_to_string(&relative)
            .map_err(|e| format!("read error: {}", self.workspace.access_error(&relative, e)))?;
        let lines: Vec<&str> = content.lines().collect();

        let start = offset.min(lines.len());
        let end = match limit {
            Some(l) => (start + l as usize).min(lines.len()),
            None => lines.len(),
        };

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate().skip(start).take(end - start) {
            result.push_str(&format!("{}:{}\n", i + 1, line));
        }
        let start_display = start + 1;
        let total = lines.len();
        result.push_str(&format!(
            "\n{file_path}:{start_display} (showing lines {start_display}-{end_display} of {total})",
            file_path = file_path,
            start_display = start_display,
            end_display = end,
            total = total
        ));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_temp(contents: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cairn-file-read-{nanos}.txt"));
        fs::write(&path, contents).unwrap();
        path
    }

    fn tool_for(path: &std::path::Path) -> FileReadTool {
        FileReadTool::new(Workspace::new(path.parent().unwrap()).unwrap())
    }

    #[test]
    fn reads_all_lines_with_numbers() {
        let path = write_temp("alpha\nbeta\ngamma\n");
        let tool = tool_for(&path);
        let input = format!(
            r#"{{"file_path":"{}"}}"#,
            path.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("1:alpha"));
        assert!(out.contains("2:beta"));
        assert!(out.contains("3:gamma"));
        assert!(out.contains("of 3)"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn offset_and_limit() {
        let path = write_temp("a\nb\nc\nd\ne\n");
        let tool = tool_for(&path);
        let input = format!(
            r#"{{"file_path":"{}","offset":1,"limit":2}}"#,
            path.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("2:b"));
        assert!(out.contains("3:c"));
        assert!(!out.contains("1:a"));
        assert!(!out.contains("4:d"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_file_errors() {
        let tool = FileReadTool::new(Workspace::current().unwrap());
        let err = tool
            .execute(r#"{"file_path":"/nonexistent/cairn-no-such-file-xyz"}"#)
            .unwrap_err();
        assert!(err.contains("read error"), "{err}");
    }

    #[test]
    fn requires_file_path() {
        let tool = FileReadTool::new(Workspace::current().unwrap());
        assert!(tool.execute(r#"{}"#).is_err());
        assert!(tool.execute("not-json").is_err());
    }

    #[test]
    fn rejects_absolute_and_parent_paths_outside_workspace() {
        let root = write_temp("inside");
        let workspace = root.parent().unwrap().join(format!(
            "cairn-file-read-workspace-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&workspace).unwrap();
        let tool = FileReadTool::new(Workspace::new(&workspace).unwrap());

        let absolute = format!(
            r#"{{"file_path":"{}"}}"#,
            root.to_string_lossy().replace('\\', "\\\\")
        );
        assert!(tool
            .execute(&absolute)
            .unwrap_err()
            .contains("outside the workspace"));
        let parent = format!(
            r#"{{"file_path":"../{}"}}"#,
            root.file_name().unwrap().to_string_lossy()
        );
        assert!(tool
            .execute(&parent)
            .unwrap_err()
            .contains("outside the workspace"));

        let _ = fs::remove_file(root);
        let _ = fs::remove_dir(workspace);
    }

    #[test]
    fn rejects_directory_link_escape() {
        let workspace_seed = write_temp("marker");
        let outside_seed = write_temp("secret");
        let workspace = workspace_seed.with_extension("workspace");
        let outside = outside_seed.with_extension("outside");
        fs::remove_file(workspace_seed).unwrap();
        fs::remove_file(outside_seed).unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "escaped secret").unwrap();
        let link = workspace.join("escape");

        assert!(
            create_dir_link(&outside, &link),
            "failed to create test link"
        );

        let tool = FileReadTool::new(Workspace::new(&workspace).unwrap());
        let input = format!(
            r#"{{"file_path":"{}"}}"#,
            link.join("secret.txt")
                .to_string_lossy()
                .replace('\\', "\\\\")
        );
        assert!(tool
            .execute(&input)
            .unwrap_err()
            .contains("outside the workspace"));

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}
