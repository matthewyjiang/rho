use std::{fs::OpenOptions, io::Write};

use anyhow::{ensure, Context, Result};
use serde_json::json;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::{Key, MouseButton},
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{sessions_hub::write_seed_session, SETTLE, STARTUP, STREAM};

const SESSION_ID: &str = "33333333-0000-4000-8000-000000000003";
const EVIDENCE: &str = "quartz evidence remains hidden until expanded";

// Covers: short session evidence must stay hidden behind its receipt, then be
// recoverable with keyboard and mouse without losing source identity or ranges.
// Owner: interactive TUI. Existing tool-card scenarios only cover long bodies.
pub(super) const SESSIONS_TOOL_SCENARIO: Scenario = Scenario::new(
    "sessions_tool_cards",
    "Search and read real session evidence through collapsed and expanded cards",
    // Keep the full anchor and character range on one row for identity checks.
    PtySize {
        rows: 50,
        cols: 160,
    },
    &[Step::Custom(exercise_cards), Step::ExitCommand],
    /*smoke*/ true,
)
.with_setup(setup);

fn setup(home: &IsolatedHome) -> Result<()> {
    let path = write_seed_session(home, &home.workspace, &home.workspace, SESSION_ID, EVIDENCE)?;
    // A following anchor is not a continuation of this passage. A full read
    // must not claim "more available" merely because next_anchor is populated.
    writeln!(
        OpenOptions::new().append(true).open(path)?,
        "{}",
        json!({
            "type": "message", "timestamp": "3",
            "message": {"User": [{"Text": "a separate historical passage"}]},
        }),
    )?;
    Ok(())
}

fn exercise_cards(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("gpt-5.5", STARTUP)?;
    harness.set_phase("search_collapsed");
    harness.submit_text("fixture sessions search")?;
    harness.wait_for_text("sessions search complete", STREAM)?;
    // Title insertion shifts card rows; settle it before the later mouse steps.
    harness.wait_for_text("session titled:", STREAM)?;
    harness.assert_screen_contains("sessions.search(\"quartz\")")?;
    harness.assert_screen_contains("1 matching session · repo")?;
    assert_evidence_hidden(harness)?;

    harness.set_phase("search_expanded");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text(EVIDENCE, SETTLE)?;
    harness.assert_screen_contains(&format!("session {SESSION_ID} ·"))?;
    let screen = harness.screen().contents();
    let session = screen
        .lines()
        .find_map(|line| line.split_once("handle: ")?.1.split_whitespace().next())
        .context("expanded search omitted full session handle")?
        .to_owned();
    ensure!(
        session.len() == 64 && session.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "incomplete search handle: {session}"
    );
    let anchor = screen
        .lines()
        .find_map(|line| {
            line.split_once("user · ")?
                .1
                .split_once(" · chars ")
                .map(|(anchor, _)| anchor.to_owned())
        })
        .context("expanded search omitted role, anchor or character range")?;
    harness.assert_screen_contains(&format!("chars 0–{} of {}", EVIDENCE.len(), EVIDENCE.len()))?;
    ensure!(
        screen.contains("/workspace"),
        "expanded search omitted workspace"
    );
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text_gone(EVIDENCE, SETTLE)?;
    assert_evidence_hidden(harness)?;

    harness.set_phase("read_collapsed");
    harness.submit_text("fixture sessions read")?;
    harness.wait_for_text("sessions read complete", STREAM)?;
    let header = format!("sessions.read({})", &session[..8]);
    harness.assert_screen_contains(&header)?;
    let summary = format!("user · chars 0–{} of {}", EVIDENCE.len(), EVIDENCE.len());
    harness.assert_screen_contains(&summary)?;
    ensure!(
        !harness.screen().contents().contains("more available"),
        "next_anchor was incorrectly presented as a continued passage"
    );
    assert_evidence_hidden(harness)?;

    harness.set_phase("read_expanded_by_click");
    click_card(harness, &header)?;
    harness.wait_for_text(EVIDENCE, SETTLE)?;
    harness.assert_screen_contains(&format!("session: {session}"))?;
    harness.assert_screen_contains(&format!("anchor: {anchor}"))?;
    harness.assert_screen_contains("next anchor:")?;
    click_card(harness, &header)?;
    harness.wait_for_text_gone(EVIDENCE, SETTLE)?;
    assert_evidence_hidden(harness)?;

    harness.set_phase("partial_read_receipt");
    harness.submit_text("fixture sessions read partial")?;
    harness.wait_for_text("sessions read partial complete", STREAM)?;
    harness.assert_screen_contains(&format!(
        "user · chars 0–16 of {} · more available",
        EVIDENCE.len()
    ))?;
    assert_evidence_hidden(harness)?;

    harness.set_phase("empty_search_receipt");
    harness.submit_text("fixture sessions empty")?;
    harness.wait_for_text("sessions empty complete", STREAM)?;
    harness.assert_screen_contains("sessions.search(\"absentneedle\")")?;
    harness.assert_screen_contains("no matching sessions · repo")?;
    assert_evidence_hidden(harness)?;

    harness.set_phase("error_stays_visible");
    harness.submit_text("fixture sessions error")?;
    harness.wait_for_text("sessions error complete", STREAM)?;
    harness.assert_screen_contains("✗ sessions.search")?;
    harness.assert_screen_contains("sessions search query must not be empty")
}

fn assert_evidence_hidden(harness: &PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    for evidence in ["quartz evidence", SESSION_ID, "handle:", "anchor:"] {
        ensure!(
            !screen.contains(evidence),
            "collapsed evidence leaked {evidence:?}:\n{screen}"
        );
    }
    Ok(())
}

fn click_card(harness: &mut PtyHarness, header: &str) -> Result<()> {
    let row = harness
        .screen()
        .rows_text()
        .iter()
        .position(|line| line.contains(header))
        .context("read card header missing")? as u16
        + 1;
    harness.mouse(MouseButton::Left, 3, row, true)?;
    harness.mouse(MouseButton::Left, 3, row, false)
}
