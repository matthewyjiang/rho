use std::time::Instant;

use ratatui::{layout::Rect, text::Line};

use super::{
    activity, render::display_width, scrollbar::HistoryScrollbar, visible_composer_start, App,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScreenLayout {
    pub(super) history: Rect,
    pub(super) history_scrollbar: Option<HistoryScrollbar>,
    pub(super) activity_background: Option<Rect>,
    pub(super) activity_rail: Option<Rect>,
    pub(super) activity: Option<Rect>,
    pub(super) jump_to_bottom: Option<Rect>,
    pub(super) pending_input: Rect,
    pub(super) subagents: Rect,
    pub(super) top_divider: Rect,
    pub(super) composer: Rect,
    pub(super) bottom_divider: Rect,
    pub(super) statusline: Rect,
    pub(super) commands: Rect,
    pub(super) composer_start: usize,
    pub(super) history_len: usize,
}

impl App {
    pub(super) fn screen_layout(&mut self, area: Rect, now: Instant) -> ScreenLayout {
        let width = area.width as usize;
        let history_len = self.history_len(width, now);
        let composer_lines = self.composer_lines(width);
        let command_lines = self.command_suggestion_lines(width);
        self.screen_layout_for_history_len(area, history_len, &composer_lines, command_lines.len())
    }

    pub(super) fn screen_layout_for_history_len(
        &self,
        area: Rect,
        history_len: usize,
        composer_lines: &[Line<'_>],
        command_line_count: usize,
    ) -> ScreenLayout {
        let width = area.width as usize;
        let height = area.height as usize;
        let full_cursor = self.composer_cursor_position(width);
        let cursor_line = (full_cursor.y as usize).min(composer_lines.len().saturating_sub(1));
        let statusline_height = self.statusline.height().min(height);
        let bottom_divider_height = usize::from(height > statusline_height);
        let command_height = command_line_count
            .min(height.saturating_sub(statusline_height + bottom_divider_height));
        let bottom_fixed_height = bottom_divider_height + statusline_height + command_height;
        let available_above_bottom = height.saturating_sub(bottom_fixed_height);
        let show_top_divider = available_above_bottom > 1 && !composer_lines.is_empty();
        let history_height_without_jump =
            self.history_height_from_line_counts(height, composer_lines.len(), command_line_count);
        let show_jump_to_bottom = history_height_without_jump > 0
            && self.visible_history_start(history_len, history_height_without_jump)
                < history_len.saturating_sub(history_height_without_jump);
        let reserved_above_composer = usize::from(show_top_divider);
        let interactive_budget = available_above_bottom.saturating_sub(reserved_above_composer);
        let desired_pending_input_height = self.pending_input_height();
        let desired_subagent_height = self.subagent_panel.desired_height();
        let minimum_composer_height = usize::from(!composer_lines.is_empty());
        let minimum_activity_history = usize::from(self.subagent_panel.is_active());
        let pending_input_reserve = desired_pending_input_height.min(2).min(
            interactive_budget.saturating_sub(minimum_composer_height + minimum_activity_history),
        );
        let subagent_reserve = desired_subagent_height.min(interactive_budget.saturating_sub(
            minimum_composer_height + minimum_activity_history + pending_input_reserve,
        ));
        let composer_budget = interactive_budget
            .saturating_sub(minimum_activity_history + pending_input_reserve + subagent_reserve);
        let visible_composer_len = composer_lines.len().min(composer_budget);
        let composer_start =
            visible_composer_start(cursor_line, composer_lines.len(), visible_composer_len);
        let pending_input_height = desired_pending_input_height
            .min(interactive_budget.saturating_sub(
                minimum_activity_history + visible_composer_len + subagent_reserve,
            ));
        let subagent_height = desired_subagent_height.min(interactive_budget.saturating_sub(
            minimum_activity_history + visible_composer_len + pending_input_height,
        ));
        let history_height = interactive_budget
            .saturating_sub(visible_composer_len + pending_input_height + subagent_height);

        let mut y = area.y;
        let history = Rect::new(area.x, y, area.width, history_height as u16);
        y = y.saturating_add(history.height);
        let activity_y = history.bottom().saturating_sub(1);
        let jump_text = show_jump_to_bottom.then(|| self.jump_to_bottom_text(width));
        let jump_width = jump_text.as_deref().map_or(0, display_width).min(width) as u16;
        let jump_to_bottom = jump_text.map(|_| {
            Rect::new(
                history
                    .x
                    .saturating_add(history.width.saturating_sub(jump_width)),
                activity_y,
                jump_width,
                1,
            )
        });
        let activity_available = if jump_width > 0 {
            width.saturating_sub(jump_width as usize + 1)
        } else {
            width
        };
        let activity_status = self.activity_status();
        let activity_width = activity_status
            .map(|status| activity::activity_width(activity_available, status))
            .unwrap_or(0) as u16;
        let activity = (activity_width > 0 && history.height > 0)
            .then(|| Rect::new(history.x, activity_y, activity_width, 1));
        let activity_background = activity.map(|_| {
            Rect::new(
                area.x,
                activity_y,
                area.width,
                area.bottom().saturating_sub(activity_y),
            )
        });
        let activity_rail = activity.map(|_| Rect::new(history.x, activity_y, history.width, 1));
        let pending_input = Rect::new(area.x, y, area.width, pending_input_height as u16);
        y = y.saturating_add(pending_input.height);
        let subagents = Rect::new(area.x, y, area.width, subagent_height as u16);
        y = y.saturating_add(subagents.height);
        let top_divider = if show_top_divider {
            let rect = Rect::new(area.x, y, area.width, 1);
            y = y.saturating_add(1);
            rect
        } else {
            Rect::new(area.x, y, area.width, 0)
        };
        let composer = Rect::new(area.x, y, area.width, visible_composer_len as u16);
        y = y.saturating_add(composer.height);
        let bottom_divider = Rect::new(area.x, y, area.width, bottom_divider_height as u16);
        y = y.saturating_add(bottom_divider.height);
        let statusline = Rect::new(area.x, y, area.width, statusline_height as u16);
        y = y.saturating_add(statusline.height);
        let commands = Rect::new(area.x, y, area.width, command_height as u16);

        ScreenLayout {
            history,
            history_scrollbar: HistoryScrollbar::new(
                history,
                history_len,
                self.visible_history_start(history_len, history_height),
            ),
            activity_background,
            activity_rail,
            activity,
            jump_to_bottom,
            pending_input,
            subagents,
            top_divider,
            composer,
            bottom_divider,
            statusline,
            commands,
            composer_start,
            history_len,
        }
    }

    pub(super) fn history_height_for_screen(
        &self,
        width: usize,
        height: usize,
        _now: Instant,
    ) -> usize {
        self.history_height_from_line_counts(
            height,
            self.composer_lines(width).len(),
            self.command_suggestion_lines(width).len(),
        )
    }

    pub(super) fn history_height_from_line_counts(
        &self,
        height: usize,
        composer_line_count: usize,
        command_line_count: usize,
    ) -> usize {
        let statusline_height = self.statusline.height().min(height);
        let bottom_divider_height = usize::from(height > statusline_height);
        let command_height = command_line_count
            .min(height.saturating_sub(statusline_height + bottom_divider_height));
        let bottom_fixed_height = bottom_divider_height + statusline_height + command_height;
        let available_above_bottom = height.saturating_sub(bottom_fixed_height);
        let show_top_divider = available_above_bottom > 1 && composer_line_count > 0;
        let reserved_above_composer = usize::from(show_top_divider);
        let interactive_budget = available_above_bottom.saturating_sub(reserved_above_composer);
        let minimum_composer_height = usize::from(composer_line_count > 0);
        let minimum_activity_history = usize::from(self.subagent_panel.is_active());
        let pending_input_reserve = self.pending_input_height().min(2).min(
            interactive_budget.saturating_sub(minimum_composer_height + minimum_activity_history),
        );
        let subagent_height =
            self.subagent_panel
                .desired_height()
                .min(interactive_budget.saturating_sub(
                    minimum_composer_height + minimum_activity_history + pending_input_reserve,
                ));
        let composer_budget = interactive_budget
            .saturating_sub(minimum_activity_history + pending_input_reserve + subagent_height);
        let visible_composer_len = composer_line_count.min(composer_budget);
        let pending_input_height = self.pending_input_height().min(
            interactive_budget
                .saturating_sub(minimum_activity_history + visible_composer_len + subagent_height),
        );
        interactive_budget
            .saturating_sub(visible_composer_len + pending_input_height + subagent_height)
    }
}
