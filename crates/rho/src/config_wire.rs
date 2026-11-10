//! Settings as the daemon protocol serves them: the agent schema, the values
//! a document holds, and changes written into a document.
//!
//! An agent change goes through the registry's typed setters, as an edit in
//! Rust would, then the edited config is saved as a diff and loaded again,
//! which runs every check across settings. A client's settings have no Rust
//! type in the daemon, so a small codec reads and writes them by their
//! [`SettingKind`] alone; clients register only plain values.

use std::collections::BTreeMap;

use rho_protocol::{
    ConfigCategory, ConfigSchema, ModelRuntime, ModelSelection, SettingChange, SettingEntry,
    SettingKey, SettingKind, SettingSpec, SettingValue, ValueSource,
};
use toml_edit::Value;

use super::{
    document::{shown, ConfigDocument},
    load,
    registry::{AGENT_SETTINGS, CREDENTIAL_STORE},
    setting::{save_settings, AnySetting, ConfigError},
    Config, ConfigWarning,
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
            let key = SettingKey::new(setting.key());
            let entry = SettingEntry {
                value: setting.wire_value(config),
                source: if setting.is_set(doc) {
                    ValueSource::Global
                } else {
                    ValueSource::Default
                },
            };
            (key, entry)
        })
        .collect()
}

/// Why agent changes were not applied; `doc` is left as it was.
#[derive(Debug, PartialEq)]
pub(crate) enum AgentChangeError {
    /// A change a setting refuses, an unknown key, or a result that would
    /// not load, such as a compaction target above its threshold.
    Refused(Vec<ConfigError>),
    /// The file does not load for keys the changes leave alone; a hand
    /// edit must fix it.
    FileInvalid(Vec<ConfigError>),
}

/// Applies protocol `changes`, in order, to the agent settings `doc`
/// holds: each through its registry setter, as a typed edit would, then
/// every setting that changed written back leaf by leaf, keeping the rest
/// of the file. A set writes its value even when it is the default; a reset
/// removes the key. A file that does not load is repaired when the changes
/// replace every key it fails on. Returns the config `doc` now loads as.
pub(crate) fn apply_agent_changes(
    doc: &mut ConfigDocument,
    changes: &[SettingChange],
) -> Result<(Config, Vec<ConfigWarning>), AgentChangeError> {
    let mut changed = Vec::new();
    let mut errors = Vec::new();
    for change in changes {
        let key = change.key();
        match AGENT_SETTINGS
            .iter()
            .find(|setting| setting.key() == key.as_str())
        {
            None => errors.push(ConfigError::at(key.as_str(), "no such setting")),
            Some(setting) if setting.key() == CREDENTIAL_STORE.key => errors.push(ConfigError::at(
                key.as_str(),
                "change it with rho/auth/choose_store",
            )),
            Some(setting) => changed.push((*setting, change)),
        }
    }
    let mut edited = loadable_base(doc, &changed)?;
    let (before, _) = load::resolve(&edited).map_err(AgentChangeError::FileInvalid)?;
    let mut after = before.clone();
    for (setting, change) in &changed {
        let value = match change {
            SettingChange::Set { value, .. } => Some(value),
            SettingChange::Reset { .. } => None,
        };
        if let Err(message) = setting.change(&mut after, value) {
            errors.push(ConfigError::at(setting.key(), message));
        }
    }
    // Auto runs only with a classifier model: a session opened on it would
    // fail. Checked only when these changes make it so, so a change
    // elsewhere still saves on a file already in that state.
    let auto_without_classifier = |config: &Config| {
        config.permission_mode == crate::permission::PermissionMode::Auto
            && config
                .internal_agent_model(crate::agent::PERMISSION_CLASSIFIER_AGENT_ID)
                .is_none()
    };
    if auto_without_classifier(&after) && !auto_without_classifier(&before) {
        errors.push(ConfigError::at(
            "behavior.permission_mode",
            "auto needs a permission classifier model; choose one first",
        ));
    }
    if !errors.is_empty() {
        return Err(AgentChangeError::Refused(errors));
    }
    // Setters reach past their own setting, such as a model change moving
    // its auth, so every setting that differs is saved; internal agents
    // by their own rows, leaving the rest of `[internal_agents]` as written.
    save_settings(AGENT_SETTINGS, &before, &after, &mut edited);
    for (setting, change) in &changed {
        match change {
            SettingChange::Set { .. } => setting.write(&after, &mut edited),
            SettingChange::Reset { .. } => setting.remove(&mut edited),
        }
    }
    let resolved = load::resolve(&edited).map_err(AgentChangeError::Refused)?;
    *doc = edited;
    Ok(resolved)
}

/// `doc`, or a copy without the values it fails to load on when `changed`
/// replaces every one of them, so a change can repair a hand edit.
fn loadable_base(
    doc: &ConfigDocument,
    changed: &[(&dyn AnySetting<Config>, &SettingChange)],
) -> Result<ConfigDocument, AgentChangeError> {
    let Err(errors) = load::resolve(doc) else {
        return Ok(doc.clone());
    };
    let replaced = |error: &ConfigError| {
        let key = error.key.as_deref()?;
        changed
            .iter()
            .map(|(setting, _)| *setting)
            .find(|setting| key == setting.key() || key.starts_with(&format!("{}.", setting.key())))
    };
    let mut base = doc.clone();
    for error in &errors {
        let Some(setting) = replaced(error) else {
            return Err(AgentChangeError::FileInvalid(errors));
        };
        setting.remove(&mut base);
    }
    Ok(base)
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

/// Writes every change into a client's `doc`, each by its setting in
/// `specs`, or reports every change it refuses: an unknown key, or a value
/// its setting does not take.
pub(crate) fn write_changes(
    doc: &mut ConfigDocument,
    specs: &[SettingSpec],
    changes: &[SettingChange],
) -> Result<(), Vec<ConfigError>> {
    let mut errors = Vec::new();
    for change in changes {
        let key = change.key();
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
            SettingChange::Reset { .. } => doc.remove(key.as_str()),
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
        if !default_fits_kind(spec) {
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
        // Clients register no model settings, and agent changes take the
        // typed path.
        SettingValue::Model(_) => return Err("takes a plain value, not a model".into()),
    }
    Ok(())
}

/// Whether a spec's default is a value its own kind accepts, so a declared
/// range or choice list cannot reject the setting's own default.
pub(crate) fn default_fits_kind(spec: &SettingSpec) -> bool {
    spec.default
        .as_ref()
        .is_none_or(|value| check(&spec.kind, value).is_ok())
}

#[cfg(test)]
#[path = "config_wire_tests.rs"]
mod tests;
