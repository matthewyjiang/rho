//! The settings a host chose for a session (model, reasoning level, fast
//! mode, permission mode), kept in its file so every host that reopens it
//! restores them: the daemon reopens a session it unloaded while idle, and
//! without them the session would fall back to the config's.
//!
//! Like a delegated agent's model override (`agent.rs`), the record rides
//! in snapshot metadata and every durable save rewrites it. It is a separate
//! key because the override also decides how a delegated child binds, which
//! a root's settings must not.
//!
//! A session keeps its settings once a host records them
//! ([`SessionRuntime::keep_settings`]), and from then on, in whichever host
//! opens it, every save records the settings as they are then, a plan
//! approval's mode switch included. A session no host recorded settings for
//! follows the config each time it opens.

use rho_providers::model::catalog::ModelSelection;
use rho_sdk::{model::ServiceTier, ReasoningLevel, SessionSnapshot};
use serde::{Deserialize, Serialize};

use super::{CurrentModel, SessionRuntime, SettingChangeError};
use crate::{config::Config, permission::PermissionMode, session::Session as StoredSession};

/// The snapshot metadata key of a session's [`SavedSettings`].
const SETTINGS_METADATA: &str = "rho.session.settings";

/// Whether a session's saves record its settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsPolicy {
    /// Nothing recorded: the session takes the config's settings when it
    /// opens.
    FollowConfig,
    /// Every save records the settings as they are then.
    Keep,
}

impl SettingsPolicy {
    /// The policy of a session whose file is `stored`: kept when the file
    /// already records settings, so a host that did not record them still
    /// keeps the record current.
    pub(super) fn of(stored: Option<&StoredSession>) -> Self {
        match stored.map(|stored| stored.session_metadata(SETTINGS_METADATA)) {
            Some(Ok(Some(_))) => Self::Keep,
            Some(Ok(None)) | None => Self::FollowConfig,
            Some(Err(error)) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not read the session's saved settings; following the config"
                );
                Self::FollowConfig
            }
        }
    }
}

/// What one save records of a session's settings.
#[derive(Clone, Debug)]
pub(super) enum SettingsRecord {
    /// Nothing; see [`SettingsPolicy::FollowConfig`].
    FollowConfig,
    Keep(SavedSettings),
}

impl SettingsRecord {
    /// `snapshot` with these settings in its metadata.
    pub(super) fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        match self {
            Self::FollowConfig => snapshot,
            Self::Keep(settings) => snapshot.with_metadata(
                SETTINGS_METADATA,
                serde_json::to_string(settings).expect("saved settings contain only plain values"),
            ),
        }
    }
}

/// A session's settings as its file keeps them. The auth is a profile id,
/// never a credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedSettings {
    provider: String,
    model: String,
    auth: String,
    reasoning: ReasoningLevel,
    fast_mode: bool,
    /// [`PermissionMode::as_str`].
    permission_mode: String,
}

impl SavedSettings {
    fn of(runtime: &SessionRuntime) -> Self {
        let CurrentModel {
            provider,
            model,
            auth,
        } = runtime.current_model();
        Self {
            provider,
            model,
            auth,
            reasoning: runtime.reasoning_level(),
            fast_mode: runtime.fast_mode(),
            permission_mode: runtime.permission_mode().as_str().to_owned(),
        }
    }
}

impl SessionRuntime {
    /// What the session's next save records of its settings.
    pub(super) fn settings_record(&self) -> SettingsRecord {
        match self.settings {
            SettingsPolicy::FollowConfig => SettingsRecord::FollowConfig,
            SettingsPolicy::Keep => SettingsRecord::Keep(SavedSettings::of(self)),
        }
    }

    /// Keeps the session's settings with its file from now on, so a reopen
    /// can restore them ([`Self::restore_settings`]), and saves them now if
    /// the session has a file; a new session's first save writes them.
    pub(crate) fn keep_settings(&mut self) -> anyhow::Result<()> {
        self.settings = SettingsPolicy::Keep;
        if self.is_replaced() {
            return Ok(());
        }
        let Some(stored) = self.storage.file() else {
            return Ok(());
        };
        // The settings changed without a commit; a new revision lets the
        // file take a snapshot that differs from its leaf only by them.
        self.built.session.advance_revision()?;
        stored.save_snapshot(&self.durable_snapshot(), &[])
    }

    /// Applies the settings the session's file keeps, if any host recorded
    /// them, through the same cores a host's changes use. A setting the
    /// session can no longer take, such as a model whose credentials are
    /// gone, is logged and left as the config has it; the session's next
    /// save then records what it runs with.
    pub(crate) async fn restore_settings(&mut self, process_config: &Config) {
        let saved = match self.saved_settings() {
            Ok(Some(saved)) => saved,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not read the session's saved settings; keeping the config's"
                );
                return;
            }
        };
        let SavedSettings {
            provider,
            model,
            auth,
            reasoning,
            fast_mode,
            permission_mode,
        } = saved;
        let selection = ModelSelection {
            provider,
            model,
            auth,
            from_catalog: false,
        };
        // The model first: a switch keeps the level it can, which the saved
        // level then replaces.
        let model = self.switch_model(&selection, process_config).await;
        warn_unrestored("model", model);
        let reasoning = self.set_reasoning_level(reasoning, process_config);
        warn_unrestored("reasoning level", reasoning);
        // The preference itself, which a model switch keeps even on a model
        // without fast mode, so a switch back to one shows it again.
        let tier = self
            .built
            .session
            .set_service_tier(fast_mode.then_some(ServiceTier::Priority));
        warn_unrestored("fast mode", tier.map_err(Into::into));
        let mode = match PermissionMode::ALL
            .into_iter()
            .find(|mode| mode.as_str() == permission_mode)
        {
            Some(mode) => self.set_permission_mode(mode, process_config).await,
            None => Err(SettingChangeError::Unsupported(format!(
                "unknown permission mode '{permission_mode}'"
            ))),
        };
        warn_unrestored("permission mode", mode);
    }

    /// The settings the session's file keeps, or `None` when it keeps none.
    fn saved_settings(&self) -> anyhow::Result<Option<SavedSettings>> {
        let Some(stored) = self.storage.file() else {
            return Ok(None);
        };
        let Some(encoded) = stored.session_metadata(SETTINGS_METADATA)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&encoded)?))
    }
}

fn warn_unrestored(setting: &str, restored: Result<(), SettingChangeError>) {
    if let Err(error) = restored {
        tracing::warn!(
            setting,
            %error,
            "could not restore a saved session setting; keeping the config's"
        );
    }
}

#[cfg(test)]
#[path = "saved_settings_tests.rs"]
mod tests;
