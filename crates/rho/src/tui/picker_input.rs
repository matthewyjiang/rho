use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    picker_overlay_layout::{
        picker_overlay_layout, OverlayLayout, OverlayPane, OverlayScrollTargets,
    },
    App, ComposerMode, InteractiveRuntime, OverlayFocus, UiPicker,
};

/// Lines one wheel event scrolls in either overlay pane. Tied to the history
/// wheel speed so the two cannot drift.
const PICKER_WHEEL_LINES: isize = super::HISTORY_MOUSE_SCROLL_LINES as isize;

/// Pointer events an open picker can consume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerMouseEvent {
    /// Wheel step, negative up and positive down.
    Wheel(isize),
    /// Left button press.
    Click,
    /// Pointer movement.
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerKeyEffect {
    None,
    Handled,
    Submit,
    Escape,
    ToggleFavorite,
    DeleteRow,
}

/// Row-space nav row under the pointer, or `None` off the nav items.
fn overlay_nav_row_at(
    picker: &UiPicker,
    layout: OverlayLayout,
    column: u16,
    row: u16,
) -> Option<usize> {
    let hit = layout
        .pane_hit(column, row)
        .filter(|hit| hit.pane == OverlayPane::Nav)?;
    let viewport_rows = layout.scroll_targets().nav_rows;
    let row_index = picker
        .nav_window_start(viewport_rows)
        .checked_add(hit.pane_row)?;
    picker.nav_item_at_row(row_index).map(|_| row_index)
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
    targets: OverlayScrollTargets,
) -> Option<super::picker_overlay_layout::DetailViewport> {
    if !picker.detail_pane_focused() {
        return None;
    }
    targets.detail
}

fn apply_page_key(picker: &mut UiPicker, targets: OverlayScrollTargets, direction: isize) {
    if let Some(viewport) = focused_detail_viewport(picker, targets) {
        picker.scroll_detail_page(direction, viewport);
        return;
    }
    picker.select_by_offset(direction.saturating_mul(targets.nav_rows as isize));
}

fn apply_home_end_key(picker: &mut UiPicker, targets: OverlayScrollTargets, home: bool) {
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
            if let Some(viewport) =
                targets.and_then(|targets| focused_detail_viewport(picker, targets))
            {
                picker.scroll_detail_by(-1, viewport);
            } else {
                picker.select_previous();
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            if let Some(viewport) =
                targets.and_then(|targets| focused_detail_viewport(picker, targets))
            {
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
            if let Some(targets) = targets {
                apply_page_key(picker, targets, -1);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::PageDown) => {
            if let Some(targets) = targets {
                apply_page_key(picker, targets, 1);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            if let Some(targets) = targets {
                apply_home_end_key(picker, targets, /*home*/ true);
            }
            PickerKeyEffect::Handled
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            if let Some(targets) = targets {
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
    /// Pointer event routed to an open picker. Returns true when the picker
    /// consumed it.
    ///
    /// An open overlay swallows every pointer event it is offered, even
    /// outside its box, so the history behind a popup never scrolls or
    /// selects by accident. An inline list picker has no box: only the wheel
    /// reaches it, stepping the selection because it has no viewport of its
    /// own; clicks and movement fall through to the history.
    ///
    /// Over an overlay the wheel moves viewports, never the selection. The
    /// pane under the pointer takes the scroll, falling back to the focused
    /// pane when the pointer sits outside the box.
    pub(super) fn route_picker_mouse(
        &mut self,
        event: PickerMouseEvent,
        column: u16,
        row: u16,
        width: u16,
        height: u16,
    ) -> bool {
        let selection_may_change = {
            let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
                return false;
            };
            if !picker.is_overlay() {
                match event {
                    PickerMouseEvent::Wheel(delta) => {
                        picker.select_by_offset(delta);
                        true
                    }
                    PickerMouseEvent::Click | PickerMouseEvent::Move => return false,
                }
            } else {
                let layout =
                    picker_overlay_layout(Rect::new(0, 0, width, height), picker.overlay_sizing());
                let may_change =
                    matches!(event, PickerMouseEvent::Click | PickerMouseEvent::Wheel(_));
                match event {
                    PickerMouseEvent::Wheel(delta) => {
                        let fallback = if picker.detail_pane_focused() {
                            OverlayPane::Detail
                        } else {
                            OverlayPane::Nav
                        };
                        let pane = layout
                            .pane_hit(column, row)
                            .map_or(fallback, |hit| hit.pane);
                        match pane {
                            OverlayPane::Nav => {
                                let rows = layout.scroll_targets().nav_rows;
                                picker
                                    .scroll_nav_by(delta.saturating_mul(PICKER_WHEEL_LINES), rows);
                            }
                            OverlayPane::Detail => {
                                if let Some(viewport) = layout.detail_viewport() {
                                    picker.scroll_detail_by(
                                        delta.saturating_mul(PICKER_WHEEL_LINES),
                                        viewport,
                                    );
                                }
                            }
                        }
                    }
                    PickerMouseEvent::Click => match layout.pane_hit(column, row) {
                        Some(hit) if hit.pane == OverlayPane::Nav => {
                            let viewport_rows = layout.scroll_targets().nav_rows;
                            let row_index = picker.nav_window_start(viewport_rows) + hit.pane_row;
                            if picker.select_nav_row(row_index, viewport_rows) {
                                picker.focus_overlay_pane(OverlayFocus::Nav);
                            }
                        }
                        Some(hit)
                            if hit.pane == OverlayPane::Detail
                                && picker.has_scrollable_detail() =>
                        {
                            picker.focus_overlay_pane(OverlayFocus::Detail);
                        }
                        _ => {}
                    },
                    PickerMouseEvent::Move => {
                        let hovered = overlay_nav_row_at(picker, layout, column, row);
                        picker.set_hovered_nav_row(hovered);
                    }
                }
                may_change
            }
        };
        if selection_may_change {
            self.preview_selected_theme_if_active();
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
                self.preview_selected_theme_if_active();
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
            PickerKeyEffect::Handled => {
                self.preview_selected_theme_if_active();
                Ok(true)
            }
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
