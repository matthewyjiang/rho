use std::{fs, path::Path};

use {
    rho_providers::model::{catalog, provider_models::ProviderModelHealth},
    rho_providers::provider::{self, ProviderAuthKind, ProviderModelSource},
    rho_providers::{auth::login_dispatch::ProviderAuthentication, credentials::CredentialStore},
};

use super::{
    picker_overlay::OverlayChrome, PickerAction, PickerBadge, PickerBadgePlacement,
    PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};
use crate::claude_runtime::auth::ClaudeProbeSnapshot;
use crate::clipboard::doctor_report as clipboard_doctor_report;

pub(super) struct DoctorContext<'a> {
    pub(super) provider: &'a str,
    pub(super) model: &'a str,
    pub(super) auth: &'a str,
    pub(super) available_auths: &'a [String],
    pub(super) credential_store: &'a dyn CredentialStore,
    pub(super) config_path: &'a Path,
    pub(super) session_root: &'a Path,
    pub(super) herdr_enabled: bool,
    pub(super) herdr_socket_reachable: Option<bool>,
    pub(super) provider_health: &'a [(String, ProviderModelHealth)],
    /// Claude Code binary and auth probe. Not a Rho credential.
    pub(super) claude: &'a ClaudeProbeSnapshot,
    pub(super) mcp_report: &'a crate::tools::mcp::McpSessionReport,
    pub(super) plugins_report: &'a crate::plugins::PluginLoadReport,
}

#[derive(Clone, Copy)]
enum DoctorSection {
    Authentication,
    Cache,
    External,
    Misc,
}

impl DoctorSection {
    const fn label(self) -> &'static str {
        match self {
            Self::Authentication => "AUTHENTICATION",
            Self::Cache => "CACHE",
            Self::External => "EXTERNAL RUNTIMES",
            Self::Misc => "MISC",
        }
    }

    const fn order(self) -> u8 {
        match self {
            Self::Authentication => 0,
            Self::Cache => 1,
            Self::External => 2,
            Self::Misc => 3,
        }
    }
}

struct DoctorCheck {
    section: DoctorSection,
    label: String,
    status: String,
    healthy: bool,
    detail: String,
}

pub(super) fn picker(context: DoctorContext<'_>) -> UiPicker {
    let mut checks = Vec::new();
    checks.extend(authentication_checks(context.credential_store));
    checks.extend(cache_checks());
    checks.extend(external_runtime_checks(context.claude));
    checks.extend(misc_checks(&context));
    checks.sort_by_key(|check| check.section.order());

    UiPicker::new(
        "Doctor diagnostics",
        checks.into_iter().map(PickerItem::from).collect(),
        PickerAction::Dismiss,
    )
    .with_layout(PickerLayout::Overlay)
    .with_badge_placement(PickerBadgePlacement::Detail)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " CHECKS".into(),
        detail_label: Some(" DETAILS".into()),
        nav_keys_hint: "↑↓ checks".into(),
    })
    .with_confirm_verb("close")
}

fn authentication_checks(store: &dyn CredentialStore) -> Vec<DoctorCheck> {
    provider::visible_providers()
        .iter()
        .flat_map(|descriptor| {
            if descriptor.is_keyless() {
                return vec![DoctorCheck {
                    section: DoctorSection::Authentication,
                    label: format!("{} authentication", descriptor.display_name),
                    status: "no authentication required".into(),
                    healthy: true,
                    detail: "This provider does not require authentication.".into(),
                }];
            }
            descriptor
                .auth_modes()
                .filter(|mode| mode.auth_kind != ProviderAuthKind::None)
                .map(|mode| {
                    let (healthy, status, detail) =
                        match ProviderAuthentication::has_credentials(store, mode.id) {
                            Ok(true)
                                if ProviderAuthentication::has_environment_override(mode.id) =>
                            {
                                (
                                    true,
                                    "authenticated",
                                    "Credentials are provided by an environment variable.".into(),
                                )
                            }
                            Ok(true) => (
                                true,
                                "authenticated",
                                "Credentials are available in the configured credential store."
                                    .into(),
                            ),
                            Ok(false) if descriptor.has_none_auth() => (
                                true,
                                "optional",
                                format!(
                                    "No key stored. This host can run without one, or run /login {} to add a key.",
                                    mode.id
                                ),
                            ),
                            Ok(false) => (
                                false,
                                "missing",
                                format!(
                                    "No credentials found. Run /login {} to authenticate this mode.",
                                    mode.id
                                ),
                            ),
                            Err(_) => (
                                false,
                                "error",
                                "The configured credential store could not be read. No secret values were inspected or displayed.".into(),
                            ),
                        };
                    DoctorCheck {
                        section: DoctorSection::Authentication,
                        label: mode.login_label.to_string(),
                        status: status.into(),
                        healthy,
                        detail,
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn cache_checks() -> Vec<DoctorCheck> {
    provider::visible_providers()
        .iter()
        .filter(|descriptor| descriptor.model_source == ProviderModelSource::CachedProviderModels)
        .map(|descriptor| {
            let count =
                rho_providers::model::provider_models::cached_provider_models(descriptor.name)
                    .len();
            DoctorCheck {
                section: DoctorSection::Cache,
                label: format!("{} model cache", descriptor.display_name),
                status: if count > 0 {
                    "populated".into()
                } else {
                    "empty".into()
                },
                healthy: count > 0,
                detail: format!("{count} cached model{}", if count == 1 { "" } else { "s" }),
            }
        })
        .collect()
}

fn external_runtime_checks(claude: &ClaudeProbeSnapshot) -> Vec<DoctorCheck> {
    let auth_healthy = claude.auth_healthy();
    let binary_healthy = claude.binary_healthy();
    vec![
        DoctorCheck {
            section: DoctorSection::External,
            label: "Claude Code authentication".into(),
            status: if auth_healthy {
                "ok".into()
            } else {
                "attention".into()
            },
            healthy: auth_healthy,
            detail: claude.auth_description(),
        },
        DoctorCheck {
            section: DoctorSection::External,
            label: "Claude Code binary".into(),
            status: if binary_healthy {
                "available".into()
            } else {
                "attention".into()
            },
            healthy: binary_healthy,
            detail: claude.version_description(),
        },
    ]
}

fn misc_checks(context: &DoctorContext<'_>) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    checks.extend(model_endpoint_checks(context.provider_health));
    checks.push(selected_model_check(context));
    checks.push(path_check(
        "Configuration",
        context.config_path,
        PathKind::File,
    ));
    checks.push(path_check(
        "Sessions",
        context.session_root,
        PathKind::Directory,
    ));
    checks.extend(clipboard_checks());
    checks.push(rtk_check());
    checks.push(herdr_check(
        context.herdr_enabled,
        context.herdr_socket_reachable,
    ));
    checks.push(mcp_check(context.mcp_report));
    checks.push(plugins_check(context.plugins_report));
    checks
}

fn model_endpoint_checks(provider_health: &[(String, ProviderModelHealth)]) -> Vec<DoctorCheck> {
    provider::visible_providers()
        .iter()
        .filter(|descriptor| descriptor.probes_configured_endpoint())
        .map(|descriptor| {
            let health = provider_health
                .iter()
                .find_map(|(name, health)| (name == descriptor.name).then_some(health));
            let (healthy, status, detail) = match health {
                Some(ProviderModelHealth::ReachableWithModels { model_count }) => (
                    true,
                    "reachable",
                    format!(
                        "The model endpoint returned {model_count} installed model{}.",
                        if *model_count == 1 { "" } else { "s" }
                    ),
                ),
                Some(ProviderModelHealth::ReachableWithoutModels) => (
                    false,
                    "no models",
                    "The model endpoint is reachable but has no installed models.".into(),
                ),
                Some(ProviderModelHealth::Unreachable { error }) => (
                    false,
                    "unreachable",
                    format!("The model endpoint could not be reached: {error}"),
                ),
                Some(ProviderModelHealth::InvalidResponse { error }) => (
                    false,
                    "invalid response",
                    format!(
                        "The model endpoint returned an invalid or unsuccessful response: {error}"
                    ),
                ),
                None => (
                    false,
                    "not checked",
                    "Run /doctor after the current model turn to check this endpoint.".into(),
                ),
            };
            DoctorCheck {
                section: DoctorSection::Misc,
                label: format!("{} connection", descriptor.display_name),
                status: status.into(),
                healthy,
                detail,
            }
        })
        .collect()
}

fn selected_model_check(context: &DoctorContext<'_>) -> DoctorCheck {
    let model_available = catalog::resolve_model_selection_for_auths(
        &rho_providers::provider::model_reference(context.provider, context.model),
        context.provider,
        context.auth,
        context.available_auths,
    )
    .is_ok();
    DoctorCheck {
        section: DoctorSection::Misc,
        label: "Selected model".into(),
        status: if model_available {
            "available".into()
        } else {
            "unavailable".into()
        },
        healthy: model_available,
        detail: format!(
            "{} using {} authentication",
            rho_providers::provider::model_reference(context.provider, context.model),
            context.auth
        ),
    }
}

fn path_check(label: &str, path: &Path, kind: PathKind) -> DoctorCheck {
    let writable = probe_writable(path, kind);
    DoctorCheck {
        section: DoctorSection::Misc,
        label: label.into(),
        status: writable_status(writable).into(),
        healthy: writable,
        detail: path.display().to_string(),
    }
}

fn clipboard_checks() -> Vec<DoctorCheck> {
    let clipboard = clipboard_doctor_report();
    vec![
        DoctorCheck {
            section: DoctorSection::Misc,
            label: "Clipboard text write".into(),
            status: clipboard.text_write_status.into(),
            healthy: clipboard.text_write_healthy,
            detail: format!(
                "session={}; {}",
                clipboard.session_label, clipboard.text_write_detail
            ),
        },
        DoctorCheck {
            section: DoctorSection::Misc,
            label: "Clipboard image helper".into(),
            status: clipboard.image_status().into(),
            healthy: clipboard.image_healthy(),
            detail: clipboard.image_detail(),
        },
    ]
}

fn rtk_check() -> DoctorCheck {
    let available = rho_tools::rtk::is_available();
    DoctorCheck {
        section: DoctorSection::Misc,
        label: "rtk".into(),
        status: if available {
            "available".into()
        } else {
            "unavailable".into()
        },
        healthy: available,
        detail: "Optional shell-command rewriting helper.".into(),
    }
}

fn herdr_check(enabled: bool, socket_reachable: Option<bool>) -> DoctorCheck {
    let (healthy, status, detail) = match (enabled, socket_reachable) {
        (false, _) => (true, "not configured", "Rho is not running inside Herdr."),
        (true, Some(true)) => (
            true,
            "connected",
            "The configured Herdr socket accepted a connection.",
        ),
        (true, Some(false)) => (
            false,
            "unreachable",
            "Herdr environment variables are set, but the socket did not accept a connection.",
        ),
        (true, None) => (
            false,
            "unavailable",
            "Herdr is configured, but socket reachability could not be determined.",
        ),
    };
    DoctorCheck {
        section: DoctorSection::Misc,
        label: "Herdr".into(),
        status: status.into(),
        healthy,
        detail: detail.into(),
    }
}

fn mcp_check(report: &crate::tools::mcp::McpSessionReport) -> DoctorCheck {
    use crate::tools::mcp::McpLoadMode;

    let summary = report.summary();
    let (status, healthy, detail) =
        if !summary.configured {
            (
                "not configured".into(),
                true,
                "No MCP servers under [mcp.servers].".into(),
            )
        } else {
            match summary.mode {
            McpLoadMode::Native if summary.connecting > 0 && summary.problems == 0 => (
                "connecting".into(),
                true,
                format!(
                    "{} connecting, {} connected, {} exported tool{}.",
                    summary.connecting,
                    summary.connected,
                    summary.exported_tools,
                    super::plural_suffix(summary.exported_tools),
                ),
            ),
            McpLoadMode::Native if summary.problems == 0 => (
                if summary.connected > 0 { "connected" } else { "idle" }.into(),
                true,
                format!(
                    "{} connected server{}, {} exported tool{}.",
                    summary.connected,
                    super::plural_suffix(summary.connected),
                    summary.exported_tools,
                    super::plural_suffix(summary.exported_tools),
                ),
            ),
            McpLoadMode::Native => (
                "degraded".into(),
                false,
                format!(
                    "{} server problem{}, {} connected, {} tool{}. Run /mcp for details.",
                    summary.problems,
                    super::plural_suffix(summary.problems),
                    summary.connected,
                    summary.exported_tools,
                    super::plural_suffix(summary.exported_tools),
                ),
            ),
            McpLoadMode::UnsupportedAgent => (
                "unsupported agent".into(),
                summary.enabled == 0 && summary.problems == 0,
                "Native MCP loads only for Rho agents. The active agent does not host MCP tools."
                    .into(),
            ),
            McpLoadMode::ToolsDisabled => (
                "tools disabled".into(),
                true,
                "This session started with tools disabled, so MCP was not connected.".into(),
            ),
        }
        };
    DoctorCheck {
        section: DoctorSection::Misc,
        label: "MCP".into(),
        status,
        healthy,
        detail,
    }
}

fn plugins_check(report: &crate::plugins::PluginLoadReport) -> DoctorCheck {
    let summary = report.summary();
    let healthy = summary.rejected == 0 && summary.problems == 0;
    let status = if !summary.discovered {
        "none discovered".into()
    } else if healthy && summary.disabled == 0 {
        format!("{} loaded", summary.loaded)
    } else if healthy {
        format!("{} loaded, {} disabled", summary.loaded, summary.disabled)
    } else {
        format!(
            "{} loaded, {} disabled, {} rejected, {} problem{}",
            summary.loaded,
            summary.disabled,
            summary.rejected,
            summary.problems,
            super::plural_suffix(summary.problems)
        )
    };
    let detail = if summary.discovered {
        format!(
            "{} skill{}, {} MCP server{}; supported: {}",
            summary.skills,
            super::plural_suffix(summary.skills),
            summary.mcp_servers,
            super::plural_suffix(summary.mcp_servers),
            crate::plugins::SUPPORTED_COMPONENTS
        )
    } else {
        format!(
            "no Agent Plugins found in the explicit roots; supported: {}",
            crate::plugins::SUPPORTED_COMPONENTS
        )
    };
    DoctorCheck {
        section: DoctorSection::Misc,
        label: "Agent Plugins".into(),
        status,
        healthy,
        detail,
    }
}

impl From<DoctorCheck> for PickerItem {
    fn from(check: DoctorCheck) -> Self {
        PickerItem {
            value: check.label.clone(),
            label: check.label,
            section: Some(check.section.label().into()),
            detail: Some(check.detail),
            preview: None,
            badge: Some(PickerBadge {
                text: check.status,
                tone: if check.healthy {
                    PickerBadgeTone::Healthy
                } else {
                    PickerBadgeTone::Warning
                },
            }),
            selection_verb: None,
        }
    }
}

fn writable_status(writable: bool) -> &'static str {
    if writable {
        "writable"
    } else {
        "not writable"
    }
}

#[derive(Clone, Copy)]
enum PathKind {
    File,
    Directory,
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
