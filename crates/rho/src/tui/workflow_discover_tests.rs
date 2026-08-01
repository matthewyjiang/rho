use super::{discover_workflow_sources, workflow_label};
use pretty_assertions::assert_eq;

#[test]
fn folder_workflow_star_uses_folder_name() {
    assert_eq!(
        workflow_label(".rho/workflows/thermo_nuclear_review/workflow.star"),
        "thermo_nuclear_review"
    );
}

#[test]
fn flat_star_uses_stem() {
    assert_eq!(workflow_label(".rho/workflows/review.star"), "review");
}

#[test]
fn discover_finds_folder_and_flat_entries() {
    let root = tempfile::tempdir().unwrap();
    let workflows = root.path().join(".rho").join("workflows");
    std::fs::create_dir_all(workflows.join("review")).unwrap();
    std::fs::write(workflows.join("review").join("workflow.star"), "x").unwrap();
    std::fs::write(workflows.join("solo.star"), "y").unwrap();
    std::fs::write(workflows.join("review").join("helper.py"), "z").unwrap();
    std::fs::write(workflows.join("review").join("common.star"), "skip").unwrap();

    let found = discover_workflow_sources(root.path());
    let paths = found
        .iter()
        .map(|item| item.relative_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            ".rho/workflows/review/workflow.star",
            ".rho/workflows/solo.star",
        ]
    );
    assert_eq!(found[0].label, "review");
    assert_eq!(found[1].label, "solo");
}

#[test]
fn missing_workflows_dir_is_empty() {
    let root = tempfile::tempdir().unwrap();
    assert!(discover_workflow_sources(root.path()).is_empty());
}

// Covers: discovery must not offer a nested entry that source loading rejects.
// Owner: workflow source discovery filesystem policy.
#[cfg(unix)]
#[test]
fn discover_skips_nested_workflow_star_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let workflows = root.path().join(".rho").join("workflows");
    std::fs::create_dir_all(workflows.join("linked")).unwrap();
    std::fs::write(workflows.join("target.star"), "x").unwrap();
    symlink(
        workflows.join("target.star"),
        workflows.join("linked").join("workflow.star"),
    )
    .unwrap();

    let found = discover_workflow_sources(root.path());
    assert_eq!(
        found
            .iter()
            .map(|workflow| workflow.relative_path.as_str())
            .collect::<Vec<_>>(),
        vec![".rho/workflows/target.star"]
    );
}
