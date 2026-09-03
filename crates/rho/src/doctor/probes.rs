//! Doctor probes: checks that spawn a child process or touch the network.
//!
//! Each probe is identified so its `Checking` placeholder rows can be replaced
//! when it finishes. Hosts spawn [`run_probe`] and poll the task; nothing here
//! blocks the caller.

use std::{process::Stdio, sync::Arc, time::Duration};

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
use crate::{
    claude_runtime::auth::ClaudeProbeSnapshot,
    config::Config,
    cursor_runtime::auth::{CursorAuthError, CursorAuthStatus},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DoctorProbeId {
    ProviderEndpoint {
        provider: String,
        endpoint: Url,
    },
    /// `claude auth status` and `claude --version`.
    Claude,
    /// `cursor-agent status --format json` and `cursor-agent --version`.
    Cursor,
    /// `rtk --version`.
    Rtk,
}

/// Cursor Agent binary and auth as seen by doctor.
#[derive(Debug)]
pub(crate) struct CursorProbeSnapshot {
    pub(crate) auth: Result<CursorAuthStatus, CursorAuthError>,
    pub(crate) version: Option<String>,
}

#[derive(Debug)]
pub(crate) enum DoctorProbeOutcome {
    ProviderEndpoint {
        provider: String,
        health: ProviderModelHealth,
    },
    Claude(ClaudeProbeSnapshot),
    Cursor(CursorProbeSnapshot),
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

/// Probes this host should spawn: the active OpenAI-compatible endpoint when
/// it is probeable, any other host the user configured an endpoint for, plus
/// the Claude Code and rtk binaries. Built-in defaults for unused keyless
/// hosts are not probed; those would fail `rho doctor` on machines that never
/// run them.
///
/// `active_provider` is the host the session is using. That can differ from
/// [`Config::provider`] when a `--provider` override was not saved or a model
/// switch did not persist.
pub(crate) fn plan_probes(
    config: &Config,
    active_provider: &str,
    gate: DoctorProbeGate,
) -> Vec<DoctorProbeId> {
    match gate {
        DoctorProbeGate::Disabled => return Vec::new(),
        DoctorProbeGate::Live => {}
    }
    let mut probes = provider::providers()
        .iter()
        .filter(|descriptor| {
            descriptor.probes_configured_endpoint()
                && (descriptor.name == active_provider
                    || config
                        .configured_provider_endpoint(descriptor.name)
                        .is_some())
        })
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
    probes.push(DoctorProbeId::Cursor);
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
        DoctorProbeId::Cursor => DoctorProbeOutcome::Cursor(CursorProbeSnapshot {
            auth: crate::cursor_runtime::auth::query().await,
            version: crate::cursor_runtime::auth::version().await.ok(),
        }),
        DoctorProbeId::Rtk => DoctorProbeOutcome::Rtk {
            available: probe_rtk().await,
        },
    }
}

/// Same budget as `rho_tools` rewrite probes. A hung `--version` must not
/// consume the whole `rho doctor` deadline.
const RTK_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

async fn probe_rtk() -> bool {
    let mut command = tokio::process::Command::new("rtk");
    command.arg("--version");
    probe_rtk_command(command).await
}

async fn probe_rtk_command(mut command: tokio::process::Command) -> bool {
    use tokio::io::AsyncReadExt;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let mut stdout = child.stdout.take();
    let collect = async {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        let status = child.wait().await.ok()?;
        Some((status.success(), buf))
    };
    match tokio::time::timeout(RTK_PROBE_TIMEOUT, collect).await {
        Ok(Some((true, buf))) => rtk_version_supports_rewrite(&String::from_utf8_lossy(&buf)),
        Ok(_) => false,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            false
        }
    }
}

/// Keep in sync with `rho_tools::rtk` version gating. Lives here so doctor
/// does not need a new published tools crate just to abort a hung child.
fn rtk_version_supports_rewrite(version: &str) -> bool {
    let version = version
        .trim()
        .strip_prefix("rtk ")
        .unwrap_or(version.trim());
    let mut parts = version.split('.');
    let (Some(Ok(major)), Some(Ok(minor)), Some(Ok(_patch))) = (
        parts.next().map(str::parse::<u64>),
        parts.next().map(str::parse::<u64>),
        parts
            .next()
            .and_then(|part| part.split_whitespace().next())
            .map(str::parse::<u64>),
    ) else {
        return true;
    };
    major > 0 || minor >= 23
}

/// Rows a probe will fill in, as `Checking` placeholders until it finishes.
pub(crate) fn placeholder_checks(id: &DoctorProbeId) -> Vec<DoctorCheck> {
    probe_rows(id)
        .into_iter()
        .map(|(check_id, label)| {
            DoctorCheck::new(check_id, label, DoctorStatus::Checking, "checking")
        })
        .collect()
}

/// Rows that replace a probe's placeholders.
pub(crate) fn probe_checks(
    outcome: &DoctorProbeOutcome,
    active_provider: &str,
) -> Vec<DoctorCheck> {
    match outcome {
        DoctorProbeOutcome::ProviderEndpoint { provider, health } => {
            vec![checks::endpoint_check(provider, health, active_provider)]
        }
        DoctorProbeOutcome::Claude(snapshot) => checks::claude_checks(snapshot),
        DoctorProbeOutcome::Cursor(snapshot) => vec![checks::cursor_check(
            &snapshot.auth,
            snapshot.version.as_deref(),
        )],
        DoctorProbeOutcome::Rtk { available } => vec![checks::rtk_check(*available)],
        DoctorProbeOutcome::Failed(id) => failed_rows(
            id,
            active_provider,
            "probe failed",
            "the probe stopped before it produced a result",
        ),
        DoctorProbeOutcome::TimedOut(id) => failed_rows(
            id,
            active_provider,
            "timed out",
            "the probe did not finish in time",
        ),
    }
}

/// Active endpoint timeouts and panics fail CI the same way `Unreachable`
/// does. Unused configured hosts stay informational; Claude and rtk stay
/// warnings because they are optional runtimes.
fn failed_rows(
    id: &DoctorProbeId,
    active_provider: &str,
    summary: &str,
    hint: &str,
) -> Vec<DoctorCheck> {
    let status = match id {
        DoctorProbeId::ProviderEndpoint { provider, .. } if provider == active_provider => {
            DoctorStatus::Fail
        }
        DoctorProbeId::ProviderEndpoint { .. } => DoctorStatus::Info,
        DoctorProbeId::Claude | DoctorProbeId::Rtk => DoctorStatus::Warn,
        DoctorProbeId::Cursor => DoctorStatus::Info,
    };
    probe_rows(id)
        .into_iter()
        .map(|(check_id, label)| DoctorCheck::new(check_id, label, status, summary).with_hint(hint))
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
        DoctorProbeId::Cursor => vec![(DoctorCheckId::Cursor, checks::CURSOR_LABEL.into())],
        DoctorProbeId::Rtk => vec![(DoctorCheckId::Rtk, checks::RTK_LABEL.into())],
    }
}

#[cfg(test)]
#[path = "probes_tests.rs"]
mod tests;
