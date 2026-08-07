//! Agent Plugins 1.0.0 `plugin.json` manifest validation.
//!
//! The manifest schema is closed. Two violations are non-fatal per the
//! specification and recorded as warnings: unknown top-level fields, and a
//! non-object `extensions` field. Every other schema violation rejects the
//! plugin before any component discovery. Schemas are recognized locally by
//! their canonical identifier and never fetched at runtime.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub(crate) const PLUGIN_MANIFEST_SCHEMA_1_0_0: &str =
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

#[derive(Clone, Debug)]
pub(crate) struct PluginManifest {
    pub(crate) name: String,
    /// Non-fatal schema violations that were reported and ignored.
    pub(crate) warnings: Vec<String>,
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(rename = "$schema")]
    schema: String,
    name: String,
    #[serde(default, deserialize_with = "optional_non_null")]
    version: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    description: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    author: Option<RawAuthor>,
    #[serde(default, deserialize_with = "optional_non_null")]
    homepage: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    repository: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    license: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    keywords: Option<Vec<String>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    extensions: Option<Value>,
    #[serde(flatten)]
    unknown: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuthor {
    #[serde(default, deserialize_with = "optional_non_null")]
    name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    email: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    url: Option<String>,
}
pub(super) fn optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub(crate) fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
    let raw: RawManifest = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if raw.schema != PLUGIN_MANIFEST_SCHEMA_1_0_0 {
        return Err(format!("unsupported Agent Plugins schema `{}`", raw.schema));
    }
    validate_plugin_name(&raw.name)?;

    let mut warnings = raw
        .unknown
        .keys()
        .map(|key| format!("unknown manifest field `{key}` ignored"))
        .collect::<Vec<_>>();
    if raw
        .extensions
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        warnings.push("non-object `extensions` field ignored".to_string());
    }

    // These values are intentionally not retained. Their typed fields make
    // Serde enforce the manifest contract without a second manual validator.
    let _ = (
        raw.version,
        raw.description,
        raw.author
            .map(|author| (author.name, author.email, author.url)),
        raw.homepage,
        raw.repository,
        raw.license,
        raw.keywords,
    );

    Ok(PluginManifest {
        name: raw.name,
        warnings,
    })
}

/// Agent Plugins 1.0.0 plugin name constraints (spec §5.5).
pub(crate) fn validate_plugin_name(name: &str) -> Result<(), String> {
    let chars: Vec<char> = name.chars().collect();
    if chars.is_empty() || chars.len() > 64 {
        return Err("plugin name must be 1-64 characters".to_string());
    }
    let alphanumeric =
        |character: &char| character.is_ascii_lowercase() || character.is_ascii_digit();
    let allowed =
        |character: &char| alphanumeric(character) || *character == '-' || *character == '.';
    if !chars.iter().all(allowed) {
        return Err("plugin name may only use lowercase letters, digits, '-' and '.'".to_string());
    }
    if !alphanumeric(&chars[0]) || !alphanumeric(&chars[chars.len() - 1]) {
        return Err("plugin name must start and end with a lowercase letter or digit".to_string());
    }
    if name.contains("--") || name.contains("..") {
        return Err("plugin name must not contain consecutive hyphens or periods".to_string());
    }
    Ok(())
}
