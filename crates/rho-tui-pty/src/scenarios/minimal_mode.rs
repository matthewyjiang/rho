//! Minimal mode: the transcript lives in the terminal's native scrollback.

use std::time::Duration;

use anyhow::{Context, Result};

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    scenario::{ScenarioOutcome, ScenarioRunner},
    screen::ScreenModel,
};

use super::{config, SETTLE, STARTUP, STREAM};

pub(super) const MINIMAL_MODE_ID: &str = "minimal_mode";

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};
/// Tall enough to lay out the session, too short for any history row.
const SQUEEZED_ROWS: u16 = 6;
/// Streams 400 rows into one answer, a few per paint, then stays live.
const FLOOD: &str = "fixture input flood";
const FLOOD_ROW: &str = "input flood event ";
const FLOOD_ROWS: u32 = 400;
const SECOND: &str = "fixture bulk two";
const BULK_ROWS: u32 = 180;
// Brand/version row is unique to the session header.
const SESSION_HEADER_MARK: &str = "rho  v";

pub(super) fn is_minimal_mode_scenario(name: &str) -> bool {
    name == MINIMAL_MODE_ID
}

// Covers: minimal mode writes each committed row to native scrollback once,
// in order, including rows an entry grows by while the history pane has no
// room; keeps every row through resizes and `/new`; leaves the transcript on
// the main screen at exit, without the alternate screen or mouse capture;
// and a resumed session's scrollback holds its transcript once.
// Owner: interactive TUI
pub(super) fn run_minimal_mode(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    std::fs::write(
        &home.config_path,
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[behavior]
permission_mode = "bypass"
credential_store = "file"
"#,
    )?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_arg("--minimal");
    let spawn = |plan: &RhoLaunchPlan| -> Result<PtyHarness> {
        let mut harness = PtyHarness::spawn_named(plan, MINIMAL_MODE_ID)?;
        harness.enable_timing(runner.record_timing);
        if let Some(root) = &runner.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }
        Ok(harness)
    };

    let mut harness = spawn(&plan)?;
    let mut result = run_session(&mut harness);
    if result.is_ok() {
        result = config::find_latest_session(&home).and_then(|(session_id, _)| {
            harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session_id))?;
            run_resume(&mut harness)
        });
    }
    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    Ok(ScenarioOutcome {
        id: MINIMAL_MODE_ID.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
        timing: harness.timing().clone(),
        artifact_dir: runner.artifact_root.clone(),
    })
}

fn run_session(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("gpt-5.5", STARTUP)?;

    harness.set_phase("squeezed_stream");
    harness.resize(SQUEEZED_ROWS, SIZE.cols)?;
    harness.submit_text(FLOOD)?;
    // Every row the answer grows by while the pane has no room must reach
    // scrollback; a lost row never shows.
    wait_for_history(harness, &format!("{FLOOD_ROW}{FLOOD_ROWS}"), STREAM)?;
    harness.inject_key(&Key::Esc)?;
    wait_for_history(harness, "model interrupted", SETTLE)?;
    // Frames for the short screen must land before it grows, as a terminal
    // reads them in order.
    settle(harness)?;
    harness.resize(SIZE.rows, SIZE.cols)?;
    settle(harness)?;
    let history = harness.screen().history_rows();
    assert_rows_kept(&history, FLOOD_ROW, FLOOD_ROWS)?;

    harness.set_phase("new_session");
    // Growing the screen may have repeated header rows already.
    let headers_before = count_rows(&history, SESSION_HEADER_MARK);
    harness.submit_text("/new")?;
    harness.wait_for_screen("the new session's header", SETTLE, |screen| {
        count_rows(&screen.history_rows(), SESSION_HEADER_MARK) > headers_before
    })?;
    settle(harness)?;
    let history = harness.screen().history_rows();
    assert_rows_kept(&history, FLOOD_ROW, FLOOD_ROWS)?;
    let headers = count_rows(&history, SESSION_HEADER_MARK);
    anyhow::ensure!(
        headers == headers_before + 1,
        "/new took the header count from {headers_before} to {headers}"
    );

    harness.set_phase("stream");
    harness.submit_text(SECOND)?;
    harness.wait_for_screen("turn footer below the answer", STREAM, |screen| {
        let rows = screen.rows_text();
        let last = rows
            .iter()
            .position(|row| row.contains(&format!("{SECOND} line {BULK_ROWS}")));
        last.is_some_and(|last| rows[last..].iter().any(|row| row.contains("Worked for")))
    })?;
    let screen = harness.screen();
    assert_rows_once(&screen.history_rows(), &bulk_row(SECOND), BULK_ROWS)?;
    anyhow::ensure!(
        !screen.contains_text(&format!("{SECOND} line 001")),
        "line 001 is still on screen, not in scrollback:\n{}",
        screen.debug_dump()
    );

    harness.set_phase("resize");
    harness.resize(20, SIZE.cols)?;
    settle(harness)?;
    harness.resize(SIZE.rows, SIZE.cols)?;
    settle(harness)?;
    assert_rows_kept(
        &harness.screen().history_rows(),
        &bulk_row(SECOND),
        BULK_ROWS,
    )?;

    let code = harness.quit_with_exit_command()?;
    anyhow::ensure!(code == 0, "session exited with code {code}");
    harness.set_phase("assert_exit");
    assert_left_on_main_screen(harness)
}

fn run_resume(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("resume");
    harness.wait_for_text(&format!("{SECOND} line {BULK_ROWS}"), STARTUP)?;
    settle(harness)?;
    let history = harness.screen().history_rows();
    assert_rows_once(&history, &bulk_row(SECOND), BULK_ROWS)?;
    let headers = count_rows(&history, SESSION_HEADER_MARK);
    anyhow::ensure!(
        headers == 1,
        "a resumed session shows {headers} headers:\n{}",
        history.join("\n")
    );
    let code = harness.quit_with_exit_command()?;
    anyhow::ensure!(code == 0, "resumed session exited with code {code}");
    Ok(())
}

fn settle(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_quiet(Duration::from_millis(250), SETTLE)
}

/// Waits until a row in scrollback or on screen contains `needle`.
fn wait_for_history(harness: &mut PtyHarness, needle: &str, timeout: WaitTimeout) -> Result<()> {
    harness.wait_for_screen(&format!("{needle:?} in scrollback"), timeout, |screen| {
        screen.history_rows().iter().any(|row| row.contains(needle))
    })
}

fn count_rows(rows: &[String], needle: &str) -> usize {
    rows.iter().filter(|row| row.contains(needle)).count()
}

/// The label of a `fixture bulk` reply's rows.
fn bulk_row(prompt: &str) -> String {
    format!("{prompt} line ")
}

/// Numbers of the fixture rows labeled `label`, in the order they appear.
fn row_numbers(rows: &[String], label: &str) -> Vec<u32> {
    rows.iter()
        .filter_map(|row| {
            let rest = &row[row.find(label)? + label.len()..];
            rest.get(..3)?.parse().ok()
        })
        .collect()
}

fn assert_rows_once(rows: &[String], label: &str, last: u32) -> Result<()> {
    let numbers = row_numbers(rows, label);
    let expected: Vec<u32> = (1..=last).collect();
    anyhow::ensure!(
        numbers == expected,
        "scrollback and screen should hold {label:?} rows 1..={last} once each, in order; got {numbers:?}"
    );
    Ok(())
}

/// A resize may repeat rows in scrollback, since vt100 pulls none back on a
/// taller screen, but no row is lost or shows first out of order.
fn assert_rows_kept(rows: &[String], label: &str, last: u32) -> Result<()> {
    let numbers = row_numbers(rows, label);
    let mut first_seen = Vec::new();
    for number in &numbers {
        if !first_seen.contains(number) {
            first_seen.push(*number);
        }
    }
    let expected: Vec<u32> = (1..=last).collect();
    anyhow::ensure!(
        first_seen == expected,
        "{label:?} rows were lost or reordered; got {numbers:?}"
    );
    Ok(())
}

fn assert_left_on_main_screen(harness: &PtyHarness) -> Result<()> {
    for (sequence, what) in [
        (&b"\x1b[?1049h"[..], "alternate screen"),
        (b"\x1b[?1000h", "mouse capture"),
    ] {
        anyhow::ensure!(
            harness.raw_sequence_occurrences(sequence) == 0,
            "minimal mode turned on the {what}"
        );
    }
    let screen = harness.screen();
    // A write to scrollback, the clear after it, and the frame share one
    // update, each sending the idempotent begin; only an update left open
    // matters, since it freezes the user's display.
    anyhow::ensure!(
        !screen.frame_open(),
        "a synchronized update (ESC[?2026h) is still open after exit"
    );
    anyhow::ensure!(!screen.hide_cursor(), "cursor still hidden after exit");
    assert_rows_kept(&screen.history_rows(), &bulk_row(SECOND), BULK_ROWS)?;
    assert_tail_on_screen(screen)
}

/// The transcript's end stays on screen with the shell's receipt below it,
/// and none of the session's chrome.
fn assert_tail_on_screen(screen: &ScreenModel) -> Result<()> {
    let rows = screen.rows_text();
    let last = rows
        .iter()
        .rposition(|row| row.contains(&format!("{SECOND} line {BULK_ROWS}")))
        .with_context(|| {
            format!(
                "last line left the screen at exit:\n{}",
                screen.debug_dump()
            )
        })?;
    // The composer's frame rules are chrome; the fixture transcript has none.
    if let Some(rule) = rows.iter().position(|row| row.contains("────")) {
        anyhow::bail!(
            "composer chrome still on row {rule} after exit:\n{}",
            screen.debug_dump()
        );
    }
    let receipt = rows
        .iter()
        .position(|row| row.contains("session saved"))
        .with_context(|| format!("no exit receipt:\n{}", screen.debug_dump()))?;
    anyhow::ensure!(
        receipt > last,
        "exit receipt on row {receipt} overwrote the transcript ending on row {last}:\n{}",
        screen.debug_dump()
    );
    Ok(())
}
