//! What a person chose for the sessions a session starts through session
//! control (the `sessions` tool's `new`): the permission mode and the model
//! they open with. A host sets it (`rho/session/configure`), never a tool
//! the model calls; the daemon applies it to each session it opens naming
//! this one as its parent (`daemon_host/sessions/new.rs`). The session keeps
//! it with its other settings (`saved_settings.rs`), so it outlasts a
//! reopen. A session started so begins with no choice of its own.

use super::{SessionRuntime, SettingChangeError};
use crate::{config::Config, permission::PermissionMode};

/// The permission mode and model the sessions a session starts open with.
/// A part left `None` leaves it to the config those sessions open with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartedSessionDefaults {
    pub(crate) permission_mode: Option<PermissionMode>,
    pub(crate) model: Option<StartedModel>,
}

/// A model by its provider and the provider's model id. Each started
/// session runs it on an auth that has credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartedModel {
    pub(crate) provider: String,
    pub(crate) model: String,
}

impl SessionRuntime {
    /// What the sessions this session starts open with.
    pub(crate) fn started_session_defaults(&self) -> &StartedSessionDefaults {
        &self.started_sessions
    }

    /// Opens the sessions this session starts in `mode`, or as the config
    /// says when `None`. A mode this session's config could not run, such
    /// as auto without a permission-classifier model, is
    /// [`SettingChangeError::Unsupported`], and the choice stays as it was:
    /// every session it starts would fail to open in it.
    pub(crate) fn set_started_session_permission_mode(
        &mut self,
        mode: Option<PermissionMode>,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        if let Some(mode) = mode {
            self.config_for_mode(mode, process_config)?;
        }
        self.started_sessions.permission_mode = mode;
        Ok(())
    }

    /// Opens the sessions this session starts on `model`, or on the
    /// config's when `None`.
    pub(crate) fn set_started_session_model(&mut self, model: Option<StartedModel>) {
        self.started_sessions.model = model;
    }
}
