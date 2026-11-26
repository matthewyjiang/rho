//! Serializable views of the hook configuration and recent hook activity:
//! [`HookReport`] for diagnostics and the `/hooks` overlay, and the typed
//! report a daemon session answers, both read from one [`HookSnapshot`].
//!
//! Kept next to the hook types rather than in the diagnostics module so the
//! hook contract stays explicit. Diagnostics and the `/hooks` overlay consume
//! that data; they do not decide which programs run.

use std::{path::PathBuf, sync::Arc};

use rho_protocol::{
    HookActivityRecord, HookContract, HookStatus, HookTools, HookWarning, SessionHooksReport,
    SkippedHooksFile,
};
use rho_sdk::hooks::HookEventKind;
use serde::{Deserialize, Serialize};

use super::{
    activity::{HookActivity, HookOutcome},
    catalog::{HookCatalog, HookSpawnContract},
    config::{ConfiguredHookEvent, WorkflowHookEventKind},
    dispatch::HookEngine,
    HookPipeline,
};

/// What a hook will execute, ready to show before trust is granted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookContractView {
    pub active: bool,
    pub id: String,
    pub event: String,
    pub tools: String,
    pub command: Vec<String>,
    pub working_directory: String,
    pub timeout: String,
    pub environment: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// One recorded hook invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookActivityView {
    pub hook: String,
    pub event: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl From<&HookActivity> for HookActivityView {
    fn from(activity: &HookActivity) -> Self {
        Self {
            hook: activity.hook_label().to_owned(),
            event: activity.event.wire_name().to_owned(),
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookReport {
    pub enabled: bool,
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_untrusted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_untrusted_error: Option<String>,
    pub hooks: Vec<HookContractView>,
    pub recent_activity: Vec<HookActivityView>,
}

/// What a session's hooks are and did, read at one moment: the source of
/// both [`HookReport`] and the typed report a daemon session answers
/// (`SessionReport::Hooks`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSnapshot {
    enabled: bool,
    files: Vec<PathBuf>,
    skipped_untrusted: Option<SkippedHooksFile>,
    contracts: Vec<HookSpawnContract>,
    activity: Vec<HookActivity>,
}

impl HookSnapshot {
    /// A session with no hook runtime.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            files: Vec::new(),
            skipped_untrusted: None,
            contracts: Vec::new(),
            activity: Vec::new(),
        }
    }

    /// A session with no hook runtime whose workspace has a project hooks
    /// file it does not trust: `catalog` holds that file, and no active hook.
    pub fn untrusted_only(catalog: &HookCatalog) -> Self {
        Self {
            skipped_untrusted: skipped_untrusted(catalog),
            contracts: catalog.spawn_contract(),
            ..Self::disabled()
        }
    }

    /// The serializable view `rho(action="hooks")` and the `/hooks` overlay
    /// show.
    pub fn report(&self) -> HookReport {
        HookReport {
            enabled: self.enabled,
            files: self
                .files
                .iter()
                .map(|path| crate::paths::display(path))
                .collect(),
            skipped_untrusted: self
                .skipped_untrusted
                .as_ref()
                .map(|skipped| crate::paths::display(&skipped.path)),
            skipped_untrusted_error: self
                .skipped_untrusted
                .as_ref()
                .and_then(|skipped| skipped.error.clone()),
            hooks: self.contracts.iter().map(contract_view).collect(),
            recent_activity: self.activity.iter().map(HookActivityView::from).collect(),
        }
    }

    /// The typed report a daemon session answers.
    pub fn session_report(&self) -> SessionHooksReport {
        SessionHooksReport {
            enabled: self.enabled,
            files: self.files.clone(),
            skipped_untrusted: self.skipped_untrusted.clone(),
            hooks: self.contracts.iter().map(contract).collect(),
            recent_activity: self.activity.iter().map(activity_record).collect(),
        }
    }
}

fn skipped_untrusted(catalog: &HookCatalog) -> Option<SkippedHooksFile> {
    catalog.skipped_untrusted().map(|skipped| SkippedHooksFile {
        path: skipped.path.clone(),
        error: skipped.error().map(ToString::to_string),
    })
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

    pub fn snapshot(&self) -> HookSnapshot {
        let catalog = self.engine.catalog();
        HookSnapshot {
            enabled: true,
            files: catalog.files().to_vec(),
            skipped_untrusted: skipped_untrusted(&catalog),
            contracts: catalog.spawn_contract(),
            activity: self.engine.activity(),
        }
    }

    pub fn report(&self) -> HookReport {
        self.snapshot().report()
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

/// Renders one hook's resolved spawn contract.
fn contract_view(contract: &HookSpawnContract) -> HookContractView {
    HookContractView {
        active: contract.active,
        id: contract.id.clone(),
        event: contract.event.wire_name().to_owned(),
        tools: contract.tools.describe(),
        command: contract.command.clone(),
        working_directory: crate::paths::display(&contract.working_directory),
        timeout: humantime::format_duration(contract.timeout).to_string(),
        environment: contract.environment.clone(),
        warning: (!contract.missing_edit_tools.is_empty()).then(|| {
            format!(
                "skips some edit tools; add {} so this hook runs for every edit format",
                contract.missing_edit_tools.join(", ")
            )
        }),
    }
}

fn contract(contract: &HookSpawnContract) -> HookContract {
    HookContract {
        id: contract.id.clone(),
        status: if contract.active {
            HookStatus::Active
        } else {
            HookStatus::Untrusted
        },
        event: wire_event(contract.event),
        tools: contract
            .tools
            .patterns()
            .map_or(HookTools::All, |patterns| HookTools::Matching { patterns }),
        command: contract.command.clone(),
        working_directory: contract.working_directory.clone(),
        timeout_ms: u64::try_from(contract.timeout.as_millis()).unwrap_or(u64::MAX),
        environment: contract.environment.clone(),
        warnings: (!contract.missing_edit_tools.is_empty())
            .then(|| HookWarning::MissingEditTools {
                tools: contract
                    .missing_edit_tools
                    .iter()
                    .map(|tool| (*tool).to_owned())
                    .collect(),
            })
            .into_iter()
            .collect(),
    }
}

fn activity_record(activity: &HookActivity) -> HookActivityRecord {
    HookActivityRecord {
        hook: activity.hook_id.clone(),
        event: wire_event(activity.event),
        outcome: match &activity.outcome {
            HookOutcome::Continued => rho_protocol::HookOutcome::Continued,
            HookOutcome::Denied { reason } => rho_protocol::HookOutcome::Denied {
                reason: reason.clone(),
            },
            HookOutcome::Observed => rho_protocol::HookOutcome::Observed,
            HookOutcome::Failed { reason } => rho_protocol::HookOutcome::Failed {
                reason: reason.clone(),
            },
            HookOutcome::Dropped => rho_protocol::HookOutcome::Dropped,
        },
        duration_ms: activity
            .duration
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        output_truncated: activity.truncated,
    }
}

fn wire_event(event: ConfiguredHookEvent) -> rho_protocol::HookEvent {
    use rho_protocol::HookEvent as Wire;
    match event {
        ConfiguredHookEvent::Sdk(event) => match event {
            HookEventKind::SessionStarted => Wire::SessionStarted,
            HookEventKind::BeforeToolUse => Wire::BeforeToolUse,
            HookEventKind::AfterToolUse => Wire::AfterToolUse,
            HookEventKind::RunCompleted => Wire::RunCompleted,
            HookEventKind::RunFailed => Wire::RunFailed,
            HookEventKind::InputRequested => Wire::InputRequested,
            HookEventKind::SessionCompleted => Wire::SessionCompleted,
            HookEventKind::SessionFailed => Wire::SessionFailed,
            // An SDK event this rho does not name yet.
            _ => Wire::Unknown,
        },
        ConfiguredHookEvent::Workflow(event) => match event {
            WorkflowHookEventKind::Started => Wire::WorkflowStarted,
            WorkflowHookEventKind::NodeStarted => Wire::WorkflowNodeStarted,
            WorkflowHookEventKind::NodeFinished => Wire::WorkflowNodeFinished,
            WorkflowHookEventKind::Completed => Wire::WorkflowCompleted,
            WorkflowHookEventKind::Failed => Wire::WorkflowFailed,
            WorkflowHookEventKind::Cancelled => Wire::WorkflowCancelled,
        },
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
