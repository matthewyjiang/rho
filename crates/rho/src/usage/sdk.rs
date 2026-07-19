use std::sync::atomic::{AtomicBool, Ordering};

use rho_sdk::{
    ProviderRequestOutcome, ProviderRequestUsageEvent, ProviderRequestUsageRecorder,
    ProviderRequestUsageRecorderError, ProviderRequestUsageRecorderFuture,
    ProviderRequestUsageRecording,
};

use super::{RequestOutcome, SqliteUsageRecorder, UsageEvent, UsageRecorder};

static INITIALIZATION_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);
static DEFAULT_RECORDING: tokio::sync::OnceCell<ProviderRequestUsageRecording> =
    tokio::sync::OnceCell::const_new();

pub(crate) async fn default_recording() -> ProviderRequestUsageRecording {
    if cfg!(test) {
        return ProviderRequestUsageRecording::default();
    }
    DEFAULT_RECORDING
        .get_or_init(initialize_default_recording)
        .await
        .clone()
}

async fn initialize_default_recording() -> ProviderRequestUsageRecording {
    let initialized = tokio::task::spawn_blocking(SqliteUsageRecorder::at_default_path).await;
    match initialized {
        Ok(Ok(recorder)) => ProviderRequestUsageRecording::new(recorder),
        Ok(Err(error)) => {
            warn_initialization_failure(&error.to_string());
            ProviderRequestUsageRecording::default()
        }
        Err(error) => {
            warn_initialization_failure(&format!("usage ledger task failed: {error}"));
            ProviderRequestUsageRecording::default()
        }
    }
}

fn warn_initialization_failure(error: &str) {
    if !INITIALIZATION_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
        eprintln!("warning: usage accounting is unavailable: {error}");
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
    let step_index = context
        .step_index()
        .map(u64::try_from)
        .transpose()
        .map_err(|_| ProviderRequestUsageRecorderError::new("usage step index exceeds u64"))?;
    let attempt_index = context
        .attempt_index()
        .map(u64::try_from)
        .transpose()
        .map_err(|_| ProviderRequestUsageRecorderError::new("usage attempt index exceeds u64"))?;
    let ledger_event = UsageEvent {
        event_id: event.event_id().to_owned(),
        occurred_at_ms,
        session_id: context.session_id().map(ToString::to_string),
        parent_session_id: context.parent_session_id().map(ToString::to_string),
        run_id: context.run_id().map(ToString::to_string),
        step_index,
        attempt_index,
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
