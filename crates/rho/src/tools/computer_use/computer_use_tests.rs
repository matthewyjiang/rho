#![cfg(unix)]

use std::{fs, num::NonZeroUsize, os::unix::fs::PermissionsExt, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    tool::{tool_progress_channel, ToolContext, ToolErrorKind, ToolInvocation},
    ToolCallId,
};
use serde_json::{json, Value};

use super::*;

// Covers: empty/relative PATH entries must not authorize repository executables.
// Owner: executable discovery policy, with injected environment values.
#[test]
fn discovery_ignores_relative_installation_locations() {
    for (path, home, expected) in [
        ("", "relative-home", vec![]),
        (".:bin", "relative-home", vec![]),
        (
            ":/trusted/bin:relative:",
            "/trusted/home",
            vec![
                PathBuf::from("/trusted/bin/cua-driver"),
                PathBuf::from("/trusted/home/.local/bin/cua-driver"),
            ],
        ),
    ] {
        assert_eq!(
            policy::driver_candidates(Some(path.into()), Some(home.into())),
            expected
        );
    }
}

// Covers: revocation kills a driver even before MCP initialization completes.
// Owner: Cua process lifecycle; Unix socket EOF proves the child released its handle.
#[tokio::test]
async fn disconnect_stops_pending_activation() {
    use tokio::io::AsyncReadExt;
    let (root, session) = fixture();
    let socket_path = root.path().join("blocked-connect.sock");
    let listener = tokio::net::UnixListener::bind(socket_path).unwrap();
    session.start_connect().unwrap();
    let (mut child_signal, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    session.disconnect().await;
    assert_eq!(session.status(), ComputerUseStatus::Off);
    assert_eq!(session.terminal_error(), None);
    let mut buffer = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        child_signal.read_to_end(&mut buffer),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(buffer.is_empty());
}

fn fixture() -> (tempfile::TempDir, ComputerUseSession) {
    let root = tempfile::tempdir().unwrap();
    let driver = root.path().join("cua-driver");
    fs::write(&driver, include_str!("fixture.py")).unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o700)).unwrap();
    // Sized from this fixture's two tiny schemas, not a production budget.
    let session = ComputerUseSession::new(Some(driver), 4096, root.path().into());
    (root, session)
}

fn invocation(arguments: Value) -> ToolInvocation {
    ToolInvocation::new(ToolCallId::new(), arguments)
}

fn context() -> ToolContext {
    let (sender, _receiver) = tool_progress_channel(NonZeroUsize::new(1).unwrap());
    ToolContext::new(None, CancellationToken::new(), sender)
}

// Covers: desktop grants are host-only, filtered, and revoked for retained tools.
// Owner: Cua session boundary over a real stdio MCP fixture.
#[tokio::test]
async fn explicit_grant_filters_remote_tools_and_revokes_retained_handles() {
    let (_root, session) = fixture();
    let mut registry =
        super::super::sdk_registry::AppToolSet::disabled().with_computer_use(session.clone());
    assert!(!registry.contains("computer"));
    let tool = session.tool();
    assert_eq!(
        tool.call(invocation(json!({"action":"list"})), context())
            .await
            .unwrap_err()
            .kind(),
        ToolErrorKind::Execution
    );
    assert_eq!(session.status(), ComputerUseStatus::Off);
    session.connect().await.unwrap();
    assert!(registry.set_computer_use_registered(true));
    let output = tool
        .call(invocation(json!({"action":"list"})), context())
        .await
        .unwrap();
    let inventory: Value = serde_json::from_str(output.content()).unwrap();
    let names: Vec<_> = inventory["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["click", "get_window_state"]);
    for args in [
        json!({"action":"call","tool":"set_config","arguments":{}}),
        json!({"action":"call","tool":"click","arguments":{"session":"other"}}),
        json!({"action":"connect"}),
    ] {
        assert_eq!(
            tool.call(invocation(args), context())
                .await
                .unwrap_err()
                .kind(),
            ToolErrorKind::InvalidArguments
        );
    }
    let output = tool
        .call(
            invocation(json!({"action":"call","tool":"get_window_state","arguments":{}})),
            context(),
        )
        .await
        .unwrap();
    assert_eq!(output.images(), &[rho_sdk::model::ImageContent {
        data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=".into(),
        mime_type: "image/png".into(),
    }]);
    session.disconnect().await;
    assert_eq!(session.status(), ComputerUseStatus::Off);
    assert_eq!(
        tool.call(
            invocation(json!({"action":"call","tool":"click","arguments":{}})),
            context()
        )
        .await
        .unwrap_err()
        .kind(),
        ToolErrorKind::Execution
    );
    assert!(registry.set_computer_use_registered(false));
}

// Covers: a cancelled desktop action has uncertain effect; subsequent calls
// cannot proceed on that transport, even if a runtime retains the old tool.
#[tokio::test]
async fn cancellation_revokes_the_grant_and_closes_the_owned_transport() {
    let (_root, session) = fixture();
    session.connect().await.unwrap();
    let tool = session.tool();
    let cancellation = CancellationToken::new();
    let (sender, mut progress) = tool_progress_channel(NonZeroUsize::new(1).unwrap());
    let task = {
        let tool = tool.clone();
        let context = ToolContext::new(None, cancellation.clone(), sender);
        tokio::spawn(async move {
            tool.call(
                invocation(json!({"action":"call","tool":"click","arguments":{}})),
                context,
            )
            .await
        })
    };
    // The fixture emits progress only after receiving the desktop action.
    tokio::time::timeout(Duration::from_secs(10), progress.recv())
        .await
        .unwrap()
        .unwrap();
    cancellation.cancel();
    assert_eq!(
        task.await.unwrap().unwrap_err().kind(),
        ToolErrorKind::Cancelled
    );
    assert_eq!(session.status(), ComputerUseStatus::Closing);
    assert_eq!(
        tool.call(invocation(json!({"action":"list"})), context())
            .await
            .unwrap_err()
            .kind(),
        ToolErrorKind::Execution
    );
    session.disconnect().await;
}

// Covers: a dropped shutdown waiter must not let re-enable overlap old cleanup,
// and a cancellation guard from an old grant must not revoke a newer one.
// Owner: Cua lifecycle over a real stdio transport; the action lock holds cleanup.
#[tokio::test]
async fn cleanup_is_retained_until_complete_and_guards_are_grant_scoped() {
    let (_root, session) = fixture();
    session.connect().await.unwrap();
    let grant = match &*session.state() {
        State::Connected { grant, .. } => grant.clone(),
        _ => unreachable!(),
    };
    let stale_guard = RevokeOnDrop::new(session.clone(), grant);
    let action = session.inner.operation.lock().await;
    session.revoke();
    assert_eq!(session.status(), ComputerUseStatus::Closing);
    assert!(session.start_connect().is_err());
    {
        let shutdown = session.disconnect();
        tokio::pin!(shutdown);
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
    }
    assert_eq!(session.status(), ComputerUseStatus::Closing);
    assert!(session.start_connect().is_err());
    drop(action);
    session.disconnect().await;
    assert_eq!(session.status(), ComputerUseStatus::Off);
    session.connect().await.unwrap();
    drop(stale_guard);
    assert_eq!(session.status(), ComputerUseStatus::Connected);
    session.disconnect().await;
}
