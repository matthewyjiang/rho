//! Serializable view of the hook configuration and recent hook activity.
//!
//! Kept next to the hook types rather than in the diagnostics module so the
//! hook contract stays explicit. Diagnostics and the `/hooks` overlay consume
//! that data; they do not decide which programs run.

use std::sync::Arc;

use serde::Serialize;

use super::{activity::HookActivity, catalog::HookCatalog, dispatch::HookEngine, HookPipeline};

/// What a hook will execute, ready to show before trust is granted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookContractView {
    pub active: bool,
    pub id: String,
    pub event: String,
    pub tools: String,
    pub command: Vec<String>,
    pub working_directory: String,
    pub timeout: String,
    pub environment: Vec<String>,
}

/// One recorded hook invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookActivityView {
    pub hook: String,
    pub event: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl From<&HookActivity> for HookActivityView {
    fn from(activity: &HookActivity) -> Self {
        Self {
            hook: activity.hook_id.clone(),
            event: activity.event.to_owned(),
            outcome: activity.outcome.label().to_owned(),
            duration_ms: activity
                .duration
                .map(|duration| duration.as_millis() as u64),
            truncated: activity.truncated,
            detail: activity.outcome.detail().map(str::to_owned),
        }
    }
}

/// Everything `rho(action="hooks")` reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookReport {
    pub enabled: bool,
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_untrusted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_untrusted_error: Option<String>,
    pub hooks: Vec<HookContractView>,
    pub recent_activity: Vec<HookActivityView>,
}

impl HookReport {
    /// Report for a session with no hook runtime.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            files: Vec::new(),
            skipped_untrusted: None,
            skipped_untrusted_error: None,
            hooks: Vec::new(),
            recent_activity: Vec::new(),
        }
    }
}

/// Live hook state a diagnostics surface can read at any time.
#[derive(Clone)]
pub struct HookInspector {
    engine: Arc<HookEngine>,
}

impl HookInspector {
    pub fn new(runtime: &HookPipeline) -> Self {
        Self {
            engine: Arc::clone(runtime.engine()),
        }
    }

    pub fn report(&self) -> HookReport {
        let catalog = self.engine.catalog();
        HookReport {
            enabled: true,
            files: catalog
                .files()
                .iter()
                .map(|path| crate::paths::display(path))
                .collect(),
            skipped_untrusted: catalog
                .skipped_untrusted()
                .map(|skipped| crate::paths::display(&skipped.path)),
            skipped_untrusted_error: catalog
                .skipped_untrusted()
                .and_then(|skipped| skipped.error())
                .map(ToString::to_string),
            hooks: contract_views(&catalog),
            recent_activity: self
                .engine
                .activity()
                .iter()
                .map(HookActivityView::from)
                .collect(),
        }
    }
}

impl std::fmt::Debug for HookInspector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookInspector")
            .field("hooks", &self.engine.catalog().len())
            .finish()
    }
}

/// Renders the resolved spawn contract for every hook in `catalog`.
pub fn contract_views(catalog: &HookCatalog) -> Vec<HookContractView> {
    catalog
        .spawn_contract()
        .into_iter()
        .map(|contract| HookContractView {
            active: contract.active,
            id: contract.id,
            event: contract.event.to_owned(),
            tools: contract.tools,
            command: contract.command,
            working_directory: crate::paths::display(&contract.working_directory),
            timeout: humantime::format_duration(contract.timeout).to_string(),
            environment: contract.environment,
        })
        .collect()
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
