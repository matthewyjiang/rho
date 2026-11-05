use pretty_assertions::assert_eq;

use super::*;

// Covers: the run writes the exact agy agent file at the path agy discovers
// (`<root>/.agents/agents/<name>.md`) with the kept tools as its allowlist.
// Owner: Antigravity agent file; the live agy E2E proves agy honors it.
#[test]
fn writes_the_allowlist_where_agy_discovers_agents() {
    let run_dir = tempfile::tempdir().unwrap();
    let root = write(
        run_dir.path(),
        &[AntigravityTool::ReadFile, AntigravityTool::RunCommand],
    )
    .unwrap();
    assert_eq!(root, run_dir.path().join("agy"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agents/agents/rho-run.md")).unwrap(),
        "---\nname: rho-run\ndescription: Rho delegated run.\ntools:\n  - read_file\n  - run_command\n---\n"
    );
}
