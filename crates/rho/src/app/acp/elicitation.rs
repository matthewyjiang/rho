//! Rho host input requests as ACP form elicitation (`elicitation/create`).
//!
//! Each question becomes one property keyed by its ID, plus a free-text
//! companion when a choice question also accepts other answers. Hosts render
//! single selects differently (T3 Code reads only `enum`, Zed prefers titled
//! `oneOf`), so single selects carry both. The response's `accept`,
//! `decline`, and `cancel` become answers, [`HostInputRefusal::Declined`], and
//! [`HostInputRefusal::Dismissed`].

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

/// Suffix of the free-text property beside a choice question that also
/// accepts its own answer.
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
    /// A select plus a free-text property for an answer of the user's own.
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

/// The form for `request`. `call_id` ties it to the tool call that asked,
/// so the host can draw it on that call's card.
pub(super) fn elicitation_request(
    request: &HostInputRequest,
    session_id: &SessionId,
    call_id: Option<&rho_sdk::ToolCallId>,
) -> CreateElicitationRequest {
    let mut schema = ElicitationSchema::new();
    for question in request.questions() {
        schema = add_question(schema, question);
    }
    let scope = ElicitationSessionScope::new(session_id.clone())
        .tool_call_id(call_id.map(|id| ToolCallId::new(id.as_str())));
    let message = match request.title() {
        "" => "Rho needs your input",
        title => title,
    };
    CreateElicitationRequest::new(ElicitationFormMode::new(scope, schema), message)
}

fn add_question(schema: ElicitationSchema, question: &HostQuestion) -> ElicitationSchema {
    let id = question.id();
    // T3 Code shows the title as the question's label and the description
    // as its text, so the full prompt goes in the description when a short
    // header takes the title.
    let (title, description) = match question.header_text() {
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
    };
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
        Field::Confirm => {
            let yes = question.choices()[0].value();
            let checked = defaults
                .first()
                .map(|value| value.eq_ignore_ascii_case(yes) || value == "true");
            let checkbox = BooleanPropertySchema::new()
                .title(title)
                .description(description)
                .default_value(checked);
            schema.property(id, checkbox, required)
        }
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
                .property(format!("{id}{OTHER_SUFFIX}"), other, false)
        }
    }
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
) -> Result<HostInputResponse, HostInputRefusal> {
    let content = match response.action {
        ElicitationAction::Accept(accept) => accept.content.unwrap_or_default(),
        ElicitationAction::Cancel => return Err(HostInputRefusal::Dismissed),
        ElicitationAction::Decline => return Err(HostInputRefusal::Declined),
        // Unknown actions are not answers; treat them like a decline.
        _ => return Err(HostInputRefusal::Declined),
    };
    let answers = answers(request, &content);
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
) -> HostInputResponse {
    let mut response = HostInputResponse::new();
    for question in request.questions() {
        let id = question.id();
        let mut values = content.get(id).map(strings).unwrap_or_default();
        match field(question) {
            // A checkbox answers with a boolean, or with its text when the
            // host draws it as a true/false choice (T3 Code).
            Field::Confirm => {
                let [yes, no] = question.choices() else {
                    unreachable!("a confirm question has exactly two choices")
                };
                for value in &mut values {
                    if value.eq_ignore_ascii_case("true") {
                        yes.value().clone_into(value);
                    } else if value.eq_ignore_ascii_case("false") {
                        no.value().clone_into(value);
                    }
                }
            }
            Field::SelectOrOther(_) => {
                let other = content
                    .get(&format!("{id}{OTHER_SUFFIX}"))
                    .map(strings)
                    .unwrap_or_default();
                values.extend(other);
            }
            Field::Text | Field::Select(_) => {}
        }
        if !values.is_empty() {
            response = response.answer(id, values);
        }
    }
    response
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
