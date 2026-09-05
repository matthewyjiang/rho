use std::sync::{Arc, Barrier};

use pretty_assertions::assert_eq;
use rho_sdk::{
    boundary_input_channel,
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    InputBoundary, Rho, RunEvent, SessionOptions, UserInput,
};

use super::lock;
use crate::app::{
    subagent_manager::SubagentManager,
    subagent_messaging::{SubagentNotice, SubagentNoticeBridge},
};

// Covers: child notice + completion published between source reads cannot skew
// a final snapshot; work released after the synchronous handoff stays deliverable.
// Owner: CLI notification publication protocol. PTY cannot force this interleaving.
#[tokio::test]
async fn publication_during_final_snapshot_stays_ordered_for_idle_delivery() {
    let root = tempfile::tempdir().unwrap();
    let manager = SubagentManager::new(
        crate::config::Config::default(),
        root.path().join("config.toml"),
        root.path().to_path_buf(),
    );
    let bridge = SubagentNoticeBridge::new();
    let (mut notices, permits) = bridge.bind_parent();
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("done".into()),
            ]))],
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let (source, mut requests) = boundary_input_channel();
    session.set_boundary_inputs(Some(source)).unwrap();
    let mut run = session.start(UserInput::text("work")).await.unwrap();
    let first = requests.recv().await.unwrap();
    assert_eq!(first.boundary(), InputBoundary::BeforeProvider);
    assert!(first.respond(None).await);
    let final_request = loop {
        tokio::select! {
            request = requests.recv() => break request.unwrap(),
            event = run.next_event() => assert!(!matches!(event, None | Some(RunEvent::Completed { .. }))),
        }
    };
    assert_eq!(final_request.boundary(), InputBoundary::BeforeCompletion);
    let notice = SubagentNotice {
        run_id: "abc123".into(),
        agent_id: "fixture".into(),
        parent_session_id: session.id().clone(),
        message: "earlier finding".into(),
        acknowledged: Default::default(),
    };
    let publication = Arc::new(Barrier::new(2));
    let (worker, receipt) = {
        let _snapshot = lock();
        assert!(notices.try_recv().is_err()); // First source already sampled.
        let worker = {
            let publication = Arc::clone(&publication);
            let manager = manager.clone();
            let bridge = bridge.clone();
            let notice = notice.clone();
            std::thread::spawn(move || {
                publication.wait();
                bridge.post(notice.clone()).unwrap();
                manager.insert_completed_for_test(
                    "abc123",
                    notice.parent_session_id.as_str(),
                    None,
                );
            })
        };
        publication.wait(); // Publisher now attempts notice, then terminal, between reads.
        assert!(manager.take_notifications(session.id().as_str()).is_empty());
        let receipt = final_request.respond(None); // Send while publication is excluded.
        (worker, receipt)
    };
    assert!(receipt.await);
    worker.join().unwrap();
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    // The idle path sees the earlier notice and terminal together, in that order.
    let received = notices.try_recv().unwrap();
    assert_eq!(received, notice);
    let terminals = manager.take_notifications(session.id().as_str());
    assert_eq!(
        terminals
            .iter()
            .map(|item| item.snapshot.id.as_str())
            .collect::<Vec<_>>(),
        vec!["abc123"]
    );
    permits.release_notice(&received);
}
