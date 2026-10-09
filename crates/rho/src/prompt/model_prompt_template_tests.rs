use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

// Covers: switching append/replace/default must discard previous model text while
// retaining host-owned instructions and reporting exact source contributions.
// Owner: prompt assembly; lifecycle tests cover committing this result to a session.
#[test]
fn model_selection_rebuilds_behavior_without_losing_retained_instructions() {
    let home = TempDir::new().unwrap();
    let directory = home.path().join(".rho/model-prompts");
    std::fs::create_dir_all(&directory).unwrap();
    for mode in ["append", "replace"] {
        std::fs::write(
            directory.join(format!("{mode}.md")),
            format!("---\nprovider: test\nmodel: {mode}\nmode: {mode}\n---\n{mode} behavior"),
        )
        .unwrap();
    }
    let mut template = ModelPromptTemplate::new(
        Some(home.path()),
        "\ncwd\n".into(),
        "retained".into(),
        vec![PromptSource {
            kind: PromptSourceKind::Base,
            path: None,
            bytes: "retained".len(),
        }],
    );
    template.append_retained("\nagent and MCP instructions");
    for (model, behavior) in [
        ("append", format!("{BASE_SYSTEM_PROMPT}\n\nappend behavior")),
        ("replace", "replace behavior".into()),
        ("unmatched", BASE_SYSTEM_PROMPT.into()),
        ("append", format!("{BASE_SYSTEM_PROMPT}\n\nappend behavior")),
    ] {
        let running = PromptModel::Rho {
            provider: "test".into(),
            model: model.into(),
        };
        let built = template.build(&running).unwrap();
        assert_eq!(built.text, format!(
            "{behavior}\ncwd\nYou are running on {}. Rho can switch this mid-session and tells you when it does.\nretained\nagent and MCP instructions",
            running.describe(),
        ));
        assert_eq!(
            built
                .sources
                .iter()
                .map(|source| source.bytes)
                .sum::<usize>(),
            built.text.len()
        );
    }
}

// Covers: session suffix attachment must not reorder MCP context on hydration.
// Owner: retained prompt template assembly.
#[test]
fn retained_suffix_and_mcp_rerender_match_the_built_prompt() {
    let running = PromptModel::Rho {
        provider: "test".into(),
        model: "model".into(),
    };
    let mut template = ModelPromptTemplate::new(
        None,
        String::new(),
        "retained".into(),
        vec![PromptSource {
            kind: PromptSourceKind::Base,
            path: None,
            bytes: "retained".len(),
        }],
    );
    use crate::tools::mcp::report::{
        ConnectedServerReport, McpServerReport, McpSessionReport, McpTransportSummary,
    };
    template.replace_mcp(&McpSessionReport {
        servers: vec![McpServerReport::connected(ConnectedServerReport {
            identity: "docs".into(),
            transport: McpTransportSummary::Stdio {
                command: "fixture".into(),
                args: Vec::new(),
            },
            tools: Vec::new(),
            instructions: Some("mcp context".into()),
            live: Default::default(),
            filtered_out_count: 0,
            collision_skipped_count: 0,
        })],
        ..Default::default()
    });
    let mut built = template.build(&running).unwrap();
    let suffix = "\n\n# Agent instructions\n\nsession suffix";
    built.text.push_str(suffix);
    template.append_retained(suffix);
    let rerendered = template.render(&running, built.model_prompt.as_ref());
    assert_eq!(rerendered.text, built.text);
    assert_eq!(
        rerendered
            .sources
            .iter()
            .map(|source| source.bytes)
            .sum::<usize>(),
        built.text.len()
    );
}

// Covers: reloading created, changed, and removed instruction files must replace
// their provenance without dropping host-owned suffixes or duplicating sources.
// Owner: prompt assembly; PTY covers committing the snapshot at session boundaries.
#[test]
fn project_instruction_reload_replaces_context_and_source_accounting() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(home.path().join(".rho")).unwrap();
    let running = PromptModel::Rho {
        provider: "test".into(),
        model: "model".into(),
    };
    let make_template = || {
        let mut template = super::super::system_prompt_template_with_home_and_models(
            &[],
            project.path(),
            Some(home.path()),
            super::super::PromptModels {
                running: &running,
                advisor: None,
            },
        );
        let skills = "\ncached skill context";
        template.append_section(
            skills.into(),
            vec![PromptSource {
                kind: PromptSourceKind::Skills,
                path: None,
                bytes: skills.len(),
            }],
        );
        template.append_retained("\nretained agent instructions");
        template
    };
    let mut template = make_template();
    let mut expected = template.build(&running).unwrap();
    for (rules, session, has_instructions) in [
        (Some("first"), PromptSession::Different, true),
        (Some("updated"), PromptSession::Current, true),
        (Some("updated"), PromptSession::Different, true),
        (None, PromptSession::Current, true),
        (None, PromptSession::Different, false),
    ] {
        for path in [
            home.path().join(".rho/AGENTS.md"),
            project.path().join("AGENTS.md"),
        ] {
            if let Some(rules) = rules {
                std::fs::write(path, rules).unwrap();
            } else if path.exists() {
                std::fs::remove_file(path).unwrap();
            }
        }
        // Model switches must keep the session's cached instruction snapshot.
        let switched = template.build(&running).unwrap();
        assert_eq!(
            (&switched.text, &switched.sources),
            (&expected.text, &expected.sources),
        );
        let refreshed = template.build_for_session(&running, session).unwrap();
        match session {
            PromptSession::Current => {}
            PromptSession::Different => expected = make_template().build(&running).unwrap(),
        }
        assert_eq!(
            (&refreshed.text, &refreshed.sources),
            (&expected.text, &expected.sources),
            "instruction state: {rules:?}",
        );
        assert_eq!(
            refreshed
                .sources
                .iter()
                .map(|source| source.kind)
                .collect::<Vec<_>>(),
            if has_instructions {
                vec![
                    PromptSourceKind::Base,
                    PromptSourceKind::Agents,
                    PromptSourceKind::Agents,
                    PromptSourceKind::Skills,
                ]
            } else {
                vec![PromptSourceKind::Base, PromptSourceKind::Skills]
            },
        );
        assert_eq!(
            refreshed
                .sources
                .iter()
                .map(|source| source.bytes)
                .sum::<usize>(),
            refreshed.text.len()
        );
    }
}

// Covers: incidental startup hydration keeps the loaded file, but an explicit
// lifecycle build reads current contents and rejects new validation failures.
// Owner: prompt assembly.
#[test]
fn cached_render_and_explicit_reload_have_distinct_file_lifetimes() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let instructions = project.path().join("AGENTS.md");
    std::fs::write(&instructions, "original instructions").unwrap();
    let directory = home.path().join(".rho/model-prompts");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("custom.md");
    std::fs::write(&path, "---\nprovider: test\nmodel: model\n---\nfirst").unwrap();
    let mut template = ModelPromptTemplate::new(
        Some(home.path()),
        String::new(),
        String::new(),
        vec![PromptSource {
            kind: PromptSourceKind::Base,
            path: None,
            bytes: 0,
        }],
    )
    .with_project_instructions(project.path());
    let running = PromptModel::Rho {
        provider: "test".into(),
        model: "model".into(),
    };
    let original = template.build(&running).unwrap();
    std::fs::write(&path, "---\nprovider: test\nmodel: model\n---\nsecond").unwrap();
    let cached = template.render(&running, original.model_prompt.as_ref());
    assert_eq!(cached.text, original.text);
    let reloaded = template.build(&running).unwrap();
    assert_eq!(reloaded.model_prompt.as_ref().unwrap().body, "second");
    assert_ne!(
        &reloaded.model_prompt.as_ref().unwrap().sha256,
        &original.model_prompt.as_ref().unwrap().sha256
    );
    std::fs::write(&path, "invalid frontmatter").unwrap();
    assert!(template.build(&running).is_err());
    std::fs::write(&instructions, "changed instructions").unwrap();
    assert!(template
        .build_for_session(&running, PromptSession::Different)
        .is_err());
    // Failed validation must not leak refreshed instructions into hydration.
    let after_failure = template.render(&running, original.model_prompt.as_ref());
    assert_eq!(
        (after_failure.text, after_failure.sources),
        (original.text, original.sources),
    );
}

// Covers: an added directory loads its own git-root-to-directory AGENTS.md
// chain, a file shared with another added directory loads once, and setting an
// unchanged set reports nothing new for the live session.
// Owner: prompt assembly; runtime tests cover delivering new files mid-session.
#[test]
fn added_dirs_load_their_instruction_chains_once() {
    let project = TempDir::new().unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "project rules").unwrap();
    let repo = TempDir::new().unwrap();
    let repo_root = std::fs::canonicalize(repo.path()).unwrap();
    std::fs::create_dir(repo_root.join(".git")).unwrap();
    std::fs::write(repo_root.join("AGENTS.md"), "repo rules").unwrap();
    for dir in ["a", "b"] {
        std::fs::create_dir(repo_root.join(dir)).unwrap();
    }
    std::fs::write(repo_root.join("a/AGENTS.md"), "a rules").unwrap();
    let mut added = crate::added_dirs::AddedDirs::default();
    for dir in ["a", "b"] {
        added.insert(project.path(), repo_root.join(dir));
    }
    let mut template = ModelPromptTemplate::new(None, String::new(), String::new(), Vec::new())
        .with_project_instructions(project.path());

    let loaded = template.set_added_dirs(&added);
    assert_eq!(
        loaded,
        vec![
            (repo_root.join("AGENTS.md"), "repo rules".to_string()),
            (repo_root.join("a/AGENTS.md"), "a rules".to_string()),
        ]
    );
    assert_eq!(template.set_added_dirs(&added), Vec::new());

    let running = PromptModel::Rho {
        provider: "test".into(),
        model: "model".into(),
    };
    let agents_sources = template
        .build(&running)
        .unwrap()
        .sources
        .iter()
        .filter(|source| source.kind == PromptSourceKind::Agents)
        .count();
    assert_eq!(agents_sources, 3);
}
