//! Serializable views of the hook configuration and recent hook activity:
//! [`HookReport`] for diagnostics and the `/hooks` overlay, and the typed
//! report a daemon session answers, both read from one [`HookSnapshot`].
//!
//! A daemon session's report reads its hooks files again and checks them
//! against the hooks the session runs ([`HookSnapshot::checked`]); it never
//! installs what it read. The `rho` tool's report is the running hooks alone.
//!
//! Kept next to the hook types rather than in the diagnostics module so the
//! hook contract stays explicit. Diagnostics and the `/hooks` overlay consume
//! that data; they do not decide which programs run.

use std::{path::PathBuf, sync::Arc};

use rho_protocol::{
    HookActivityRecord, HookContract, HookStatus, HookTools, HookWarning, InvalidHooksFile,
    SessionHooksReport, SkippedHooksFile,
};
use rho_sdk::hooks::HookEventKind;
use serde::{Deserialize, Serialize};

use super::{
    activity::{HookActivity, HookOutcome},
    catalog::{HookCatalog, HookSpawnContract},
    config::{ConfiguredHookEvent, HookConfigError, WorkflowHookEventKind},
    dispatch::HookEngine,
    HookPipeline, HookPorts,
};

/// What a hook will execute, ready to show before trust is granted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookContractView {
    /// Whether the session runs it now: false for a pending hook, true for
    /// a stale one.
    pub active: bool,
    /// How it differs from what the session runs, when its file changed
    /// since the session loaded its hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<HookDrift>,
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

/// How a listed hook differs from what its session runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookDrift {
    /// Its file defines it, but the session does not run it yet.
    Pending,
    /// Its file defines it, but the session started without a hook of its
    /// kind, so it runs only once the session is opened again.
    NeedsReopen,
    /// The session still runs it, but its file no longer defines it so.
    Stale,
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
    /// A hooks file that loads but could not be read or validated; the
    /// session keeps the hooks listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid: Option<InvalidHooksFileView>,
    pub hooks: Vec<HookContractView>,
    pub recent_activity: Vec<HookActivityView>,
}

/// A hooks file that does not load, as [`HookReport`] shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidHooksFileView {
    pub path: String,
    /// What is wrong, without the path.
    pub error: String,
}

/// What a session's hooks are and did, read at one moment: the source of
/// both [`HookReport`] and the typed report a daemon session answers
/// (`SessionReport::Hooks`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSnapshot {
    enabled: bool,
    files: Vec<PathBuf>,
    skipped_untrusted: Option<SkippedHooksFile>,
    invalid: Option<InvalidHooksFile>,
    hooks: Vec<ListedHook>,
    activity: Vec<HookActivity>,
}

/// One hook a report lists, and whether its session runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ListedHook {
    contract: HookSpawnContract,
    status: ListedStatus,
}

/// Whether a session runs a listed hook, as [`HookStatus`] tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListedStatus {
    Active,
    Untrusted,
    Pending,
    NeedsReopen,
    Stale,
}

impl HookSnapshot {
    /// A session with no hook runtime.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            files: Vec::new(),
            skipped_untrusted: None,
            invalid: None,
            hooks: Vec::new(),
            activity: Vec::new(),
        }
    }

    /// The hooks files as read now (`files`), checked against the hooks a
    /// session running `running` runs; `None` for a session without a hook
    /// runtime.
    ///
    /// A hook the files define is active when the session runs it exactly
    /// so; otherwise it is pending when a reload would load it, and needs a
    /// reopen when the files define a kind of hook the session started
    /// without (see [`HookPorts`]), which refuses the whole reload. One the
    /// session runs that the files no longer define is stale. Files that do
    /// not load are reported as `invalid` with the hooks the session runs.
    pub(super) fn checked(
        running: Option<&HookPipeline>,
        files: Result<HookCatalog, HookConfigError>,
    ) -> Self {
        let ports = HookPorts::of(running);
        let current = running.map(|hooks| hooks.engine().catalog());
        let activity = running.map_or_else(Vec::new, |hooks| hooks.engine().activity());
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                return Self {
                    enabled: running.is_some(),
                    invalid: Some(InvalidHooksFile {
                        path: error.path.clone(),
                        error: error.detail(),
                    }),
                    activity,
                    ..current
                        .as_deref()
                        .map_or_else(Self::disabled, Self::running)
                };
            }
        };
        let runs: Vec<HookSpawnContract> = current
            .map(|catalog| {
                catalog
                    .spawn_contract()
                    .into_iter()
                    .filter(|contract| contract.active)
                    .collect()
            })
            .unwrap_or_default();
        let reloadable = ports.missing(&files).is_none();
        let defined = files.spawn_contract();
        let stale: Vec<ListedHook> = runs
            .iter()
            .filter(|contract| !defined.contains(contract))
            .map(|contract| ListedHook {
                contract: contract.clone(),
                status: ListedStatus::Stale,
            })
            .collect();
        let hooks = defined
            .into_iter()
            .map(|contract| {
                let status = if !contract.active {
                    ListedStatus::Untrusted
                } else if runs.contains(&contract) {
                    ListedStatus::Active
                } else if reloadable {
                    ListedStatus::Pending
                } else {
                    ListedStatus::NeedsReopen
                };
                ListedHook { contract, status }
            })
            .chain(stale)
            .collect();
        Self {
            enabled: running.is_some(),
            files: files.files().to_vec(),
            skipped_untrusted: skipped_untrusted(&files),
            invalid: None,
            hooks,
            activity,
        }
    }

    /// The hooks `catalog` runs, with no activity.
    fn running(catalog: &HookCatalog) -> Self {
        Self {
            enabled: true,
            files: catalog.files().to_vec(),
            skipped_untrusted: skipped_untrusted(catalog),
            invalid: None,
            hooks: catalog
                .spawn_contract()
                .into_iter()
                .map(|contract| ListedHook {
                    status: if contract.active {
                        ListedStatus::Active
                    } else {
                        ListedStatus::Untrusted
                    },
                    contract,
                })
                .collect(),
            activity: Vec::new(),
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
            invalid: self.invalid.as_ref().map(|invalid| InvalidHooksFileView {
                path: crate::paths::display(&invalid.path),
                error: invalid.error.clone(),
            }),
            hooks: self.hooks.iter().map(contract_view).collect(),
            recent_activity: self.activity.iter().map(HookActivityView::from).collect(),
        }
    }

    /// The typed report a daemon session answers.
    pub fn session_report(&self) -> SessionHooksReport {
        SessionHooksReport {
            enabled: self.enabled,
            files: self.files.clone(),
            skipped_untrusted: self.skipped_untrusted.clone(),
            invalid: self.invalid.clone(),
            hooks: self.hooks.iter().map(listed_contract).collect(),
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

    /// The hooks the session runs and what they did.
    pub fn snapshot(&self) -> HookSnapshot {
        HookSnapshot {
            activity: self.engine.activity(),
            ..HookSnapshot::running(&self.engine.catalog())
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

/// Renders one listed hook's resolved spawn contract.
fn contract_view(hook: &ListedHook) -> HookContractView {
    let contract = &hook.contract;
    let (active, drift) = match hook.status {
        ListedStatus::Active => (true, None),
        ListedStatus::Untrusted => (false, None),
        ListedStatus::Pending => (false, Some(HookDrift::Pending)),
        ListedStatus::NeedsReopen => (false, Some(HookDrift::NeedsReopen)),
        ListedStatus::Stale => (true, Some(HookDrift::Stale)),
    };
    HookContractView {
        active,
        drift,
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

/// A listed hook as the typed report carries it.
fn listed_contract(hook: &ListedHook) -> HookContract {
    let status = match hook.status {
        ListedStatus::Active => HookStatus::Active,
        ListedStatus::Untrusted => HookStatus::Untrusted,
        ListedStatus::Pending => HookStatus::Pending,
        ListedStatus::NeedsReopen => HookStatus::NeedsReopen,
        ListedStatus::Stale => HookStatus::Stale,
    };
    contract(&hook.contract, status)
}

/// `contract` as the typed reports carry it, with `status`.
pub(super) fn contract(contract: &HookSpawnContract, status: HookStatus) -> HookContract {
    HookContract {
        id: contract.id.clone(),
        status,
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
