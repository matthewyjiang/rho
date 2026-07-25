use std::{path::PathBuf, time::Duration};

use pretty_assertions::assert_eq;

use crate::subagent::RunState;

use super::*;

fn system_prompt() -> PromptPolicy {
    PromptPolicy::Replace("Be brief.".into())
}

fn logged_in() -> ClaudeAuthStatus {
    ClaudeAuthStatus {
        logged_in: true,
        auth_method: Some("oauth".into()),
        api_provider: None,
        email: Some("t@example.com".into()),
        org_id: None,
        org_name: None,
        subscription_type: None,
    }
}

fn exit_status(success: bool) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        let program = if success { "true" } else { "false" };
        std::process::Command::new(program).status().unwrap()
    }
    #[cfg(windows)]
    {
        let code = if success { "0" } else { "1" };
        std::process::Command::new("cmd")
            .args(["/C", &format!("exit {code}")])
            .status()
            .unwrap()
    }
}

#[tokio::test]
async fn cancelled_before_start_writes_stopped_status() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let cancellation = RunCancellation::new();
    cancellation.cancel();
    run_session(ClaudeSessionRequest {
        system_prompt: system_prompt(),
        identity: ClaudeRunIdentity {
            agent_id: "claude-planner".into(),
            agent_fingerprint: "fp".into(),
            model: Some("opus".into()),
        },
        model: Some("opus".into()),
        tools: vec!["Read".into()],
        inherit_claude_config: false,
        max_turns: 8,
        effort: None,
        prompt: "hi".into(),
        output_file: output.clone(),
        cwd: dir.path().to_path_buf(),
        permission_mode: PermissionMode::Auto,
        cancellation,
        status_tx: None,
        started_status: None,
        overrides: ClaudeSessionOverrides {
            auth_status: Some(Ok(logged_in())),
            ..ClaudeSessionOverrides::default()
        },
    })
    .await
    .unwrap();
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Stopped);
    assert_eq!(status.provider.as_deref(), Some("claude-code"));
    assert_eq!(status.model.as_deref(), Some("opus"));
}

#[test]
fn decide_final_outcome_matrix() {
    use crate::claude_runtime::stream::{TerminalClassification, TerminalResult};

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

    let ok_status = exit_status(true);
    let err_status = exit_status(false);

    match decide_final_outcome(Some(&success), ok_status, "") {
        FinalOutcome::Success(terminal) => assert!(terminal.classification.is_success()),
        FinalOutcome::Failure { .. } => panic!("expected success"),
    }
    match decide_final_outcome(Some(&success), err_status, "") {
        FinalOutcome::Failure { .. } => {}
        FinalOutcome::Success(_) => panic!("nonzero exit must not succeed"),
    }
    match decide_final_outcome(Some(&failure), ok_status, "") {
        FinalOutcome::Failure { prefer_detail, .. } => assert!(!prefer_detail),
        FinalOutcome::Success(_) => panic!("failure terminal must not succeed"),
    }
    match decide_final_outcome(Some(&invalid), ok_status, "") {
        FinalOutcome::Failure { .. } => {}
        FinalOutcome::Success(_) => panic!("invalid terminal must not succeed"),
    }
    match decide_final_outcome(None, ok_status, "") {
        FinalOutcome::Failure {
            terminal: None,
            detail,
            ..
        } => assert!(detail.contains("without a terminal result")),
        _ => panic!("missing terminal must fail"),
    }
    match decide_final_outcome(None, err_status, "error: unknown option '--max-turns'") {
        FinalOutcome::Failure { detail, .. } => {
            assert!(detail.contains("max-turns"));
        }
        FinalOutcome::Success(_) => panic!("max-turns rejection must fail"),
    }
}

#[cfg(unix)]
#[path = "session_process_tests.rs"]
mod unix_fake_matrix;
