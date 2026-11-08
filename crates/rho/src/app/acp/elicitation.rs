//! A daemon question (`wire::QuestionAsk`) as ACP form elicitation
//! (`elicitation/create`).
//!
//! Each question becomes one property keyed by its ID. On hosts that draw
//! the standard schema, a choice question that also accepts other answers
//! gets a free-text companion, single selects carry titled `oneOf` options,
//! and multiple selects are string arrays. T3 Code reads only a top-level
//! `enum` and lets every question take free text, so it gets one string
//! property per question instead (see [`FormDialect::T3Code`]). The
//! response's `accept`, `decline`, and `cancel` become
//! [`InputAnswer::Answers`], [`InputAnswer::Decline`], and
//! [`InputAnswer::Dismiss`]. The daemon checks the answers against the
//! questions.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::{
    BooleanPropertySchema, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EnumOption, MultiSelectPropertySchema, SessionId, StringPropertySchema, ToolCallId,
};
use rho_protocol::{
    self as wire, Choice, DefaultStyle, InputAnswer, Question, QuestionAsk, Selection,
};

use super::host_features::FormDialect;

/// Suffix of the free-text property beside a choice question that also
/// accepts its own answer, on hosts that draw the standard schema.
const OTHER_SUFFIX: &str = ".other";

/// The form control a question maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// Only an "other" answer: the questionnaire tool's and MCP's encoding
    /// of a text field.
    Text,
    /// A yes/no single select, drawn as a checkbox.
    Confirm,
    Select(Selection),
    /// A select plus a free-text answer of the user's own.
    SelectOrOther(Selection),
}

fn field(question: &Question) -> Field {
    let selection = question.selection;
    match question.choices.as_slice() {
        [only] if question.allow_other && only.value == "other" => Field::Text,
        [yes, no]
            if selection == Selection::One
                && !question.allow_other
                && yes.value.eq_ignore_ascii_case("yes")
                && no.value.eq_ignore_ascii_case("no") =>
        {
            Field::Confirm
        }
        _ if question.allow_other => Field::SelectOrOther(selection),
        _ => Field::Select(selection),
    }
}

/// The key of `question`'s free-text companion, unique among the request's
/// question IDs (MCP property names may contain `.`).
fn other_key(ask: &QuestionAsk, question: &Question) -> String {
    let mut key = format!("{}{OTHER_SUFFIX}", question.id);
    while ask.questions.iter().any(|other| other.id == key) {
        key.push('_');
    }
    key
}

/// The form for `ask`. `call_id` ties it to the tool call that asked, so
/// the host can draw it on that call's card.
pub(super) fn elicitation_request(
    ask: &QuestionAsk,
    session_id: &SessionId,
    call_id: Option<&wire::ToolCallId>,
    dialect: FormDialect,
) -> CreateElicitationRequest {
    let scope = ElicitationSessionScope::new(session_id.clone())
        .tool_call_id(call_id.map(|id| ToolCallId::new(id.as_str())));
    let mut schema = ElicitationSchema::new();
    for question in &ask.questions {
        schema = match dialect {
            FormDialect::Standard => add_question(schema, ask, question),
            FormDialect::T3Code => add_t3_question(schema, question),
        };
    }
    let message = match ask.title.as_str() {
        "" => "Rho needs your input",
        title => title,
    };
    CreateElicitationRequest::new(ElicitationFormMode::new(scope, schema), message)
}

/// A property's title and description. T3 Code shows the title as the
/// question's label and the description as its text, so the full prompt
/// goes in the description when a short header takes the title.
fn title_and_description(question: &Question) -> (String, Option<String>) {
    match &question.header {
        Some(header) => (
            header.clone(),
            Some(match &question.help {
                Some(help) => format!("{}\n\n{help}", question.prompt),
                None => question.prompt.clone(),
            }),
        ),
        None => (question.prompt.clone(), question.help.clone()),
    }
}

fn checkbox(
    question: &Question,
    title: String,
    description: Option<String>,
    defaults: &[String],
) -> BooleanPropertySchema {
    let yes = question.choices[0].value.as_str();
    let checked = defaults
        .first()
        .map(|value| value.eq_ignore_ascii_case(yes) || value == "true");
    BooleanPropertySchema::new()
        .title(title)
        .description(description)
        .default_value(checked)
}

fn add_question(
    schema: ElicitationSchema,
    ask: &QuestionAsk,
    question: &Question,
) -> ElicitationSchema {
    let id = question.id.as_str();
    let (title, description) = title_and_description(question);
    let defaults = selected_defaults(question);
    let required = question.required;
    match field(question) {
        Field::Text => {
            let text = StringPropertySchema::new()
                .title(title)
                .description(description)
                .default_value(defaults.into_iter().next());
            schema.property(id, text, required)
        }
        Field::Confirm => schema.property(
            id,
            checkbox(question, title, description, &defaults),
            required,
        ),
        Field::Select(selection) => schema.property(
            id,
            select(question, selection, title, description, defaults),
            required,
        ),
        // Either property may carry the answer, so neither is required on
        // the form; answer validation enforces the question's own rule.
        Field::SelectOrOther(selection) => {
            let (known, other): (Vec<_>, Vec<_>) = defaults
                .into_iter()
                .partition(|value| question.choices.iter().any(|c| &c.value == value));
            let other = StringPropertySchema::new()
                .title(format!("{title} (other)"))
                .description("Your own answer instead of a listed choice")
                .default_value((!other.is_empty()).then(|| other.join(", ")));
            schema
                .property(
                    id,
                    select(question, selection, title, description, known),
                    false,
                )
                .property(other_key(ask, question), other, false)
        }
    }
}

/// T3 Code's shape: one string property per question whose `enum` lists
/// the choice labels it shows. Its free text replaces the `.other`
/// companion, and a multiple choice is typed as a comma-separated list.
fn add_t3_question(schema: ElicitationSchema, question: &Question) -> ElicitationSchema {
    let id = question.id.as_str();
    let (title, description) = title_and_description(question);
    let required = question.required;
    let selection = match field(question) {
        Field::Text => {
            let text = StringPropertySchema::new()
                .title(title)
                .description(description);
            return schema.property(id, text, required);
        }
        Field::Confirm => {
            return schema.property(id, checkbox(question, title, description, &[]), required);
        }
        Field::Select(selection) | Field::SelectOrOther(selection) => selection,
    };
    let hint = match (selection, Separator::for_question(question)) {
        (Selection::Many, Some(separator)) => Some(format!(
            "Pick one or more; separate several with {}.",
            separator.name()
        )),
        (Selection::Many, None) => Some("Pick one.".to_owned()),
        _ => None,
    };
    let description = match (description, hint) {
        (Some(text), Some(hint)) => Some(format!("{text}\n\n{hint}")),
        (text, hint) => text.or(hint),
    };
    let labels = question
        .choices
        .iter()
        .map(|choice| choice.label.clone())
        .collect::<Vec<_>>();
    let select = StringPropertySchema::new()
        .title(title)
        .description(description)
        .enum_values(labels);
    schema.property(id, select, required)
}

fn select(
    question: &Question,
    selection: Selection,
    title: String,
    description: Option<String>,
    defaults: Vec<String>,
) -> agent_client_protocol::schema::v1::ElicitationPropertySchema {
    let options = question.choices.iter().map(option).collect::<Vec<_>>();
    if selection == Selection::Many {
        return MultiSelectPropertySchema::titled(options)
            .title(title)
            .description(description)
            .default_value((!defaults.is_empty()).then_some(defaults))
            .into();
    }
    let values = question
        .choices
        .iter()
        .map(|choice| choice.value.clone())
        .collect::<Vec<_>>();
    StringPropertySchema::new()
        .title(title)
        .description(description)
        .enum_values(values)
        .one_of(options)
        .default_value(defaults.into_iter().next())
        .into()
}

fn option(choice: &Choice) -> EnumOption {
    EnumOption::new(choice.value.as_str(), choice.label.as_str())
        .description(choice.description.clone())
}

/// Defaults the question pre-selects. A focused default only highlights a
/// choice, which a form cannot express, so it is left unset.
fn selected_defaults(question: &Question) -> Vec<String> {
    let Some(default) = &question.default else {
        return Vec::new();
    };
    match default.style {
        DefaultStyle::Selected => {}
        // Focused, or a style a newer daemon added: nothing to pre-select.
        _ => return Vec::new(),
    }
    default
        .values
        .iter()
        .filter(|value| !value.is_empty())
        // A default names a choice by value; one that names it by label
        // still means that choice.
        .map(|value| {
            question
                .choices
                .iter()
                .find(|choice| &choice.value == value)
                .or_else(|| find_choice(question, value))
                .map_or_else(|| value.clone(), |choice| choice.value.clone())
        })
        .collect()
}

/// The answer to `ask` from the host's response.
pub(super) fn host_answer(
    ask: &QuestionAsk,
    response: CreateElicitationResponse,
    dialect: FormDialect,
) -> InputAnswer {
    let content = match response.action {
        ElicitationAction::Accept(accept) => accept.content.unwrap_or_default(),
        ElicitationAction::Cancel => return InputAnswer::Dismiss,
        ElicitationAction::Decline => return InputAnswer::Decline,
        // Unknown actions are not answers; treat them like a decline.
        _ => return InputAnswer::Decline,
    };
    InputAnswer::Answers {
        answers: answers(ask, &content, dialect),
    }
}

fn answers(
    ask: &QuestionAsk,
    content: &BTreeMap<String, ElicitationContentValue>,
    dialect: FormDialect,
) -> BTreeMap<String, Vec<String>> {
    let mut answers = BTreeMap::new();
    for question in &ask.questions {
        let id = question.id.as_str();
        let values = content.get(id).map(strings).unwrap_or_default();
        let values = match (dialect, field(question)) {
            (_, Field::Text) => values,
            // A checkbox answers with a boolean, or with its text when the
            // host draws it as a true/false choice (T3 Code).
            (_, Field::Confirm) => {
                let [yes, no] = question.choices.as_slice() else {
                    unreachable!("a confirm question has exactly two choices")
                };
                values
                    .into_iter()
                    .map(|value| match value.to_ascii_lowercase().as_str() {
                        "true" => yes.value.clone(),
                        "false" => no.value.clone(),
                        _ => choice_value(question, value),
                    })
                    .collect()
            }
            (FormDialect::Standard, Field::Select(_)) => values,
            // A typed answer of the user's own replaces a single choice the
            // host could not clear, and joins a multiple choice.
            (FormDialect::Standard, Field::SelectOrOther(selection)) => {
                let other = content
                    .get(&other_key(ask, question))
                    .map(strings)
                    .unwrap_or_default();
                match selection {
                    Selection::Many => dedup(values.into_iter().chain(other)),
                    _ if !other.is_empty() => other,
                    _ => values,
                }
            }
            (FormDialect::T3Code, Field::Select(selection) | Field::SelectOrOther(selection)) => {
                t3_choices(question, selection, values)
            }
        };
        if !values.is_empty() {
            answers.insert(id.to_owned(), values);
        }
    }
    answers
}

/// What a T3 Code multiple choice is typed with: a character no choice
/// label contains, so a typed list splits back into exactly the choices
/// the user listed. Common punctuation comes first; past it, the first free
/// visible character from Unicode's arrow-to-symbol blocks. Labels that use
/// every one leave none, and the question then takes a single typed choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Separator(char);

impl Separator {
    fn for_question(question: &Question) -> Option<Self> {
        let labels = question.choices.iter().map(|choice| choice.label.as_str());
        let free = |c: &char| labels.clone().all(|label| !label.contains(*c));
        [',', ';', '|', '/', '+', '•']
            .into_iter()
            .chain('\u{2190}'..='\u{2BFF}')
            .find(free)
            .map(Self)
    }

    /// The separator as the form's hint names it.
    fn name(self) -> String {
        match self.0 {
            ',' => "commas".to_owned(),
            ';' => "semicolons".to_owned(),
            c => format!("\"{c}\""),
        }
    }
}

/// T3 Code answers with a chosen label or typed text; a multiple choice
/// types several, split by the question's [`Separator`] when it has one.
fn t3_choices(
    question: &Question,
    selection: Selection,
    values: Vec<String>,
) -> Vec<String> {
    let separator = Separator::for_question(question);
    let pieces = values
        .into_iter()
        .flat_map(|value| match (selection, separator) {
            (Selection::Many, Some(Separator(separator))) => value
                .split(separator)
                .map(|piece| piece.trim().to_owned())
                .filter(|piece| !piece.is_empty())
                .collect(),
            _ => vec![value],
        });
    dedup(pieces.map(|value| choice_value(question, value)))
}

/// The choice `text` names: its exact displayed label first, since T3 Code
/// answers with labels, then its value, then its label in any case.
fn find_choice<'a>(question: &'a Question, text: &str) -> Option<&'a Choice> {
    let choices = &question.choices;
    choices
        .iter()
        .find(|choice| choice.label == text)
        .or_else(|| choices.iter().find(|choice| choice.value == text))
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.label.eq_ignore_ascii_case(text))
        })
}

/// The value of the choice `text` names by value or label, or `text` itself
/// as the user's own answer.
fn choice_value(question: &Question, text: String) -> String {
    find_choice(question, &text).map_or(text, |choice| choice.value.clone())
}

fn dedup(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

/// A content value as answers; blank text is no answer.
fn strings(value: &ElicitationContentValue) -> Vec<String> {
    let values = match value {
        ElicitationContentValue::String(value) => vec![value.trim().to_owned()],
        ElicitationContentValue::StringArray(values) => values.clone(),
        ElicitationContentValue::Integer(value) => vec![value.to_string()],
        ElicitationContentValue::Number(value) => vec![value.to_string()],
        ElicitationContentValue::Boolean(value) => vec![value.to_string()],
        _ => Vec::new(),
    };
    values
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "elicitation_tests.rs"]
mod tests;
