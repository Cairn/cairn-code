use super::registry::Tool;
use super::workspace::Workspace;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use regex::Regex;
#[cfg(test)]
use std::fs;
use std::io::Read;
use std::path::{Component, Path};

const MAX_DEPTH: usize = 64;
const MAX_FILE_BYTES: u64 = 1_048_576;
const MAX_RESULTS: usize = 1_000;
const MAX_RESULT_BYTES: usize = 1_048_576;
const MAX_VISITED_ENTRIES: usize = 100_000;

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

        let re = Regex::new(pattern).map_err(|e| format!("invalid pattern: {e}"))?;

        let search_path = self.workspace.relative_path(search_path)?;
        let access_path = if search_path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &search_path
        };
        let mut results = SearchResults::default();
        let relative = display_path(&search_path);
        let target = match open_search_target(self.workspace.dir(), access_path) {
            Ok(target) => target,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok("No matches found.".into());
            }
            Err(error) => return Err(self.workspace.access_error(&search_path, error)),
        };
        match target {
            Some(SearchTarget::File(file)) => {
                search_file(file, &search_path, &re, include, &relative, &mut results);
            }
            Some(SearchTarget::Dir(dir)) => {
                search_dir(&dir, &re, include, &relative, 0, &mut results)?;
            }
            None => {}
        }

        if results.matches.is_empty() {
            let mut output = String::from("No matches found.");
            if results.truncated {
                output.push_str("\nSearch truncated at safety limit.");
            }
            return Ok(output);
        }

        let mut output = String::new();
        for (file, line_num, line) in &results.matches {
            output.push_str(&format!("{file}:{line_num}:{line}\n"));
        }
        output.push_str(&format!("{} result(s)", results.matches.len()));
        if results.truncated {
            output.push_str("\nSearch truncated at safety limit.");
        }
        Ok(output)
    }
}

enum SearchTarget {
    File(File),
    Dir(Dir),
}

fn open_search_target(root: &Dir, path: &Path) -> std::io::Result<Option<SearchTarget>> {
    let mut current = root.try_clone()?;
    let components: Vec<_> = path.components().collect();
    if components.is_empty() || path == Path::new(".") {
        return Ok(Some(SearchTarget::Dir(current)));
    }

    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            continue;
        };
        let metadata = current.symlink_metadata(name)?;
        if metadata.file_type().is_symlink() {
            return Ok(None);
        }
        let is_last = index == components.len() - 1;
        if metadata.is_dir() {
            let directory = current.open_dir_nofollow(name)?;
            if is_last {
                return Ok(Some(SearchTarget::Dir(directory)));
            }
            current = directory;
        } else if metadata.is_file() && is_last {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            return current
                .open_with(name, &options)
                .map(|file| Some(SearchTarget::File(file)));
        } else {
            return Ok(None);
        }
    }

    Ok(Some(SearchTarget::Dir(current)))
}

#[derive(Default)]
struct SearchResults {
    matches: Vec<(String, usize, String)>,
    bytes: usize,
    visited: usize,
    truncated: bool,
}

impl SearchResults {
    fn visit(&mut self) -> bool {
        if self.visited >= MAX_VISITED_ENTRIES {
            self.truncated = true;
            return false;
        }
        self.visited += 1;
        true
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
    fn supports_documented_regex_semantics() {
        let dir = temp_dir();
        fs::write(dir.join("x.txt"), "alpha-42\nalpha-nope\nprefix alpha-7\n").unwrap();
        let tool = GrepTool::new(Workspace::new(&dir).unwrap());
        let input = format!(
            r#"{{"pattern":"^alpha-[0-9]+$","path":"{}"}}"#,
            dir.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("alpha-42"), "{out}");
        assert!(!out.contains("alpha-nope"), "{out}");
        assert!(!out.contains("prefix alpha-7"), "{out}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_invalid_regex_patterns() {
        let dir = temp_dir();
        let tool = GrepTool::new(Workspace::new(&dir).unwrap());

        let error = tool.execute(r#"{"pattern":"("}"#).unwrap_err();
        assert!(error.contains("invalid pattern"), "{error}");

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
    fn explicit_search_path_does_not_follow_directory_links() {
        let workspace = temp_dir();
        fs::create_dir_all(workspace.join("real/sub")).unwrap();
        fs::create_dir(workspace.join("parent")).unwrap();
        fs::write(workspace.join("real/sub/token.txt"), "linked token").unwrap();
        assert!(
            create_dir_link(&workspace.join("real"), &workspace.join("final-link")),
            "failed to create final-component test link"
        );
        assert!(
            create_dir_link(&workspace.join("real"), &workspace.join("parent/link")),
            "failed to create intermediate-component test link"
        );
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        for path in ["final-link", "parent/link/sub"] {
            let input = format!(r#"{{"pattern":"linked token","path":"{path}"}}"#);
            let out = tool.execute(&input).unwrap();
            assert_eq!(out, "No matches found.", "{path}: {out}");
        }

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

    #[test]
    fn recursive_search_stops_at_depth_limit() {
        let workspace = temp_dir();
        let mut deepest = workspace.clone();
        for index in 0..=MAX_DEPTH {
            deepest.push(format!("d{index}"));
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("token.txt"), "too deep token").unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let out = tool.execute(r#"{"pattern":"too deep token"}"#).unwrap();
        assert!(!out.contains("token.txt:"), "{out}");
        assert!(out.contains("Search truncated"), "{out}");

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn search_stops_at_result_count_limit() {
        let workspace = temp_dir();
        fs::write(
            workspace.join("many.txt"),
            "bounded-hit\n".repeat(MAX_RESULTS + 1),
        )
        .unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let out = tool.execute(r#"{"pattern":"bounded-hit"}"#).unwrap();
        assert_eq!(out.matches(":bounded-hit").count(), MAX_RESULTS, "{out}");
        assert!(out.contains("Search truncated"), "{out}");

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn skips_files_over_size_limit() {
        let workspace = temp_dir();
        fs::write(
            workspace.join("large.txt"),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        )
        .unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let out = tool.execute(r#"{"pattern":"x"}"#).unwrap();
        assert_eq!(out, "No matches found.");

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn search_stops_at_result_byte_limit() {
        let workspace = temp_dir();
        let line = format!("match {}", "x".repeat(MAX_RESULT_BYTES / 2));
        fs::write(workspace.join("a.txt"), format!("{line}\n")).unwrap();
        fs::write(workspace.join("b.txt"), format!("{line}\n")).unwrap();
        let tool = GrepTool::new(Workspace::new(&workspace).unwrap());

        let out = tool.execute(r#"{"pattern":"^match"}"#).unwrap();
        assert!(out.contains("1 result(s)"), "result count missing");
        assert!(out.contains("Search truncated"), "truncation missing");

        let _ = fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        let target_win = target.to_string_lossy().replace('/', "\\");
        let link_win = link.to_string_lossy().replace('/', "\\");
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", &link_win, &target_win])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}

fn search_dir(
    dir: &Dir,
    re: &Regex,
    include: Option<&str>,
    relative: &str,
    depth: usize,
    results: &mut SearchResults,
) -> Result<(), String> {
    if depth >= MAX_DEPTH {
        results.truncated = true;
        return Ok(());
    }

    if let Ok(entries) = dir.entries() {
        for entry in entries.flatten() {
            if results.truncated || !results.visit() {
                break;
            }
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
                    search_dir(&child, re, include, &rel, depth + 1, results)?;
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
    re: &Regex,
    include: Option<&str>,
    relative: &str,
    results: &mut SearchResults,
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

    if file
        .metadata()
        .map(|metadata| metadata.len() > MAX_FILE_BYTES)
        .unwrap_or(true)
    {
        return;
    }

    let mut content = String::new();
    if file
        .by_ref()
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .is_ok()
        && content.len() as u64 <= MAX_FILE_BYTES
    {
        for (index, line) in content.lines().enumerate() {
            if re.is_match(line) {
                let result_bytes = relative.len() + line.len() + 32;
                if results.matches.len() >= MAX_RESULTS
                    || results.bytes.saturating_add(result_bytes) > MAX_RESULT_BYTES
                {
                    results.truncated = true;
                    break;
                }
                results.bytes += result_bytes;
                results
                    .matches
                    .push((relative.to_string(), index + 1, line.to_string()));
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
