use super::registry::Tool;
use super::workspace::Workspace;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Cap listed paths so broad globs (`src/**/*.rs`) do not flood the transcript.
/// The total count is always reported so the model knows the full match size.
const MAX_LISTED_PATHS: usize = 15;

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

        let mut results = glob_match(pattern, &self.workspace)?;
        // Stable order keeps caps deterministic across runs.
        results.sort();

        Ok(format_glob_results(&results, MAX_LISTED_PATHS))
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
    if Path::new(pattern).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "refusing to access '{pattern}': outside the workspace"
        ));
    }

    let mut results = Vec::new();
    let base = workspace.root();

    // Split pattern into parts
    let parts: Vec<&str> = pattern.split(std::path::is_separator).collect();

    // If pattern starts with **, we just need to match the rest
    if parts.len() == 1 && !parts[0].contains('*') && !parts[0].contains('?') {
        let path = base.join(parts[0]);
        if path.exists() {
            if workspace.resolve_existing(&path).is_ok() {
                results.push(parts[0].to_string());
            }
        }
        return Ok(results);
    }

    let mut visited = HashSet::from([base.to_path_buf()]);
    walk_pattern(
        base,
        workspace,
        &mut visited,
        &parts,
        0,
        String::new(),
        &mut results,
    )?;

    Ok(results)
}

fn walk_pattern(
    dir: &Path,
    workspace: &Workspace,
    visited: &mut HashSet<PathBuf>,
    parts: &[&str],
    idx: usize,
    prefix: String,
    results: &mut Vec<String>,
) -> Result<(), String> {
    if idx >= parts.len() {
        return Ok(());
    }

    let part = parts[idx];
    let is_last = idx == parts.len() - 1;

    if part == "**" {
        // ** matches this dir and all subdirs
        let rest = &parts[idx + 1..];

        if rest.is_empty() {
            // ** at end - match everything recursively
            walk_dir_recursive(dir, workspace, visited, &prefix, results)?;
            return Ok(());
        }

        // ** followed by more pattern - try at current level and subdirs
        walk_pattern(dir, workspace, visited, rest, 0, prefix.clone(), results)?;

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let Ok(path) = workspace.resolve_existing(entry.path()) else {
                    continue;
                };
                if path.is_dir() && visited.insert(path.clone()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let new_prefix = if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    };
                    walk_pattern(&path, workspace, visited, parts, idx, new_prefix, results)?;
                }
            }
        }
    } else if part.contains('*') || part.contains('?') {
        // Wildcard pattern. Do not wrap with `^$` as literal characters — SimpleRe
        // treats those as literals, so `^.*\.rs$` never matched real filenames.
        let re_pattern = part
            .replace('.', "\\.")
            .replace('*', ".*")
            .replace('?', ".");
        let re = regex_wrapper(&re_pattern)?;

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let Ok(path) = workspace.resolve_existing(entry.path()) else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().to_string();
                if re.is_full_match(&name) {
                    let new_prefix = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}/{name}")
                    };
                    if is_last || path.is_dir() {
                        results.push(new_prefix);
                    }
                    if !is_last && path.is_dir() && visited.insert(path.clone()) {
                        walk_pattern(
                            &path,
                            workspace,
                            visited,
                            parts,
                            idx + 1,
                            if prefix.is_empty() {
                                name
                            } else {
                                format!("{prefix}/{name}")
                            },
                            results,
                        )?;
                    }
                }
            }
        }
    } else {
        // Literal component
        let child = dir.join(part);
        let new_prefix = if prefix.is_empty() {
            part.to_string()
        } else {
            format!("{prefix}/{part}")
        };

        if is_last {
            if child.exists() && workspace.resolve_existing(&child).is_ok() {
                results.push(new_prefix);
            }
        } else if child.is_dir() {
            let child = workspace.resolve_existing(child)?;
            if visited.insert(child.clone()) {
                walk_pattern(
                    &child,
                    workspace,
                    visited,
                    parts,
                    idx + 1,
                    new_prefix,
                    results,
                )?;
            }
        }
    }

    Ok(())
}

fn walk_dir_recursive(
    dir: &Path,
    workspace: &Workspace,
    visited: &mut HashSet<PathBuf>,
    prefix: &str,
    results: &mut Vec<String>,
) -> Result<(), String> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(path) = workspace.resolve_existing(entry.path()) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let new_prefix = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            results.push(new_prefix.clone());
            if path.is_dir() && visited.insert(path.clone()) {
                walk_dir_recursive(&path, workspace, visited, &new_prefix, results)?;
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
    fn simple_re_full_match_star_rs() {
        let re = regex_wrapper(r".*\.rs").unwrap();
        assert!(re.is_full_match("main.rs"));
        assert!(re.is_full_match("lib.rs"));
        assert!(!re.is_full_match("main.rs.bak"));
        assert!(!re.is_full_match("README.md"));
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

struct SimpleRe {
    pattern: Vec<Segment>,
}

enum Segment {
    Literal(String),
    AnySeq,  // .*
    AnyChar, // .
}

impl SimpleRe {
    fn new(pattern: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
                // Escaped literal (e.g. `\.` from wildcard conversion of `*.rs`).
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
        self.match_from(text, 0, 0)
    }

    /// Match the entire filename (not a substring).
    fn is_full_match(&self, text: &str) -> bool {
        self.is_match(text)
    }

    fn match_from(&self, text: &str, pi: usize, ti: usize) -> bool {
        if pi >= self.pattern.len() {
            return ti >= text.len();
        }

        match &self.pattern[pi] {
            Segment::Literal(lit) => {
                if text[ti..].starts_with(lit) {
                    self.match_from(text, pi + 1, ti + lit.len())
                } else {
                    false
                }
            }
            Segment::AnyChar => {
                if ti < text.len() {
                    self.match_from(text, pi + 1, ti + 1)
                } else {
                    false
                }
            }
            Segment::AnySeq => {
                for i in ti..=text.len() {
                    if self.match_from(text, pi + 1, i) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

fn regex_wrapper(pattern: &str) -> Result<SimpleRe, String> {
    SimpleRe::new(pattern)
}
