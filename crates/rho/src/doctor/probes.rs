//! Doctor probes: checks that spawn a child process or touch the network.
//!
//! Each probe is identified so its `Checking` placeholder rows can be replaced
//! when it finishes. Hosts spawn [`run_probe`] and poll the task; nothing here
//! blocks the caller.

use std::sync::Arc;

use rho_providers::{
    credentials::CredentialStore,
    model::provider_models::{probe_provider_models, ProviderModelHealth},
    provider,
};
use url::Url;

use super::{
    checks,
    report::{DoctorCheck, DoctorCheckId, DoctorStatus},
};
use crate::{claude_runtime::auth::ClaudeProbeSnapshot, config::Config};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DoctorProbeId {
    ProviderEndpoint {
        provider: String,
        endpoint: Url,
    },
    /// `claude auth status` and `claude --version`.
    Claude,
    /// `rtk --version`.
    Rtk,
}

#[derive(Debug)]
pub(crate) enum DoctorProbeOutcome {
    ProviderEndpoint {
        provider: String,
        health: ProviderModelHealth,
    },
    Claude(ClaudeProbeSnapshot),
    Rtk {
        available: bool,
    },
    /// The task was aborted or panicked before producing a result.
    Failed(DoctorProbeId),
    /// The host stopped waiting.
    TimedOut(DoctorProbeId),
}

/// Whether live probes may run. `Disabled` keeps child processes and the
/// network out of unit tests, mirroring `/limits`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoctorProbeGate {
    Live,
    Disabled,
}

/// How planned-but-unfinished probe rows render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbePlaceholder {
    /// A task is running; the row is replaced when it finishes.
    Checking,
    /// The host chose not to probe.
    NotChecked,
}

/// Probes this host should spawn: one per provider with a resolvable
/// OpenAI-compatible endpoint, plus the Claude Code and rtk binaries.
pub(crate) fn plan_probes(config: &Config, gate: DoctorProbeGate) -> Vec<DoctorProbeId> {
    match gate {
        DoctorProbeGate::Disabled => return Vec::new(),
        DoctorProbeGate::Live => {}
    }
    let mut probes = provider::providers()
        .iter()
        .filter(|descriptor| descriptor.probes_configured_endpoint())
        .filter_map(|descriptor| {
            config
                .resolved_provider_endpoint(descriptor.name)
                .map(|endpoint| DoctorProbeId::ProviderEndpoint {
                    provider: descriptor.name.into(),
                    endpoint,
                })
        })
        .collect::<Vec<_>>();
    probes.push(DoctorProbeId::Claude);
    probes.push(DoctorProbeId::Rtk);
    probes
}

pub(crate) async fn run_probe(
    id: DoctorProbeId,
    store: Arc<dyn CredentialStore>,
) -> DoctorProbeOutcome {
    match id {
        DoctorProbeId::ProviderEndpoint { provider, endpoint } => {
            let health = probe_provider_models(&provider, &endpoint, store.as_ref()).await;
            DoctorProbeOutcome::ProviderEndpoint { provider, health }
        }
        DoctorProbeId::Claude => {
            DoctorProbeOutcome::Claude(crate::claude_runtime::auth::probe_snapshot().await)
        }
        // `rtk --version` is a blocking child run. A blocking task cannot be
        // aborted mid-run, but the command is short-lived.
        DoctorProbeId::Rtk => DoctorProbeOutcome::Rtk {
            available: tokio::task::spawn_blocking(rho_tools::rtk::is_available)
                .await
                .unwrap_or(false),
        },
    }
}

/// Rows a probe will fill in, in their pre-result state.
pub(crate) fn placeholder_checks(
    id: &DoctorProbeId,
    placeholder: ProbePlaceholder,
) -> Vec<DoctorCheck> {
    let (status, summary) = match placeholder {
        ProbePlaceholder::Checking => (DoctorStatus::Checking, "checking"),
        ProbePlaceholder::NotChecked => (DoctorStatus::Info, "not checked"),
    };
    probe_rows(id)
        .into_iter()
        .map(|(check_id, label)| DoctorCheck::new(check_id, label, status, summary))
        .collect()
}

/// Rows that replace a probe's placeholders.
pub(crate) fn probe_checks(outcome: &DoctorProbeOutcome) -> Vec<DoctorCheck> {
    match outcome {
        DoctorProbeOutcome::ProviderEndpoint { provider, health } => {
            vec![checks::endpoint_check(provider, health)]
        }
        DoctorProbeOutcome::Claude(snapshot) => checks::claude_checks(snapshot),
        DoctorProbeOutcome::Rtk { available } => vec![checks::rtk_check(*available)],
        DoctorProbeOutcome::Failed(id) => failed_rows(
            id,
            "probe failed",
            "the probe stopped before it produced a result",
        ),
        DoctorProbeOutcome::TimedOut(id) => {
            failed_rows(id, "timed out", "the probe did not finish in time")
        }
    }
}

fn failed_rows(id: &DoctorProbeId, summary: &str, hint: &str) -> Vec<DoctorCheck> {
    probe_rows(id)
        .into_iter()
        .map(|(check_id, label)| {
            DoctorCheck::new(check_id, label, DoctorStatus::Warn, summary).with_hint(hint)
        })
        .collect()
}

fn probe_rows(id: &DoctorProbeId) -> Vec<(DoctorCheckId, String)> {
    match id {
        DoctorProbeId::ProviderEndpoint { provider, .. } => vec![(
            DoctorCheckId::ProviderEndpoint {
                provider: provider.clone(),
            },
            checks::endpoint_label(provider),
        )],
        DoctorProbeId::Claude => vec![
            (DoctorCheckId::ClaudeAuth, checks::CLAUDE_AUTH_LABEL.into()),
            (
                DoctorCheckId::ClaudeBinary,
                checks::CLAUDE_BINARY_LABEL.into(),
            ),
        ],
        DoctorProbeId::Rtk => vec![(DoctorCheckId::Rtk, checks::RTK_LABEL.into())],
    }
}

#[cfg(test)]
#[path = "probes_tests.rs"]
mod tests;
