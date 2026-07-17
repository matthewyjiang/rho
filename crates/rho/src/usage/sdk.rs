use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use rho_sdk::{
    ProviderRequestOutcome, ProviderRequestUsageEvent, ProviderRequestUsageRecorder,
    ProviderRequestUsageRecorderError, ProviderRequestUsageRecorderFuture,
};

use super::{RequestOutcome, SqliteUsageRecorder, UsageEvent, UsageRecorder};

static INITIALIZATION_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn default_recorder() -> Option<Arc<SqliteUsageRecorder>> {
    if cfg!(test) {
        return None;
    }
    match SqliteUsageRecorder::at_default_path() {
        Ok(recorder) => Some(Arc::new(recorder)),
        Err(error) => {
            if !INITIALIZATION_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
                eprintln!("warning: usage accounting is unavailable: {error}");
            }
            None
        }
    }
}

impl ProviderRequestUsageRecorder for SqliteUsageRecorder {
    fn record(&self, event: ProviderRequestUsageEvent) -> ProviderRequestUsageRecorderFuture<'_> {
        let recorder = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || record_sdk_event(&recorder, event))
                .await
                .map_err(|error| {
                    ProviderRequestUsageRecorderError::new(format!(
                        "usage ledger task failed: {error}"
                    ))
                })?
        })
    }
}

fn record_sdk_event(
    recorder: &SqliteUsageRecorder,
    event: ProviderRequestUsageEvent,
) -> Result<(), ProviderRequestUsageRecorderError> {
    let context = event.context();
    let identity = context.identity();
    let outcome = match event.outcome() {
        ProviderRequestOutcome::Completed => RequestOutcome::Completed,
        ProviderRequestOutcome::Cancelled => RequestOutcome::Cancelled,
        ProviderRequestOutcome::InvalidResponse | ProviderRequestOutcome::Failed(_) => {
            RequestOutcome::Failed
        }
        _ => RequestOutcome::Failed,
    };
    let occurred_at_ms = i64::try_from(event.timestamp_utc_ms()).map_err(|_| {
        ProviderRequestUsageRecorderError::new("usage event timestamp exceeds SQLite integer range")
    })?;
    let ledger_event = UsageEvent {
        event_id: event.event_id().to_owned(),
        occurred_at_ms,
        session_id: Some(context.session_id().to_string()),
        parent_session_id: None,
        run_id: Some(context.run_id().to_string()),
        step_index: Some(context.step_index() as u64),
        attempt_index: Some(context.attempt_index() as u64),
        workspace_path: context
            .workspace_path()
            .map(|path| path.to_string_lossy().into_owned()),
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        purpose: context.purpose().to_owned(),
        outcome,
        usage: event.usage().clone(),
        rho_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
    };
    UsageRecorder::record(recorder, &ledger_event)
        .map(|_| ())
        .map_err(|error| ProviderRequestUsageRecorderError::new(error.to_string()))
}
