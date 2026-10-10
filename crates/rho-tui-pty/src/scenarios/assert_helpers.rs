use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};

use super::STREAM;
use crate::harness::{PtyHarness, WaitTimeout};

/// Waits until the cursor's row, the composer line, shows `text`. The
/// transcript repeats submitted prompts, so screen-wide text is ambiguous.
pub(super) fn wait_for_composer(harness: &mut PtyHarness, text: &str) -> Result<()> {
    const COMPOSER: WaitTimeout = WaitTimeout::secs(5, "composer text");
    let deadline = Instant::now() + COMPOSER.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let screen = harness.screen();
        let (row, _) = screen.cursor();
        let line = screen.rows_text().get(usize::from(row)).cloned();
        if line.as_deref().is_some_and(|line| line.contains(text)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "composer never showed {text:?}; cursor row: {line:?}\n{}",
                screen.debug_dump()
            );
        }
    }
}

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

/// Find one saved session by a committed transcript marker, never timestamps
/// that can tie. Matrix HOME is a sibling of the harness working directory.
pub(super) fn saved_session_with_text(harness: &PtyHarness, marker: &str) -> Result<String> {
    let root = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?
        .join("home/.rho/sessions");
    let mut directories = vec![root];
    let mut matching_ids = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            } else if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                let contents = std::fs::read_to_string(entry.path())?;
                if contents.contains(marker) {
                    let header: serde_json::Value = serde_json::from_str(
                        contents.lines().next().context("empty session file")?,
                    )?;
                    matching_ids.push(
                        header["id"]
                            .as_str()
                            .context("missing session id")?
                            .to_owned(),
                    );
                }
            }
        }
    }
    ensure!(
        matching_ids.len() == 1,
        "expected one committed session matching {marker:?}, found {}",
        matching_ids.len()
    );
    Ok(matching_ids.remove(0))
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
