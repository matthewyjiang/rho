use super::*;

use crate::claude_runtime::stream::{TerminalClassification, TerminalResult};

const PROGRAM: &str = "claude code";

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
        assess_terminal(Some(success.clone()), ok_status, "", PROGRAM),
        TerminalOutcome::Success(_)
    ));
    assert!(matches!(
        assess_terminal(Some(success), err_status, "", PROGRAM),
        TerminalOutcome::Failure { .. }
    ));
    assert!(matches!(
        assess_terminal(Some(failure), ok_status, "", PROGRAM),
        TerminalOutcome::Failure {
            prefer_detail: false,
            ..
        }
    ));
    assert!(matches!(
        assess_terminal(Some(invalid), ok_status, "", PROGRAM),
        TerminalOutcome::Failure { .. }
    ));
    assert!(matches!(
        assess_terminal(None, ok_status, "", PROGRAM),
        TerminalOutcome::Failure { terminal: None, .. }
    ));
    let TerminalOutcome::Failure { detail, .. } = assess_terminal(
        None,
        err_status,
        "error: unknown option '--max-turns'",
        PROGRAM,
    ) else {
        panic!("non-zero exit without a stream result must fail");
    };
    assert!(
        detail.contains("process exited") && detail.contains("max-turns"),
        "unsupported-flag diagnosis is Claude session policy; assess_terminal keeps the process detail: {detail}"
    );

    // Non-zero exit with empty stderr still carries the API/safeguard reason on
    // the stream-json result line (Claude Code advisor and subagent path).
    let safeguard = TerminalResult {
        classification: TerminalClassification::Failure {
            subtype: "success".into(),
            is_error: true,
        },
        result_text: Some(
            "API Error: Fable 5's safeguards flagged this message (https://www.anthropic.com/legal/aup)."
                .into(),
        ),
        error: Some(
            "API Error: Fable 5's safeguards flagged this message (https://www.anthropic.com/legal/aup)."
                .into(),
        ),
        session_id: None,
        num_turns: Some(1),
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };
    let TerminalOutcome::Failure { detail, .. } =
        assess_terminal(Some(safeguard), err_status, "", PROGRAM)
    else {
        panic!("safeguard stream failure must fail");
    };
    assert!(
        detail.contains("safeguards flagged"),
        "stream failure text must surface, got: {detail}"
    );
    assert!(
        !detail.contains("process exited"),
        "empty-stderr protocol failures should not bury the stream reason under exit code: {detail}"
    );

    // Success stream + non-zero exit keeps the process diagnosis (contradiction).
    let success_with_text = TerminalResult {
        classification: TerminalClassification::Success {
            subtype: "success".into(),
        },
        result_text: Some("ok".into()),
        error: None,
        session_id: None,
        num_turns: Some(1),
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };
    let TerminalOutcome::Failure { detail, .. } =
        assess_terminal(Some(success_with_text), err_status, "", PROGRAM)
    else {
        panic!("success stream with non-zero exit must fail");
    };
    assert!(
        detail.contains("process exited"),
        "success stream text must not replace exit diagnosis: {detail}"
    );
    assert!(
        !detail.contains("ok"),
        "success answer must not be reported as the failure detail: {detail}"
    );

    // When stderr also explains the crash, keep both lines.
    let failed_with_stderr = TerminalResult {
        classification: TerminalClassification::Failure {
            subtype: "error_during_execution".into(),
            is_error: true,
        },
        result_text: Some("stream said boom".into()),
        error: Some("stream said boom".into()),
        session_id: None,
        num_turns: None,
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };
    let TerminalOutcome::Failure { detail, .. } = assess_terminal(
        Some(failed_with_stderr),
        err_status,
        "segfault nearby",
        PROGRAM,
    ) else {
        panic!("failure with stderr must fail");
    };
    assert!(
        detail.contains("segfault nearby") && detail.contains("stream said boom"),
        "stderr and stream failure should both surface: {detail}"
    );
}
