//! Generated session titles reported to ACP hosts as `session_info_update`.

use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::AcpServer;

// Covers: the title Rho generates after a session's first turn must reach
// the host as `session_info_update` for that session (Zed names threads
// from it, T3 Code records it), matching what `session/list` reports.
// Owner: ACP session titles, over the real stdio server
#[test]
fn generated_title_reaches_the_host() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[web_search]
hosted = false
provider = "disabled"
"#,
    )
    .unwrap();
    let project = home.path().canonicalize().unwrap().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let mut command = AcpServer::command(home.path(), &project, "fixed", &project);
    // The title agent's one-shot provider skips the automation fixture; the
    // TUI fixture answers its request instead.
    command.env("RHO_TUI_TEST_MODE", "matrix");
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let created = server
        .request("session/new", json!({"cwd": project, "mcpServers": []}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    let (replied, during_turn) = server.request_notifications(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "rename the widget"}]}),
    );
    assert_eq!(replied.unwrap()["stopReason"], "end_turn");

    // The title is generated after the turn, so it usually follows the reply.
    let titled = during_turn
        .into_iter()
        .chain(std::iter::repeat_with(|| server.next_notification()))
        .find(|notification| notification["update"]["sessionUpdate"] == "session_info_update")
        .unwrap();
    let title = "Fixture session title";
    assert_eq!(
        titled,
        json!({
            "sessionId": session_id,
            "update": {"sessionUpdate": "session_info_update", "title": title},
        })
    );
    let listed = server
        .request("session/list", json!({"cwd": project}))
        .0
        .unwrap();
    assert_eq!(listed["sessions"][0]["title"], title);
}
