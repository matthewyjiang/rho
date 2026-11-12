//! The options of a session a client has not opened yet, so it can show
//! and change them before anything exists in the daemon: the TUI opens its
//! session with the first prompt, carrying what was chosen here.
//!
//! [`DraftOptions`] builds them as [`super::of`] builds a session's, from
//! the client's own settled config, and applies the setting changes a
//! daemon session takes between turns (model, reasoning, fast mode,
//! permission mode) with the same checks the daemon makes before it builds
//! anything. The daemon stays the judge: the client sends the result as
//! changes once the session opens, and one it refuses is reported then.

use std::path::PathBuf;

use rho_protocol as wire;
use rho_providers::{
    model::{models_dev, ReasoningRequestSource},
    providers::fast_mode::supports_fast_mode,
};
use rho_sdk::ReasoningLevel;

use super::{build, models::AuthPick, reasoning_from_wire, resolve_model};
use crate::{
    app::{conversation_switch::resolve_model_switch_reasoning, session_runtime::CurrentModel},
    config::Config,
    permission::PermissionMode,
};

/// A session's settings before it opens, and the options they make.
#[derive(Clone, Debug)]
pub(crate) struct DraftOptions {
    current: CurrentModel,
    reasoning: ReasoningLevel,
    /// The fast-mode preference, kept across models as a session keeps it.
    fast_mode: bool,
    permission_mode: PermissionMode,
    /// Whether the person switched the model here, rather than took the
    /// one the session opens or was carried with.
    switched_model: bool,
    /// The settled config of the client, read as a session reads its
    /// process config: favorites, per-model reasoning, the classifier.
    process_config: Config,
    available_auths: Vec<String>,
    commands: Vec<wire::SessionCommand>,
    added_dirs: Vec<PathBuf>,
}

impl DraftOptions {
    /// The options a session opened on `process_config` starts with, with
    /// the models of `available_auths`, listing `commands`, its tools
    /// reaching `added_dirs`.
    pub(crate) fn new(
        process_config: Config,
        available_auths: Vec<String>,
        commands: Vec<wire::SessionCommand>,
        added_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            current: CurrentModel {
                provider: process_config.provider.clone(),
                model: process_config.model.clone(),
                auth: process_config.auth.clone(),
            },
            reasoning: process_config.reasoning,
            fast_mode: process_config.fast_mode,
            permission_mode: process_config.permission_mode,
            switched_model: false,
            process_config,
            available_auths,
            commands,
            added_dirs,
        }
    }

    /// The settings of a session that runs with `options`, for one that
    /// replaces it, as `/new` does: its commands too, which the same setup
    /// lists again. Its tools reach `added_dirs`, the launch's.
    pub(crate) fn carried(
        options: &wire::SessionOptions,
        process_config: Config,
        available_auths: Vec<String>,
        added_dirs: Vec<PathBuf>,
    ) -> Self {
        let mut draft = Self::new(
            process_config,
            available_auths,
            options.commands.clone(),
            added_dirs,
        );
        draft.current = CurrentModel {
            provider: options.model.current.provider.clone(),
            model: options.model.current.model.clone(),
            auth: options.model.auth.clone(),
        };
        if let Some(level) = options
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning_from_wire(reasoning.current))
        {
            draft.reasoning = level;
        }
        draft.fast_mode = options.fast_mode.as_ref().is_some_and(|fast| fast.enabled);
        if let Ok(mode) = PermissionMode::try_from(options.permission_mode.current) {
            draft.permission_mode = mode;
        }
        draft
    }

    /// The options these settings make now.
    pub(crate) fn options(&self) -> wire::SessionOptions {
        let mut options = build(
            &self.current,
            &self.process_config,
            &self.available_auths,
            self.reasoning,
            wire::FastModeOptions {
                enabled: self.fast_mode,
            },
            self.permission_mode,
            self.commands.clone(),
        );
        options.added_dirs.clone_from(&self.added_dirs);
        options
    }

    /// Whether the person switched the model before the session opened,
    /// so the switch tells them which prompt file it runs on, as one
    /// later in the session does.
    pub(crate) fn switched_model(&self) -> bool {
        self.switched_model
    }

    /// Lists the models of `available_auths` from now on, as a session's
    /// options follow a credential write.
    pub(crate) fn set_available_auths(&mut self, available_auths: Vec<String>) {
        self.available_auths = available_auths;
    }

    /// Applies `change`, a setting, as a daemon session would; the error
    /// says why it refuses one, which leaves the settings as they were.
    /// Any other change is refused: it needs an open session.
    pub(crate) fn apply(&mut self, change: &wire::SessionChange) -> Result<(), String> {
        match change {
            wire::SessionChange::Model { model, auth, .. } => {
                let pick = auth
                    .as_deref()
                    .map_or(AuthPick::AnyOf(&self.available_auths), AuthPick::Named);
                let selection = resolve_model(&model.provider, &model.model, &self.current, pick)
                    .map_err(|refused| refused.to_string())?;
                let capabilities = models_dev::current_reasoning_capabilities(
                    &selection.provider,
                    &selection.model,
                );
                let reasoning = resolve_model_switch_reasoning(
                    &capabilities,
                    self.reasoning,
                    ReasoningRequestSource::PersistedOrDefault,
                )
                .map_err(|level| {
                    format!(
                        "reasoning level '{level}' is not supported for '{}/{}'",
                        selection.provider, selection.model
                    )
                })?
                .effective;
                self.current = CurrentModel {
                    provider: selection.provider,
                    model: selection.model,
                    auth: selection.auth,
                };
                self.reasoning = reasoning;
                self.switched_model = true;
                Ok(())
            }
            wire::SessionChange::Reasoning { level } => {
                let offered = self
                    .options()
                    .reasoning
                    .is_some_and(|reasoning| reasoning.available.contains(level));
                match reasoning_from_wire(*level) {
                    Some(level) if offered => {
                        self.reasoning = level;
                        Ok(())
                    }
                    Some(level) => Err(format!(
                        "reasoning level '{level}' is not supported for '{}/{}'",
                        self.current.provider, self.current.model
                    )),
                    None => Err("unknown reasoning level".into()),
                }
            }
            wire::SessionChange::FastMode { enabled } => {
                if *enabled
                    && !supports_fast_mode(
                        &self.current.provider,
                        &self.current.model,
                        &self.current.auth,
                    )
                {
                    return Err("the current model has no fast mode".into());
                }
                self.fast_mode = *enabled;
                Ok(())
            }
            wire::SessionChange::PermissionMode { mode } => {
                let offered = self.options().permission_mode.available.contains(mode);
                match PermissionMode::try_from(*mode).ok().filter(|_| offered) {
                    Some(mode) => {
                        self.permission_mode = mode;
                        Ok(())
                    }
                    None => Err("that permission mode is not offered".into()),
                }
            }
            _ => Err("the session has not started yet; send a prompt first".into()),
        }
    }
}

/// The auths this process's credentials can use, whose models a draft
/// lists, read off the async workers: a locked OS keyring blocks its
/// reader.
pub(crate) async fn available_auths() -> Vec<String> {
    tokio::task::spawn_blocking(|| {
        rho_providers::credentials::available_auth_modes(
            &crate::credential_store::AppCredentialStore,
        )
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
