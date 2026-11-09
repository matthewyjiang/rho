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
    Applies, IntegerUnit, ModelRuntime, SettingChoice, SettingKey, SettingKind, SettingSpec,
    SettingValue,
};
use toml_edit::{Item, Value};

use super::document::{shown, ConfigDocument};

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
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "rho/config/schema serves it in the next commit")
    )]
    fn spec(&self) -> SettingSpec;
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
    /// Whether `path` is a key this setting at `key` reads.
    fn owns(key: &str, path: &[String]) -> bool {
        path.len() == key.split('.').count() && path_starts_with(path, key)
    }
    /// `None` for an unset optional value.
    fn to_wire(&self) -> Option<SettingValue>;
}

/// The range of an integer setting. Limits name their receipts where the
/// registry declares them.
pub(crate) struct IntegerSpec {
    pub(crate) min: u64,
    pub(crate) max: Option<u64>,
    pub(crate) unit: Option<IntegerUnit>,
}

/// The runtimes a model setting may pick.
pub(crate) struct ModelSpec {
    pub(crate) runtimes: &'static [ModelRuntime],
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

/// The model a model setting holds: the `runtime`, `provider`, `model`, and
/// `auth` leaves of its table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelSetting {
    Rho {
        provider: String,
        /// A concrete model id, or `@alias`.
        model: String,
        auth: String,
    },
    ClaudeCli {
        model: Option<String>,
    },
}

const MODEL_LEAVES: [&str; 4] = ["runtime", "provider", "model", "auth"];

fn read_model(
    doc: &ConfigDocument,
    key: &str,
    spec: &ModelSpec,
) -> Result<Option<ModelSetting>, String> {
    if doc.get(key).is_some_and(|item| !item.is_table_like()) {
        return Err("must be a table with provider and model keys".into());
    }
    let leaf = |name: &str| read_string(doc, &format!("{key}.{name}"));
    let (runtime, provider, model, auth) = (
        leaf("runtime")?,
        leaf("provider")?,
        leaf("model")?,
        leaf("auth")?,
    );
    if runtime.is_none() && provider.is_none() && model.is_none() && auth.is_none() {
        return Ok(None);
    }
    let runtime_allowed = |runtime: ModelRuntime| spec.runtimes.contains(&runtime);
    match runtime.as_deref().unwrap_or(super::RHO_RUNTIME_KEY) {
        super::RHO_RUNTIME_KEY if runtime_allowed(ModelRuntime::Rho) => {
            Ok(Some(ModelSetting::Rho {
                provider: provider.unwrap_or_else(|| super::DEFAULT_PROVIDER.into()),
                model: model.unwrap_or_else(|| super::DEFAULT_MODEL.into()),
                auth: auth.unwrap_or_else(|| super::DEFAULT_AUTH.into()),
            }))
        }
        super::CLAUDE_CLI_RUNTIME_KEY if runtime_allowed(ModelRuntime::ClaudeCli) => {
            if provider.is_some() || auth.is_some() {
                return Err(format!(
                    "runtime {} takes no provider or auth",
                    super::CLAUDE_CLI_RUNTIME_KEY
                ));
            }
            Ok(Some(ModelSetting::ClaudeCli { model }))
        }
        other => {
            let expected = spec
                .runtimes
                .iter()
                .filter_map(|runtime| match runtime {
                    ModelRuntime::Rho => Some(super::RHO_RUNTIME_KEY),
                    ModelRuntime::ClaudeCli => Some(super::CLAUDE_CLI_RUNTIME_KEY),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "runtime must be one of {expected}; got \"{other}\""
            ))
        }
    }
}

fn write_model(model: &ModelSetting, doc: &mut ConfigDocument, key: &str) {
    match model {
        ModelSetting::Rho {
            provider,
            model,
            auth,
        } => {
            doc.remove(&format!("{key}.runtime"));
            doc.set_value(&format!("{key}.provider"), Value::from(provider.as_str()));
            doc.set_value(&format!("{key}.model"), Value::from(model.as_str()));
            doc.set_value(&format!("{key}.auth"), Value::from(auth.as_str()));
        }
        ModelSetting::ClaudeCli { model } => {
            doc.set_value(
                &format!("{key}.runtime"),
                Value::from(super::CLAUDE_CLI_RUNTIME_KEY),
            );
            doc.remove(&format!("{key}.provider"));
            doc.remove(&format!("{key}.auth"));
            match model {
                Some(model) => doc.set_value(&format!("{key}.model"), Value::from(model.as_str())),
                None => doc.remove(&format!("{key}.model")),
            }
        }
    }
}

fn model_wire(model: &ModelSetting) -> SettingValue {
    SettingValue::Model(match model {
        ModelSetting::Rho {
            provider, model, ..
        } => rho_protocol::ModelSelection::Rho {
            provider: provider.clone(),
            model: model.clone(),
        },
        ModelSetting::ClaudeCli { model } => rho_protocol::ModelSelection::ClaudeCli {
            model: model.clone(),
        },
    })
}

fn owns_model_leaf(key: &str, path: &[String]) -> bool {
    let depth = key.split('.').count();
    path_starts_with(path, key)
        && (path.len() == depth
            || (path.len() == depth + 1 && MODEL_LEAVES.contains(&path[depth].as_str())))
}

impl SettingType for ModelSetting {
    type Spec = ModelSpec;
    fn kind(spec: &ModelSpec) -> SettingKind {
        SettingKind::Model {
            runtimes: spec.runtimes.to_vec(),
        }
    }
    fn read(doc: &ConfigDocument, key: &str, spec: &ModelSpec) -> Result<Option<Self>, String> {
        read_model(doc, key, spec)
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        write_model(self, doc, key);
    }
    fn owns(key: &str, path: &[String]) -> bool {
        owns_model_leaf(key, path)
    }
    fn to_wire(&self) -> Option<SettingValue> {
        Some(model_wire(self))
    }
}

/// A model that is unset until chosen, such as an internal agent's.
impl SettingType for Option<ModelSetting> {
    type Spec = ModelSpec;
    fn kind(spec: &ModelSpec) -> SettingKind {
        ModelSetting::kind(spec)
    }
    fn read(doc: &ConfigDocument, key: &str, spec: &ModelSpec) -> Result<Option<Self>, String> {
        Ok(read_model(doc, key, spec)?.map(Some))
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        match self {
            Some(model) => write_model(model, doc, key),
            None => {
                for leaf in MODEL_LEAVES {
                    doc.remove(&format!("{key}.{leaf}"));
                }
            }
        }
    }
    fn owns(key: &str, path: &[String]) -> bool {
        owns_model_leaf(key, path)
    }
    fn to_wire(&self) -> Option<SettingValue> {
        self.as_ref().map(model_wire)
    }
}

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

/// Whether a spec's default is a value its own kind accepts, so a declared
/// range or choice list cannot reject the setting's own default.
#[cfg(test)]
pub(crate) fn default_fits_kind(spec: &SettingSpec) -> bool {
    match (&spec.kind, &spec.default) {
        (_, None) => true,
        (SettingKind::Bool, Some(SettingValue::Bool(_)))
        | (SettingKind::Text, Some(SettingValue::Text(_))) => true,
        (SettingKind::Integer { min, max, .. }, Some(SettingValue::Integer(value))) => {
            value >= min && max.is_none_or(|max| *value <= max)
        }
        (SettingKind::Choice { choices }, Some(SettingValue::Text(value))) => {
            choices.iter().any(|choice| &choice.value == value)
        }
        (SettingKind::Model { runtimes }, Some(SettingValue::Model(selection))) => runtimes
            .contains(&match selection {
                rho_protocol::ModelSelection::Rho { .. } => ModelRuntime::Rho,
                rho_protocol::ModelSelection::ClaudeCli { .. } => ModelRuntime::ClaudeCli,
            }),
        _ => false,
    }
}

#[cfg(test)]
#[path = "config_setting_tests.rs"]
mod tests;
