//! Picker rendering and navigation on a caller-owned terminal and event stream.
//!
//! Callers that want pointer input hold mouse capture for the terminal. The
//! wheel scrolls the pane under the pointer, a click selects a nav row, a
//! double click submits it like Enter, and hover highlights the row.

use std::time::Instant;

use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::Rect,
    widgets::{Clear, Paragraph},
    DefaultTerminal,
};

use super::{
    apply_picker_key, input::apply_overlay_pointer, overlay_layout::picker_overlay_layout,
    overlay_scroll_targets, picker_overlay_frame, PickerKeyEffect, PickerMouseEvent, UiPicker,
};
use crate::{
    keybindings::Keybindings,
    tui::{click_sequence::ClickSequence, terminal_events::TerminalEvents, Theme},
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
    let mut clicks = ClickSequence::default();
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
            let effect = apply_mouse(&mut picker, area, *mouse, Instant::now(), &mut clicks);
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

/// What a pointer event asks the runner to do after updating the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseEffect {
    None,
    /// Return the selected row, like Enter.
    Submit,
}

/// Adapts a terminal pointer event to the shared overlay policy
/// ([`apply_overlay_pointer`]) and pairs nav-row clicks into a double click.
fn apply_mouse(
    picker: &mut UiPicker,
    area: Rect,
    mouse: MouseEvent,
    now: Instant,
    clicks: &mut ClickSequence,
) -> MouseEffect {
    if !picker.is_overlay() {
        return MouseEffect::None;
    }
    let event = match mouse.kind {
        MouseEventKind::ScrollUp => PickerMouseEvent::Wheel(-1),
        MouseEventKind::ScrollDown => PickerMouseEvent::Wheel(1),
        MouseEventKind::Down(MouseButton::Left) => PickerMouseEvent::Click(now),
        MouseEventKind::Drag(MouseButton::Left) => PickerMouseEvent::Drag,
        MouseEventKind::Up(MouseButton::Left) => PickerMouseEvent::Release,
        MouseEventKind::Moved => PickerMouseEvent::Move,
        MouseEventKind::Down(MouseButton::Right | MouseButton::Middle)
        | MouseEventKind::Up(MouseButton::Right | MouseButton::Middle)
        | MouseEventKind::Drag(MouseButton::Right | MouseButton::Middle)
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => return MouseEffect::None,
    };
    let layout = picker_overlay_layout(area, picker.overlay_sizing());
    let clicked = apply_overlay_pointer(picker, layout, event, mouse.column, mouse.row);
    match event {
        PickerMouseEvent::Click(_) => match clicked {
            // The item keeps a double click from pairing two rows that share
            // a cell after the nav window moved.
            Some(item) if clicks.register(now, mouse.column, mouse.row, item) => {
                MouseEffect::Submit
            }
            Some(_) => MouseEffect::None,
            None => {
                clicks.cancel();
                MouseEffect::None
            }
        },
        PickerMouseEvent::Wheel(_) => {
            clicks.cancel();
            MouseEffect::None
        }
        PickerMouseEvent::Drag | PickerMouseEvent::Release | PickerMouseEvent::Move => {
            MouseEffect::None
        }
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
