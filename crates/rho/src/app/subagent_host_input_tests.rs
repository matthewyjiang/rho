use pretty_assertions::assert_eq;
use rho_sdk::{
    CancellationToken, HostChoice, HostInputRequest, HostInputResponse, HostQuestion, SelectionMode,
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
            sample_request(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("interactive parent session"));
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
                .request("abc123", "worker", sample_request(), &cancellation)
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
async fn reports_a_parent_that_stopped_accepting_requests() {
    let bridge = SubagentHostInputBridge::new();
    let receiver = bridge.bind_parent();
    drop(receiver);
    let cancellation = CancellationToken::new();

    let error = timeout(
        Duration::from_secs(1),
        bridge.request("run-1", "worker", sample_request(), &cancellation),
    )
    .await
    .expect("request timeout")
    .unwrap_err();

    assert!(matches!(
        error,
        rho_sdk::Error::Interrupted { message, .. }
            if message.contains("stopped accepting delegated questionnaires")
    ));
}

#[tokio::test]
async fn reports_a_queued_request_the_parent_dropped() {
    let bridge = SubagentHostInputBridge::new();
    let receiver = bridge.bind_parent();
    let cancellation = CancellationToken::new();
    let child = tokio::spawn({
        let bridge = bridge.clone();
        async move {
            bridge
                .request("run-1", "worker", sample_request(), &cancellation)
                .await
        }
    });

    timeout(Duration::from_secs(1), async {
        while receiver.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request should be queued");
    drop(receiver);

    let error = timeout(Duration::from_secs(1), child)
        .await
        .expect("request timeout")
        .expect("request task")
        .unwrap_err();
    assert!(matches!(
        error,
        rho_sdk::Error::Interrupted { message, .. }
            if message.contains("dropped without a response")
    ));
}

#[tokio::test]
async fn forwards_the_child_identity_to_the_parent() {
    let bridge = SubagentHostInputBridge::new();
    let mut receiver = bridge.bind_parent();
    let request = sample_request();
    let request_id = request.id().clone();
    let cancellation = CancellationToken::new();

    let child = tokio::spawn({
        let bridge = bridge.clone();
        async move {
            bridge
                .request("run-1", "worker", request, &cancellation)
                .await
        }
    });

    let pending = timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("bridge delivery")
        .expect("pending request");
    assert_eq!(
        (
            pending.run_id.as_str(),
            pending.agent_id.as_str(),
            pending.request.id(),
        ),
        ("run-1", "worker", &request_id)
    );
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
