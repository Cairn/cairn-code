//! `skill` tool: load a skill body by name into the conversation.

use super::registry::Tool;
use crate::skills::{self, Skill};

pub struct SkillTool {
    skills: Vec<Skill>,
}

impl SkillTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        SkillTool { skills }
    }
}

impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load a reusable skill pack by name. Use when an available skill's \
         description matches the task. Returns the full skill instructions."
    }

    fn needs_permission(&self) -> bool {
        false
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"name":{"type":"string","description":"Skill name from the available_skills catalog"}},"required":["name"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("name required")?;

        if let Some(skill) = skills::find_skill(&self.skills, name) {
            return Ok(format!(
                "# Skill: {}\n\n{}\n\n---\nSource: {}",
                skill.name,
                skill.content,
                skill.path.display()
            ));
        }

        if self.skills.is_empty() {
            return Err(
                "No skills available. Built-ins should always load; add custom SKILL.md packs \
                 under the skills directory (CAIRN_SKILLS_DIR or ~/.config/cairn-code/skills)."
                    .into(),
            );
        }
        let names: Vec<_> = self.skills.iter().map(|s| s.name.as_str()).collect();
        Err(format!(
            "Unknown skill {name:?}. Available: {}",
            names.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tool_with(name: &str, body: &str) -> SkillTool {
        SkillTool::new(vec![Skill {
            name: name.into(),
            description: "desc".into(),
            content: body.into(),
            path: PathBuf::from("t/SKILL.md"),
        }])
    }

    #[test]
    fn loads_known_skill() {
        let t = tool_with("demo", "Do X then Y.");
        let out = t.execute(r#"{"name":"demo"}"#).unwrap();
        assert!(out.contains("Do X then Y."));
        assert!(out.contains("Skill: demo"));
    }

    #[test]
    fn unknown_lists_available() {
        let t = tool_with("demo", "body");
        let err = t.execute(r#"{"name":"nope"}"#).unwrap_err();
        assert!(err.contains("demo"));
        assert!(err.contains("Unknown"));
    }
}
