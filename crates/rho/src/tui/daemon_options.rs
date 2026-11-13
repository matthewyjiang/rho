//! Daemon-owned settings rendered through the existing TUI controls.
//!
//! Only attach and OptionsChanged update displayed state. No provider is
//! replaced here and no local config default is written on behalf of a session.

use rho_protocol::{ModelRef, SessionChange, SessionOptions};
use rho_providers::model::favorites::{CycleDirection, FavoriteModel};

use super::{
    config_row::{permission_mode_picker, ConfigRow},
    settings_screen::ConfigPage,
    App, CommandId, CommandInvocation, ComposerMode, Entry, UiPicker,
};
use crate::{
    app::{reasoning_from_wire, OfferedModels},
    permission::PermissionMode,
};

/// Resolve only advertised identities (or their unique labels). Local aliases,
/// credentials and catalog capabilities are not the daemon's choices; a
/// current alias's row also answers to its listed id (see [`OfferedModels`]).
fn model_choice(options: &rho_protocol::ModelOptions, reference: &str) -> anyhow::Result<ModelRef> {
    let offered = OfferedModels::new(options);
    if let Some(choice) = offered.by_reference(reference) {
        return Ok(choice.model.clone());
    }
    let matches: Vec<_> = options
        .available
        .iter()
        .filter(|choice| offered.answers(choice, reference) || reference == choice.label)
        .collect();
    match matches.as_slice() {
        [choice] => Ok(choice.model.clone()),
        [] => anyhow::bail!("model '{reference}' is not offered by the daemon session"),
        _ => anyhow::bail!("model '{reference}' is ambiguous; use provider/model"),
    }
}

/// The setting `change` changes, as an error names it.
pub(super) fn setting_name(change: &SessionChange) -> &'static str {
    match change {
        SessionChange::Model { .. } | SessionChange::ModelAfterCompaction { .. } => "model",
        SessionChange::Reasoning { .. } => "reasoning level",
        SessionChange::FastMode { .. } => "fast mode",
        SessionChange::PermissionMode { .. } => "permission mode",
        SessionChange::Context { .. } => "session context",
        SessionChange::AgentCatalog => "agent catalog",
        SessionChange::AddDirectory { .. } => "added directories",
        SessionChange::ModelCatalog => "model lists",
        SessionChange::SelectNode { .. } => "conversation state",
        SessionChange::RewindWorkspace { .. } => "workspace",
        // `SessionChange` is non-exhaustive; the TUI sends only the above.
        _ => "session setting",
    }
}

/// How an error says `change` was not made, naming the model a refused
/// switch asked for.
pub(super) fn change_failure(change: &SessionChange) -> String {
    match change {
        SessionChange::Model { model, .. } | SessionChange::ModelAfterCompaction { model, .. } => {
            format!(
                "could not switch to {}",
                rho_providers::provider::model_reference(&model.provider, &model.model)
            )
        }
        _ => format!("could not change the {}", setting_name(change)),
    }
}

fn next_choice<T: Copy + PartialEq>(current: T, available: &[T]) -> Option<T> {
    let index = available.iter().position(|value| *value == current)?;
    available.get((index + 1) % available.len()).copied()
}

/// The offered model pin cycling moves to from `current`, in pin order, or
/// `None` when no pin is offered. A pin of a listed id names the row a
/// current alias holds in its place.
fn next_pinned_model<'a>(
    offered: &'a [rho_protocol::ModelChoice],
    favorites: &[FavoriteModel],
    current: &ModelRef,
    direction: CycleDirection,
) -> Option<&'a ModelRef> {
    let pinned: Vec<_> = favorites
        .iter()
        .filter_map(|favorite| {
            offered
                .iter()
                .find(|choice| favorite.matches(&choice.model.provider, &choice.model.model))
        })
        .map(|choice| &choice.model)
        .collect();
    let last = pinned.len().checked_sub(1)?;
    let index = pinned.iter().position(|model| *model == current);
    let next = match (direction, index) {
        (CycleDirection::Forward, Some(index)) => (index + 1) % pinned.len(),
        (CycleDirection::Backward, Some(index)) => (index + last) % pinned.len(),
        (CycleDirection::Forward, None) => 0,
        (CycleDirection::Backward, None) => last,
    };
    Some(pinned[next])
}

/// What a change asked for mid-turn says when it has no wording of its own.
const QUEUED_CHANGE: &str = "the change applies after the current turn";

impl App {
    pub(super) fn show_daemon_options(&mut self, options: SessionOptions) {
        // The session runs on another provider or auth than the one last
        // read as signed out: the launch config's at first, since a resumed
        // session keeps its own, and after a switch the one it left. What
        // it runs on now is not known to lack credentials, and the daemon
        // says so at the prompt if it does.
        let runtime = &self.info.runtime;
        let model_changed = runtime.provider != options.model.current.provider
            || runtime.model != options.model.current.model;
        if runtime.provider != options.model.current.provider || runtime.auth != options.model.auth
        {
            self.info.services.auth_unavailable = None;
        }
        self.info
            .runtime
            .provider
            .clone_from(&options.model.current.provider);
        self.info
            .runtime
            .model
            .clone_from(&options.model.current.model);
        self.info.runtime.auth.clone_from(&options.model.auth);
        let reasoning = options
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning_from_wire(reasoning.current))
            .unwrap_or(rho_providers::reasoning::ReasoningLevel::Off);
        // A level that moved with the model is the daemon's rounding, not
        // one the user asked for; otherwise it is as explicit as it was.
        let source = if model_changed && reasoning != self.info.runtime.reasoning {
            rho_providers::model::ReasoningRequestSource::PersistedOrDefault
        } else {
            self.info.runtime.reasoning_source
        };
        self.info.set_reasoning(reasoning, source);
        // A switch this TUI asked for says where it landed, with the
        // reasoning level the daemon gave the new model, whether its
        // request is still pending or already finished.
        let unshown = self.daemon_unshown_switch.take();
        let switched = self.daemon_options.as_ref().is_some_and(|shown| {
            shown.model.current != options.model.current
                && (unshown.as_ref() == Some(&options.model.current)
                    || self.daemon_option_requests.iter().any(|change| {
                        matches!(change, SessionChange::Model { model, .. } if *model == options.model.current)
                    }))
        });
        // Login's own status line, which also names the stored credentials,
        // stays.
        let said_by_login =
            switched && self.daemon_login_selected.take().as_ref() == Some(&options.model.current);
        if switched && !said_by_login {
            let current = &options.model.current;
            self.set_status(format!(
                "model switched to {} with reasoning {reasoning}",
                super::model_names::ModelNames::new(&options.model.available)
                    .reference(&current.provider, &current.model)
            ));
        }
        if let Ok(mode) = PermissionMode::try_from(options.permission_mode.current) {
            self.info.runtime.permission_mode = mode;
        }
        self.info.runtime.service_tier = options
            .fast_mode
            .as_ref()
            .filter(|fast| fast.enabled)
            .map(|_| rho_sdk::model::ServiceTier::Priority);
        self.info.runtime.added_dirs =
            crate::added_dirs::AddedDirs::reported(options.added_dirs.clone());
        // Metadata is display-only; the daemon's options are never normalized
        // here.
        self.model_metadata = super::reasoning_metadata::cached_metadata(
            &options.model.current.provider,
            &options.model.current.model,
        )
        .map(|(metadata, _)| metadata);
        self.daemon_options = Some(options);
        self.refresh_open_config_page();
        // First-run setup's model step opened on the model the session had:
        // one it moved to since, as after signing in to another provider,
        // is the one Enter should confirm.
        if model_changed && self.setup_step() == Some(super::setup_screen::SetupStep::ChooseModel) {
            self.show_setup_model_picker();
        }
    }

    /// The oldest change this TUI asked the daemon for finished, applied or
    /// refused. Login's note of the model it asked for goes with its
    /// request: one the daemon refused must not quiet a later `/model` to
    /// the same model.
    ///
    /// A switch's options usually show before its request finishes, but
    /// not always: a reattach delivers them in its response, after the
    /// request finished. A switch to a model not shown yet waits for the
    /// next options report, which says where it landed; a refused one
    /// leaves that report on another model and says nothing. Login's
    /// switch has nothing to wait for: its status line names the model.
    pub(super) fn finish_daemon_option_request(&mut self) {
        let finished = self.daemon_option_requests.pop_front();
        if let Some(SessionChange::Model { model, .. }) = finished {
            let by_login = self
                .daemon_login_selected
                .take_if(|selected| *selected == model)
                .is_some();
            let shown = self
                .daemon_options
                .as_ref()
                .is_some_and(|options| options.model.current == model);
            if !by_login && !shown {
                self.daemon_unshown_switch = Some(model);
            }
        }
    }

    pub(super) fn configure_daemon(&mut self, change: SessionChange) -> anyhow::Result<()> {
        match self.info.session.daemon.configure(change.clone()) {
            Ok(()) => {
                if self.turn.is_provider_turn() {
                    self.set_status(match &change {
                        SessionChange::PermissionMode { mode } => {
                            match PermissionMode::try_from(*mode) {
                                Ok(mode) => format!(
                                    "permission mode {} queued for next turn",
                                    mode.as_str()
                                ),
                                Err(_) => QUEUED_CHANGE.to_owned(),
                            }
                        }
                        SessionChange::Model { model, .. } => format!(
                            "model change to {} queued for after this run",
                            rho_providers::provider::model_reference(&model.provider, &model.model)
                        ),
                        _ => QUEUED_CHANGE.to_owned(),
                    });
                }
                // Only cycling intent is projected through pending requests.
                // Visible state remains the daemon's acknowledged options.
                self.daemon_option_requests.push_back(change);
            }
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "{}: {error}",
                change_failure(&change)
            ))),
        }
        Ok(())
    }

    /// Whether a daemon session's first launch has a model to choose: its
    /// own, signed in, or another auth's. Its picker always lists the
    /// current model, so an empty picker cannot say there is none.
    pub(super) fn daemon_setup_can_choose_model(&self) -> bool {
        self.setup_state().signed_in
            || self
                .daemon_options
                .as_ref()
                .is_some_and(|options| options.model.available.len() > 1)
    }

    /// The model picker over the models the daemon offers: pins, scope and
    /// badges are the TUI's own, as the pin list is.
    pub(super) fn daemon_model_picker(&self) -> Option<UiPicker> {
        let options = self.daemon_options.as_ref()?;
        let offered = super::model_picker::offered_models(&options.model);
        let scope = self.conversation_model_picker_scope();
        let picker = if self.is_provider_turn_ui() {
            let pending =
                self.daemon_option_requests
                    .iter()
                    .rev()
                    .find_map(|change| match change {
                        SessionChange::Model { model, .. } => Some(model),
                        _ => None,
                    });
            super::model_picker::offered_model_picker_during_run(
                &self.info.runtime,
                pending,
                offered,
                scope,
            )
        } else {
            super::model_picker::offered_model_picker(&self.info.runtime, offered, scope)
        };
        Some(picker.with_config_page(ConfigPage::SessionModel))
    }

    pub(super) fn select_daemon_model(&mut self, reference: &str) -> anyhow::Result<()> {
        let Some(options) = &self.daemon_options else {
            return Ok(());
        };
        match model_choice(&options.model, reference) {
            Ok(model) => {
                // The daemon rounds a level the model lacks, as a pin cycle
                // wants. A model picked by name keeps a level the user
                // asked for, or is refused.
                let capabilities = rho_providers::model::models_dev::current_reasoning_capabilities(
                    &model.provider,
                    &model.model,
                );
                // A level asked for and not yet confirmed is the one the
                // daemon applies before this switch.
                let reasoning = self
                    .daemon_option_requests
                    .iter()
                    .rev()
                    .find_map(|change| match change {
                        SessionChange::Reasoning { level } => reasoning_from_wire(*level),
                        _ => None,
                    })
                    .unwrap_or(self.info.runtime.reasoning);
                if let Err(requested) = super::reasoning_metadata::resolve_model_switch_reasoning(
                    &capabilities,
                    reasoning,
                    self.info.runtime.reasoning_source,
                ) {
                    self.insert_entry(&Entry::Error(format!(
                        "could not switch to {}: reasoning level '{requested}' is not supported",
                        rho_providers::provider::model_reference(&model.provider, &model.model)
                    )));
                    self.set_status("model switch rejected");
                    return Ok(());
                }
                let asked = self.daemon_option_requests.len();
                self.configure_daemon(SessionChange::Model {
                    model,
                    auth: None,
                    setup: false,
                })?;
                // First-run setup ends with its model, once the daemon was
                // asked for it: a change that could not be sent leaves the
                // step open under its error. One the daemon then refuses
                // shows as an error row in the session.
                if self.daemon_option_requests.len() > asked {
                    self.finish_setup_screen();
                }
                Ok(())
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not switch model: {error}")));
                Ok(())
            }
        }
    }

    pub(super) fn cycle_daemon_model(&mut self, direction: CycleDirection) -> anyhow::Result<()> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return Ok(());
        }
        let Some(options) = &self.daemon_options else {
            return Ok(());
        };
        let favorites = rho_providers::model::favorites::normalized_favorite_models(
            &self.info.runtime.favorite_models,
        );
        let current = self
            .daemon_option_requests
            .iter()
            .rev()
            .find_map(|change| match change {
                SessionChange::Model { model, .. } => Some(model),
                _ => None,
            })
            .unwrap_or(&options.model.current);
        let Some(model) =
            next_pinned_model(&options.model.available, &favorites, current, direction)
        else {
            self.notify_status("no pinned models offered by the daemon session");
            return Ok(());
        };
        let model = model.clone();
        self.configure_daemon(SessionChange::Model {
            model,
            auth: None,
            setup: false,
        })
    }

    pub(super) fn cycle_daemon_reasoning(&mut self) -> anyhow::Result<()> {
        let Some(reasoning) = self
            .daemon_options
            .as_ref()
            .and_then(|options| options.reasoning.as_ref())
        else {
            return Ok(());
        };
        let available: Vec<_> = reasoning
            .available
            .iter()
            .copied()
            .filter(|level| reasoning_from_wire(*level).is_some())
            .collect();
        let current = self
            .daemon_option_requests
            .iter()
            .rev()
            .find_map(|change| match change {
                SessionChange::Reasoning { level } => Some(*level),
                _ => None,
            })
            .unwrap_or(reasoning.current);
        if let Some(level) = next_choice(current, &available) {
            self.configure_daemon(SessionChange::Reasoning { level })?;
            // Asked for by name: a later `/model` keeps it or is refused.
            self.info.runtime.reasoning_source =
                rho_providers::model::ReasoningRequestSource::Explicit;
        }
        Ok(())
    }

    pub(super) fn cycle_daemon_permission(&mut self) -> anyhow::Result<()> {
        let Some(options) = &self.daemon_options else {
            return Ok(());
        };
        let mut available: Vec<_> = options
            .permission_mode
            .available
            .iter()
            .copied()
            .filter_map(|mode| PermissionMode::try_from(mode).ok())
            .collect();
        available.sort_by_key(|mode| mode.restrictiveness_rank());
        let current = self
            .daemon_option_requests
            .iter()
            .rev()
            .find_map(|change| match change {
                SessionChange::PermissionMode { mode } => Some(*mode),
                _ => None,
            })
            .unwrap_or(options.permission_mode.current);
        if let Some(mode) = PermissionMode::try_from(current)
            .ok()
            .and_then(|current| next_choice(current, &available))
        {
            self.configure_daemon(SessionChange::PermissionMode { mode: mode.into() })?;
        }
        Ok(())
    }

    pub(super) fn execute_daemon_option_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Model => {
                if invocation.args.trim().is_empty() {
                    if let Some(picker) = self.daemon_model_picker() {
                        self.input_ui.set_composer(ComposerMode::Picker(picker));
                    }
                    Ok(())
                } else {
                    self.select_daemon_model(invocation.args.trim())
                }
            }
            CommandId::Permissions => {
                let args = invocation.args.trim();
                if args.is_empty() {
                    self.open_daemon_permission_picker();
                    return Ok(());
                }
                match args.parse::<PermissionMode>() {
                    Ok(mode) => self.set_daemon_permission(mode),
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not change permission mode: {error}"
                        )));
                        Ok(())
                    }
                }
            }
            CommandId::Fast => self.set_daemon_fast(invocation.args.trim()),
            CommandId::Advisor => {
                self.execute_daemon_advisor_command(&invocation.args);
                Ok(())
            }
            CommandId::Codemode => {
                self.execute_daemon_codemode_command(&invocation.args);
                Ok(())
            }
            _ => unreachable!("only daemon option commands reach option dispatch"),
        }
    }

    pub(super) fn set_daemon_permission(&mut self, mode: PermissionMode) -> anyhow::Result<()> {
        if self
            .daemon_options
            .as_ref()
            .is_some_and(|options| options.permission_mode.available.contains(&mode.into()))
        {
            self.configure_daemon(SessionChange::PermissionMode { mode: mode.into() })
        } else {
            self.insert_entry(&Entry::Error(format!(
                "permission mode '{}' is not offered by the daemon session",
                mode.as_str()
            )));
            Ok(())
        }
    }

    pub(super) fn set_daemon_fast(&mut self, args: &str) -> anyhow::Result<()> {
        let current = self
            .daemon_options
            .as_ref()
            .and_then(|options| options.fast_mode.as_ref());
        let requested = self
            .daemon_option_requests
            .iter()
            .rev()
            .find_map(|change| match change {
                SessionChange::FastMode { enabled } => Some(*enabled),
                _ => None,
            })
            .unwrap_or_else(|| current.is_some_and(|fast| fast.enabled));
        let enabled = match args.to_ascii_lowercase().as_str() {
            "" => !requested,
            "on" => true,
            "off" => false,
            _ => {
                self.insert_entry(&Entry::Error("usage: /fast [on|off]".into()));
                return Ok(());
            }
        };
        let supported = current.is_some();
        if enabled && !supported {
            self.refuse_fast_mode();
            return Ok(());
        }
        // The preference outlives a switch to a model with no fast mode,
        // so turning it off is always sent.
        let asked = self.daemon_option_requests.len();
        self.configure_daemon(SessionChange::FastMode { enabled })?;
        // A change that was not sent has said why, and one sent mid-turn
        // has said it is queued.
        if self.daemon_option_requests.len() > asked && !self.turn.is_provider_turn() {
            self.report_fast_mode(enabled, supported);
        }
        Ok(())
    }

    pub(super) fn open_daemon_permission_picker(&mut self) {
        if let Some(picker) = self.daemon_permission_picker() {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
    }

    pub(super) fn daemon_permission_picker(&self) -> Option<UiPicker> {
        let options = self.daemon_options.as_ref()?;
        let mut picker = permission_mode_picker(self.info.runtime.permission_mode);
        picker
            .items
            .retain(|item| match ConfigRow::parse(&item.value) {
                Some(ConfigRow::SetPermissionMode { mode }) => {
                    options.permission_mode.available.contains(&mode.into())
                }
                _ => false,
            });
        picker.select_badged();
        Some(picker.with_config_page(ConfigPage::SessionPermissionMode))
    }
}

#[cfg(test)]
#[path = "daemon_options_tests.rs"]
mod tests;
