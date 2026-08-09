//! Translating an MCP elicitation schema into Rho's questionnaire, and back.
//!
//! This translation is lossy in one direction and typed in the other, so the two
//! halves live together.
//!
//! MCP elicitation schemas carry string, number, integer, boolean, and enum
//! fields with constraints such as ranges and lengths. Rho's [`HostQuestion`] is
//! choice-only: every question is a list of values, optionally with free text,
//! and every answer comes back as `Vec<String>`. So a schema field becomes the
//! nearest question Rho can actually show, exactly as the questionnaire tool
//! already does it:
//!
//! - an enum becomes its choices, single or multi select;
//! - a boolean becomes `yes`/`no`, which the interactive host renders as a
//!   confirm field;
//! - a string becomes one throwaway choice plus free text;
//! - a number or integer rides as free text and is parsed back here.
//!
//! Constraints (`minLength`, `minimum`, `maxItems`, `format`) are shown to the
//! user as help text but are **not** enforced, which is why Rho declares
//! `schemaValidation: false`. The typed content this module builds is therefore
//! the right JSON *type* for every field, never a guarantee that the value
//! satisfies the server's constraints.

use rho_sdk::{HostChoice, HostInputResponse, HostQuestion, SelectionMode};
use rmcp::model::{
    BooleanSchema, ElicitationSchema, EnumSchema, IntegerSchema, MultiSelectEnumSchema,
    NumberSchema, PrimitiveSchemaDefinition, SingleSelectEnumSchema, StringSchema,
};

/// A schema Rho could not turn into a form, or an answer it could not turn back
/// into the declared type. Both are declines: a half-built form asks the user
/// the wrong question, and a wrongly typed answer breaks the server's contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FormError {
    reason: String,
}

impl FormError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub(super) fn reason(&self) -> &str {
        &self.reason
    }
}

/// What a field's answers must become on the way back to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    /// Free text, sent back verbatim.
    Text,
    /// Free text parsed into a JSON number.
    Number,
    /// Free text parsed into a JSON integer.
    Integer,
    /// `yes`/`no` parsed into a JSON boolean.
    Boolean,
    /// One choice value, sent back as a string.
    Choice,
    /// Any number of choice values, sent back as an array of strings.
    MultiChoice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FormField {
    /// Schema property name, which is also the question ID.
    name: String,
    kind: FieldKind,
    required: bool,
}

/// One elicitation schema, prepared as questions plus the typing rules needed to
/// read the answers back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ElicitationForm {
    questions: Vec<HostQuestion>,
    fields: Vec<FormField>,
}

impl ElicitationForm {
    pub(super) fn from_schema(schema: &ElicitationSchema) -> Result<Self, FormError> {
        if schema.properties.is_empty() {
            return Err(FormError::new(
                "elicitation schema declares no properties to ask about",
            ));
        }
        let required = schema.required.clone().unwrap_or_default();
        let mut questions = Vec::with_capacity(schema.properties.len());
        let mut fields = Vec::with_capacity(schema.properties.len());
        for (name, property) in &schema.properties {
            let is_required = required.iter().any(|entry| entry == name);
            let (question, kind) = question_for(name, property, is_required)?;
            questions.push(question);
            fields.push(FormField {
                name: name.clone(),
                kind,
                required: is_required,
            });
        }
        Ok(Self { questions, fields })
    }

    pub(super) fn questions(&self) -> &[HostQuestion] {
        &self.questions
    }

    /// Build the server's content object from the user's answers.
    ///
    /// An optional field the user left empty is omitted rather than sent as an
    /// empty string, because the schema means "absent", not "blank". A required
    /// field left empty fails the whole form.
    pub(super) fn content(
        &self,
        response: &HostInputResponse,
    ) -> Result<serde_json::Value, FormError> {
        let mut content = serde_json::Map::with_capacity(self.fields.len());
        for field in &self.fields {
            let answers = response
                .answers()
                .get(&field.name)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if let Some(value) = field_value(field, answers)? {
                content.insert(field.name.clone(), value);
            }
        }
        Ok(serde_json::Value::Object(content))
    }
}

fn field_value(
    field: &FormField,
    answers: &[String],
) -> Result<Option<serde_json::Value>, FormError> {
    if let FieldKind::MultiChoice = field.kind {
        if answers.is_empty() {
            return missing(field);
        }
        return Ok(Some(serde_json::Value::Array(
            answers
                .iter()
                .map(|answer| serde_json::Value::String(answer.clone()))
                .collect(),
        )));
    }
    let Some(answer) = answers
        .first()
        .map(String::as_str)
        .filter(|answer| !answer.is_empty())
    else {
        return missing(field);
    };
    let value = match field.kind {
        FieldKind::Text | FieldKind::Choice => serde_json::Value::String(answer.to_owned()),
        FieldKind::Number => {
            let parsed = answer
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .ok_or_else(|| FormError::new(format!("`{}` needs a number", field.name)))?;
            serde_json::Value::Number(parsed)
        }
        FieldKind::Integer => {
            let parsed = answer
                .trim()
                .parse::<i64>()
                .map_err(|_| FormError::new(format!("`{}` needs a whole number", field.name)))?;
            serde_json::Value::Number(parsed.into())
        }
        FieldKind::Boolean => match answer {
            YES => serde_json::Value::Bool(true),
            NO => serde_json::Value::Bool(false),
            _ => {
                return Err(FormError::new(format!(
                    "`{}` needs a yes or no answer",
                    field.name
                )))
            }
        },
        // Handled above, before the single-answer shape is assumed.
        FieldKind::MultiChoice => unreachable!("multi-select answered above"),
    };
    Ok(Some(value))
}

/// An optional field with no answer is absent, which is what the schema means.
/// A required one with no answer cannot be sent at all, so the request declines
/// rather than handing the server an object that breaks its own schema.
fn missing(field: &FormField) -> Result<Option<serde_json::Value>, FormError> {
    if field.required {
        return Err(FormError::new(format!(
            "`{}` was left unanswered",
            field.name
        )));
    }
    Ok(None)
}

const YES: &str = "yes";
const NO: &str = "no";

/// Text and typing rules shared by every schema variant, gathered before the
/// question is built so each arm only supplies what makes it different.
struct FieldPresentation {
    title: Option<String>,
    description: Option<String>,
    default: Option<serde_json::Value>,
    choices: Vec<HostChoice>,
    selection: SelectionMode,
    allow_other: bool,
    kind: FieldKind,
}

fn question_for(
    name: &str,
    property: &PrimitiveSchemaDefinition,
    required: bool,
) -> Result<(HostQuestion, FieldKind), FormError> {
    let presentation = match property {
        PrimitiveSchemaDefinition::Enum(schema) => enum_presentation(schema)?,
        PrimitiveSchemaDefinition::String(schema) => string_presentation(schema),
        PrimitiveSchemaDefinition::Number(schema) => number_presentation(schema),
        PrimitiveSchemaDefinition::Integer(schema) => integer_presentation(schema),
        PrimitiveSchemaDefinition::Boolean(schema) => boolean_presentation(schema),
        // `PrimitiveSchemaDefinition` is non-exhaustive. A field kind Rho has
        // never seen cannot be shown honestly, so the whole form is refused
        // rather than silently dropping a property the server will expect.
        _ => {
            return Err(FormError::new(format!(
                "`{name}` uses a field kind Rho does not support"
            )))
        }
    };
    let prompt = presentation
        .title
        .clone()
        .or_else(|| presentation.description.clone())
        .unwrap_or_else(|| name.to_owned());
    let mut question =
        HostQuestion::new(name, prompt, presentation.choices, presentation.selection)
            .map_err(|error| FormError::new(error.to_string()))?;
    if presentation.allow_other {
        question = question.allow_other();
    }
    // The title becomes the prompt, so the description is what is left to
    // explain the field, including constraints Rho does not enforce.
    if let Some(help) = presentation
        .description
        .filter(|_| presentation.title.is_some())
    {
        question = question.help(help);
    }
    if let Some(default) = presentation.default {
        question = question.default_value(default);
    }
    if !required {
        question = question.optional();
    }
    Ok((question, presentation.kind))
}

fn free_text_choices() -> Vec<HostChoice> {
    // A host question must offer at least one choice, so a free-text field
    // carries one throwaway entry and takes its real answer through
    // `allow_other`. This is the shape the questionnaire tool already uses.
    vec![HostChoice::new("other", "Other")]
}

fn string_presentation(schema: &StringSchema) -> FieldPresentation {
    FieldPresentation {
        title: schema.title.as_ref().map(ToString::to_string),
        description: text_with_hint(
            schema.description.as_ref().map(ToString::to_string),
            string_hint(schema),
        ),
        default: schema.default.clone().map(serde_json::Value::String),
        choices: free_text_choices(),
        selection: SelectionMode::One,
        allow_other: true,
        kind: FieldKind::Text,
    }
}

fn string_hint(schema: &StringSchema) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(format) = schema.format {
        parts.push(format!("format {}", format_label(format)));
    }
    match (schema.min_length, schema.max_length) {
        (Some(min), Some(max)) => parts.push(format!("{min} to {max} characters")),
        (Some(min), None) => parts.push(format!("at least {min} characters")),
        (None, Some(max)) => parts.push(format!("at most {max} characters")),
        (None, None) => {}
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn format_label(format: rmcp::model::StringFormat) -> &'static str {
    use rmcp::model::StringFormat;
    match format {
        StringFormat::Email => "email",
        StringFormat::Uri => "uri",
        StringFormat::Date => "date",
        StringFormat::DateTime => "date-time",
        // Non-exhaustive upstream: an unknown format is still just a hint.
        _ => "unrecognized",
    }
}

fn number_presentation(schema: &NumberSchema) -> FieldPresentation {
    FieldPresentation {
        title: schema.title.as_ref().map(ToString::to_string),
        description: text_with_hint(
            schema.description.as_ref().map(ToString::to_string),
            range_hint(
                schema.minimum.map(|value| value.to_string()),
                schema.maximum.map(|value| value.to_string()),
            ),
        ),
        default: schema
            .default
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number),
        choices: free_text_choices(),
        selection: SelectionMode::One,
        allow_other: true,
        kind: FieldKind::Number,
    }
}

fn integer_presentation(schema: &IntegerSchema) -> FieldPresentation {
    FieldPresentation {
        title: schema.title.as_ref().map(ToString::to_string),
        description: text_with_hint(
            schema.description.as_ref().map(ToString::to_string),
            range_hint(
                schema.minimum.map(|value| value.to_string()),
                schema.maximum.map(|value| value.to_string()),
            ),
        ),
        default: schema
            .default
            .map(|value| serde_json::Value::Number(value.into())),
        choices: free_text_choices(),
        selection: SelectionMode::One,
        allow_other: true,
        kind: FieldKind::Integer,
    }
}

fn range_hint(minimum: Option<String>, maximum: Option<String>) -> Option<String> {
    match (minimum, maximum) {
        (Some(min), Some(max)) => Some(format!("between {min} and {max}")),
        (Some(min), None) => Some(format!("{min} or more")),
        (None, Some(max)) => Some(format!("{max} or less")),
        (None, None) => None,
    }
}

fn boolean_presentation(schema: &BooleanSchema) -> FieldPresentation {
    FieldPresentation {
        title: schema.title.as_ref().map(ToString::to_string),
        description: schema.description.as_ref().map(ToString::to_string),
        // `yes`/`no` is what the interactive host recognizes as a confirm field.
        default: schema
            .default
            .map(|value| serde_json::Value::String(if value { YES.into() } else { NO.into() })),
        choices: vec![HostChoice::new(YES, "Yes"), HostChoice::new(NO, "No")],
        selection: SelectionMode::One,
        allow_other: false,
        kind: FieldKind::Boolean,
    }
}

fn enum_presentation(schema: &EnumSchema) -> Result<FieldPresentation, FormError> {
    let presentation = match schema {
        EnumSchema::Single(SingleSelectEnumSchema::Untitled(single)) => FieldPresentation {
            title: single.title.as_ref().map(ToString::to_string),
            description: single.description.as_ref().map(ToString::to_string),
            default: single.default.clone().map(serde_json::Value::String),
            choices: untitled_choices(&single.enum_),
            selection: SelectionMode::One,
            allow_other: false,
            kind: FieldKind::Choice,
        },
        EnumSchema::Single(SingleSelectEnumSchema::Titled(single)) => FieldPresentation {
            title: single.title.as_ref().map(ToString::to_string),
            description: single.description.as_ref().map(ToString::to_string),
            default: single.default.clone().map(serde_json::Value::String),
            choices: titled_choices(&single.one_of),
            selection: SelectionMode::One,
            allow_other: false,
            kind: FieldKind::Choice,
        },
        EnumSchema::Multi(MultiSelectEnumSchema::Untitled(multi)) => FieldPresentation {
            title: multi.title.as_ref().map(ToString::to_string),
            description: multi.description.as_ref().map(ToString::to_string),
            default: multi.default.clone().map(string_array),
            choices: untitled_choices(&multi.items.enum_),
            selection: SelectionMode::Many,
            allow_other: false,
            kind: FieldKind::MultiChoice,
        },
        EnumSchema::Multi(MultiSelectEnumSchema::Titled(multi)) => FieldPresentation {
            title: multi.title.as_ref().map(ToString::to_string),
            description: multi.description.as_ref().map(ToString::to_string),
            default: multi.default.clone().map(string_array),
            choices: titled_choices(&multi.items.any_of),
            selection: SelectionMode::Many,
            allow_other: false,
            kind: FieldKind::MultiChoice,
        },
        EnumSchema::Legacy(legacy) => FieldPresentation {
            title: legacy.title.as_ref().map(ToString::to_string),
            description: legacy.description.as_ref().map(ToString::to_string),
            default: legacy.default.clone().map(serde_json::Value::String),
            choices: legacy_choices(&legacy.enum_, legacy.enum_names.as_deref()),
            selection: SelectionMode::One,
            allow_other: false,
            kind: FieldKind::Choice,
        },
        // Non-exhaustive upstream: an unknown enum shape has no choices Rho can
        // show, and an empty list is not a question.
        _ => return Err(FormError::new("enum field uses an unsupported shape")),
    };
    if presentation.choices.is_empty() {
        return Err(FormError::new("enum field offers no values to choose from"));
    }
    Ok(presentation)
}

fn untitled_choices(values: &[String]) -> Vec<HostChoice> {
    values
        .iter()
        .map(|value| HostChoice::new(value, value))
        .collect()
}

fn titled_choices(values: &[rmcp::model::ConstTitle]) -> Vec<HostChoice> {
    values
        .iter()
        .map(|value| HostChoice::new(&value.const_, &value.title))
        .collect()
}

/// Legacy schemas carry labels in a parallel array. A short or missing label
/// list leaves the value as its own label rather than pairing them up wrongly.
fn legacy_choices(values: &[String], labels: Option<&[String]>) -> Vec<HostChoice> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let label = labels
                .and_then(|labels| labels.get(index))
                .map_or(value.as_str(), String::as_str);
            HostChoice::new(value, label)
        })
        .collect()
}

fn string_array(values: Vec<String>) -> serde_json::Value {
    serde_json::Value::Array(values.into_iter().map(serde_json::Value::String).collect())
}

/// Join the server's own wording with a constraint hint Rho only displays.
fn text_with_hint(description: Option<String>, hint: Option<String>) -> Option<String> {
    match (description, hint) {
        (Some(description), Some(hint)) => Some(format!("{description} ({hint})")),
        (Some(description), None) => Some(description),
        (None, Some(hint)) => Some(hint),
        (None, None) => None,
    }
}

#[cfg(test)]
#[path = "elicitation_form_tests.rs"]
mod tests;
