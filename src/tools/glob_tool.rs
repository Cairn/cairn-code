use super::registry::Tool;
use super::workspace::Workspace;
use cap_std::fs::Dir;
use glob::Pattern;
use std::collections::HashSet;
#[cfg(test)]
use std::fs;
use std::path::Path;

/// Cap listed paths so broad globs (`src/**/*.rs`) do not flood the transcript.
/// The total count is always reported so the model knows the full match size.
const MAX_LISTED_PATHS: usize = 15;
const MAX_DEPTH: usize = 64;
const MAX_RESULTS: usize = 1_000;
const MAX_RESULT_BYTES: usize = 1_048_576;
const MAX_VISITED_ENTRIES: usize = 100_000;

pub struct GlobTool {
    workspace: Workspace,
}

impl GlobTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files matching a glob pattern (supports **, *). \
         Returns a compact path list (capped) plus the total match count."
    }
    fn needs_permission(&self) -> bool {
        false
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}"#
            .into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let pattern = obj
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or("pattern required")?;

        let mut results = collect_matches(pattern, &self.workspace)?;
        // Stable order keeps caps deterministic across runs.
        results.paths.sort();
        results.paths.dedup();

        let mut output = format_glob_results(&results.paths, MAX_LISTED_PATHS);
        if results.truncated {
            output.push_str("\nSearch truncated at safety limit.");
        }
        Ok(output)
    }
}

/// Format match paths for the transcript/model: list up to `max_listed`, then
/// a single summary line with the total (and how many were omitted).
pub(crate) fn format_glob_results(results: &[String], max_listed: usize) -> String {
    if results.is_empty() {
        return "No matches found.".into();
    }
    let total = results.len();
    let show = total.min(max_listed);
    let mut out = String::new();
    for path in &results[..show] {
        out.push_str(path);
        out.push('\n');
    }
    if total > show {
        let omitted = total - show;
        out.push_str(&format!("… and {omitted} more ({total} total)"));
    } else {
        out.push_str(&format!("{total} result(s)"));
    }
    out
}

pub(crate) fn glob_match(pattern: &str, workspace: &Workspace) -> Result<Vec<String>, String> {
    Ok(collect_matches(pattern, workspace)?.paths)
}

fn collect_matches(pattern: &str, workspace: &Workspace) -> Result<GlobResults, String> {
    if Path::new(pattern).is_absolute() {
        return Err(format!(
            "refusing to access '{pattern}': outside the workspace"
        ));
    }

    let pattern = workspace.relative_path(pattern)?;
    let mut results = GlobResults::default();
    let parts: Vec<String> = pattern
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    for part in &parts {
        Pattern::new(part).map_err(|error| format!("invalid pattern: {error}"))?;
    }
    if parts.len() > MAX_DEPTH {
        results.truncated = true;
        return Ok(results);
    }
    walk_pattern(workspace.dir(), &parts, 0, String::new(), 0, &mut results)?;

    Ok(results)
}

#[derive(Default)]
struct GlobResults {
    paths: Vec<String>,
    seen: HashSet<String>,
    bytes: usize,
    visited: usize,
    truncated: bool,
}

impl GlobResults {
    fn visit(&mut self) -> bool {
        if self.visited >= MAX_VISITED_ENTRIES {
            self.truncated = true;
            return false;
        }
        self.visited += 1;
        true
    }

    fn push(&mut self, path: String) -> bool {
        if self.seen.contains(&path) {
            return true;
        }
        if self.paths.len() >= MAX_RESULTS
            || self.bytes.saturating_add(path.len()) > MAX_RESULT_BYTES
        {
            self.truncated = true;
            return false;
        }
        self.bytes += path.len();
        self.seen.insert(path.clone());
        self.paths.push(path);
        true
    }
}

fn walk_pattern(
    dir: &Dir,
    parts: &[String],
    idx: usize,
    prefix: String,
    depth: usize,
    results: &mut GlobResults,
) -> Result<(), String> {
    if idx >= parts.len() {
        return Ok(());
    }
    if depth >= MAX_DEPTH {
        results.truncated = true;
        return Ok(());
    }

    let part = &parts[idx];
    let is_last = idx == parts.len() - 1;

    if part == "**" {
        // ** matches this dir and all subdirs
        let rest = &parts[idx + 1..];

        if rest.is_empty() {
            // ** at end - match everything recursively
            walk_dir_recursive(dir, &prefix, depth, results)?;
            return Ok(());
        }

        // ** followed by more pattern - try at current level and subdirs
        walk_pattern(dir, rest, 0, prefix.clone(), depth, results)?;

        if let Ok(entries) = dir.entries() {
            for entry in entries.flatten() {
                if results.truncated || !results.visit() {
                    break;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let new_prefix = if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    };
                    if let Ok(child) = entry.open_dir() {
                        walk_pattern(&child, parts, idx, new_prefix, depth + 1, results)?;
                    }
                }
            }
        }
    } else if part
        .chars()
        .any(|character| matches!(character, '*' | '?' | '['))
    {
        let matcher = Pattern::new(part).map_err(|error| format!("invalid pattern: {error}"))?;

        if let Ok(entries) = dir.entries() {
            for entry in entries.flatten() {
                if results.truncated || !results.visit() {
                    break;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().to_string();
                if matcher.matches(&name) {
                    let new_prefix = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}/{name}")
                    };
                    if is_last {
                        if !results.push(new_prefix) {
                            break;
                        }
                    }
                    if !is_last && file_type.is_dir() {
                        if let Ok(child) = entry.open_dir() {
                            walk_pattern(
                                &child,
                                parts,
                                idx + 1,
                                if prefix.is_empty() {
                                    name
                                } else {
                                    format!("{prefix}/{name}")
                                },
                                depth + 1,
                                results,
                            )?;
                        }
                    }
                }
            }
        }
    } else {
        // Literal component
        let new_prefix = if prefix.is_empty() {
            part.to_string()
        } else {
            format!("{prefix}/{part}")
        };

        if let Ok(entries) = dir.entries() {
            for entry in entries.flatten() {
                if results.truncated || !results.visit() {
                    break;
                }
                if !names_equal(&entry.file_name(), part) {
                    continue;
                }
                if is_last {
                    if !results.push(new_prefix) {
                        break;
                    }
                } else if entry
                    .file_type()
                    .map(|file_type| file_type.is_dir())
                    .unwrap_or(false)
                {
                    if let Ok(child) = entry.open_dir() {
                        walk_pattern(&child, parts, idx + 1, new_prefix, depth + 1, results)?;
                    }
                }
                break;
            }
        }
    }

    Ok(())
}

fn names_equal(actual: &std::ffi::OsStr, expected: &str) -> bool {
    #[cfg(windows)]
    {
        actual.to_string_lossy().eq_ignore_ascii_case(expected)
    }
    #[cfg(not(windows))]
    {
        actual == expected
    }
}

fn walk_dir_recursive(
    dir: &Dir,
    prefix: &str,
    depth: usize,
    results: &mut GlobResults,
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
            let new_prefix = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if !results.push(new_prefix.clone()) {
                break;
            }
            if file_type.is_dir() {
                if let Ok(child) = entry.open_dir() {
                    walk_dir_recursive(&child, &new_prefix, depth + 1, results)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_tree() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cairn-glob-{nanos}"));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main(){}\n").unwrap();
        fs::write(dir.join("src/lib.rs"), "").unwrap();
        fs::write(dir.join("README.md"), "").unwrap();
        dir
    }

    #[test]
    fn finds_rs_files() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();
        // Single-level wildcard (more reliable across path styles than `**` alone).
        let results = glob_match("src/*.rs", &workspace).unwrap();
        assert!(
            results
                .iter()
                .any(|p| p.replace('\\', "/").ends_with("src/main.rs") || p.contains("main.rs")),
            "src/*.rs => {results:?}"
        );
        assert!(
            results.iter().any(|p| p.contains("lib.rs")),
            "src/*.rs => {results:?}"
        );
        // Recursive form
        let deep = glob_match("**/*.rs", &workspace).unwrap();
        assert!(
            deep.iter().any(|p| p.contains("main.rs")),
            "**/*.rs => {deep:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dot_prefix_does_not_suppress_matches() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();

        let results = glob_match("./*.md", &workspace).unwrap();
        assert_eq!(results, vec!["README.md"]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recursive_wildcards_visit_directories_for_each_pattern_state() {
        let dir = temp_tree();
        fs::create_dir_all(dir.join("deep/src")).unwrap();
        fs::write(dir.join("deep/src/deep.rs"), "").unwrap();
        let workspace = Workspace::new(&dir).unwrap();

        let results = glob_match("**/*/*.rs", &workspace).unwrap();
        assert!(
            results.iter().any(|path| path == "deep/src/deep.rs"),
            "{results:?}"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repeated_recursive_wildcards_do_not_duplicate_results() {
        let dir = temp_tree();
        let tool = GlobTool::new(Workspace::new(&dir).unwrap());

        let output = tool.execute(r#"{"pattern":"**/**/main.rs"}"#).unwrap();
        assert_eq!(output.matches("src/main.rs").count(), 1, "{output}");

        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn literal_components_use_windows_case_insensitive_lookup() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();

        let results = glob_match("readme.md", &workspace).unwrap();
        assert_eq!(results, vec!["readme.md"]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn maintained_glob_matcher_matches_full_names() {
        let pattern = Pattern::new("*.rs").unwrap();
        assert!(pattern.matches("main.rs"));
        assert!(pattern.matches("lib.rs"));
        assert!(!pattern.matches("main.rs.bak"));
        assert!(!pattern.matches("README.md"));
    }

    #[test]
    fn supports_glob_character_classes() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();

        let results = glob_match("src/[ml]*.rs", &workspace).unwrap();
        assert!(results.iter().any(|path| path == "src/main.rs"));
        assert!(results.iter().any(|path| path == "src/lib.rs"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_invalid_glob_patterns() {
        let dir = temp_tree();
        let tool = GlobTool::new(Workspace::new(&dir).unwrap());

        let error = tool.execute(r#"{"pattern":"missing/["}"#).unwrap_err();
        assert!(error.contains("invalid pattern"), "{error}");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn does_not_return_intermediate_wildcard_directories() {
        let dir = temp_tree();
        fs::create_dir_all(dir.join("src/nested")).unwrap();
        fs::write(dir.join("src/nested/deep.rs"), "").unwrap();
        let workspace = Workspace::new(&dir).unwrap();

        let results = glob_match("src/*/*.rs", &workspace).unwrap();
        assert_eq!(results, vec!["src/nested/deep.rs"]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn literal_path() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();
        let results = glob_match("README.md", &workspace).unwrap();
        assert_eq!(results.len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn execute_no_matches() {
        let tool = GlobTool::new(Workspace::current().unwrap());
        let out = tool
            .execute(r#"{"pattern":"**/totally_missing_file_xyz_123.nope"}"#)
            .unwrap();
        assert!(out.contains("No matches"), "{out}");
    }

    #[test]
    fn requires_pattern() {
        let tool = GlobTool::new(Workspace::current().unwrap());
        assert!(tool.execute("{}").is_err());
    }

    #[test]
    fn rejects_absolute_and_parent_patterns() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();
        let tool = GlobTool::new(workspace);
        let absolute = format!(
            r#"{{"pattern":"{}/*"}}"#,
            dir.parent()
                .unwrap()
                .to_string_lossy()
                .replace('\\', "\\\\")
        );

        assert!(tool
            .execute(&absolute)
            .unwrap_err()
            .contains("outside the workspace"));
        assert!(tool
            .execute(r#"{"pattern":"../*"}"#)
            .unwrap_err()
            .contains("outside the workspace"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recursive_glob_skips_directory_link_escape() {
        let workspace_dir = temp_tree();
        let outside = workspace_dir.parent().unwrap().join(format!(
            "cairn-glob-outside-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "escaped secret").unwrap();
        let link = workspace_dir.join("escape");

        assert!(
            create_dir_link(&outside, &link),
            "failed to create test link"
        );

        let workspace = Workspace::new(&workspace_dir).unwrap();
        let results = glob_match("**/*", &workspace).unwrap();
        assert!(
            !results.iter().any(|path| path.contains("secret.txt")),
            "{results:?}"
        );

        let _ = fs::remove_dir_all(workspace_dir);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn recursive_glob_does_not_follow_link_cycles() {
        let workspace_dir = temp_tree();
        assert!(
            create_dir_link(&workspace_dir, &workspace_dir.join("loop")),
            "failed to create test link"
        );

        let workspace = Workspace::new(&workspace_dir).unwrap();
        let results = glob_match("**", &workspace).unwrap();
        assert!(results.iter().any(|path| path == "loop"), "{results:?}");
        assert!(
            !results.iter().any(|path| path.starts_with("loop/")),
            "{results:?}"
        );
        assert!(
            !results.iter().any(|path| path.starts_with('/')),
            "{results:?}"
        );

        let _ = fs::remove_dir_all(workspace_dir);
    }

    #[test]
    fn recursive_glob_does_not_follow_cycles_below_pattern_components() {
        let workspace_dir = temp_tree();
        let src = workspace_dir.join("src");
        assert!(
            create_dir_link(&src, &src.join("loop")),
            "failed to create test link"
        );

        let workspace = Workspace::new(&workspace_dir).unwrap();
        for pattern in ["src/**", "*/**"] {
            let results = glob_match(pattern, &workspace).unwrap();
            assert!(
                !results.iter().any(|path| path.contains("loop/main.rs")),
                "{pattern} => {results:?}"
            );
        }

        let _ = fs::remove_dir_all(workspace_dir);
    }

    #[test]
    fn recursive_glob_stops_at_depth_limit() {
        let workspace_dir = temp_tree();
        let mut deepest = workspace_dir.clone();
        for index in 0..=MAX_DEPTH {
            deepest.push(format!("d{index}"));
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("needle.txt"), "").unwrap();
        let tool = GlobTool::new(Workspace::new(&workspace_dir).unwrap());

        let output = tool.execute(r#"{"pattern":"**/needle.txt"}"#).unwrap();
        assert!(!output.contains("needle.txt\n"), "{output}");
        assert!(output.contains("Search truncated"), "{output}");

        let _ = fs::remove_dir_all(workspace_dir);
    }

    #[test]
    fn globstar_chain_stops_at_pattern_depth_limit() {
        let workspace_dir = temp_tree();
        let tool = GlobTool::new(Workspace::new(&workspace_dir).unwrap());
        let pattern = std::iter::repeat_n("**", MAX_DEPTH + 1)
            .collect::<Vec<_>>()
            .join("/");
        let input = format!(r#"{{"pattern":"{pattern}"}}"#);

        let output = tool.execute(&input).unwrap();
        assert_eq!(
            output,
            "No matches found.\nSearch truncated at safety limit."
        );

        let _ = fs::remove_dir_all(workspace_dir);
    }

    #[test]
    fn glob_stops_at_result_count_limit() {
        let workspace_dir = temp_tree();
        for index in 0..=MAX_RESULTS {
            fs::write(workspace_dir.join(format!("result-{index}.txt")), "").unwrap();
        }
        let tool = GlobTool::new(Workspace::new(&workspace_dir).unwrap());

        let output = tool.execute(r#"{"pattern":"*"}"#).unwrap();
        assert!(output.contains("1000 total"), "{output}");
        assert!(output.contains("Search truncated"), "{output}");

        let _ = fs::remove_dir_all(workspace_dir);
    }

    #[cfg(windows)]
    #[test]
    fn accepts_native_windows_separators_and_rejects_backslash_parent() {
        let dir = temp_tree();
        let workspace = Workspace::new(&dir).unwrap();
        assert!(!glob_match(r"src\*.rs", &workspace).unwrap().is_empty());
        assert!(glob_match(r"..\*", &workspace)
            .unwrap_err()
            .contains("outside the workspace"));
        let _ = fs::remove_dir_all(dir);
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

    #[test]
    fn format_caps_long_lists() {
        let paths: Vec<String> = (0..40).map(|i| format!("src/f{i}.rs")).collect();
        let out = format_glob_results(&paths, 15);
        assert!(out.contains("src/f0.rs"), "{out}");
        assert!(out.contains("src/f14.rs"), "{out}");
        assert!(
            !out.contains("src/f15.rs"),
            "should not list past cap: {out}"
        );
        assert!(out.contains("… and 25 more (40 total)"), "{out}");
        // Compact: cap + summary, not 40 path lines.
        let path_lines = out.lines().filter(|l| l.starts_with("src/")).count();
        assert_eq!(path_lines, 15, "{out}");
    }

    #[test]
    fn format_short_list_shows_all() {
        let paths = vec!["a.rs".into(), "b.rs".into()];
        let out = format_glob_results(&paths, 15);
        assert!(out.contains("a.rs"));
        assert!(out.contains("b.rs"));
        assert!(out.contains("2 result(s)"));
        assert!(!out.contains("… and"));
    }
}
