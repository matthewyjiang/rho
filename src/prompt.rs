use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{skills, tool::ToolSpec};

pub const BASE_SYSTEM_PROMPT: &str = r#"You are a coding agent in the rho coding-agent harness, working with the user in a shared workspace. Use available tools to inspect files, run commands, and edit or create files.

Match actions to the request: for reviews or diagnoses, inspect and explain; for implementations or fixes, modify files. Continue until resolved. Make reasonable in-scope assumptions, but ask when a missing decision would materially affect the result or require new authority.

During substantial work, give concise progress updates. Preserve existing work and unrelated changes. Never run destructive commands unless explicitly requested. Verify changes in proportion to risk, then report the outcome and any remaining concerns."#;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSourceKind {
    Base,
    Agents,
    Skills,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptSource {
    pub kind: PromptSourceKind,
    pub path: Option<String>,
    pub bytes: usize,
}

pub struct SystemPrompt {
    pub text: String,
    pub sources: Vec<PromptSource>,
}

pub fn system_prompt(tools: &[ToolSpec], cwd: &Path) -> SystemPrompt {
    let home = crate::paths::home_dir();
    system_prompt_with_home(tools, cwd, home.as_deref())
}

fn system_prompt_with_home(tools: &[ToolSpec], cwd: &Path, home: Option<&Path>) -> SystemPrompt {
    let mut text = BASE_SYSTEM_PROMPT.to_string();
    text.push_str(
        r#"
Use tools only when needed. For questions answerable from context, reply directly.
Web access is available through tool schemas; invoke it only when needed and retrieve stored content handles selectively.

Use structured tool calls when available. Do not write tool calls in prose.

Do not invent tool results. When done, answer directly.
"#,
    );
    let mut sources = vec![PromptSource {
        kind: PromptSourceKind::Base,
        path: None,
        bytes: text.len(),
    }];

    let agent_instructions = agent_instruction_files(cwd, home);
    if !agent_instructions.is_empty() {
        let start = text.len();
        text.push_str(
            "\nAdditional instructions from AGENTS.md files follow. More specific files appear later and take precedence:\n",
        );
        sources[0].bytes += text.len() - start;
        for (path, contents) in agent_instructions {
            let start = text.len();
            push_context_file(&mut text, "agents_instructions", &path, &contents);
            sources.push(PromptSource {
                kind: PromptSourceKind::Agents,
                path: Some(path.display().to_string()),
                bytes: text.len() - start,
            });
        }
    }

    let skills = if tools.iter().any(|tool| tool.name == "skill") {
        skills::discover_with_home(cwd, home)
    } else {
        Vec::new()
    };
    if !skills.is_empty() {
        let start = text.len();
        text.push_str("\nAvailable skills from skill files, in discovery order:\n");
        text.push_str("Use the skill tool to load a skill when the task matches its description. If a skill references relative paths, resolve them against the skill directory.\n");
        text.push_str("<available_skills>\n");
        for skill in skills {
            text.push_str("  <skill>\n");
            text.push_str("    <name>");
            text.push_str(&skill.name);
            text.push_str("</name>\n");
            text.push_str("    <description>");
            text.push_str(&skill.description);
            text.push_str("</description>\n");
            text.push_str("    <source>");
            text.push_str(&skill.source.to_string());
            text.push_str("</source>\n");
            text.push_str("  </skill>\n");
        }
        text.push_str("</available_skills>\n");
        sources.push(PromptSource {
            kind: PromptSourceKind::Skills,
            path: None,
            bytes: text.len() - start,
        });
    }

    SystemPrompt { text, sources }
}

fn push_context_file(out: &mut String, tag: &str, path: &Path, contents: &str) {
    out.push('\n');
    out.push('<');
    out.push_str(tag);
    out.push_str(" path=\"");
    out.push_str(&path.display().to_string());
    out.push_str("\">\n");
    out.push_str(contents.trim_end());
    out.push_str("\n</");
    out.push_str(tag);
    out.push_str(">\n");
}

fn agent_instruction_files(cwd: &Path, home: Option<&Path>) -> Vec<(PathBuf, String)> {
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(home.join(".rho").join("AGENTS.md"));
    }
    paths.extend(
        crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .map(|path| path.join("AGENTS.md")),
    );
    read_existing_files(paths)
}

fn read_existing_files(paths: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    paths
        .into_iter()
        .filter_map(|path| {
            if !path.is_file() {
                return None;
            }
            std::fs::read_to_string(&path)
                .ok()
                .map(|contents| (path, contents))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn includes_home_and_project_agents_files_in_order() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        std::fs::create_dir(home.path().join(".rho")).unwrap();
        std::fs::write(home.path().join(".rho").join("AGENTS.md"), "home rules").unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "project rules").unwrap();

        let prompt = system_prompt_with_home(&[], project.path(), Some(home.path())).text;

        let home_index = prompt.find("home rules").unwrap();
        let project_index = prompt.find("project rules").unwrap();
        assert!(home_index < project_index);
        assert!(prompt.contains(&format!(
            "path=\"{}\"",
            home.path().join(".rho").join("AGENTS.md").display()
        )));
        assert!(prompt.contains(&format!(
            "path=\"{}\"",
            project.path().join("AGENTS.md").display()
        )));
    }

    #[test]
    fn includes_parent_agents_files_before_child_agents_files() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let child = project.path().join("src/nested");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(home.path().join(".rho")).unwrap();
        std::fs::write(home.path().join(".rho").join("AGENTS.md"), "home rules").unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "project rules").unwrap();
        std::fs::write(child.join("AGENTS.md"), "nested rules").unwrap();

        let prompt = system_prompt_with_home(&[], &child, Some(home.path())).text;

        let home_index = prompt.find("home rules").unwrap();
        let project_index = prompt.find("project rules").unwrap();
        let nested_index = prompt.find("nested rules").unwrap();
        assert!(home_index < project_index);
        assert!(project_index < nested_index);
    }

    #[test]
    fn skips_missing_agents_files() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        let prompt = system_prompt_with_home(&[], project.path(), Some(home.path())).text;

        assert!(!prompt.contains("Additional instructions from AGENTS.md files"));
    }

    #[test]
    fn includes_valid_skills_with_names_and_descriptions() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let skill_dir = home.path().join(".rho/skills/rho-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rho-skill\ndescription: rho skill desc\n---\nrho skill rules",
        )
        .unwrap();

        let prompt =
            system_prompt_with_home(&[skill_tool_spec()], project.path(), Some(home.path())).text;

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>rho-skill</name>"));
        assert!(prompt.contains("<description>rho skill desc</description>"));
        assert!(prompt.contains(&format!(
            "<source>{}</source>",
            skill_dir.join("SKILL.md").display()
        )));
        assert!(!prompt.contains("rho skill rules"));
    }

    #[test]
    fn prompt_sources_partition_the_exact_system_prompt() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        std::fs::create_dir(home.path().join(".rho")).unwrap();
        std::fs::write(home.path().join(".rho/AGENTS.md"), "home rules").unwrap();
        let skill_dir = home.path().join(".rho/skills/rho-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rho-skill\ndescription: rho skill desc\n---\nrules",
        )
        .unwrap();

        let prompt =
            system_prompt_with_home(&[skill_tool_spec()], project.path(), Some(home.path()));

        assert_eq!(
            prompt
                .sources
                .iter()
                .map(|source| source.bytes)
                .sum::<usize>(),
            prompt.text.len()
        );
        assert!(prompt.sources[0].bytes > BASE_SYSTEM_PROMPT.len());
        assert_eq!(prompt.sources[0].kind, PromptSourceKind::Base);
        assert!(prompt
            .sources
            .iter()
            .any(|source| source.kind == PromptSourceKind::Agents));
        assert!(prompt
            .sources
            .iter()
            .any(|source| source.kind == PromptSourceKind::Skills));
    }

    #[test]
    fn keeps_web_access_guidance_concise_and_lazy() {
        let project = TempDir::new().unwrap();

        let prompt = system_prompt_with_home(&[], project.path(), None).text;

        assert!(prompt.contains("Web access is available through tool schemas"));
        assert!(!prompt.contains("GitHub URLs are cloned locally instead of scraped"));
        assert!(!prompt.contains("BRAVE_SEARCH_API_KEY"));
    }

    #[test]
    fn skips_skills_when_skill_tool_is_unavailable() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let skill_dir = home.path().join(".rho/skills/rho-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rho-skill\ndescription: rho skill desc\n---\nrho skill rules",
        )
        .unwrap();

        let prompt = system_prompt_with_home(&[], project.path(), Some(home.path())).text;

        assert!(!prompt.contains("<available_skills>"));
        assert!(!prompt.contains("rho-skill"));
    }

    fn skill_tool_spec() -> ToolSpec {
        ToolSpec {
            name: "skill".into(),
            description: "load skills".into(),
            input_schema: serde_json::json!({}),
        }
    }
}
