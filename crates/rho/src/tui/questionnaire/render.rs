use ratatui::{
    layout::Position,
    style::Style,
    text::{Line, Span},
};

use crate::questionnaire::QuestionnaireQuestionKind;

use super::{
    answer_is_empty, choice_count, normalize_questionnaire_answer, questionnaire_answer_display,
    FieldSelection, QuestionnaireComposer, QuestionnaireFieldState, QuestionnaireQuestion,
};
use crate::tui::{
    render::{
        display_width, input_cursor_position, input_visual_lines, styled_line, truncate_one_line,
        wrap_line_at_whitespace, LineFill,
    },
    theme::Theme,
};

pub(in crate::tui) fn questionnaire_lines(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> Vec<Line<'static>> {
    questionnaire_frame(questionnaire, width).0
}

pub(in crate::tui) fn questionnaire_cursor_position(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> Position {
    questionnaire_frame(questionnaire, width).1
}

pub(super) fn questionnaire_frame(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> (Vec<Line<'static>>, Position) {
    let width = width.max(1);
    let questions = &questionnaire.request.questions;
    let mut lines = Vec::new();

    push_header_lines(&mut lines, questionnaire, width);
    if questions.len() > 1 {
        push_tab_lines(&mut lines, questionnaire, width);
        lines.push(Line::raw(""));
    }

    let active = questionnaire.active_index;
    let cursor = push_active_question(
        &mut lines,
        &questions[active],
        &questionnaire.fields[active],
        active,
        questions.len(),
        width,
    );

    lines.push(Line::raw(""));
    lines.push(styled_line(
        truncate_one_line(&footer_hint(questionnaire), width),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    (lines, cursor)
}

fn push_header_lines(
    lines: &mut Vec<Line<'static>>,
    questionnaire: &QuestionnaireComposer,
    width: usize,
) {
    let request = &questionnaire.request;
    if let Some(title) = &request.title {
        push_hanging_text(lines, "", title, width, Theme::input_prompt());
    }
    if let Some(reason) = &request.reason {
        push_hanging_text(lines, "", reason, width, Theme::dim_italic());
    }
    if !lines.is_empty() {
        lines.push(Line::raw(""));
    }
}

const TAB_LABEL_MAX: usize = 16;
const TAB_SEPARATOR: &str = " │ ";
const TAB_OVERFLOW_LEFT: &str = "… ";
const TAB_OVERFLOW_RIGHT: &str = " …";

/// A single-row tab bar with one chip per question. When the chips do not all
/// fit, the bar scrolls: a contiguous window around the active chip is shown
/// and hidden chips are indicated with dim ellipses. The active chip is
/// highlighted; answered chips carry a check mark.
fn push_tab_lines(
    lines: &mut Vec<Line<'static>>,
    questionnaire: &QuestionnaireComposer,
    width: usize,
) {
    let chips = questionnaire
        .request
        .questions
        .iter()
        .zip(questionnaire.fields.iter())
        .enumerate()
        .map(|(index, (question, field))| {
            let answered = field_answer_summary(question, field).is_some();
            let label = question.header.as_deref().unwrap_or(&question.question);
            format!(
                "{} {}{}",
                index + 1,
                truncate_one_line(label, TAB_LABEL_MAX),
                if answered { " ✓" } else { "" }
            )
        })
        .collect::<Vec<_>>();
    let chip_widths = chips
        .iter()
        .map(|chip| display_width(chip))
        .collect::<Vec<_>>();
    let (start, end) = tab_window(&chip_widths, questionnaire.active_index, width);

    let mut spans: Vec<Span<'static>> = Vec::new();
    if start > 0 {
        spans.push(Span::styled(TAB_OVERFLOW_LEFT, Theme::dim()));
    }
    for (index, chip) in chips.into_iter().enumerate().take(end).skip(start) {
        if index > start {
            spans.push(Span::styled(TAB_SEPARATOR, Theme::dim()));
        }
        let style = if index == questionnaire.active_index {
            Theme::input_prompt()
        } else {
            Theme::dim()
        };
        spans.push(Span::styled(chip, style));
    }
    if end < chip_widths.len() {
        spans.push(Span::styled(TAB_OVERFLOW_RIGHT, Theme::dim()));
    }
    lines.push(Line::from(spans));
}

/// Pick the contiguous chip window `[start, end)` to display: the earliest
/// start whose fitting window still contains the active chip, so the bar only
/// scrolls once the active chip would otherwise fall off the right edge.
fn tab_window(chip_widths: &[usize], active: usize, width: usize) -> (usize, usize) {
    let separator_width = display_width(TAB_SEPARATOR);
    let left_overflow_width = display_width(TAB_OVERFLOW_LEFT);
    let right_overflow_width = display_width(TAB_OVERFLOW_RIGHT);
    for start in 0..=active {
        let mut used = if start > 0 { left_overflow_width } else { 0 };
        let mut end = start;
        while end < chip_widths.len() {
            let mut needed = used + chip_widths[end];
            if end > start {
                needed += separator_width;
            }
            if end + 1 < chip_widths.len() {
                needed += right_overflow_width;
            }
            if needed > width && end > start {
                break;
            }
            used += chip_widths[end] + if end > start { separator_width } else { 0 };
            end += 1;
        }
        if end > active {
            return (start, end);
        }
    }
    (active, active + 1)
}

fn push_active_question(
    lines: &mut Vec<Line<'static>>,
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    index: usize,
    total: usize,
    width: usize,
) -> Position {
    push_hanging_text(
        lines,
        "▸ ",
        &format!("{}{}", question_number(index, total), question.question),
        width,
        Theme::input_prompt(),
    );

    let mut meta = Vec::new();
    if !question.required {
        meta.push("optional".to_string());
    }
    if let Some(help) = &question.help {
        meta.push(help.clone());
    }
    if !meta.is_empty() {
        push_hanging_text(lines, "  ", &meta.join(" · "), width, Theme::dim());
    }

    match question.kind {
        QuestionnaireQuestionKind::Text => {
            let start = lines.len();
            push_prefixed_input_lines(lines, "  ", &field.other_value, width, Theme::text());
            prefixed_input_cursor(&field.other_value, field.other_cursor, "  ", start, width)
        }
        QuestionnaireQuestionKind::Choice
        | QuestionnaireQuestionKind::MultiSelect
        | QuestionnaireQuestionKind::Confirm => {
            let mut cursor = Position { x: 0, y: 0 };
            for choice_index in 0..choice_count(question) {
                let highlighted = field.choice_cursor == choice_index;
                let is_other = question.allow_other && choice_index == question.choices.len();
                let marker = questionnaire_selection_marker(question, field, choice_index);
                let arrow = if highlighted { "→" } else { " " };
                let style = questionnaire_choice_style(question, field, choice_index, highlighted);
                let row_start = lines.len();
                if is_other && questionnaire_other_selected(field) {
                    let prefix = format!("  {arrow} {marker} other: ");
                    push_prefixed_input_lines(lines, &prefix, &field.other_value, width, style);
                    if highlighted {
                        cursor = if field.text_entry_active(question) {
                            prefixed_input_cursor(
                                &field.other_value,
                                field.other_cursor,
                                &prefix,
                                row_start,
                                width,
                            )
                        } else {
                            Position {
                                x: 2,
                                y: row_start as u16,
                            }
                        };
                    }
                } else {
                    let recommended = super::choice_is_focused_default(question, choice_index);
                    let label = if is_other {
                        if field.other_value.is_empty() {
                            "other…".to_string()
                        } else {
                            format!("other: {}", field.other_value)
                        }
                    } else {
                        questionnaire_choice_label(question, choice_index)
                    };
                    let prefix = format!("  {arrow} {marker} ");
                    if recommended {
                        push_choice_label_with_recommended(lines, &prefix, &label, width, style);
                    } else {
                        push_hanging_text(lines, &prefix, &label, width, style);
                    }
                    if let Some(description) = question
                        .choices
                        .get(choice_index)
                        .filter(|_| !is_other)
                        .and_then(|choice| choice.description_text())
                    {
                        push_hanging_text(lines, "        ", description, width, Theme::dim());
                    }
                    if highlighted {
                        cursor = Position {
                            x: 2,
                            y: row_start as u16,
                        };
                    }
                }
            }
            cursor
        }
    }
}

fn field_answer_summary(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
) -> Option<String> {
    let value = normalize_questionnaire_answer(question, field).ok()?;
    if answer_is_empty(&value) {
        return None;
    }
    Some(questionnaire_answer_display(Some(question), &value))
}

fn question_number(index: usize, total: usize) -> String {
    if total > 1 {
        format!("{}. ", index + 1)
    } else {
        String::new()
    }
}

fn footer_hint(questionnaire: &QuestionnaireComposer) -> String {
    let question = questionnaire.active_question();
    let mut parts = Vec::new();
    if matches!(question.kind, QuestionnaireQuestionKind::Text) {
        parts.push("type your answer");
    } else {
        parts.push("↑↓ choose");
        if matches!(question.kind, QuestionnaireQuestionKind::MultiSelect) {
            parts.push("space toggle");
        }
        if question.allow_other {
            parts.push("type for other");
        }
    }
    if questionnaire.on_last_question() {
        parts.push("enter submit");
    } else {
        parts.push("enter next");
    }
    parts.push("esc cancel");
    parts.join(" · ")
}

fn questionnaire_choice_style(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
    highlighted: bool,
) -> Style {
    if highlighted {
        return Theme::accent();
    }
    if questionnaire_choice_selected(question, field, choice_index) {
        return Theme::text_strong();
    }
    Theme::text()
}

fn questionnaire_choice_selected(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
) -> bool {
    match &field.selection {
        FieldSelection::Multi { selected, other } => {
            if choice_index < question.choices.len() {
                selected.contains(&choice_index)
            } else {
                *other
            }
        }
        FieldSelection::Single(index) => *index == choice_index,
        FieldSelection::Other => question.allow_other && choice_index == question.choices.len(),
        FieldSelection::Text | FieldSelection::None => false,
    }
}

fn questionnaire_selection_marker(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
) -> &'static str {
    let selected = questionnaire_choice_selected(question, field, choice_index);
    match question.kind {
        QuestionnaireQuestionKind::MultiSelect => {
            if selected {
                "■"
            } else {
                "□"
            }
        }
        _ => {
            if selected {
                "●"
            } else {
                "○"
            }
        }
    }
}

fn questionnaire_choice_label(question: &QuestionnaireQuestion, choice_index: usize) -> String {
    match question.kind {
        QuestionnaireQuestionKind::Confirm => question
            .choices
            .get(choice_index)
            .map(|choice| choice.label().to_string())
            .unwrap_or_else(|| if choice_index == 0 { "yes" } else { "no" }.into()),
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect => question
            .choices
            .get(choice_index)
            .map(|choice| choice.label().to_string())
            .unwrap_or_else(|| "other…".into()),
        QuestionnaireQuestionKind::Text => String::new(),
    }
}

fn questionnaire_other_selected(field: &QuestionnaireFieldState) -> bool {
    match &field.selection {
        FieldSelection::Other => true,
        FieldSelection::Multi { other, .. } => *other,
        FieldSelection::Text | FieldSelection::None | FieldSelection::Single(_) => false,
    }
}

fn push_hanging_text(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    text: &str,
    width: usize,
    style: Style,
) {
    let prefix_width = display_width(prefix);
    let inner_width = width.saturating_sub(prefix_width).max(1);
    let continuation = " ".repeat(prefix_width);
    let mut first = true;
    for raw_line in text.lines() {
        let chunks = wrap_line_at_whitespace(raw_line, inner_width);
        let chunks = if chunks.is_empty() {
            vec![String::new()]
        } else {
            chunks
        };
        for chunk in chunks {
            let prefix = if first { prefix } else { continuation.as_str() };
            first = false;
            lines.push(styled_line(
                format!("{prefix}{chunk}"),
                width,
                style,
                LineFill::Natural,
            ));
        }
    }
    if first {
        lines.push(styled_line(
            prefix.to_string(),
            width,
            style,
            LineFill::Natural,
        ));
    }
}

/// Render a choice label with a dim "(recommended)" badge as its own span.
fn push_choice_label_with_recommended(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    label: &str,
    width: usize,
    style: Style,
) {
    const BADGE: &str = " (recommended)";
    let prefix_width = display_width(prefix);
    let inner_width = width.saturating_sub(prefix_width).max(1);
    let badge_width = display_width(BADGE);

    // Common path: label and badge share one line with separate styles.
    if !label.contains('\n') && display_width(label) + badge_width <= inner_width {
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix}{label}"), style),
            Span::styled(BADGE.to_string(), Theme::dim()),
        ]));
        return;
    }

    // Rare overflow: wrap the label, then put the badge on the following line.
    push_hanging_text(lines, prefix, label, width, style);
    lines.push(Line::from(vec![
        Span::styled(" ".repeat(prefix_width), style),
        Span::styled(BADGE.trim_start().to_string(), Theme::dim()),
    ]));
}

fn push_prefixed_input_lines(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    value: &str,
    width: usize,
    style: Style,
) {
    let prefix_width = display_width(prefix);
    let input_width = width.saturating_sub(prefix_width).max(1);
    let continuation = " ".repeat(prefix_width);
    for (index, line) in input_visual_lines(value, input_width)
        .into_iter()
        .enumerate()
    {
        let prefix = if index == 0 { prefix } else { &continuation };
        lines.push(styled_line(
            format!("{prefix}{line}"),
            width,
            style,
            LineFill::Natural,
        ));
    }
}

fn prefixed_input_cursor(
    value: &str,
    cursor: usize,
    prefix: &str,
    start_y: usize,
    width: usize,
) -> Position {
    let prefix_width = display_width(prefix);
    let mut position =
        input_cursor_position(value, cursor, width.saturating_sub(prefix_width).max(1));
    position.x = position.x.saturating_add(prefix_width as u16);
    position.y = position.y.saturating_add(start_y as u16);
    position
}
