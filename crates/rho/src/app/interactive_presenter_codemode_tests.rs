use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::json;

use super::finished_card;

fn expected(status: ToolStatus, primary: Option<&str>, script: &[&str]) -> ToolCard {
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call("codemode", primary.map(str::to_string)),
    )
    .with_body(ToolBody::Code {
        language: "python".into(),
        lines: script.iter().map(|line| line.to_string()).collect(),
    })
}

// Covers: bridge progress rows never reach the running card, so it keeps the
// started card's shape and nothing collapses when the script finishes.
// Owner: codemode presenter (through the shared progress dispatch).
#[test]
fn running_card_ignores_call_rows() {
    let arguments = json!({"script": "result = 1\n"});
    let view = crate::app::interactive_presenter::ToolView {
        kind: crate::app::interactive_presenter::ToolKind::Codemode,
        name: "codemode".into(),
        arguments: arguments.clone(),
        metadata: Default::default(),
    };
    let rows = rho_sdk::tool::ToolProgress::message("✓ read_file a.rs 1ms\n● bash sleep 5");
    let card = crate::app::interactive_presenter::format::progress_card(
        Some((&view, std::path::Path::new("."))),
        &rows,
    );
    assert_eq!(card, expected(ToolStatus::Running, None, &["result = 1"]));
}

// Covers: finished cards are the script with the call count, live and replayed
// alike (replay has no count); a failed script names its error, not its prints.
// Owner: codemode presenter.
#[test]
fn finished_card_is_the_script() {
    let arguments = json!({"script": "a = 1\nfail(\"x\")"});
    let script = ["a = 1", "fail(\"x\")"];

    let ok = finished_card(&arguments, "printed\n\"ignored\"", true, Some(3));
    assert_eq!(ok, expected(ToolStatus::Ok, Some("3 calls"), &script));

    let replayed = finished_card(&arguments, "printed", true, /*calls*/ None);
    assert_eq!(replayed, expected(ToolStatus::Ok, None, &script));

    let failed = finished_card(
        &arguments,
        "printed line\n\nscript failed: x",
        false,
        Some(1),
    );
    let mut want = expected(ToolStatus::Error, Some("1 call"), &script);
    want.push_fact(ToolFact::Error {
        text: "script failed: x".into(),
    });
    assert_eq!(failed, want);
}
