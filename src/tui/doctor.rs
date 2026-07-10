use std::{fs, path::Path, process::Command};

use crate::{
    credentials::{provider_has_credentials, provider_has_env_override, CredentialStore},
    model::catalog,
    provider::{self, ProviderModelSource},
};

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
}

pub(super) fn report(context: DoctorContext<'_>) -> Vec<String> {
    let mut lines = vec!["rho doctor".into()];
    for descriptor in provider::providers() {
        let auth = match provider_has_credentials(context.credential_store, descriptor.name) {
            Ok(true) if provider_has_env_override(descriptor.name) => "ok (environment)",
            Ok(true) => "ok (credential store)",
            Ok(false) => "missing",
            Err(_) => "error reading credential store",
        };
        lines.push(format!(
            "[{}] auth {}: {auth}",
            tone(auth.starts_with("ok")),
            descriptor.name
        ));
    }

    let model_available = catalog::resolve_model_selection_for_auths(
        &format!("{}/{}", context.provider, context.model),
        context.provider,
        context.auth,
        context.available_auths,
    )
    .is_ok();
    lines.push(format!(
        "[{}] selected model: {}/{} ({})",
        tone(model_available),
        context.provider,
        context.model,
        if model_available {
            "available"
        } else {
            "unavailable"
        }
    ));
    lines.push(writability_line("config", context.config_path));
    lines.push(writability_line("sessions", context.session_root));

    for descriptor in provider::providers() {
        if descriptor.model_source == ProviderModelSource::CachedProviderModels {
            let count =
                crate::model::provider_models::cached_provider_models(descriptor.name).len();
            lines.push(format!(
                "[{}] model cache {}: {count} model{}",
                tone(count > 0),
                descriptor.name,
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    let helpers = clipboard_helpers();
    lines.push(format!(
        "[{}] clipboard image helper: {}",
        tone(!helpers.is_empty()),
        if helpers.is_empty() {
            "not found".into()
        } else {
            helpers.join(", ")
        }
    ));
    let rtk = crate::tools::rtk::is_available();
    lines.push(format!(
        "[{}] rtk: {}",
        tone(rtk),
        if rtk { "available" } else { "unavailable" }
    ));
    let herdr = match (context.herdr_enabled, context.herdr_socket_reachable) {
        (false, _) => "not configured",
        (true, Some(true)) => "connected environment and socket present",
        (true, Some(false)) => "configured but socket missing",
        (true, None) => "configured",
    };
    lines.push(format!(
        "[{}] Herdr: {herdr}",
        tone(!context.herdr_enabled || context.herdr_socket_reachable == Some(true))
    ));
    lines
}

fn tone(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "warn"
    }
}

fn writability_line(label: &str, path: &Path) -> String {
    let writable = probe_writable(path);
    format!(
        "[{}] {label} writable: {} ({})",
        tone(writable),
        if writable { "yes" } else { "no" },
        path.display()
    )
}

fn probe_writable(path: &Path) -> bool {
    if path.exists() && !path.is_dir() {
        return fs::OpenOptions::new().write(true).open(path).is_ok();
    }
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    if fs::create_dir_all(directory).is_err() {
        return false;
    }
    let probe = directory.join(format!(".rho-doctor-{}", uuid::Uuid::new_v4()));
    let result = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    let _ = fs::remove_file(probe);
    result
}

fn clipboard_helpers() -> Vec<&'static str> {
    let candidates: &[&str] = if cfg!(target_os = "linux") {
        &["wl-paste", "xclip"]
    } else if cfg!(target_os = "macos") {
        &["pngpaste"]
    } else if cfg!(target_os = "windows") {
        &["powershell"]
    } else {
        &[]
    };
    candidates
        .iter()
        .copied()
        .filter(|command| command_available(command))
        .collect()
}

fn command_available(command: &str) -> bool {
    Command::new(command).arg("--help").output().is_ok()
}
