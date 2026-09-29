use super::{resume_argv_is_valid, HerdrDelivery, HerdrReporter};
#[cfg(unix)]
use super::{HerdrSession, HerdrSource, HerdrState};
use pretty_assertions::assert_eq;
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

// Covers: Herdr only accepts a self-reported resume command from a report that
// holds the pane under a non-reserved source, ignores reports whose seq is not
// above the last accepted one, and lets only the claiming source release.
// Owner: herdr wire contract
#[cfg(unix)]
#[tokio::test]
async fn reports_carry_source_seq_and_resume_command() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = super::test_support::TestHerdrServer::bind(&socket_path).await;
    let reporter = super::test_support::reporter_for_socket(&socket_path);
    let resume = vec!["rho".to_string(), "--resume".into(), "session-1".into()];
    let session = HerdrSession {
        id: "session-1".into(),
        resume_argv: Some(resume.clone()),
    };

    let deliveries = [
        reporter
            .report_state(HerdrState::Working, Some("running tools"), Some(&session))
            .await,
        reporter.report_session(&session).await,
        reporter.release().await,
    ];
    assert_eq!(deliveries, [HerdrDelivery::Accepted; 3]);
    let state = server.next_request().await;
    let session_report = server.next_request().await;
    let release = server.next_request().await;

    let params = &state["params"];
    assert_eq!(state["method"], "pane.report_agent");
    assert_eq!(params["pane_id"], "w1:p1");
    assert_eq!(params["source"], "rho");
    assert_eq!(params["agent"], "rho");
    assert_eq!(params["state"], "working");
    assert_eq!(params["message"], "running tools");
    assert_eq!(params["agent_session_id"], "session-1");
    assert_eq!(params["resume_argv"], serde_json::json!(resume));
    assert_eq!(session_report["method"], "pane.report_agent_session");
    assert_eq!(
        session_report["params"]["resume_argv"],
        serde_json::json!(resume)
    );
    assert_eq!(release["method"], "pane.release_agent");
    assert_eq!(release["params"]["source"], "rho");

    // A headless child in the same pane releases only its own claim.
    let _ = reporter
        .clone()
        .with_source(HerdrSource::Headless)
        .release()
        .await;
    let headless_release = server.next_request().await;
    assert_eq!(headless_release["params"]["source"], "rho-headless");

    let seqs = [&state, &session_report, &release, &headless_release]
        .map(|request| request["params"]["seq"].as_u64().unwrap());
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
}

// Covers: an argv Herdr would reject makes it drop the whole report, so the
// state must still arrive without the command.
// Owner: herdr wire contract
#[cfg(unix)]
#[tokio::test]
async fn invalid_resume_command_is_dropped_not_sent() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = super::test_support::TestHerdrServer::bind(&socket_path).await;
    let reporter = super::test_support::reporter_for_socket(&socket_path);
    let session = HerdrSession {
        id: "session-1".into(),
        resume_argv: Some(vec!["/usr/bin/rho".into()]),
    };

    let _ = reporter
        .report_state(HerdrState::Idle, None, Some(&session))
        .await;

    let request = server.next_request().await;
    assert_eq!(request["params"]["state"], "idle");
    assert_eq!(request["params"]["agent_session_id"], "session-1");
    assert!(request["params"].get("resume_argv").is_none());
}

#[test]
fn resume_argv_validity_matches_herdr_rules() {
    let many = vec!["x".to_string(); 65];
    let huge = vec!["rho".to_string(), "x".repeat(8 * 1024)];
    let cases: [(&str, Vec<String>, bool); 8] = [
        (
            "plain command",
            vec!["rho".into(), "--resume".into(), "id".into()],
            true,
        ),
        ("empty", Vec::new(), false),
        ("path command", vec!["/usr/bin/rho".into()], false),
        ("flag first", vec!["--resume".into()], false),
        ("apostrophe", vec!["rho".into(), "it's".into()], false),
        (
            "control character",
            vec!["rho".into(), "a\nb".into()],
            false,
        ),
        ("too many arguments", many, false),
        ("too many bytes", huge, false),
    ];
    for (name, argv, expected) in cases {
        assert_eq!(resume_argv_is_valid(&argv), expected, "{name}");
    }
}

// Covers: a report Herdr answered with an error (for example
// `resume_not_accepted`) or never answered must count as failed so callers
// that keep Herdr in sync retry it.
// Owner: herdr wire contract
#[test]
fn delivery_requires_a_response_without_error() {
    let cases = [
        (
            "ok",
            Ok(b"{\"id\":\"rho:1\",\"result\":{}}\n".to_vec()),
            HerdrDelivery::Accepted,
        ),
        (
            "error body",
            Ok(b"{\"id\":\"rho:1\",\"error\":{\"code\":\"resume_not_accepted\"}}\n".to_vec()),
            HerdrDelivery::Failed,
        ),
        ("unparsable", Ok(b"garbage".to_vec()), HerdrDelivery::Failed),
        (
            "timed out",
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut)),
            HerdrDelivery::Failed,
        ),
    ];

    for (name, response, expected) in cases {
        assert_eq!(HerdrDelivery::from_exchange(response), expected, "{name}");
    }
}
