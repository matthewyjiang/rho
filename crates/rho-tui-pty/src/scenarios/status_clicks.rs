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
    screen::ScreenModel,
};

const CLICK: WaitTimeout = WaitTimeout::secs(5, "status click");

/// Model id the isolated home configures; painted on the statusline.
const MODEL: &str = "gpt-5.5";

/// Display name of the configured provider, painted left of the model.
const PROVIDER: &str = "OpenAI";

/// Reasoning levels the statusline can paint as the row's last field.
const REASONING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The statusline fields row: the last screen row.
fn statusline_row(screen: &ScreenModel) -> (usize, String) {
    let rows = screen.rows_text();
    let row = rows.len().saturating_sub(1);
    (row, rows.get(row).cloned().unwrap_or_default())
}

/// 0-based `(column, row)` of `needle` on the statusline fields row.
fn statusline_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    let (row, line) = statusline_row(harness.screen());
    if let Some(offset) = line.find(needle) {
        let column = UnicodeWidthStr::width(&line[..offset]);
        return Ok((column as u16, row as u16));
    }
    anyhow::bail!(
        "{needle:?} not on the statusline row:\n{}",
        harness.screen().debug_dump()
    );
}

/// The reasoning level painted as the statusline row's last field, if any.
fn painted_reasoning(screen: &ScreenModel) -> Option<String> {
    let (_, line) = statusline_row(screen);
    let last = line.split_whitespace().last()?;
    REASONING_LEVELS.contains(&last).then(|| last.to_owned())
}

/// Whether every cell of `width` columns from 0-based (`column`, `row`) is
/// underlined, the statusline hover emphasis.
fn span_underlined(harness: &PtyHarness, (column, row): (u16, u16), width: u16) -> bool {
    let screen = harness.screen();
    (column..column + width).all(|col| screen.cell(row, col).is_some_and(|cell| cell.underline))
}

/// Poll until the `text` span at `cell` has underline `want`, failing with `what`.
fn wait_for_underline(
    harness: &mut PtyHarness,
    cell: (u16, u16),
    text: &str,
    want: bool,
    what: &str,
) -> Result<()> {
    let width = UnicodeWidthStr::width(text) as u16;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.pump(Duration::from_millis(20));
        if span_underlined(harness, cell, width) == want {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("{what}:\n{}", harness.screen().debug_dump());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Press and release the primary button at 0-based `cell`.
fn click_cell(harness: &mut PtyHarness, (column, row): (u16, u16)) -> Result<()> {
    // SGR mouse coordinates are 1-based.
    click(harness, (column + 1, row + 1))
}

// Covers: provider and model are one click target. Hovering the provider
// lifts the model too, leaving drops the lift, and a click on the provider
// runs `/model` and opens the model picker.
// Owner: interactive UX (PTY).
fn hover_then_click_provider_and_model(harness: &mut PtyHarness) -> Result<()> {
    let provider = statusline_cell(harness, PROVIDER)?;
    let model = statusline_cell(harness, MODEL)?;
    wait_for_underline(harness, model, MODEL, false, "model field starts lifted")?;
    let pointer = (provider.0 + 2, provider.1 + 1);
    harness.mouse_move(pointer.0, pointer.1)?;
    wait_for_underline(
        harness,
        provider,
        PROVIDER,
        true,
        "hover did not lift the provider",
    )?;
    wait_for_underline(
        harness,
        model,
        MODEL,
        true,
        "provider hover did not lift the model",
    )?;
    harness.mouse_move(1, 1)?;
    wait_for_underline(
        harness,
        model,
        MODEL,
        false,
        "lift stayed after the pointer left",
    )?;
    click_cell(harness, provider)?;
    harness.wait_for_text("select model", CLICK)
}

// Covers: a click on the reasoning field steps the effort in place: the
// field shows the next level and no picker opens.
// Owner: interactive UX (PTY).
fn click_reasoning_cycles_effort(harness: &mut PtyHarness) -> Result<()> {
    let Some(before) = painted_reasoning(harness.screen()) else {
        anyhow::bail!(
            "no reasoning field on the statusline row:\n{}",
            harness.screen().debug_dump()
        );
    };
    click_cell(harness, statusline_cell(harness, &before)?)?;
    harness.wait_for_screen(&format!("reasoning to leave {before:?}"), CLICK, |screen| {
        painted_reasoning(screen).is_some_and(|level| level != before)
    })?;
    if harness.screen().contains_text("select model") {
        anyhow::bail!(
            "a reasoning click opened the model picker:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

const MODEL_FIELD_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: MODEL,
        timeout: STARTUP,
    },
    Step::Phase("hover_and_click_provider"),
    Step::Custom(hover_then_click_provider_and_model),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "select model",
        timeout: SETTLE,
    },
    Step::Phase("click_reasoning"),
    Step::Custom(click_reasoning_cycles_effort),
    Step::ExitCommand,
];

pub(super) const STATUSLINE_MODEL_CLICK_SCENARIO: Scenario = Scenario::new(
    "statusline_model_click",
    "Open the model picker from the provider and model, and cycle reasoning from its field",
    DEFAULT_SIZE,
    MODEL_FIELD_CLICK_STEPS,
    /* smoke */ false,
)
.with_env(OPENAI_KEY_ENV);

const COPILOT_TOKEN_ENV: &[(&str, &str)] = &[("GITHUB_COPILOT_TOKEN", "fixture-copilot-token")];

/// A model whose provider never takes a reasoning level.
const NO_REASONING_MODEL: &str = "gpt-4.1";

fn setup_no_reasoning_model(home: &IsolatedHome) -> Result<()> {
    std::fs::write(
        &home.config_path,
        format!(
            r#"web_search.mode = "off"

[model]
provider = "github-copilot"
model = "{NO_REASONING_MODEL}"

[behavior]
credential_store = "file"
"#
        ),
    )?;
    Ok(())
}

// Covers: a model without reasoning paints no reasoning target, so there is
// nothing for a click to cycle: the row ends at the model.
// Owner: interactive UX (PTY).
fn no_reasoning_target_without_reasoning(harness: &mut PtyHarness) -> Result<()> {
    let (_, line) = statusline_row(harness.screen());
    if !line.trim_end().ends_with(NO_REASONING_MODEL) {
        anyhow::bail!(
            "statusline row does not end at the model:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

const NO_REASONING_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: NO_REASONING_MODEL,
        timeout: STARTUP,
    },
    Step::Phase("no_reasoning_target"),
    Step::Custom(no_reasoning_target_without_reasoning),
    Step::ExitCommand,
];

pub(super) const STATUSLINE_NO_REASONING_CLICK_SCENARIO: Scenario = Scenario::new(
    "statusline_no_reasoning_click",
    "Paint no reasoning target for a model without reasoning",
    DEFAULT_SIZE,
    NO_REASONING_CLICK_STEPS,
    /* smoke */ false,
)
.with_setup(setup_no_reasoning_model)
.with_env(COPILOT_TOKEN_ENV);

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

/// Remove button painted at the end of every attachment label row.
const REMOVE_BUTTON: &str = "✕";

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

/// 1-based SGR cells of the attachment label and of the remove button on
/// the same (lowest) row.
fn label_and_button_cells(harness: &PtyHarness) -> Result<((u16, u16), (u16, u16))> {
    for (row, line) in harness.screen().rows_text().iter().enumerate().rev() {
        let Some(label) = line.find(IMAGE_LABEL) else {
            continue;
        };
        let Some(button) = line[label..]
            .find(REMOVE_BUTTON)
            .map(|offset| label + offset)
        else {
            break;
        };
        let cell = |offset: usize| {
            (
                UnicodeWidthStr::width(&line[..offset]) as u16 + 1,
                row as u16 + 1,
            )
        };
        return Ok((cell(label + 2), cell(button)));
    }
    anyhow::bail!(
        "attachment label with its remove button not found:\n{}",
        harness.screen().debug_dump()
    );
}

fn click(harness: &mut PtyHarness, (column, row): (u16, u16)) -> Result<()> {
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)
}

// Covers: a click on an attachment's label keeps it (a stray click on a
// preview must never delete it); a click on its always-painted remove button
// removes it.
// Owner: interactive UX (PTY).
fn only_the_remove_button_removes_attachment(harness: &mut PtyHarness) -> Result<()> {
    let (label, button) = label_and_button_cells(harness)?;
    click(harness, label)?;
    // A remove would repaint without the label; the button stays as a marker
    // that the frame after the click has settled.
    harness.wait_for_text(IMAGE_LABEL, CLICK)?;
    if !harness.screen().contains_text(IMAGE_LABEL) {
        anyhow::bail!(
            "a click on the label removed the attachment:\n{}",
            harness.screen().debug_dump()
        );
    }
    click(harness, button)?;
    harness.wait_for_text_gone(IMAGE_LABEL, CLICK)
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
    Step::Phase("click_remove_button"),
    Step::Custom(only_the_remove_button_removes_attachment),
    Step::ExitCommand,
];

pub(super) const ATTACHMENT_CLICK_REMOVE_SCENARIO: Scenario = Scenario::new(
    "attachment_click_remove",
    "Keep a composer attachment on a label click and remove it with its remove button",
    DEFAULT_SIZE,
    ATTACHMENT_CLICK_REMOVE_STEPS,
    /* smoke */ false,
)
.with_setup(setup_image);
