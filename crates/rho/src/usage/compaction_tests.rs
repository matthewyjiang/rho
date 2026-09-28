use pretty_assertions::assert_eq;
use rusqlite::Connection;

use super::super::SqliteUsageRecorder;
use crate::compaction_metrics::{
    CompactionRecord, CompactionRunOutcome, CompactionTier, CompactionTriggerKind, RecordIdentity,
    RereadStats, SummaryRequestPath,
};

fn record(next_prompt_tokens: Option<u64>, reread: Option<RereadStats>) -> CompactionRecord {
    CompactionRecord {
        identity: RecordIdentity {
            event_id: "compaction-1".into(),
            occurred_at_ms: 1_000,
            session_id: Some("session".into()),
            ..RecordIdentity::default()
        },
        outcome: CompactionRunOutcome::Completed,
        trigger: CompactionTriggerKind::Automatic,
        tier: Some(CompactionTier::TextSummary),
        request_path: Some(SummaryRequestPath::SessionHistory),
        model: Some("anthropic/claude-test".into()),
        elided_tool_results: 2,
        context_tokens: 90_000,
        prompt_tokens: Some(88_000),
        output_tokens: Some(1_200),
        cache_read_tokens: Some(80_000),
        cost_usd_micros: Some(4_500),
        latency_ms: 7_250,
        next_prompt_tokens,
        reread,
    }
}

type Row = (
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    Option<i64>,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn rows(recorder: &SqliteUsageRecorder) -> Vec<Row> {
    let connection = Connection::open(recorder.path()).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT outcome, trigger, tier, request_path, context_tokens, output_tokens,
                    cache_read_tokens, latency_ms, next_prompt_tokens, reread_tool_calls,
                    reread_repeated
             FROM compaction_events",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

// Covers: a record is written when the compactor returns and again when its
// follow-up ends, and the two writes may land in either order (they run on
// separate blocking tasks). Either way the row ends up with one copy of the
// immutable fields plus the follow-up fields.
// Owner: usage ledger compaction rows.
#[test]
fn follow_up_write_fills_the_row_in_either_order() {
    let reread = RereadStats {
        window: 24,
        tool_calls: 24,
        repeated: 3,
        tracked: 5,
    };
    let initial = record(None, None);
    let finished = record(Some(21_000), Some(reread));
    let expected = vec![(
        "completed".to_string(),
        "automatic".to_string(),
        Some("text_summary".to_string()),
        Some("session_history".to_string()),
        90_000,
        Some(1_200),
        Some(80_000),
        7_250,
        Some(21_000),
        Some(24),
        Some(3),
    )];
    for (case, writes) in [
        ("in order", [&initial, &finished]),
        ("reversed", [&finished, &initial]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let recorder = SqliteUsageRecorder::new(directory.path().join("usage.sqlite3")).unwrap();
        for write in writes {
            recorder.record_compaction(write).unwrap();
        }
        assert_eq!(rows(&recorder), expected, "{case}");
    }
}

// Covers: a v1 ledger written by an older Rho upgrades in place, keeping its
// usage rows and gaining the compaction table.
// Owner: usage ledger migrations.
#[test]
fn v1_ledger_upgrades_and_keeps_usage_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("usage.sqlite3");
    std::fs::copy(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/usage-v1.sqlite3"),
        &path,
    )
    .unwrap();

    let recorder = SqliteUsageRecorder::new(&path).unwrap();
    recorder.record_compaction(&record(None, None)).unwrap();

    let connection = Connection::open(&path).unwrap();
    let count = |table: &str| -> i64 {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(
        (version, count("usage_events"), count("compaction_events")),
        (2, 1, 1)
    );
}
