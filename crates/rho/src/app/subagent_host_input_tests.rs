use pretty_assertions::assert_eq;
use rho_sdk::{
    CancellationToken, HostChoice, HostInputRequest, HostInputResponse, HostQuestion,
    SelectionMode, SessionId,
};
use tokio::time::{timeout, Duration};

use super::SubagentHostInputBridge;

fn sample_request() -> HostInputRequest {
    let question = HostQuestion::new(
        "color",
        "Choose one color",
        vec![
            HostChoice::new("red", "red"),
            HostChoice::new("blue", "blue"),
        ],
        SelectionMode::One,
    )
    .unwrap();
    HostInputRequest::questionnaire("pick", vec![question]).unwrap()
}

#[tokio::test]
async fn unbound_bridge_rejects_child_requests() {
    let bridge = SubagentHostInputBridge::new();
    let error = bridge
        .request(
            "abc123",
            "worker",
            SessionId::new(),
            sample_request(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("interactive parent session"));
}

#[tokio::test]
async fn parent_answer_reaches_the_same_child_request() {
    let bridge = SubagentHostInputBridge::new();
    let mut receiver = bridge.bind_parent();
    let cancellation = CancellationToken::new();
    let request = sample_request();
    let request_id = request.id().clone();
    let parent_session_id = SessionId::new();

    let child = tokio::spawn({
        let bridge = bridge.clone();
        let cancellation = cancellation.clone();
        let parent_session_id = parent_session_id.clone();
        async move {
            bridge
                .request(
                    "abc123",
                    "worker",
                    parent_session_id,
                    request,
                    &cancellation,
                )
                .await
        }
    });

    let pending = timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("bridge delivery")
        .expect("pending request");
    assert_eq!(pending.run_id, "abc123");
    assert_eq!(pending.agent_id, "worker");
    assert_eq!(pending.parent_session_id, parent_session_id);
    assert_eq!(pending.request.id(), &request_id);
    pending
        .response
        .send(Ok(HostInputResponse::new().answer("color", ["blue"])))
        .unwrap();

    let response = timeout(Duration::from_secs(1), child)
        .await
        .expect("child join")
        .expect("child task")
        .expect("child response");
    assert_eq!(
        response.answers().get("color").map(Vec::as_slice),
        Some(["blue".to_string()].as_slice())
    );
}

#[tokio::test]
async fn child_cancellation_ends_a_waiting_request() {
    let bridge = SubagentHostInputBridge::new();
    let _receiver = bridge.bind_parent();
    let cancellation = CancellationToken::new();
    let child = tokio::spawn({
        let bridge = bridge.clone();
        let cancellation = cancellation.clone();
        async move {
            bridge
                .request(
                    "abc123",
                    "worker",
                    SessionId::new(),
                    sample_request(),
                    &cancellation,
                )
                .await
        }
    });
    // Let the child park on the parent answer before cancelling.
    tokio::task::yield_now().await;
    cancellation.cancel();
    let error = timeout(Duration::from_secs(1), child)
        .await
        .expect("child join")
        .expect("child task")
        .unwrap_err();
    assert!(matches!(error, rho_sdk::Error::Cancelled));
}

#[tokio::test]
async fn unbinding_the_parent_fails_later_requests() {
    let bridge = SubagentHostInputBridge::new();
    let _receiver = bridge.bind_parent();
    assert!(bridge.is_bound());
    bridge.unbind_parent();
    assert!(!bridge.is_bound());
    let error = bridge
        .request(
            "abc123",
            "worker",
            SessionId::new(),
            sample_request(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("interactive parent session"));
}
