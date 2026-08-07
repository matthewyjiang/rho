use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    picker_overlay::{picker_overlay_layout, OverlayPane, OverlayScrollTargets},
    App, ComposerMode, InteractiveRuntime, OverlayFocus, UiPicker,
};

/// Detail lines one wheel event scrolls, matching history wheel speed.
const DETAIL_WHEEL_LINES: isize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerKeyEffect {
    None,
    Handled,
    Submit,
    Escape,
    ToggleFavorite,
    DeleteRow,
}

fn overlay_scroll_targets(
    picker: &UiPicker,
    terminal: &DefaultTerminal,
) -> Option<OverlayScrollTargets> {
    if !picker.is_overlay() {
        return None;
    }
    let size = terminal.size().ok()?;
    Some(
        picker_overlay_layout(
            Rect::new(0, 0, size.width, size.height),
            picker.overlay_sizing(),
        )
        .scroll_targets(),
    )
}

/// Detail viewport when keyboard scrolling targets the detail pane.
fn focused_detail_viewport(
    picker: &UiPicker,
    targets: Option<OverlayScrollTargets>,
) -> Option<super::picker_overlay::DetailViewport> {
    if !picker.detail_pane_focused() {
        return None;
    }
    targets.and_then(|targets| targets.detail)
}

fn apply_page_key(picker: &mut UiPicker, targets: Option<OverlayScrollTargets>, direction: isize) {
    if let Some(viewport) = focused_detail_viewport(picker, targets) {
        picker.scroll_detail_page(direction, viewport);
        return;
    }
    let rows = targets.map_or(1, |targets| targets.nav_rows);
    picker.select_by_offset(direction.saturating_mul(rows as isize));
}

fn apply_home_end_key(picker: &mut UiPicker, targets: Option<OverlayScrollTargets>, home: bool) {
    if let Some(viewport) = focused_detail_viewport(picker, targets) {
        if home {
            picker.scroll_detail_home();
        } else {
            picker.scroll_detail_end(viewport);
        }
        return;
    }
    if home {
        picker.select_first_match();
    } else {
        picker.select_last_match();
    }
}

fn apply_picker_key(
    picker: &mut UiPicker,
    key: KeyEvent,
    targets: Option<OverlayScrollTargets>,
    space_confirms: bool,
) -> PickerKeyEffect {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Up) => {
            if let Some(viewport) = focused_detail_viewport(picker, targets) {
                picker.scroll_detail_by(-1, viewport);
            } else {
                picker.select_previous();
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            if let Some(viewport) = focused_detail_viewport(picker, targets) {
                picker.scroll_detail_by(1, viewport);
            } else {
                picker.select_next();
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Left) if picker.has_scrollable_detail() => {
            picker.focus_overlay_pane(OverlayFocus::Nav);
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Right) if picker.has_scrollable_detail() => {
            picker.focus_overlay_pane(OverlayFocus::Detail);
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::PageUp) => {
            if targets.is_some() {
                apply_page_key(picker, targets, -1);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::PageDown) => {
            if targets.is_some() {
                apply_page_key(picker, targets, 1);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            if targets.is_some() {
                apply_home_end_key(picker, targets, /*home*/ true);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            if targets.is_some() {
                apply_home_end_key(picker, targets, /*home*/ false);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Tab) if picker.key_hints.tab_complete => {
            picker.complete_filter();
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            picker.pop_filter_char();
            PickerKeyEffect::Handled
        }
        (KeyModifiers::CONTROL, KeyCode::Char('p')) if picker.key_hints.pin_toggle => {
            PickerKeyEffect::ToggleFavorite
        }
        (KeyModifiers::NONE, KeyCode::Char('d') | KeyCode::Delete)
            if picker.key_hints.row_delete =>
        {
            PickerKeyEffect::DeleteRow
        }
        (KeyModifiers::NONE, KeyCode::Char(' ')) if space_confirms => PickerKeyEffect::Submit,
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
            picker.push_filter_char(ch);
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Enter) => PickerKeyEffect::Submit,
        (_, KeyCode::Esc) => PickerKeyEffect::Escape,
        _ => PickerKeyEffect::None,
    }
}

impl App {
    /// Wheel scroll routed to an open picker. Returns true when a picker
    /// consumed the event; open pickers swallow the wheel even outside their
    /// box so the history behind a popup never scrolls by accident.
    pub(super) fn scroll_picker_on_wheel(
        &mut self,
        delta: isize,
        column: u16,
        row: u16,
        width: u16,
        height: u16,
    ) -> bool {
        let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
            return false;
        };
        if !picker.is_overlay() {
            picker.select_by_offset(delta);
            return true;
        }
        let layout = picker_overlay_layout(Rect::new(0, 0, width, height), picker.overlay_sizing());
        let fallback = if picker.detail_pane_focused() {
            OverlayPane::Detail
        } else {
            OverlayPane::Nav
        };
        match layout.pane_at(column, row).unwrap_or(fallback) {
            OverlayPane::Nav => picker.select_by_offset(delta),
            OverlayPane::Detail => {
                if let Some(viewport) = layout.detail_viewport() {
                    picker.scroll_detail_by(delta.saturating_mul(DETAIL_WHEEL_LINES), viewport);
                }
            }
        }
        true
    }

    pub(super) async fn handle_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), super::ComposerMode::Picker(_)) {
            return Ok(false);
        }

        let space_confirms = self.picker_space_confirms_selection();
        let delete_action = {
            let super::ComposerMode::Picker(picker) = self.input_ui.composer() else {
                return Ok(false);
            };
            picker.action
        };
        let effect = {
            let super::ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
                return Ok(false);
            };
            let targets = overlay_scroll_targets(picker, terminal);
            apply_picker_key(picker, key, targets, space_confirms)
        };

        match effect {
            PickerKeyEffect::None => Ok(true),
            PickerKeyEffect::Handled => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            PickerKeyEffect::Submit => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.submit_picker_selection(terminal, agent).await?;
                Ok(true)
            }
            PickerKeyEffect::Escape => {
                self.handle_picker_escape(/*running*/ false)?;
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            PickerKeyEffect::ToggleFavorite => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            PickerKeyEffect::DeleteRow => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                match delete_action {
                    super::PickerAction::ResumeSession => {
                        self.prompt_delete_selected_session()?;
                    }
                    super::PickerAction::Workflow => {
                        self.prompt_delete_selected_workflow_item()?;
                    }
                    _ => {}
                }
                Ok(true)
            }
        }
    }

    pub(super) fn handle_running_picker_key(
        &mut self,
        key: KeyEvent,
        terminal: &DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), super::ComposerMode::Picker(_)) {
            return Ok(false);
        }

        let space_confirms = self.picker_space_confirms_selection();
        let effect = {
            let super::ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
                return Ok(false);
            };
            let targets = overlay_scroll_targets(picker, terminal);
            apply_picker_key(picker, key, targets, space_confirms)
        };

        match effect {
            PickerKeyEffect::None => Ok(true),
            PickerKeyEffect::Handled => Ok(true),
            PickerKeyEffect::Submit => {
                self.submit_picker_selection_during_turn()?;
                Ok(true)
            }
            PickerKeyEffect::Escape => {
                self.handle_picker_escape(/*running*/ true)?;
                Ok(true)
            }
            PickerKeyEffect::ToggleFavorite => {
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            PickerKeyEffect::DeleteRow => {
                // Session switch/delete is idle-only; ignore while a turn runs.
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
#[path = "picker_input_tests.rs"]
mod tests;
