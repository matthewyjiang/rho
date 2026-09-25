//! Pointer targets in the bottom chrome: statusline fields and composer
//! attachments.
//!
//! A statusline field press runs that field's slash command through the event
//! loop's [`PointerAction`], exactly like typing it, or opens its picker
//! directly when no command opens it. A press on a composer attachment
//! removes it. Hover for both derives from the last pointer cell at
//! draw time, so scroll, resize, and removal re-anchor it without extra state.

use ratatui::layout::{Position, Rect};

use super::{
    app_state::PointerAction,
    composer_attachments::attachment_target_at,
    config_picker::{permission_mode_picker, PERMISSION_MODE_VALUE},
    screen_layout::{terminal_meets_minimum, ScreenLayout},
    statusline::{StatusClick, FIELDS_ROW},
    App, ComposerAttachment, ComposerMode,
};

/// Row-relative column on the statusline fields row under (`column`, `row`).
fn fields_row_column(statusline: Rect, column: u16, row: u16) -> Option<usize> {
    let fields_row = statusline.y.checked_add(u16::try_from(FIELDS_ROW).ok()?)?;
    (usize::from(statusline.height) > FIELDS_ROW
        && row == fields_row
        && statusline.contains(Position { x: column, y: row }))
    .then(|| usize::from(column - statusline.x))
}

impl App {
    /// Handle a primary press on a statusline field or a composer attachment.
    /// Returns true when the press was consumed.
    ///
    /// Both need the idle-or-running `Input` composer: a click never acts
    /// behind a modal the user is in the middle of.
    pub(super) fn handle_chrome_click(
        &mut self,
        layout: &ScreenLayout,
        screen: Rect,
        column: u16,
        row: u16,
    ) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::Input)
            || self.setup_step().is_some()
            || !terminal_meets_minimum(screen)
        {
            return false;
        }
        let width = usize::from(screen.width);
        if let Some(offset) = fields_row_column(layout.statusline, column, row) {
            // Render first so the hit spans match this width.
            self.statusline_lines(width);
            let Some(action) = self.statusline.hit_at(offset).map(|hit| hit.action) else {
                return false;
            };
            self.clear_pointer_state_for_chrome_click();
            match action {
                StatusClick::Command(command) => self
                    .input_ui
                    .request_pointer_action(PointerAction::RunCommand(command.into())),
                StatusClick::PermissionMode => self.open_permission_mode_picker(),
            }
            return true;
        }
        let attachments = self.composer_attachment_layout(width);
        let Some(index) = attachment_target_at(
            &attachments,
            layout.composer,
            layout.composer_start,
            column,
            row,
        )
        .map(|target| target.attachment) else {
            return false;
        };
        self.clear_pointer_state_for_chrome_click();
        match self.input_ui.remove_attachment(index) {
            Some(ComposerAttachment::Pending { id, .. }) => {
                self.cancel_pending_attachment(id);
            }
            Some(ComposerAttachment::Ready(_)) | None => {}
        }
        self.set_status(format!("removed attachment {}", index + 1));
        true
    }

    /// Point statusline hover at the clickable field under the last pointer
    /// cell. Draw calls this before painting the statusline.
    pub(super) fn sync_statusline_hover(&mut self, statusline: Rect, width: usize) {
        let column = self
            .last_mouse_position
            .filter(|_| matches!(self.input_ui.composer(), ComposerMode::Input))
            .and_then(|(column, row)| fields_row_column(statusline, column, row));
        // Hit spans come from the render at this width.
        self.statusline_lines(width);
        self.statusline.set_hovered_column(column);
    }

    /// Open the permission mode chooser under its config row, so Esc walks
    /// back through config like picking the row by hand. Mid-turn it still
    /// opens; committing a change is what a turn blocks, and the config
    /// commit path reports that.
    fn open_permission_mode_picker(&mut self) {
        if let Err(error) = self.open_main_config_picker_selected(PERMISSION_MODE_VALUE) {
            self.set_status(format!("could not open config: {error}"));
            return;
        }
        let child = permission_mode_picker(self.info.runtime.permission_mode);
        self.open_child_picker(child);
    }

    fn clear_pointer_state_for_chrome_click(&mut self) {
        self.screen_selection = None;
        self.input_ui.clear_selection();
        self.input_ui.cancel_pointer_click_sequence();
        self.history.clear_text_selection();
        self.history.set_scrollbar_drag(None);
        self.clear_rail_pointer_state();
    }
}
