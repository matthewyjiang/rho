//! Typed setting declarations.
//!
//! A [`Setting`] names one key, how its Rust value reads from and writes to
//! the TOML document ([`SettingType`]), and how it reaches the store type
//! that consumers read (`Config` for the agent, the TUI's settings for the
//! `tui` client). [`AnySetting`] erases the value type so one registry can
//! list every setting, load them, save the ones that changed, and describe
//! them as protocol [`SettingSpec`]s.

use std::fmt;

use rho_protocol::{
    Applies, IntegerUnit, SettingChoice, SettingKey, SettingKind, SettingSpec, SettingValue,
};
use toml_edit::{Item, Value};

use super::document::{shown, ConfigDocument};

#[path = "config_setting_model.rs"]
mod model;
pub(crate) use model::{ModelSetting, ModelSpec};

/// When a changed setting takes effect. The protocol's [`Applies`] is
/// derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Apply {
    Immediately,
    NextTurn,
    NextSession,
    Restart,
}

impl Apply {
    fn wire(self) -> Applies {
        match self {
            Self::Immediately => Applies::Immediately,
            Self::NextTurn => Applies::NextTurn,
            Self::NextSession => Applies::NextSession,
            Self::Restart => Applies::Restart,
        }
    }
}

/// How a settings screen presents a setting.
pub(crate) struct SettingMeta {
    pub(crate) category: &'static str,
    pub(crate) section: Option<&'static str>,
    pub(crate) label: &'static str,
    /// What the setting does; [`Self::applies`] says when.
    pub(crate) help: &'static str,
    pub(crate) applies: Apply,
    pub(crate) overridable_per_project: bool,
}

/// One declared setting over the store `S`.
pub(crate) struct Setting<S: 'static, T: SettingType> {
    /// Dotted TOML location, which is also the protocol key.
    pub(crate) key: &'static str,
    pub(crate) meta: SettingMeta,
    pub(crate) spec: T::Spec,
    pub(crate) get: fn(&S) -> T,
    pub(crate) set: fn(&mut S, T),
    /// Per-value checks beyond the type's own, such as a URL's shape.
    pub(crate) validate: fn(&T) -> Result<(), String>,
}

/// [`Setting::validate`] for settings whose type checks everything.
pub(crate) fn any_value<T>(_: &T) -> Result<(), String> {
    Ok(())
}

/// A setting with its value type erased, so one registry lists them all.
pub(crate) trait AnySetting<S>: Sync {
    fn key(&self) -> &'static str;
    /// Reads the setting from `doc` into `store`; an unset key leaves the
    /// store's value alone. The error names the problem, not the key.
    fn load(&self, doc: &ConfigDocument, store: &mut S) -> Result<(), String>;
    /// Writes the setting to `doc` when `after` holds a different value
    /// than `before`.
    fn save(&self, before: &S, after: &S, doc: &mut ConfigDocument);
    /// Whether the key path `path` in a file belongs to this setting.
    fn owns(&self, path: &[String]) -> bool;
    fn spec(&self) -> SettingSpec;
    /// The setting's value in `store` as the protocol carries it; `None`
    /// for an unset optional value.
    fn wire_value(&self, store: &S) -> Option<SettingValue>;
    /// Whether `doc` sets this setting, rather than leaving its default.
    fn is_set(&self, doc: &ConfigDocument) -> bool;
    /// Removes the setting from `doc`, so its default applies even where
    /// the file held a value equal to it.
    fn remove(&self, doc: &mut ConfigDocument);
    /// Writes the value `store` holds to `doc`, even when it is the default.
    fn write(&self, store: &S, doc: &mut ConfigDocument);
    /// Sets the setting in `store` to the protocol `value`, or to its
    /// default for `None`, through the same setter a typed edit uses. The
    /// error names the problem, not the key: a value the setting does not
    /// take, or one it cannot hold yet, such as an internal agent's
    /// reasoning while the agent has no model.
    fn change(&self, store: &mut S, value: Option<&SettingValue>) -> Result<(), String>;
    #[cfg(test)]
    fn applies(&self) -> Apply;
    /// Whether `a` and `b` hold different values.
    #[cfg(test)]
    fn differs(&self, a: &S, b: &S) -> bool;
}

impl<S: Default + 'static, T: SettingType> AnySetting<S> for Setting<S, T> {
    fn key(&self) -> &'static str {
        self.key
    }

    fn load(&self, doc: &ConfigDocument, store: &mut S) -> Result<(), String> {
        let Some(value) = T::read(doc, self.key, &self.spec)? else {
            return Ok(());
        };
        (self.validate)(&value)?;
        (self.set)(store, value);
        Ok(())
    }

    fn save(&self, before: &S, after: &S, doc: &mut ConfigDocument) {
        let value = (self.get)(after);
        if value != (self.get)(before) {
            value.write(doc, self.key);
        }
    }

    fn owns(&self, path: &[String]) -> bool {
        T::owns(self.key, path)
    }

    fn spec(&self) -> SettingSpec {
        SettingSpec {
            key: SettingKey::new(self.key),
            kind: T::kind(&self.spec),
            default: (self.get)(&S::default()).to_wire(),
            category: self.meta.category.into(),
            section: self.meta.section.map(Into::into),
            label: self.meta.label.into(),
            help: self.meta.help.into(),
            applies: self.meta.applies.wire(),
            overridable_per_project: self.meta.overridable_per_project,
        }
    }

    fn wire_value(&self, store: &S) -> Option<SettingValue> {
        (self.get)(store).to_wire()
    }

    fn is_set(&self, doc: &ConfigDocument) -> bool {
        T::read(doc, self.key, &self.spec).is_ok_and(|value| value.is_some())
    }

    fn remove(&self, doc: &mut ConfigDocument) {
        T::remove(doc, self.key);
    }

    fn write(&self, store: &S, doc: &mut ConfigDocument) {
        (self.get)(store).write(doc, self.key);
    }

    fn change(&self, store: &mut S, value: Option<&SettingValue>) -> Result<(), String> {
        let next = match value {
            Some(value) => T::from_wire(value, &self.spec, &(self.get)(store))?,
            None => (self.get)(&S::default()),
        };
        (self.validate)(&next)?;
        let wanted = next.to_wire();
        (self.set)(store, next);
        if self.wire_value(store) != wanted {
            return Err("cannot hold that value; set the setting it belongs to first".into());
        }
        Ok(())
    }

    #[cfg(test)]
    fn applies(&self) -> Apply {
        self.meta.applies
    }

    #[cfg(test)]
    fn differs(&self, a: &S, b: &S) -> bool {
        (self.get)(a) != (self.get)(b)
    }
}

/// A problem found while loading a settings file, by key when it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigError {
    pub(crate) key: Option<String>,
    pub(crate) message: String,
}

impl ConfigError {
    pub(crate) fn at(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            key: Some(key.into()),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            Some(key) => write!(formatter, "`{key}`: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

/// Loads every setting in `settings` from `doc`, collecting each failure.
pub(crate) fn load_settings<S>(
    settings: &[&dyn AnySetting<S>],
    doc: &ConfigDocument,
    store: &mut S,
    errors: &mut Vec<ConfigError>,
) {
    for setting in settings {
        if let Err(message) = setting.load(doc, store) {
            errors.push(ConfigError::at(setting.key(), message));
        }
    }
}

/// Writes every setting whose value differs between `before` and `after`.
pub(crate) fn save_settings<S>(
    settings: &[&dyn AnySetting<S>],
    before: &S,
    after: &S,
    doc: &mut ConfigDocument,
) {
    for setting in settings {
        setting.save(before, after, doc);
    }
}

/// One error per key in `doc` that no setting owns and that is not inside
/// one of the `tables` (dotted keys of hand-edited structured tables).
pub(crate) fn unknown_keys<S>(
    doc: &ConfigDocument,
    settings: &[&dyn AnySetting<S>],
    tables: &[&str],
) -> Vec<ConfigError> {
    doc.leaf_paths()
        .into_iter()
        .filter(|path| {
            !tables.iter().any(|table| path_starts_with(path, table))
                && !settings.iter().any(|setting| setting.owns(path))
        })
        .map(|path| ConfigError {
            key: None,
            message: format!("unknown key `{}`", path.join(".")),
        })
        .collect()
}

/// Whether `path` is `key` or lies under it.
pub(crate) fn path_starts_with(path: &[String], key: &str) -> bool {
    let segments = key.split('.').collect::<Vec<_>>();
    path.len() >= segments.len() && path.iter().zip(&segments).all(|(a, b)| a == b)
}

/// How one Rust value maps to a TOML location and to the protocol.
pub(crate) trait SettingType: Sized + PartialEq + Clone + 'static {
    /// Static facts about the values allowed, such as an integer's range.
    type Spec: Sync;
    fn kind(spec: &Self::Spec) -> SettingKind;
    /// The value at `key`, or `None` when the file leaves it unset.
    fn read(doc: &ConfigDocument, key: &str, spec: &Self::Spec) -> Result<Option<Self>, String>;
    fn write(&self, doc: &mut ConfigDocument, key: &str);
    /// Removes what this setting at `key` reads from `doc`, so its default
    /// applies.
    fn remove(doc: &mut ConfigDocument, key: &str) {
        doc.remove(key);
    }
    /// Whether `path` is a key this setting at `key` reads.
    fn owns(key: &str, path: &[String]) -> bool {
        path.len() == key.split('.').count() && path_starts_with(path, key)
    }
    /// `None` for an unset optional value.
    fn to_wire(&self) -> Option<SettingValue>;
    /// The protocol `value` as this type, if `spec` takes it. `current` is
    /// the value it replaces, which a model's auth may follow.
    fn from_wire(value: &SettingValue, spec: &Self::Spec, current: &Self) -> Result<Self, String>;
}

/// Why a protocol value was refused: it is not the kind `takes` names.
fn takes<T>(kind: &str) -> Result<T, String> {
    Err(format!("takes {kind}"))
}

fn integer_in(value: &SettingValue, spec: &IntegerSpec) -> Result<u64, String> {
    let SettingValue::Integer(number) = value else {
        return takes("a whole number");
    };
    if *number < spec.min {
        return Err(format!("must be at least {}; got {number}", spec.min));
    }
    // The file stores integers as i64.
    let max = spec.max.unwrap_or(i64::MAX as u64);
    if *number > max {
        return Err(format!("must be at most {max}; got {number}"));
    }
    Ok(*number)
}

fn text_of(value: &SettingValue) -> Result<String, String> {
    match value {
        SettingValue::Text(text) => Ok(text.clone()),
        _ => takes("text"),
    }
}

/// The range of an integer setting. Limits name their receipts where the
/// registry declares them.
pub(crate) struct IntegerSpec {
    pub(crate) min: u64,
    pub(crate) max: Option<u64>,
    pub(crate) unit: Option<IntegerUnit>,
}

fn read_value<'a>(doc: &'a ConfigDocument, key: &str) -> Result<Option<&'a Value>, String> {
    match doc.get(key) {
        None => Ok(None),
        Some(Item::Value(value)) => Ok(Some(value)),
        Some(_) => Err("must be a value, not a table".into()),
    }
}

fn read_string(doc: &ConfigDocument, key: &str) -> Result<Option<String>, String> {
    read_value(doc, key)?
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("must be a string; got {}", shown(value)))
        })
        .transpose()
}

fn read_integer(
    doc: &ConfigDocument,
    key: &str,
    spec: &IntegerSpec,
) -> Result<Option<u64>, String> {
    let Some(value) = read_value(doc, key)? else {
        return Ok(None);
    };
    let Some(number) = value.as_integer() else {
        return Err(format!("must be a whole number; got {}", shown(value)));
    };
    let too_small = || format!("must be at least {}; got {number}", spec.min);
    let number_u64 = u64::try_from(number).map_err(|_| too_small())?;
    if number_u64 < spec.min {
        return Err(too_small());
    }
    if let Some(max) = spec.max.filter(|max| number_u64 > *max) {
        return Err(format!("must be at most {max}; got {number}"));
    }
    Ok(Some(number_u64))
}

fn integer_value(value: u64) -> Value {
    // Every registry range fits in i64; saturate rather than wrap if one
    // ever does not.
    Value::from(i64::try_from(value).unwrap_or(i64::MAX))
}

impl SettingType for bool {
    type Spec = ();
    fn kind(_: &()) -> SettingKind {
        SettingKind::Bool
    }
    fn read(doc: &ConfigDocument, key: &str, _: &()) -> Result<Option<Self>, String> {
        read_value(doc, key)?
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| format!("must be true or false; got {}", shown(value)))
            })
            .transpose()
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        doc.set_value(key, Value::from(*self));
    }
    fn to_wire(&self) -> Option<SettingValue> {
        Some(SettingValue::Bool(*self))
    }
    fn from_wire(value: &SettingValue, _: &(), _: &Self) -> Result<Self, String> {
        match value {
            SettingValue::Bool(flag) => Ok(*flag),
            _ => takes("true or false"),
        }
    }
}

impl SettingType for u64 {
    type Spec = IntegerSpec;
    fn kind(spec: &IntegerSpec) -> SettingKind {
        integer_kind(spec)
    }
    fn read(doc: &ConfigDocument, key: &str, spec: &IntegerSpec) -> Result<Option<Self>, String> {
        read_integer(doc, key, spec)
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        doc.set_value(key, integer_value(*self));
    }
    fn to_wire(&self) -> Option<SettingValue> {
        Some(SettingValue::Integer(*self))
    }
    fn from_wire(value: &SettingValue, spec: &IntegerSpec, _: &Self) -> Result<Self, String> {
        integer_in(value, spec)
    }
}

/// An integer that is off until set.
impl SettingType for Option<u64> {
    type Spec = IntegerSpec;
    fn kind(spec: &IntegerSpec) -> SettingKind {
        integer_kind(spec)
    }
    fn read(doc: &ConfigDocument, key: &str, spec: &IntegerSpec) -> Result<Option<Self>, String> {
        Ok(read_integer(doc, key, spec)?.map(Some))
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        match self {
            Some(value) => doc.set_value(key, integer_value(*value)),
            None => doc.remove(key),
        }
    }
    fn to_wire(&self) -> Option<SettingValue> {
        self.map(SettingValue::Integer)
    }
    fn from_wire(value: &SettingValue, spec: &IntegerSpec, _: &Self) -> Result<Self, String> {
        integer_in(value, spec).map(Some)
    }
}

fn integer_kind(spec: &IntegerSpec) -> SettingKind {
    SettingKind::Integer {
        min: spec.min,
        max: spec.max,
        unit: spec.unit,
    }
}

impl SettingType for String {
    type Spec = ();
    fn kind(_: &()) -> SettingKind {
        SettingKind::Text
    }
    fn read(doc: &ConfigDocument, key: &str, _: &()) -> Result<Option<Self>, String> {
        read_string(doc, key)
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        doc.set_value(key, Value::from(self.as_str()));
    }
    fn to_wire(&self) -> Option<SettingValue> {
        Some(SettingValue::Text(self.clone()))
    }
    fn from_wire(value: &SettingValue, _: &(), _: &Self) -> Result<Self, String> {
        text_of(value)
    }
}

/// Text that is unset until set, such as an endpoint override.
impl SettingType for Option<String> {
    type Spec = ();
    fn kind(_: &()) -> SettingKind {
        SettingKind::Text
    }
    fn read(doc: &ConfigDocument, key: &str, _: &()) -> Result<Option<Self>, String> {
        Ok(read_string(doc, key)?.map(Some))
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        match self {
            Some(value) => doc.set_value(key, Value::from(value.as_str())),
            None => doc.remove(key),
        }
    }
    fn to_wire(&self) -> Option<SettingValue> {
        self.clone().map(SettingValue::Text)
    }
    fn from_wire(value: &SettingValue, _: &(), _: &Self) -> Result<Self, String> {
        text_of(value).map(Some)
    }
}

/// A closed set of string values, such as a permission mode.
pub(crate) trait Choice: Copy + PartialEq + Sync + 'static {
    fn choices() -> Vec<Self>;
    /// What the file and the protocol hold.
    fn choice_value(self) -> &'static str;
    fn choice_label(self) -> &'static str;
    fn choice_help(self) -> Option<&'static str> {
        None
    }
}

fn choice_kind<T: Choice>() -> SettingKind {
    SettingKind::Choice {
        choices: T::choices()
            .into_iter()
            .map(|choice| SettingChoice {
                value: choice.choice_value().into(),
                label: choice.choice_label().into(),
                help: choice.choice_help().map(Into::into),
            })
            .collect(),
    }
}

fn read_choice<T: Choice>(doc: &ConfigDocument, key: &str) -> Result<Option<T>, String> {
    let Some(value) = read_value(doc, key)? else {
        return Ok(None);
    };
    let choices = T::choices();
    choices
        .iter()
        .copied()
        .find(|choice| value.as_str() == Some(choice.choice_value()))
        .map(Some)
        .ok_or_else(|| {
            let expected = choices
                .iter()
                .map(|choice| choice.choice_value())
                .collect::<Vec<_>>()
                .join(", ");
            format!("must be one of {expected}; got {}", shown(value))
        })
}

fn choice_from_wire<T: Choice>(value: &SettingValue) -> Result<T, String> {
    let text = text_of(value).or_else(|_| takes("one of its choices"))?;
    let choices = T::choices();
    choices
        .iter()
        .copied()
        .find(|choice| choice.choice_value() == text)
        .ok_or_else(|| {
            let expected = choices
                .iter()
                .map(|choice| choice.choice_value())
                .collect::<Vec<_>>()
                .join(", ");
            format!("must be one of {expected}; got \"{text}\"")
        })
}

/// Implements [`SettingType`] for [`Choice`] types and for their optional
/// (unset until chosen) form.
macro_rules! choice_setting_types {
    ($($choice:ty),+ $(,)?) => {$(
        impl SettingType for $choice {
            type Spec = ();
            fn kind(_: &()) -> SettingKind {
                choice_kind::<$choice>()
            }
            fn read(doc: &ConfigDocument, key: &str, _: &()) -> Result<Option<Self>, String> {
                read_choice(doc, key)
            }
            fn write(&self, doc: &mut ConfigDocument, key: &str) {
                doc.set_value(key, Value::from(self.choice_value()));
            }
            fn to_wire(&self) -> Option<SettingValue> {
                Some(SettingValue::Text(self.choice_value().into()))
            }
            fn from_wire(value: &SettingValue, _: &(), _: &Self) -> Result<Self, String> {
                choice_from_wire(value)
            }
        }

        impl SettingType for Option<$choice> {
            type Spec = ();
            fn kind(_: &()) -> SettingKind {
                choice_kind::<$choice>()
            }
            fn read(doc: &ConfigDocument, key: &str, _: &()) -> Result<Option<Self>, String> {
                Ok(read_choice(doc, key)?.map(Some))
            }
            fn write(&self, doc: &mut ConfigDocument, key: &str) {
                match self {
                    Some(choice) => doc.set_value(key, Value::from(choice.choice_value())),
                    None => doc.remove(key),
                }
            }
            fn to_wire(&self) -> Option<SettingValue> {
                self.map(|choice| SettingValue::Text(choice.choice_value().into()))
            }
            fn from_wire(value: &SettingValue, _: &(), _: &Self) -> Result<Self, String> {
                choice_from_wire(value).map(Some)
            }
        }
    )+};
}

choice_setting_types!(
    crate::permission::PermissionMode,
    rho_providers::reasoning::ReasoningLevel,
    super::WebSearchMode,
    super::SearchBackend,
    super::OpenAiSearchConnection,
    super::ExaSearchConnection,
    super::CodemodeMode,
    super::EditTool,
    rho_providers::credentials::CredentialStoreBackend,
    super::StreamingMode,
    crate::keybindings::EditingMode,
);

impl Choice for crate::permission::PermissionMode {
    fn choices() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        self.label()
    }
}

impl Choice for rho_providers::reasoning::ReasoningLevel {
    fn choices() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn choice_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
    fn choice_label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Xhigh => "Extra high",
            Self::Max => "Max",
        }
    }
}

impl Choice for super::WebSearchMode {
    fn choices() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        self.label()
    }
}

impl Choice for super::SearchBackend {
    fn choices() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        self.label()
    }
}

impl Choice for super::OpenAiSearchConnection {
    fn choices() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        self.label()
    }
}

impl Choice for super::ExaSearchConnection {
    fn choices() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        self.label()
    }
}

impl Choice for super::CodemodeMode {
    fn choices() -> Vec<Self> {
        vec![Self::On, Self::Only]
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Only => "Only",
        }
    }
}

impl Choice for super::EditTool {
    fn choices() -> Vec<Self> {
        Self::all()
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        self.label()
    }
    fn choice_help(self) -> Option<&'static str> {
        Some(self.detail())
    }
}

impl Choice for rho_providers::credentials::CredentialStoreBackend {
    fn choices() -> Vec<Self> {
        vec![Self::Os, Self::File]
    }
    fn choice_value(self) -> &'static str {
        self.as_str()
    }
    fn choice_label(self) -> &'static str {
        match self {
            Self::Os => "OS keyring",
            Self::File => "File",
        }
    }
}

impl Choice for super::StreamingMode {
    fn choices() -> Vec<Self> {
        vec![Self::Live, Self::Paragraph, Self::Off]
    }
    fn choice_value(self) -> &'static str {
        self.label()
    }
    fn choice_label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Paragraph => "Paragraph",
            Self::Off => "Off",
        }
    }
}

impl Choice for crate::keybindings::EditingMode {
    fn choices() -> Vec<Self> {
        vec![Self::Default, Self::Vim]
    }
    fn choice_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Vim => "vim",
        }
    }
    fn choice_label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Vim => "Vim",
        }
    }
}

#[cfg(test)]
#[path = "config_setting_tests.rs"]
mod tests;
