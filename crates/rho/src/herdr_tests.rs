use super::HerdrReporter;
#[cfg(unix)]
use super::HerdrState;
use std::collections::HashMap;

#[test]
fn disabled_without_complete_herdr_environment() {
    let reporter = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        _ => None,
    });

    assert!(!reporter.is_enabled());
}

#[cfg(unix)]
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

#[cfg(windows)]
#[test]
fn disabled_when_platform_does_not_support_herdr_socket() {
    let values = HashMap::from([
        ("HERDR_ENV", "1"),
        ("HERDR_SOCKET_PATH", r"C:\\temp\\herdr.sock"),
        ("HERDR_PANE_ID", "w1:p1"),
    ]);
    let reporter = HerdrReporter::from_env_vars(|key| values.get(key).map(|value| (*value).into()));

    assert!(!reporter.is_enabled());
}

#[cfg(unix)]
#[test]
fn socket_reachability_connects_to_live_socket() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let reporter = super::test_support::reporter_for_socket(&socket_path);

    assert_eq!(reporter.socket_is_reachable(), Some(true));
}

#[cfg(unix)]
#[test]
fn socket_reachability_rejects_regular_file() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    std::fs::write(&socket_path, "not a socket").unwrap();
    let reporter = super::test_support::reporter_for_socket(&socket_path);

    assert_eq!(reporter.socket_is_reachable(), Some(false));
}

#[cfg(unix)]
#[tokio::test]
async fn report_state_sends_herdr_json_rpc_request() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = super::test_support::TestHerdrServer::bind(&socket_path).await;
    let reporter = super::test_support::reporter_for_socket(&socket_path);

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
    let mut server = super::test_support::TestHerdrServer::bind(&socket_path).await;
    let reporter = super::test_support::reporter_for_socket(&socket_path);

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
    let mut server = super::test_support::TestHerdrServer::bind(&socket_path).await;
    let reporter = super::test_support::reporter_for_socket(&socket_path);

    reporter.release().await;

    let request = server.next_request().await;
    assert_eq!(request["method"], "pane.release_agent");
    assert_eq!(request["params"]["pane_id"], "w1:p1");
    assert_eq!(request["params"]["agent"], "rho");
    assert!(request["params"].get("seq").is_none());
}
