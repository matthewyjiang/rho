//! Real delegated tool-argument streaming, live attach, and finished replay.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};

use super::{attach_view, SessionHost};

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
    delegated_argument_previews(SessionHost::InProcess);
}

// Covers: with RHO_TUI_DAEMON=1, the attach view follows a run the daemon
// hosts as it does one in process.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_delegated_argument_previews_live_and_replay() {
    delegated_argument_previews(SessionHost::Daemon);
}

fn delegated_argument_previews(host: SessionHost) {
    let home = IsolatedHome::new().unwrap();
    let plan = host.launch(RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize {
            rows: 40,
            cols: 110,
        },
    ));
    let wait = WaitTimeout::secs(10, "delegated argument stream");
    let mut parent = PtyHarness::spawn_named(&plan, "preview_parent").unwrap();
    parent.wait_for_text("gpt-5.5", wait).unwrap();
    parent.submit_text("fixture delegated write input").unwrap();
    parent
        .wait_for_text("delegated write dispatched", wait)
        .unwrap();
    attach_view::open(&mut parent, wait);
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
    // Reopen the view so a fresh reader joins mid-run and still sees the
    // replaceable preview.
    attach_view::leave(&mut parent, wait);
    attach_view::open(&mut parent, wait);
    parent
        .wait_for_text(".rho-tui-fixture-streamed-write.txt", wait)
        .unwrap();
    parent.inject_key(&Key::Ctrl('o')).unwrap();
    parent.wait_for_text("SECOND_WRITE_FRAGMENT", wait).unwrap();
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
    attach_view::leave(&mut parent, wait);

    // Replay the finished run from its durable journal.
    super::acp_e2e::wait_for_terminal_result(journal.parent().unwrap(), Duration::from_secs(10));
    // The result's delivery turn takes the terminal as it starts and ends,
    // which closes a picker opened meanwhile.
    parent
        .wait_for_screen("the result's delivery turn ended", wait, |screen| {
            screen
                .contents()
                .rsplit_once("streamed write completed successfully")
                .is_some_and(|(_, after)| after.contains("Worked for"))
        })
        .unwrap();
    attach_view::open(&mut parent, wait);
    parent
        .wait_for_text("streamed write completed successfully", wait)
        .unwrap();
    parent.inject_key(&Key::Ctrl('o')).unwrap();
    parent.wait_for_text("SECOND_WRITE_FRAGMENT", wait).unwrap();
    attach_view::leave(&mut parent, wait);
    parent.submit_text("/exit").unwrap();
    assert_eq!(parent.wait_for_exit(wait).unwrap(), 0);
}
