use super::{HerdrReporter, HerdrState};
use serde_json::Value;
use std::{collections::HashMap, path::Path};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[test]
fn disabled_without_complete_herdr_environment() {
    let reporter = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        _ => None,
    });

    assert!(!reporter.is_enabled());
}

#[test]
fn enabled_from_complete_herdr_environment() {
    let values = HashMap::from([
        ("HERDR_ENV", "1"),
        ("HERDR_SOCKET_PATH", "/tmp/herdr.sock"),
        ("HERDR_PANE_ID", "w1:p1"),
    ]);
    let reporter = HerdrReporter::from_env_vars(|key| values.get(key).map(|value| (*value).into()));

    assert!(reporter.is_enabled());
}

#[cfg(unix)]
#[test]
fn socket_reachability_connects_to_live_socket() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let reporter = reporter_for_socket(&socket_path);

    assert_eq!(reporter.socket_is_reachable(), Some(true));
}

#[cfg(unix)]
#[test]
fn socket_reachability_rejects_regular_file() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    std::fs::write(&socket_path, "not a socket").unwrap();
    let reporter = reporter_for_socket(&socket_path);

    assert_eq!(reporter.socket_is_reachable(), Some(false));
}

#[cfg(unix)]
#[tokio::test]
async fn report_state_sends_herdr_json_rpc_request() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let server = TestHerdrServer::bind(&socket_path).await;
    let reporter = reporter_for_socket(&socket_path);

    reporter
        .report_state(
            HerdrState::Working,
            Some("running tools"),
            Some("session-1"),
        )
        .await;

    let request = server.next_request().await;
    assert_eq!(request["method"], "pane.report_agent");
    assert_eq!(request["params"]["pane_id"], "w1:p1");
    assert_eq!(request["params"]["source"], "herdr:rho");
    assert_eq!(request["params"]["agent"], "rho");
    assert_eq!(request["params"]["state"], "working");
    assert_eq!(request["params"]["message"], "running tools");
    assert_eq!(request["params"]["agent_session_id"], "session-1");
    assert!(request["params"].get("seq").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn report_session_sends_session_reference() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let server = TestHerdrServer::bind(&socket_path).await;
    let reporter = reporter_for_socket(&socket_path);

    reporter.report_session(Some("session-2")).await;

    let request = server.next_request().await;
    assert_eq!(request["method"], "pane.report_agent_session");
    assert_eq!(request["params"]["agent_session_id"], "session-2");
    assert!(request["params"].get("seq").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn release_sends_release_request() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let server = TestHerdrServer::bind(&socket_path).await;
    let reporter = reporter_for_socket(&socket_path);

    reporter.release().await;

    let request = server.next_request().await;
    assert_eq!(request["method"], "pane.release_agent");
    assert_eq!(request["params"]["pane_id"], "w1:p1");
    assert_eq!(request["params"]["agent"], "rho");
    assert!(request["params"].get("seq").is_none());
}

#[cfg(unix)]
fn reporter_for_socket(socket_path: &Path) -> HerdrReporter {
    let socket_path = socket_path.to_string_lossy().to_string();
    HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some(socket_path.clone()),
        "HERDR_PANE_ID" => Some("w1:p1".into()),
        _ => None,
    })
}

#[cfg(unix)]
struct TestHerdrServer {
    requests: tokio::sync::mpsc::UnboundedReceiver<Value>,
}

#[cfg(unix)]
impl TestHerdrServer {
    async fn bind(socket_path: &Path) -> Self {
        let listener = tokio::net::UnixListener::bind(socket_path).unwrap();
        let (tx, requests) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    let mut line = String::new();
                    stream.read_line(&mut line).await.unwrap();
                    let request = serde_json::from_str(&line).unwrap();
                    tx.send(request).unwrap();
                    stream.get_mut().write_all(b"{}\n").await.unwrap();
                });
            }
        });
        Self { requests }
    }

    async fn next_request(mut self) -> Value {
        self.requests.recv().await.unwrap()
    }
}
