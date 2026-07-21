use std::path::Path;

use super::*;

#[test]
fn builtins_share_one_catalog() {
    let root = tempfile::tempdir().unwrap();
    let catalog = AgentCatalog::discover_with_home(root.path(), None).unwrap();
    let ids = catalog
        .iter()
        .map(|entry| entry.definition.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["default", "explorer", "reviewer", "worker"]);
}

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
