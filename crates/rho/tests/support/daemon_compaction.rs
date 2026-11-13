//! What a daemon session records about a compaction after it commits: the
//! `/info` "After run" figures and the usage ledger's follow-up columns.

use pretty_assertions::assert_eq;
use rho_protocol::{methods::SessionInspect, SessionInspectParams, SessionReport};
use serde_json::{json, Value};

use super::sessions::{settled_job, Sandbox};

/// The prompt size the fixture provider reports for its first request; each
/// later request reports one more.
const PROMPT_TOKENS: u64 = 4_000;

// Covers: after `/compact` in a daemon session, `/info` shows the size the
// provider reports for the first request after it (not one from before),
// once the one-step turn that sent it ends; the tool calls that follow, one
// of them a repeat of a read the compaction removed, count in the re-read
// window, and reach the usage ledger when the daemon stops with that window
// still open (#1908).
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
    let prompt = async |texts: &[String]| {
        for text in texts {
            sandbox.prompt_text(&client, &session, text).await.unwrap();
            settled_job(&sandbox, &client, &session).await;
        }
    };
    let after_run = async || {
        let report = client
            .request::<SessionInspect>(&SessionInspectParams {
                session: session.clone(),
                report: SessionReport::Compaction,
            })
            .await
            .unwrap()
            .report;
        report["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["label"] == "After run")
            .map(|row| row["value"].clone())
    };

    // Requests 0 and 1 read the file and reply; 2 writes the summary; 3
    // answers the plain prompt in one step.
    prompt(&[
        read("first-read", "notes.txt"),
        "/compact".to_owned(),
        "after the compaction".to_owned(),
    ])
    .await;
    let after_prompt = after_run().await;
    prompt(&[
        read("repeated-read", "notes.txt"),
        read("other-read", "other.txt"),
    ])
    .await;
    let after_calls = after_run().await;
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

    let watched = "removed reads or commands (1 watched, window 24)";
    assert_eq!(
        (after_prompt, after_calls, followed),
        (
            Some(Value::from(format!(
                "next prompt 4,003 · 0/0 calls repeated {watched}"
            ))),
            Some(Value::from(format!(
                "next prompt 4,003 · 1/2 calls repeated {watched}"
            ))),
            vec![json!(["manual", PROMPT_TOKENS + 3, 24, 2, 1, 1])]
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
