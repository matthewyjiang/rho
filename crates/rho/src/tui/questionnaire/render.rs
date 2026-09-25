use ratatui::{
    layout::Position,
    style::Style,
    text::{Line, Span},
};
use rho_sdk::{HostQuestion, SelectionMode};

use super::{
    choice_count, is_confirm, normalize_questionnaire_answer, questionnaire_answer_display,
    request_title, FieldSelection, QuestionnaireComposer, QuestionnaireFieldState,
    QuestionnaireTarget,
};
use crate::tui::{
    composer_pointer::ComposerHit,
    render::{
        display_width, editable_input_visual_lines, input_visual_lines, styled_line,
        truncate_one_line, visual_caret_position, wrap_line_at_whitespace, LineFill,
    },
    theme::Theme,
};

/// Questionnaire composer rows, the caret, and the clickable tab chips and
/// choice rows, derived from one walk so pointer targets match the paint.
pub(in crate::tui) struct QuestionnaireFrame {
    pub(in crate::tui) lines: Vec<Line<'static>>,
    pub(in crate::tui) cursor: Position,
    pub(in crate::tui) hits: Vec<ComposerHit<QuestionnaireTarget>>,
}

/// Renders the questionnaire. `hovered` is the chip or choice under the
/// pointer; unless it is already the active chip or the focused choice, its
/// label lifts to strong text.
pub(in crate::tui) fn questionnaire_frame(
    questionnaire: &QuestionnaireComposer,
    width: usize,
    hovered: Option<QuestionnaireTarget>,
) -> QuestionnaireFrame {
    let width = width.max(1);
    let questions = questionnaire.request.questions();
    let mut lines = Vec::new();
    let mut hits = Vec::new();

    push_header_lines(&mut lines, questionnaire, width);
    if questions.len() > 1 {
        let hovered_tab = match hovered {
            Some(QuestionnaireTarget::Question(index)) => Some(index),
            Some(QuestionnaireTarget::Choice { .. }) | None => None,
        };
        push_tab_lines(&mut lines, &mut hits, questionnaire, width, hovered_tab);
        lines.push(Line::raw(""));
    }

    let active = questionnaire.active_index;
    let hovered_choice = match hovered {
        Some(QuestionnaireTarget::Choice { index, .. }) => Some(index),
        Some(QuestionnaireTarget::Question(_)) | None => None,
    };
    let cursor = push_active_question(
        &mut lines,
        &mut hits,
        ActiveQuestion {
            question: &questions[active],
            field: &questionnaire.fields[active],
            index: active,
            total: questions.len(),
            hovered_choice,
        },
        width,
    );

    lines.push(Line::raw(""));
    for part in crate::tui::composer_chrome::wrap_footer_parts(footer_parts(questionnaire), width) {
        lines.push(styled_line(
            truncate_one_line(&part, width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
    }
    QuestionnaireFrame {
        lines,
        cursor,
        hits,
    }
}

fn push_header_lines(
    lines: &mut Vec<Line<'static>>,
    questionnaire: &QuestionnaireComposer,
    width: usize,
) {
    let request = &questionnaire.request;
    if let Some(title) = request_title(request) {
        push_hanging_text(lines, "", title, width, Theme::input_prompt());
    }
    if let Some(notice) = questionnaire.timeout_notice(std::time::Instant::now()) {
        push_hanging_text(lines, "", &notice, width, Theme::input_prompt());
        if let Some(reason) = request.timeout_reason() {
            push_hanging_text(lines, "", reason, width, Theme::text());
        }
        if let Some(fallback) = request.timeout_fallback() {
            for (id, values) in fallback.answers() {
                push_hanging_text(
                    lines,
                    "",
                    &format!("{id}: {}", values.join(", ")),
                    width,
                    Theme::dim(),
                );
            }
        }
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
/// highlighted, a hovered inactive chip lifts; answered chips carry a check
/// mark.
fn push_tab_lines(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<ComposerHit<QuestionnaireTarget>>,
    questionnaire: &QuestionnaireComposer,
    width: usize,
    hovered: Option<usize>,
) {
    let chips = questionnaire
        .request
        .questions()
        .iter()
        .zip(questionnaire.fields.iter())
        .enumerate()
        .map(|(index, (question, field))| {
            let answered = field_answer_summary(question, field).is_some();
            let label = question.header_text().unwrap_or(question.prompt());
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

    let row = lines.len();
    let mut column = 0;
    let mut spans: Vec<Span<'static>> = Vec::new();
    if start > 0 {
        spans.push(Span::styled(TAB_OVERFLOW_LEFT, Theme::dim()));
        column += display_width(TAB_OVERFLOW_LEFT);
    }
    for (index, chip) in chips.into_iter().enumerate().take(end).skip(start) {
        if index > start {
            spans.push(Span::styled(TAB_SEPARATOR, Theme::dim()));
            column += display_width(TAB_SEPARATOR);
        }
        let style = if index == questionnaire.active_index {
            Theme::input_prompt()
        } else if hovered == Some(index) {
            Theme::text_strong()
        } else {
            Theme::dim()
        };
        hits.push(ComposerHit {
            lines: row..row + 1,
            columns: column..column + chip_widths[index],
            target: QuestionnaireTarget::Question(index),
        });
        column += chip_widths[index];
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

/// The question being answered, and which of its choices the pointer is on.
struct ActiveQuestion<'a> {
    question: &'a HostQuestion,
    field: &'a QuestionnaireFieldState,
    index: usize,
    total: usize,
    hovered_choice: Option<usize>,
}

fn push_active_question(
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<ComposerHit<QuestionnaireTarget>>,
    active: ActiveQuestion<'_>,
    width: usize,
) -> Position {
    let ActiveQuestion {
        question,
        field,
        index,
        total,
        hovered_choice,
    } = active;
    push_hanging_text(
        lines,
        "▸ ",
        &format!("{}{}", question_number(index, total), question.prompt()),
        width,
        Theme::input_prompt(),
    );

    let mut meta = Vec::new();
    if !question.is_required() {
        meta.push("optional".to_string());
    }
    if let Some(help) = question.help_text() {
        meta.push(help.to_string());
    }
    if !meta.is_empty() {
        push_hanging_text(lines, "  ", &meta.join(" · "), width, Theme::dim());
    }

    let mut cursor = Position { x: 0, y: 0 };
    for choice_index in 0..choice_count(question) {
        let highlighted = field.choice_cursor == choice_index;
        let is_other = question.permits_other() && choice_index == question.choices().len();
        let marker = questionnaire_selection_marker(question, field, choice_index);
        let arrow = if highlighted { "→" } else { " " };
        let style = questionnaire_choice_style(
            question,
            field,
            choice_index,
            highlighted,
            /*hovered*/ hovered_choice == Some(choice_index),
        );
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
                .choices()
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
        hits.push(ComposerHit::rows(
            row_start..lines.len(),
            QuestionnaireTarget::Choice {
                index: choice_index,
                confirms: question.selection() != SelectionMode::Many && !is_other,
            },
        ));
    }
    cursor
}

fn field_answer_summary(
    question: &HostQuestion,
    field: &QuestionnaireFieldState,
) -> Option<String> {
    let value = normalize_questionnaire_answer(question, field).ok()?;
    if value.is_empty() {
        return None;
    }
    Some(questionnaire_answer_display(question, &value))
}

fn question_number(index: usize, total: usize) -> String {
    if total > 1 {
        format!("{}. ", index + 1)
    } else {
        String::new()
    }
}

fn footer_parts(questionnaire: &QuestionnaireComposer) -> Vec<&'static str> {
    let question = questionnaire.active_question();
    let mut parts = Vec::new();
    parts.push("↑↓ choose");
    if question.selection() == SelectionMode::Many {
        parts.push("Space toggle");
    }
    if question.permits_other() {
        parts.push("type for other");
    }
    if questionnaire.fields.len() > 1 {
        parts.push("Shift+Tab previous");
        parts.push("Tab next");
    }
    if questionnaire.on_last_question() {
        parts.push("Enter submit");
    } else {
        parts.push("Enter next");
    }
    parts.push("Esc cancel");
    parts
}

/// Choice label ink: the focused row is accented; selected and hovered rows
/// are strong.
fn questionnaire_choice_style(
    question: &HostQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
    highlighted: bool,
    hovered: bool,
) -> Style {
    if highlighted {
        return Theme::accent();
    }
    if hovered || questionnaire_choice_selected(question, field, choice_index) {
        return Theme::text_strong();
    }
    Theme::text()
}

fn questionnaire_choice_selected(
    question: &HostQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
) -> bool {
    match &field.selection {
        FieldSelection::Multi { selected, other } => {
            if choice_index < question.choices().len() {
                selected.contains(&choice_index)
            } else {
                *other
            }
        }
        FieldSelection::Single(index) => *index == choice_index,
        FieldSelection::Other => {
            question.permits_other() && choice_index == question.choices().len()
        }
        FieldSelection::None => false,
    }
}

fn questionnaire_selection_marker(
    question: &HostQuestion,
    field: &QuestionnaireFieldState,
    choice_index: usize,
) -> &'static str {
    let selected = questionnaire_choice_selected(question, field, choice_index);
    match question.selection() {
        SelectionMode::Many => {
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

fn questionnaire_choice_label(question: &HostQuestion, choice_index: usize) -> String {
    if is_confirm(question) {
        question
            .choices()
            .get(choice_index)
            .map(|choice| choice.label().to_string())
            .unwrap_or_else(|| if choice_index == 0 { "yes" } else { "no" }.into())
    } else {
        question
            .choices()
            .get(choice_index)
            .map(|choice| choice.label().to_string())
            .unwrap_or_else(|| "other…".into())
    }
}

fn questionnaire_other_selected(field: &QuestionnaireFieldState) -> bool {
    match &field.selection {
        FieldSelection::Other => true,
        FieldSelection::Multi { other, .. } => *other,
        FieldSelection::None | FieldSelection::Single(_) => false,
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
        let chunks = if chunks.is_empty() { vec![""] } else { chunks };
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
    let inner_width = width.saturating_sub(prefix_width).max(1);
    let visual_lines = editable_input_visual_lines(value, inner_width);
    let mut position = visual_caret_position(&visual_lines, value, cursor);
    position.x = position.x.saturating_add(prefix_width as u16);
    position.y = position.y.saturating_add(start_y as u16);
    position
}
