//! Agent Skills discovery and SKILL.md parsing.
//!
//! Frontmatter follows the Agent Skills specification (name, description,
//! license, compatibility, metadata, allowed-tools) and is parsed with a real
//! YAML parser. `disable-model-invocation` is a Rho extension outside the
//! Agent Skills field set; it stays opt-in per skill and is enforced only by
//! prompt metadata injection.
//!
//! Precedence, highest first; the first source for a name wins and conflicts
//! are reported instead of hidden:
//!
//! 1. built-in skills
//! 2. loose user skills: `~/.rho/skills`, then `~/.agents/skills`
//! 3. loose project skills: nearest `.agents/skills` ancestor first
//! 4. project Agent Plugins skills: nearest `.agents/plugins` ancestor first
//! 5. user Agent Plugins skills: `~/.agents/plugins`

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillSource {
    BuiltIn,
    Filesystem {
        skill_file: PathBuf,
        owner: Option<String>,
    },
}

impl SkillSource {
    fn file(skill_file: PathBuf) -> Self {
        Self::Filesystem {
            skill_file,
            owner: None,
        }
    }

    pub(crate) fn plugin(skill_file: PathBuf, plugin: String) -> Self {
        Self::Filesystem {
            skill_file,
            owner: Some(plugin),
        }
    }
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuiltIn => formatter.write_str("built in to rho"),
            Self::Filesystem {
                skill_file,
                owner: None,
            } => formatter.write_str(&crate::paths::display(skill_file)),
            Self::Filesystem {
                skill_file,
                owner: Some(owner),
            } => write!(
                formatter,
                "plugin {owner} ({})",
                crate::paths::display(skill_file.parent().unwrap_or(skill_file))
            ),
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
    include_str!("builtin_skills/rho-config/SKILL.md"),
    include_str!("builtin_skills/rho-diagnostics/SKILL.md"),
    include_str!("builtin_skills/rho-agent-creator/SKILL.md"),
    include_str!("builtin_skills/rho-workflow-authoring/SKILL.md"),
];

pub fn discover(cwd: &Path) -> Vec<Skill> {
    let home = crate::paths::home_dir();
    discover_with_home(cwd, home.as_deref())
}

pub(crate) fn find_builtin(name: &str) -> Option<Skill> {
    builtin_skills()
        .into_iter()
        .find(|skill| skill.name == name)
}

pub fn discover_with_home(cwd: &Path, home: Option<&Path>) -> Vec<Skill> {
    let plugin_skills = crate::plugins::skills_by_precedence(cwd, home);
    discover_with_plugin_skills(cwd, home, plugin_skills)
}

pub(crate) fn discover_with_plugin_skills(
    cwd: &Path,
    home: Option<&Path>,
    plugin_skills: Vec<Skill>,
) -> Vec<Skill> {
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

    // Candidates arrive in precedence order: built-ins, loose user, loose
    // project, then plugin skills (project plugins before user plugins).
    let mut candidates = builtin_skills();
    candidates.extend(
        roots
            .into_iter()
            .flat_map(|root| skill_paths(&root))
            .filter_map(|path| match read_skill(&path) {
                Ok(skill) => Some(skill),
                Err(error) => {
                    tracing::warn!(skill = %path.display(), error = %error, "skipping invalid skill");
                    None
                }
            }),
    );
    candidates.extend(plugin_skills);

    let mut skills: Vec<Skill> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if let Some(winner) = skills.iter().find(|skill| skill.name == candidate.name) {
            tracing::warn!(
                skill = %candidate.name,
                selected = %winner.source,
                ignored = %candidate.source,
                "duplicate skill name; keeping the higher-precedence source"
            );
            continue;
        }
        skills.push(candidate);
    }
    // Sort after precedence/dedup so every presentation surface shares one order.
    skills.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    skills
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
    parse_skill(&contents, SkillSource::file(path.to_path_buf()), Some(path))
}

fn builtin_skills() -> Vec<Skill> {
    BUILTIN_SKILLS
        .iter()
        .map(|contents| read_builtin_skill(contents).expect("embedded skills must be valid"))
        .collect()
}

fn read_builtin_skill(contents: &str) -> anyhow::Result<Skill> {
    parse_skill(contents, SkillSource::BuiltIn, None)
}

/// Parse and validate one SKILL.md document.
///
/// `skill_path` enables the Agent Skills rule that `name` must match the
/// containing directory; built-in skills have no directory and skip it.
pub(crate) fn parse_skill(
    contents: &str,
    source: SkillSource,
    skill_path: Option<&Path>,
) -> anyhow::Result<Skill> {
    let block = frontmatter_block(contents)?;
    let frontmatter: SkillFrontmatter = serde_yaml_ng::from_str(&block)
        .map_err(|error| anyhow::anyhow!("invalid SKILL.md frontmatter: {error}"))?;
    validate_frontmatter(&frontmatter)?;
    if let Some(path) = skill_path {
        let directory_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("missing skill directory name"))?;
        if frontmatter.name != directory_name {
            anyhow::bail!("skill name must match directory name");
        }
    }

    Ok(Skill {
        name: frontmatter.name,
        description: frontmatter.description,
        disable_model_invocation: frontmatter
            .disable_model_invocation
            .map(|flag| flag.0)
            .unwrap_or(false),
        source,
        contents: contents.into(),
    })
}

/// The YAML block between the opening and closing `---` frontmatter fences.
fn frontmatter_block(contents: &str) -> anyhow::Result<String> {
    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        anyhow::bail!("SKILL.md must start with YAML frontmatter");
    }
    let mut block = Vec::new();
    for line in lines {
        if line == "---" {
            return Ok(block.join("\n"));
        }
        block.push(line.to_string());
    }
    anyhow::bail!("unterminated YAML frontmatter")
}

/// Typed Agent Skills frontmatter plus Rho's `disable-model-invocation`
/// extension. Unknown fields are ignored, matching prior behavior and the
/// ecosystem convention that clients may add their own frontmatter fields.
#[derive(serde::Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: Option<BTreeMap<String, String>>,
    #[serde(rename = "allowed-tools", default)]
    allowed_tools: Option<String>,
    #[serde(rename = "disable-model-invocation", default)]
    disable_model_invocation: Option<TolerantBool>,
}

/// Accepts YAML booleans plus the legacy string forms `"true"`/`"false"`
/// (case-insensitive) that the earlier hand-written parser tolerated.
#[derive(Clone, Copy)]
struct TolerantBool(bool);

impl<'de> serde::Deserialize<'de> for TolerantBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = TolerantBool;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean or the string \"true\"/\"false\"")
            }

            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<TolerantBool, E> {
                Ok(TolerantBool(value))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<TolerantBool, E> {
                match value.to_ascii_lowercase().as_str() {
                    "true" => Ok(TolerantBool(true)),
                    "false" => Ok(TolerantBool(false)),
                    _ => Err(serde::de::Error::custom(
                        "disable-model-invocation must be true or false",
                    )),
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// Agent Skills field constraints, separate from YAML syntax parsing.
fn validate_frontmatter(frontmatter: &SkillFrontmatter) -> anyhow::Result<()> {
    validate_name(&frontmatter.name)?;
    let description_chars = frontmatter.description.chars().count();
    if description_chars == 0 || description_chars > 1024 {
        anyhow::bail!("skill description must be 1-1024 characters");
    }
    if let Some(compatibility) = &frontmatter.compatibility {
        let compatibility_chars = compatibility.chars().count();
        if compatibility_chars == 0 || compatibility_chars > 500 {
            anyhow::bail!("skill compatibility must be 1-500 characters");
        }
    }
    // `license`, `metadata` (string keys and values), and `allowed-tools`
    // (space-separated tool list) are constrained by their deserialized types.
    let _ = (
        &frontmatter.license,
        &frontmatter.metadata,
        &frontmatter.allowed_tools,
    );
    Ok(())
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn discovers_embedded_rho_config_skill() {
        let root = TempDir::new().unwrap();

        let skills = discover_with_home(root.path(), None);
        let skill = skills
            .iter()
            .find(|skill| skill.name == "rho-config")
            .unwrap();

        assert_eq!(skill.source, SkillSource::BuiltIn);
        assert!(skill.contents.contains("config.toml"));
    }

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
                "agent-skill",
                "project-skill",
                "rho-agent-creator",
                "rho-config",
                "rho-diagnostics",
                "rho-skill",
                "rho-workflow-authoring",
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
    fn rejects_invalid_discovered_skills() {
        let cases = [
            ("bad-skill", "# bad", "bad-skill"),
            (
                "dir-name",
                "---\nname: other-name\ndescription: desc\n---\n",
                "other-name",
            ),
            (
                "bad--skill",
                "---\nname: bad--skill\ndescription: desc\n---\n",
                "bad--skill",
            ),
            (
                "bad-skill",
                "---\nname: bad-skill\ndescription: \n---\n",
                "bad-skill",
            ),
        ];

        for (directory, contents, rejected) in cases {
            let root = TempDir::new().unwrap();
            let skill_dir = root.path().join(".rho/skills").join(directory);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();

            let skills = discover_with_home(root.path(), Some(root.path()));
            assert_only_builtins_excluding(&skills, rejected);
        }
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

    /// Table-driven frontmatter coverage for the YAML parser and the Agent
    /// Skills validation pass.
    #[test]
    fn parses_frontmatter_variants() {
        #[derive(Debug)]
        struct Case {
            name: &'static str,
            frontmatter: &'static str,
            expected: Result<Expected, &'static str>,
        }

        #[derive(Debug)]
        struct Expected {
            name: &'static str,
            description: &'static str,
            disable_model_invocation: bool,
        }

        let cases = [
            Case {
                name: "double quoted with escapes",
                frontmatter: r#"name: quote-skill
description: "a \"quoted\" description"
"#,
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "a \"quoted\" description",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "single quoted",
                frontmatter: "name: 'quote-skill'\ndescription: 'plain'\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "plain",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "trailing comments",
                frontmatter: "name: quote-skill # the name\ndescription: desc # the desc\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "desc",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "folded multiline description",
                frontmatter: "name: quote-skill\ndescription: >-\n  first\n  second\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "first second",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "literal multiline description",
                frontmatter: "name: quote-skill\ndescription: |-\n  first\n  second\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "first\nsecond",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "nested metadata and optional fields",
                frontmatter: "name: quote-skill\ndescription: desc\nlicense: MIT\ncompatibility: needs git\nmetadata:\n  author: example-org\n  version: \"1.0\"\nallowed-tools: Bash(git:*) Read\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "desc",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "disable-model-invocation string legacy form",
                frontmatter: "name: quote-skill\ndescription: desc\ndisable-model-invocation: \"True\"\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "desc",
                    disable_model_invocation: true,
                }),
            },
            Case {
                name: "unknown fields ignored",
                frontmatter: "name: quote-skill\ndescription: desc\nsome-client-field: 1\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "desc",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "missing description",
                frontmatter: "name: quote-skill\n",
                expected: Err("description"),
            },
            Case {
                name: "metadata value must not be a mapping",
                frontmatter: "name: quote-skill\ndescription: desc\nmetadata:\n  nested:\n    deep: x\n",
                expected: Err("metadata"),
            },
            Case {
                name: "numeric metadata scalar coerces to its string form",
                frontmatter: "name: quote-skill\ndescription: desc\nmetadata:\n  version: 1.0\n",
                expected: Ok(Expected {
                    name: "quote-skill",
                    description: "desc",
                    disable_model_invocation: false,
                }),
            },
            Case {
                name: "invalid disable-model-invocation type",
                frontmatter: "name: quote-skill\ndescription: desc\ndisable-model-invocation: 3\n",
                expected: Err("disable-model-invocation"),
            },
            Case {
                name: "invalid compatibility type",
                frontmatter: "name: quote-skill\ndescription: desc\ncompatibility: [git]\n",
                expected: Err("compatibility"),
            },
        ];
        for case in cases {
            let contents = format!("---\n{}---\n# body\n", case.frontmatter);
            let result = parse_skill(&contents, SkillSource::BuiltIn, None);
            match &case.expected {
                Ok(expected) => {
                    let skill = result.unwrap_or_else(|error| {
                        panic!("{}: expected success, got {error}", case.name)
                    });
                    assert_eq!(skill.name, expected.name, "{}", case.name);
                    assert_eq!(skill.description, expected.description, "{}", case.name);

                    assert_eq!(
                        skill.disable_model_invocation, expected.disable_model_invocation,
                        "{}",
                        case.name
                    );
                }
                Err(fragment) => {
                    let error = result
                        .err()
                        .unwrap_or_else(|| panic!("{}: expected failure", case.name));
                    assert!(
                        error.to_string().contains(fragment),
                        "{}: error `{error}` should mention `{fragment}`",
                        case.name
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_overlong_compatibility() {
        let compatibility = "aaaaaaaaaa ".repeat(60);
        assert!(compatibility.chars().count() > 500);
        let contents = format!(
            "---\nname: quote-skill\ndescription: desc\ncompatibility: {compatibility}\n---\n"
        );
        let error = parse_skill(&contents, SkillSource::BuiltIn, None).unwrap_err();
        assert!(error.to_string().contains("compatibility"));
    }

    /// Asserts the discovered skills are exactly the built-ins, minus the one
    /// rejected skill. Derived from `builtin_skills()` so this stays correct
    /// as built-ins are added or removed without editing every rejection test.
    fn assert_only_builtins_excluding(skills: &[Skill], rejected: &str) {
        let mut expected: Vec<_> = builtin_skills()
            .into_iter()
            .filter(|skill| skill.name != rejected)
            .map(|skill| skill.name)
            .collect();
        expected.sort_by_key(|a| a.to_ascii_lowercase());

        let mut actual: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();
        actual.sort_by_key(|a| a.to_ascii_lowercase());

        assert_eq!(actual, expected);
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
