//! Cache writes for CLI-sourced model lists (for example Cursor Agent).
//!
//! These rows live in the same `provider-models.sqlite3` as HTTP discovery,
//! keyed by a provider string that is not necessarily a Rho registry id.

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{ModelError, ReasoningCapabilities};

use super::{
    open_provider_models_cache, provider_snapshot_timestamp_is_fresh,
    replace_cached_provider_model_records_with_context, ProviderModel, ProviderModelRecord,
};

/// Account/version metadata stored on the refresh row for a CLI-sourced list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliProviderRefreshContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_version: Option<String>,
}

/// One CLI-discovered model plus the flags that do not fit [`ProviderModel`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliProviderModel {
    pub model: ProviderModel,
    pub raw_json: Value,
}

/// Replace cached rows for a CLI-sourced provider key such as `"cursor"`.
///
/// `raw_json` on each row holds CLI flags (default / current / zdr). Context is
/// stored on the refresh row so a later account mismatch can treat the snapshot
/// as stale.
pub fn replace_cli_provider_models(
    provider: &str,
    models: Vec<CliProviderModel>,
    context: &CliProviderRefreshContext,
) -> Result<(), ModelError> {
    let context_json = serde_json::to_string(context).map_err(|error| {
        ModelError::InvalidResponse(format!(
            "failed to serialize CLI provider refresh context: {error}"
        ))
    })?;
    let records = models
        .into_iter()
        .map(|entry| ProviderModelRecord {
            model: entry.model,
            raw_json: entry.raw_json,
        })
        .collect::<Vec<_>>();
    replace_cached_provider_model_records_with_context(provider, &records, Some(&context_json))
}

/// Whether the provider's refresh snapshot is within the 24h freshness window.
pub fn provider_models_are_fresh(provider: &str) -> bool {
    let Ok(connection) = open_provider_models_cache() else {
        return false;
    };
    let Ok(updated_at) = connection.query_row(
        "select updated_at from provider_model_refresh where provider = ?1",
        params![provider],
        |row| row.get::<_, i64>(0),
    ) else {
        return false;
    };
    provider_snapshot_timestamp_is_fresh(updated_at)
}

/// Refresh-row context last written by [`replace_cli_provider_models`].
pub fn cli_provider_refresh_context(provider: &str) -> Option<CliProviderRefreshContext> {
    let connection = open_provider_models_cache().ok()?;
    let raw: Option<String> = connection
        .query_row(
            "select context_json from provider_model_refresh where provider = ?1",
            params![provider],
            |row| row.get(0),
        )
        .ok()?;
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

/// Cached CLI rows including `raw_json`. Ids are not registry-canonicalized.
pub fn cached_cli_provider_models(provider: &str) -> Vec<CliProviderModel> {
    let Ok(connection) = open_provider_models_cache() else {
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare(
        "select model, display_name, context_window, max_output_tokens, reasoning_capabilities_json, raw_json
         from provider_models where provider = ?1 order by model",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![provider], |row| {
        let model: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        let context_window: Option<u64> = row.get(2)?;
        let max_output_tokens: Option<u64> = row.get(3)?;
        let reasoning_capabilities = row
            .get::<_, Option<String>>(4)?
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or(ReasoningCapabilities::Unknown);
        let raw_json = row
            .get::<_, Option<String>>(5)?
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or(Value::Null);
        Ok(CliProviderModel {
            model: ProviderModel {
                provider: provider.to_string(),
                model,
                display_name,
                context_window,
                max_output_tokens,
                reasoning_capabilities,
            },
            raw_json,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

pub(super) fn ensure_refresh_context_column(connection: &rusqlite::Connection) {
    let _ = connection.execute(
        "alter table provider_model_refresh add column context_json text",
        [],
    );
}
