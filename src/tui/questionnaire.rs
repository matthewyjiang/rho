use ratatui::{layout::Position, style::Style, text::Line};
use tokio::sync::oneshot;

use crate::agent::{
    QuestionnaireAnswer, QuestionnaireQuestion, QuestionnaireQuestionKind, QuestionnaireRequest,
    QuestionnaireResponse,
};

use super::{
    render::{
        display_width, input_cursor_position, input_visual_lines, push_wrapped_text,
        push_wrapped_text_with, styled_line, truncate_one_line, wrap_line_at_whitespace, LineFill,
    },
    theme::Theme,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuestionnaireCancelReason {
    UserCancelled,
    UiUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QuestionnaireReply {
    Answer(QuestionnaireResponse),
    Cancelled(QuestionnaireCancelReason),
}

pub(super) struct QuestionnaireResponseChannel {
    reply_tx: Option<oneshot::Sender<QuestionnaireReply>>,
}

impl std::fmt::Debug for QuestionnaireResponseChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionnaireResponseChannel")
            .field("reply_pending", &self.reply_tx.is_some())
            .finish()
    }
}

impl QuestionnaireResponseChannel {
    pub(super) fn new(reply_tx: oneshot::Sender<QuestionnaireReply>) -> Self {
        Self {
            reply_tx: Some(reply_tx),
        }
    }

    fn send_response(&mut self, response: QuestionnaireResponse) {
        if let Some(reply_tx) = self.reply_tx.take() {
            let _ = reply_tx.send(QuestionnaireReply::Answer(response));
        }
    }

    fn cancel(&mut self, reason: QuestionnaireCancelReason) {
        if let Some(reply_tx) = self.reply_tx.take() {
            let _ = reply_tx.send(QuestionnaireReply::Cancelled(reason));
        }
    }
}

impl Drop for QuestionnaireResponseChannel {
    fn drop(&mut self) {
        self.cancel(QuestionnaireCancelReason::UiUnavailable);
    }
}

#[derive(Debug)]
pub(super) struct QuestionAnswerRequest {
    pub(super) request: QuestionnaireRequest,
    pub(super) response: QuestionnaireResponseChannel,
}

#[derive(Debug)]
pub(super) struct QuestionnaireComposer {
    request: QuestionnaireRequest,
    response: QuestionnaireResponseChannel,
    fields: Vec<QuestionnaireFieldState>,
    active_index: usize,
}

#[derive(Debug)]
struct QuestionnaireFieldState {
    selection: FieldSelection,
    choice_cursor: usize,
    other_value: String,
    other_cursor: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FieldSelection {
    Text,
    None,
    Single(usize),
    Multi { selected: Vec<usize>, other: bool },
    Other,
}

impl QuestionnaireComposer {
    pub(super) fn new(
        request: QuestionnaireRequest,
        response: QuestionnaireResponseChannel,
    ) -> Self {
        let fields = request
            .questions
            .iter()
            .map(QuestionnaireFieldState::new)
            .collect::<Vec<_>>();
        Self {
            request,
            response,
            fields,
            active_index: 0,
        }
    }

    fn active_question(&self) -> &QuestionnaireQuestion {
        &self.request.questions[self.active_index]
    }

    fn active_field(&self) -> &QuestionnaireFieldState {
        &self.fields[self.active_index]
    }

    fn active_field_mut(&mut self) -> &mut QuestionnaireFieldState {
        &mut self.fields[self.active_index]
    }

    fn active_char_len(&self) -> usize {
        self.active_field().char_len()
    }

    pub(super) fn move_to_previous_field(&mut self) {
        self.active_index = self.active_index.saturating_sub(1);
    }

    pub(super) fn move_to_next_field(&mut self) {
        self.active_index = (self.active_index + 1).min(self.fields.len().saturating_sub(1));
    }

    pub(super) fn move_active_choice_previous(&mut self) {
        let question = self.active_question().clone();
        self.active_field_mut().move_choice_previous(&question);
    }

    pub(super) fn move_active_choice_next(&mut self) {
        let question = self.active_question().clone();
        self.active_field_mut().move_choice_next(&question);
    }

    pub(super) fn toggle_active_choice(&mut self) {
        let question = self.active_question().clone();
        self.active_field_mut().toggle_highlighted(&question);
    }

    pub(super) fn clear_active_answer(&mut self) {
        let question = self.active_question().clone();
        *self.active_field_mut() = QuestionnaireFieldState::empty(&question);
    }

    pub(super) fn active_text_entry_active(&self) -> bool {
        self.active_field()
            .text_entry_active(self.active_question())
    }

    pub(super) fn accepts_paste_burst_char(&self, ch: char) -> bool {
        self.active_text_entry_active() || (ch != ' ' && self.active_question().allow_other)
    }

    pub(super) fn accepts_pending_paste_burst_enter(&self) -> bool {
        self.active_text_entry_active() || self.active_question().allow_other
    }

    pub(super) fn insert_char(&mut self, ch: char) -> bool {
        if !self.active_text_entry_active() && !self.activate_other_for_typing() {
            return false;
        }
        self.active_field_mut().insert_char(ch);
        true
    }

    pub(super) fn insert_text(&mut self, text: &str) -> bool {
        if !self.active_text_entry_active() && !self.activate_other_for_typing() {
            return false;
        }
        self.active_field_mut().insert_text(text);
        true
    }

    pub(super) fn backspace(&mut self) {
        if self.active_text_entry_active() {
            self.active_field_mut().backspace();
        }
    }

    pub(super) fn delete(&mut self) {
        if self.active_text_entry_active() {
            self.active_field_mut().delete();
        }
    }

    pub(super) fn delete_previous_word(&mut self) {
        if !self.active_text_entry_active() {
            return;
        }
        let field = self.active_field_mut();
        let new_cursor = previous_word_boundary(&field.other_value, field.other_cursor);
        let start = field.byte_index(new_cursor);
        let end = field.byte_index(field.other_cursor);
        field.other_value.replace_range(start..end, "");
        field.other_cursor = new_cursor;
    }

    pub(super) fn move_text_cursor_previous_word(&mut self) {
        if self.active_text_entry_active() {
            let field = self.active_field_mut();
            field.other_cursor = previous_word_boundary(&field.other_value, field.other_cursor);
        }
    }

    pub(super) fn move_text_cursor_next_word(&mut self) {
        if self.active_text_entry_active() {
            let field = self.active_field_mut();
            field.other_cursor = next_word_boundary(&field.other_value, field.other_cursor);
        }
    }

    pub(super) fn move_cursor_left(&mut self) {
        if self.active_text_entry_active() {
            self.active_field_mut().other_cursor =
                self.active_field().other_cursor.saturating_sub(1);
        } else {
            self.move_active_choice_previous();
        }
    }

    pub(super) fn move_cursor_right(&mut self) {
        if self.active_text_entry_active() {
            let char_len = self.active_char_len();
            let field = self.active_field_mut();
            field.other_cursor = (field.other_cursor + 1).min(char_len);
        } else {
            self.move_active_choice_next();
        }
    }

    pub(super) fn move_cursor_home(&mut self) {
        if self.active_text_entry_active() {
            self.active_field_mut().other_cursor = 0;
        }
    }

    pub(super) fn move_cursor_end(&mut self) {
        if self.active_text_entry_active() {
            let char_len = self.active_char_len();
            self.active_field_mut().other_cursor = char_len;
        }
    }

    pub(super) fn submit(&mut self) -> Result<SubmittedQuestionnaire, String> {
        let answers = questionnaire_answers(self)?;
        let response = QuestionnaireResponse { answers };
        let display = submitted_questionnaire_entry(&self.request, &response);
        self.response.send_response(response);
        Ok(SubmittedQuestionnaire { display })
    }

    pub(super) fn cancel_by_user(&mut self) {
        self.response
            .cancel(QuestionnaireCancelReason::UserCancelled);
    }

    fn activate_other_for_typing(&mut self) -> bool {
        let question = self.active_question().clone();
        if !question.allow_other {
            return false;
        }
        match &mut self.active_field_mut().selection {
            FieldSelection::Text | FieldSelection::Other => true,
            FieldSelection::None | FieldSelection::Single(_) => {
                let field = self.active_field_mut();
                field.selection = FieldSelection::Other;
                field.choice_cursor = question.choices.len();
                true
            }
            FieldSelection::Multi { .. } => {
                let field = self.active_field_mut();
                if let FieldSelection::Multi { other, .. } = &mut field.selection {
                    *other = true;
                }
                field.choice_cursor = question.choices.len();
                true
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SubmittedQuestionnaire {
    pub(super) display: String,
}

impl QuestionnaireFieldState {
    fn empty(question: &QuestionnaireQuestion) -> Self {
        let selection = match question.kind {
            QuestionnaireQuestionKind::Text => FieldSelection::Text,
            QuestionnaireQuestionKind::Choice => FieldSelection::None,
            QuestionnaireQuestionKind::MultiSelect => FieldSelection::Multi {
                selected: Vec::new(),
                other: false,
            },
            QuestionnaireQuestionKind::Confirm => FieldSelection::None,
        };
        Self {
            selection,
            choice_cursor: 0,
            other_value: String::new(),
            other_cursor: 0,
        }
    }

    fn new(question: &QuestionnaireQuestion) -> Self {
        let (selection, choice_cursor, other_value) = match question.kind {
            QuestionnaireQuestionKind::Text => (
                FieldSelection::Text,
                0,
                question
                    .default
                    .as_ref()
                    .map(questionnaire_default_string)
                    .unwrap_or_default(),
            ),
            QuestionnaireQuestionKind::Choice => default_choice_selection(question),
            QuestionnaireQuestionKind::MultiSelect => default_multi_selection(question),
            QuestionnaireQuestionKind::Confirm => default_confirm_selection(question),
        };
        let other_cursor = other_value.chars().count();
        Self {
            selection,
            choice_cursor,
            other_value,
            other_cursor,
        }
    }

    fn char_len(&self) -> usize {
        self.other_value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.other_value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.other_value.len())
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        let byte_index = self.byte_index(self.other_cursor);
        self.other_value.insert(byte_index, ch);
        self.other_cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        let byte_index = self.byte_index(self.other_cursor);
        self.other_value.insert_str(byte_index, text);
        self.other_cursor += text.chars().count();
    }

    pub(super) fn backspace(&mut self) {
        if self.other_cursor == 0 {
            return;
        }
        let start = self.byte_index(self.other_cursor - 1);
        let end = self.byte_index(self.other_cursor);
        self.other_value.replace_range(start..end, "");
        self.other_cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.other_cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.other_cursor);
        let end = self.byte_index(self.other_cursor + 1);
        self.other_value.replace_range(start..end, "");
    }

    fn text_entry_active(&self, question: &QuestionnaireQuestion) -> bool {
        match &self.selection {
            FieldSelection::Text | FieldSelection::Other => true,
            FieldSelection::None | FieldSelection::Single(_) => false,
            FieldSelection::Multi { other, .. } => {
                *other && self.choice_cursor == question.choices.len()
            }
        }
    }

    fn move_choice_previous(&mut self, question: &QuestionnaireQuestion) {
        let count = choice_count(question);
        if count == 0 || matches!(self.selection, FieldSelection::Text) {
            return;
        }
        self.choice_cursor = self.choice_cursor.saturating_sub(1);
        self.select_highlighted_for_single(question);
    }

    fn move_choice_next(&mut self, question: &QuestionnaireQuestion) {
        let count = choice_count(question);
        if count == 0 || matches!(self.selection, FieldSelection::Text) {
            return;
        }
        self.choice_cursor = (self.choice_cursor + 1).min(count.saturating_sub(1));
        self.select_highlighted_for_single(question);
    }

    fn toggle_highlighted(&mut self, question: &QuestionnaireQuestion) {
        match &mut self.selection {
            FieldSelection::Text => {}
            FieldSelection::None | FieldSelection::Single(_) => {
                if question.allow_other && self.choice_cursor == question.choices.len() {
                    self.selection = FieldSelection::Other;
                } else {
                    self.selection = FieldSelection::Single(
                        self.choice_cursor
                            .min(choice_count(question).saturating_sub(1)),
                    );
                }
            }
            FieldSelection::Multi { selected, other } => {
                if question.allow_other && self.choice_cursor == question.choices.len() {
                    *other = !*other;
                } else {
                    let cursor = self
                        .choice_cursor
                        .min(question.choices.len().saturating_sub(1));
                    if selected.contains(&cursor) {
                        selected.retain(|index| *index != cursor);
                    } else {
                        selected.push(cursor);
                        selected.sort_unstable();
                        selected.dedup();
                    }
                }
            }
            FieldSelection::Other => {
                if self.choice_cursor < question.choices.len() {
                    self.selection = FieldSelection::Single(self.choice_cursor);
                }
            }
        }
    }

    fn select_highlighted_for_single(&mut self, question: &QuestionnaireQuestion) {
        match &mut self.selection {
            FieldSelection::None | FieldSelection::Single(_) => {
                if question.allow_other && self.choice_cursor == question.choices.len() {
                    self.selection = FieldSelection::Other;
                } else {
                    self.selection = FieldSelection::Single(
                        self.choice_cursor
                            .min(choice_count(question).saturating_sub(1)),
                    );
                }
            }
            FieldSelection::Other if self.choice_cursor < question.choices.len() => {
                self.selection = FieldSelection::Single(self.choice_cursor);
            }
            FieldSelection::Text | FieldSelection::Multi { .. } | FieldSelection::Other => {}
        }
    }
}

fn default_choice_selection(question: &QuestionnaireQuestion) -> (FieldSelection, usize, String) {
    let Some(default) = question.default.as_ref().map(questionnaire_default_string) else {
        return (FieldSelection::None, 0, String::new());
    };
    if let Some(index) = question
        .choices
        .iter()
        .position(|choice| choice.eq_ignore_ascii_case(&default))
    {
        return (FieldSelection::Single(index), index, String::new());
    }
    if question.allow_other {
        return (FieldSelection::Other, question.choices.len(), default);
    }
    (FieldSelection::None, 0, String::new())
}

fn default_multi_selection(question: &QuestionnaireQuestion) -> (FieldSelection, usize, String) {
    let mut selected = Vec::new();
    let mut other_values = Vec::new();
    for value in question
        .default
        .as_ref()
        .map(questionnaire_default_strings)
        .unwrap_or_default()
    {
        if let Some(index) = question
            .choices
            .iter()
            .position(|choice| choice.eq_ignore_ascii_case(&value))
        {
            selected.push(index);
        } else if question.allow_other {
            other_values.push(value);
        }
    }
    selected.sort_unstable();
    selected.dedup();
    let other = !other_values.is_empty();
    let choice_cursor = selected
        .first()
        .copied()
        .or_else(|| other.then_some(question.choices.len()))
        .unwrap_or(0);
    (
        FieldSelection::Multi { selected, other },
        choice_cursor,
        other_values.join(", "),
    )
}

fn default_confirm_selection(question: &QuestionnaireQuestion) -> (FieldSelection, usize, String) {
    match question
        .default
        .as_ref()
        .map(questionnaire_default_string)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "yes" | "y" | "true" => (FieldSelection::Single(0), 0, String::new()),
        "no" | "n" | "false" => (FieldSelection::Single(1), 1, String::new()),
        _ => (FieldSelection::None, 0, String::new()),
    }
}

fn questionnaire_default_strings(default: &serde_json::Value) -> Vec<String> {
    match default {
        serde_json::Value::Array(values) => values
            .iter()
            .map(questionnaire_default_string)
            .filter(|value| !value.is_empty())
            .collect(),
        value => {
            let value = questionnaire_default_string(value);
            if value.is_empty() {
                Vec::new()
            } else {
                vec![value]
            }
        }
    }
}

fn questionnaire_default_string(default: &serde_json::Value) -> String {
    match default {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => default.to_string(),
    }
}

fn questionnaire_default_display(default: &serde_json::Value) -> String {
    match default {
        serde_json::Value::Array(values) => values
            .iter()
            .map(questionnaire_default_display)
            .collect::<Vec<_>>()
            .join(", "),
        value => questionnaire_default_string(value),
    }
}

fn choice_count(question: &QuestionnaireQuestion) -> usize {
    match question.kind {
        QuestionnaireQuestionKind::Text => 0,
        QuestionnaireQuestionKind::Confirm => 2,
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect => {
            question.choices.len() + usize::from(question.allow_other)
        }
    }
}

pub(super) fn questionnaire_lines(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> Vec<Line<'static>> {
    questionnaire_frame(questionnaire, width).0
}

fn questionnaire_frame(
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

pub(super) fn questionnaire_cursor_position(
    questionnaire: &QuestionnaireComposer,
    width: usize,
) -> Position {
    questionnaire_frame(questionnaire, width).1
}

fn questionnaire_question_cursor(
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

pub(super) fn questionnaire_answers(
    questionnaire: &QuestionnaireComposer,
) -> Result<Vec<QuestionnaireAnswer>, String> {
    questionnaire
        .request
        .questions
        .iter()
        .zip(questionnaire.fields.iter())
        .enumerate()
        .map(|(index, (question, field))| {
            let answer = normalize_questionnaire_answer(question, field)
                .map_err(|error| format!("question {}: {error}", index + 1))?;
            Ok(QuestionnaireAnswer {
                id: question.id.clone(),
                answer,
            })
        })
        .collect()
}

fn normalize_questionnaire_answer(
    question: &QuestionnaireQuestion,
    field: &QuestionnaireFieldState,
) -> Result<serde_json::Value, String> {
    match question.kind {
        QuestionnaireQuestionKind::Text => {
            normalize_text_answer(question, &field.other_value).map(serde_json::Value::String)
        }
        QuestionnaireQuestionKind::Choice => match &field.selection {
            FieldSelection::Single(index) => question
                .choices
                .get(*index)
                .cloned()
                .map(serde_json::Value::String)
                .ok_or_else(|| "answer is not selected".into()),
            FieldSelection::Other => {
                normalize_text_answer(question, &field.other_value).map(serde_json::Value::String)
            }
            FieldSelection::None if !question.required => Ok(serde_json::Value::Null),
            FieldSelection::Text | FieldSelection::None | FieldSelection::Multi { .. } => {
                Err("answer is not selected".into())
            }
        },
        QuestionnaireQuestionKind::MultiSelect => match &field.selection {
            FieldSelection::Multi { selected, other } => {
                let mut answers = selected
                    .iter()
                    .filter_map(|index| question.choices.get(*index).cloned())
                    .collect::<Vec<_>>();
                if *other {
                    let other_answer = normalize_text_answer(question, &field.other_value)?;
                    if !other_answer.is_empty() {
                        answers.push(other_answer);
                    }
                }
                if answers.is_empty() && question.required {
                    return Err("select at least one answer".into());
                }
                Ok(serde_json::Value::Array(
                    answers.into_iter().map(serde_json::Value::String).collect(),
                ))
            }
            FieldSelection::Text
            | FieldSelection::None
            | FieldSelection::Single(_)
            | FieldSelection::Other => Err("answer is not selected".into()),
        },
        QuestionnaireQuestionKind::Confirm => match field.selection {
            FieldSelection::Single(0) => Ok(serde_json::json!("yes")),
            FieldSelection::Single(1) => Ok(serde_json::json!("no")),
            FieldSelection::None if !question.required => Ok(serde_json::Value::Null),
            FieldSelection::Text
            | FieldSelection::None
            | FieldSelection::Single(_)
            | FieldSelection::Multi { .. }
            | FieldSelection::Other => Err("answer is not selected".into()),
        },
    }
}

fn normalize_text_answer(question: &QuestionnaireQuestion, value: &str) -> Result<String, String> {
    let answer = value.trim().to_string();
    if answer.is_empty() && question.required {
        Err("answer cannot be empty".into())
    } else {
        Ok(answer)
    }
}

pub(super) fn submitted_questionnaire_entry(
    request: &QuestionnaireRequest,
    response: &QuestionnaireResponse,
) -> String {
    if response.answers.len() == 1 {
        return questionnaire_answer_display(&response.answers[0].answer);
    }
    let mut lines = Vec::new();
    if let Some(title) = &request.title {
        lines.push(title.clone());
    } else {
        lines.push("questionnaire answers".into());
    }
    for answer in &response.answers {
        let label = request
            .questions
            .iter()
            .find(|question| question.id == answer.id)
            .map(|question| question.question.as_str())
            .unwrap_or(answer.id.as_str());
        lines.push(format!(
            "{label}: {}",
            questionnaire_answer_display(&answer.answer)
        ));
    }
    lines.join("\n")
}

fn questionnaire_answer_display(answer: &serde_json::Value) -> String {
    match answer {
        serde_json::Value::Array(values) => values
            .iter()
            .map(questionnaire_answer_display)
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(_) => answer.to_string(),
    }
}

pub(super) fn questionnaire_notice_text(request: &QuestionnaireRequest) -> String {
    match (&request.title, request.questions.as_slice()) {
        (Some(title), _) => format!("agent asks: {title}"),
        (None, [question]) => format!("agent asks: {}", question.question),
        (None, questions) => format!("agent asks {} questions", questions.len()),
    }
}

fn previous_word_boundary(input: &str, cursor: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut index = cursor.min(chars.len());
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}

fn next_word_boundary(input: &str, cursor: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut index = cursor.min(chars.len());
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    while index < chars.len() && !chars[index].is_whitespace() {
        index += 1;
    }
    index
}

#[cfg(test)]
#[path = "questionnaire_tests.rs"]
mod tests;
