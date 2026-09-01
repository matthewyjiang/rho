//! Instant doctor checks: functions from injected inputs to [`DoctorCheck`]s.
//!
//! Nothing here spawns a process or touches the network; those live in
//! `probes`. Path checks touch the filesystem because writability is what
//! they verify.

use std::{fs, path::Path};

use rho_providers::{
    auth::login_dispatch::ProviderAuthentication,
    credentials::CredentialStore,
    model::{
        catalog,
        provider_models::{cached_provider_models, ProviderModelHealth},
    },
    provider::{self, ProviderAuthKind, ProviderModelSource},
};

use super::{
    plural_suffix,
    report::{DoctorCheck, DoctorCheckId, DoctorStatus},
};
use crate::{
    claude_runtime::auth::ClaudeProbeSnapshot,
    clipboard::ClipboardDoctorReport,
    herdr::HerdrReporter,
    plugins::{PluginLoadReport, PluginLoadSummary},
    tools::mcp::{McpLoadMode, McpSessionReport},
};

pub(super) const CLAUDE_AUTH_LABEL: &str = "Claude Code authentication";
pub(super) const CLAUDE_BINARY_LABEL: &str = "Claude Code binary";
pub(super) const RTK_LABEL: &str = "rtk";

/// One row per auth mode. Only the active mode warns when its key is
/// missing; other providers stay informational so the report scans.
/// `env_override` reports whether a mode's credentials come from the
/// environment; inject it so tests never read process env.
pub(super) fn authentication_checks(
    store: &dyn CredentialStore,
    active_auth: &str,
    env_override: &dyn Fn(&str) -> bool,
) -> Vec<DoctorCheck> {
    provider::providers()
        .iter()
        .flat_map(|descriptor| {
            if descriptor.is_keyless() {
                return vec![DoctorCheck::new(
                    DoctorCheckId::KeylessProvider {
                        provider: descriptor.name.into(),
                    },
                    format!("{} authentication", descriptor.display_name),
                    DoctorStatus::Info,
                    "not required",
                )];
            }
            descriptor
                .auth_modes()
                .filter(|mode| mode.auth_kind != ProviderAuthKind::None)
                .map(|mode| {
                    let id = DoctorCheckId::ProviderAuth {
                        auth_mode: mode.id.into(),
                    };
                    match ProviderAuthentication::has_credentials(store, mode.id) {
                        Ok(true) if env_override(mode.id) => DoctorCheck::new(
                            id,
                            mode.login_label,
                            DoctorStatus::Ok,
                            "authenticated via environment",
                        ),
                        Ok(true) => DoctorCheck::new(
                            id,
                            mode.login_label,
                            DoctorStatus::Ok,
                            "authenticated",
                        ),
                        Ok(false) if mode.id == active_auth => {
                            DoctorCheck::new(id, mode.login_label, DoctorStatus::Warn, "missing")
                                .with_hint(format!("run /login {}", mode.id))
                        }
                        Ok(false) if descriptor.has_none_auth() => {
                            DoctorCheck::new(id, mode.login_label, DoctorStatus::Info, "optional")
                        }
                        Ok(false) => {
                            DoctorCheck::new(id, mode.login_label, DoctorStatus::Info, "missing")
                        }
                        Err(error) => DoctorCheck::new(
                            id,
                            mode.login_label,
                            DoctorStatus::Fail,
                            "store unreadable",
                        )
                        .with_hint(format!(
                            "credential store could not be read: {error}; no secret values were inspected"
                        )),
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(super) fn cache_checks() -> Vec<DoctorCheck> {
    provider::providers()
        .iter()
        .filter(|descriptor| descriptor.model_source == ProviderModelSource::CachedProviderModels)
        .map(|descriptor| {
            let count = cached_provider_models(descriptor.name).len();
            let id = DoctorCheckId::ModelCache {
                provider: descriptor.name.into(),
            };
            let label = format!("{} model cache", descriptor.display_name);
            if count > 0 {
                DoctorCheck::new(
                    id,
                    label,
                    DoctorStatus::Ok,
                    format!("{count} model{}", plural_suffix(count)),
                )
            } else {
                DoctorCheck::new(id, label, DoctorStatus::Info, "empty")
            }
        })
        .collect()
}

pub(super) fn claude_checks(claude: &ClaudeProbeSnapshot) -> Vec<DoctorCheck> {
    let auth = match &claude.auth {
        Ok(status) if status.logged_in => DoctorCheck::new(
            DoctorCheckId::ClaudeAuth,
            CLAUDE_AUTH_LABEL,
            DoctorStatus::Ok,
            status.account_summary(),
        ),
        Ok(_) => DoctorCheck::new(
            DoctorCheckId::ClaudeAuth,
            CLAUDE_AUTH_LABEL,
            DoctorStatus::Warn,
            "not signed in",
        )
        .with_hint("run /login claude-code"),
        Err(error) => DoctorCheck::new(
            DoctorCheckId::ClaudeAuth,
            CLAUDE_AUTH_LABEL,
            DoctorStatus::Warn,
            "unavailable",
        )
        .with_hint(error.clone()),
    };
    let binary = match &claude.version {
        Ok(version) => DoctorCheck::new(
            DoctorCheckId::ClaudeBinary,
            CLAUDE_BINARY_LABEL,
            DoctorStatus::Ok,
            version.clone(),
        ),
        Err(error) => DoctorCheck::new(
            DoctorCheckId::ClaudeBinary,
            CLAUDE_BINARY_LABEL,
            DoctorStatus::Warn,
            "unavailable",
        )
        .with_hint(error.clone()),
    };
    vec![auth, binary]
}

pub(super) fn endpoint_label(provider: &str) -> String {
    let display_name = provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.display_name)
        .unwrap_or(provider);
    format!("{display_name} connection")
}

/// One row per probed host. Only the active provider fails or warns; unused
/// configured hosts, including `[providers.custom.*]`, stay informational so
/// `rho doctor` can gate CI on the selected setup.
pub(super) fn endpoint_check(
    provider: &str,
    health: &ProviderModelHealth,
    active_provider: &str,
) -> DoctorCheck {
    let id = DoctorCheckId::ProviderEndpoint {
        provider: provider.into(),
    };
    let label = endpoint_label(provider);
    let active = provider == active_provider;
    match health {
        ProviderModelHealth::ReachableWithModels { model_count } => DoctorCheck::new(
            id,
            label,
            DoctorStatus::Ok,
            format!(
                "reachable, {model_count} model{}",
                plural_suffix(*model_count)
            ),
        ),
        ProviderModelHealth::ReachableWithoutModels => DoctorCheck::new(
            id,
            label,
            if active {
                DoctorStatus::Warn
            } else {
                DoctorStatus::Info
            },
            "no models",
        )
        .with_hint("the endpoint is reachable but has no installed models"),
        ProviderModelHealth::Unreachable { error } => DoctorCheck::new(
            id,
            label,
            if active {
                DoctorStatus::Fail
            } else {
                DoctorStatus::Info
            },
            "unreachable",
        )
        .with_hint(error.clone()),
        ProviderModelHealth::InvalidResponse { error } => DoctorCheck::new(
            id,
            label,
            if active {
                DoctorStatus::Fail
            } else {
                DoctorStatus::Info
            },
            "invalid response",
        )
        .with_hint(error.clone()),
    }
}

pub(super) fn selected_model_check(
    provider: &str,
    model: &str,
    auth: &str,
    available_auths: &[String],
) -> DoctorCheck {
    let reference = provider::model_reference(provider, model);
    let available =
        catalog::resolve_model_selection_for_auths(&reference, provider, auth, available_auths)
            .is_ok();
    let id = DoctorCheckId::SelectedModel;
    if available {
        DoctorCheck::new(id, "Selected model", DoctorStatus::Ok, reference)
    } else {
        DoctorCheck::new(id, "Selected model", DoctorStatus::Fail, "unavailable")
            .with_hint(format!("{reference} using {auth} authentication"))
    }
}

#[derive(Clone, Copy)]
pub(super) enum PathKind {
    File,
    Directory,
}

pub(super) fn path_check(
    id: DoctorCheckId,
    label: &str,
    path: &Path,
    kind: PathKind,
) -> DoctorCheck {
    let status = if probe_writable(path, kind) {
        DoctorStatus::Ok
    } else {
        DoctorStatus::Fail
    };
    let summary = match status {
        DoctorStatus::Ok => "writable",
        DoctorStatus::Info | DoctorStatus::Warn | DoctorStatus::Fail | DoctorStatus::Checking => {
            "not writable"
        }
    };
    DoctorCheck::new(id, label, status, summary).with_hint(path.display().to_string())
}

pub(super) fn clipboard_checks(clipboard: &ClipboardDoctorReport) -> Vec<DoctorCheck> {
    let text_status = if clipboard.text_write_healthy {
        DoctorStatus::Ok
    } else {
        DoctorStatus::Warn
    };
    let image_status = if clipboard.image_healthy() {
        DoctorStatus::Ok
    } else {
        DoctorStatus::Warn
    };
    vec![
        DoctorCheck::new(
            DoctorCheckId::ClipboardText,
            "Clipboard text write",
            text_status,
            clipboard.text_write_status,
        )
        .with_hint(format!(
            "{} session; {}",
            clipboard.session_label, clipboard.text_write_detail
        )),
        DoctorCheck::new(
            DoctorCheckId::ClipboardImage,
            "Clipboard image helper",
            image_status,
            clipboard.image_status(),
        )
        .with_hint(clipboard.image_detail()),
    ]
}

pub(super) fn rtk_check(available: bool) -> DoctorCheck {
    if available {
        DoctorCheck::new(DoctorCheckId::Rtk, RTK_LABEL, DoctorStatus::Ok, "available")
    } else {
        DoctorCheck::new(
            DoctorCheckId::Rtk,
            RTK_LABEL,
            DoctorStatus::Info,
            "unavailable",
        )
        .with_hint("optional shell-command rewriting helper")
    }
}

/// Herdr socket state as seen from this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HerdrProbe {
    NotConfigured,
    Reachable,
    Unreachable,
    /// Configured, but reachability could not be determined.
    Unknown,
}

impl HerdrProbe {
    pub(crate) fn from_reporter(reporter: &HerdrReporter) -> Self {
        match (reporter.is_enabled(), reporter.socket_is_reachable()) {
            (false, _) => Self::NotConfigured,
            (true, Some(true)) => Self::Reachable,
            (true, Some(false)) => Self::Unreachable,
            (true, None) => Self::Unknown,
        }
    }
}

pub(super) fn herdr_check(probe: HerdrProbe) -> DoctorCheck {
    let id = DoctorCheckId::Herdr;
    match probe {
        HerdrProbe::NotConfigured => {
            DoctorCheck::new(id, "Herdr", DoctorStatus::Info, "not configured")
                .with_hint("Rho is not running inside Herdr")
        }
        HerdrProbe::Reachable => DoctorCheck::new(id, "Herdr", DoctorStatus::Ok, "connected"),
        HerdrProbe::Unreachable => DoctorCheck::new(id, "Herdr", DoctorStatus::Fail, "unreachable")
            .with_hint(
                "Herdr environment variables are set, but the socket did not accept a connection",
            ),
        HerdrProbe::Unknown => DoctorCheck::new(id, "Herdr", DoctorStatus::Warn, "unknown")
            .with_hint("Herdr is configured, but socket reachability could not be determined"),
    }
}

pub(super) fn mcp_check(report: &McpSessionReport) -> DoctorCheck {
    let summary = report.summary();
    let id = DoctorCheckId::Mcp;
    let label = "MCP";
    if !summary.configured {
        return DoctorCheck::new(id, label, DoctorStatus::Info, "not configured")
            .with_hint("no MCP servers under [mcp.servers]");
    }
    match summary.mode {
        McpLoadMode::Native if summary.connecting > 0 && summary.problems == 0 => {
            DoctorCheck::new(id, label, DoctorStatus::Info, "connecting").with_hint(format!(
                "{} connecting, {} connected, {} exported tool{}",
                summary.connecting,
                summary.connected,
                summary.exported_tools,
                plural_suffix(summary.exported_tools),
            ))
        }
        McpLoadMode::Native if summary.problems == 0 => {
            let (status, word) = if summary.connected > 0 {
                (DoctorStatus::Ok, "connected")
            } else {
                (DoctorStatus::Info, "idle")
            };
            DoctorCheck::new(id, label, status, word).with_hint(format!(
                "{} connected server{}, {} exported tool{}",
                summary.connected,
                plural_suffix(summary.connected),
                summary.exported_tools,
                plural_suffix(summary.exported_tools),
            ))
        }
        McpLoadMode::Native => DoctorCheck::new(id, label, DoctorStatus::Warn, "degraded")
            .with_hint(format!(
                "{} server problem{}, {} connected, {} tool{}; run /mcp for details",
                summary.problems,
                plural_suffix(summary.problems),
                summary.connected,
                summary.exported_tools,
                plural_suffix(summary.exported_tools),
            )),
        McpLoadMode::UnsupportedAgent => {
            let status = if summary.enabled == 0 && summary.problems == 0 {
                DoctorStatus::Info
            } else {
                DoctorStatus::Warn
            };
            DoctorCheck::new(id, label, status, "unsupported agent").with_hint(
                "native MCP loads only for Rho agents; the active agent does not host MCP tools",
            )
        }
        McpLoadMode::ToolsDisabled => {
            DoctorCheck::new(id, label, DoctorStatus::Info, "tools disabled")
                .with_hint("this session started with tools disabled, so MCP was not connected")
        }
    }
}

pub(super) fn plugins_check(report: &PluginLoadReport) -> DoctorCheck {
    let summary = report.summary();
    let id = DoctorCheckId::Plugins;
    let label = "Agent Plugins";
    if !summary.discovered {
        return DoctorCheck::new(id, label, DoctorStatus::Info, "none discovered").with_hint(
            format!(
                "no Agent Plugins found in the explicit roots; supported: {}",
                crate::plugins::SUPPORTED_COMPONENTS
            ),
        );
    }
    let status = if summary.rejected == 0 && summary.problems == 0 {
        DoctorStatus::Ok
    } else {
        DoctorStatus::Warn
    };
    let mut hint = format!(
        "{} skill{}, {} MCP server{}; supported: {}",
        summary.skills,
        plural_suffix(summary.skills),
        summary.mcp_servers,
        plural_suffix(summary.mcp_servers),
        crate::plugins::SUPPORTED_COMPONENTS
    );
    if summary.untrusted > 0 {
        hint.push_str(&format!(
            "; set {} to activate project plugins",
            crate::plugins::TRUST_PROJECT_PLUGINS_ENV
        ));
    }
    DoctorCheck::new(id, label, status, plugins_summary(&summary)).with_hint(hint)
}

fn plugins_summary(summary: &PluginLoadSummary) -> String {
    let mut parts = vec![format!("{} loaded", summary.loaded)];
    if summary.disabled > 0 {
        parts.push(format!("{} disabled", summary.disabled));
    }
    if summary.untrusted > 0 {
        parts.push(format!("{} untrusted", summary.untrusted));
    }
    if summary.rejected > 0 {
        parts.push(format!("{} rejected", summary.rejected));
    }
    if summary.problems > 0 {
        parts.push(format!(
            "{} problem{}",
            summary.problems,
            plural_suffix(summary.problems)
        ));
    }
    parts.join(", ")
}

fn probe_writable(path: &Path, kind: PathKind) -> bool {
    if path.exists() {
        return match kind {
            PathKind::File if path.is_file() => {
                fs::OpenOptions::new().write(true).open(path).is_ok()
            }
            PathKind::Directory if path.is_dir() => probe_directory(path),
            PathKind::File | PathKind::Directory => false,
        };
    }
    let directory = match kind {
        PathKind::File => path.parent().unwrap_or(path),
        PathKind::Directory => path,
    };
    if fs::create_dir_all(directory).is_err() {
        return false;
    }
    probe_directory(directory)
}

fn probe_directory(directory: &Path) -> bool {
    let probe = directory.join(format!(".rho-doctor-{}", uuid::Uuid::new_v4()));
    let result = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    let _ = fs::remove_file(probe);
    result
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
