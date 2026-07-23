use super::registry::Tool;
use std::fs;

pub struct FileEditTool;

impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }
    fn description(&self) -> &str {
        "Find and replace text in a file"
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["file_path","old_string","new_string"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let file_path = obj
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or("file_path required")?;
        let old_string = obj
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or("old_string required")?;
        let new_string = obj.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        let replace_all = obj
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let resolved = super::workspace::resolve_in_workspace(file_path)?;
        let content = fs::read_to_string(&resolved).map_err(|e| format!("read error: {e}"))?;

        // Exact match first; if that fails, retry tolerant of CRLF/LF
        // differences between the file and a model-generated old_string
        // (most common on Windows), then apply on the original content so
        // the file's line-ending convention is preserved.
        let uses_exact = content.contains(old_string);
        let normalized_content = content.replace("\r\n", "\n");
        let normalized_old = old_string.replace("\r\n", "\n");
        let uses_normalized = !uses_exact && normalized_content.contains(&normalized_old);

        if !uses_exact && !uses_normalized {
            return Err(format!("old_string not found in {file_path}"));
        }

        let new_content = if uses_exact {
            if replace_all {
                content.replace(old_string, new_string)
            } else {
                match content.find(old_string) {
                    Some(pos) => {
                        let mut result = content[..pos].to_string();
                        result.push_str(new_string);
                        result.push_str(&content[pos + old_string.len()..]);
                        result
                    }
                    None => return Err("old_string not found".into()),
                }
            }
        } else {
            let new_string_normalized = new_string.replace("\r\n", "\n");
            let edited = if replace_all {
                normalized_content.replace(&normalized_old, &new_string_normalized)
            } else {
                match normalized_content.find(&normalized_old) {
                    Some(pos) => {
                        let mut result = normalized_content[..pos].to_string();
                        result.push_str(&new_string_normalized);
                        result.push_str(&normalized_content[pos + normalized_old.len()..]);
                        result
                    }
                    None => return Err("old_string not found".into()),
                }
            };
            // Restore the file's original CRLF convention across the whole
            // result, since the edit above was computed on LF-normalized text.
            if content.contains("\r\n") {
                edited.replace('\n', "\r\n")
            } else {
                edited
            }
        };

        let count = if uses_exact {
            if replace_all {
                content.matches(old_string).count()
            } else {
                1
            }
        } else if replace_all {
            normalized_content.matches(&normalized_old).count()
        } else {
            1
        };

        super::file_history::record_snapshot(resolved.clone(), file_path, content);
        fs::write(&resolved, &new_content).map_err(|e| format!("write error: {e}"))?;
        Ok(format!("Applied {count} edit(s) to {file_path}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_escape_is_rejected() {
        let tool = FileEditTool;
        let input =
            r#"{"file_path":"../outside_cairn_edit_test.txt","old_string":"a","new_string":"b"}"#;
        let err = tool.execute(input).unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_exact_match_replaces_content() {
        let path = "target/cairn_file_edit_test_exact.txt";
        fs::write(path, "hello world").unwrap();
        let tool = FileEditTool;
        let input = format!(r#"{{"file_path":"{path}","old_string":"world","new_string":"rust"}}"#);
        tool.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "hello rust");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_crlf_fallback_match_preserves_crlf() {
        let path = "target/cairn_file_edit_test_crlf.txt";
        fs::write(path, "line1\r\nline2\r\nline3").unwrap();
        let tool = FileEditTool;
        // old_string uses bare \n, the file uses \r\n: exact match fails, the
        // CRLF-tolerant fallback should still find and apply it.
        let input = format!(
            r#"{{"file_path":"{path}","old_string":"line1\nline2","new_string":"REPLACED"}}"#
        );
        tool.execute(&input).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "REPLACED\r\nline3");
        let _ = fs::remove_file(path);
    }
}
