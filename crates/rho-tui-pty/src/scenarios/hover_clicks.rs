//! Pointer hover lift on composer choices, and clicks on inline list pickers.

use std::time::{Duration, Instant};

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    harness::WaitTimeout,
    keys::{Key, MouseButton},
    scenario::{Scenario, Step},
    PtyHarness,
};

const CLICK: WaitTimeout = WaitTimeout::secs(5, "picker click");

/// 1-based SGR cell of the first character of `needle` on the lowest row
/// where `row_matches` holds. The composer sits below the transcript, whose
/// tool cards can echo the same labels.
fn cell_of(
    harness: &PtyHarness,
    needle: &str,
    row_matches: &dyn Fn(&str) -> bool,
) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate().rev() {
        if !row_matches(line.as_str()) {
            continue;
        }
        if let Some(offset) = line.find(needle) {
            let column = UnicodeWidthStr::width(&line[..offset]);
            return Ok((column as u16 + 1, row as u16 + 1));
        }
    }
    anyhow::bail!("'{needle}' not found:\n{}", harness.screen().debug_dump());
}

/// Bold flags of the `len` cells starting at 1-based SGR `cell`. The hover
/// lift paints strong (bold) text, which a plain row does not use.
fn span_bold(harness: &PtyHarness, (column, row): (u16, u16), len: u16) -> Vec<bool> {
    let screen = harness.screen();
    (column - 1..column - 1 + len)
        .filter_map(|col| screen.cell(row - 1, col).map(|cell| cell.bold))
        .collect()
}

/// Poll until every cell of the span reads `bold`, failing with `what`.
fn wait_for_span_bold(
    harness: &mut PtyHarness,
    cell: (u16, u16),
    len: u16,
    bold: bool,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Polling drains PTY output for up to the budget, so it paces the loop.
        harness.poll(Duration::from_millis(20));
        let look = span_bold(harness, cell, len);
        if !look.is_empty() && look.iter().all(|cell_bold| *cell_bold == bold) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "{what}; bold flags at {cell:?}: {look:?}\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

fn click(harness: &mut PtyHarness, (column, row): (u16, u16)) -> Result<()> {
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)
}

/// Two clicks on one cell. Moving away first ends any earlier click sequence,
/// so a single click just before cannot pair with the first of these.
fn double_click(harness: &mut PtyHarness, cell: (u16, u16)) -> Result<()> {
    harness.mouse_move(1, 1)?;
    click(harness, cell)?;
    click(harness, cell)
}

// Covers: hovering an unfocused questionnaire choice lifts its label to strong
// text, and moving the pointer off the choices drops the lift.
// Owner: interactive UX (PTY).
fn hover_lifts_questionnaire_choice(harness: &mut PtyHarness) -> Result<()> {
    // `red` is the focused default; `blue` renders as a plain row.
    let blue = cell_of(harness, "blue", &|line| line.contains("○ blue"))?;
    let len = "blue".len() as u16;
    wait_for_span_bold(harness, blue, len, false, "blue starts unlifted")?;
    harness.mouse_move(blue.0, blue.1)?;
    wait_for_span_bold(harness, blue, len, true, "hover did not lift blue")?;
    harness.mouse_move(1, 1)?;
    wait_for_span_bold(
        harness,
        blue,
        len,
        false,
        "the lift stayed after the pointer left",
    )
}

const QUESTIONNAIRE_HOVER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture questionnaire"),
    Step::WaitText {
        text: "A cool primary color",
        timeout: STREAM,
    },
    Step::Phase("hover_choice"),
    Step::Custom(hover_lifts_questionnaire_choice),
    // Answer so the turn ends and /exit reaches an idle composer.
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "questionnaire response observed exactly 1 time",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const QUESTIONNAIRE_HOVER_SCENARIO: Scenario = Scenario::new(
    "questionnaire_hover",
    "Lift a questionnaire choice under the pointer and drop the lift when it leaves",
    DEFAULT_SIZE,
    QUESTIONNAIRE_HOVER_STEPS,
    /*smoke*/ false,
);

/// Whether `line` is the inline picker row for `label`, highlighted or not.
fn picker_row(line: &str, label: &str) -> bool {
    line.trim_start()
        .trim_start_matches('→')
        .trim_start()
        .starts_with(label)
}

// Covers: an inline (non-overlay) list picker lifts the hovered row, a click
// selects a row without submitting, and a double click submits it like Enter.
// Owner: interactive UX (PTY).
fn hover_click_then_double_click_inline_picker_row(harness: &mut PtyHarness) -> Result<()> {
    // `/config` opens on Models; Tools is a plain row.
    let tools = cell_of(harness, "Tools", &|line| picker_row(line, "Tools"))?;
    let len = "Tools".len() as u16;
    wait_for_span_bold(harness, tools, len, false, "Tools starts unlifted")?;
    harness.mouse_move(tools.0, tools.1)?;
    wait_for_span_bold(
        harness,
        tools,
        len,
        true,
        "hover did not lift the Tools row",
    )?;

    click(harness, tools)?;
    harness.wait_for_text("→ Tools", CLICK)?;
    let screen = harness.screen();
    if screen.contains_text("Config / Tools")
        || !screen.contains_text("Config · saves automatically")
    {
        anyhow::bail!(
            "a single click submitted the picker:\n{}",
            screen.debug_dump()
        );
    }

    double_click(harness, tools)?;
    harness.wait_for_text("Config / Tools", CLICK)
}

const INLINE_PICKER_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_config"),
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Config · saves automatically",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Providers",
        timeout: SETTLE,
    },
    Step::Phase("click_rows"),
    Step::Custom(hover_click_then_double_click_inline_picker_row),
    Step::Key(Key::Esc),
    Step::Key(Key::Esc),
    Step::ExitCommand,
];

pub(super) const INLINE_PICKER_CLICK_SCENARIO: Scenario = Scenario::new(
    "inline_picker_click",
    "Hover, select with a click, and submit with a double click in an inline list picker",
    DEFAULT_SIZE,
    INLINE_PICKER_CLICK_STEPS,
    /*smoke*/ false,
);

// Covers: a click on an overlay picker nav row selects it without closing the
// overlay, and a double click submits it like Enter.
// Owner: interactive UX (PTY).
fn click_then_double_click_overlay_nav_row(harness: &mut PtyHarness) -> Result<()> {
    // Only the help overlay's nav pane paints the key label inside the box
    // border; the detail pane shows the selected row's text.
    // "External editor" is a short nav label, so narrow panes cannot
    // truncate it; its detail text only appears once the row is selected.
    let editor = cell_of(harness, "External editor", &|line| line.contains('│'))?;
    click(harness, editor)?;
    harness.wait_for_text("Open the composer contents in VISUAL", CLICK)?;
    if !harness.screen().contains_text("Keyboard shortcuts") {
        anyhow::bail!(
            "a single click closed the overlay:\n{}",
            harness.screen().debug_dump()
        );
    }
    double_click(harness, editor)?;
    harness.wait_for_text_gone("Keyboard shortcuts", CLICK)
}

const OVERLAY_PICKER_DOUBLE_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_help"),
    Step::SubmitText("/help"),
    Step::WaitText {
        text: "Keyboard shortcuts",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "External editor",
        timeout: SETTLE,
    },
    Step::Phase("click_rows"),
    Step::Custom(click_then_double_click_overlay_nav_row),
    Step::ExitCommand,
];

pub(super) const OVERLAY_PICKER_DOUBLE_CLICK_SCENARIO: Scenario = Scenario::new(
    "overlay_picker_double_click",
    "Select an overlay picker row with a click and submit it with a double click",
    DEFAULT_SIZE,
    OVERLAY_PICKER_DOUBLE_CLICK_STEPS,
    /*smoke*/ false,
);
