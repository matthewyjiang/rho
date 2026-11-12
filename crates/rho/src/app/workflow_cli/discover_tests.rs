use super::{command_workflows, discover_workflow_sources, resolve_launch, WorkflowLaunch};
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

// Covers: discovery must surface folder and flat workflow sources with stable
// paths; a missing workflows directory is an empty inventory, not an error.
// Owner: workflow source discovery.
#[test]
fn discover_finds_folder_and_flat_entries() {
    let root = tempfile::tempdir().unwrap();
    assert!(discover_workflow_sources(root.path()).is_empty());
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

// Covers: command menus list each name once, so `<name>.star` beside
// `<name>/workflow.star`, or names that differ only by case, cannot advertise
// two commands for one ambiguous name.
// Owner: `/workflow:<name>` advertising.
#[test]
fn command_workflows_lists_each_name_once() {
    let root = workspace(&[
        ".rho/workflows/twin.star",
        ".rho/workflows/twin/workflow.star",
        ".rho/workflows/solo.star",
        ".rho/workflows/Case.star",
        ".rho/workflows/case/workflow.star",
    ]);

    let names = command_workflows(root.path())
        .iter()
        .filter_map(super::DiscoveredWorkflow::command_name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["workflow:Case", "workflow:solo", "workflow:twin"]
    );
}

// Covers: the hub's prompts for a saved plan and a stopped run resolve to
// that plan and run, with recovery only when asked for, and each prompt the
// hub writes resolves back to what it names; a malformed id or stray
// argument is refused with the reason, and a workflow's name still starts
// its source.
// Owner: `/workflow:` resolution shared by every session host.
#[test]
fn launch_resolves_plans_runs_and_sources() {
    let root = workspace(&[".rho/workflows/review.star"]);
    let (plan, run) = (
        crate::workflow::PlanId::new(),
        crate::workflow::RunId::new(),
    );
    let resolve = |prompt: &str| {
        let (name, args) = prompt[1..].split_once(' ').unwrap_or((&prompt[1..], ""));
        resolve_launch(root.path(), name, args).map(|resolved| resolved.is_ok().then_some(resolved))
    };
    let launch = |prompt: &str| resolve(prompt).flatten().and_then(Result::ok);

    assert_eq!(
        (
            launch(&WorkflowLaunch::plan_command(plan)),
            launch(&WorkflowLaunch::resume_command(
                run, /*recover_uncertain*/ false
            )),
            launch(&WorkflowLaunch::resume_command(
                run, /*recover_uncertain*/ true
            )),
            launch("/workflow:review").map(|launch| matches!(launch, WorkflowLaunch::Source(_))),
            resolve(&format!("/workflow:plan/{plan} extra")),
            resolve("/workflow:plan/nope"),
            resolve(&format!("/workflow:run/{run} again")),
            resolve("/prompt:review"),
        ),
        (
            Some(WorkflowLaunch::Plan(plan)),
            Some(WorkflowLaunch::Resume {
                run,
                recover_uncertain: false
            }),
            Some(WorkflowLaunch::Resume {
                run,
                recover_uncertain: true
            }),
            Some(true),
            Some(None),
            Some(None),
            Some(None),
            None,
        )
    );
}
