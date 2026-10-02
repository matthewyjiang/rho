use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::json;

use super::{finished_card, progress_card};

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

// Covers: running cards keep only the latest call rows (failed ones as errors)
// above the script; earlier rows collapse to a count.
// Owner: codemode presenter.
#[test]
fn progress_card_shows_latest_rows_above_script() {
    let mut rows: Vec<String> = (0..9).map(|_| "✓ read_file a.rs 1ms".to_string()).collect();
    rows.push("✗ bash false 2ms · exit 1".into());
    let card = progress_card(&json!({"script": "result = 1\n"}), &rows.join("\n"));

    let mut want = expected(ToolStatus::Running, None, &["result = 1"]);
    want.push_fact(ToolFact::Meta {
        text: "… 2 earlier calls".into(),
    });
    for _ in 0..7 {
        want.push_fact(ToolFact::Text {
            text: "✓ read_file a.rs 1ms".into(),
        });
    }
    want.push_fact(ToolFact::Error {
        text: "✗ bash false 2ms · exit 1".into(),
    });
    assert_eq!(card, want);
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
