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
//! someone chose ([`SettingOrigin`]) as changes, and one it refuses is reported
//! then. The rest the daemon takes from `config.toml` as it is when the
//! session opens, so a config edit applies to the next new session; `/new`
//! reads it again too ([`read_config_file`]), so the draft shows it. A
//! choice holds in the next new session until `config.toml` changes that
//! setting ([`SettingOrigins::holding_on`]).

use std::{path::PathBuf, sync::Arc};

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

/// `config.toml` as it is now, without an agent's model policy or a
/// launch's flags: what a choice of setting is made against. Its warnings
/// go to the log, since the TUI owns the terminal. A file that does not
/// load is an error, naming it.
pub(crate) fn read_config_file() -> anyhow::Result<Config> {
    let path = Config::default_path()?;
    let (config, warnings) = Config::load_with_warnings(&path)?;
    for warning in warnings {
        tracing::warn!(path = %path.display(), "{}", warning.message());
    }
    Ok(config)
}

/// The config a draft opened as `agent` on `file`, a [`read_config_file`],
/// shows its options from: under the agent's model policy, as startup
/// settles it. The daemon settles its own as it opens the session
/// (`daemon_host::sessions::open`), which the settings from config then
/// follow, so this only has to match what the person sees until then.
/// Activates the file's custom providers process-wide, as startup does, so
/// the draft can name their models.
pub(crate) fn config_for_new_session(
    file: &Config,
    agent: &AgentDefinition,
) -> anyhow::Result<Config> {
    file.providers.activate()?;
    let mut config = crate::app::relaunch_config(agent, file);
    crate::app::cli_config::normalize_reasoning(
        &mut config,
        rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
    )?;
    Ok(config)
}

/// Where a session's setting comes from, which decides whether the session
/// a client opens is sent it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingOrigin {
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

/// A launch flag that chooses a session's setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchFlag {
    /// `--provider` or `--model`, with `--auth` or not.
    Model,
    /// `--auth` alone, which keeps the configured model where the auth is
    /// its provider's: it holds until `config.toml` changes the model.
    Auth,
    Reasoning,
    PermissionMode,
}

/// A setting a client may choose for a session before or after it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    /// The model and its auth.
    Model,
    Reasoning,
    FastMode,
    PermissionMode,
}

impl Setting {
    /// Every setting, the model first, since it decides what the others
    /// may pick.
    const ALL: [Self; 4] = [
        Self::Model,
        Self::Reasoning,
        Self::FastMode,
        Self::PermissionMode,
    ];

    /// The setting `change` moves, if it moves one.
    fn of(change: &wire::SessionChange) -> Option<Self> {
        match change {
            wire::SessionChange::Model { .. } => Some(Self::Model),
            wire::SessionChange::Reasoning { .. } => Some(Self::Reasoning),
            wire::SessionChange::FastMode { .. } => Some(Self::FastMode),
            wire::SessionChange::PermissionMode { .. } => Some(Self::PermissionMode),
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

    /// Whether `before` and `now` configure this setting alike.
    fn configured_alike(self, before: &Config, now: &Config) -> bool {
        match self {
            Self::Model => {
                (&before.provider, &before.model, &before.auth)
                    == (&now.provider, &now.model, &now.auth)
            }
            Self::Reasoning => before.reasoning == now.reasoning,
            Self::FastMode => before.fast_mode == now.fast_mode,
            Self::PermissionMode => before.permission_mode == now.permission_mode,
        }
    }
}

/// Where one setting comes from, and what a choice of it was made against.
#[derive(Clone, Debug, Default)]
struct Origin {
    from: SettingOrigin,
    /// `config.toml` as it was when the setting was chosen: the choice
    /// holds in later sessions while the file configures the setting as it
    /// did then. `None` for a launch flag, which holds for the launch.
    made_on: Option<Arc<Config>>,
}

/// The origin of each [`Setting`] of a session.
#[derive(Clone, Debug, Default)]
pub(crate) struct SettingOrigins {
    model: Origin,
    reasoning: Origin,
    fast_mode: Origin,
    permission_mode: Origin,
}

impl SettingOrigins {
    /// The settings `flagged`, a launch's flags, chose, which hold for
    /// the launch. A flag choosing the model carries the reasoning level
    /// the launch settled for it, made on `launch_file`, the `config.toml`
    /// the launch read: it goes with the model where the daemon takes it,
    /// until the file's reasoning changes. A bare `--auth` holds the same
    /// way until the file's model changes.
    pub(crate) fn of_launch_flags(
        flagged: impl IntoIterator<Item = LaunchFlag>,
        launch_file: Arc<Config>,
    ) -> Self {
        let held = || Origin {
            from: SettingOrigin::Chosen,
            made_on: None,
        };
        let made_on_launch = |from| Origin {
            from,
            made_on: Some(Arc::clone(&launch_file)),
        };
        let mut flags = Self::default();
        let mut model_flag = false;
        for flag in flagged {
            match flag {
                LaunchFlag::Model => {
                    flags.model = held();
                    model_flag = true;
                }
                LaunchFlag::Auth => flags.model = made_on_launch(SettingOrigin::Chosen),
                LaunchFlag::Reasoning => flags.reasoning = held(),
                LaunchFlag::PermissionMode => flags.permission_mode = held(),
            }
        }
        if model_flag && flags.reasoning.from == SettingOrigin::Config {
            flags.reasoning = made_on_launch(SettingOrigin::Carried);
        }
        flags
    }

    /// Whether `change` moves a setting, which a choice of it records.
    pub(crate) fn moves_a_setting(change: &wire::SessionChange) -> bool {
        Setting::of(change).is_some()
    }

    /// The origin of the setting `change` moves; a change that moves none
    /// is no setting a draft defers to config, so it must apply.
    pub(crate) fn of(&self, change: &wire::SessionChange) -> SettingOrigin {
        Setting::of(change).map_or(SettingOrigin::Chosen, |setting| self.origin(setting).from)
    }

    /// Records that someone chose the setting `change` moves, against
    /// `made_on`, `config.toml` as it is now; a change that moves none
    /// leaves them.
    pub(crate) fn choose(&mut self, change: &wire::SessionChange, made_on: Arc<Config>) {
        if let Some(setting) = Setting::of(change) {
            *self.origin_mut(setting) = Origin {
                from: SettingOrigin::Chosen,
                made_on: Some(made_on),
            };
        }
    }

    /// These origins, with the settings chosen in `picks` taken from it.
    pub(crate) fn with_picks(mut self, picks: &SettingOrigins) -> Self {
        for setting in Setting::ALL {
            let pick = picks.origin(setting);
            if pick.from == SettingOrigin::Chosen {
                *self.origin_mut(setting) = pick.clone();
            }
        }
        self
    }

    /// These origins, but for the choices `file`, `config.toml` as it is
    /// now, configures otherwise than when they were made, which come from
    /// config again: a file edit since a choice wins over it.
    pub(crate) fn holding_on(mut self, file: &Config) -> Self {
        for setting in Setting::ALL {
            let origin = self.origin_mut(setting);
            if origin
                .made_on
                .as_ref()
                .is_some_and(|made_on| !setting.configured_alike(made_on, file))
            {
                *origin = Origin::default();
            }
        }
        self
    }

    fn origin(&self, setting: Setting) -> &Origin {
        match setting {
            Setting::Model => &self.model,
            Setting::Reasoning => &self.reasoning,
            Setting::FastMode => &self.fast_mode,
            Setting::PermissionMode => &self.permission_mode,
        }
    }

    fn origin_mut(&mut self, setting: Setting) -> &mut Origin {
        match setting {
            Setting::Model => &mut self.model,
            Setting::Reasoning => &mut self.reasoning,
            Setting::FastMode => &mut self.fast_mode,
            Setting::PermissionMode => &mut self.permission_mode,
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

    /// Takes the settings `settings` says someone chose from a session
    /// that runs with `options`, as `/new` carries them to the session it
    /// opens: each as a pick, re-checked against the draft's model,
    /// counted as carried. One the draft refuses stays its own, as the
    /// daemon keeps its own when it refuses a carried setting.
    pub(crate) fn carry(&mut self, options: &wire::SessionOptions, settings: &SettingOrigins) {
        let switched_model = self.switched_model;
        for setting in Setting::ALL {
            let origin = settings.origin(setting);
            if origin.from != SettingOrigin::Config && self.take(setting, options) {
                *self.origins.origin_mut(setting) = Origin {
                    from: SettingOrigin::Carried,
                    made_on: origin.made_on.clone(),
                };
            }
        }
        self.switched_model = switched_model;
    }

    /// Sets `setting` as `options` have it; whether the draft took it.
    fn take(&mut self, setting: Setting, options: &wire::SessionOptions) -> bool {
        match setting {
            Setting::Model => self
                .set_model(&options.model.current, Some(&options.model.auth))
                .is_ok(),
            Setting::Reasoning => options
                .reasoning
                .as_ref()
                .is_some_and(|reasoning| self.set_reasoning(reasoning.current).is_ok()),
            Setting::FastMode => self
                .set_fast_mode(options.fast_mode.as_ref().is_some_and(|fast| fast.enabled))
                .is_ok(),
            Setting::PermissionMode => self
                .set_permission_mode(options.permission_mode.current)
                .is_ok(),
        }
    }

    /// Where each setting comes from, which the session keeps once open.
    pub(crate) fn origins(&self) -> &SettingOrigins {
        &self.origins
    }

    /// Counts `flags`, the settings a launch's flags chose, as the
    /// draft's, whose config already holds their values.
    pub(crate) fn set_launch_flags(&mut self, flags: SettingOrigins) {
        self.origins = flags;
    }

    /// Records that someone chose the setting `change` moved, which the
    /// draft took, against `made_on`, `config.toml` as it is now.
    pub(crate) fn chose(&mut self, change: &wire::SessionChange, made_on: Arc<Config>) {
        self.origins.choose(change, made_on);
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
        Ok(())
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
