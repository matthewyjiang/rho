use std::path::Path;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

#[cfg(unix)]
#[tokio::test]
async fn waiting_for_user_reports_herdr_blocked_then_working_resumes() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = TestHerdrServer::bind(&socket_path).await;
    let mut bootstrap = test_bootstrap();
    bootstrap.services.herdr = herdr_for_socket(&socket_path);
    bootstrap.session.session_id = Some("session-questionnaire".into());
    let app = App::new(bootstrap);

    app.report_herdr_waiting_for_user("waiting for your answers")
        .await;
    app.report_herdr_working().await;

    let blocked = server.next_request().await;
    assert_eq!(blocked["method"], "pane.report_agent");
    assert_eq!(blocked["params"]["state"], "blocked");
    assert_eq!(blocked["params"]["message"], "waiting for your answers");
    assert_eq!(
        blocked["params"]["agent_session_id"],
        "session-questionnaire"
    );

    let working = server.next_request().await;
    assert_eq!(working["method"], "pane.report_agent");
    assert_eq!(working["params"]["state"], "working");
    assert!(working["params"].get("message").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn resting_herdr_state_stays_blocked_when_auth_is_unavailable() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = TestHerdrServer::bind(&socket_path).await;
    let mut bootstrap = test_bootstrap();
    bootstrap.services.herdr = herdr_for_socket(&socket_path);
    bootstrap.services.auth_unavailable = Some("login required".into());
    let app = App::new(bootstrap);

    app.report_resting_herdr_state().await;

    let request = server.next_request().await;
    assert_eq!(request["params"]["state"], "blocked");
    assert_eq!(request["params"]["message"], "login required");
}

#[cfg(unix)]
fn herdr_for_socket(socket_path: &Path) -> HerdrReporter {
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

    async fn next_request(&mut self) -> Value {
        self.requests.recv().await.unwrap()
    }
}
