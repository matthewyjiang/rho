use std::time::Duration;

use super::PendingSessionTitle;
use crate::tui::SessionTitleResult;

#[tokio::test]
async fn dropping_pending_session_title_cancels_its_owned_task() {
    let cancellation = rho_sdk::CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        task_cancellation.cancelled().await;
        let _ = completed_tx.send(());
        SessionTitleResult {
            session_id: "title-session".into(),
            title: Err(anyhow::anyhow!("cancelled")),
        }
    });
    let pending = PendingSessionTitle::new("title-session".into(), cancellation, handle);

    drop(pending);

    tokio::time::timeout(Duration::from_secs(1), completed_rx)
        .await
        .expect("title task completed cancellation cleanup")
        .expect("title task reported completion");
}
