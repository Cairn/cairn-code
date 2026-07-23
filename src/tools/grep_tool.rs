use super::registry::Tool;
use super::workspace::Workspace;
use cap_std::fs::{Dir, File};
#[cfg(test)]
use std::fs;
use std::io::Read;
use std::path::Path;

pub struct GrepTool {
    workspace: Workspace,
}

impl GrepTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents using regex"
    }
    fn needs_permission(&self) -> bool {
        false
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"pattern":{"type":"string"},"include":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let pattern = obj
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or("pattern required")?;
        let include = obj.get("include").and_then(|v| v.as_str());
        let search_path = obj.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        // Escape literals first, then expand wildcards (same order as glob_tool).
        // Doing `*` before `.` would turn `.*` into `\.*` and break matches.
        let re = SimpleRe::new(&format!(
            ".*{}.*",
            pattern
                .replace('.', "\\.")
                .replace('*', ".*")
                .replace('?', ".")
        ))
        .map_err(|e| format!("invalid pattern: {e}"))?;

        let search_path = self.workspace.relative_path(search_path)?;
        let access_path = if search_path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &search_path
        };
        let mut results = Vec::new();
        let relative = display_path(&search_path);
        let metadata = match self.workspace.dir().metadata(access_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok("No matches found.".into());
            }
            Err(error) => return Err(self.workspace.access_error(&search_path, error)),
        };
        if metadata.is_file() {
            let file = self
                .workspace
                .dir()
                .open(access_path)
                .map_err(|error| self.workspace.access_error(&search_path, error))?;
            search_file(file, &search_path, &re, include, &relative, &mut results);
        } else if metadata.is_dir() {
            let dir = self
                .workspace
                .dir()
                .open_dir(access_path)
                .map_err(|error| self.workspace.access_error(&search_path, error))?;
            search_dir(&dir, &re, include, &relative, &mut results)?;
        }

        if results.is_empty() {
            return Ok("No matches found.".into());
        }

        let mut output = String::new();
        for (file, line_num, line) in &results {
            output.push_str(&format!("{file}:{line_num}:{line}\n"));
        }
        output.push_str(&format!("{} result(s)", results.len()));
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cairn-grep-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_literal_in_file() {
        let dir = temp_dir();
        fs::write(
            dir.join("a.rs"),
            "fn main() {\n    println!(\"hello unique_token_xyz\");\n}\n",
        )
        .unwrap();
        fs::write(dir.join("b.txt"), "nope\n").unwrap();
        let tool = GrepTool::new(Workspace::new(&dir).unwrap());
        let input = format!(
            r#"{{"pattern":"unique_token_xyz","path":"{}"}}"#,
            dir.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("unique_token_xyz"), "{out}");
        assert!(
            out.contains("1 result") || out.contains("result(s)"),
            "{out}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn wildcard_star_matches() {
        let dir = temp_dir();
        fs::write(dir.join("x.txt"), "alpha-beta-gamma\n").unwrap();
        let tool = GrepTool::new(Workspace::new(&dir).unwrap());
        let input = format!(
            r#"{{"pattern":"alpha*gamma","path":"{}"}}"#,
            dir.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("alpha-beta-gamma"), "{out}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn no_matches() {
        let dir = temp_dir();
        fs::write(dir.join("x.txt"), "nothing here\n").unwrap();
        let tool = GrepTool::new(Workspace::new(&dir).unwrap());
        let input = format!(
            r#"{{"pattern":"definitely_not_present_zzz","path":"{}"}}"#,
            dir.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("No matches"), "{out}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_in_workspace_path_is_an_empty_result() {
        let dir = temp_dir();
        let tool = GrepTool::new(Workspace::new(&dir).unwrap());

        let out = tool
            .execute(r#"{"pattern":"anything","path":"missing"}"#)
            .unwrap();
        assert_eq!(out, "No matches found.");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn requires_pattern() {
        let tool = GrepTool::new(Workspace::current().unwrap());
        assert!(tool.execute(r#"{}"#).is_err());
    }

    #[test]
    fn rejects_absolute_and_parent_search_paths() {
        let workspace = temp_dir();
        let outside = workspace.parent().unwrap().join(format!(
            "cairn-grep-outside-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "escaped secret").unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let absolute = format!(
            r#"{{"pattern":"escaped secret","path":"{}"}}"#,
            outside.to_string_lossy().replace('\\', "\\\\")
        );
        assert!(tool
            .execute(&absolute)
            .unwrap_err()
            .contains("outside the workspace"));
        let parent = format!(
            r#"{{"pattern":"escaped secret","path":"../{}"}}"#,
            outside.file_name().unwrap().to_string_lossy()
        );
        assert!(tool
            .execute(&parent)
            .unwrap_err()
            .contains("outside the workspace"));

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn recursive_search_skips_directory_link_escape() {
        let workspace = temp_dir();
        let outside = workspace.parent().unwrap().join(format!(
            "cairn-grep-link-outside-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "escaped secret").unwrap();
        let link = workspace.join("escape");

        assert!(
            create_dir_link(&outside, &link),
            "failed to create test link"
        );

        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());
        let out = tool.execute(r#"{"pattern":"escaped secret"}"#).unwrap();
        assert_eq!(out, "No matches found.");

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn recursive_search_does_not_follow_link_cycles() {
        let workspace = temp_dir();
        fs::write(workspace.join("token.txt"), "cycle token").unwrap();
        assert!(
            create_dir_link(&workspace, &workspace.join("loop")),
            "failed to create test link"
        );

        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());
        let out = tool.execute(r#"{"pattern":"cycle token"}"#).unwrap();
        assert_eq!(out.matches("cycle token").count(), 1, "{out}");

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn subdirectory_search_reports_workspace_relative_paths() {
        let workspace = temp_dir();
        fs::create_dir(workspace.join("src")).unwrap();
        fs::write(workspace.join("main.rs"), "unrelated").unwrap();
        fs::write(workspace.join("src/main.rs"), "nested token").unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let out = tool
            .execute(r#"{"pattern":"nested token","path":"src"}"#)
            .unwrap();
        assert!(out.contains("src/main.rs:1:nested token"), "{out}");

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn searches_a_single_file_path() {
        let workspace = temp_dir();
        fs::create_dir(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/main.rs"), "single file token").unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let out = tool
            .execute(r#"{"pattern":"single file token","path":"src/main.rs"}"#)
            .unwrap();
        assert!(out.contains("src/main.rs:1:single file token"), "{out}");

        let _ = fs::remove_dir_all(workspace);
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

fn search_dir(
    dir: &Dir,
    re: &SimpleRe,
    include: Option<&str>,
    relative: &str,
    results: &mut Vec<(String, usize, String)>,
) -> Result<(), String> {
    if let Ok(entries) = dir.entries() {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip .git, node_modules etc.
            if name == ".git" || name == "node_modules" || name == "target" {
                continue;
            }

            let rel = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };

            if file_type.is_dir() {
                if let Ok(child) = entry.open_dir() {
                    search_dir(&child, re, include, &rel, results)?;
                }
            } else if file_type.is_file() {
                if let Ok(file) = entry.open() {
                    search_file(file, Path::new(&name), re, include, &rel, results);
                }
            }
        }
    }
    Ok(())
}

fn search_file(
    mut file: File,
    path: &Path,
    re: &SimpleRe,
    include: Option<&str>,
    relative: &str,
    results: &mut Vec<(String, usize, String)>,
) {
    if let Some(inc) = include {
        let extension_matches = path
            .extension()
            .map(|extension| extension.to_string_lossy().as_ref() == inc.trim_start_matches('.'))
            .unwrap_or(false);
        if !extension_matches && !relative.contains(inc) {
            return;
        }
    }

    let mut content = String::new();
    if file.read_to_string(&mut content).is_ok() {
        for (index, line) in content.lines().enumerate() {
            if re.is_match(line) {
                results.push((relative.to_string(), index + 1, line.to_string()));
            }
        }
    }
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

struct SimpleRe {
    pattern: Vec<Segment>,
}

enum Segment {
    Literal(String),
    AnySeq,
    AnyChar,
}

impl SimpleRe {
    fn new(pattern: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
                '\\' if i + 1 < chars.len() => {
                    literal.push(chars[i + 1]);
                    i += 2;
                }
                '.' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(literal.clone()));
                        literal.clear();
                    }
                    segments.push(Segment::AnySeq);
                    i += 2;
                }
                '.' => {
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(literal.clone()));
                        literal.clear();
                    }
                    segments.push(Segment::AnyChar);
                    i += 1;
                }
                c => {
                    literal.push(c);
                    i += 1;
                }
            }
        }
        if !literal.is_empty() {
            segments.push(Segment::Literal(literal));
        }

        Ok(SimpleRe { pattern: segments })
    }

    fn is_match(&self, text: &str) -> bool {
        for start in 0..=text.len() {
            if self.match_from(text, start, 0) {
                return true;
            }
        }
        false
    }

    fn match_from(&self, text: &str, ti: usize, pi: usize) -> bool {
        if pi >= self.pattern.len() {
            return true;
        }

        match &self.pattern[pi] {
            Segment::Literal(lit) => {
                if ti + lit.len() <= text.len() && &text[ti..ti + lit.len()] == lit {
                    self.match_from(text, ti + lit.len(), pi + 1)
                } else {
                    false
                }
            }
            Segment::AnyChar => {
                if ti < text.len() {
                    self.match_from(text, ti + 1, pi + 1)
                } else {
                    false
                }
            }
            Segment::AnySeq => {
                for i in ti..=text.len() {
                    if self.match_from(text, i, pi + 1) {
                        return true;
                    }
                }
                false
            }
        }
    }
}
