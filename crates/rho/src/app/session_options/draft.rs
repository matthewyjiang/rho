//! The options of a session a client has not opened yet, so it can show
//! and change them before anything exists in the daemon: the TUI opens its
//! session with the first prompt, carrying what was chosen here.
//!
//! [`DraftOptions`] builds them as [`super::of`] builds a session's, from
//! the client's own settled config, and applies the setting changes a
//! daemon session takes between turns (model, reasoning, fast mode,
//! permission mode) with the same checks the daemon makes before it builds
//! anything, refusing with the daemon's [`SettingChangeError`]. The daemon
//! stays the judge: once the session opens, the client sends it the settings
//! someone chose ([`Origin`]) as changes, and one it refuses is reported
//! then. The rest the daemon takes from `config.toml` as it is when the
//! session opens, so a config edit applies to the next new session; `/new`
//! reads it again too ([`config_for_new_session`]), so the draft shows it.

use std::path::PathBuf;

use rho_protocol as wire;
use rho_providers::{provider::model_reference, providers::fast_mode::supports_fast_mode};
use rho_sdk::ReasoningLevel;

use super::{build, models::AuthPick, reasoning_from_wire, resolve_model};
use crate::{
    agent::AgentDefinition,
    app::session_runtime::{resolve_switch_reasoning, CurrentModel, SettingChangeError},
    config::Config,
    permission::PermissionMode,
};

/// The config a session opened now as `agent` starts on: `config.toml` as
/// it is, under the agent's model policy, as the daemon reads it for the
/// session (`daemon_host::sessions::open`). A file that does not load is
/// an error, naming it.
pub(crate) fn config_for_new_session(agent: &AgentDefinition) -> anyhow::Result<Config> {
    let config = Config::load(None)?;
    // Custom providers this config adds name models the draft may show.
    config.providers.activate()?;
    let mut config = crate::app::relaunch_config(agent, &config);
    super::super::cli_config::normalize_reasoning(
        &mut config,
        rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
    )?;
    Ok(config)
}

/// Where a session's setting comes from, which decides whether the session
/// a client opens is sent it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Origin {
    /// `config.toml`, which the daemon reads as the session opens:
    /// nothing to send.
    #[default]
    Config,
    /// Carried from the session this one replaces, where it was chosen:
    /// applies where the daemon takes it.
    Carried,
    /// Chosen for this session, by a launch flag or a pick: must apply.
    Chosen,
}

/// The [`Origin`] of each setting a client may choose for a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SettingOrigins {
    model: Origin,
    reasoning: Origin,
    fast_mode: Origin,
    permission_mode: Origin,
}

impl SettingOrigins {
    /// The origin of the setting `change` moves; one no client chooses
    /// before a session opens must apply.
    pub(crate) fn of(mut self, change: &wire::SessionChange) -> Origin {
        self.setting(change)
            .map_or(Origin::Chosen, |origin| *origin)
    }

    /// Records that the setting `change` moves comes from `origin` now;
    /// a change that moves none leaves them.
    pub(crate) fn set(&mut self, change: &wire::SessionChange, origin: Origin) {
        if let Some(setting) = self.setting(change) {
            *setting = origin;
        }
    }

    fn setting(&mut self, change: &wire::SessionChange) -> Option<&mut Origin> {
        match change {
            wire::SessionChange::Model { .. } => Some(&mut self.model),
            wire::SessionChange::Reasoning { .. } => Some(&mut self.reasoning),
            wire::SessionChange::FastMode { .. } => Some(&mut self.fast_mode),
            wire::SessionChange::PermissionMode { .. } => Some(&mut self.permission_mode),
            wire::SessionChange::ModelAfterCompaction { .. }
            | wire::SessionChange::Context { .. }
            | wire::SessionChange::AgentCatalog
            | wire::SessionChange::AddDirectory { .. }
            | wire::SessionChange::ModelCatalog
            | wire::SessionChange::SelectNode { .. }
            | wire::SessionChange::RewindWorkspace { .. } => None,
            // `SessionChange` is non-exhaustive; nothing else is a setting.
            _ => None,
        }
    }
}

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
    origins: SettingOrigins,
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
            origins: SettingOrigins::default(),
            process_config,
            available_auths,
            commands,
            added_dirs,
        }
    }

    /// The settings of a session on `process_config` that replaces one
    /// running with `options`, as `/new` does: of those, it carries the
    /// ones `origins` says someone chose, and its commands, which the same
    /// setup lists again. Its tools reach `added_dirs`.
    pub(crate) fn carried(
        options: &wire::SessionOptions,
        origins: SettingOrigins,
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
        let carried = |origin: Origin| match origin {
            Origin::Config => Origin::Config,
            Origin::Carried | Origin::Chosen => Origin::Carried,
        };
        draft.origins = SettingOrigins {
            model: carried(origins.model),
            reasoning: carried(origins.reasoning),
            fast_mode: carried(origins.fast_mode),
            permission_mode: carried(origins.permission_mode),
        };
        if draft.origins.model == Origin::Carried {
            draft.current = CurrentModel {
                provider: options.model.current.provider.clone(),
                model: options.model.current.model.clone(),
                auth: options.model.auth.clone(),
            };
        }
        if draft.origins.reasoning == Origin::Carried {
            if let Some(level) = options
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning_from_wire(reasoning.current))
            {
                draft.reasoning = level;
            }
        }
        if draft.origins.fast_mode == Origin::Carried {
            draft.fast_mode = options.fast_mode.as_ref().is_some_and(|fast| fast.enabled);
        }
        if draft.origins.permission_mode == Origin::Carried {
            if let Ok(mode) = PermissionMode::try_from(options.permission_mode.current) {
                draft.permission_mode = mode;
            }
        }
        draft
    }

    /// Where the setting `change` moves the session to comes from: one
    /// from config is not sent, a carried one the daemon refuses keeps the
    /// session's own, and a chosen one it refuses fails the open.
    pub(crate) fn origin(&self, change: &wire::SessionChange) -> Origin {
        self.origins.of(change)
    }

    /// Where each setting comes from, which the session keeps once open.
    pub(crate) fn origins(&self) -> SettingOrigins {
        self.origins
    }

    /// Counts the setting `change` moves as chosen, as a launch flag does,
    /// whose value the draft's config already holds.
    pub(crate) fn chose(&mut self, change: &wire::SessionChange) {
        self.origins.set(change, Origin::Chosen);
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
        // The reasoning level the switch kept stays where it came from:
        // the daemon keeps its own the same way where it is from config.
        self.origins.model = Origin::Chosen;
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
        self.origins.reasoning = Origin::Chosen;
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
        self.origins.fast_mode = Origin::Chosen;
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
        self.origins.permission_mode = Origin::Chosen;
        Ok(())
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
