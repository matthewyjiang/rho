//! Rho host input requests as ACP form elicitation (`elicitation/create`).
//!
//! Each question becomes one property keyed by its ID. On hosts that draw
//! the standard schema, a choice question that also accepts other answers
//! gets a free-text companion, single selects carry titled `oneOf` options,
//! and multiple selects are string arrays. T3 Code reads only a top-level
//! `enum` and lets every question take free text, so it gets one string
//! property per question instead (see [`FormDialect::T3Code`]). The
//! response's `accept`, `decline`, and `cancel` become answers,
//! [`HostInputRefusal::Declined`], and [`HostInputRefusal::Dismissed`].

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::{
    BooleanPropertySchema, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EnumOption, MultiSelectPropertySchema, SessionId, StringPropertySchema, ToolCallId,
};
use rho_sdk::{
    DefaultSelection, HostChoice, HostInputRefusal, HostInputRequest, HostInputResponse,
    HostQuestion, SelectionMode,
};

use super::host_updates::FormDialect;

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
    Select(SelectionMode),
    /// A select plus a free-text answer of the user's own.
    SelectOrOther(SelectionMode),
}

fn field(question: &HostQuestion) -> Field {
    let selection = question.selection();
    match question.choices() {
        [only] if question.permits_other() && only.value() == "other" => Field::Text,
        [yes, no]
            if selection == SelectionMode::One
                && !question.permits_other()
                && yes.value().eq_ignore_ascii_case("yes")
                && no.value().eq_ignore_ascii_case("no") =>
        {
            Field::Confirm
        }
        _ if question.permits_other() => Field::SelectOrOther(selection),
        _ => Field::Select(selection),
    }
}

/// The key of `question`'s free-text companion, unique among the request's
/// question IDs (MCP property names may contain `.`).
fn other_key(request: &HostInputRequest, question: &HostQuestion) -> String {
    let mut key = format!("{}{OTHER_SUFFIX}", question.id());
    while request.questions().iter().any(|other| other.id() == key) {
        key.push('_');
    }
    key
}

/// The form for `request`. `call_id` ties it to the tool call that asked,
/// so the host can draw it on that call's card.
pub(super) fn elicitation_request(
    request: &HostInputRequest,
    session_id: &SessionId,
    call_id: Option<&rho_sdk::ToolCallId>,
    dialect: FormDialect,
) -> CreateElicitationRequest {
    let mut schema = ElicitationSchema::new();
    for question in request.questions() {
        schema = match dialect {
            FormDialect::Standard => add_question(schema, request, question),
            FormDialect::T3Code => add_t3_question(schema, question),
        };
    }
    let scope = ElicitationSessionScope::new(session_id.clone())
        .tool_call_id(call_id.map(|id| ToolCallId::new(id.as_str())));
    let message = match request.title() {
        "" => "Rho needs your input",
        title => title,
    };
    CreateElicitationRequest::new(ElicitationFormMode::new(scope, schema), message)
}

/// A property's title and description. T3 Code shows the title as the
/// question's label and the description as its text, so the full prompt
/// goes in the description when a short header takes the title.
fn title_and_description(question: &HostQuestion) -> (String, Option<String>) {
    match question.header_text() {
        Some(header) => (
            header.to_owned(),
            Some(match question.help_text() {
                Some(help) => format!("{}\n\n{help}", question.prompt()),
                None => question.prompt().to_owned(),
            }),
        ),
        None => (
            question.prompt().to_owned(),
            question.help_text().map(str::to_owned),
        ),
    }
}

fn checkbox(
    question: &HostQuestion,
    title: String,
    description: Option<String>,
    defaults: &[String],
) -> BooleanPropertySchema {
    let yes = question.choices()[0].value();
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
    request: &HostInputRequest,
    question: &HostQuestion,
) -> ElicitationSchema {
    let id = question.id();
    let (title, description) = title_and_description(question);
    let defaults = selected_defaults(question);
    let required = question.is_required();
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
                .partition(|value| question.choices().iter().any(|c| c.value() == value));
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
                .property(other_key(request, question), other, false)
        }
    }
}

/// T3 Code's shape: one string property per question whose `enum` lists
/// the choice labels it shows. Its free text replaces the `.other`
/// companion, and a multiple choice is typed as a comma-separated list.
fn add_t3_question(schema: ElicitationSchema, question: &HostQuestion) -> ElicitationSchema {
    let id = question.id();
    let (title, description) = title_and_description(question);
    let required = question.is_required();
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
    let hint = (selection == SelectionMode::Many).then(|| {
        format!(
            "Pick one or more; separate several with {}.",
            Separator::for_question(question).name()
        )
    });
    let description = match (description, hint) {
        (Some(text), Some(hint)) => Some(format!("{text}\n\n{hint}")),
        (text, hint) => text.or(hint),
    };
    let labels = question
        .choices()
        .iter()
        .map(|choice| choice.label().to_owned())
        .collect::<Vec<_>>();
    let select = StringPropertySchema::new()
        .title(title)
        .description(description)
        .enum_values(labels);
    schema.property(id, select, required)
}

fn select(
    question: &HostQuestion,
    selection: SelectionMode,
    title: String,
    description: Option<String>,
    defaults: Vec<String>,
) -> agent_client_protocol::schema::v1::ElicitationPropertySchema {
    let options = question.choices().iter().map(option).collect::<Vec<_>>();
    if selection == SelectionMode::Many {
        return MultiSelectPropertySchema::titled(options)
            .title(title)
            .description(description)
            .default_value((!defaults.is_empty()).then_some(defaults))
            .into();
    }
    let values = question
        .choices()
        .iter()
        .map(|choice| choice.value().to_owned())
        .collect::<Vec<_>>();
    StringPropertySchema::new()
        .title(title)
        .description(description)
        .enum_values(values)
        .one_of(options)
        .default_value(defaults.into_iter().next())
        .into()
}

fn option(choice: &HostChoice) -> EnumOption {
    EnumOption::new(choice.value(), choice.label())
        .description(choice.description_text().map(str::to_owned))
}

/// Defaults the question pre-selects. A focused default only highlights a
/// choice, which a form cannot express, so it is left unset.
fn selected_defaults(question: &HostQuestion) -> Vec<String> {
    if question.default_selection_mode() != DefaultSelection::Selected {
        return Vec::new();
    }
    let values = match question.default_value_ref() {
        Some(serde_json::Value::Array(values)) => values.iter().collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|value| match value {
            serde_json::Value::String(value) if !value.is_empty() => Some(value.clone()),
            serde_json::Value::Bool(value) => Some(value.to_string()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        // A default that names a choice by its label still means that choice.
        .map(|value| {
            question
                .choices()
                .iter()
                .find(|choice| choice.label().eq_ignore_ascii_case(&value))
                .map_or(value, |choice| choice.value().to_owned())
        })
        .collect()
}

/// The run's answer to `request` from the host's response. Content that does
/// not answer the request counts as declined: the user submitted something,
/// but not an answer Rho can use.
pub(super) fn host_answer(
    request: &HostInputRequest,
    response: CreateElicitationResponse,
    dialect: FormDialect,
) -> Result<HostInputResponse, HostInputRefusal> {
    let content = match response.action {
        ElicitationAction::Accept(accept) => accept.content.unwrap_or_default(),
        ElicitationAction::Cancel => return Err(HostInputRefusal::Dismissed),
        ElicitationAction::Decline => return Err(HostInputRefusal::Declined),
        // Unknown actions are not answers; treat them like a decline.
        _ => return Err(HostInputRefusal::Declined),
    };
    let answers = answers(request, &content, dialect);
    match request.validate(&answers) {
        Ok(()) => Ok(answers),
        Err(error) => {
            tracing::warn!(%error, "ACP host accepted a form without a valid answer");
            Err(HostInputRefusal::Declined)
        }
    }
}

fn answers(
    request: &HostInputRequest,
    content: &BTreeMap<String, ElicitationContentValue>,
    dialect: FormDialect,
) -> HostInputResponse {
    let mut response = HostInputResponse::new();
    for question in request.questions() {
        let id = question.id();
        let values = content.get(id).map(strings).unwrap_or_default();
        let values = match (dialect, field(question)) {
            (_, Field::Text) => values,
            // A checkbox answers with a boolean, or with its text when the
            // host draws it as a true/false choice (T3 Code).
            (_, Field::Confirm) => {
                let [yes, no] = question.choices() else {
                    unreachable!("a confirm question has exactly two choices")
                };
                values
                    .into_iter()
                    .map(|value| match value.to_ascii_lowercase().as_str() {
                        "true" => yes.value().to_owned(),
                        "false" => no.value().to_owned(),
                        _ => choice_value(question, value),
                    })
                    .collect()
            }
            (FormDialect::Standard, Field::Select(_)) => values,
            // A typed answer of the user's own replaces a single choice the
            // host could not clear, and joins a multiple choice.
            (FormDialect::Standard, Field::SelectOrOther(selection)) => {
                let other = content
                    .get(&other_key(request, question))
                    .map(strings)
                    .unwrap_or_default();
                match selection {
                    SelectionMode::Many => dedup(values.into_iter().chain(other)),
                    _ if !other.is_empty() => other,
                    _ => values,
                }
            }
            (FormDialect::T3Code, Field::Select(selection) | Field::SelectOrOther(selection)) => {
                t3_choices(question, selection, values)
            }
        };
        if !values.is_empty() {
            response = response.answer(id, values);
        }
    }
    response
}

/// What a T3 Code multiple choice is typed with: the first of these that
/// no choice label contains, so a typed list splits back into exactly the
/// choices the user listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Separator {
    Comma,
    Semicolon,
    Bar,
    NewLine,
}

impl Separator {
    fn for_question(question: &HostQuestion) -> Self {
        [Self::Comma, Self::Semicolon, Self::Bar]
            .into_iter()
            .find(|separator| {
                question
                    .choices()
                    .iter()
                    .all(|choice| !choice.label().contains(separator.char()))
            })
            .unwrap_or(Self::NewLine)
    }

    fn char(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Semicolon => ';',
            Self::Bar => '|',
            Self::NewLine => '\n',
        }
    }

    /// The separator as the form's hint names it.
    fn name(self) -> &'static str {
        match self {
            Self::Comma => "commas",
            Self::Semicolon => "semicolons",
            Self::Bar => "\"|\"",
            Self::NewLine => "new lines",
        }
    }
}

/// T3 Code answers with a chosen label or typed text; a multiple choice
/// types several, split by the question's [`Separator`].
fn t3_choices(
    question: &HostQuestion,
    selection: SelectionMode,
    values: Vec<String>,
) -> Vec<String> {
    let separator = Separator::for_question(question).char();
    let pieces = values.into_iter().flat_map(|value| match selection {
        SelectionMode::Many => value
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
fn find_choice<'a>(question: &'a HostQuestion, text: &str) -> Option<&'a HostChoice> {
    let choices = question.choices();
    choices
        .iter()
        .find(|choice| choice.label() == text)
        .or_else(|| choices.iter().find(|choice| choice.value() == text))
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.label().eq_ignore_ascii_case(text))
        })
}

/// The value of the choice `text` names by value or label, or `text` itself
/// as the user's own answer.
fn choice_value(question: &HostQuestion, text: String) -> String {
    find_choice(question, &text).map_or(text, |choice| choice.value().to_owned())
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
