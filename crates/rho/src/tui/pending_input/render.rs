use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::*;
use crate::tui::{
    render::{display_width, truncate_one_line},
    theme::Theme,
};

const MAX_VISIBLE_ITEMS: usize = 3;

impl App {
    pub(in crate::tui) fn pending_input_height(&self) -> usize {
        let count = self.pending_input_count();
        if count == 0 {
            return 0;
        }
        1 + count.min(MAX_VISIBLE_ITEMS) + usize::from(self.pending_input_panel.focused)
    }

    pub(in crate::tui) fn pending_input_lines(&mut self, width: usize) -> Vec<Line<'static>> {
        let items = self.pending_input_refs();
        if items.is_empty() {
            self.pending_input_panel.focused = false;
            return Vec::new();
        }
        self.clamp_pending_input_selection(items.len());
        let selected = self.pending_input_panel.selected;
        let start = visible_start(selected, items.len(), MAX_VISIBLE_ITEMS);
        let steering_count = self.accepted_steering.len() + self.steering_prompts.len();
        let hint = if self.pending_input_panel.focused {
            "↑↓ select · enter edit · backspace discard · esc close".to_string()
        } else {
            format!(
                "{} edit · {} manage",
                self.info.keybindings.edit_pending_input,
                self.info.keybindings.manage_pending_input
            )
        };
        let mut lines = vec![pending_header_line(
            width,
            steering_count,
            self.queued_prompts.len(),
            &hint,
        )];
        lines.extend(
            items
                .iter()
                .skip(start)
                .take(MAX_VISIBLE_ITEMS)
                .enumerate()
                .map(|(visible_index, item)| {
                    self.pending_item_line(width, *item, start + visible_index == selected)
                }),
        );
        if self.pending_input_panel.focused {
            lines.push(Line::styled(
                truncate_one_line(
                    "  steer affects this run · next starts after this turn",
                    width,
                ),
                Theme::dim(),
            ));
        }
        lines
    }

    fn pending_item_line(
        &self,
        width: usize,
        item: PendingInputRef,
        selected: bool,
    ) -> Line<'static> {
        let marker = if selected { "▸ " } else { "  " };
        let (label, context, prompt, label_style) = match item {
            PendingInputRef::AcceptedSteering(index) => {
                let entry = &self.accepted_steering[index];
                let context = if self.retracting_steering.as_ref() == Some(&entry.id) {
                    "retracting"
                } else {
                    "current run"
                };
                (
                    "STEER",
                    context,
                    &entry.prompt.display_prompt,
                    Theme::warning(),
                )
            }
            PendingInputRef::LocalSteering(index) => (
                "STEER",
                "sending",
                &self.steering_prompts[index].display_prompt,
                Theme::warning(),
            ),
            PendingInputRef::FollowUp(index) => (
                "NEXT",
                "after turn",
                &self.queued_prompts[index].display_prompt,
                Theme::accent(),
            ),
        };
        pending_item_line(width, marker, label, context, prompt, label_style, selected)
    }
}

fn pending_header_line(
    width: usize,
    steering_count: usize,
    follow_up_count: usize,
    hint: &str,
) -> Line<'static> {
    let steering = count_label(steering_count, "steer", "steers");
    let follow_up = count_label(follow_up_count, "follow-up", "follow-ups");
    let counts = match (steering_count, follow_up_count) {
        (0, _) => follow_up,
        (_, 0) => steering,
        _ => format!("{steering} · {follow_up}"),
    };
    let left = format!("  pending input · {counts}");
    let left_width = display_width(&left);
    let hint_width = display_width(hint);
    if left_width + hint_width + 2 <= width {
        return Line::from(vec![
            Span::styled(left, Theme::text_strong()),
            Span::raw(" ".repeat(width - left_width - hint_width)),
            Span::styled(hint.to_string(), Theme::dim()),
        ]);
    }
    Line::styled(
        truncate_one_line(&format!("{left}  {hint}"), width),
        Theme::text_strong(),
    )
}

fn pending_item_line(
    width: usize,
    marker: &str,
    label: &str,
    context: &str,
    prompt: &str,
    label_style: Style,
    selected: bool,
) -> Line<'static> {
    let compact = width < 34;
    let context = if compact { "" } else { context };
    let prefix = if context.is_empty() {
        format!("{marker}{label:<6} ")
    } else {
        format!("{marker}{label:<6} · {context:<11} ")
    };
    let prefix_width = display_width(&prefix);
    if prefix_width >= width {
        return Line::styled(
            truncate_one_line(&format!("{marker}{label} {prompt}"), width),
            label_style,
        );
    }
    let prompt = truncate_one_line(prompt, width - prefix_width);
    let marker_style = if selected {
        Theme::accent()
    } else {
        Theme::dim()
    };
    let context_prefix = if context.is_empty() {
        format!("{label:<6} ")
    } else {
        format!("{label:<6} · {context:<11} ")
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(label.to_string(), label_style),
        Span::styled(context_prefix[label.len()..].to_string(), Theme::dim()),
        Span::styled(
            prompt,
            if selected {
                Theme::text_strong()
            } else {
                Theme::text()
            },
        ),
    ])
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    let label = if count == 1 { singular } else { plural };
    format!("{count} {label}")
}

fn visible_start(selected: usize, count: usize, visible: usize) -> usize {
    selected
        .saturating_sub(visible.saturating_sub(1))
        .min(count.saturating_sub(visible))
}
