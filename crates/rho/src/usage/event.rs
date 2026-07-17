use rho_sdk::model::ModelUsage;

/// The way a provider request terminated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl RequestOutcome {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Immutable accounting metadata for one provider request.
///
/// Construct this once per request and retain it when retrying a failed ledger
/// write. A separately billed provider retry must use a new event ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageEvent {
    pub event_id: String,
    pub occurred_at_ms: i64,
    pub session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub run_id: Option<String>,
    pub step_index: Option<u64>,
    pub attempt_index: Option<u64>,
    pub workspace_path: Option<String>,
    pub provider: String,
    pub model: String,
    pub purpose: String,
    pub outcome: RequestOutcome,
    pub usage: ModelUsage,
    pub rho_version: Option<String>,
}

impl UsageEvent {
    /// Creates an event with a random stable identity and the current UTC time.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        purpose: impl Into<String>,
        outcome: RequestOutcome,
        usage: ModelUsage,
    ) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at_ms: chrono::Utc::now().timestamp_millis(),
            session_id: None,
            parent_session_id: None,
            run_id: None,
            step_index: None,
            attempt_index: None,
            workspace_path: None,
            provider: provider.into(),
            model: model.into(),
            purpose: purpose.into(),
            outcome,
            usage,
            rho_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }
    }
}
