//! Pointer input on the bottom chrome: statusline fields and composer
//! attachments.

use std::time::{Duration, Instant};

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use super::{pickers::OPENAI_KEY_ENV, DEFAULT_SIZE, SETTLE, STARTUP};
use crate::{
    env::IsolatedHome,
    harness::{PtyHarness, WaitTimeout},
    keys::{Key, MouseButton},
    scenario::{Scenario, Step},
};

const CLICK: WaitTimeout = WaitTimeout::secs(5, "status click");

/// Model id the isolated home configures; painted on the statusline.
const MODEL: &str = "gpt-5.5";

/// 0-based `(column, row)` of `needle` on the last screen row, the
/// statusline fields row.
fn statusline_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    let rows = harness.screen().rows_text();
    let row = rows.len().saturating_sub(1);
    if let Some(offset) = rows.get(row).and_then(|line| line.find(needle)) {
        let column = UnicodeWidthStr::width(&rows[row][..offset]);
        return Ok((column as u16, row as u16));
    }
    anyhow::bail!(
        "{needle:?} not on the statusline row:\n{}",
        harness.screen().debug_dump()
    );
}

/// Whether every cell of `width` columns from 0-based (`column`, `row`) is
/// underlined, the statusline hover emphasis.
fn span_underlined(harness: &PtyHarness, (column, row): (u16, u16), width: u16) -> bool {
    let screen = harness.screen();
    (column..column + width).all(|col| screen.cell(row, col).is_some_and(|cell| cell.underline))
}

/// Poll until the model span's underline matches `want`, failing with `what`.
fn wait_for_underline(
    harness: &mut PtyHarness,
    cell: (u16, u16),
    want: bool,
    what: &str,
) -> Result<()> {
    let width = UnicodeWidthStr::width(MODEL) as u16;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        if span_underlined(harness, cell, width) == want {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("{what}:\n{}", harness.screen().debug_dump());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Covers: hovering a clickable statusline field lifts it and leaving drops
// the lift; a click on the model field runs `/model` and opens the picker.
// Owner: interactive UX (PTY).
fn hover_then_click_model_field(harness: &mut PtyHarness) -> Result<()> {
    let cell = statusline_cell(harness, MODEL)?;
    wait_for_underline(harness, cell, false, "model field starts lifted")?;
    // SGR mouse coordinates are 1-based.
    let (column, row) = (cell.0 + 2, cell.1 + 1);
    harness.mouse_move(column, row)?;
    wait_for_underline(harness, cell, true, "hover did not lift the model field")?;
    harness.mouse_move(1, 1)?;
    wait_for_underline(harness, cell, false, "lift stayed after the pointer left")?;
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)?;
    harness.wait_for_text("select model", CLICK)
}

const MODEL_FIELD_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: MODEL,
        timeout: STARTUP,
    },
    Step::Phase("hover_and_click_model"),
    Step::Custom(hover_then_click_model_field),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const STATUSLINE_MODEL_CLICK_SCENARIO: Scenario = Scenario::new(
    "statusline_model_click",
    "Lift a statusline field on hover and open the model picker by clicking the model",
    DEFAULT_SIZE,
    MODEL_FIELD_CLICK_STEPS,
    /* smoke */ false,
)
.with_env(OPENAI_KEY_ENV);

const IMAGE_FILE: &str = "click-remove-fixture.png";

/// Smallest valid 4x4 RGBA PNG. The isolated terminal has no graphics
/// protocol, so the attachment paints as a label row, which is still a target.
const PNG_4X4: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x06, 0x00, 0x00, 0x00, 0xa9, 0xf1, 0x9e,
    0x7e, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x10, 0x50, 0x30, 0xf8,
    0x8f, 0x8c, 0x19, 0x48, 0x17, 0x00, 0x00, 0xd3, 0x5a, 0x15, 0xf1, 0xf5, 0xca, 0xfe, 0xb4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Composer label of the pasted fixture: first attachment, 75-byte PNG.
const IMAGE_LABEL: &str = "[image 1: image/png 75 B]";

/// Affordance painted over a hovered attachment label.
const REMOVE_HINT: &str = "✕ remove";

fn setup_image(home: &IsolatedHome) -> Result<()> {
    std::fs::write(home.workspace.join(IMAGE_FILE), PNG_4X4)?;
    Ok(())
}

fn paste_image_path(harness: &mut PtyHarness) -> Result<()> {
    let path = harness
        .working_directory()
        .ok_or_else(|| anyhow::anyhow!("attachment scenario has no working directory"))?
        .join(IMAGE_FILE);
    harness.paste(&path.to_string_lossy())
}

/// 1-based SGR cell inside the lowest on-screen occurrence of `needle`.
fn label_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate().rev() {
        if let Some(offset) = line.find(needle) {
            let column = UnicodeWidthStr::width(&line[..offset]);
            return Ok((column as u16 + 3, row as u16 + 1));
        }
    }
    anyhow::bail!("{needle:?} not found:\n{}", harness.screen().debug_dump());
}

// Covers: hovering a composer attachment shows the remove affordance and a
// click removes that attachment from the composer.
// Owner: interactive UX (PTY).
fn hover_then_click_removes_attachment(harness: &mut PtyHarness) -> Result<()> {
    let (column, row) = label_cell(harness, IMAGE_LABEL)?;
    harness.mouse_move(column, row)?;
    harness.wait_for_text(REMOVE_HINT, CLICK)?;
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)?;
    harness.wait_for_text_gone(REMOVE_HINT, CLICK)?;
    if harness.screen().contains_text("[image 1:") {
        anyhow::bail!(
            "clicked attachment is still in the composer:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

const ATTACHMENT_CLICK_REMOVE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: MODEL,
        timeout: STARTUP,
    },
    Step::Phase("attach_image"),
    Step::Custom(paste_image_path),
    Step::WaitText {
        text: IMAGE_LABEL,
        timeout: SETTLE,
    },
    Step::Phase("hover_and_click_remove"),
    Step::Custom(hover_then_click_removes_attachment),
    Step::ExitCommand,
];

pub(super) const ATTACHMENT_CLICK_REMOVE_SCENARIO: Scenario = Scenario::new(
    "attachment_click_remove",
    "Show a remove affordance on a hovered composer attachment and remove it by click",
    DEFAULT_SIZE,
    ATTACHMENT_CLICK_REMOVE_STEPS,
    /* smoke */ false,
)
.with_setup(setup_image);
