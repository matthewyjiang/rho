use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::json;

use super::finished_card;

fn card(status: ToolStatus) -> ToolCard {
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call("tool_search", Some("\"github issue\"".into())),
    )
}

// Covers: hits render as one `name · first sentence` row each with schemas
// dropped; a miss notice or unparseable output shows as written instead of
// being reinterpreted; a failure surfaces its reason.
// Owner: tool_search presenter.
#[test]
fn finished_card_lists_hits_without_schemas() {
    let arguments = json!({"query": "github issue"});
    let hits = serde_json::to_string_pretty(&json!([
        {
            "name": "mcp__github__create_issue",
            "description": "Create a GitHub issue. Requires repo access.",
            "parameters": {"type": "object"},
            "returns": {"type": "object"},
        },
        {"name": "bare", "description": "", "parameters": {}, "returns": {}},
    ]))
    .unwrap();
    let found = card(ToolStatus::Ok)
        .with_facts(vec![ToolFact::Count {
            label: "tools".into(),
            value: 2,
            detail: None,
        }])
        .with_body(ToolBody::Lines(vec![
            "mcp__github__create_issue · Create a GitHub issue".into(),
            "bare".into(),
        ]));
    let miss_text = "no tools matched \"github issue\"; call `list_tools()` to browse.";
    let miss = card(ToolStatus::Ok).with_body(ToolBody::Lines(vec![miss_text.into()]));
    let cut = card(ToolStatus::Ok).with_body(ToolBody::Lines(vec!["[".into(), "  {".into()]));
    let failed = card(ToolStatus::Error).with_facts(vec![ToolFact::Error {
        text: "invalid arguments".into(),
    }]);
    for (case, content, ok, want) in [
        ("hits", hits.as_str(), true, &found),
        ("miss notice", miss_text, true, &miss),
        ("unparseable output", "[\n  {", true, &cut),
        ("failure", "invalid arguments", false, &failed),
    ] {
        assert_eq!(finished_card(&arguments, content, ok), *want, "{case}");
    }
}
