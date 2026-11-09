//! How the daemon describes one setting: its key, the values it takes,
//! its default, and when a change takes effect. Rho declares every setting
//! once and builds these from that declaration, so a frontend renders a
//! settings screen from data instead of knowing each setting.

use std::fmt;

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
