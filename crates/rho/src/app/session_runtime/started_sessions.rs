//! What a person chose for the sessions a session starts through session
//! control (the `sessions` tool's `new`): the permission mode and the model
//! they open with. A host sets it (`rho/session/configure`), never a tool
//! the model calls; the daemon applies it to each session it opens under
//! this one (`daemon_host/sessions/new.rs`), and that session keeps it as
//! its own choice, so the sessions it starts in turn open with it too. A
//! fork keeps the choice of the session it forks.
//!
//! The choice rides in snapshot metadata under a key of its own, which
//! every durable save rewrites, apart from the session's own settings
//! (`saved_settings.rs`): setting it never makes a session that follows the
//! config keep its own settings. Every host that opens the session restores
//! it, so none of their saves drops it.

use rho_sdk::SessionSnapshot;
use serde::{Deserialize, Serialize};

use super::{SessionRuntime, SettingChangeError};
use crate::{
    agent::ModelPolicy, config::Config, permission::PermissionMode,
    session::Session as StoredSession,
};

/// The snapshot metadata key of a session's [`StartedSessionDefaults`].
const STARTED_SESSION_DEFAULTS_METADATA: &str = "rho.session.started_session_defaults";

/// The permission mode and model the sessions a session starts open with,
/// as its file keeps them. A part left `None` leaves it to the config those
/// sessions open with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StartedSessionDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permission_mode: Option<StartedMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<StartedModel>,
}

/// A chosen permission mode, saved as [`PermissionMode::as_str`] names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub(crate) enum StartedMode {
    Known(PermissionMode),
    /// A name this rho does not know, as a newer one may have saved, or
    /// [`UNREADABLE`] for a record that did not read. Kept as it was read,
    /// so a save does not erase it, and no session opens in it: the daemon
    /// refuses to start one under it.
    Unknown(String),
}

/// The mode a session takes for a choice its file keeps but this rho could
/// not read.
const UNREADABLE: &str = "unreadable";

impl From<String> for StartedMode {
    fn from(name: String) -> Self {
        PermissionMode::ALL
            .into_iter()
            .find(|mode| mode.as_str() == name)
            .map_or(Self::Unknown(name), Self::Known)
    }
}

impl From<StartedMode> for String {
    fn from(mode: StartedMode) -> Self {
        match mode {
            StartedMode::Known(mode) => mode.as_str().to_owned(),
            StartedMode::Unknown(name) => name,
        }
    }
}

/// A model by its provider and the provider's model id. Each started
/// session runs it on an auth that has credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StartedModel {
    pub(crate) provider: String,
    pub(crate) model: String,
}

impl StartedSessionDefaults {
    /// What the session saved under `id` in workspace `cwd` keeps, read
    /// without opening it, which another process may hold. An error when
    /// nothing is saved there or the record does not read.
    pub(crate) fn saved_by_id(cwd: &std::path::Path, id: &str) -> anyhow::Result<Self> {
        StoredSession::session_metadata_by_id(cwd, id, STARTED_SESSION_DEFAULTS_METADATA)
            .and_then(decode)
    }

    /// What `stored`'s file keeps.
    pub(super) fn saved_in(stored: &StoredSession) -> anyhow::Result<Self> {
        stored
            .session_metadata(STARTED_SESSION_DEFAULTS_METADATA)
            .and_then(decode)
    }

    /// `snapshot` with this choice in its metadata; with none, unchanged,
    /// so a save after the choice is cleared records none.
    pub(super) fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        if *self == Self::default() {
            return snapshot;
        }
        snapshot.with_metadata(
            STARTED_SESSION_DEFAULTS_METADATA,
            serde_json::to_string(self).expect("the choice holds only plain values"),
        )
    }
}

/// The choice `encoded` records, or none when there is no record.
fn decode(encoded: Option<String>) -> anyhow::Result<StartedSessionDefaults> {
    encoded.map_or_else(
        || Ok(StartedSessionDefaults::default()),
        |encoded| Ok(serde_json::from_str(&encoded)?),
    )
}

impl SessionRuntime {
    /// What the sessions this session starts open with.
    pub(crate) fn started_session_defaults(&self) -> &StartedSessionDefaults {
        &self.started_sessions
    }

    /// Whether the agent the session runs as names its own model, which a
    /// chosen model for started sessions does not replace.
    pub(crate) fn agent_pins_model(&self) -> bool {
        !matches!(
            self.agent.definition().model_policy().as_ref(),
            ModelPolicy::Inherit
        )
    }

    /// Opens the sessions this session starts in `mode`, or as the config
    /// says when `None`, and saves the choice when it changed. A mode this
    /// session's config could not run, such as auto without a
    /// permission-classifier model, is [`SettingChangeError::Unsupported`],
    /// and the choice stays as it was: every session it starts would fail
    /// to open in it.
    pub(crate) fn set_started_session_permission_mode(
        &mut self,
        mode: Option<PermissionMode>,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        if let Some(mode) = mode {
            self.config_for_mode(mode, process_config)?;
        }
        self.change_started_session_defaults(StartedSessionDefaults {
            permission_mode: mode.map(StartedMode::Known),
            ..self.started_sessions.clone()
        })
    }

    /// Opens the sessions this session starts on `model`, or on the
    /// config's when `None`, and saves the choice when it changed.
    pub(crate) fn set_started_session_model(
        &mut self,
        model: Option<StartedModel>,
    ) -> Result<(), SettingChangeError> {
        self.change_started_session_defaults(StartedSessionDefaults {
            model,
            ..self.started_sessions.clone()
        })
    }

    /// Takes `defaults` and saves them, unless they are what the session
    /// has: a change that changes nothing writes nothing.
    fn change_started_session_defaults(
        &mut self,
        defaults: StartedSessionDefaults,
    ) -> Result<(), SettingChangeError> {
        if self.started_sessions == defaults {
            return Ok(());
        }
        self.started_sessions = defaults;
        self.save_state().map_err(SettingChangeError::Apply)
    }

    /// Takes `saved`, the choice a file keeps ([`StartedSessionDefaults::saved_in`]):
    /// the session's own or, for a fork, that of the session it forks. A
    /// mode this rho does not know is kept as it was read (see
    /// [`StartedMode::Unknown`]). A record that does not read is logged and
    /// taken as an unknown mode, so no session starts under this one on the
    /// config's until a person chooses again: the record may have chosen a
    /// stricter mode.
    pub(super) fn restore_started_session_defaults(
        &mut self,
        saved: anyhow::Result<StartedSessionDefaults>,
    ) {
        match saved {
            Ok(defaults) => {
                if let Some(StartedMode::Unknown(name)) = &defaults.permission_mode {
                    tracing::warn!(
                        permission_mode = %name,
                        "the permission mode chosen for the sessions this one starts is unknown to this rho; none will start until a person chooses another"
                    );
                }
                self.started_sessions = defaults;
            }
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not read what the sessions this one starts open with; none will start until a person chooses again"
                );
                self.started_sessions = StartedSessionDefaults {
                    permission_mode: Some(StartedMode::Unknown(UNREADABLE.to_owned())),
                    model: None,
                };
            }
        }
    }
}
