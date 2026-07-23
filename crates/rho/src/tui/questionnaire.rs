use tokio::sync::oneshot;

mod render;

#[cfg(test)]
use render::questionnaire_frame;
pub(in crate::tui) use render::{questionnaire_cursor_position, questionnaire_lines};

use crate::questionnaire::{QuestionnaireAnswer, QuestionnaireQuestionKind, QuestionnaireResponse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QuestionnaireRequest {
    pub(super) title: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) questions: Vec<QuestionnaireQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QuestionnaireQuestion {
    pub(super) id: String,
    pub(super) question: String,
    pub(super) header: Option<String>,
    pub(super) help: Option<String>,
    pub(super) default: Option<serde_json::Value>,
    pub(super) kind: QuestionnaireQuestionKind,
    pub(super) required: bool,
    pub(super) choices: Vec<QuestionnaireChoice>,
    pub(super) allow_other: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QuestionnaireChoice {
    value: String,
    label: String,
    description: Option<String>,
}

impl QuestionnaireChoice {
    pub(super) fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }

    pub(super) fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    fn same(value: impl Into<String>) -> Self {
        let value = value.into();
        Self::new(value.clone(), value)
    }

    pub(super) fn value(&self) -> &str {
        &self.value
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn matches_default(&self, default: &str) -> bool {
        self.value.eq_ignore_ascii_case(default) || self.label.eq_ignore_ascii_case(default)
    }
}

impl From<String> for QuestionnaireChoice {
    fn from(value: String) -> Self {
        Self::same(value)
    }
}

impl From<&str> for QuestionnaireChoice {
    fn from(value: &str) -> Self {
        Self::same(value)
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuestionnaireEnterAction {
    Advance,
    Submit,
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

    /// Move up within the active question's choices, flowing to the previous
    /// question once the cursor is already on the first choice.
    pub(super) fn move_up(&mut self) {
        if self.active_choice_navigable() && self.active_field().choice_cursor > 0 {
            self.move_active_choice_previous();
        } else {
            self.move_to_previous_field();
        }
    }

    /// Move down within the active question's choices, flowing to the next
    /// question once the cursor is already on the last choice.
    pub(super) fn move_down(&mut self) {
        let count = choice_count(self.active_question());
        if self.active_choice_navigable() && self.active_field().choice_cursor + 1 < count {
            self.move_active_choice_next();
        } else {
            self.move_to_next_field();
        }
    }

    fn active_choice_navigable(&self) -> bool {
        !matches!(self.active_field().selection, FieldSelection::Text)
            && choice_count(self.active_question()) > 0
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

    pub(super) fn on_last_question(&self) -> bool {
        self.active_index + 1 >= self.fields.len()
    }

    pub(super) fn confirm_active_question(&mut self) -> QuestionnaireEnterAction {
        let question = self.active_question().clone();
        if matches!(
            question.kind,
            QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::Confirm
        ) {
            self.active_field_mut().toggle_highlighted(&question);
        }
        if self.on_last_question() {
            QuestionnaireEnterAction::Submit
        } else {
            self.move_to_next_field();
            QuestionnaireEnterAction::Advance
        }
    }

    pub(super) fn submit(&mut self) -> Result<SubmittedQuestionnaire, String> {
        let answers = match questionnaire_answers(self) {
            Ok(answers) => answers,
            Err((index, error)) => {
                // Jump to the offending question so the user sees what the
                // status message refers to.
                self.active_index = index;
                return Err(format!("question {}: {error}", index + 1));
            }
        };
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
        .position(|choice| choice.matches_default(&default))
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
            .position(|choice| choice.matches_default(&value))
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

fn choice_count(question: &QuestionnaireQuestion) -> usize {
    match question.kind {
        QuestionnaireQuestionKind::Text => 0,
        QuestionnaireQuestionKind::Confirm => 2,
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect => {
            question.choices.len() + usize::from(question.allow_other)
        }
    }
}

pub(super) fn questionnaire_answers(
    questionnaire: &QuestionnaireComposer,
) -> Result<Vec<QuestionnaireAnswer>, (usize, String)> {
    questionnaire
        .request
        .questions
        .iter()
        .zip(questionnaire.fields.iter())
        .enumerate()
        .map(|(index, (question, field))| {
            let answer =
                normalize_questionnaire_answer(question, field).map_err(|error| (index, error))?;
            Ok((question, answer))
        })
        .filter_map(|result| match result {
            Ok((question, answer)) if !question.required && answer_is_empty(&answer) => None,
            Ok((question, answer)) => Some(Ok(QuestionnaireAnswer {
                id: question.id.clone(),
                answer,
            })),
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn answer_is_empty(answer: &serde_json::Value) -> bool {
    match answer {
        serde_json::Value::Null => true,
        serde_json::Value::String(value) => value.trim().is_empty(),
        serde_json::Value::Array(values) => values.is_empty(),
        serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Object(_) => false,
    }
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
                .map(|choice| serde_json::Value::String(choice.value().to_string()))
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
                    .filter_map(|index| question.choices.get(*index))
                    .map(|choice| choice.value().to_string())
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
            FieldSelection::Single(index @ 0..=1) => Ok(serde_json::Value::String(
                question
                    .choices
                    .get(index)
                    .map_or(if index == 0 { "yes" } else { "no" }, |choice| {
                        choice.value()
                    })
                    .to_string(),
            )),
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
    if let [answer] = response.answers.as_slice() {
        let question = request
            .questions
            .iter()
            .find(|question| question.id == answer.id);
        return questionnaire_answer_display(question, &answer.answer);
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
        let question = request
            .questions
            .iter()
            .find(|question| question.id == answer.id);
        lines.push(format!(
            "{label}: {}",
            questionnaire_answer_display(question, &answer.answer)
        ));
    }
    lines.join("\n")
}

fn questionnaire_answer_display(
    question: Option<&QuestionnaireQuestion>,
    answer: &serde_json::Value,
) -> String {
    match answer {
        serde_json::Value::Array(values) => values
            .iter()
            .map(|answer| questionnaire_answer_display(question, answer))
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::String(value) => question
            .and_then(|question| {
                question
                    .choices
                    .iter()
                    .find(|choice| choice.value() == value)
            })
            .map_or_else(|| value.clone(), |choice| choice.label().to_string()),
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
