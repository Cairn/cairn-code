use std::fs;
use std::path::Path;
use super::registry::Tool;

pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &str { "glob" }
    fn description(&self) -> &str { "Find files matching a glob pattern (supports **, *)" }
    fn needs_permission(&self) -> bool { false }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let pattern = obj.get("pattern").and_then(|v| v.as_str()).ok_or("pattern required")?;

        let results = glob_match(pattern, ".")?;

        if results.is_empty() {
            return Ok("No matches found.".into());
        }

        let mut result = String::new();
        for path in &results {
            result.push_str(path);
            result.push('\n');
        }
        result.push_str(&format!("{} result(s)", results.len()));
        Ok(result)
    }
}

fn glob_match(pattern: &str, base_dir: &str) -> Result<Vec<String>, String> {
    let mut results = Vec::new();
    let base = Path::new(base_dir);

    // Split pattern into parts
    let parts: Vec<&str> = pattern.split('/').collect();

    // If pattern starts with **, we just need to match the rest
    if parts.len() == 1 && !parts[0].contains('*') && !parts[0].contains('?') {
        let path = base.join(parts[0]);
        if path.exists() {
            results.push(path.to_string_lossy().to_string());
        }
        return Ok(results);
    }

    walk_pattern(base, &parts, 0, String::new(), &mut results)?;

    Ok(results)
}

fn walk_pattern(
    dir: &Path,
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
            walk_dir_recursive(dir, &prefix, results)?;
            return Ok(());
        }

        // ** followed by more pattern - try at current level and subdirs
        walk_pattern(dir, rest, 0, prefix.clone(), results)?;

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let new_prefix = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                    walk_pattern(&path, parts, idx, new_prefix, results)?;
                }
            }
        }
    } else if part.contains('*') || part.contains('?') {
        // Wildcard pattern
        let re_pattern = format!("^{}$", part.replace('.', "\\.").replace('*', ".*").replace('?', "."));
        let re = regex_wrapper(&re_pattern)?;

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if re.is_match(&name) {
                    let new_prefix = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
                    let path = entry.path();
                    if is_last || path.is_dir() {
                        results.push(new_prefix);
                    }
                    if !is_last && path.is_dir() {
                        walk_pattern(&path, parts, idx + 1, if prefix.is_empty() { name } else { format!("{prefix}/{name}") }, results)?;
                    }
                }
            }
        }
    } else {
        // Literal component
        let child = dir.join(part);
        let new_prefix = if prefix.is_empty() { part.to_string() } else { format!("{prefix}/{part}") };

        if is_last {
            if child.exists() {
                results.push(new_prefix);
            }
        } else if child.is_dir() {
            walk_pattern(&child, parts, idx + 1, new_prefix, results)?;
        }
    }

    Ok(())
}

fn walk_dir_recursive(dir: &Path, prefix: &str, results: &mut Vec<String>) -> Result<(), String> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let new_prefix = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            results.push(new_prefix);
            if path.is_dir() {
                walk_dir_recursive(&path, &format!("{prefix}/{name}"), results)?;
            }
        }
    }
    Ok(())
}

struct SimpleRe {
    pattern: Vec<Segment>,
}

enum Segment {
    Literal(String),
    AnySeq,     // .*
    AnyChar,    // .
}

impl SimpleRe {
    fn new(pattern: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
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
