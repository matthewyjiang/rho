use pretty_assertions::assert_eq;

use super::super::ComputerUseStatus;
use super::*;

// Covers: revoke racing a successful installer must suppress follow-up activation.
// Owner: session setup lifecycle; OS tree cleanup is tested at the installer boundary.
#[tokio::test]
async fn revocation_wins_over_completed_installation() {
    let root = tempfile::tempdir().unwrap();
    // Explicit path avoids any discovery or process on the developer's host.
    // The minimal output budget is unused because connection must be rejected.
    let session = ComputerUseSession::new(
        Some(root.path().join("fixture-driver")),
        1,
        root.path().into(),
    );
    let task = super::super::retained_task(async { Ok(()) });
    *session.state() = State::Installing(Installation {
        cancellation: Arc::new(CancellationToken::new()),
        task: task.clone(),
    });
    assert!(session.start_connect().is_err());
    task.await.unwrap();
    session.revoke();
    assert_eq!(
        session.take_installation_result(),
        Some(ComputerSetupUpdate::Cancelled)
    );
    assert_eq!(session.take_installation_result(), None);
    assert_eq!(session.status(), ComputerUseStatus::Off);
}

// Covers: shutdown must await installer cleanup rather than returning after cancel.
// Owner: session lifecycle; the installer tests own actual process-tree termination.
#[tokio::test]
async fn disconnect_waits_for_installer_cleanup_and_blocks_reactivation() {
    let session = ComputerUseSession::new(None, 1, PathBuf::new());
    let cancellation = Arc::new(CancellationToken::new());
    let token = cancellation.clone();
    let (cleaned, cleanup) = tokio::sync::oneshot::channel();
    let task = super::super::retained_task(async move {
        token.cancelled().await;
        cleanup.await?;
        Ok(())
    });
    *session.state() = State::Installing(Installation {
        cancellation: cancellation.clone(),
        task,
    });
    let shutdown = session.disconnect();
    tokio::pin!(shutdown);
    assert!(futures_util::poll!(&mut shutdown).is_pending());
    assert!(cancellation.is_cancelled());
    assert_eq!(session.status(), ComputerUseStatus::Installing);
    assert!(session.start_connect().is_err());
    assert!(session.start_installation().is_err());
    assert_eq!(session.take_installation_result(), None);
    cleaned.send(()).unwrap();
    shutdown.await;
    assert_eq!(session.status(), ComputerUseStatus::Off);
    assert_eq!(session.take_installation_result(), None);
}
