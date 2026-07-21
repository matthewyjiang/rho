use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillSource {
    BuiltIn,
    File(PathBuf),
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuiltIn => formatter.write_str("built in to rho"),
            Self::File(path) => formatter.write_str(&crate::paths::display(path)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub disable_model_invocation: bool,
    pub source: SkillSource,
    pub contents: String,
}

const BUILTIN_SKILLS: &[&str] = &[
    include_str!("builtin_skills/rho-diagnostics/SKILL.md"),
    include_str!("builtin_skills/rho-agent-creator/SKILL.md"),
];

pub fn discover(cwd: &Path) -> Vec<Skill> {
    let home = crate::paths::home_dir();
    discover_with_home(cwd, home.as_deref())
}

pub fn discover_with_home(cwd: &Path, home: Option<&Path>) -> Vec<Skill> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".rho").join("skills"));
        roots.push(home.join(".agents").join("skills"));
    }
    roots.extend(
        crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .rev()
            .map(|path| path.join(".agents").join("skills")),
    );

    let mut seen = HashSet::new();
    let mut discovered = BUILTIN_SKILLS
        .iter()
        .map(|contents| read_builtin_skill(contents).expect("embedded skills must be valid"))
        .collect::<Vec<_>>();
    discovered.extend(
        roots
            .into_iter()
            .flat_map(|root| skill_paths(&root))
            .filter_map(|path| read_skill(&path).ok()),
    );
    discovered
        .into_iter()
        .filter(|skill| seen.insert(skill.name.clone()))
        .collect()
}

fn skill_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                Some(path.join("SKILL.md"))
            } else {
                None
            }
        })
        .collect();
    paths.sort();
    paths
}

fn read_skill(path: &Path) -> anyhow::Result<Skill> {
    let contents = std::fs::read_to_string(path)?;
    parse_skill(&contents, SkillSource::File(path.to_path_buf()), Some(path))
}

fn read_builtin_skill(contents: &str) -> anyhow::Result<Skill> {
    parse_skill(contents, SkillSource::BuiltIn, None)
}

fn parse_skill(
    contents: &str,
    source: SkillSource,
    file_path: Option<&Path>,
) -> anyhow::Result<Skill> {
    let frontmatter = parse_frontmatter(contents)?;
    let name = frontmatter
        .iter()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("missing required name"))?;
    let description = frontmatter
        .iter()
        .find(|(key, _)| key == "description")
        .map(|(_, value)| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("missing required description"))?;

    let disable_model_invocation = frontmatter
        .iter()
        .find(|(key, _)| key == "disable-model-invocation")
        .map(|(_, value)| match value.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => anyhow::bail!("disable-model-invocation must be true or false"),
        })
        .transpose()?
        .unwrap_or(false);

    validate_name(&name)?;
    validate_description(&description)?;
    if let Some(path) = file_path {
        let directory_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("missing skill directory name"))?;
        if name != directory_name {
            anyhow::bail!("skill name must match directory name");
        }
    }

    Ok(Skill {
        name,
        description,
        disable_model_invocation,
        source,
        contents: contents.into(),
    })
}

fn parse_frontmatter(contents: &str) -> anyhow::Result<Vec<(String, String)>> {
    let lines: Vec<_> = contents.lines().collect();
    if lines.first().copied() != Some("---") {
        anyhow::bail!("SKILL.md must start with YAML frontmatter");
    }

    let mut fields = Vec::new();
    let mut index = 1;
    while index < lines.len() {
        let line = lines[index];
        if line == "---" {
            return Ok(fields);
        }
        index += 1;
        if line.starts_with(' ') || line.starts_with('\t') || line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if !matches!(
            key,
            "name" | "description" | "license" | "compatibility" | "disable-model-invocation"
        ) {
            continue;
        }

        let value = if let Some(block_style) = yaml_block_style(value) {
            let mut block_lines = Vec::new();
            while index < lines.len() {
                let block_line = lines[index];
                if block_line == "---" {
                    break;
                }
                if !block_line.starts_with(' ') && !block_line.starts_with('\t') {
                    break;
                }
                block_lines.push(block_line.trim());
                index += 1;
            }
            if block_style == '>' {
                block_lines.join(" ").trim().to_string()
            } else {
                block_lines.join("\n").trim().to_string()
            }
        } else {
            unquote_yaml_scalar(value)
        };
        fields.push((key.to_string(), value));
    }

    anyhow::bail!("unterminated YAML frontmatter")
}

fn yaml_block_style(value: &str) -> Option<char> {
    match value {
        "|" | "|-" | "|+" => Some('|'),
        ">" | ">-" | ">+" => Some('>'),
        _ => None,
    }
}

fn unquote_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("skill name must be 1-64 characters");
    }
    let bytes = name.as_bytes();
    if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') || name.contains("--") {
        anyhow::bail!("skill name must use single hyphen separators");
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        anyhow::bail!("skill name must be lowercase alphanumeric with hyphen separators");
    }
    Ok(())
}

fn validate_description(description: &str) -> anyhow::Result<()> {
    if description.is_empty() || description.len() > 1024 {
        anyhow::bail!("skill description must be 1-1024 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn discovers_embedded_rho_diagnostics_skill() {
        let root = TempDir::new().unwrap();

        let skills = discover_with_home(root.path(), None);
        let skill = skills
            .iter()
            .find(|skill| skill.name == "rho-diagnostics")
            .unwrap();

        assert_eq!(skill.source, SkillSource::BuiltIn);
        assert!(skill.contents.contains("Available actions:"));
    }

    #[test]
    fn discovers_embedded_rho_agent_creator_skill() {
        let root = TempDir::new().unwrap();

        let skills = discover_with_home(root.path(), None);
        let skill = skills
            .iter()
            .find(|skill| skill.name == "rho-agent-creator")
            .unwrap();

        assert_eq!(skill.source, SkillSource::BuiltIn);
        assert!(skill.contents.contains("questionnaire"));
    }

    #[test]
    fn parses_disable_model_invocation() {
        let root = TempDir::new().unwrap();
        let skill_dir = root.path().join(".agents/skills/manual-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: manual-skill\ndescription: manual skill\ndisable-model-invocation: true\n---\nrules\n",
        )
        .unwrap();

        let skill = discover_with_home(root.path(), None)
            .into_iter()
            .find(|skill| skill.name == "manual-skill")
            .unwrap();

        assert!(skill.disable_model_invocation);
    }

    #[test]
    fn discovers_valid_skills_in_order() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write_skill(
            home.path(),
            ".rho/skills/rho-skill",
            "rho-skill",
            "rho desc",
        );
        write_skill(
            home.path(),
            ".agents/skills/agent-skill",
            "agent-skill",
            "agent desc",
        );
        write_skill(
            project.path(),
            ".agents/skills/project-skill",
            "project-skill",
            "project desc",
        );

        let skills = discover_with_home(project.path(), Some(home.path()));

        let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "rho-diagnostics",
                "rho-agent-creator",
                "rho-skill",
                "agent-skill",
                "project-skill"
            ]
        );
    }

    #[test]
    fn discovers_project_skills_from_ancestor_directories() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let child = project.path().join("src/nested");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        write_skill(
            project.path(),
            ".agents/skills/project-skill",
            "project-skill",
            "project desc",
        );

        let skills = discover_with_home(&child, Some(home.path()));

        assert!(skills.iter().any(|skill| skill.name == "project-skill"));
    }

    #[test]
    fn prefers_nearest_project_skill_when_names_duplicate() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let child = project.path().join("src/nested");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        write_skill(
            project.path(),
            ".agents/skills/dup-skill",
            "dup-skill",
            "parent desc",
        );
        write_skill(
            &child,
            ".agents/skills/dup-skill",
            "dup-skill",
            "child desc",
        );

        let skills = discover_with_home(&child, Some(home.path()));

        let skill = skills
            .iter()
            .find(|skill| skill.name == "dup-skill")
            .unwrap();
        assert_eq!(skill.description, "child desc");
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let root = TempDir::new().unwrap();
        let skill_dir = root.path().join(".rho/skills/bad-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# bad").unwrap();

        let skills = discover_with_home(root.path(), Some(root.path()));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "rho-diagnostics");
        assert_eq!(skills[1].name, "rho-agent-creator");
    }

    #[test]
    fn rejects_name_that_does_not_match_directory() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), ".rho/skills/dir-name", "other-name", "desc");

        let skills = discover_with_home(root.path(), Some(root.path()));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "rho-diagnostics");
        assert_eq!(skills[1].name, "rho-agent-creator");
    }

    #[test]
    fn rejects_invalid_name_format() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), ".rho/skills/bad--skill", "bad--skill", "desc");

        let skills = discover_with_home(root.path(), Some(root.path()));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "rho-diagnostics");
        assert_eq!(skills[1].name, "rho-agent-creator");
    }

    #[test]
    fn rejects_empty_description() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), ".rho/skills/bad-skill", "bad-skill", "");

        let skills = discover_with_home(root.path(), Some(root.path()));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "rho-diagnostics");
        assert_eq!(skills[1].name, "rho-agent-creator");
    }

    #[test]
    fn parses_block_scalar_description() {
        let root = TempDir::new().unwrap();
        let skill_dir = root.path().join(".rho/skills/block-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: block-skill\ndescription: >\n  first line\n  second line\n---\n# block\n",
        )
        .unwrap();

        let skills = discover_with_home(root.path(), Some(root.path()));

        let skill = skills
            .iter()
            .find(|skill| skill.name == "block-skill")
            .unwrap();
        assert_eq!(skill.description, "first line second line");
    }

    #[test]
    fn parses_block_scalar_chomping_description() {
        let root = TempDir::new().unwrap();
        let skill_dir = root.path().join(".rho/skills/chomp-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: chomp-skill\ndescription: |-\n  first line\n  second line\n---\n# block\n",
        )
        .unwrap();

        let skills = discover_with_home(root.path(), Some(root.path()));

        let skill = skills
            .iter()
            .find(|skill| skill.name == "chomp-skill")
            .unwrap();
        assert_eq!(skill.description, "first line\nsecond line");
    }

    #[test]
    fn skips_duplicate_skill_names_after_first_match() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write_skill(
            home.path(),
            ".rho/skills/dup-skill",
            "dup-skill",
            "first desc",
        );
        write_skill(
            home.path(),
            ".agents/skills/dup-skill",
            "dup-skill",
            "second desc",
        );

        let skills = discover_with_home(project.path(), Some(home.path()));

        let duplicates: Vec<_> = skills
            .iter()
            .filter(|skill| skill.name == "dup-skill")
            .collect();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].description, "first desc");
    }

    fn write_skill(root: &Path, relative_dir: &str, name: &str, description: &str) {
        let skill_dir = root.join(relative_dir);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
        )
        .unwrap();
    }
}
