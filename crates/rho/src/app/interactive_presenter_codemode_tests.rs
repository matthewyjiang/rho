use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::json;

use super::{finished_card, preview_card};

fn header(status: ToolStatus, primary: Option<&str>) -> ToolCard {
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call("codemode", primary.map(str::to_string)),
    )
}

// Covers: the script renders as source lines, never as escaped argument JSON.
// Owner: codemode presenter.
#[test]
fn preview_shows_script_lines() {
    let arguments =
        json!({"script": "a = call_tool(\"bash\", {\"command\": \"ls\"})\nresult = a\n"});
    let mut expected = header(ToolStatus::Running, None);
    expected.body = ToolBody::Lines(vec![
        "a = call_tool(\"bash\", {\"command\": \"ls\"})".into(),
        "result = a".into(),
    ]);
    assert_eq!(preview_card(&arguments, ToolStatus::Running), expected);
}

// Covers: finished cards lead with the latest call rows (failed ones as
// errors), then output, then the script; past the row budget, earlier rows
// collapse to a count and the full list moves into the body.
// Owner: codemode presenter.
#[test]
fn finished_card_lists_calls_then_output_then_script() {
    let call = |name: &str, status: &str| json!({"name": name, "args": "", "status": status});
    let mut calls: Vec<_> = (0..9).map(|_| call("read_file", "ok")).collect();
    calls.push(call("bash", "error"));
    let data = json!({
        "return_value": {"n": 1},
        "prints": ["hi"],
        "calls": calls,
    });
    let card = finished_card(
        &json!({"script": "result = 1"}),
        "ignored",
        true,
        Some(&data),
    );

    let mut expected = header(ToolStatus::Ok, Some("10 calls"));
    expected.push_fact(ToolFact::Meta {
        text: "… 2 earlier calls".into(),
    });
    for _ in 0..7 {
        expected.push_fact(ToolFact::Text {
            text: "✓ read_file".into(),
        });
    }
    expected.push_fact(ToolFact::Error {
        text: "✗ bash".into(),
    });
    let mut body = vec![
        "hi".to_string(),
        "{".into(),
        "  \"n\": 1".into(),
        "}".into(),
    ];
    body.push("─── all calls ───".into());
    body.extend(std::iter::repeat_n("✓ read_file".to_string(), 9));
    body.push("✗ bash".into());
    body.push("─── script ───".into());
    body.push("result = 1".into());
    expected.body = ToolBody::Lines(body);
    assert_eq!(card, expected);
}

// Covers: replayed history has no structured output, so the card falls back
// to the model text and keeps a failure reason visible.
// Owner: codemode presenter.
#[test]
fn history_card_falls_back_to_model_text() {
    let card = finished_card(
        &json!({"script": "fail(\"x\")"}),
        "script failed: x",
        false,
        /*data*/ None,
    );
    let mut expected = header(ToolStatus::Error, None);
    expected.push_fact(ToolFact::Error {
        text: "script failed: x".into(),
    });
    expected.body = ToolBody::Lines(vec!["fail(\"x\")".into()]);
    assert_eq!(card, expected);
}
