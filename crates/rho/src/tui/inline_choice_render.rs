//! Compact choice groups with a distinct focus marker and wrapped explanations.

use ratatui::{
    layout::Position,
    style::Modifier,
    text::{Line, Span},
};

use super::{InlineChoice, InlineChoiceOption};
use crate::tui::{
    composer_chrome::{wrap_footer_parts, SELECTION_MARKER_ACTIVE, SELECTION_MARKER_INACTIVE},
    display_width,
    panel_text::indented_wrapped_lines,
    render::{truncate_to_display_width, wrap_line_at_whitespace},
    view_composer::ComposerFrame,
    Theme,
};

pub(in crate::tui) fn inline_choice_frame(
    choice: &InlineChoice,
    width: usize,
    return_to_parent: bool,
) -> ComposerFrame {
    let width = width.max(1);
    let mut lines = indented_wrapped_lines(
        &choice.title,
        /*indent*/ 0,
        width,
        Theme::text().add_modifier(Modifier::BOLD),
    );
    if !choice.description.is_empty() {
        lines.extend(indented_wrapped_lines(
            &choice.description,
            /*indent*/ 0,
            width,
            Theme::dim(),
        ));
    }
    lines.push(Line::default());

    let mut focused_row = 0;
    let mut previous_wrapped = false;
    for (index, option) in choice.options.iter().enumerate() {
        let group = option_lines(option, index == choice.active, width);
        let wrapped = group.len() > 1 + usize::from(!option.detail.is_empty());
        // Single-line labels/details stay compact; separate multiline groups.
        if index > 0 && (previous_wrapped || wrapped) {
            lines.push(Line::default());
        }
        if index == choice.active {
            focused_row = lines.len();
        }
        lines.extend(group);
        previous_wrapped = wrapped;
    }

    let shortcuts = choice
        .options
        .iter()
        .filter(|option| option.available)
        .map(|option| option.shortcut.to_string())
        .collect::<Vec<_>>()
        .join("/");
    let shortcut_hint = format!("{shortcuts} choose");
    lines.push(Line::default());
    for part in wrap_footer_parts(
        [
            "↑↓ move",
            "Enter/Space choose",
            &shortcut_hint,
            if return_to_parent {
                "Esc back"
            } else {
                "Esc cancel"
            },
        ],
        width,
    ) {
        lines.extend(indented_wrapped_lines(
            &part,
            /*indent*/ 0,
            width,
            Theme::dim(),
        ));
    }
    // The composer viewport follows this row even though choices hide the caret.
    ComposerFrame::new(
        lines,
        Position {
            x: 0,
            y: u16::try_from(focused_row).unwrap_or(u16::MAX),
        },
    )
}

fn option_lines(option: &InlineChoiceOption, focused: bool, width: usize) -> Vec<Line<'static>> {
    let focused = focused && option.available;
    let marker = if focused {
        SELECTION_MARKER_ACTIVE
    } else if option.available {
        SELECTION_MARKER_INACTIVE
    } else {
        "·"
    };
    let style = if focused {
        Theme::text().add_modifier(Modifier::BOLD)
    } else if option.available {
        Theme::text()
    } else {
        Theme::dim()
    };
    // Reserve at least one cell for the label, even in a tiny terminal.
    let prefix = format!("{marker} {}  ", option.shortcut);
    let prefix = truncate_to_display_width(&prefix, width.saturating_sub(1));
    let indent = display_width(&prefix);
    let mut lines = Vec::new();
    for (index, part) in wrap_line_at_whitespace(&option.label, width - indent)
        .into_iter()
        .enumerate()
    {
        let mut spans = if index == 0 {
            let mut chars = prefix.chars();
            vec![
                Span::styled(
                    chars.next().map(String::from).unwrap_or_default(),
                    if focused {
                        Theme::input_prompt()
                    } else {
                        Theme::dim()
                    },
                ),
                Span::styled(chars.as_str().to_owned(), Theme::dim()),
            ]
        } else {
            vec![Span::raw(" ".repeat(indent))]
        };
        spans.push(Span::styled(part.trim_start().to_owned(), style));
        lines.push(Line::from(spans));
    }
    if !option.detail.is_empty() {
        lines.extend(indented_wrapped_lines(
            &option.detail,
            indent,
            width,
            Theme::dim(),
        ));
    }
    lines
}
