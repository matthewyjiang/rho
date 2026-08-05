use std::path::{Path, PathBuf};

use super::*;

#[test]
fn rejects_unknown_tools_with_context() {
    let root = tempfile::tempdir().unwrap();
    let agents = root.path().join(".rho/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let path = agents.join("bad.md");
    std::fs::write(&path, "---\ndescription: bad\ntools: [teleport]\n---\n").unwrap();

    let error = AgentCatalog::discover_with_home(root.path(), Some(root.path())).unwrap_err();

    assert_eq!(error.path, path);
    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.to_string().contains("unknown tool 'teleport'"));
    assert!(error.to_string().contains("runtime: rho"));
}

#[test]
fn semantic_fingerprint_ignores_formatting_and_source() {
    let a = parse_definition(
        Path::new("a.md"),
        "worker",
        "---\ndescription: work\ntools: [read_file, write]\n---\nship it\n",
    )
    .unwrap();
    let b = parse_definition(
        Path::new("elsewhere.md"),
        "worker",
        "---\nid: worker\ndescription: work\ntools:\n  - write\n  - read_file\n---\n\nship it\n",
    )
    .unwrap();
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn write_file_capability_alias_matches_write() {
    let canonical = parse_definition(
        Path::new("a.md"),
        "worker",
        "---\ndescription: work\ntools: [write]\n---\n",
    )
    .unwrap();
    let legacy_name = parse_definition(
        Path::new("b.md"),
        "worker",
        "---\ndescription: work\ntools: [write_file]\n---\n",
    )
    .unwrap();
    assert_eq!(canonical.fingerprint(), legacy_name.fingerprint());
    assert_eq!(ToolCapability::parse("write_file".into()).as_str(), "write");
}

#[test]
fn current_fingerprint_uses_v2_marker_and_differs_from_legacy_v1() {
    let definition = parse_definition(
        Path::new("default.md"),
        "default",
        "---\ndescription: demo\ntools: all\n---\n",
    )
    .unwrap();
    let current = definition.fingerprint().to_string();
    let legacy = definition
        .legacy_v1_fingerprint()
        .expect("default rho definition encodes legacy v1")
        .to_string();
    assert_ne!(current, legacy);
    assert!(definition.accepts_stored_fingerprint(&current));
    assert!(definition.accepts_stored_fingerprint(&legacy));
    assert!(!definition.accepts_stored_fingerprint("deadbeef"));
}

/// Golden v1 fingerprints for builtin agents, so resume keeps accepting the
/// values sessions stored before the runtime axis.
///
/// Editing a builtin definition changes its fingerprint and stops older
/// sessions for that agent from resuming. Update a value here only alongside a
/// deliberate change to the matching `builtin_agents/*.md`.
#[test]
fn golden_legacy_v1_fingerprints_for_builtin_rho_agents() {
    let root = tempfile::tempdir().unwrap();
    let catalog = AgentCatalog::discover_with_home(root.path(), None).unwrap();
    let expected = [
        (
            "default",
            "ffc3f694800c9e3d284e457e63b2a61ad97f361f84ce3493314cc9c69892826d",
        ),
        (
            "explorer",
            "52a0868729579676fbcff35089221a5a59d52da787d995a9f3a776b94e041dc0",
        ),
        (
            "reviewer",
            "b6dbdf4028def08031a039f757116526e833c45b3c73318246b519d50246c469",
        ),
        (
            "worker",
            "b89c52ef5f589a6472764151b6baf50640d362a2a69d5d01a81ff1cc744fe5f3",
        ),
    ];
    for (id, expected_legacy) in expected {
        let definition = &catalog.find(id).unwrap().definition;
        assert!(
            matches!(definition.runtime, AgentRuntimeSpec::Rho { .. }),
            "{id} must remain default Rho for legacy resume"
        );
        let legacy = definition
            .legacy_v1_fingerprint()
            .unwrap_or_else(|| panic!("{id} should expose legacy v1"))
            .to_string();
        assert_eq!(legacy, expected_legacy, "legacy v1 drift for {id}");
        assert_ne!(
            definition.fingerprint().to_string(),
            legacy,
            "{id} current fingerprint must be v2"
        );
        assert!(definition.accepts_stored_fingerprint(&legacy));
    }
}

#[test]
fn real_definition_change_still_rejects_resume() {
    let original = parse_definition(
        Path::new("worker.md"),
        "worker",
        "---\ndescription: work\ntools: [read_file]\n---\nship it\n",
    )
    .unwrap();
    let changed = parse_definition(
        Path::new("worker.md"),
        "worker",
        "---\ndescription: work\ntools: [read_file, write]\n---\nship it\n",
    )
    .unwrap();
    let stored_v2 = original.fingerprint().to_string();
    let stored_v1 = original.legacy_v1_fingerprint().unwrap().to_string();
    assert!(!changed.accepts_stored_fingerprint(&stored_v2));
    assert!(!changed.accepts_stored_fingerprint(&stored_v1));
}

#[test]
fn claude_definitions_have_no_legacy_v1_fingerprint() {
    let definition = parse_definition(
        Path::new("claude.md"),
        "claude",
        "---\ndescription: demo\nruntime: claude-cli\ntools: [Read]\n---\n",
    )
    .unwrap();
    assert!(definition.legacy_v1_fingerprint().is_none());
    assert!(!definition.accepts_stored_fingerprint("anything"));
    assert!(definition.accepts_stored_fingerprint(&definition.fingerprint().to_string()));
}

#[test]
fn same_tier_duplicates_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let agents = root.path().join(".rho/agents");
    std::fs::create_dir_all(&agents).unwrap();
    for file in ["one.md", "two.md"] {
        std::fs::write(
            agents.join(file),
            "---\nid: duplicate\ndescription: duplicate\n---\n",
        )
        .unwrap();
    }
    let error = AgentCatalog::discover_with_home(root.path(), Some(root.path())).unwrap_err();
    assert_eq!(error.field.as_deref(), Some("id"));
    assert!(error.to_string().contains("duplicate agent ID"));
}

#[test]
fn internal_agents_are_visible_but_not_selectable() {
    let root = tempfile::tempdir().unwrap();
    let catalog = AgentCatalog::discover_with_home(root.path(), None).unwrap();

    assert!(catalog.find(SESSION_TITLE_AGENT_ID).is_err());
    assert!(catalog.find(GOAL_JUDGE_AGENT_ID).is_err());
    assert!(catalog
        .iter()
        .all(|entry| entry.metadata.origin != AgentOrigin::Internal));
    let origins = catalog
        .iter_with_internal()
        .map(|entry| entry.metadata.origin)
        .collect::<Vec<_>>();
    assert_eq!(origins[..2], [AgentOrigin::Internal, AgentOrigin::Internal]);
    assert!(origins[2..]
        .iter()
        .all(|origin| *origin != AgentOrigin::Internal));
}

#[test]
fn rejects_files_with_reserved_internal_agent_ids() {
    let root = tempfile::tempdir().unwrap();
    let agents = root.path().join(".rho/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let path = agents.join("session-title.md");
    std::fs::write(&path, "---\ndescription: shadow\n---\nshadow prompt\n").unwrap();

    let error = AgentCatalog::discover_with_home(root.path(), Some(root.path())).unwrap_err();

    assert_eq!(error.path, path);
    assert_eq!(error.field.as_deref(), Some("id"));
    assert!(error.to_string().contains("session-title"));
    assert!(error.to_string().contains("reserved"));
}

#[test]
fn project_definitions_require_explicit_trust() {
    let project = tempfile::tempdir().unwrap();
    let agents = project.path().join(".agents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("project.md"),
        "---\ndescription: project agent\n---\n",
    )
    .unwrap();

    let untrusted =
        AgentCatalog::discover_with_home_and_trust(project.path(), None, ProjectTrust::Untrusted)
            .unwrap();
    assert!(untrusted.find("project").is_err());
    let trusted =
        AgentCatalog::discover_with_home_and_trust(project.path(), None, ProjectTrust::Trusted)
            .unwrap();
    assert_eq!(
        trusted.find("project").unwrap().metadata.origin,
        AgentOrigin::Project
    );
}

// Covers: workflow planning must see agents shipped beside the entry file.
// Owner: agent catalog discovery.
#[test]
fn workflow_entry_loads_local_agents_directory() {
    let root = tempfile::tempdir().unwrap();
    let workflow_dir = root.path().join("review");
    let agents = workflow_dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    let entry = workflow_dir.join("workflow.star");
    std::fs::write(&entry, "def build(inputs):\n    pass\n").unwrap();
    std::fs::write(
        agents.join("boundary-reviewer.md"),
        "---\ndescription: workflow-local specialist\ntools: [read_file, grep, glob, list_dir]\n---\nReview boundaries only.\n",
    )
    .unwrap();

    let without =
        AgentCatalog::discover_with_home_and_trust(root.path(), None, ProjectTrust::Untrusted)
            .unwrap();
    assert!(without.find("boundary-reviewer").is_err());

    let with = AgentCatalog::discover_for_workflow_entry(
        root.path(),
        &entry,
        None,
        ProjectTrust::Untrusted,
    )
    .unwrap();
    let entry = with.find("boundary-reviewer").unwrap();
    assert_eq!(entry.metadata.origin, AgentOrigin::Workflow);
    assert_eq!(entry.definition.description, "workflow-local specialist");
}

// Covers: path helper keeps agents next to the workflow entry parent.
// Owner: agent catalog discovery.
#[test]
fn workflow_local_agents_root_is_sibling_agents_dir() {
    assert_eq!(
        workflow_local_agents_root(Path::new(".rho/workflows/review/workflow.star")),
        PathBuf::from(".rho/workflows/review/agents")
    );
    assert_eq!(
        workflow_local_agents_root(Path::new("workflow.star")),
        PathBuf::from("agents")
    );
}
