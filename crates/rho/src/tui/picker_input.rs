use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    picker_overlay_layout::{
        clamp_overlay_scroll, picker_overlay_layout, OverlayLayout, OverlayPane,
        OverlayScrollTargets, OverlayScrollbarState,
    },
    scrollbar::HistoryScrollbar,
    App, ComposerMode, InteractiveRuntime, OverlayFocus, OverlayScrollbarDrag, UiPicker,
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
    /// Left button drag.
    Drag,
    /// Left button release.
    Release,
    /// Pointer movement.
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PickerKeyEffect {
    None,
    Handled,
    Submit,
    Escape,
    ToggleFavorite,
    ToggleModelScope,
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

pub(in crate::tui) fn overlay_scroll_targets(
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

pub(in crate::tui) fn apply_picker_key(
    picker: &mut UiPicker,
    key: KeyEvent,
    targets: Option<OverlayScrollTargets>,
    space_confirms: bool,
    keybindings: &crate::keybindings::Keybindings,
) -> PickerKeyEffect {
    // Model-list keys reuse the composer bindings so a rebind moves both, and
    // are checked before the tuple match so a rebind onto a plain character
    // cannot be swallowed by the filter arm.
    if picker.key_hints.pin_toggle && keybindings.cycle_pinned_model.matches(key) {
        return PickerKeyEffect::ToggleFavorite;
    }
    if picker.key_hints.scope_toggle && keybindings.toggle_tool_output.matches(key) {
        return PickerKeyEffect::ToggleModelScope;
    }
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

fn nav_scrollbar(picker: &UiPicker, layout: OverlayLayout) -> Option<HistoryScrollbar> {
    let viewport_rows = layout.nav_viewport_rows();
    let matching = picker.matching_indices();
    let total_rows = super::picker_rows::picker_row_count(&picker.items, &matching);
    let nav_rect = layout.nav_body_rect();
    OverlayScrollbarState::nav(
        nav_rect.width as usize,
        total_rows,
        viewport_rows,
        picker.nav_window_start(viewport_rows),
    )
    .map(|scrollbar| scrollbar.hitbox(nav_rect))
}
fn detail_scrollbar(picker: &UiPicker, layout: OverlayLayout) -> Option<HistoryScrollbar> {
    let viewport = layout.detail_viewport()?;
    let detail_rect = layout.detail_body_rect()?;
    let line_count = {
        let lines = picker.wrapped_detail_lines(viewport.width);
        super::picker_overlay::detail_content_line_count(
            lines.len(),
            picker.selected_detail_badge().is_some(),
        )
    };
    let scroll = clamp_overlay_scroll(picker.detail_scroll, line_count, viewport.rows);
    OverlayScrollbarState::detail(line_count, viewport.rows, scroll)
        .map(|scrollbar| scrollbar.hitbox(detail_rect))
}

fn apply_nav_scrollbar_top(picker: &mut UiPicker, layout: OverlayLayout, top_line: usize) {
    picker.scroll_nav_to(top_line, layout.nav_viewport_rows());
}

fn apply_detail_scrollbar_top(picker: &mut UiPicker, layout: OverlayLayout, top_line: usize) {
    let Some(viewport) = layout.detail_viewport() else {
        return;
    };
    let line_count = {
        let lines = picker.wrapped_detail_lines(viewport.width);
        super::picker_overlay::detail_content_line_count(
            lines.len(),
            picker.selected_detail_badge().is_some(),
        )
    };
    picker.detail_scroll = clamp_overlay_scroll(top_line, line_count, viewport.rows);
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
                    PickerMouseEvent::Click
                    | PickerMouseEvent::Drag
                    | PickerMouseEvent::Release
                    | PickerMouseEvent::Move => return false,
                }
            } else {
                let layout =
                    picker_overlay_layout(Rect::new(0, 0, width, height), picker.overlay_sizing());
                let may_change =
                    matches!(event, PickerMouseEvent::Click | PickerMouseEvent::Wheel(_));
                match event {
                    PickerMouseEvent::Wheel(delta) => {
                        picker.set_overlay_scrollbar_drag(None);
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
                    PickerMouseEvent::Click => {
                        if let Some(scrollbar) = nav_scrollbar(picker, layout)
                            .filter(|scrollbar| scrollbar.contains(column, row))
                        {
                            let drag = scrollbar.begin_drag(row);
                            let top = scrollbar.top_line_for_pointer(row, drag);
                            apply_nav_scrollbar_top(picker, layout, top);
                            picker
                                .set_overlay_scrollbar_drag(Some(OverlayScrollbarDrag::Nav(drag)));
                            picker.focus_overlay_pane(OverlayFocus::Nav);
                        } else if let Some(scrollbar) = detail_scrollbar(picker, layout)
                            .filter(|scrollbar| scrollbar.contains(column, row))
                        {
                            let drag = scrollbar.begin_drag(row);
                            let top = scrollbar.top_line_for_pointer(row, drag);
                            apply_detail_scrollbar_top(picker, layout, top);
                            picker.set_overlay_scrollbar_drag(Some(OverlayScrollbarDrag::Detail(
                                drag,
                            )));
                            picker.focus_overlay_pane(OverlayFocus::Detail);
                        } else {
                            picker.set_overlay_scrollbar_drag(None);
                            match layout.pane_hit(column, row) {
                                Some(hit) if hit.pane == OverlayPane::Nav => {
                                    let viewport_rows = layout.scroll_targets().nav_rows;
                                    let row_index =
                                        picker.nav_window_start(viewport_rows) + hit.pane_row;
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
                            }
                        }
                    }
                    PickerMouseEvent::Drag => match picker.overlay_scrollbar_drag() {
                        Some(OverlayScrollbarDrag::Nav(drag)) => {
                            if let Some(scrollbar) = nav_scrollbar(picker, layout) {
                                apply_nav_scrollbar_top(
                                    picker,
                                    layout,
                                    scrollbar.top_line_for_pointer(row, drag),
                                );
                            }
                        }
                        Some(OverlayScrollbarDrag::Detail(drag)) => {
                            if let Some(scrollbar) = detail_scrollbar(picker, layout) {
                                apply_detail_scrollbar_top(
                                    picker,
                                    layout,
                                    scrollbar.top_line_for_pointer(row, drag),
                                );
                            }
                        }
                        None => {}
                    },
                    PickerMouseEvent::Release => {
                        picker.set_overlay_scrollbar_drag(None);
                    }
                    PickerMouseEvent::Move => {
                        if picker.overlay_scrollbar_drag().is_none() {
                            let hovered = overlay_nav_row_at(picker, layout, column, row);
                            picker.set_hovered_nav_row(hovered);
                        }
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
        if self.toggle_attach_filter_if_requested(key) {
            return Ok(true);
        }

        let space_confirms = self.picker_space_confirms_selection();
        let keybindings = self.info.runtime.keybindings.clone();
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
            apply_picker_key(picker, key, targets, space_confirms, &keybindings)
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
                if matches!(self.input_ui.composer(), super::ComposerMode::Input) {
                    self.reconcile_auto_classifier_gate(agent).await?;
                }
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
            PickerKeyEffect::ToggleModelScope => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.toggle_model_picker_scope()?;
                Ok(true)
            }
            PickerKeyEffect::DeleteRow => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                match delete_action {
                    super::PickerAction::ResumeSession => {
                        self.prompt_delete_selected_session()?;
                    }
                    super::PickerAction::ManageSessions => {
                        self.prompt_delete_selected_sessions_item()?;
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
        if self.toggle_attach_filter_if_requested(key) {
            return Ok(true);
        }

        let space_confirms = self.picker_space_confirms_selection();
        let keybindings = self.info.runtime.keybindings.clone();
        let effect = {
            let super::ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
                return Ok(false);
            };
            let targets = overlay_scroll_targets(picker, terminal);
            apply_picker_key(picker, key, targets, space_confirms, &keybindings)
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
                // Startup Auto classifier repair is idle-only. Cancel may mark a
                // pending demote; the next idle reconcile applies it with a runtime.
                self.handle_picker_escape(/*running*/ true)?;
                Ok(true)
            }
            PickerKeyEffect::ToggleFavorite => {
                self.toggle_selected_model_favorite()?;
                Ok(true)
            }
            PickerKeyEffect::ToggleModelScope => {
                self.toggle_model_picker_scope()?;
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
