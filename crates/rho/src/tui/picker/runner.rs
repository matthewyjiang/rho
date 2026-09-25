//! Picker rendering and navigation on a caller-owned terminal and event stream.
//!
//! Callers that want pointer input hold mouse capture for the terminal. The
//! wheel scrolls the pane under the pointer, a click selects a nav row, a
//! double click submits it like Enter, and hover highlights the row.

use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::Rect,
    widgets::{Clear, Paragraph},
    DefaultTerminal,
};

use super::{
    apply_picker_key,
    overlay_layout::{picker_overlay_layout, OverlayLayout, OverlayPane},
    overlay_scroll_targets, picker_overlay_frame, OverlayFocus, PickerKeyEffect, UiPicker,
};
use crate::{
    keybindings::Keybindings,
    tui::{
        click_sequence::DOUBLE_CLICK_GAP, terminal_events::TerminalEvents, Theme,
        HISTORY_MOUSE_SCROLL_LINES,
    },
};

pub(in crate::tui) enum EmptySubmit {
    Stay,
    Cancel,
}

pub(in crate::tui) enum EventHandling {
    Handled,
    Continue,
}

/// The feature handler runs before navigation and owns special keys and paste.
/// Reuse the caller's stream when continuing into another terminal UI.
pub(in crate::tui) async fn run(
    terminal: &mut DefaultTerminal,
    events: &mut TerminalEvents,
    mut picker: UiPicker,
    keybindings: &Keybindings,
    empty_submit: EmptySubmit,
    mut handle_event: impl FnMut(&mut UiPicker, &Event) -> EventHandling,
) -> anyhow::Result<Option<String>> {
    let mut last_click = None;
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Clear, area);
            if let Some(overlay) = picker_overlay_frame(&picker, area) {
                frame.render_widget(
                    Paragraph::new(overlay.lines).style(Theme::text()),
                    overlay.outer,
                );
                frame.set_cursor_position(overlay.cursor);
            }
        })?;
        let event = events.next().await?;
        if let Event::Key(key) = &event {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(None);
            }
        }
        if matches!(handle_event(&mut picker, &event), EventHandling::Handled) {
            continue;
        }
        if let Event::Mouse(mouse) = &event {
            let size = terminal.size()?;
            let area = Rect::new(0, 0, size.width, size.height);
            let effect = apply_mouse(
                &mut picker,
                area,
                PointerInput {
                    kind: mouse.kind,
                    column: mouse.column,
                    row: mouse.row,
                    now: Instant::now(),
                },
                &mut last_click,
            );
            match effect {
                MouseEffect::Submit => {
                    if let Some(item) = picker.selected_item() {
                        return Ok(Some(item.value.clone()));
                    }
                }
                MouseEffect::None => {}
            }
            continue;
        }
        if let Event::Key(key) = event {
            let targets = overlay_scroll_targets(&picker, terminal);
            match apply_picker_key(
                &mut picker,
                key,
                targets,
                /*space_confirms*/ false,
                keybindings,
            ) {
                PickerKeyEffect::Submit => {
                    if let Some(item) = picker.selected_item() {
                        return Ok(Some(item.value.clone()));
                    }
                    match empty_submit {
                        EmptySubmit::Stay => {}
                        EmptySubmit::Cancel => return Ok(None),
                    }
                }
                PickerKeyEffect::Escape => return Ok(None),
                PickerKeyEffect::None
                | PickerKeyEffect::Handled
                | PickerKeyEffect::ToggleFavorite
                | PickerKeyEffect::ToggleModelScope
                | PickerKeyEffect::DeleteRow => {}
            }
        }
    }
}

/// Last primary press on a nav row, for double-click detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NavClick {
    at: Instant,
    row: usize,
}

/// One pointer event in screen cells.
#[derive(Clone, Copy, Debug)]
struct PointerInput {
    kind: MouseEventKind,
    column: u16,
    row: u16,
    now: Instant,
}

/// What a pointer event asks the runner to do after updating the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseEffect {
    None,
    /// Return the selected row, like Enter.
    Submit,
}

/// Apply one pointer event to an overlay picker painted over `area`.
///
/// Uses the same overlay geometry the in-app picker routes through, so the
/// rows under the pointer are the rows that were painted.
fn apply_mouse(
    picker: &mut UiPicker,
    area: Rect,
    input: PointerInput,
    last_click: &mut Option<NavClick>,
) -> MouseEffect {
    if !picker.is_overlay() {
        return MouseEffect::None;
    }
    let layout = picker_overlay_layout(area, picker.overlay_sizing());
    let nav_rows = layout.scroll_targets().nav_rows;
    let PointerInput {
        kind,
        column,
        row,
        now,
    } = input;
    match kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let lines = HISTORY_MOUSE_SCROLL_LINES as isize;
            let delta = if kind == MouseEventKind::ScrollUp {
                -lines
            } else {
                lines
            };
            let fallback = if picker.detail_pane_focused() {
                OverlayPane::Detail
            } else {
                OverlayPane::Nav
            };
            match layout
                .pane_hit(column, row)
                .map_or(fallback, |hit| hit.pane)
            {
                OverlayPane::Nav => picker.scroll_nav_by(delta, nav_rows),
                OverlayPane::Detail => {
                    if let Some(viewport) = layout.detail_viewport() {
                        picker.scroll_detail_by(delta, viewport);
                    }
                }
            }
            // The rows moved under the pointer; re-aim hover, drop the sequence.
            picker.set_hovered_nav_row(nav_row_at(picker, layout, column, row));
            *last_click = None;
            MouseEffect::None
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(row_index) = nav_row_at(picker, layout, column, row) else {
                *last_click = None;
                return MouseEffect::None;
            };
            let double_click = last_click.is_some_and(|click| {
                click.row == row_index
                    && now.saturating_duration_since(click.at) <= DOUBLE_CLICK_GAP
            });
            picker.select_nav_row(row_index, nav_rows);
            picker.focus_overlay_pane(OverlayFocus::Nav);
            *last_click = (!double_click).then_some(NavClick {
                at: now,
                row: row_index,
            });
            if double_click {
                MouseEffect::Submit
            } else {
                MouseEffect::None
            }
        }
        MouseEventKind::Moved => {
            picker.set_hovered_nav_row(nav_row_at(picker, layout, column, row));
            MouseEffect::None
        }
        MouseEventKind::Down(MouseButton::Right | MouseButton::Middle)
        | MouseEventKind::Up(_)
        | MouseEventKind::Drag(_)
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => MouseEffect::None,
    }
}

/// Row-space nav row under the pointer, or `None` off the nav items.
fn nav_row_at(picker: &UiPicker, layout: OverlayLayout, column: u16, row: u16) -> Option<usize> {
    let hit = layout
        .pane_hit(column, row)
        .filter(|hit| hit.pane == OverlayPane::Nav)?;
    let row_index = picker
        .nav_window_start(layout.scroll_targets().nav_rows)
        .checked_add(hit.pane_row)?;
    picker.nav_item_at_row(row_index).map(|_| row_index)
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
