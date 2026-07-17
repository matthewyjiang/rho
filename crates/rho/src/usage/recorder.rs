use super::{UsageEvent, UsageLedgerError};

/// Result of an idempotent ledger write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordOutcome {
    Inserted,
    Duplicate,
}

/// Synchronous, replaceable boundary for durable per-request accounting.
///
/// Implementations should finish the durable write before returning. Callers
/// must treat errors as accounting diagnostics rather than model-call errors.
pub trait UsageRecorder: Send + Sync {
    fn record(&self, event: &UsageEvent) -> Result<RecordOutcome, UsageLedgerError>;
}
