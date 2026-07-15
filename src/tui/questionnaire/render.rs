use ratatui::{layout::Position, style::Style, text::Line};

use crate::questionnaire::{QuestionnaireQuestion, QuestionnaireQuestionKind};

use super::{
    choice_count, questionnaire_default_display, FieldSelection, QuestionnaireComposer,
    QuestionnaireFieldState,
};
use crate::tui::{
    render::{
        display_width, input_cursor_position, input_visual_lines, push_wrapped_text,
        push_wrapped_text_with, styled_line, truncate_one_line, wrap_line_at_whitespace, LineFill,
    },
    theme::Theme,
};

pub(in crate::tui) fn questionnaire_lines(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> Vec<Line<'static>> {
    questionnaire_frame(questionnaire, width).0
}

pub(super) fn questionnaire_frame(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> (Vec<Line<'static>>, Position) {
    let mut lines = Vec::new();
    if let Some(title) = &questionnaire.request.title {
        push_wrapped_text(
            &mut lines,
            title,
            width,
            Theme::input_prompt(),
            LineFill::Natural,
        );
    } else {
        push_wrapped_text(
            &mut lines,
            &format!(
                "answer {} question(s)",
                questionnaire.request.questions.len()
            ),
            width,
            Theme::input_prompt(),
            LineFill::Natural,
        );
    }
    if let Some(reason) = &questionnaire.request.reason {
        push_wrapped_text(
            &mut lines,
            &format!("reason: {reason}"),
            width,
            Theme::dim(),
            LineFill::Natural,
        );
    }
    lines.push(styled_line(
        truncate_one_line(
            "enter submit · up/down choose · space toggle · tab next · type only for other",
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));

    let mut cursor = Position { x: 0, y: 0 };
    for (index, (question, field)) in questionnaire
        .request
        .questions
        .iter()
        .zip(questionnaire.fields.iter())
        .enumerate()
    {
        let active = questionnaire.active_index == index;
        let before = lines.len();
        questionnaire_push_question_lines(&mut lines, question, field, index, active, width);
        if active {
            cursor = questionnaire_question_cursor(question, field, before, width);
        }
    }
    (lines, cursor)
}

fn questionnaire_push_question_lines(
    lines: &mut Vec<Line<'static>>,
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    index: usize,
    active: bool,
    width: usize,
) {
    let marker = if active { ">" } else { " " };
    let required = if question.required { "" } else { " optional" };
    push_wrapped_text(
        lines,
        &format!("{marker} {}. {}{required}", index + 1, question.question),
        width,
        if active {
            Theme::input_prompt()
        } else {
            Theme::dim()
        },
        LineFill::Natural,
    );
    if let Some(help) = &question.help {
        push_wrapped_text(
            lines,
            &format!("  help: {help}"),
            width,
            Theme::dim(),
            LineFill::Natural,
        );
    }
    let answer_hint = questionnaire_answer_hint(question);
    if !answer_hint.is_empty() {
        push_wrapped_text(
            lines,
            &format!("  {answer_hint}"),
            width,
            Theme::dim(),
            LineFill::Natural,
        );
    }

    match question.kind {
        QuestionnaireQuestionKind::Text => {
            push_prefixed_input_lines(lines, "  ", &field.other_value, width, Theme::text());
        }
        QuestionnaireQuestionKind::Choice
        | QuestionnaireQuestionKind::MultiSelect
        | QuestionnaireQuestionKind::Confirm => {
            for choice_index in 0..choice_count(question) {
                let highlighted = active && field.choice_cursor == choice_index;
                let line_marker = if highlighted { " >" } else { "  " };
                let selection_marker =
                    questionnaire_selection_marker(question, field, choice_index);
                let label = questionnaire_choice_label(question, choice_index);
                push_wrapped_text(
                    lines,
                    &format!("{line_marker} {selection_marker} {label}"),
                    width,
                    if highlighted {
                        Theme::input_prompt()
                    } else {
                        Theme::text()
                    },
                    LineFill::Natural,
                );
                if question.allow_other
                    && choice_index == question.choices.len()
                    && questionnaire_other_selected(field)
                {
                    push_prefixed_input_lines(
                        lines,
                        "      other: ",
                        &field.other_value,
                        width,
                        Theme::text(),
                    );
                }
            }
        }
    }
}

fn questionnaire_answer_hint(question: &QuestionnaireQuestion) -> String {
    let mut hints = Vec::new();
    match question.kind {
        QuestionnaireQuestionKind::Text => hints.push("free text only when needed".into()),
        QuestionnaireQuestionKind::Choice => hints.push("select one".into()),
        QuestionnaireQuestionKind::MultiSelect => hints.push("select one or more".into()),
        QuestionnaireQuestionKind::Confirm => hints.push("select yes or no".into()),
    }
    if !question.choices.is_empty() && question.allow_other {
        hints.push("other available".into());
    }
    if let Some(default) = &question.default {
        hints.push(format!(
            "default: {}",
            questionnaire_default_display(default)
        ));
    }
    hints.join(" · ")
}

fn questionnaire_selection_marker(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
) -> &'static str {
    match (&question.kind, &field.selection) {
        (QuestionnaireQuestionKind::MultiSelect, FieldSelection::Multi { selected, other }) => {
            if choice_index < question.choices.len() {
                if selected.contains(&choice_index) {
                    "[x]"
                } else {
                    "[ ]"
                }
            } else if *other {
                "[x]"
            } else {
                "[ ]"
            }
        }
        (_, FieldSelection::Single(index)) if *index == choice_index => "(x)",
        (_, FieldSelection::Other)
            if question.allow_other && choice_index == question.choices.len() =>
        {
            "(x)"
        }
        (_, FieldSelection::None) => "( )",
        _ => "( )",
    }
}

fn questionnaire_choice_label(question: &QuestionnaireQuestion, choice_index: usize) -> String {
    match question.kind {
        QuestionnaireQuestionKind::Confirm => {
            if choice_index == 0 {
                "yes".into()
            } else {
                "no".into()
            }
        }
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect => question
            .choices
            .get(choice_index)
            .cloned()
            .unwrap_or_else(|| "Other".into()),
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

pub(in crate::tui) fn questionnaire_cursor_position(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> Position {
    questionnaire_frame(questionnaire, width).1
}

pub(super) fn questionnaire_question_cursor(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
    start_y: usize,
    width: usize,
) -> Position {
    let prefix_height = questionnaire_question_prefix_line_count(question, width);
    match question.kind {
        QuestionnaireQuestionKind::Text => prefixed_input_cursor(
            &field.other_value,
            field.other_cursor,
            "  ",
            start_y + prefix_height,
            width,
        ),
        QuestionnaireQuestionKind::Choice
        | QuestionnaireQuestionKind::MultiSelect
        | QuestionnaireQuestionKind::Confirm => {
            let mut y = start_y + prefix_height;
            for choice_index in 0..choice_count(question) {
                if field.choice_cursor == choice_index {
                    if question.allow_other
                        && choice_index == question.choices.len()
                        && field.text_entry_active(question)
                    {
                        let option_lines = wrapped_line_count(
                            format!(
                                " > {} {}",
                                questionnaire_selection_marker(question, field, choice_index),
                                questionnaire_choice_label(question, choice_index)
                            ),
                            width,
                        );
                        return prefixed_input_cursor(
                            &field.other_value,
                            field.other_cursor,
                            "      other: ",
                            y + option_lines,
                            width,
                        );
                    }
                    return Position { x: 1, y: y as u16 };
                }
                y += wrapped_line_count(
                    format!(
                        "   {} {}",
                        questionnaire_selection_marker(question, field, choice_index),
                        questionnaire_choice_label(question, choice_index)
                    ),
                    width,
                );
                if question.allow_other
                    && choice_index == question.choices.len()
                    && questionnaire_other_selected(field)
                {
                    y += input_visual_lines(
                        &field.other_value,
                        width.saturating_sub(display_width("      other: ")).max(1),
                    )
                    .len();
                }
            }
            Position { x: 1, y: y as u16 }
        }
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

fn questionnaire_question_prefix_line_count(
    question: &QuestionnaireQuestion,
    width: usize,
) -> usize {
    let required = if question.required { "" } else { " optional" };
    let mut count = wrapped_line_count(format!("> 1. {}{required}", question.question), width);
    if let Some(help) = &question.help {
        count += wrapped_line_count(format!("  help: {help}"), width);
    }
    let answer_hint = questionnaire_answer_hint(question);
    if !answer_hint.is_empty() {
        count += wrapped_line_count(format!("  {answer_hint}"), width);
    }
    count
}

fn wrapped_line_count(text: String, width: usize) -> usize {
    let mut lines = Vec::new();
    push_wrapped_text_with(
        &mut lines,
        &text,
        width,
        Theme::text(),
        LineFill::Natural,
        wrap_line_at_whitespace,
    );
    lines.len()
}
