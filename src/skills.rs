//! On-demand skill packs (Claude Code / zero style).
//!
//! Skills live as `*/SKILL.md` under a skills root. Only a short catalog is
//! injected into the system prompt; the full body is returned when the model
//! calls the `skill` tool with `{ "name": "..." }`.

use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub path: PathBuf,
}

/// Primary skills directory.
///
/// Order: `CAIRN_SKILLS_DIR` → `~/.local/share/cairn-code/skills` (Unix) /
/// `%USERPROFILE%\.local\share\cairn-code\skills` (Windows fallback) →
/// `~/.config/cairn-code/skills`.
pub fn default_skills_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CAIRN_SKILLS_DIR") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    let home = PathBuf::from(home);
    let xdg = home
        .join(".local")
        .join("share")
        .join("cairn-code")
        .join("skills");
    if xdg.exists() {
        return xdg;
    }
    home.join(".config").join("cairn-code").join("skills")
}

/// Optional shared agents skills root (`~/.agents/skills`), if present.
pub fn agents_skills_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let p = PathBuf::from(home).join(".agents").join("skills");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// Load skills from default roots (primary first; later roots fill name gaps),
/// then fill remaining name gaps from the built-in pack.
pub fn load_skills() -> Vec<Skill> {
    let mut roots = vec![default_skills_dir()];
    if let Some(a) = agents_skills_dir() {
        roots.push(a);
    }
    with_builtins(load_from_roots(&roots))
}

/// Built-in skill packs shipped with the binary (always available).
///
/// Disk skills with the same `name` win; builtins only fill gaps.
pub fn builtin_skills() -> Vec<Skill> {
    const ROAST_ME: &str = include_str!("../skills/roast-me/SKILL.md");
    let mut out = Vec::new();
    if let Some(skill) = parse_skill(
        ROAST_ME,
        "roast-me",
        PathBuf::from("builtin:roast-me/SKILL.md"),
    ) {
        out.push(skill);
    }
    out
}

/// Merge disk-loaded skills with builtins. Existing names are kept (disk wins).
pub fn with_builtins(mut disk: Vec<Skill>) -> Vec<Skill> {
    let mut seen: std::collections::HashSet<String> =
        disk.iter().map(|s| s.name.clone()).collect();
    for skill in builtin_skills() {
        if seen.insert(skill.name.clone()) {
            disk.push(skill);
        }
    }
    disk.sort_by(|a, b| a.name.cmp(&b.name));
    disk
}

pub fn load_from_roots(roots: &[PathBuf]) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let mut names: Vec<_> = entries.flatten().collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&skill_md) else {
                continue;
            };
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let Some(mut skill) = parse_skill(&raw, &dir_name, skill_md) else {
                continue;
            };
            if skill.name.is_empty() {
                skill.name = dir_name;
            }
            if !seen.insert(skill.name.clone()) {
                continue;
            }
            out.push(skill);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse SKILL.md with optional YAML-ish frontmatter (`name`, `description` only).
pub fn parse_skill(raw: &str, default_name: &str, path: PathBuf) -> Option<Skill> {
    let (front, body) = split_frontmatter(raw);
    let mut name = default_name.to_string();
    let mut description = String::new();
    if let Some(fm) = front {
        for line in fm.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("name:") {
                let v = unquote(rest.trim());
                if !v.is_empty() {
                    name = v;
                }
            } else if let Some(rest) = line.strip_prefix("description:") {
                description = unquote(rest.trim());
            }
        }
    }
    let content = body.trim().to_string();
    if content.is_empty() && description.is_empty() {
        return None;
    }
    if description.is_empty() {
        // First non-empty line of body as a weak description.
        description = content
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("skill")
            .chars()
            .take(120)
            .collect();
    }
    Some(Skill {
        name,
        description,
        content,
        path,
    })
}

fn split_frontmatter(raw: &str) -> (Option<String>, String) {
    let (first_line, rest_after_first) = if let Some(rest) = raw.strip_prefix("---\r\n") {
        ("---", rest)
    } else if let Some(rest) = raw.strip_prefix("---\n") {
        ("---", rest)
    } else {
        return (None, raw.to_string());
    };
    let _ = first_line;

    let mut curr = rest_after_first;
    let mut front_len = 0;
    loop {
        if curr.starts_with("---\r\n")
            || curr.starts_with("---\n")
            || curr == "---"
            || curr == "---\r"
        {
            let front = rest_after_first[..front_len]
                .trim_end_matches(['\r', '\n'])
                .to_string();
            let after_idx = if curr.starts_with("---\r\n") {
                5
            } else if curr.starts_with("---\n") {
                4
            } else {
                curr.len()
            };
            let after = curr[after_idx..].to_string();
            return (Some(front), after);
        }
        if let Some(next_line_idx) = curr.find('\n') {
            let next_start = next_line_idx + 1;
            front_len += next_start;
            curr = &curr[next_start..];
        } else {
            break;
        }
    }
    (None, raw.to_string())
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

/// Compact catalog for the system prompt (not full bodies).
pub fn catalog_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<available_skills>\n\
         Reusable, on-demand instruction packs. Load one with the `skill` tool \
         when its description matches the task.\n",
    );
    for s in skills {
        out.push_str(&format!("- {}: {}\n", s.name, s.description));
    }
    out.push_str("</available_skills>");
    out
}

/// Resolve a skill by name (case-sensitive first, then case-insensitive).
pub fn find_skill<'a>(skills: &'a [Skill], name: &str) -> Option<&'a Skill> {
    skills.iter().find(|s| s.name == name).or_else(|| {
        let lower = name.to_ascii_lowercase();
        skills.iter().find(|s| s.name.to_ascii_lowercase() == lower)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_name_and_description() {
        let raw = "---\nname: demo\ndescription: Do the thing\n---\n\n# Body\n\nStep 1.\n";
        let s = parse_skill(raw, "folder", PathBuf::from("x")).unwrap();
        assert_eq!(s.name, "demo");
        assert_eq!(s.description, "Do the thing");
        assert!(s.content.contains("Step 1"));
    }

    #[test]
    fn parse_without_frontmatter_uses_dir_name() {
        let raw = "# Hello\n\nInstructions here.\n";
        let s = parse_skill(raw, "hello-skill", PathBuf::from("x")).unwrap();
        assert_eq!(s.name, "hello-skill");
        assert!(s.description.contains("Hello") || s.description.contains("Instructions"));
        assert!(s.content.contains("Instructions"));
    }

    #[test]
    fn catalog_lists_skills() {
        let skills = vec![Skill {
            name: "a".into(),
            description: "A skill".into(),
            content: "body".into(),
            path: PathBuf::from("a"),
        }];
        let cat = catalog_prompt(&skills);
        assert!(cat.contains("<available_skills>"));
        assert!(cat.contains("- a: A skill"));
    }

    #[test]
    fn load_from_temp_root() {
        let root = std::env::temp_dir().join(format!("cairn-skills-test-{}", std::process::id()));
        let skill_dir = root.join("greet");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Say hello\n---\n\nAlways greet the user.\n",
        )
        .unwrap();
        let skills = load_from_roots(&[root.clone()]);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "greet");
        assert_eq!(skills[0].description, "Say hello");
        assert!(skills[0].content.contains("greet"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_split_frontmatter_strict_boundaries() {
        assert_eq!(split_frontmatter("---text\nfoo\n---").0, None);
        assert_eq!(split_frontmatter("  ---\nfoo\n---").0, None);
        let (front, body) = split_frontmatter("---\nname: foo\n---\nbody text");
        assert_eq!(front.unwrap(), "name: foo");
        assert_eq!(body, "body text");
        let (front_crlf, body_crlf) = split_frontmatter("---\r\nname: foo\r\n---\r\nbody text");
        assert_eq!(front_crlf.unwrap(), "name: foo");
        assert_eq!(body_crlf, "body text");
    }

    #[test]
    fn find_skill_case_insensitive() {
        let skills = vec![Skill {
            name: "MySkill".into(),
            description: "d".into(),
            content: "c".into(),
            path: PathBuf::from("p"),
        }];
        assert!(find_skill(&skills, "myskill").is_some());
        assert!(find_skill(&skills, "nope").is_none());
    }

    #[test]
    fn builtin_roast_me_present_when_disk_empty() {
        let skills = with_builtins(Vec::new());
        let s = find_skill(&skills, "roast-me").expect("builtin roast-me");
        assert!(s.content.contains("Roast Me") || s.content.contains("# Roast"));
        assert!(s.description.to_ascii_lowercase().contains("architecture")
            || s.description.to_ascii_lowercase().contains("constructive"));
        assert!(s.path.to_string_lossy().starts_with("builtin:"));
    }

    #[test]
    fn disk_skill_overrides_builtin_by_name() {
        let disk = vec![Skill {
            name: "roast-me".into(),
            description: "Disk override".into(),
            content: "Custom disk body for roast-me.".into(),
            path: PathBuf::from("/tmp/disk-roast-me/SKILL.md"),
        }];
        let skills = with_builtins(disk);
        let s = find_skill(&skills, "roast-me").unwrap();
        assert_eq!(s.description, "Disk override");
        assert!(s.content.contains("Custom disk body"));
        assert!(!s.path.to_string_lossy().starts_with("builtin:"));
        // Builtin still fills other names only; roast-me must appear once.
        assert_eq!(
            skills.iter().filter(|s| s.name == "roast-me").count(),
            1
        );
    }

    #[test]
    fn parse_vendored_roast_me_skill_md() {
        let raw = include_str!("../skills/roast-me/SKILL.md");
        let s = parse_skill(raw, "roast-me", PathBuf::from("skills/roast-me/SKILL.md")).unwrap();
        assert_eq!(s.name, "roast-me");
        assert!(!s.description.is_empty());
        assert!(s.content.contains("Operating Mode") || s.content.contains("Initial Setup"));
    }
}
