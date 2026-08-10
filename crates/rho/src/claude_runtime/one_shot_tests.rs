use super::*;

fn exit_status() -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        std::process::Command::new("false").status().unwrap()
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "exit 1"])
            .status()
            .unwrap()
    }
}

// Covers: advisor callers receive the shared unsupported-flag diagnosis, not
// a generic missing-result error.
// Owner: Claude one-shot adapter.
#[test]
fn unsupported_max_turns_uses_shared_terminal_diagnosis() {
    let result = finish(
        String::new(),
        None,
        "error: unknown option '--max-turns'",
        exit_status(),
    );
    let Err(error) = result else {
        panic!("unsupported --max-turns must fail");
    };

    assert_eq!(
        error,
        "claude code: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap"
    );
}

// Covers: Claude Code safeguard / API errors arrive as stream-json result text
// with exit 1 and empty stderr; the advisor tool error must carry that text.
// Owner: Claude one-shot adapter.
#[test]
fn safeguard_stream_failure_surfaces_in_one_shot_error() {
    use crate::claude_runtime::stream::{TerminalClassification, TerminalResult};

    let terminal = TerminalResult {
        classification: TerminalClassification::Failure {
            subtype: "success".into(),
            is_error: true,
        },
        result_text: Some(
            "API Error: Fable 5's safeguards flagged this message (https://www.anthropic.com/legal/aup). Claude Code can't respond to this message with Fable 5. Try rephrasing the request in a new session or change your model."
                .into(),
        ),
        error: Some(
            "API Error: Fable 5's safeguards flagged this message (https://www.anthropic.com/legal/aup). Claude Code can't respond to this message with Fable 5. Try rephrasing the request in a new session or change your model."
                .into(),
        ),
        session_id: Some("s".into()),
        num_turns: Some(1),
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
    };

    let result = finish(String::new(), Some(terminal), "", exit_status());
    let Err(error) = result else {
        panic!("safeguard failure must error");
    };
    assert!(
        error.contains("safeguards flagged") && error.contains("change your model"),
        "advisor one-shot must return the stream API error, got: {error}"
    );
}
