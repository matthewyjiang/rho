//! Pointer input on single-pane overlays: scrollbar hover and drag, and
//! drag-to-select with copy, for a panel (`/hooks`) and the side chat.

use std::{
    fmt::Write as _,
    fs,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use unicode_width::UnicodeWidthChar;

use super::{SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::{Key, MouseButton},
    pty::PtySize,
    scenario::{Scenario, Step},
};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

/// Enough hooks that the `/hooks` body overflows its viewport.
const HOOK_COUNT: usize = 8;
const FIRST_HOOK: &str = "user:hook-01";
const LAST_HOOK: &str = "user:hook-08";
/// Complete OSC 52 write of exactly `user:hook-08`.
const LAST_HOOK_COPY: &[u8] = b"\x1b]52;c;dXNlcjpob29rLTA4\x1b\\";

const SIDE_PROMPT_ECHO: &str = "fixture response: copy me please";
/// Complete OSC 52 write of exactly `fixture response`.
const SIDE_RESPONSE_COPY: &[u8] = b"\x1b]52;c;Zml4dHVyZSByZXNwb25zZQ==\x1b\\";
const SIDE_FIRST_BULK_LINE: &str = "fixture bulk one line 001";

// Covers: a panel scrollbar lights on hover and scrolls under a thumb drag,
// and a drag over panel text copies exactly the selected span.
// Owner: interactive TUI
const PANEL_POINTER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_hooks"),
    Step::SubmitText("/hooks"),
    Step::WaitText {
        text: FIRST_HOOK,
        timeout: SETTLE,
    },
    Step::Phase("hover_scrollbar"),
    Step::Custom(hover_lights_thumb),
    Step::Phase("drag_scrollbar"),
    Step::Custom(drag_panel_thumb_to_bottom),
    Step::Phase("drag_copy"),
    Step::Custom(drag_copies_last_hook_id),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: LAST_HOOK,
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const PANEL_POINTER_SCENARIO: Scenario = Scenario::new(
    "panel_pointer",
    "Hover and drag a panel scrollbar, then drag-select panel text to copy it",
    SIZE,
    PANEL_POINTER_STEPS,
    /* smoke */ false,
)
.with_setup(setup_many_hooks)
// Exercise OSC52 on every host instead of the runner's native clipboard.
.with_env(&[("SSH_TTY", "rho-pty-clipboard")]);

// Covers: a drag over side-chat transcript text copies the span, and the
// side-chat scrollbar drags a followed-to-end transcript back to its start.
// Owner: interactive TUI
const SIDE_POINTER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_btw"),
    Step::SubmitText("/btw copy me please"),
    Step::WaitText {
        text: SIDE_PROMPT_ECHO,
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Enter send   Esc close",
        timeout: SETTLE,
    },
    Step::Phase("drag_copy"),
    Step::Custom(drag_copies_side_response),
    Step::Phase("fill_transcript"),
    Step::SubmitText("fixture bulk one"),
    Step::WaitText {
        text: "fixture bulk one line 180",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Enter send   Esc close",
        timeout: SETTLE,
    },
    Step::Phase("drag_scrollbar"),
    Step::Custom(drag_side_thumb_to_top),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const SIDE_POINTER_SCENARIO: Scenario = Scenario::new(
    "side_chat_pointer",
    "Drag-select side chat text to copy it, then drag its scrollbar",
    SIZE,
    SIDE_POINTER_STEPS,
    /* smoke */ false,
)
.with_env(&[("SSH_TTY", "rho-pty-clipboard")]);

/// Scrollbar column and the body rows it spans, in 0-based screen cells.
struct Track {
    column: u16,
    top: u16,
    bottom: u16,
}

/// Waits for the overlay chrome to finish painting, then locates its
/// scrollbar. Body text can land a frame before the footer rule, so reading
/// the chrome once right after a text wait races the redraw.
fn overlay_track(harness: &mut PtyHarness) -> Result<Track> {
    wait_until(harness, "overlay chrome not found", |harness| {
        find_overlay_track(harness).is_some()
    })?;
    find_overlay_track(harness).context("overlay chrome vanished after it painted")
}

/// Locates the overlay scrollbar from the panel chrome: the footer rule
/// (`├──┤`) sits under the body with the panel's `┌` corner above it, the
/// track is the column left of `┤`, and the body starts under the `┌`. The
/// copy notice can cover the top-right corner, so the right edge is read from
/// the footer rule.
fn find_overlay_track(harness: &PtyHarness) -> Option<Track> {
    let screen = harness.screen();
    let symbol = |row: u16, col: u16| screen.cell(row, col).map(|cell| cell.contents);
    (0..screen.rows()).rev().find_map(|rule_row| {
        let left = (0..screen.cols()).find(|&col| symbol(rule_row, col).as_deref() == Some("├"))?;
        let right =
            (left..screen.cols()).find(|&col| symbol(rule_row, col).as_deref() == Some("┤"))?;
        let top_border = (0..rule_row)
            .rev()
            .find(|&row| symbol(row, left).as_deref() == Some("┌"))?;
        (right > left + 2 && rule_row > top_border + 1).then_some(Track {
            column: right - 1,
            top: top_border + 1,
            bottom: rule_row - 1,
        })
    })
}

/// Polls until `done` holds, failing with `what` and a screen dump.
fn wait_until(
    harness: &mut PtyHarness,
    what: &str,
    done: impl Fn(&PtyHarness) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + SETTLE.duration;
    loop {
        harness.poll(Duration::from_millis(20));
        if done(harness) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("{what}:\n{}", harness.screen().debug_dump());
        }
    }
}

fn thumb_lit(harness: &PtyHarness, track: &Track) -> bool {
    harness
        .screen()
        .cell(track.top, track.column)
        .is_some_and(|cell| cell.contents == "█" && cell.bold)
}

fn hover_lights_thumb(harness: &mut PtyHarness) -> Result<()> {
    let track = overlay_track(harness)?;
    if thumb_lit(harness, &track) {
        bail!("thumb lit before hover:\n{}", harness.screen().debug_dump());
    }
    // SGR mouse coordinates are 1-based.
    harness.mouse_move(track.column + 1, track.top + 1)?;
    wait_until(
        harness,
        "hovered scrollbar thumb did not light",
        |harness| thumb_lit(harness, &track),
    )?;
    harness.mouse_move(track.column - 3, track.top + 1)?;
    wait_until(
        harness,
        "thumb stayed lit after the pointer left",
        |harness| !thumb_lit(harness, &track),
    )
}

fn drag_panel_thumb_to_bottom(harness: &mut PtyHarness) -> Result<()> {
    if harness.screen().contains_text(LAST_HOOK) {
        bail!(
            "last hook visible before scrolling; the body must overflow:\n{}",
            harness.screen().debug_dump()
        );
    }
    let track = overlay_track(harness)?;
    // The panel opens at the top, so the thumb starts on the first track row.
    harness.mouse(MouseButton::Left, track.column + 1, track.top + 1, true)?;
    harness.mouse_drag(track.column + 1, track.bottom + 1)?;
    harness.mouse(MouseButton::Left, track.column + 1, track.bottom + 1, false)?;
    harness.wait_for_text(LAST_HOOK, SETTLE)?;
    if harness.screen().contains_text(FIRST_HOOK) {
        bail!(
            "thumb drag to the bottom left the first hook on screen:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

/// Presses on the first cell of `needle`, drags to its last cell, releases,
/// and waits for the OSC 52 write of exactly that text.
fn drag_copy(harness: &mut PtyHarness, needle: &str, osc52: &[u8]) -> Result<()> {
    let (row, column) = screen_cell(harness, needle)?;
    let last = column + needle.chars().count() as u16 - 1;
    let before = harness.raw_sequence_occurrences(osc52);
    harness.mouse(MouseButton::Left, column + 1, row + 1, true)?;
    harness.mouse_drag(last + 1, row + 1)?;
    harness.mouse(MouseButton::Left, last + 1, row + 1, false)?;
    harness.wait_for_raw_sequence_occurrences(osc52, before + 1, SETTLE)
}

fn drag_copies_last_hook_id(harness: &mut PtyHarness) -> Result<()> {
    drag_copy(harness, LAST_HOOK, LAST_HOOK_COPY)?;
    if !harness.screen().contains_text(LAST_HOOK) {
        bail!(
            "selecting text closed the hooks overlay:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

fn drag_copies_side_response(harness: &mut PtyHarness) -> Result<()> {
    drag_copy(harness, "fixture response", SIDE_RESPONSE_COPY)
}

fn drag_side_thumb_to_top(harness: &mut PtyHarness) -> Result<()> {
    if harness.screen().contains_text(SIDE_FIRST_BULK_LINE) {
        bail!(
            "first bulk line visible before scrolling; the aside must follow its end:\n{}",
            harness.screen().debug_dump()
        );
    }
    let track = overlay_track(harness)?;
    // A followed-to-end transcript keeps the thumb on the last track row.
    harness.mouse(MouseButton::Left, track.column + 1, track.bottom + 1, true)?;
    harness.mouse_drag(track.column + 1, track.top + 1)?;
    harness.mouse(MouseButton::Left, track.column + 1, track.top + 1, false)?;
    harness.wait_for_text(SIDE_FIRST_BULK_LINE, SETTLE)
}

/// Row and 0-based display column where `needle` first renders.
fn screen_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate() {
        if let Some(byte_offset) = line.find(needle) {
            let column = line[..byte_offset]
                .chars()
                .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
                .sum::<usize>();
            return Ok((row as u16, column as u16));
        }
    }
    bail!("'{needle}' not found:\n{}", harness.screen().debug_dump())
}

fn setup_many_hooks(home: &IsolatedHome) -> Result<()> {
    let rho_dir = home.home.join(".rho");
    let program = rho_dir.join("noop-hook.sh");
    fs::write(&program, "#!/bin/sh\nexit 0\n").context("failed to write the hook program")?;
    let mut config = String::from("version = 1\n");
    for index in 1..=HOOK_COUNT {
        write!(
            config,
            r#"
[[hook]]
id = "hook-{index:02}"
on = "before_tool_use"
tools = ["bash"]
command = ["/bin/sh", "{}"]
timeout = "2s"
"#,
            program.display()
        )
        .expect("writing to a String cannot fail");
    }
    fs::write(rho_dir.join("hooks.toml"), config)
        .context("failed to write the isolated hooks.toml")?;
    Ok(())
}
