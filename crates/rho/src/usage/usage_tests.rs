use std::{path::PathBuf, sync::Arc, thread, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage,
    },
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

use super::*;

fn recorder() -> (TempDir, SqliteUsageRecorder) {
    let directory = tempfile::tempdir().unwrap();
    let recorder =
        SqliteUsageRecorder::new(directory.path().join("private/usage.sqlite3")).unwrap();
    (directory, recorder)
}

fn event(id: &str, outcome: RequestOutcome) -> UsageEvent {
    let mut event = UsageEvent::new(
        "provider-exact",
        "model-exact",
        "agent",
        outcome,
        ModelUsage {
            input_tokens: Some(11),
            output_tokens: Some(12),
            cache_read_tokens: Some(13),
            cache_write_tokens: Some(14),
            total_tokens: Some(50),
            context_window: Some(200_000),
            cost_usd_micros: Some(123_456),
        },
    );
    event.event_id = id.to_owned();
    event.occurred_at_ms = 1_742_000_123_456;
    event.session_id = Some("session-1".to_owned());
    event.parent_session_id = Some("parent-1".to_owned());
    event.run_id = Some("run-1".to_owned());
    event.step_index = Some(2);
    event.attempt_index = Some(3);
    event.workspace_path = Some("/workspace/project".to_owned());
    event.rho_version = Some("1.2.3-test".to_owned());
    event
}

#[tokio::test]
async fn recorded_model_request_writes_accumulated_usage() {
    let (_directory, recorder) = recorder();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider-exact", "fixture-api", "model-exact"),
        [ScriptedTurn::streaming(
            vec![
                rho_sdk::model::ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(10),
                    cache_read_tokens: Some(4),
                    ..ModelUsage::default()
                }),
                rho_sdk::model::ModelEvent::Usage(ModelUsage {
                    output_tokens: Some(3),
                    cost_usd_micros: Some(99),
                    ..ModelUsage::default()
                }),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )],
    );
    let messages = [Message::user_text("sensitive prompt")];
    let session_id = rho_sdk::SessionId::from_string("title-session").unwrap();
    let context = rho_sdk::ProviderRequestUsageContext::for_purpose(provider.identity(), "title")
        .with_session_id(session_id)
        .with_workspace_path("/workspace/title");
    let (response, usage) = send_recorded(
        &provider,
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
        context,
        rho_sdk::ProviderRequestUsageRecording::new(recorder.clone()),
    )
    .await
    .unwrap();

    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Text("done".into())])
    );
    assert_eq!(
        usage,
        ModelUsage {
            input_tokens: Some(10),
            output_tokens: Some(3),
            cache_read_tokens: Some(4),
            cost_usd_micros: Some(99),
            ..ModelUsage::default()
        }
    );
    let connection = Connection::open(recorder.path()).unwrap();
    let row = connection
        .query_row(
            "SELECT purpose, session_id, workspace_path, attempt_index,
                    input_tokens, output_tokens, cache_read_tokens, cost_usd_micros
             FROM usage_events",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "title".into(),
            "title-session".into(),
            "/workspace/title".into(),
            1,
            10,
            3,
            4,
            99,
        )
    );
    let stored_text: i64 = connection
        .query_row(
            "SELECT count(*) FROM usage_events
             WHERE event_id LIKE '%sensitive%' OR provider LIKE '%sensitive%'
                OR model LIKE '%sensitive%' OR purpose LIKE '%sensitive%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_text, 0);
}

#[tokio::test]
async fn recorded_non_agent_request_persists_each_physical_attempt() {
    let temp_dir = tempfile::tempdir().unwrap();
    let recorder = SqliteUsageRecorder::new(temp_dir.path().join("usage.db")).unwrap();
    let database_path = recorder.path().to_owned();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "fixture-api", "model"),
        [ScriptedTurn::streaming(
            vec![ModelEvent::RequestAttemptFailed {
                kind: rho_sdk::ProviderErrorKind::Unavailable,
                usage: ModelUsage::default(),
            }],
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )],
    );
    let messages = [Message::user_text("request")];

    send_recorded(
        &provider,
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
        rho_sdk::ProviderRequestUsageContext::for_purpose(provider.identity(), "compaction")
            .with_parent_session_id(rho_sdk::SessionId::from_string("parent-agent").unwrap()),
        rho_sdk::ProviderRequestUsageRecording::new(recorder),
    )
    .await
    .unwrap();

    let connection =
        Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let rows = connection
        .prepare(
            "SELECT attempt_index, request_outcome, parent_session_id
             FROM usage_events ORDER BY attempt_index",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (1, "failed".into(), "parent-agent".into()),
            (2, "completed".into(), "parent-agent".into())
        ]
    );
}

struct FailingSdkRecorder;

impl rho_sdk::ProviderRequestUsageRecorder for FailingSdkRecorder {
    fn record(
        &self,
        _event: rho_sdk::ProviderRequestUsageEvent,
    ) -> rho_sdk::ProviderRequestUsageRecorderFuture<'_> {
        Box::pin(async {
            Err(rho_sdk::ProviderRequestUsageRecorderError::new(
                "ledger unavailable",
            ))
        })
    }
}

#[tokio::test]
async fn non_agent_recorder_failures_are_non_fatal_bounded_diagnostics() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "fixture-api", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let messages = [Message::user_text("request")];
    let recording = rho_sdk::ProviderRequestUsageRecording::new(FailingSdkRecorder);

    let (response, _) = send_recorded(
        &provider,
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
        rho_sdk::ProviderRequestUsageContext::for_purpose(provider.identity(), "title"),
        recording.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Text("done".into())])
    );
    let diagnostics = recording.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message(), "ledger unavailable");
}

#[test]
fn creates_and_migrates_a_new_database() {
    let (_directory, recorder) = recorder();
    assert!(recorder.path().is_file());
    let connection = Connection::open(recorder.path()).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(mode.to_ascii_lowercase(), "wal");
}

#[test]
fn writes_exact_usage_and_request_identity() {
    let (_directory, recorder) = recorder();
    recorder
        .record(&event("exact", RequestOutcome::Completed))
        .unwrap();
    let connection = Connection::open(recorder.path()).unwrap();
    let row = connection
        .query_row(
            "SELECT event_id, schema_version, occurred_at_ms, session_id,
                    parent_session_id, run_id, step_index, attempt_index, workspace_path,
                    provider, model, purpose, request_outcome, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, total_tokens, cost_usd_micros,
                    rho_version FROM usage_events",
            [],
            |row| {
                (0..20)
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
            },
        )
        .unwrap();
    use rusqlite::types::Value::{Integer, Text};
    assert_eq!(
        row,
        vec![
            Text("exact".into()),
            Integer(1),
            Integer(1_742_000_123_456),
            Text("session-1".into()),
            Text("parent-1".into()),
            Text("run-1".into()),
            Integer(2),
            Integer(3),
            Text("/workspace/project".into()),
            Text("provider-exact".into()),
            Text("model-exact".into()),
            Text("agent".into()),
            Text("completed".into()),
            Integer(11),
            Integer(12),
            Integer(13),
            Integer(14),
            Integer(50),
            Integer(123_456),
            Text("1.2.3-test".into()),
        ]
    );
}

#[test]
fn preserves_unreported_usage_as_null() {
    let (_directory, recorder) = recorder();
    let mut event = event("nulls", RequestOutcome::Failed);
    event.usage = ModelUsage::default();
    recorder.record(&event).unwrap();
    let connection = Connection::open(recorder.path()).unwrap();
    let values: [Option<i64>; 6] = connection
        .query_row(
            "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    total_tokens, cost_usd_micros FROM usage_events",
            [],
            |row| {
                Ok([
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ])
            },
        )
        .unwrap();
    assert_eq!(values, [None, None, None, None, None, None]);
}

#[test]
fn rejects_values_outside_sqlite_integer_range() {
    let (_directory, recorder) = recorder();
    let mut event = event("overflow", RequestOutcome::Completed);
    event.usage.input_tokens = Some(i64::MAX as u64 + 1);
    let error = recorder.record(&event).unwrap_err();
    assert!(matches!(
        error,
        UsageLedgerError::IntegerOverflow {
            field: "input_tokens",
            ..
        }
    ));
    let connection = Connection::open(recorder.path()).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn retries_are_distinct_but_persistence_retries_are_idempotent() {
    let (_directory, recorder) = recorder();
    let first = event("request-attempt-1", RequestOutcome::Failed);
    let mut second = event("request-attempt-2", RequestOutcome::Completed);
    second.attempt_index = Some(4);
    assert_eq!(recorder.record(&first).unwrap(), RecordOutcome::Inserted);
    assert_eq!(recorder.record(&first).unwrap(), RecordOutcome::Duplicate);
    assert_eq!(recorder.record(&second).unwrap(), RecordOutcome::Inserted);
    let connection = Connection::open(recorder.path()).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn stores_all_purposes_and_termination_outcomes_without_payloads() {
    let (_directory, recorder) = recorder();
    let cases = [
        ("interactive", "agent", RequestOutcome::Completed),
        ("goal", "goal", RequestOutcome::Completed),
        ("delegated", "subagent", RequestOutcome::Failed),
        ("compact", "compaction", RequestOutcome::Cancelled),
        ("title", "title", RequestOutcome::Completed),
    ];
    for (id, purpose, outcome) in cases {
        let mut event = event(id, outcome);
        event.purpose = purpose.to_owned();
        recorder.record(&event).unwrap();
    }
    let connection = Connection::open(recorder.path()).unwrap();
    let mut statement = connection
        .prepare("SELECT event_id, purpose, request_outcome FROM usage_events ORDER BY event_id")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("compact".into(), "compaction".into(), "cancelled".into()),
            ("delegated".into(), "subagent".into(), "failed".into()),
            ("goal".into(), "goal".into(), "completed".into()),
            ("interactive".into(), "agent".into(), "completed".into()),
            ("title".into(), "title".into(), "completed".into()),
        ]
    );
}

#[test]
fn concurrent_initializers_migrate_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = Arc::new(directory.path().join("usage.sqlite3"));
    let handles = (0..6)
        .map(|_| {
            let path = Arc::clone(&path);
            thread::spawn(move || SqliteUsageRecorder::new(path.as_path()))
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let connection = Connection::open(path.as_path()).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn concurrent_writers_and_read_only_client_share_wal_database() {
    let (_directory, recorder) = recorder();
    let recorder = Arc::new(recorder);
    let read_only =
        Connection::open_with_flags(recorder.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let handles = (0..6)
        .map(|writer| {
            let recorder = Arc::clone(&recorder);
            thread::spawn(move || {
                for request in 0..10 {
                    recorder
                        .record(&event(
                            &format!("writer-{writer}-{request}"),
                            RequestOutcome::Completed,
                        ))
                        .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    let count: i64 = read_only
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 60);
}

#[test]
fn waits_for_short_write_lock_contention() {
    let (_directory, recorder) = recorder();
    let locker = Connection::open(recorder.path()).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let writer =
        thread::spawn(move || recorder.record(&event("after-lock", RequestOutcome::Completed)));
    thread::sleep(Duration::from_millis(100));
    locker.execute_batch("COMMIT").unwrap();
    assert_eq!(writer.join().unwrap().unwrap(), RecordOutcome::Inserted);
}

#[cfg(unix)]
#[test]
fn applies_private_unix_permissions_to_new_ledger_paths() {
    use std::os::unix::fs::PermissionsExt;
    let (_directory, recorder) = recorder();
    let file_mode = std::fs::metadata(recorder.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let directory_mode = std::fs::metadata(recorder.path().parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);
    assert_eq!(directory_mode, 0o700);
}

#[cfg(unix)]
#[test]
fn custom_path_preserves_pre_existing_parent_and_sibling_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("shared");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o750)).unwrap();
    let sibling = parent.join("sibling");
    std::fs::write(&sibling, "keep these permissions").unwrap();
    std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o640)).unwrap();

    let recorder = SqliteUsageRecorder::new(parent.join("usage.sqlite3")).unwrap();

    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&parent), 0o750);
    assert_eq!(mode(&sibling), 0o640);
    assert_eq!(mode(recorder.path()), 0o600);
}

#[test]
fn sanitized_v1_fixture_is_readable() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/usage-v1.sqlite3");
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let row: (String, String, String, i64) = connection
        .query_row(
            "SELECT provider, model, purpose, total_tokens FROM usage_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "fixture-provider".into(),
            "fixture-model".into(),
            "agent".into(),
            42
        )
    );
}
