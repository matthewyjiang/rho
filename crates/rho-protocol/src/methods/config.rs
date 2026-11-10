//! How the daemon describes one setting: its key, the values it takes,
//! its default, and when a change takes effect. Rho declares every setting
//! once and builds these from that declaration, so a frontend renders a
//! settings screen from data instead of knowing each setting.

use std::{collections::BTreeMap, fmt};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A setting's dotted name, which is also where `config.toml` keeps it:
/// `behavior.permission_mode` is the `permission_mode` key of the
/// `[behavior]` table.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SettingKey(String);

impl SettingKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SettingKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One setting, as a settings screen shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingSpec {
    pub key: SettingKey,
    pub kind: SettingKind,
    /// Absent for an optional setting that is unset by default, such as a
    /// timeout that is off until set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<SettingValue>,
    /// The id of the category whose page lists the setting.
    pub category: String,
    /// A heading within the category page, when the page groups its rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub label: String,
    /// What the setting does, without when it applies: [`Self::applies`]
    /// says that.
    pub help: String,
    pub applies: Applies,
    /// Whether a project may override the setting once project config
    /// exists; settings that size the daemon itself never may.
    pub overridable_per_project: bool,
}

/// The values a setting takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SettingKind {
    Bool,
    /// One of `choices`, by [`SettingChoice::value`].
    Choice {
        choices: Vec<SettingChoice>,
    },
    /// A whole number from `min` to `max`, inclusive; no `max` means no
    /// upper bound.
    Integer {
        min: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<IntegerUnit>,
    },
    Text,
    /// A model on one of `runtimes`.
    Model {
        runtimes: Vec<ModelRuntime>,
    },
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One value of a [`SettingKind::Choice`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingChoice {
    /// What `config.toml` and [`SettingValue::Text`] hold.
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// What a [`SettingKind::Integer`] counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IntegerUnit {
    Bytes,
    Lines,
    Percent,
    Seconds,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// What runs a [`SettingKind::Model`] setting's model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ModelRuntime {
    /// Rho's own model stack.
    Rho,
    /// The Claude Code CLI, listed only where the daemon can run it.
    ClaudeCli,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// When a changed setting takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Applies {
    /// As soon as the change is saved.
    Immediately,
    /// From the next turn of every loaded session.
    NextTurn,
    /// From the next session the daemon opens.
    NextSession,
    /// When the daemon next starts.
    Restart,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A setting's value; which variant a setting takes follows its
/// [`SettingKind`]: a choice is [`Self::Text`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Integer(u64),
    Text(String),
    Model(ModelSelection),
}

/// The value of a [`SettingKind::Model`] setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "runtime", rename_all = "kebab-case")]
pub enum ModelSelection {
    Rho {
        provider: String,
        model: String,
    },
    /// Absent `model` lets the Claude Code CLI pick its own default.
    ClaudeCli {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

/// Describe every agent setting: [`ConfigSchema`].
pub const CONFIG_SCHEMA: &str = "rho/config/schema";
/// Declare a client's own settings, which the daemon then stores for it.
pub const CONFIG_REGISTER: &str = "rho/config/register";
/// Read a namespace's values and subscribe this connection to
/// [`CONFIG_CHANGED`] for it.
pub const CONFIG_GET: &str = "rho/config/get";
/// Change or reset settings of a namespace in one atomic write.
pub const CONFIG_SET: &str = "rho/config/set";
/// Notification: settings of a namespace this connection read changed.
pub const CONFIG_CHANGED: &str = "rho/config/changed";

/// Whose settings a request names.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConfigNamespace {
    /// The agent's settings in `config.toml`, described by
    /// [`CONFIG_SCHEMA`].
    Agent,
    /// A client's own settings, described by its [`CONFIG_REGISTER`].
    Client { name: ClientName },
    /// A namespace a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A client's settings namespace, such as `tui`. It names the file the
/// daemon stores them in, so it is lowercase ASCII letters, digits, and
/// `-`, starting with a letter or digit, at most [`Self::MAX_LEN`] bytes.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct ClientName(String);

impl ClientName {
    /// The longest name: a file name holds 255 bytes, less `.toml`.
    pub const MAX_LEN: usize = 250;

    pub fn new(name: impl Into<String>) -> Result<Self, InvalidClientName> {
        let name = name.into();
        let mut bytes = name.bytes();
        let valid_first = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        let valid_rest =
            bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if valid_first && valid_rest && name.len() <= Self::MAX_LEN {
            Ok(Self(name))
        } else {
            Err(InvalidClientName(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ClientName {
    type Error = InvalidClientName;
    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

impl From<ClientName> for String {
    fn from(name: ClientName) -> Self {
        name.0
    }
}

impl fmt::Display for ClientName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A name [`ClientName::new`] refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid client name {0:?}: use lowercase letters, digits, and `-`, starting with a letter or digit, at most 250 bytes"
)]
pub struct InvalidClientName(String);

/// Identifies one state of a namespace's values. Opaque: compare it only
/// for equality, and send back the one [`CONFIG_GET`] or
/// [`CONFIG_CHANGED`] gave.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ConfigRevision(String);

impl ConfigRevision {
    pub fn new(revision: impl Into<String>) -> Self {
        Self(revision.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Names one client instance's settings writes, such as a terminal's, so
/// they never conflict with each other (see [`ConfigSetParams::writer`]).
/// Opaque: a client picks one unique to it, such as a random UUID, and
/// sends it with each write.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ConfigWriter(String);

impl ConfigWriter {
    pub fn new(writer: impl Into<String>) -> Self {
        Self(writer.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Params of [`CONFIG_SCHEMA`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigSchemaParams {}

/// Result of [`CONFIG_SCHEMA`]: every agent setting, in display order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigSchema {
    /// The pages settings sit on, in display order.
    pub categories: Vec<ConfigCategory>,
    pub settings: Vec<SettingSpec>,
    /// Structured tables `config.toml` holds that only a hand edit
    /// changes, such as provider profiles.
    pub file_only: Vec<FileOnlyTable>,
}

/// A settings page; [`SettingSpec::category`] names its `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigCategory {
    pub id: String,
    pub label: String,
}

/// A table of `config.toml` the protocol does not edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileOnlyTable {
    /// Its dotted key, such as `providers`.
    pub key: String,
    pub label: String,
    /// Where its format is documented.
    pub docs: String,
}

/// Params of [`CONFIG_REGISTER`]. Registering again replaces the client's
/// earlier schema; the daemon keeps it until it stops.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigRegisterParams {
    pub client: ClientName,
    /// The client's settings. Each takes a [`SettingKind::Bool`],
    /// [`SettingKind::Choice`], [`SettingKind::Integer`], or
    /// [`SettingKind::Text`] value.
    pub settings: Vec<SettingSpec>,
}

/// Result of [`CONFIG_REGISTER`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigRegisterResponse {}

/// Params of [`CONFIG_GET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigGetParams {
    pub namespace: ConfigNamespace,
}

/// Result of [`CONFIG_GET`]. From then on the connection gets
/// [`CONFIG_CHANGED`] for the namespace whenever a value or the error list
/// changes; reading the namespace again replaces that subscription.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigGetResponse {
    /// Send it with the next [`CONFIG_SET`].
    pub revision: ConfigRevision,
    /// Every setting of the namespace.
    pub values: BTreeMap<SettingKey, SettingEntry>,
    /// What keeps the file from loading. While the list is not empty, the
    /// values are the last ones that loaded, or the defaults.
    pub errors: Vec<ConfigError>,
}

/// A setting's value and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingEntry {
    /// Absent for an optional setting that is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<SettingValue>,
    pub source: ValueSource,
}

/// Where a setting's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ValueSource {
    /// Nothing sets it; the value is its default.
    Default,
    /// The user's global file sets it.
    Global,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One problem in a settings file, by setting when it has one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<SettingKey>,
    pub message: String,
}

/// Params of [`CONFIG_SET`]. Every change lands, or none does: a change the
/// daemon refuses leaves the file as it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigSetParams {
    pub namespace: ConfigNamespace,
    pub layer: ConfigLayer,
    /// The revision the changes were made against. A stale one is refused
    /// with [`crate::ErrorCode::ConfigConflict`].
    pub revision: ConfigRevision,
    /// Who makes the changes. A revision stays good for its writer while
    /// only the writer's own changes followed it, so a client that sends a
    /// write before [`CONFIG_CHANGED`] for its last one arrives is not
    /// refused for that change. A writer still sends each write only once
    /// the last one's response came back: writes in flight together may
    /// land in either order. Without a writer, only the current revision is
    /// good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<ConfigWriter>,
    /// Applied in order, so a setting another belongs to, such as an
    /// internal agent's model before its reasoning, comes first.
    pub changes: Vec<SettingChange>,
}

/// Which file a [`CONFIG_SET`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConfigLayer {
    /// The user's global file.
    Global,
    /// A value a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One change of a [`CONFIG_SET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SettingChange {
    /// Write `value`, which the setting's [`SettingKind`] takes.
    Set {
        key: SettingKey,
        value: SettingValue,
    },
    /// Remove the setting from the file, so its default applies.
    Reset { key: SettingKey },
}

impl SettingChange {
    /// The setting the change is to.
    pub fn key(&self) -> &SettingKey {
        match self {
            Self::Set { key, .. } | Self::Reset { key } => key,
        }
    }
}

/// Result of [`CONFIG_SET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigSetResponse {
    /// The namespace's revision with the changes.
    pub revision: ConfigRevision,
}

/// Params of [`CONFIG_CHANGED`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigChangedParams {
    pub namespace: ConfigNamespace,
    pub revision: ConfigRevision,
    /// Only the settings whose entries changed.
    pub values: BTreeMap<SettingKey, SettingEntry>,
    /// The whole error list, which replaces the last one.
    pub errors: Vec<ConfigError>,
}
