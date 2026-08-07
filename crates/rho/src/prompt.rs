use std::path::{Path, PathBuf};

use serde::Serialize;

use {crate::skills, rho_tools::tool::ToolSpec};

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

When structure is the point - architecture, control flow, state machines, request sequences, class or module relationships, or entity relationships - prefer a short Mermaid diagram over a long prose walkthrough. Always wrap valid Mermaid source in a closed `mermaid` fenced code block. Bare Mermaid source does not render.

Use only flowchart, stateDiagram, sequenceDiagram, classDiagram, or erDiagram. Keep diagrams small with short labels. Skip diagrams for routine edits, simple answers, linear checklists, or anything that mostly restates bullets. The interactive transcript also renders CommonMark.

For display math, use closed `$$ ... $$` blocks. The TUI renders a limited TeX subset (TXM), not full LaTeX: core commands (`\frac`, `\sqrt`, sums/integrals, Greek, `\mathbf`/`\mathrm`, `matrix`/`bmatrix`/`pmatrix`) work; prefer separate `$$` equations. Avoid `aligned`/`align`/`gather`, `\dfrac`, `\varepsilon`, and `\leq`/`\geq`/`\neq` (use `\frac`, `\epsilon`, `\le`/`\ge`/`\ne`). Keep formulas compact.

Inline `$...$` math renders only when it fits one text row: simple superscripts or subscripts (`x^2`, `a_i`), Greek letters, and symbols. Taller inline formulas (`\frac`, summation limits, mixed `x_i^2` scripts) stay raw source text, so put those in `$$` blocks instead.
"#,
    );
    if tools.iter().any(|tool| tool.name == "grep") {
        text.push_str(
            r#"
Prefer the `grep` tool over shell `rg` or `grep` for workspace content search. Content mode returns chainable `[path#TAG]` headers and match line numbers (`N | preview`) so you can target `edit` anchors. Match text is search preview only and may be truncated - copy TAG and line numbers, not preview bodies, into PUT rows; use `read_file` when you need exact line text. Use `files_with_matches` or `count` when you only need paths or tallies. Prefer `glob` over shell `fd` or `find` for file discovery when it is available.
"#,
        );
    }
    if tools.iter().any(|tool| tool.name == "edit") {
        text.push_str(
            r#"
Use `edit` (not shell or Python rewrites) for existing UTF-8 files once you have a fresh `[path#TAG]`. Copy locator forms and the PUT body/span contract from the tool description (`PUT 12:` never `PUT 12.:`). Put every hunk for one path in a single document; do not stack two `edit` calls on the same path in one batch. After a structural edit the tool returns TAG + ops summary without chainable body lines — re-read before further ops on that path. Prefer `write` only to create or fully rewrite a file.
"#,
        );
    }
    if tools.iter().any(|tool| tool.name == "agent") {
        text.push_str(
            r#"
Work directly by default. A subagent starts with fresh context and adds latency, token use, and coordination overhead. Delegate only a substantial, self-contained task when the saved work is likely to exceed that cost.

Do not delegate simple questions, routine codebase inspection, or small/local changes. Foreground agent calls wait for completion. Batching a foreground agent with other tools does not background it and can delay the rest of that batch until the run finishes. Independent agents in the same batch run together - when work is parallel, issue multiple agent calls in one turn rather than one after another. For work you can continue past without waiting, set background=true so the call returns an id immediately; completions arrive automatically. Subagents share the workspace, so avoid overlapping edits.
"#,
        );
    }

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
            .into_iter()
            .filter(|skill| !skill.disable_model_invocation)
            .collect()
    } else {
        Vec::new()
    };
    if !skills.is_empty() {
        let start = text.len();
        text.push_str("\nAvailable skills from skill files, in alphabetical order:\n");
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

pub fn append_subagents_disabled_instruction(text: &mut String) {
    text.push_str("\n\nAgent delegation is disabled. Do not attempt to delegate work.\n");
}

/// Tells the executor when to consult the `advisor` tool.
///
/// Appended only while advisor mode is active and an advisor model is set, so
/// the prompt never describes a tool the run does not have.
pub fn append_advisor_instruction(text: &mut String) {
    text.push_str(ADVISOR_INSTRUCTION);
}

const ADVISOR_INSTRUCTION: &str = "\n\n# Advisor\n\nYou have access to an `advisor` tool backed by a stronger reviewer model. It takes NO parameters. When you call advisor, your entire conversation history is forwarded automatically. The advisor sees the task, every tool call you have made, and every result you have seen.\n\nCall advisor BEFORE substantive work: before writing, before committing to an interpretation, before building on an assumption. If the task needs orientation first (finding files, fetching a source, seeing what is there), do that, then call advisor. Orientation is not substantive work. Writing, editing, and declaring an answer are.\n\nAlso call advisor:\n- When you believe the task is complete. BEFORE this call, make your deliverable durable: write the file, save the result, commit the change.\n- When stuck: errors recurring, approach not converging, results that do not fit.\n- When considering a change of approach.\n\nOn tasks longer than a few steps, call advisor at least once before committing to an approach and once before declaring done. On short reactive tasks where the next action follows from tool output you just read, you do not need to keep calling. The advisor adds most of its value on the first call, before the approach hardens.\n\nGive the advice serious weight. If you follow a step and it fails in practice, or you have primary-source evidence that contradicts a specific claim, adapt. If you have already retrieved data pointing one way and the advisor points another, do not switch silently: surface the conflict in one more advisor call.\n";

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
            crate::paths::display(&skill_dir.join("SKILL.md"))
        )));
        assert!(!prompt.contains("rho skill rules"));
    }

    #[test]
    fn excludes_skills_that_disable_model_invocation() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let skill_dir = home.path().join(".rho/skills/manual-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: manual-skill\ndescription: only users may invoke this skill\ndisable-model-invocation: true\n---\nmanual rules",
        )
        .unwrap();

        let prompt =
            system_prompt_with_home(&[skill_tool_spec()], project.path(), Some(home.path())).text;

        assert!(prompt.contains("<available_skills>"));
        assert!(!prompt.contains("<name>manual-skill</name>"));
        assert!(!prompt.contains("only users may invoke this skill"));
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
    fn includes_txm_math_rendering_guidance() {
        let project = TempDir::new().unwrap();

        let prompt = system_prompt_with_home(&[], project.path(), None).text;

        // Loose markers only: the guidance must mention display and inline math
        // without locking the exact copy.
        assert!(prompt.contains("$$ ... $$"));
        assert!(prompt.contains("Inline `$...$` math"));
    }

    #[test]
    fn includes_grep_preference_only_when_grep_tool_is_available() {
        let project = TempDir::new().unwrap();
        let grep_tool = ToolSpec {
            name: "grep".into(),
            description: "search".into(),
            input_schema: serde_json::json!({}),
        };

        let enabled = system_prompt_with_home(&[grep_tool], project.path(), None).text;
        let disabled = system_prompt_with_home(&[], project.path(), None).text;

        assert!(enabled.contains("Prefer the `grep` tool over shell `rg` or `grep`"));
        assert!(enabled.contains("chainable `[path#TAG]`"));
        assert!(enabled.contains("not preview bodies"));
        assert!(enabled.contains("`N | preview`"));
        assert!(!disabled.contains("Prefer the `grep` tool over shell `rg` or `grep`"));
    }

    #[test]
    fn includes_edit_policy_only_when_edit_tool_is_available() {
        let project = TempDir::new().unwrap();
        let edit_tool = ToolSpec {
            name: "edit".into(),
            description: "edit".into(),
            input_schema: serde_json::json!({}),
        };

        let enabled = system_prompt_with_home(&[edit_tool], project.path(), None).text;
        let disabled = system_prompt_with_home(&[], project.path(), None).text;

        assert!(enabled.contains("Use `edit` (not shell or Python rewrites)"));
        assert!(enabled.contains("never `PUT 12.:`"));
        assert!(enabled.contains("without chainable body lines"));
        assert!(!disabled.contains("Use `edit` (not shell or Python rewrites)"));
    }

    #[test]
    fn includes_subagent_cost_guidance_only_when_agent_tool_is_available() {
        let project = TempDir::new().unwrap();
        let agent_tool = ToolSpec {
            name: "agent".into(),
            description: "delegate work".into(),
            input_schema: serde_json::json!({}),
        };

        let enabled = system_prompt_with_home(&[agent_tool], project.path(), None).text;
        let disabled = system_prompt_with_home(&[], project.path(), None).text;

        assert!(enabled.contains("Work directly by default"));
        assert!(enabled.contains("adds latency, token use, and coordination overhead"));
        assert!(enabled.contains("background=true"));
        assert!(
            enabled.contains("Batching a foreground agent with other tools does not background it")
        );
        assert!(enabled.contains("can delay the rest of that batch until the run finishes"));
        assert!(enabled.contains("Independent agents in the same batch run together"));
        assert!(enabled.contains("issue multiple agent calls in one turn"));
        assert!(enabled.contains("avoid overlapping edits"));
        assert!(!disabled.contains("Work directly by default"));
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

    #[test]
    fn appends_disabled_subagent_instruction() {
        let mut text = "base".to_string();

        append_subagents_disabled_instruction(&mut text);

        assert!(text.contains("Agent delegation is disabled"));
        assert!(text.contains("Do not attempt to delegate work"));
    }

    fn skill_tool_spec() -> ToolSpec {
        ToolSpec {
            name: "skill".into(),
            description: "load skills".into(),
            input_schema: serde_json::json!({}),
        }
    }
}
