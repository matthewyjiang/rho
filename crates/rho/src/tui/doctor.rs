use std::{fs, path::Path};

use {
    rho_providers::model::{catalog, provider_models::ProviderModelHealth},
    rho_providers::provider::{self, ProviderAuthKind, ProviderModelSource},
    rho_providers::{auth::login_dispatch::ProviderAuthentication, credentials::CredentialStore},
};

use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, UiPicker};
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
}

pub(super) fn picker(context: DoctorContext<'_>) -> UiPicker {
    let mut items = Vec::new();
    for descriptor in provider::providers() {
        let (healthy, status, detail) = if descriptor.auth_kind == ProviderAuthKind::None {
            (
                true,
                "no authentication required",
                "This provider does not require authentication.",
            )
        } else {
            match ProviderAuthentication::has_credentials(
                context.credential_store,
                descriptor.name,
            ) {
                Ok(true) if ProviderAuthentication::has_environment_override(descriptor.name) => (
                    true,
                    "authenticated",
                    "Credentials are provided by an environment variable.",
                ),
                Ok(true) => (
                    true,
                    "authenticated",
                    "Credentials are available in the configured credential store.",
                ),
                Ok(false) => (
                    false,
                    "missing",
                    "No credentials found. Run /login to authenticate this provider.",
                ),
                Err(_) => (
                    false,
                    "error",
                    "The configured credential store could not be read. No secret values were inspected or displayed.",
                ),
            }
        };
        items.push(item(
            format!("{} authentication", descriptor.display_name),
            status,
            healthy,
            detail.into(),
        ));
    }

    for descriptor in provider::providers().iter().filter(|descriptor| {
        descriptor.auth_kind == ProviderAuthKind::None
            && descriptor.model_refresh
                == Some(provider::ProviderModelRefreshKind::OpenAiCompatible)
    }) {
        let health = context
            .provider_health
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
                format!("The model endpoint returned an invalid or unsuccessful response: {error}"),
            ),
            None => (
                false,
                "not checked",
                "Run /doctor after the current model turn to check this endpoint.".into(),
            ),
        };
        items.push(item(
            format!("{} connection", descriptor.display_name),
            status,
            healthy,
            detail,
        ));
    }

    let model_available = catalog::resolve_model_selection_for_auths(
        &format!("{}/{}", context.provider, context.model),
        context.provider,
        context.auth,
        context.available_auths,
    )
    .is_ok();
    items.push(item(
        "Selected model",
        if model_available {
            "available"
        } else {
            "unavailable"
        },
        model_available,
        format!(
            "{}/{} using {} authentication",
            context.provider, context.model, context.auth
        ),
    ));

    let config_writable = probe_writable(context.config_path, PathKind::File);
    items.push(item(
        "Configuration",
        writable_status(config_writable),
        config_writable,
        context.config_path.display().to_string(),
    ));
    let sessions_writable = probe_writable(context.session_root, PathKind::Directory);
    items.push(item(
        "Sessions",
        writable_status(sessions_writable),
        sessions_writable,
        context.session_root.display().to_string(),
    ));

    for descriptor in provider::providers() {
        if descriptor.model_source == ProviderModelSource::CachedProviderModels {
            let count =
                rho_providers::model::provider_models::cached_provider_models(descriptor.name)
                    .len();
            items.push(item(
                format!("{} model cache", descriptor.display_name),
                if count > 0 { "populated" } else { "empty" },
                count > 0,
                format!("{count} cached model{}", if count == 1 { "" } else { "s" }),
            ));
        }
    }

    let clipboard = clipboard_doctor_report();
    items.push(item(
        "Clipboard text write",
        clipboard.text_write_status,
        clipboard.text_write_healthy,
        format!(
            "session={}; {}",
            clipboard.session_label, clipboard.text_write_detail
        ),
    ));
    items.push(item(
        "Clipboard image helper",
        clipboard.image_status(),
        clipboard.image_healthy(),
        clipboard.image_detail(),
    ));
    let rtk = rho_tools::rtk::is_available();
    items.push(item(
        "rtk",
        if rtk { "available" } else { "unavailable" },
        rtk,
        "Optional shell-command rewriting helper.".into(),
    ));
    let (herdr_healthy, herdr_status, herdr_detail) =
        match (context.herdr_enabled, context.herdr_socket_reachable) {
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
    items.push(item(
        "Herdr",
        herdr_status,
        herdr_healthy,
        herdr_detail.into(),
    ));

    UiPicker::new(
        "Doctor diagnostics",
        "up/down inspect, type to filter, enter or esc close",
        items,
        PickerAction::Doctor,
    )
}

fn item(
    label: impl Into<String>,
    status: impl Into<String>,
    healthy: bool,
    detail: String,
) -> PickerItem {
    let label = label.into();
    PickerItem {
        value: label.clone(),
        label,
        detail: Some(detail),
        preview: None,
        badge: Some(PickerBadge {
            text: status.into(),
            tone: if healthy {
                PickerBadgeTone::Healthy
            } else {
                PickerBadgeTone::Warning
            },
        }),
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

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
