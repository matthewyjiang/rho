//! The options of a session a client has not opened yet, so it can show
//! and change them before anything exists in the daemon: the TUI opens its
//! session with the first prompt, carrying what was chosen here.
//!
//! [`DraftOptions`] builds them as [`super::of`] builds a session's, from
//! the client's own settled config, and applies the setting changes a
//! daemon session takes between turns (model, reasoning, fast mode,
//! permission mode) with the same checks the daemon makes before it builds
//! anything, refusing with the daemon's [`SettingChangeError`]. The daemon
//! stays the judge: the client sends the result as
//! changes once the session opens, and one it refuses is reported then.

use std::path::PathBuf;

use rho_protocol as wire;
use rho_providers::{provider::model_reference, providers::fast_mode::supports_fast_mode};
use rho_sdk::ReasoningLevel;

use super::{build, models::AuthPick, reasoning_from_wire, resolve_model};
use crate::{
    app::session_runtime::{resolve_switch_reasoning, CurrentModel, SettingChangeError},
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
    /// The settings taken from the session this one replaces and not
    /// chosen since, which the session takes where the daemon lets it.
    inherited: Inherited,
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
            inherited: Inherited::default(),
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
        draft.inherited = Inherited {
            model: true,
            reasoning: true,
            fast_mode: true,
            permission_mode: true,
        };
        draft
    }

    /// Whether `change` moves the session to a setting carried from the
    /// one it replaces rather than one the person or the launch chose: one
    /// the daemon refuses then keeps the session's own, where a chosen one
    /// fails the open.
    pub(crate) fn inherits(&self, change: &wire::SessionChange) -> bool {
        match change {
            wire::SessionChange::Model { .. } => self.inherited.model,
            wire::SessionChange::Reasoning { .. } => self.inherited.reasoning,
            wire::SessionChange::FastMode { .. } => self.inherited.fast_mode,
            wire::SessionChange::PermissionMode { .. } => self.inherited.permission_mode,
            wire::SessionChange::ModelAfterCompaction { .. }
            | wire::SessionChange::Context { .. }
            | wire::SessionChange::AgentCatalog
            | wire::SessionChange::AddDirectory { .. }
            | wire::SessionChange::ModelCatalog
            | wire::SessionChange::SelectNode { .. }
            | wire::SessionChange::RewindWorkspace { .. } => false,
            // `SessionChange` is non-exhaustive; a draft makes only the above.
            _ => false,
        }
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
    /// options follow a credential write; whether that changed them.
    pub(crate) fn set_available_auths(&mut self, available_auths: &[String]) -> bool {
        let changed = self.available_auths != available_auths;
        if changed {
            self.available_auths = available_auths.to_vec();
        }
        changed
    }

    /// Switches to `model`, on `auth` when it names one, else on an auth
    /// with credentials, keeping the reasoning level where the model takes
    /// it, as a daemon session's switch does.
    pub(crate) fn set_model(
        &mut self,
        model: &wire::ModelRef,
        auth: Option<&str>,
    ) -> Result<(), SettingChangeError> {
        let pick = auth.map_or(AuthPick::AnyOf(&self.available_auths), AuthPick::Named);
        let selection = resolve_model(&model.provider, &model.model, &self.current, pick)
            .map_err(|refused| SettingChangeError::Unsupported(refused.to_string()))?;
        self.reasoning = resolve_switch_reasoning(&selection, self.reasoning)?;
        self.current = CurrentModel {
            provider: selection.provider,
            model: selection.model,
            auth: selection.auth,
        };
        self.switched_model = true;
        // A switch keeps the reasoning level where the model takes it,
        // which then is as chosen as the model.
        self.inherited.model = false;
        self.inherited.reasoning = false;
        Ok(())
    }

    /// Spends `level`, one the current model offers, on the session's turns.
    pub(crate) fn set_reasoning(
        &mut self,
        level: wire::ReasoningLevel,
    ) -> Result<(), SettingChangeError> {
        let offered = self
            .options()
            .reasoning
            .is_some_and(|reasoning| reasoning.available.contains(&level));
        let Some(level) = reasoning_from_wire(level) else {
            return Err(SettingChangeError::Unsupported(
                "unknown reasoning level".into(),
            ));
        };
        if !offered {
            return Err(SettingChangeError::Unsupported(format!(
                "reasoning level '{level}' is not supported for '{}'",
                model_reference(&self.current.provider, &self.current.model)
            )));
        }
        self.reasoning = level;
        self.inherited.reasoning = false;
        Ok(())
    }

    /// Turns fast mode on, for a model that has it, or off.
    pub(crate) fn set_fast_mode(&mut self, enabled: bool) -> Result<(), SettingChangeError> {
        let current = &self.current;
        if enabled && !supports_fast_mode(&current.provider, &current.model, &current.auth) {
            return Err(SettingChangeError::Unsupported(format!(
                "'{}' has no fast mode",
                model_reference(&current.provider, &current.model)
            )));
        }
        self.fast_mode = enabled;
        self.inherited.fast_mode = false;
        Ok(())
    }

    /// The directories the session's tools reach besides its workspace,
    /// which it opens with.
    pub(crate) fn added_dirs(&self) -> &[PathBuf] {
        &self.added_dirs
    }

    /// Lets the session's tools reach `dir`, an absolute directory, which
    /// it opens with; one it already reaches is refused, as the daemon
    /// refuses it.
    pub(crate) fn add_directory(&mut self, dir: PathBuf) -> Result<(), SettingChangeError> {
        if self.added_dirs.contains(&dir) {
            return Err(SettingChangeError::Unsupported(format!(
                "the session already reaches {}",
                crate::paths::display(&dir)
            )));
        }
        self.added_dirs.push(dir);
        Ok(())
    }

    /// Gates the session's tool calls with `mode`, one it offers.
    pub(crate) fn set_permission_mode(
        &mut self,
        mode: wire::PermissionMode,
    ) -> Result<(), SettingChangeError> {
        let offered = self.options().permission_mode.available.contains(&mode);
        self.permission_mode = PermissionMode::try_from(mode)
            .ok()
            .filter(|_| offered)
            .ok_or_else(|| {
                SettingChangeError::Unsupported("that permission mode is not offered".into())
            })?;
        self.inherited.permission_mode = false;
        Ok(())
    }
}

/// Which of a draft's settings it carries from the session it replaces.
#[derive(Clone, Copy, Debug, Default)]
struct Inherited {
    model: bool,
    reasoning: bool,
    fast_mode: bool,
    permission_mode: bool,
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
