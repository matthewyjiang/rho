//! Durable per-compaction metrics in the usage ledger's `compaction_events`.

use rusqlite::params;

use crate::compaction_metrics::CompactionRecord;

use super::{sqlite::sqlite_integer, SqliteUsageRecorder, UsageLedgerError};

static DEFAULT_RECORDER: std::sync::OnceLock<Option<SqliteUsageRecorder>> =
    std::sync::OnceLock::new();

/// Saves `record` to the default ledger off the async runtime. Failures are
/// logged, never surfaced: metrics must not fail a compaction. Tests skip it.
pub(crate) fn save_compaction(record: Option<CompactionRecord>) {
    let Some(record) = record else {
        return;
    };
    if cfg!(test) {
        return;
    }
    let save = move || {
        let recorder = DEFAULT_RECORDER.get_or_init(|| {
            SqliteUsageRecorder::at_default_path()
                .inspect_err(|error| tracing::warn!(%error, "compaction metrics are unavailable"))
                .ok()
        });
        if let Some(recorder) = recorder {
            if let Err(error) = recorder.record_compaction(&record) {
                tracing::warn!(%error, "could not record compaction metrics");
            }
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => drop(runtime.spawn_blocking(save)),
        Err(_) => save(),
    }
}

impl SqliteUsageRecorder {
    /// Inserts `record`, or fills in follow-up fields of an existing row.
    ///
    /// A record is written when the compactor returns, again when the next
    /// prompt size is known, and again when its re-read window ends. Writes
    /// may land in any order, so each follow-up column keeps its first
    /// non-null value and every other column never changes.
    pub(crate) fn record_compaction(
        &self,
        record: &CompactionRecord,
    ) -> Result<(), UsageLedgerError> {
        let reread = record.reread;
        let integers = [
            sqlite_integer(
                "elided_tool_results",
                Some(record.elided_tool_results as u64),
            )?,
            sqlite_integer("context_tokens", Some(record.context_tokens))?,
            sqlite_integer("prompt_tokens", record.prompt_tokens)?,
            sqlite_integer("output_tokens", record.output_tokens)?,
            sqlite_integer("cache_read_tokens", record.cache_read_tokens)?,
            sqlite_integer("cost_usd_micros", record.cost_usd_micros)?,
            sqlite_integer("latency_ms", Some(record.latency_ms))?,
            sqlite_integer("next_prompt_tokens", record.next_prompt_tokens)?,
        ];
        let identity = &record.identity;
        self.db.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO compaction_events (
                    event_id, occurred_at_ms, session_id, parent_session_id, run_id,
                    workspace_path, outcome, trigger, tier, request_path, model,
                    elided_tool_results, context_tokens, prompt_tokens, output_tokens,
                    cache_read_tokens, cost_usd_micros, latency_ms, next_prompt_tokens,
                    reread_window, reread_tool_calls, reread_repeated, reread_tracked,
                    rho_version
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
                 )
                 ON CONFLICT (event_id) DO UPDATE SET
                    next_prompt_tokens = COALESCE(next_prompt_tokens, excluded.next_prompt_tokens),
                    reread_window = COALESCE(reread_window, excluded.reread_window),
                    reread_tracked = COALESCE(reread_tracked, excluded.reread_tracked),
                    reread_tool_calls = COALESCE(reread_tool_calls, excluded.reread_tool_calls),
                    reread_repeated = COALESCE(reread_repeated, excluded.reread_repeated)",
                params![
                    identity.event_id,
                    identity.occurred_at_ms,
                    identity.session_id,
                    identity.parent_session_id,
                    identity.run_id,
                    identity.workspace_path,
                    record.outcome.label(),
                    record.trigger.label(),
                    record.tier.map(|tier| tier.label()),
                    record.request_path.map(|path| path.label()),
                    record.model,
                    integers[0],
                    integers[1],
                    integers[2],
                    integers[3],
                    integers[4],
                    integers[5],
                    integers[6],
                    integers[7],
                    reread.map(|reread| reread.window),
                    reread.map(|reread| reread.tool_calls),
                    reread.map(|reread| reread.repeated),
                    reread.map(|reread| reread.tracked),
                    env!("CARGO_PKG_VERSION"),
                ],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
