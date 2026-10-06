use std::time::{Duration, Instant};

use anyhow::{ensure, Result};

use super::STREAM;
use crate::harness::PtyHarness;

pub(super) fn assert_inline_shell_cancelled(harness: &mut PtyHarness) -> Result<()> {
    if harness.screen().contains_text("cancel-escaped-output") {
        anyhow::bail!("inline shell produced output after Escape cancelled it");
    }
    Ok(())
}

pub(super) fn assert_idle_shell_still_streaming(harness: &mut PtyHarness) -> Result<()> {
    if harness.screen().contains_text("idle-stream-end") {
        anyhow::bail!("idle shell output was not rendered until the command completed");
    }
    Ok(())
}

pub(super) fn assert_terminal_restored(harness: &mut PtyHarness) -> Result<()> {
    // After a clean exit, ratatui/crossterm must leave the alternate screen.
    // Mouse disable alone is not enough: a regression that skips ESC[?1049l
    // would leave the user stuck in the alternate screen.
    let raw = harness.raw_output();
    let left = raw.windows(8).any(|window| window == b"\x1b[?1049l")
        || String::from_utf8_lossy(raw).contains("?1049l");
    if !left {
        anyhow::bail!("did not observe alternate-screen leave sequence (ESC[?1049l)");
    }
    // Frames paint inside synchronized updates so the caret does not jump
    // across cells. An update left open freezes the user's terminal display.
    let begun = harness.raw_sequence_occurrences(b"\x1b[?2026h");
    let ended = harness.raw_sequence_occurrences(b"\x1b[?2026l");
    if begun == 0 || begun != ended {
        anyhow::bail!(
            "synchronized updates unbalanced: {begun} begin (ESC[?2026h), {ended} end (ESC[?2026l)"
        );
    }
    Ok(())
}

/// Waits for the turn that printed `marker` to finish. Idle-only commands and
/// fresh prompts need it: seeing the response is not enough, because the
/// provider can still own the turn. Waits for the durable receipt after this
/// marker, not an earlier turn's receipt.
pub(super) fn wait_for_turn_completion_after(harness: &mut PtyHarness, marker: &str) -> Result<()> {
    let deadline = Instant::now() + STREAM.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let screen = harness.screen().contents();
        if screen
            .rfind(marker)
            .is_some_and(|start| screen[start..].contains("Worked for"))
        {
            return Ok(());
        }
        ensure!(
            harness.is_running() && Instant::now() < deadline,
            "turn did not finish before the next idle command:\n{screen}"
        );
    }
}
