use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub description: String,
}

/// Lists skills under `.agents/skills/<id>/SKILL.md`.
pub fn discover_skills(workspace: &Path) -> Vec<SkillInfo> {
    let root = workspace.join(".agents").join("skills");
    let mut skills = Vec::new();
    let entries = fs::read_dir(&root);
    let Ok(entries) = entries else {
        return skills;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("skill")
            .to_owned();
        let contents = fs::read_to_string(&skill_md).unwrap_or_default();
        let (name, description) = parse_frontmatter(&contents, &id);
        skills.push(SkillInfo {
            id,
            name,
            path: skill_md,
            description,
        });
    }
    skills.sort_by(|left, right| left.id.cmp(&right.id));
    skills
}

pub fn skill_prompt_prefix(skill: &SkillInfo) -> String {
    format!(
        "[Skill: {name}]\n{description}\n\n",
        name = skill.name,
        description = skill.description
    )
}

fn parse_frontmatter(contents: &str, fallback_id: &str) -> (String, String) {
    if !contents.starts_with("---") {
        return (fallback_id.to_owned(), String::new());
    }
    let Some(end) = contents[3..].find("\n---") else {
        return (fallback_id.to_owned(), String::new());
    };
    let yaml = &contents[3..3 + end];
    let mut name = fallback_id.to_owned();
    let mut description = String::new();
    for line in yaml.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            name = value.trim().trim_matches('"').to_owned();
        }
        if let Some(value) = line.strip_prefix("description:") {
            description = value.trim().trim_matches('"').to_owned();
        }
    }
    (name, description)
}
