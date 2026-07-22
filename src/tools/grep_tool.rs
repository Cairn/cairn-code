use std::fs;
use std::path::Path;
use super::registry::Tool;

pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &str { "grep" }
    fn description(&self) -> &str { "Search file contents using regex" }
    fn needs_permission(&self) -> bool { false }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"pattern":{"type":"string"},"include":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let pattern = obj.get("pattern").and_then(|v| v.as_str()).ok_or("pattern required")?;
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

        let mut results = Vec::new();
        search_dir(Path::new(search_path), &re, include, "", &mut results)?;

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
        fs::write(dir.join("a.rs"), "fn main() {\n    println!(\"hello unique_token_xyz\");\n}\n").unwrap();
        fs::write(dir.join("b.txt"), "nope\n").unwrap();
        let tool = GrepTool;
        let input = format!(
            r#"{{"pattern":"unique_token_xyz","path":"{}"}}"#,
            dir.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("unique_token_xyz"), "{out}");
        assert!(out.contains("1 result") || out.contains("result(s)"), "{out}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn wildcard_star_matches() {
        let dir = temp_dir();
        fs::write(dir.join("x.txt"), "alpha-beta-gamma\n").unwrap();
        let tool = GrepTool;
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
        let tool = GrepTool;
        let input = format!(
            r#"{{"pattern":"definitely_not_present_zzz","path":"{}"}}"#,
            dir.to_string_lossy().replace('\\', "\\\\")
        );
        let out = tool.execute(&input).unwrap();
        assert!(out.contains("No matches"), "{out}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn requires_pattern() {
        assert!(GrepTool.execute(r#"{}"#).is_err());
    }
}

fn search_dir(
    dir: &Path,
    re: &SimpleRe,
    include: Option<&str>,
    relative: &str,
    results: &mut Vec<(String, usize, String)>,
) -> Result<(), String> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip .git, node_modules etc.
            if name == ".git" || name == "node_modules" || name == "target" {
                continue;
            }

            let rel = if relative.is_empty() { name.clone() } else { format!("{relative}/{name}") };

            if path.is_dir() {
                search_dir(&path, re, include, &rel, results)?;
            } else if path.is_file() {
                // Check include filter
                if let Some(inc) = include {
                    if !path.extension().map(|e| e.to_string_lossy().as_ref() == inc.trim_start_matches('.')).unwrap_or(false) {
                        if !rel.contains(inc) {
                            continue;
                        }
                    }
                }

                if let Ok(content) = fs::read_to_string(&path) {
                    for (i, line) in content.lines().enumerate() {
                        if re.is_match(line) {
                            results.push((rel.clone(), i + 1, line.to_string()));
                        }
                    }
                }
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
