//! Settings as the daemon protocol serves them: the agent schema, the values
//! a document holds, and changes written into a document.
//!
//! The codec reads and writes a value by its [`SettingKind`] alone, so it
//! also stores the settings a client registers, which have no Rust type in
//! the daemon. A change to the agent's file is checked again by loading the
//! edited document through the registry, which runs every typed check.

use std::collections::BTreeMap;

use rho_protocol::{
    ConfigCategory, ConfigSchema, ModelRuntime, ModelSelection, SettingChange, SettingEntry,
    SettingKey, SettingKind, SettingSpec, SettingValue, ValueSource,
};
use toml_edit::Value;

use super::{
    document::{shown, ConfigDocument},
    inferred_provider_auth, load,
    registry::AGENT_SETTINGS,
    setting::ConfigError,
    Config, CLAUDE_CLI_RUNTIME_KEY, DEFAULT_AUTH, DEFAULT_MODEL, DEFAULT_PROVIDER,
};

/// The agent's settings pages, in display order. Every registry row names
/// one of these ids.
const CATEGORIES: [(&str, &str); 5] = [
    ("models", "Models"),
    ("agent", "Agent"),
    ("context", "Context"),
    ("tools", "Tools"),
    ("providers", "Providers"),
];

/// Every agent setting, with the pages they sit on and the tables only a
/// hand edit changes.
pub(crate) fn agent_schema() -> ConfigSchema {
    ConfigSchema {
        categories: CATEGORIES
            .iter()
            .map(|(id, label)| ConfigCategory {
                id: (*id).into(),
                label: (*label).into(),
            })
            .collect(),
        settings: AGENT_SETTINGS
            .iter()
            .map(|setting| setting.spec())
            .collect(),
        file_only: load::file_only_tables(),
    }
}

/// The agent settings `config` holds, each marked as set by `doc` or
/// defaulted.
pub(crate) fn agent_values(
    doc: &ConfigDocument,
    config: &Config,
) -> BTreeMap<SettingKey, SettingEntry> {
    AGENT_SETTINGS
        .iter()
        .map(|setting| {
            let SettingSpec { key, kind, .. } = setting.spec();
            let set = read(doc, key.as_str(), &kind).is_ok_and(|value| value.is_some());
            let entry = SettingEntry {
                value: setting.wire_value(config),
                source: if set {
                    ValueSource::Global
                } else {
                    ValueSource::Default
                },
            };
            (key, entry)
        })
        .collect()
}

/// The values `doc` holds for `specs`, defaults for the ones it leaves
/// unset, and one error per value it holds that its spec refuses, which
/// then reads as its default.
pub(crate) fn spec_values(
    doc: &ConfigDocument,
    specs: &[SettingSpec],
) -> (BTreeMap<SettingKey, SettingEntry>, Vec<ConfigError>) {
    let mut errors = Vec::new();
    let values = specs
        .iter()
        .map(|spec| {
            let entry = match read(doc, spec.key.as_str(), &spec.kind) {
                Ok(Some(value)) => SettingEntry {
                    value: Some(value),
                    source: ValueSource::Global,
                },
                Ok(None) => default_entry(spec),
                Err(message) => {
                    errors.push(ConfigError::at(spec.key.as_str(), message));
                    default_entry(spec)
                }
            };
            (spec.key.clone(), entry)
        })
        .collect();
    (values, errors)
}

fn default_entry(spec: &SettingSpec) -> SettingEntry {
    SettingEntry {
        value: spec.default.clone(),
        source: ValueSource::Default,
    }
}

/// Writes every change into `doc`, each by its setting in `specs`, or
/// reports every change it refuses: an unknown key, or a value its setting
/// does not take.
pub(crate) fn write_changes(
    doc: &mut ConfigDocument,
    specs: &[SettingSpec],
    changes: &[SettingChange],
) -> Result<(), Vec<ConfigError>> {
    let mut errors = Vec::new();
    for change in changes {
        let key = match change {
            SettingChange::Set { key, .. } | SettingChange::Reset { key } => key,
        };
        let Some(spec) = specs.iter().find(|spec| &spec.key == key) else {
            errors.push(ConfigError::at(key.as_str(), "no such setting"));
            continue;
        };
        match change {
            SettingChange::Set { value, .. } => {
                if let Err(message) = write(doc, key.as_str(), &spec.kind, value) {
                    errors.push(ConfigError::at(key.as_str(), message));
                }
            }
            SettingChange::Reset { .. } => remove(doc, key.as_str(), &spec.kind),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Why a client's settings cannot be stored, or `None` when they can: each
/// takes a plain value, its key is a dotted path of bare TOML keys no other
/// setting uses or lies under, and its default is a value its kind takes.
pub(crate) fn client_specs_problem(specs: &[SettingSpec]) -> Option<String> {
    for (index, spec) in specs.iter().enumerate() {
        let key = spec.key.as_str();
        let bare = |segment: &str| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        };
        if !key.split('.').all(bare) {
            return Some(format!(
                "setting key `{key}` must be dotted letters, digits, `_`, or `-`"
            ));
        }
        if matches!(spec.kind, SettingKind::Model { .. } | SettingKind::Unknown) {
            return Some(format!(
                "setting `{key}` must be a bool, choice, integer, or text setting"
            ));
        }
        if !super::setting::default_fits_kind(spec) {
            return Some(format!("setting `{key}` has a default its kind refuses"));
        }
        let overlaps = |other: &SettingSpec| {
            let other = other.key.as_str();
            other == key
                || other.starts_with(&format!("{key}."))
                || key.starts_with(&format!("{other}."))
        };
        if specs[..index].iter().any(overlaps) {
            return Some(format!("setting key `{key}` repeats or nests another key"));
        }
    }
    None
}

/// The value `doc` holds at `key`, read as `kind`; `None` when unset.
fn read(
    doc: &ConfigDocument,
    key: &str,
    kind: &SettingKind,
) -> Result<Option<SettingValue>, String> {
    if let SettingKind::Model { .. } = kind {
        return read_model(doc, key);
    }
    let Some(item) = doc.get(key) else {
        return Ok(None);
    };
    let value = item
        .as_value()
        .ok_or_else(|| "must be a value, not a table".to_owned())?;
    check(kind, &plain(value)?).map(Some)
}

/// `value` as the protocol value of its TOML type.
fn plain(value: &Value) -> Result<SettingValue, String> {
    match value {
        Value::Boolean(flag) => Ok(SettingValue::Bool(*flag.value())),
        Value::String(text) => Ok(SettingValue::Text(text.value().clone())),
        Value::Integer(number) => u64::try_from(*number.value())
            .map(SettingValue::Integer)
            .map_err(|_| format!("must not be negative; got {}", shown(value))),
        _ => Err(format!(
            "must be true, false, a number, or a string; got {}",
            shown(value)
        )),
    }
}

/// `value` if `kind` takes it, else why not.
fn check(kind: &SettingKind, value: &SettingValue) -> Result<SettingValue, String> {
    match (kind, value) {
        (SettingKind::Bool, SettingValue::Bool(_)) | (SettingKind::Text, SettingValue::Text(_)) => {
        }
        (SettingKind::Integer { min, max, .. }, SettingValue::Integer(number)) => {
            if number < min {
                return Err(format!("must be at least {min}; got {number}"));
            }
            if let Some(max) = max.filter(|max| number > max) {
                return Err(format!("must be at most {max}; got {number}"));
            }
            if i64::try_from(*number).is_err() {
                return Err(format!("must be at most {}; got {number}", i64::MAX));
            }
        }
        (SettingKind::Choice { choices }, SettingValue::Text(text)) => {
            if !choices.iter().any(|choice| &choice.value == text) {
                let expected = choices
                    .iter()
                    .map(|choice| choice.value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!("must be one of {expected}; got \"{text}\""));
            }
        }
        (SettingKind::Model { runtimes }, SettingValue::Model(selection)) => {
            let runtime = match selection {
                ModelSelection::Rho { .. } => ModelRuntime::Rho,
                ModelSelection::ClaudeCli { .. } => ModelRuntime::ClaudeCli,
            };
            if !runtimes.contains(&runtime) {
                return Err("does not run on that runtime".into());
            }
        }
        (kind, _) => return Err(format!("takes {}", kind_name(kind))),
    }
    Ok(value.clone())
}

fn kind_name(kind: &SettingKind) -> &'static str {
    match kind {
        SettingKind::Bool => "true or false",
        SettingKind::Choice { .. } => "one of its choices",
        SettingKind::Integer { .. } => "a whole number",
        SettingKind::Text => "text",
        SettingKind::Model { .. } => "a model",
        _ => "a kind this daemon does not know",
    }
}

/// Writes `value` at `key` if `kind` takes it.
fn write(
    doc: &mut ConfigDocument,
    key: &str,
    kind: &SettingKind,
    value: &SettingValue,
) -> Result<(), String> {
    match check(kind, value)? {
        SettingValue::Bool(flag) => doc.set_value(key, Value::from(flag)),
        SettingValue::Text(text) => doc.set_value(key, Value::from(text)),
        SettingValue::Integer(number) => doc.set_value(
            key,
            Value::from(i64::try_from(number).expect("check bounds integers to i64")),
        ),
        SettingValue::Model(selection) => write_model(doc, key, &selection),
    }
    Ok(())
}

/// Removes the setting at `key`, so its default applies.
fn remove(doc: &mut ConfigDocument, key: &str, kind: &SettingKind) {
    match kind {
        SettingKind::Model { .. } => {
            for leaf in MODEL_LEAVES {
                doc.remove(&format!("{key}.{leaf}"));
            }
        }
        _ => doc.remove(key),
    }
}

const MODEL_LEAVES: [&str; 4] = ["runtime", "provider", "model", "auth"];

fn leaf(doc: &ConfigDocument, key: &str, leaf: &str) -> Option<String> {
    doc.get(&format!("{key}.{leaf}"))?
        .as_str()
        .map(str::to_owned)
}

/// The model the leaves of the table at `key` select; `None` when it has
/// none. The registry's loader checks the leaves' types.
fn read_model(doc: &ConfigDocument, key: &str) -> Result<Option<SettingValue>, String> {
    if MODEL_LEAVES
        .iter()
        .all(|name| doc.get(&format!("{key}.{name}")).is_none())
    {
        return Ok(None);
    }
    let model = leaf(doc, key, "model");
    Ok(Some(SettingValue::Model(
        match leaf(doc, key, "runtime").as_deref() {
            Some(CLAUDE_CLI_RUNTIME_KEY) => ModelSelection::ClaudeCli { model },
            _ => ModelSelection::Rho {
                provider: leaf(doc, key, "provider").unwrap_or_else(|| DEFAULT_PROVIDER.into()),
                model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
            },
        },
    )))
}

/// Writes `selection` into the leaves of the table at `key`. A Rho model
/// keeps the table's auth while its provider stays, else takes the new
/// provider's default auth.
fn write_model(doc: &mut ConfigDocument, key: &str, selection: &ModelSelection) {
    let path = |leaf: &str| format!("{key}.{leaf}");
    match selection {
        ModelSelection::Rho { provider, model } => {
            let auth = inferred_provider_auth(
                provider,
                &leaf(doc, key, "provider").unwrap_or_else(|| DEFAULT_PROVIDER.into()),
                &leaf(doc, key, "auth").unwrap_or_else(|| DEFAULT_AUTH.into()),
            );
            doc.remove(&path("runtime"));
            doc.set_value(&path("provider"), Value::from(provider.as_str()));
            doc.set_value(&path("model"), Value::from(model.as_str()));
            doc.set_value(&path("auth"), Value::from(auth));
        }
        ModelSelection::ClaudeCli { model } => {
            doc.set_value(&path("runtime"), Value::from(CLAUDE_CLI_RUNTIME_KEY));
            doc.remove(&path("provider"));
            doc.remove(&path("auth"));
            match model {
                Some(model) => doc.set_value(&path("model"), Value::from(model.as_str())),
                None => doc.remove(&path("model")),
            }
        }
    }
}

#[cfg(test)]
#[path = "config_wire_tests.rs"]
mod tests;
