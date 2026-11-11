//! Opening and leaving the TUI's in-place attach view from a PTY test.

use rho_tui_pty::{Key, PtyHarness, WaitTimeout};

/// Opens the only workspace run, which runs as `agent_id`, in the in-place
/// attach view. The picker starts on running runs; Ctrl+R lists finished
/// ones too, so this works before and after the run ends.
pub fn open(parent: &mut PtyHarness, agent_id: &str, wait: WaitTimeout) {
    parent.submit_text("/attach").unwrap();
    parent.wait_for_text("attach subagent", wait).unwrap();
    parent.inject_key(&Key::Ctrl('r')).unwrap();
    parent.wait_for_text("running only", wait).unwrap();
    // Enter on an empty list only closes the picker, so wait for the run's
    // row, which the picker labels with its agent.
    parent.wait_for_text(agent_id, wait).unwrap();
    parent.inject_key(&Key::Enter).unwrap();
    parent.wait_for_text("q back", wait).unwrap();
}

/// Returns from the attach view to the parent's composer.
pub fn leave(parent: &mut PtyHarness, wait: WaitTimeout) {
    parent.inject_key(&Key::Esc).unwrap();
    parent.wait_for_text_gone("q back", wait).unwrap();
}
