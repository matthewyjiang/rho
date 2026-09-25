//! Pointer input on the `/` command palette and the `@` path palette.

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use super::{SETTLE, STARTUP};
use crate::{
    env::IsolatedHome,
    harness::{PtyHarness, WaitTimeout},
    keys::{Key, MouseButton},
    pty::PtySize,
    scenario::{Scenario, Step},
};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

const CLICK: WaitTimeout = WaitTimeout::secs(5, "palette click");

/// 1-based SGR cell of the lowest on-screen occurrence of `needle`. The
/// palette sits just above the composer, below anything the transcript shows.
fn click_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate().rev() {
        if let Some(offset) = line.find(needle) {
            let column = UnicodeWidthStr::width(&line[..offset]);
            return Ok((column as u16 + 1, row as u16 + 1));
        }
    }
    anyhow::bail!("'{needle}' not found:\n{}", harness.screen().debug_dump());
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

/// Fails unless some screen row reads exactly `composer` (the `> ` prompt
/// plus the typed text), so a palette row that merely contains it can't pass.
fn assert_composer_reads(harness: &PtyHarness, composer: &str) -> Result<()> {
    if harness
        .screen()
        .rows_text()
        .iter()
        .any(|line| line.trim() == composer)
    {
        return Ok(());
    }
    anyhow::bail!(
        "composer does not read {composer:?}:\n{}",
        harness.screen().debug_dump()
    )
}

// Covers: a click on a `/` palette row moves the highlight without running or
// completing anything, the wheel over the palette steps the highlight instead
// of scrolling the transcript, and a double click completes the row like Tab
// (never submitting like Enter).
// Owner: interactive UX (PTY).
fn click_wheel_then_double_click_command_row(harness: &mut PtyHarness) -> Result<()> {
    // `/co` lists /compact, /computer, /config, /copy; /compact starts highlighted.
    let config = click_cell(harness, "/config")?;
    click(harness, config)?;
    harness.wait_for_text("> /config", CLICK)?;
    assert_composer_reads(harness, "> /co")?;

    let (column, row) = config;
    harness.mouse(MouseButton::WheelDown, column, row, true)?;
    harness.wait_for_text("> /copy", CLICK)?;
    harness.mouse(MouseButton::WheelUp, column, row, true)?;
    harness.mouse(MouseButton::WheelUp, column, row, true)?;
    harness.wait_for_text("> /computer", CLICK)?;
    assert_composer_reads(harness, "> /co")?;

    // /computer completes to `/computer ` and reopens on its argument rows; a
    // submitted /computer would print its status and clear the composer.
    let computer = click_cell(harness, "/computer")?;
    double_click(harness, computer)?;
    harness.wait_for_text("/computer status", CLICK)?;
    harness.wait_for_text_gone("/compact", CLICK)?;
    assert_composer_reads(harness, "> /computer")
}

const SLASH_PALETTE_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_palette"),
    Step::TypeText("/co"),
    Step::WaitText {
        text: "/copy",
        timeout: SETTLE,
    },
    // `/` and `/c` both list /changelog; its absence means `/co` is painted,
    // so the rows clicked below cannot shift under the pointer.
    Step::WaitTextGone {
        text: "/changelog",
        timeout: SETTLE,
    },
    Step::Phase("click_rows"),
    Step::Custom(click_wheel_then_double_click_command_row),
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const SLASH_PALETTE_CLICK_SCENARIO: Scenario = Scenario::new(
    "slash_palette_click",
    "Highlight slash palette rows by click and wheel, and complete one with a double click",
    SIZE,
    SLASH_PALETTE_CLICK_STEPS,
    /* smoke */ false,
);

const FILE_ALPHA: &str = "alpha-click-fixture.txt";
const FILE_BETA: &str = "beta-click-fixture.txt";

fn setup_click_files(home: &IsolatedHome) -> Result<()> {
    std::fs::write(home.workspace.join(FILE_ALPHA), "alpha fixture body\n")?;
    std::fs::write(home.workspace.join(FILE_BETA), "beta fixture body\n")?;
    Ok(())
}

// Covers: a double click on an `@` palette row inserts that path, the same as
// Tab, even when it is not the highlighted row.
// Owner: interactive UX (PTY).
fn double_click_file_row(harness: &mut PtyHarness) -> Result<()> {
    // Fuzzy ranking decides which fixture leads; click the other one.
    let (target, other) = if harness.screen().contains_text(&format!("> @{FILE_ALPHA}")) {
        (FILE_BETA, FILE_ALPHA)
    } else {
        (FILE_ALPHA, FILE_BETA)
    };
    let cell = click_cell(harness, target)?;
    double_click(harness, cell)?;
    harness.wait_for_text_gone(other, CLICK)?;
    assert_composer_reads(harness, &format!("> @{target}"))
}

const FILE_PALETTE_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_file_palette"),
    Step::TypeText("@click"),
    // The composer row; palette rows read `> @alpha-…` or `  @beta-…`. Once the
    // whole query is painted, so is the list it filters.
    Step::WaitText {
        text: "> @click",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: FILE_ALPHA,
        timeout: SETTLE,
    },
    Step::WaitText {
        text: FILE_BETA,
        timeout: SETTLE,
    },
    Step::Phase("double_click_row"),
    Step::Custom(double_click_file_row),
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const FILE_PALETTE_CLICK_SCENARIO: Scenario = Scenario::new(
    "file_palette_click",
    "Insert an @ path palette row with a double click",
    SIZE,
    FILE_PALETTE_CLICK_STEPS,
    /* smoke */ false,
)
.with_setup(setup_click_files);
