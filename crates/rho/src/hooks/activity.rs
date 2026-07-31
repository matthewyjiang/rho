//! Bounded record of what hooks did.
//!
//! Successful observational hooks are not worth scrollback, but every hook is
//! worth a diagnostic. This keeps a sanitized ring so `/diagnostics` can answer
//! "did my hook run, and what happened" without flooding a session.

use std::{collections::VecDeque, sync::Mutex, time::Duration};

/// How many records are retained before the oldest is dropped.
const MAX_RECORDS: usize = 256;

/// What one hook invocation resulted in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookOutcome {
    Continued,
    Denied {
        reason: String,
    },
    Observed,
    /// The hook's own machinery failed. Blocking hooks turn this into a denial.
    Failed {
        reason: String,
    },
    /// The observational queue was full and the event was dropped.
    Dropped,
}

impl HookOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Continued => "continued",
            Self::Denied { .. } => "denied",
            Self::Observed => "observed",
            Self::Failed { .. } => "failed",
            Self::Dropped => "dropped",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Denied { reason } | Self::Failed { reason } => Some(reason),
            Self::Continued | Self::Observed | Self::Dropped => None,
        }
    }
}

/// One sanitized hook invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookActivity {
    pub hook_id: String,
    pub event: &'static str,
    pub outcome: HookOutcome,
    pub duration: Option<Duration>,
    pub truncated: bool,
}

impl std::fmt::Display for HookActivity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {} {}",
            self.hook_id,
            self.event,
            self.outcome.label()
        )?;
        if let Some(duration) = self.duration {
            write!(formatter, " in {}ms", duration.as_millis())?;
        }
        if self.truncated {
            formatter.write_str(" (output truncated)")?;
        }
        if let Some(detail) = self.outcome.detail() {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

/// Thread-safe ring of recent hook activity.
#[derive(Debug, Default)]
pub struct HookActivityLog {
    records: Mutex<VecDeque<HookActivity>>,
}

impl HookActivityLog {
    pub fn record(&self, activity: HookActivity) {
        tracing::debug!(
            hook = %activity.hook_id,
            event = activity.event,
            outcome = activity.outcome.label(),
            "hook finished"
        );
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if records.len() == MAX_RECORDS {
            records.pop_front();
        }
        records.push_back(activity);
    }

    pub fn snapshot(&self) -> Vec<HookActivity> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
