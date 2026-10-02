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
    .with_body(ToolBody::Lines(
        script.iter().map(|line| line.to_string()).collect(),
    ))
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
    use crate::app::interactive_presenter::{InteractiveToolPresenter, PresentedToolCard};
    use rho_sdk::{model::ToolCall, tool::ToolOutput, ToolCallId, ToolCompletion};

    let call = ToolCall {
        id: "script-call".into(),
        name: "codemode".into(),
        arguments: json!({"script": "a = 1\nresult = a"}),
    };
    let script = ["a = 1", "result = a"];
    let mut presenter = InteractiveToolPresenter::new(".".into());
    presenter.proposed(call.clone());
    let output = ToolOutput::text("printed line").with_structured_content(json!({"calls": 3}));
    let (ok, live) = presenter.finished(
        &ToolCallId::from_string(call.id.clone()).unwrap(),
        ToolCompletion::from_output(output),
    );
    assert!(ok);
    let want = PresentedToolCard {
        card: expected(ToolStatus::Ok, Some("3 calls"), &script),
        body_syntax: super::body_syntax(super::ToolBodyWindow::Head),
    };
    assert_eq!(live.presentation, want.into());

    let replayed = presenter.historical(&call, /*ok*/ true, "printed line");
    let want = PresentedToolCard {
        card: expected(ToolStatus::Ok, None, &script),
        body_syntax: super::body_syntax(super::ToolBodyWindow::Head),
    };
    assert_eq!(replayed.presentation, want.into());

    presenter.proposed(call.clone());
    let failed_output = ToolOutput::text("[output truncated] printed line")
        .failed()
        .with_structured_content(json!({"calls": 1, "error": "x"}));
    let (ok, failed) = presenter.finished(
        &ToolCallId::from_string(call.id).unwrap(),
        ToolCompletion::from_output(failed_output),
    );
    assert!(!ok);
    let mut card = expected(ToolStatus::Error, Some("1 call"), &script);
    card.push_fact(ToolFact::Error {
        text: "script failed: x".into(),
    });
    let want = PresentedToolCard {
        card,
        body_syntax: super::body_syntax(super::ToolBodyWindow::Head),
    };
    assert_eq!(failed.presentation, want.into());
}

// Covers: traceback tails survive live structured output and historical text;
// missing/truncated markers must not turn ordinary prints into error reasons.
// Owner: codemode diagnostic parsing, not terminal layout.
#[test]
fn failed_cards_keep_full_diagnostics_without_mistaking_prints_for_errors() {
    let arguments = json!({"script": r#"fail("x")"#});
    let traceback =
        "Traceback (most recent call last):\n  File <codemode>, line 1, in <module>\nerror: x";
    let structured = json!({"error": traceback});
    let history = format!("printed line\n\nscript failed: {traceback}");
    let marker_in_prints = format!("script failed: printed lookalike\n\n{history}\n");
    let reason = format!("script failed: {traceback}");
    for (case, content, data, reason) in [
        (
            "live full error despite truncated text",
            "[output truncated]\nprinted line",
            Some(&structured),
            Some(reason.as_str()),
        ),
        (
            "historical multiline traceback",
            history.as_str(),
            None,
            Some(reason.as_str()),
        ),
        (
            "last historical marker",
            marker_in_prints.as_str(),
            None,
            Some(reason.as_str()),
        ),
        (
            "truncated marker missing",
            "printed line\n[output truncated]",
            None,
            None,
        ),
        (
            "empty diagnostic",
            "printed line\nscript failed: ",
            None,
            None,
        ),
        ("empty output", "", None, None),
    ] {
        let mut want = expected(ToolStatus::Error, None, &[r#"fail("x")"#]);
        if let Some(reason) = reason {
            want.push_fact(ToolFact::Error {
                text: reason.into(),
            });
        }
        assert_eq!(
            finished_card(&arguments, content, false, data),
            want,
            "{case}"
        );
    }
}
