//! Pointer input for the `/` command palette and the path palette (`@`
//! mentions, or shell-mode Tab completion).
//!
//! The palette renderer reports which of its lines hold which match through
//! [`PaletteFrame::hits`](super::palette::PaletteFrame). A click highlights the
//! row under it; a double click completes it the way Tab does. It never submits
//! the way Enter can, so a stray double click cannot run a command like
//! `/clear`. The wheel over the rows steps the highlight instead of scrolling
//! the transcript.

use std::time::Instant;

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{
    composer_pointer::{composer_target_at, ChoiceClick, ComposerHit},
    mouse::COMPOSER_DOUBLE_CLICK,
    palette::{ActivePalette, PaletteRow},
    App,
};

impl App {
    /// Handles a pointer event over the palette painted into `area` (the
    /// frame's `layout.commands`) with row spans `hits`. `false` when no
    /// palette row owns the event, so the screen handler keeps its behavior.
    pub(super) fn handle_palette_mouse(
        &mut self,
        kind: MouseEventKind,
        hits: &[ComposerHit<PaletteRow>],
        area: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) -> bool {
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(target) = composer_target_at(hits, area, /*start*/ 0, column, row) else {
                    return false;
                };
                self.click_palette_row(target, column, row, now)
            }
            MouseEventKind::ScrollUp => self.wheel_palette(area, column, row, /*delta*/ -1),
            MouseEventKind::ScrollDown => self.wheel_palette(area, column, row, /*delta*/ 1),
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => false,
        }
    }

    /// Highlights, or on a double click completes, the palette row `target`.
    /// A hit that no longer names a live row of the same palette is ignored
    /// and falls through.
    fn click_palette_row(
        &mut self,
        target: PaletteRow,
        column: u16,
        row: u16,
        now: Instant,
    ) -> bool {
        let palette = self.active_palette();
        let live = match (target, &palette) {
            (PaletteRow::Command(index), Some(ActivePalette::Command(matches))) => {
                index < matches.len()
            }
            (PaletteRow::File(index), Some(ActivePalette::File(matches))) => index < matches.len(),
            (PaletteRow::Command(_) | PaletteRow::File(_), Some(_) | None) => false,
        };
        if !live {
            return false;
        }

        self.input_ui.clear_selection();
        self.history.clear_text_selection();
        self.history.set_scrollbar_drag(None);
        self.screen_selection = None;
        self.clear_rail_pointer_state();
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        let index = match target {
            PaletteRow::Command(index) | PaletteRow::File(index) => index,
        };
        // The index keeps a double click from pairing two rows that share a
        // cell after the scrolled window shifts under the first click.
        let click =
            if self
                .input_ui
                .register_pointer_click(now, column, row, index, COMPOSER_DOUBLE_CLICK)
            {
                ChoiceClick::Double
            } else {
                ChoiceClick::Single
            };

        match (palette, click) {
            (Some(ActivePalette::Command(_)), ChoiceClick::Single) => {
                // Same explicit pick an arrow key records, so Enter honors it.
                self.input_ui.move_command_selection(index);
            }
            (Some(ActivePalette::Command(matches)), ChoiceClick::Double) => {
                self.input_ui.move_command_selection(index);
                // Mirrors Tab in `handle_command_palette_key`.
                if let Some(choice) = matches.get(index) {
                    self.complete_command_choice(choice);
                    self.input_ui.set_command_palette_dismissed(false);
                    self.clamp_command_selection();
                }
            }
            (Some(ActivePalette::File(_)), ChoiceClick::Single) => {
                self.input_ui.set_file_selection(index);
            }
            (Some(ActivePalette::File(matches)), ChoiceClick::Double) => {
                self.input_ui.set_file_selection(index);
                if let Some(entry) = matches.get(index) {
                    // The mouse handler is backend-generic and cannot carry an
                    // anyhow error, so report it where the user is looking.
                    if let Err(error) = self.apply_file_palette_selection(&entry) {
                        self.set_status(format!("could not apply palette selection: {error:#}"));
                    }
                }
            }
            (None, ChoiceClick::Single | ChoiceClick::Double) => {}
        }
        true
    }

    /// Steps the palette highlight one row per wheel notch, clamped at both
    /// ends, when the pointer is over the palette. `false` elsewhere, so the
    /// transcript scrolls.
    fn wheel_palette(&mut self, area: Rect, column: u16, row: u16, delta: isize) -> bool {
        if !area.contains(Position { x: column, y: row }) {
            return false;
        }
        match self.active_palette() {
            Some(ActivePalette::Command(matches)) => {
                let last = matches.len().saturating_sub(1);
                let current = self.input_ui.command_selection().min(last);
                // An explicit pick, like an arrow key, so Enter honors it.
                self.input_ui
                    .move_command_selection(current.saturating_add_signed(delta).min(last));
                true
            }
            Some(ActivePalette::File(matches)) => {
                let last = matches.len().saturating_sub(1);
                let current = self.input_ui.file_selection().min(last);
                self.input_ui
                    .set_file_selection(current.saturating_add_signed(delta).min(last));
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
#[path = "palette_pointer_tests.rs"]
mod tests;
