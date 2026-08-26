use std::path::Path;

use pretty_assertions::assert_eq;

use super::{persist_definition, AgentSaveLocation, PersistDefinitionError};
use crate::{agent::parse_definition, workspace::ProjectTrust};

fn draft(id: &str) -> String {
    format!(
        "---\nid: {id}\ndescription: persist fixture\nprompt: extend\n---\nYou are a fixture.\n"
    )
}

// Covers: persist creates missing parent dirs, canonicalizes, and refuses a
// second write without the reviewed revision.
// Owner: agent persist
#[test]
fn persist_creates_canonical_file_and_requires_revision() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let outcome = persist_definition(
        AgentSaveLocation::RhoHome,
        &draft("reviewer"),
        None,
        cwd.path(),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap();

    let expected = home.path().join(".rho/agents/reviewer.md");
    assert_eq!(outcome.path, expected);
    assert!(outcome.created);
    let on_disk = std::fs::read_to_string(&expected).unwrap();
    assert_eq!(on_disk, outcome.contents);
    let parsed = parse_definition(&expected, "reviewer", &on_disk).unwrap();
    assert_eq!(parsed.id.as_str(), "reviewer");
    assert_eq!(on_disk, crate::agent::serialize_definition(&parsed));

    let exists = persist_definition(
        AgentSaveLocation::RhoHome,
        &draft("reviewer"),
        None,
        cwd.path(),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap_err();
    let revision = match exists {
        PersistDefinitionError::Exists {
            path,
            contents,
            revision,
        } => {
            assert_eq!(path, expected);
            assert_eq!(contents, on_disk);
            assert_eq!(revision, super::content_revision(&on_disk));
            revision
        }
        other => panic!("expected exists, got {other:?}"),
    };

    let overwritten = persist_definition(
        AgentSaveLocation::RhoHome,
        &draft("reviewer"),
        Some(revision.as_str()),
        cwd.path(),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap();
    assert!(!overwritten.created);
    assert_eq!(overwritten.path, expected);
}

fn draft_with_description(id: &str, description: &str) -> String {
    format!("---\nid: {id}\ndescription: {description}\nprompt: extend\n---\nYou are a fixture.\n")
}

// Covers: overwrite is bound to the reviewed revision; if B appears after A
// was reviewed, persist must conflict and leave B in place.
// Owner: agent persist
#[test]
fn persist_conflicts_when_reviewed_revision_is_stale() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    persist_definition(
        AgentSaveLocation::RhoHome,
        &draft_with_description("reviewer", "version a"),
        None,
        cwd.path(),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap();
    let path = home.path().join(".rho/agents/reviewer.md");
    let reviewed = persist_definition(
        AgentSaveLocation::RhoHome,
        &draft_with_description("reviewer", "replacement"),
        None,
        cwd.path(),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap_err();
    let PersistDefinitionError::Exists {
        revision: revision_a,
        contents: contents_a,
        ..
    } = reviewed
    else {
        panic!("expected exists for version A");
    };
    assert_eq!(contents_a, std::fs::read_to_string(&path).unwrap());

    let contents_b = draft_with_description("reviewer", "version b");
    std::fs::write(&path, &contents_b).unwrap();

    let stale = persist_definition(
        AgentSaveLocation::RhoHome,
        &draft_with_description("reviewer", "replacement"),
        Some(revision_a.as_str()),
        cwd.path(),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap_err();
    assert!(matches!(stale, PersistDefinitionError::Conflict));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents_b);
}

// Covers: project persist is gated on trust and writes under the project root.
// Owner: agent persist
#[test]
fn persist_project_location_requires_trust() {
    let project = tempfile::tempdir().unwrap();
    let untrusted = persist_definition(
        AgentSaveLocation::Project,
        &draft("worker"),
        None,
        project.path(),
        None,
        ProjectTrust::Untrusted,
    )
    .unwrap_err();
    assert!(matches!(untrusted, PersistDefinitionError::Unauthorized(_)));
    assert!(!project.path().join(".agents/agents/worker.md").exists());

    let outcome = persist_definition(
        AgentSaveLocation::Project,
        &draft("worker"),
        None,
        project.path(),
        None,
        ProjectTrust::Trusted,
    )
    .unwrap();
    assert_eq!(
        outcome.path,
        project.path().join(".agents/agents/worker.md")
    );
    assert!(outcome.created);
}

// Covers: invalid drafts never create files.
// Owner: agent persist
#[test]
fn persist_rejects_invalid_drafts_without_writing() {
    let home = tempfile::tempdir().unwrap();
    let error = persist_definition(
        AgentSaveLocation::AgentsHome,
        "---\nid: bad\n---\n",
        None,
        Path::new("."),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap_err();
    assert!(matches!(error, PersistDefinitionError::Validation(_)));
    assert!(!home.path().join(".agents/agents/bad.md").exists());
}

#[cfg(unix)]
// Covers: persist refuses a symlink destination instead of following it.
// Owner: agent persist
#[test]
fn persist_rejects_symlink_destination() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let agents = home.path().join(".rho/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let target = home.path().join("outside.md");
    std::fs::write(&target, "secret\n").unwrap();
    symlink(&target, agents.join("reviewer.md")).unwrap();

    let error = persist_definition(
        AgentSaveLocation::RhoHome,
        &draft("reviewer"),
        None,
        Path::new("."),
        Some(home.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap_err();
    assert!(matches!(error, PersistDefinitionError::Unauthorized(_)));
    assert_eq!(std::fs::read_to_string(target).unwrap(), "secret\n");
}
