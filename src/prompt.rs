use std::path::{Path, PathBuf};

use crate::{skills, tool::ToolSpec};

pub const BASE_SYSTEM_PROMPT: &str = "You are an expert coding assistant operating inside rho, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.";

pub fn system_prompt(tools: &[ToolSpec], cwd: &Path) -> String {
    let home = crate::paths::home_dir();
    system_prompt_with_home(tools, cwd, home.as_deref())
}

fn system_prompt_with_home(tools: &[ToolSpec], cwd: &Path, home: Option<&Path>) -> String {
    let mut out = BASE_SYSTEM_PROMPT.to_string();
    out.push_str(
        r#"
Use tools only when needed. For questions answerable from context, reply directly.

Use structured tool calls when available. Do not write tool calls in prose.

Do not invent tool results. When done, answer directly.
"#,
    );

    let agent_instructions = agent_instruction_files(cwd, home);
    if !agent_instructions.is_empty() {
        out.push_str("\nAdditional instructions from AGENTS.md files, in precedence order:\n");
        for (path, contents) in agent_instructions {
            push_context_file(&mut out, "agents_instructions", &path, &contents);
        }
    }

    let skills = if tools.iter().any(|tool| tool.name == "skill") {
        skills::discover_with_home(cwd, home)
    } else {
        Vec::new()
    };
    if !skills.is_empty() {
        out.push_str("\nAvailable skills from skill files, in discovery order:\n");
        out.push_str("Use the skill tool to load a skill when the task matches its description. If a skill references relative paths, resolve them against the skill directory.\n");
        out.push_str("<available_skills>\n");
        for skill in skills {
            out.push_str("  <skill>\n");
            out.push_str("    <name>");
            out.push_str(&skill.name);
            out.push_str("</name>\n");
            out.push_str("    <description>");
            out.push_str(&skill.description);
            out.push_str("</description>\n");
            out.push_str("    <path>");
            out.push_str(&skill.path.display().to_string());
            out.push_str("</path>\n");
            out.push_str("  </skill>\n");
        }
        out.push_str("</available_skills>\n");
    }

    out
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

        let prompt = system_prompt_with_home(&[], project.path(), Some(home.path()));

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

        let prompt = system_prompt_with_home(&[], &child, Some(home.path()));

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

        let prompt = system_prompt_with_home(&[], project.path(), Some(home.path()));

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
            system_prompt_with_home(&[skill_tool_spec()], project.path(), Some(home.path()));

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>rho-skill</name>"));
        assert!(prompt.contains("<description>rho skill desc</description>"));
        assert!(prompt.contains(&format!(
            "<path>{}</path>",
            skill_dir.join("SKILL.md").display()
        )));
        assert!(!prompt.contains("rho skill rules"));
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

        let prompt = system_prompt_with_home(&[], project.path(), Some(home.path()));

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
