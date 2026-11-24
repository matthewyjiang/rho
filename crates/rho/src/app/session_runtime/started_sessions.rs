//! What a person chose for the sessions a session starts through session
//! control (the `sessions` tool's `new`): the permission mode and the model
//! they open with. A host sets it (`rho/session/configure`), never a tool
//! the model calls; the daemon applies it to each session it opens under
//! this one (`daemon_host/sessions/new.rs`), and that session keeps it as
//! its own choice, so the sessions it starts in turn open with it too. A
//! fork keeps the choice of the session it forks. A session takes a copy
//! of the choice as it opens: changing the starter's choice later does not
//! change what the sessions it already started hand to the sessions they
//! start.
//!
//! The choice rides in snapshot metadata under a key of its own, which
//! every durable save rewrites, apart from the session's own settings
//! (`saved_settings.rs`): setting it never makes a session that follows the
//! config keep its own settings. Every host that opens the session restores
//! it, so none of their saves drops it. A record this rho cannot read is
//! kept as it was saved ([`StartedSessions::Unreadable`]).

use rho_sdk::SessionSnapshot;
use serde::{Deserialize, Serialize};

use super::{SessionRuntime, SettingChangeError};
use crate::{
    agent::ModelPolicy, config::Config, permission::PermissionMode,
    session::Session as StoredSession,
};

/// The snapshot metadata key of a session's [`StartedSessionDefaults`].
pub(super) const STARTED_SESSION_DEFAULTS_METADATA: &str = "rho.session.started_session_defaults";

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
    /// A name this rho does not know, as a newer one may have saved. Kept
    /// as it was read, so a save does not erase it, and no session opens in
    /// it: the daemon refuses to start one under it.
    Unknown(String),
}

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

/// What the sessions a session starts open with, as the session holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartedSessions {
    /// A choice this rho reads; [`StartedSessionDefaults::default`] when
    /// nothing is chosen.
    Chosen(StartedSessionDefaults),
    /// A record the session's file keeps that this rho could not read, as
    /// it was saved, or `None` when the file itself did not read. Every
    /// save writes it back as it was, so a rho that reads it still finds
    /// it, and the daemon starts no session under it: it may choose a
    /// stricter mode than the config's. Setting the mode for the sessions
    /// this one starts replaces it, and clearing that mode removes it.
    Unreadable(Option<String>),
}

impl Default for StartedSessions {
    fn default() -> Self {
        Self::Chosen(StartedSessionDefaults::default())
    }
}

impl StartedSessions {
    /// What the session saved under `id` in workspace `cwd` keeps, read
    /// without opening it, which another process may hold. An error when
    /// nothing is saved there or its file does not read.
    pub(crate) fn saved_by_id(cwd: &std::path::Path, id: &str) -> anyhow::Result<Self> {
        StoredSession::session_metadata_by_id(cwd, id, STARTED_SESSION_DEFAULTS_METADATA)
            .map(Self::read)
    }

    /// What `stored`'s file keeps; [`Self::Unreadable`] when the file does
    /// not read.
    pub(super) fn saved_in(stored: &StoredSession) -> Self {
        match stored.session_metadata(STARTED_SESSION_DEFAULTS_METADATA) {
            Ok(record) => Self::read(record),
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not read what the sessions this one starts open with; none will start until a person sets their permission mode again"
                );
                Self::Unreadable(None)
            }
        }
    }

    /// The choice `record` holds: none without a record, and
    /// [`Self::Unreadable`], keeping it, for one this rho cannot read.
    fn read(record: Option<String>) -> Self {
        let Some(record) = record else {
            return Self::default();
        };
        match serde_json::from_str(&record) {
            Ok(defaults) => Self::Chosen(defaults),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "could not read the saved choice of what the sessions this one starts open with; it is kept as saved, and none will start until a person sets their permission mode again"
                );
                Self::Unreadable(Some(record))
            }
        }
    }

    /// `snapshot` with this choice in its metadata. Nothing chosen writes
    /// no record where the snapshot has none, and an empty one where it
    /// has one, as a reopened session's snapshot does, so a cleared choice
    /// stays cleared. An unreadable record is written back as it was read.
    pub(super) fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        let record = match self {
            Self::Chosen(defaults) => {
                if *defaults == StartedSessionDefaults::default()
                    && !snapshot
                        .metadata()
                        .contains_key(STARTED_SESSION_DEFAULTS_METADATA)
                {
                    return snapshot;
                }
                serde_json::to_string(defaults).expect("the choice holds only plain values")
            }
            Self::Unreadable(Some(record)) => record.clone(),
            Self::Unreadable(None) => return snapshot,
        };
        snapshot.with_metadata(STARTED_SESSION_DEFAULTS_METADATA, record)
    }
}

/// Why a started-session model change is refused while the saved choice
/// does not read.
const UNREADABLE_MODEL_CHANGE: &str = "could not read the choice saved for the sessions this one starts, as when a newer rho saved it; set their permission mode first, which replaces it";

impl SessionRuntime {
    /// What the sessions this session starts open with.
    pub(crate) fn started_session_defaults(&self) -> &StartedSessions {
        &self.started_sessions
    }

    /// Whether the agent the session runs as names its own model, which a
    /// chosen model for started sessions does not replace: any agent that
    /// names one, preferred ([`ModelPolicy::Prefer`]) or required, keeps it.
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
    /// to open in it. A saved choice that did not read is replaced whole,
    /// its model included.
    pub(crate) fn set_started_session_permission_mode(
        &mut self,
        mode: Option<PermissionMode>,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        if let Some(mode) = mode {
            self.config_for_mode(mode, process_config)?;
        }
        let model = match &self.started_sessions {
            StartedSessions::Chosen(defaults) => defaults.model.clone(),
            StartedSessions::Unreadable(_) => None,
        };
        self.change_started_sessions(StartedSessions::Chosen(StartedSessionDefaults {
            permission_mode: mode.map(StartedMode::Known),
            model,
        }))
    }

    /// Opens the sessions this session starts on `model`, or on the
    /// config's when `None`, and saves the choice when it changed. While a
    /// saved choice does not read, [`SettingChangeError::Unsupported`]:
    /// keeping its mode is not possible, and dropping it could loosen it.
    pub(crate) fn set_started_session_model(
        &mut self,
        model: Option<StartedModel>,
    ) -> Result<(), SettingChangeError> {
        let StartedSessions::Chosen(defaults) = &self.started_sessions else {
            return Err(SettingChangeError::Unsupported(
                UNREADABLE_MODEL_CHANGE.to_owned(),
            ));
        };
        let defaults = StartedSessionDefaults {
            model,
            ..defaults.clone()
        };
        self.change_started_sessions(StartedSessions::Chosen(defaults))
    }

    /// Takes `started` and saves it, unless it is what the session has: a
    /// change that changes nothing writes nothing. A failed save keeps the
    /// choice as it was, so no client is told of one that was not saved.
    fn change_started_sessions(
        &mut self,
        started: StartedSessions,
    ) -> Result<(), SettingChangeError> {
        if self.started_sessions == started {
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.started_sessions, started);
        if let Err(error) = self.save_state() {
            self.started_sessions = previous;
            return Err(SettingChangeError::Apply(error));
        }
        Ok(())
    }

    /// Takes `saved`, the choice a file keeps ([`StartedSessions::saved_in`]):
    /// the session's own or, for a fork, that of the session it forks. A
    /// mode this rho does not know is kept as it was read (see
    /// [`StartedMode::Unknown`]), and so is a record that does not read
    /// (see [`StartedSessions::Unreadable`]).
    pub(super) fn restore_started_session_defaults(&mut self, saved: StartedSessions) {
        if let StartedSessions::Chosen(StartedSessionDefaults {
            permission_mode: Some(StartedMode::Unknown(name)),
            ..
        }) = &saved
        {
            tracing::warn!(
                permission_mode = %name,
                "the permission mode chosen for the sessions this one starts is unknown to this rho; none will start until a person chooses another"
            );
        }
        self.started_sessions = saved;
    }
}
