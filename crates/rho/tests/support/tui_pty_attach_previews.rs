//! Real delegated tool-argument streaming, live attach, and finished replay.

use std::path::{Path, PathBuf};

use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};

fn find_journal(root: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == "events.jsonl") {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(path) = find_journal(&path) {
                return Some(path);
            }
        }
    }
    None
}

// Covers: transient argument cards must stay live without bloating durable replay.
// Owner: delegated-run/attach product contract (not a synthetic journal fixture).
#[test]
fn delegated_argument_previews_live_and_replay() {
    let home = IsolatedHome::new().unwrap();
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize {
            rows: 40,
            cols: 110,
        },
    );
    // Reuse the standalone attach startup failure bound.
    let wait = WaitTimeout::secs(10, "delegated argument stream");
    let mut parent = PtyHarness::spawn_named(&plan, "preview_parent").unwrap();
    parent.wait_for_text("gpt-5.5", wait).unwrap();
    parent.submit_text("fixture delegated write input").unwrap();
    parent
        .wait_for_text("delegated write dispatched", wait)
        .unwrap();
    parent.submit_text("/attach").unwrap();
    parent.wait_for_text("attach subagent", wait).unwrap();
    parent.inject_key(&Key::Enter).unwrap();
    parent.wait_for_text("q back", wait).unwrap();
    parent.inject_key(&Key::Ctrl('o')).unwrap();
    parent.wait_for_text("FIRST_WRITE_FRAGMENT", wait).unwrap();
    let journal = find_journal(&home.home.join(".rho")).expect("delegated journal");
    let before = std::fs::read(&journal).unwrap();
    std::fs::write(home.workspace.join(".rho-fixture-release-write-next"), "").unwrap();
    parent.wait_for_text("SECOND_WRITE_FRAGMENT", wait).unwrap();
    assert_eq!(
        std::fs::read(&journal).unwrap(),
        before,
        "previews must not append history"
    );
    assert!(!home
        .workspace
        .join(".rho-tui-fixture-streamed-write.txt")
        .exists());
    let run_id = journal
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let attach_plan = plan.with_arg("attach").with_arg(run_id);
    let mut external = PtyHarness::spawn_named(&attach_plan, "preview_live_external").unwrap();
    external
        .wait_for_text(".rho-tui-fixture-streamed-write.txt", wait)
        .unwrap();
    external.inject_key(&Key::Ctrl('o')).unwrap();
    external
        .wait_for_text("SECOND_WRITE_FRAGMENT", wait)
        .unwrap();
    std::fs::write(
        home.workspace.join(".rho-fixture-release-write-complete"),
        "",
    )
    .unwrap();
    parent
        .wait_for_text("streamed write completed successfully", wait)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(home.workspace.join(".rho-tui-fixture-streamed-write.txt"))
            .unwrap(),
        "FIRST_WRITE_FRAGMENT\nSECOND_WRITE_FRAGMENT\n",
    );
    external
        .wait_for_text("streamed write completed successfully", wait)
        .unwrap();
    external.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(external.wait_for_exit(wait).unwrap(), 0);
    parent.inject_key(&Key::Esc).unwrap();
    parent.wait_for_text_gone("q back", wait).unwrap();
    parent.submit_text("/exit").unwrap();
    assert_eq!(parent.wait_for_exit(wait).unwrap(), 0);

    let mut replay = PtyHarness::spawn_named(&attach_plan, "preview_replay").unwrap();
    replay
        .wait_for_text("streamed write completed successfully", wait)
        .unwrap();
    replay.inject_key(&Key::Ctrl('o')).unwrap();
    replay.wait_for_text("SECOND_WRITE_FRAGMENT", wait).unwrap();
    replay.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(replay.wait_for_exit(wait).unwrap(), 0);
}
