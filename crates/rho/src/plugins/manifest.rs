//! Agent Plugins 1.0.0 `plugin.json` manifest validation.
//!
//! The manifest schema is closed. Two violations are non-fatal per the
//! specification and recorded as warnings: unknown top-level fields, and a
//! non-object `extensions` field. Every other schema violation rejects the
//! plugin before any component discovery. Schemas are recognized locally by
//! their canonical identifier and never fetched at runtime.

use serde_json::Value;

pub(crate) const PLUGIN_MANIFEST_SCHEMA_1_0_0: &str =
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub(crate) const SPEC_VERSION_1_0_0: &str = "1.0.0";

const ALLOWED_TOP_LEVEL_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];

const ALLOWED_AUTHOR_FIELDS: &[&str] = &["name", "email", "url"];

#[derive(Clone, Debug)]
pub(crate) struct PluginManifest {
    pub(crate) name: String,
    /// Agent Plugins specification version selected from `$schema`.
    pub(crate) spec_version: String,
    /// Non-fatal schema violations that were reported and ignored.
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "manifest must be a JSON object".to_string())?;

    let mut warnings = Vec::new();
    for key in object.keys() {
        if !ALLOWED_TOP_LEVEL_FIELDS.contains(&key.as_str()) {
            warnings.push(format!("unknown manifest field `{key}` ignored"));
        }
    }
    // Rho implements no extension namespaces, so every member is ignored
    // without validating its value, as required for unimplemented namespaces.
    if let Some(extensions) = object.get("extensions") {
        if !extensions.is_object() {
            warnings.push("non-object `extensions` field ignored".to_string());
        }
    }

    let schema = required_string(object, "$schema")?;
    if schema != PLUGIN_MANIFEST_SCHEMA_1_0_0 {
        return Err(format!("unsupported Agent Plugins schema `{schema}`"));
    }

    let name = required_string(object, "name")?;
    validate_plugin_name(&name)?;

    // Metadata fields are validated only by their JSON types; the spec bars
    // rejecting a manifest over unconstrained values (semver, URLs, SPDX).
    for field in [
        "version",
        "description",
        "homepage",
        "repository",
        "license",
    ] {
        if let Some(value) = object.get(field) {
            if !value.is_string() {
                return Err(format!("`{field}` must be a string"));
            }
        }
    }

    if let Some(author) = object.get("author") {
        let author = author.as_object().ok_or("`author` must be an object")?;
        for (key, value) in author {
            if !ALLOWED_AUTHOR_FIELDS.contains(&key.as_str()) {
                return Err(format!("`author` field `{key}` is not permitted"));
            }
            if !value.is_string() {
                return Err(format!("`author.{key}` must be a string"));
            }
        }
    }

    if let Some(keywords) = object.get("keywords") {
        let keywords = keywords
            .as_array()
            .ok_or("`keywords` must be an array of strings")?;
        if keywords.iter().any(|keyword| !keyword.is_string()) {
            return Err("`keywords` entries must be strings".to_string());
        }
    }

    Ok(PluginManifest {
        name,
        spec_version: SPEC_VERSION_1_0_0.to_string(),
        warnings,
    })
}

fn required_string(object: &serde_json::Map<String, Value>, field: &str) -> Result<String, String> {
    object
        .get(field)
        .ok_or_else(|| format!("missing required `{field}`"))?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("`{field}` must be a string"))
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
