use std::path::Path;

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
        "---\ndescription: work\ntools: [read_file, write_file]\n---\nship it\n",
    )
    .unwrap();
    let b = parse_definition(
        Path::new("elsewhere.md"),
        "worker",
        "---\nid: worker\ndescription: work\ntools:\n  - write_file\n  - read_file\n---\n\nship it\n",
    )
    .unwrap();
    assert_eq!(a.fingerprint(), b.fingerprint());
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
            "ef6f425945a8cee8742e53e8abb5c8f4cf8da391e48a2a44982c740bb735c249",
        ),
        (
            "reviewer",
            "d1d123fac162706d7a40921acbef3af30d6737932afc6fbec4f39ec3e16c76ea",
        ),
        (
            "worker",
            "6a1f787c17442841a11703c25cb1ef48501be615656a22aba42237c8ccece071",
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
        "---\ndescription: work\ntools: [read_file, write_file]\n---\nship it\n",
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
