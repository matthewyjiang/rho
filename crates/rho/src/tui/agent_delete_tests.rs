use pretty_assertions::assert_eq;

use super::super::{tests::test_app, ComposerMode, UiPicker};
use super::{delete_target, AgentDeleteTarget};
use crate::agent::{AgentCatalog, AgentOrigin};

// Covers: only the user's own agents (`~/.rho/agents`, project) can be
// deleted; shared, built-in, and internal rows must never offer a delete.
// Owner: tui agent delete
#[test]
fn only_user_agents_are_delete_targets() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let agent = "---\ndescription: d\n---\nbody\n";
    for (dir, id) in [
        (home.path().join(".rho/agents"), "mine"),
        (home.path().join(".agents/agents"), "shared"),
        (cwd.path().join(".agents/agents"), "project"),
    ] {
        std::fs::create_dir_all(&dir).expect("agents dir");
        std::fs::write(dir.join(format!("{id}.md")), agent).expect("write agent");
    }
    let catalog = AgentCatalog::discover_with_home(cwd.path(), Some(home.path())).expect("catalog");

    let origins = ["mine", "project", "shared", "reviewer", "goal-judge"]
        .map(|id| delete_target(&catalog, id).map(|target| target.origin));

    assert_eq!(
        origins,
        [
            Some(AgentOrigin::RhoHome),
            Some(AgentOrigin::Project),
            None,
            None,
            None,
        ]
    );
}

// Covers: cancelling the confirmation keeps the file and returns to the
// agents picker.
// Owner: tui agent delete
#[test]
fn cancelled_delete_keeps_agent() {
    let project = tempfile::tempdir().expect("project");
    let agents = project.path().join(".agents/agents");
    std::fs::create_dir_all(&agents).expect("agents dir");
    let path = agents.join("demo.md");
    std::fs::write(&path, "---\ndescription: demo\n---\nbody\n").expect("write agent");
    let mut app = test_app();
    app.info.runtime.cwd = project.path().to_path_buf();

    app.submit_delete_agent_choice(
        "cancel",
        AgentDeleteTarget {
            id: "demo".into(),
            origin: AgentOrigin::Project,
            path: path.clone(),
        },
        Some(Box::new(UiPicker::view_agent("Loaded agents", Vec::new()))),
    );

    assert!(path.exists());
    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::Picker(picker) if picker.is_view_agent()
    ));
}

// Covers: a target outside an editable agent root is refused at confirm time,
// so a stale or forged path cannot delete arbitrary files.
// Owner: tui agent delete
#[test]
fn delete_agent_refuses_path_outside_agent_root() {
    let project = tempfile::tempdir().expect("project");
    let path = project.path().join("not-an-agent.md");
    std::fs::write(&path, "keep me").expect("write file");
    let mut app = test_app();
    app.info.runtime.cwd = project.path().to_path_buf();

    app.submit_delete_agent_choice(
        "delete",
        AgentDeleteTarget {
            id: "demo".into(),
            origin: AgentOrigin::Project,
            path: path.clone(),
        },
        Some(Box::new(UiPicker::view_agent("Loaded agents", Vec::new()))),
    );

    assert!(path.exists());
    assert_eq!(app.status(), "agent delete failed");
}
