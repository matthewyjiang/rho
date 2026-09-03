//! Cursor Agent program names, sink labels, and cached model discovery.

use std::time::Duration;

use rho_providers::model::provider_models::{
    cached_cli_provider_models, cli_provider_refresh_context, provider_models_are_fresh,
    replace_cli_provider_models, CliProviderModel, CliProviderRefreshContext, ProviderModel,
};
use rho_providers::model::ReasoningCapabilities;
use serde_json::{json, Value};
use thiserror::Error;

use crate::claude_runtime::persist::RuntimeLabel;
use crate::cli_runtime::{run_bounded_probe, BoundedOutput, ProbeError};

use super::{auth, executable};

/// Program name resolved on `PATH`. Not `agent`: that collides with other tools.
pub(crate) const CURSOR_PROGRAM: &str = "cursor-agent";

/// How Cursor names itself in a `<source>/<model>` slot.
///
/// Phase D wires this into pickers and `/login cursor`.
pub(crate) const CURSOR_SOURCE_LABEL: &str = "cursor";

/// Error prefixes and [`RuntimeLabel::program`]: `cursor: ...`.
pub(crate) const CURSOR_PROGRAM_LABEL: &str = "cursor";

/// Starting activity and program name for the shared artifact sink.
pub(crate) const CURSOR_LABEL: RuntimeLabel = RuntimeLabel {
    starting_activity: "starting cursor",
    program: CURSOR_PROGRAM_LABEL,
};

/// Same wall-clock budget as auth/version probes.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

const FAMILY_SUFFIXES: &[&str] = &[
    "Extra High",
    "Low",
    "Medium",
    "High",
    "Max",
    "None",
    "Minimal",
    "Fast",
    "Thinking",
    "1M",
];

/// One row from `cursor-agent models`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorModel {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
    pub is_current: bool,
    pub zdr: bool,
}

/// Failures when listing or caching Cursor models.
#[derive(Debug, Error)]
pub(crate) enum CursorModelsError {
    #[error("cursor: binary not found on PATH")]
    BinaryMissing,
    #[error("cursor: {0}")]
    Probe(ProbeError),
    #[error("cursor: `{program}` exited with {status}")]
    ExitStatus {
        program: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("cursor: models output was not valid UTF-8")]
    InvalidUtf8,
    #[error("cursor: models list contained no model rows")]
    EmptyList,
    #[error("cursor: could not cache models: {0}")]
    Cache(String),
}

impl From<ProbeError> for CursorModelsError {
    fn from(error: ProbeError) -> Self {
        match error {
            ProbeError::BinaryMissing => Self::BinaryMissing,
            other => Self::Probe(other),
        }
    }
}

impl CursorModel {
    /// Display-name family used as a picker section.
    ///
    /// Strips trailing effort/speed/thinking tokens repeatedly so variants of
    /// one model sit together without collapsing their ids.
    pub(crate) fn display_family(&self) -> String {
        display_family(&self.display_name)
    }
}

/// Parse `cursor-agent models` plain text. Skips headers, blanks, and the
/// trailing tip. Unknown parentheticals stay in the display name. Errors only
/// when no model rows parse.
pub(crate) fn parse_models_output(output: &str) -> Result<Vec<CursorModel>, CursorModelsError> {
    let models = output
        .lines()
        .filter_map(parse_model_line)
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(CursorModelsError::EmptyList);
    }
    Ok(models)
}

pub(crate) async fn fetch() -> Result<Vec<CursorModel>, CursorModelsError> {
    let executable = executable::resolve().map_err(|_| CursorModelsError::BinaryMissing)?;
    let output = run_bounded_probe(&executable, &["models"], PROBE_TIMEOUT).await?;
    parse_models_probe_output(&executable.display(), &output)
}

pub(crate) fn cached() -> Vec<CursorModel> {
    cached_cli_provider_models(CURSOR_SOURCE_LABEL)
        .into_iter()
        .map(cursor_model_from_cached)
        .collect()
}

pub(crate) fn needs_refresh() -> bool {
    cached().is_empty() || !provider_models_are_fresh(CURSOR_SOURCE_LABEL)
}

/// True when the cached snapshot belongs to a different Cursor account.
pub(crate) fn needs_refresh_for_account(email: Option<&str>) -> bool {
    if needs_refresh() {
        return true;
    }
    let cached_email = cli_provider_refresh_context(CURSOR_SOURCE_LABEL)
        .and_then(|context| context.account_email)
        .filter(|value| !value.is_empty());
    match (
        email.filter(|value| !value.is_empty()),
        cached_email.as_deref(),
    ) {
        (Some(current), Some(cached)) => current != cached,
        (Some(_), None) => true,
        _ => false,
    }
}

pub(crate) async fn refresh() -> Result<Vec<CursorModel>, CursorModelsError> {
    let models = fetch().await?;
    cache_models(&models, current_refresh_context().await)?;
    Ok(models)
}

pub(crate) fn cache_models(
    models: &[CursorModel],
    context: CliProviderRefreshContext,
) -> Result<(), CursorModelsError> {
    let rows = models
        .iter()
        .map(|model| CliProviderModel {
            model: ProviderModel {
                provider: CURSOR_SOURCE_LABEL.into(),
                model: model.id.clone(),
                display_name: model.display_name.clone(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            },
            raw_json: json!({
                "default": model.is_default,
                "current": model.is_current,
                "zdr": model.zdr,
            }),
        })
        .collect();
    replace_cli_provider_models(CURSOR_SOURCE_LABEL, rows, &context)
        .map_err(|error| CursorModelsError::Cache(error.to_string()))
}

pub(crate) async fn current_refresh_context() -> CliProviderRefreshContext {
    let account_email = match auth::query().await {
        Ok(status) => status
            .user_info
            .and_then(|info| info.email)
            .filter(|email| !email.is_empty()),
        Err(_) => None,
    };
    CliProviderRefreshContext {
        account_email,
        cursor_version: auth::version().await.ok(),
    }
}

fn parse_models_probe_output(
    program: &str,
    output: &BoundedOutput,
) -> Result<Vec<CursorModel>, CursorModelsError> {
    let stdout =
        String::from_utf8(output.stdout.clone()).map_err(|_| CursorModelsError::InvalidUtf8)?;
    match parse_models_output(&stdout) {
        Ok(models) => Ok(models),
        Err(CursorModelsError::EmptyList) if !output.status.success() => {
            Err(CursorModelsError::ExitStatus {
                program: program.into(),
                status: output.status,
                stderr: output.stderr_lossy_trimmed(),
            })
        }
        Err(error) => Err(error),
    }
}

fn parse_model_line(line: &str) -> Option<CursorModel> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (id, rest) = line.split_once(" - ")?;
    let id = id.trim();
    if id.is_empty() || id.contains(' ') {
        return None;
    }
    let (display_name, is_default, is_current, zdr) = strip_known_annotations(rest.trim());
    if display_name.is_empty() {
        return None;
    }
    Some(CursorModel {
        id: id.to_string(),
        display_name,
        is_default,
        is_current,
        zdr,
    })
}

fn strip_known_annotations(rest: &str) -> (String, bool, bool, bool) {
    let mut display = rest.to_string();
    let mut is_default = false;
    let mut is_current = false;
    let mut zdr = true;
    loop {
        let trimmed = display.trim_end();
        let Some(open) = trimmed.rfind('(') else {
            display = trimmed.to_string();
            break;
        };
        if !trimmed.ends_with(')') {
            display = trimmed.to_string();
            break;
        }
        let annotation = trimmed[open + 1..trimmed.len() - 1].trim();
        match annotation {
            "default" => is_default = true,
            "current" => is_current = true,
            "NO ZDR" => zdr = false,
            _ => {
                display = trimmed.to_string();
                break;
            }
        }
        display = trimmed[..open].trim_end().to_string();
    }
    (display.trim().to_string(), is_default, is_current, zdr)
}

fn display_family(display_name: &str) -> String {
    let mut family = display_name.trim().to_string();
    loop {
        let trimmed = family.trim_end();
        let Some(suffix) = FAMILY_SUFFIXES
            .iter()
            .find(|suffix| suffix_token(trimmed, suffix))
        else {
            family = trimmed.to_string();
            break;
        };
        family = trimmed[..trimmed.len() - suffix.len()]
            .trim_end()
            .to_string();
    }
    family
}

fn suffix_token(haystack: &str, suffix: &str) -> bool {
    haystack
        .strip_suffix(suffix)
        .is_some_and(|rest| rest.is_empty() || rest.ends_with(' '))
}

fn cursor_model_from_cached(entry: CliProviderModel) -> CursorModel {
    CursorModel {
        id: entry.model.model,
        display_name: entry.model.display_name,
        is_default: flag(&entry.raw_json, "default", false),
        is_current: flag(&entry.raw_json, "current", false),
        zdr: flag(&entry.raw_json, "zdr", true),
    }
}

fn flag(raw_json: &Value, key: &str, default: bool) -> bool {
    raw_json
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
