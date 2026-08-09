use pretty_assertions::assert_eq;
use serde_json::json;

use rho_sdk::{HostInputResponse, SelectionMode};
use rmcp::model::ElicitationSchema;

use super::ElicitationForm;

/// The parts of a built question this suite makes claims about.
#[derive(Debug, PartialEq, Eq)]
struct QuestionShape {
    id: &'static str,
    prompt: String,
    help: Option<String>,
    choices: Vec<(String, String)>,
    many: bool,
    free_text: bool,
    required: bool,
    default: Option<serde_json::Value>,
}

fn schema(properties: serde_json::Value, required: &[&str]) -> ElicitationSchema {
    serde_json::from_value(json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }))
    .expect("elicitation schema fixture parses")
}

fn shape(form: &ElicitationForm, id: &'static str) -> QuestionShape {
    let question = form
        .questions()
        .iter()
        .find(|question| question.id() == id)
        .expect("question for the declared property");
    QuestionShape {
        id,
        prompt: question.prompt().to_owned(),
        help: question.help_text().map(str::to_owned),
        choices: question
            .choices()
            .iter()
            .map(|choice| (choice.value().to_owned(), choice.label().to_owned()))
            .collect(),
        many: question.selection() == SelectionMode::Many,
        free_text: question.permits_other(),
        required: question.is_required(),
        default: question.default_value_ref().cloned(),
    }
}

fn answered(id: &str, values: &[&str]) -> HostInputResponse {
    HostInputResponse::new().answer(id, values.to_vec())
}

// Covers: every primitive elicitation field kind must become a question a
// choice-only host can actually show, because a field Rho renders wrongly
// collects the wrong answer.
// Owner: MCP elicitation schema translation.
#[test]
fn every_field_kind_becomes_a_showable_question() {
    let free_text = vec![("other".to_owned(), "Other".to_owned())];

    let text = schema(
        json!({"name": {"type": "string", "title": "Your name", "description": "as printed", "minLength": 2}}),
        &["name"],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&text).unwrap(), "name"),
        QuestionShape {
            id: "name",
            prompt: "Your name".into(),
            help: Some("as printed (at least 2 characters)".into()),
            choices: free_text.clone(),
            many: false,
            free_text: true,
            required: true,
            default: None,
        }
    );

    let number = schema(
        json!({"ratio": {"type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5}}),
        &[],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&number).unwrap(), "ratio"),
        QuestionShape {
            id: "ratio",
            // No title, so the description Rho composed becomes the prompt.
            prompt: "between 0 and 1".into(),
            help: None,
            choices: free_text.clone(),
            many: false,
            free_text: true,
            required: false,
            default: Some(json!(0.5)),
        }
    );

    let integer = schema(
        json!({"count": {"type": "integer", "minimum": 1}}),
        &["count"],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&integer).unwrap(), "count"),
        QuestionShape {
            id: "count",
            prompt: "1 or more".into(),
            help: None,
            choices: free_text,
            many: false,
            free_text: true,
            required: true,
            default: None,
        }
    );

    // `yes`/`no` is what the interactive host renders as a confirm field.
    let boolean = schema(
        json!({"agree": {"type": "boolean", "title": "Agree?", "default": true}}),
        &["agree"],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&boolean).unwrap(), "agree"),
        QuestionShape {
            id: "agree",
            prompt: "Agree?".into(),
            help: None,
            choices: vec![
                ("yes".to_owned(), "Yes".to_owned()),
                ("no".to_owned(), "No".to_owned())
            ],
            many: false,
            free_text: false,
            required: true,
            default: Some(json!("yes")),
        }
    );

    let single = schema(
        json!({"size": {"type": "string", "enum": ["s", "m"], "title": "Size"}}),
        &["size"],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&single).unwrap(), "size"),
        QuestionShape {
            id: "size",
            prompt: "Size".into(),
            help: None,
            choices: vec![
                ("s".to_owned(), "s".to_owned()),
                ("m".to_owned(), "m".to_owned())
            ],
            many: false,
            free_text: false,
            required: true,
            default: None,
        }
    );

    // A titled enum carries labels the user reads and values the server gets.
    let titled = schema(
        json!({"region": {"type": "string", "oneOf": [
            {"const": "us", "title": "United States"},
            {"const": "uk", "title": "United Kingdom"}
        ]}}),
        &["region"],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&titled).unwrap(), "region"),
        QuestionShape {
            id: "region",
            prompt: "region".into(),
            help: None,
            choices: vec![
                ("us".to_owned(), "United States".to_owned()),
                ("uk".to_owned(), "United Kingdom".to_owned())
            ],
            many: false,
            free_text: false,
            required: true,
            default: None,
        }
    );

    let multi = schema(
        json!({"colors": {"type": "array", "items": {"type": "string", "enum": ["red", "blue"]}}}),
        &[],
    );
    assert_eq!(
        shape(&ElicitationForm::from_schema(&multi).unwrap(), "colors"),
        QuestionShape {
            id: "colors",
            prompt: "colors".into(),
            help: None,
            choices: vec![
                ("red".to_owned(), "red".to_owned()),
                ("blue".to_owned(), "blue".to_owned())
            ],
            many: true,
            free_text: false,
            required: false,
            default: None,
        }
    );
}

// Covers: answers must go back with the JSON type the schema declared, because
// a server that asked for a number and got the string "3" gets a broken
// contract rather than an answer.
// Owner: MCP elicitation answer typing.
#[test]
fn answers_are_returned_with_their_declared_types() {
    let cases: Vec<(&str, serde_json::Value, &[&str], serde_json::Value)> = vec![
        (
            "name",
            json!({"name": {"type": "string"}}),
            &["Ada"],
            json!({"name": "Ada"}),
        ),
        (
            "ratio",
            json!({"ratio": {"type": "number"}}),
            &["0.25"],
            json!({"ratio": 0.25}),
        ),
        (
            "count",
            json!({"count": {"type": "integer"}}),
            &["7"],
            json!({"count": 7}),
        ),
        (
            "agree",
            json!({"agree": {"type": "boolean"}}),
            &["no"],
            json!({"agree": false}),
        ),
        (
            "size",
            json!({"size": {"type": "string", "enum": ["s", "m"]}}),
            &["m"],
            json!({"size": "m"}),
        ),
        (
            "colors",
            json!({"colors": {"type": "array", "items": {"type": "string", "enum": ["red", "blue"]}}}),
            &["red", "blue"],
            json!({"colors": ["red", "blue"]}),
        ),
    ];
    for (id, properties, answers, expected) in cases {
        let form = ElicitationForm::from_schema(&schema(properties, &[id])).unwrap();
        assert_eq!(
            form.content(&answered(id, answers)).unwrap(),
            expected,
            "content for {id}"
        );
    }
}

// Covers: an unanswered optional field must be absent rather than blank, and an
// answer that cannot carry its declared type must fail rather than be coerced.
// Owner: MCP elicitation answer typing.
#[test]
fn unusable_answers_do_not_reach_the_server() {
    let optional = ElicitationForm::from_schema(&schema(
        json!({"nickname": {"type": "string"}, "count": {"type": "integer"}}),
        &["count"],
    ))
    .unwrap();
    assert_eq!(
        optional.content(&answered("count", &["2"])).unwrap(),
        json!({"count": 2})
    );

    let numeric =
        ElicitationForm::from_schema(&schema(json!({"count": {"type": "integer"}}), &["count"]))
            .unwrap();
    assert_eq!(
        numeric
            .content(&answered("count", &["not a number"]))
            .unwrap_err()
            .reason(),
        "`count` needs a whole number"
    );

    // A required field with nothing in it cannot be omitted either, because the
    // server declared that it must be present.
    assert_eq!(
        numeric
            .content(&answered("count", &[""]))
            .unwrap_err()
            .reason(),
        "`count` was left unanswered"
    );
}

// Covers: a schema Rho cannot turn into a question must be refused whole, so a
// half-built form never asks the user for something the server did not want.
// Owner: MCP elicitation schema translation.
#[test]
fn unusable_schemas_are_refused() {
    let empty: ElicitationSchema =
        serde_json::from_value(json!({"type": "object", "properties": {}})).unwrap();
    assert_eq!(
        ElicitationForm::from_schema(&empty).unwrap_err().reason(),
        "elicitation schema declares no properties to ask about"
    );

    let no_values = schema(json!({"size": {"type": "string", "enum": []}}), &["size"]);
    assert_eq!(
        ElicitationForm::from_schema(&no_values)
            .unwrap_err()
            .reason(),
        "enum field offers no values to choose from"
    );
}
