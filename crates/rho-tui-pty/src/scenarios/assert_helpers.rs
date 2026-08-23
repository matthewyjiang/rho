use anyhow::Result;

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

pub(super) fn assert_applied_steer_is_user_line(harness: &mut PtyHarness) -> Result<()> {
    const STEER: &str = "fixture steer detail";
    let has_standalone_line = harness
        .screen()
        .rows_text()
        .iter()
        .any(|row| row.trim() == STEER);
    if !has_standalone_line {
        anyhow::bail!(
            "applied steer did not appear as a transcript user line:\n{}",
            harness.screen().debug_dump()
        );
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
    Ok(())
}
