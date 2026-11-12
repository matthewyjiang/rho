//! What a daemon session records about a compaction after it commits: the
//! `/info` "After run" figures and the usage ledger's follow-up columns.

use pretty_assertions::assert_eq;
use rho_protocol::{methods::SessionInspect, SessionInspectParams, SessionReport};
use serde_json::{json, Value};

use super::sessions::{settled_job, Sandbox};

/// The prompt size the fixture provider reports for every request.
const PROMPT_TOKENS: u64 = 4_321;

// Covers: after `/compact` in a daemon session, the next prompt's
// provider-reported size and the tool calls that follow, one of them a
// repeat of a read the compaction removed, show in `/info` while the
// re-read window is open, and reach the usage ledger when the daemon stops
// with that window still open (#1908).
// Owner: session runtime compaction follow-up, through the real daemon.
#[tokio::test]
async fn a_daemon_session_records_what_follows_a_compaction() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.workspace.join("notes.txt"), "notes\n".repeat(2_000)).unwrap();
    std::fs::write(sandbox.workspace.join("other.txt"), "other\n").unwrap();
    let mut command = sandbox.rho("tool-call");
    command.env(
        "RHO_AUTOMATION_TEST_PROMPT_TOKENS",
        PROMPT_TOKENS.to_string(),
    );
    let (mut daemon, client) = sandbox.spawn_daemon(command, &[]).await;
    let session = sandbox.new_session(&client).await;
    let read = |id: &str, path: &str| {
        json!({"id": id, "name": "read_file", "arguments": {"path": path}}).to_string()
    };
    for text in [
        read("first-read", "notes.txt"),
        "/compact".to_owned(),
        read("repeated-read", "notes.txt"),
        read("other-read", "other.txt"),
    ] {
        sandbox.prompt_text(&client, &session, &text).await.unwrap();
        settled_job(&sandbox, &client, &session).await;
    }

    let report = client
        .request::<SessionInspect>(&SessionInspectParams {
            session: session.clone(),
            report: SessionReport::Compaction,
        })
        .await
        .unwrap()
        .report;
    let after_run = report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["label"] == "After run")
        .map(|row| row["value"].clone());
    let stop = sandbox
        .rho("tool-call")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.isolated.daemon_log());
    daemon.wait().unwrap();
    let ledger =
        rusqlite::Connection::open(sandbox.isolated.rho_home().join("usage.sqlite3")).unwrap();
    let followed = ledger
        .prepare(
            "SELECT trigger, next_prompt_tokens, reread_window, reread_tool_calls,
                    reread_repeated, reread_tracked
             FROM compaction_events",
        )
        .unwrap()
        .query_map([], |row| {
            Ok(json!([
                row.get::<_, String>(0)?,
                row.get::<_, Option<u64>>(1)?,
                row.get::<_, Option<u32>>(2)?,
                row.get::<_, Option<u32>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, Option<u32>>(5)?,
            ]))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<Value>>>()
        .unwrap();

    assert_eq!(
        (after_run, followed),
        (
            Some(Value::from(
                "next prompt 4,321 · 1/2 calls repeated removed reads or commands \
                 (1 watched, window 24)"
            )),
            vec![json!(["manual", PROMPT_TOKENS, 24, 2, 1, 1])]
        ),
        "report: {report:#}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}
