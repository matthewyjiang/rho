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
    const AUTHENTICATION: &str = "AUTHENTICATION";
    const CACHE: &str = "CACHE";
    const MISC: &str = "MISC";

    let mut authentication_items = Vec::new();
    let mut cache_items = Vec::new();
    let mut misc_items = Vec::new();
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
        authentication_items.push(item(
            AUTHENTICATION,
            if descriptor.auth_kind == ProviderAuthKind::None {
                format!("{} authentication", descriptor.display_name)
            } else {
                descriptor.login_label.to_string()
            },
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
        misc_items.push(item(
            MISC,
            format!("{} connection", descriptor.display_name),
            status,
            healthy,
            detail,
        ));
    }

    let model_available = catalog::resolve_model_selection_for_auths(
        &rho_providers::provider::model_reference(context.provider, context.model),
        context.provider,
        context.auth,
        context.available_auths,
    )
    .is_ok();
    misc_items.push(item(
        MISC,
        "Selected model",
        if model_available {
            "available"
        } else {
            "unavailable"
        },
        model_available,
        format!(
            "{} using {} authentication",
            rho_providers::provider::model_reference(context.provider, context.model),
            context.auth
        ),
    ));

    let config_writable = probe_writable(context.config_path, PathKind::File);
    misc_items.push(item(
        MISC,
        "Configuration",
        writable_status(config_writable),
        config_writable,
        context.config_path.display().to_string(),
    ));
    let sessions_writable = probe_writable(context.session_root, PathKind::Directory);
    misc_items.push(item(
        MISC,
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
            cache_items.push(item(
                CACHE,
                format!(
                    "{} model cache",
                    if descriptor.auth_kind == ProviderAuthKind::None {
                        descriptor.display_name
                    } else {
                        descriptor.login_label
                    }
                ),
                if count > 0 { "populated" } else { "empty" },
                count > 0,
                format!("{count} cached model{}", if count == 1 { "" } else { "s" }),
            ));
        }
    }

    let clipboard = clipboard_doctor_report();
    misc_items.push(item(
        MISC,
        "Clipboard text write",
        clipboard.text_write_status,
        clipboard.text_write_healthy,
        format!(
            "session={}; {}",
            clipboard.session_label, clipboard.text_write_detail
        ),
    ));
    misc_items.push(item(
        MISC,
        "Clipboard image helper",
        clipboard.image_status(),
        clipboard.image_healthy(),
        clipboard.image_detail(),
    ));
    let rtk = rho_tools::rtk::is_available();
    misc_items.push(item(
        MISC,
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
    misc_items.push(item(
        MISC,
        "Herdr",
        herdr_status,
        herdr_healthy,
        herdr_detail.into(),
    ));

    let items = authentication_items
        .into_iter()
        .chain(cache_items)
        .chain(misc_items)
        .collect();

    UiPicker::new(
        "Doctor diagnostics",
        "type regex filter, enter or esc closes",
        items,
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

fn item(
    section: &str,
    label: impl Into<String>,
    status: impl Into<String>,
    healthy: bool,
    detail: String,
) -> PickerItem {
    let label = label.into();
    PickerItem {
        value: label.clone(),
        label,
        section: Some(section.into()),
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
