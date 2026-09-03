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

fn claude_identity() -> RunArtifactIdentity {
    RunArtifactIdentity {
        agent_id: "claude-planner".into(),
        agent_fingerprint: "fp".into(),
        provider: "claude-code".into(),
        model: Some("opus".into()),
        runtime: crate::agent::AgentRuntime::ClaudeCli,
        reasoning: None,
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
        identity: claude_identity(),
        tools: vec!["Read".into()],
        inherit_claude_config: false,
        max_turns: 8,
        prompt: "hi".into(),
        output_file: output.clone(),
        cwd: dir.path().to_path_buf(),
        permission_mode: PermissionMode::Bypass,
        cancellation,
        status_tx: None,
        started_status: None,
        parent_messages: None,
        auth_status: Some(Ok(logged_in())),
        rate_limit_state_path: None,
        overrides: CliSessionOverrides::default(),
    })
    .await
    .unwrap();
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Stopped);
    assert_eq!(status.provider.as_deref(), Some("claude-code"));
    assert_eq!(status.model.as_deref(), Some("opus"));
}

#[test]
fn ensure_stream_json_input_is_idempotent() {
    let bare = vec!["-p".into(), "--output-format".into(), "stream-json".into()];
    let once = ensure_stream_json_input(bare);
    assert!(once
        .windows(2)
        .any(|window| window == ["--input-format", "stream-json"]));
    let twice = ensure_stream_json_input(once.clone());
    assert_eq!(once, twice);
}

#[cfg(unix)]
#[path = "session_process_tests.rs"]
mod unix_fake_matrix;
