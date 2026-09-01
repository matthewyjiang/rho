//! Local setup diagnostics shared by `/doctor` and `rho doctor`.
//!
//! [`build_report`] runs the instant checks and inserts placeholder rows for
//! the probes a host plans to run. The host then spawns [`run_probe`] for each
//! id and feeds [`probe_checks`] back through [`DoctorReport::replace_checks`].

mod checks;
mod probes;
mod report;
pub(crate) mod text_report;

use std::path::Path;

use rho_providers::{auth::login_dispatch::ProviderAuthentication, credentials::CredentialStore};

pub(crate) use checks::HerdrProbe;
pub(crate) use probes::{
    plan_probes, probe_checks, run_probe, DoctorProbeGate, DoctorProbeId, DoctorProbeOutcome,
    ProbePlaceholder,
};
pub(crate) use report::{
    DoctorCheck, DoctorCheckId, DoctorReport, DoctorSection, DoctorStatus, DoctorSummary,
};

use crate::{
    clipboard::ClipboardDoctorReport, plugins::PluginLoadReport, tools::mcp::McpSessionReport,
};

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
    pub(crate) mcp_report: &'a McpSessionReport,
    pub(crate) plugins_report: &'a PluginLoadReport,
    /// Probes the host will run (or skipped), rendered as placeholder rows.
    pub(crate) probes: &'a [DoctorProbeId],
    pub(crate) placeholder: ProbePlaceholder,
}

pub(crate) fn build_report(inputs: DoctorInputs<'_>) -> DoctorReport {
    let mut rows = checks::authentication_checks(
        inputs.credential_store,
        inputs.auth,
        &ProviderAuthentication::has_environment_override,
    );
    for probe in inputs.probes {
        rows.extend(probes::placeholder_checks(probe, inputs.placeholder));
    }
    rows.extend(checks::cache_checks());
    rows.push(checks::selected_model_check(
        inputs.provider,
        inputs.model,
        inputs.auth,
        inputs.available_auths,
    ));
    rows.push(checks::herdr_check(inputs.herdr));
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
    rows.push(checks::mcp_check(inputs.mcp_report));
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
