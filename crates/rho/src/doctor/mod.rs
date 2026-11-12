//! Local setup diagnostics shared by `/doctor` and `rho doctor`.
//!
//! [`build_report`] runs the instant checks and inserts placeholder rows for
//! the probes a host plans to run. The host then spawns [`run_probe`] for each
//! id and feeds [`probe_checks`] back through [`DoctorReport::replace_checks`].

mod checks;
mod daemon;
mod daemon_observe;
mod daemon_store;
mod probes;
mod report;
pub(crate) mod text_report;

use std::path::Path;

use rho_providers::{auth::login_dispatch::ProviderAuthentication, credentials::CredentialStore};

pub(crate) use checks::{mcp_check, HerdrProbe};
pub(crate) use probes::{
    plan_probes, probe_checks, run_probe, DoctorProbeGate, DoctorProbeId, DoctorProbeOutcome,
};
pub(crate) use report::{
    DoctorCheck, DoctorCheckId, DoctorReport, DoctorSection, DoctorStatus, DoctorSummary,
};

use rho_protocol::SessionMcpInventory;

use crate::{
    clipboard::ClipboardDoctorReport, config::InternalAgentModelConfig, plugins::PluginLoadReport,
    tools::mcp::McpSessionSummary,
};

/// The session's MCP servers, as the MCP row reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DoctorMcp {
    Summary(McpSessionSummary),
    /// A daemon session's servers, still on their way. The host replaces
    /// the row with [`mcp_check`] through [`DoctorReport::replace_checks`]
    /// once they arrive.
    Reading,
    /// Why a daemon session's servers could not be read, as the whole
    /// hint: the host's error already names what failed.
    Unreadable(String),
}

impl DoctorMcp {
    /// The answer to a read of a daemon session's inventory.
    pub(crate) fn of_inventory(inventory: Result<&SessionMcpInventory, &str>) -> Self {
        match inventory {
            Ok(inventory) => McpSessionSummary::of_inventory(inventory).map_or_else(
                || {
                    Self::Unreadable(
                        "the daemon reports an MCP mode this client does not know".into(),
                    )
                },
                Self::Summary,
            ),
            Err(error) => Self::Unreadable(error.to_owned()),
        }
    }
}

/// Everything the instant checks need, borrowed from the host.
pub(crate) struct DoctorInputs<'a> {
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
    pub(crate) auth: &'a str,
    pub(crate) available_auths: &'a [String],
    pub(crate) credential_store: &'a dyn CredentialStore,
    pub(crate) config_path: &'a Path,
    pub(crate) session_root: &'a Path,
    pub(crate) herdr: HerdrProbe,
    pub(crate) clipboard: &'a ClipboardDoctorReport,
    pub(crate) mcp: DoctorMcp,
    pub(crate) plugins_report: &'a PluginLoadReport,
    /// The permission screen's configured model, when it is not the classifier's.
    pub(crate) permission_screen: Option<&'a InternalAgentModelConfig>,
    /// `[behavior].rtk`. When off, the rtk row says so instead of probing.
    pub(crate) rtk_enabled: bool,
    /// Probes the host will run, rendered as `Checking` rows until they finish.
    pub(crate) probes: &'a [DoctorProbeId],
}

pub(crate) fn build_report(inputs: DoctorInputs<'_>) -> DoctorReport {
    let mut rows = checks::authentication_checks(
        inputs.credential_store,
        inputs.provider,
        inputs.auth,
        &ProviderAuthentication::has_environment_override,
    );
    for probe in inputs.probes {
        rows.extend(probes::placeholder_checks(probe));
    }
    rows.extend(checks::cache_checks());
    rows.push(checks::selected_model_check(
        inputs.provider,
        inputs.model,
        inputs.auth,
        inputs.available_auths,
    ));
    rows.extend(
        inputs
            .permission_screen
            .map(checks::permission_screen_check),
    );
    rows.push(checks::herdr_check(inputs.herdr));
    if !inputs.rtk_enabled {
        rows.push(checks::rtk_disabled_check());
    }
    rows.push(checks::path_check(
        DoctorCheckId::ConfigPath,
        "Configuration",
        inputs.config_path,
        checks::PathKind::File,
    ));
    rows.push(checks::path_check(
        DoctorCheckId::SessionRoot,
        "Sessions",
        inputs.session_root,
        checks::PathKind::Directory,
    ));
    rows.extend(checks::clipboard_checks(inputs.clipboard));
    rows.push(checks::mcp_check(&inputs.mcp));
    rows.push(checks::plugins_check(inputs.plugins_report));
    DoctorReport::from_checks(rows)
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}
