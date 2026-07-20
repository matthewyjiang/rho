use std::num::NonZeroUsize;

use rho_sdk::{
    tool::{tool_progress_channel, ToolContext, ToolErrorKind},
    CancellationToken, ExecutableSelection, ProcessInvocation, Workspace,
};

use crate::legacy_sdk_adapter::LegacyToolProfile;

use super::{authorize_legacy, process_invocation};

fn denying_context() -> ToolContext {
    let (progress, _receiver) = tool_progress_channel(NonZeroUsize::new(1).unwrap());
    ToolContext::new(
        Some(Workspace::new(std::env::temp_dir()).unwrap()),
        CancellationToken::new(),
        progress,
    )
}

#[tokio::test]
async fn process_profile_requests_process_authorization_for_start() {
    let error = authorize_legacy(
        LegacyToolProfile::Process,
        &serde_json::json!({"action": "start", "command": "echo hello"}),
        &denying_context(),
        1_000,
    )
    .await
    .expect_err("deny-all policy must reject the process request");

    assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
}

#[tokio::test]
async fn web_search_profile_requests_network_authorization() {
    let error = authorize_legacy(
        LegacyToolProfile::WebSearch,
        &serde_json::json!({"query": "hello"}),
        &denying_context(),
        1_000,
    )
    .await
    .expect_err("deny-all policy must reject the network request");

    assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
}

#[cfg(unix)]
#[test]
fn process_authorization_uses_the_unix_execution_plan() {
    let command = "printf '%s' hello";

    let invocation = process_invocation(command);

    assert_eq!(
        invocation,
        ProcessInvocation::Shell {
            executable: "bash".into(),
            selection: ExecutableSelection::SearchPath,
            arguments: vec!["-lc".into()],
            command: command.into(),
        }
    );
}

#[cfg(windows)]
#[test]
fn process_authorization_uses_the_wrapped_windows_execution_plan() {
    let command = "Write-Output hello";

    let invocation = process_invocation(command);

    assert_eq!(
        invocation,
        ProcessInvocation::Shell {
            executable: "powershell.exe".into(),
            selection: ExecutableSelection::SearchPath,
            arguments: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
            ],
            command: crate::powershell::wrapped_command(command),
        }
    );
}
