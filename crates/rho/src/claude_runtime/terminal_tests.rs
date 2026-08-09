use super::*;

use crate::claude_runtime::stream::{TerminalClassification, TerminalResult};

enum ExitResult {
    Success,
    Failure,
}

fn exit_status(result: ExitResult) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        let program = match result {
            ExitResult::Success => "true",
            ExitResult::Failure => "false",
        };
        std::process::Command::new(program).status().unwrap()
    }
    #[cfg(windows)]
    {
        let code = match result {
            ExitResult::Success => "0",
            ExitResult::Failure => "1",
        };
        std::process::Command::new("cmd")
            .args(["/C", &format!("exit {code}")])
            .status()
            .unwrap()
    }
}

// Covers: delegated sessions and advisor one-shot calls make the same decision
// for each Claude terminal result and process exit combination.
// Owner: Claude runtime terminal protocol.
#[test]
fn terminal_assessment_matrix() {
    let success = TerminalResult {
        classification: TerminalClassification::Success {
            subtype: "success".into(),
        },
        result_text: Some("ok".into()),
        error: None,
        session_id: Some("s".into()),
        num_turns: Some(1),
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };
    let failure = TerminalResult {
        classification: TerminalClassification::Failure {
            subtype: "error_max_turns".into(),
            is_error: true,
        },
        result_text: Some("hit max".into()),
        error: Some("hit max".into()),
        session_id: None,
        num_turns: Some(3),
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };
    let invalid = TerminalResult {
        classification: TerminalClassification::Invalid {
            reason: "missing subtype".into(),
        },
        result_text: None,
        error: Some("missing subtype".into()),
        session_id: None,
        num_turns: None,
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };

    let ok_status = exit_status(ExitResult::Success);
    let err_status = exit_status(ExitResult::Failure);

    assert!(matches!(
        assess_terminal(Some(success.clone()), ok_status, ""),
        TerminalOutcome::Success(_)
    ));
    assert!(matches!(
        assess_terminal(Some(success), err_status, ""),
        TerminalOutcome::Failure { .. }
    ));
    assert!(matches!(
        assess_terminal(Some(failure), ok_status, ""),
        TerminalOutcome::Failure {
            prefer_detail: false,
            ..
        }
    ));
    assert!(matches!(
        assess_terminal(Some(invalid), ok_status, ""),
        TerminalOutcome::Failure { .. }
    ));
    assert!(matches!(
        assess_terminal(None, ok_status, ""),
        TerminalOutcome::Failure { terminal: None, .. }
    ));
    let TerminalOutcome::Failure { detail, .. } =
        assess_terminal(None, err_status, "error: unknown option '--max-turns'")
    else {
        panic!("unsupported --max-turns must fail");
    };
    assert_eq!(
        detail,
        "claude code: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap"
    );
}
