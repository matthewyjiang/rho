//! CLI handler for `rho doctor`.
//!
//! Runs the same checks as the interactive `/doctor` overlay without a TUI.
//! Root `--provider`/`--model`/`--auth`/`--reasoning` apply in memory so the
//! report matches `rho --provider X run`. Probes run concurrently under one
//! deadline so a hung endpoint cannot stall the report.

use std::{sync::Arc, time::Duration};

use rho_providers::credentials::{available_auth_modes, CredentialStore};
use serde::Serialize;

use super::config_repository::ConfigRepository;
use crate::{
    cli::Cli,
    credential_store::AppCredentialStore,
    doctor::{
        build_report, plan_probes, probe_checks, run_probe, text_report, DoctorInputs,
        DoctorProbeGate, DoctorProbeId, DoctorProbeOutcome, DoctorReport, DoctorSection,
        DoctorSummary, HerdrProbe, ProbePlaceholder,
    },
    herdr::HerdrReporter,
    plugins::{self, ProjectTrust},
    tools::mcp::{McpLoadMode, McpSessionReport},
};

/// Wall-clock budget for every probe together. Longer than the Claude probe
/// timeout so a slow binary reports its own error instead of `timed out`.
const PROBE_DEADLINE: Duration = Duration::from_secs(20);

pub(super) async fn run(json: bool, cli: &Cli) -> anyhow::Result<()> {
    let config_repository = ConfigRepository::new(cli.config.clone());
    let mut config = config_repository.load()?;
    // Register every [providers.custom.*] name before the checks read the
    // provider list. Early dispatch runs before `prepare_startup` does it.
    config.providers.activate()?;
    let config_path = super::bootstrap::absolute_config_path(&config_repository)?;
    // The store defaults to the OS backend when unset. A store that cannot be
    // opened is reported by the authentication rows instead of aborting.
    if let Err(error) = crate::credential_store::initialize_from_config(&mut config, &config_path) {
        eprintln!("warning: credential store not initialized: {error:#}");
    }
    // Honor root --provider/--model/--auth/--reasoning for this invocation.
    // Persistence stays with `--save` in `prepare_startup`; doctor never writes.
    super::cli_config::apply_overrides(&mut config, cli)?;
    let store: Arc<dyn CredentialStore> = Arc::new(AppCredentialStore);

    let cwd = std::env::current_dir()?;
    let home = crate::paths::home_dir();
    let rho_home = crate::paths::rho_dir()?;
    // One discovery feeds both the MCP inventory and the plugin row so the
    // report matches what a session would load.
    let discovery = plugins::discover_with_trust(
        &cwd,
        home.as_deref(),
        Some(&rho_home),
        ProjectTrust::from_plugins_env(),
    );
    plugins::log(&discovery.report);
    let mut mcp_config = config.mcp.clone();
    mcp_config.merge(discovery.mcp);
    let mcp_report = McpSessionReport::from_config_unloaded(&mcp_config, McpLoadMode::Native);

    let available_auths = available_auth_modes(store.as_ref());
    let clipboard = crate::clipboard::doctor_report();
    let probes = plan_probes(&config, &config.provider, DoctorProbeGate::Live);
    let mut report = build_report(DoctorInputs {
        provider: &config.provider,
        model: &config.model,
        auth: &config.auth,
        available_auths: &available_auths,
        credential_store: store.as_ref(),
        config_path: &config_path,
        session_root: &rho_home.join("sessions"),
        herdr: HerdrProbe::from_reporter(&HerdrReporter::from_env()),
        clipboard: &clipboard,
        mcp_report: &mcp_report,
        plugins_report: &discovery.report,
        probes: &probes,
        placeholder: ProbePlaceholder::Checking,
    });
    let handles = probes
        .into_iter()
        .map(|id| (id.clone(), tokio::spawn(run_probe(id, store.clone()))))
        .collect();
    for outcome in collect_probes(handles, PROBE_DEADLINE).await {
        report.replace_checks(probe_checks(&outcome));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&DoctorDocument::new(&report))?
        );
    } else {
        print!("{}", text_report::render(&report));
    }
    let failing = report.summary().fail;
    if failing > 0 {
        anyhow::bail!(
            "{failing} check{} failing",
            if failing == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// Collect every spawned probe under one shared deadline. A probe that
/// outlives it is aborted and reported as timed out.
async fn collect_probes(
    handles: Vec<(DoctorProbeId, tokio::task::JoinHandle<DoctorProbeOutcome>)>,
    deadline: Duration,
) -> Vec<DoctorProbeOutcome> {
    let deadline = tokio::time::Instant::now() + deadline;
    let mut outcomes = Vec::with_capacity(handles.len());
    for (id, mut handle) in handles {
        outcomes.push(match tokio::time::timeout_at(deadline, &mut handle).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => DoctorProbeOutcome::Failed(id),
            Err(_) => {
                handle.abort();
                DoctorProbeOutcome::TimedOut(id)
            }
        });
    }
    outcomes
}

#[derive(Serialize)]
struct DoctorDocument<'a> {
    summary: DoctorSummary,
    sections: &'a [DoctorSection],
}

impl<'a> DoctorDocument<'a> {
    fn new(report: &'a DoctorReport) -> Self {
        Self {
            summary: report.summary(),
            sections: &report.sections,
        }
    }
}

#[cfg(test)]
#[path = "doctor_cli_tests.rs"]
mod tests;
