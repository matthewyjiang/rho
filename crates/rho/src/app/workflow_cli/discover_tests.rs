use super::{command_workflows, discover_workflow_sources, resolve_start_command};
use pretty_assertions::assert_eq;

fn workspace(files: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for file in files {
        let path = root.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }
    root
}

// Covers: discovery must surface folder and flat workflow sources with stable paths.
// Owner: workflow source discovery.
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
}

// Covers: missing workflows directory is a valid empty inventory, not an error path.
// Owner: workflow source discovery.
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

// Covers: a command resolves to exactly one entry file with its JSON inputs,
// and every other request fails with the reason instead of reaching the model.
// Owner: `/workflow:<name>` resolution shared by the TUI and ACP.
#[test]
fn start_command_resolves_one_workflow_and_its_inputs() {
    let root = workspace(&[
        ".rho/workflows/review/workflow.star",
        ".rho/workflows/twin.star",
        ".rho/workflows/twin/workflow.star",
    ]);
    let resolve = |name: &str, args: &str| {
        resolve_start_command(root.path(), name, args).map(|resolved| {
            resolved.map(|command| (command.workflow.relative_path, command.inputs))
        })
    };

    assert_eq!(resolve("prompt:review", ""), None);
    assert_eq!(
        resolve("workflow:review", r#" {"target": "src", "depth": 2} "#),
        Some(Ok((
            ".rho/workflows/review/workflow.star".to_owned(),
            [
                ("depth".to_owned(), serde_json::json!(2)),
                ("target".to_owned(), serde_json::json!("src")),
            ]
            .into(),
        )))
    );
    assert_eq!(
        resolve("workflow:review", "").map(|resolved| resolved.map(|(_, inputs)| inputs)),
        Some(Ok(Default::default()))
    );
    for (name, args) in [
        ("workflow:missing", ""),
        ("workflow:twin", ""),
        ("workflow:review", "src"),
        ("workflow:review", "[1]"),
    ] {
        assert!(
            matches!(resolve(name, args), Some(Err(_))),
            "/{name} {args} should be rejected"
        );
    }
}

// Covers: command menus list each name once, so `<name>.star` beside
// `<name>/workflow.star` cannot advertise two identical commands.
// Owner: `/workflow:<name>` advertising.
#[test]
fn command_workflows_lists_each_name_once() {
    let root = workspace(&[
        ".rho/workflows/twin.star",
        ".rho/workflows/twin/workflow.star",
        ".rho/workflows/solo.star",
    ]);

    let names = command_workflows(root.path())
        .iter()
        .filter_map(super::DiscoveredWorkflow::command_name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["workflow:solo", "workflow:twin"]);
}
