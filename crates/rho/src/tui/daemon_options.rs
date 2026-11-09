//! Daemon-owned settings rendered through the existing TUI controls.
//!
//! Only attach and OptionsChanged update displayed state. No provider is
//! replaced here and no local config default is written on behalf of a session.

use rho_protocol::{ModelRef, SessionChange, SessionOptions};
use rho_providers::model::favorites::CycleDirection;

use super::{
    config_picker, App, CommandId, CommandInvocation, ComposerMode, Entry, PickerBadge,
    PickerBadgeTone, PickerItem, UiPicker,
};
use crate::{app::reasoning_from_wire, permission::PermissionMode};

const FAST_MODE: &str = "daemon_fast_mode";

/// Resolve only advertised identities (or their unique labels). Local aliases,
/// credentials and catalog capabilities are not the daemon's choices.
fn model_choice(options: &rho_protocol::ModelOptions, reference: &str) -> anyhow::Result<ModelRef> {
    if let Some(choice) = options
        .available
        .iter()
        .find(|choice| reference == format!("{}/{}", choice.model.provider, choice.model.model))
    {
        return Ok(choice.model.clone());
    }
    let matches: Vec<_> = options
        .available
        .iter()
        .filter(|choice| reference == choice.model.model || reference == choice.label)
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
        SessionChange::Model { .. } => "model",
        SessionChange::Reasoning { .. } => "reasoning level",
        SessionChange::FastMode { .. } => "fast mode",
        SessionChange::PermissionMode { .. } => "permission mode",
        // `SessionChange` is non-exhaustive; the TUI sends only the above.
        _ => "session setting",
    }
}

fn next_choice<T: Copy + PartialEq>(current: T, available: &[T]) -> Option<T> {
    let index = available.iter().position(|value| *value == current)?;
    available.get((index + 1) % available.len()).copied()
}

fn model_picker(options: &SessionOptions) -> UiPicker {
    UiPicker::models(
        "Select model",
        options
            .model
            .available
            .iter()
            .map(|choice| PickerItem {
                badge: (choice.model == options.model.current).then_some(PickerBadge {
                    text: "selected".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                ..PickerItem::new(
                    choice.label.clone(),
                    format!("{}/{}", choice.model.provider, choice.model.model),
                )
            })
            .collect(),
    )
}

impl App {
    pub(super) fn show_daemon_options(&mut self, options: SessionOptions) {
        self.info
            .runtime
            .provider
            .clone_from(&options.model.current.provider);
        self.info
            .runtime
            .model
            .clone_from(&options.model.current.model);
        let reasoning = options
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning_from_wire(reasoning.current))
            .unwrap_or(rho_providers::reasoning::ReasoningLevel::Off);
        self.info.set_reasoning(
            reasoning,
            rho_providers::model::ReasoningRequestSource::Explicit,
        );
        if let Ok(mode) = PermissionMode::try_from(options.permission_mode.current) {
            self.info.runtime.permission_mode = mode;
        }
        self.info.runtime.service_tier = options
            .fast_mode
            .as_ref()
            .filter(|fast| fast.enabled)
            .map(|_| rho_sdk::model::ServiceTier::Priority);
        // Metadata is display-only in this path; never normalize daemon options
        // or push a context window into the unused InteractiveRuntime.
        self.model_metadata = super::reasoning_metadata::cached_metadata(
            &options.model.current.provider,
            &options.model.current.model,
        )
        .map(|(metadata, _)| metadata);
        self.daemon_options = Some(options);
        let refresh = matches!(self.input_ui.composer(), ComposerMode::Picker(picker)
            if picker.is_conversation_model() || matches!(picker.config_page(),
                Some(config_picker::ConfigPage::SessionOptions | config_picker::ConfigPage::SessionPermissionMode)));
        if refresh {
            let ComposerMode::Picker(mut stale) = self.input_ui.take_composer() else {
                unreachable!();
            };
            let picker = if stale.is_conversation_model() {
                let value = stale
                    .selected_item()
                    .map(|item| item.value.clone())
                    .unwrap_or_default();
                let mut picker = self.daemon_model_picker().expect("options just received");
                Self::restore_picker_position(
                    &mut picker,
                    &value,
                    std::mem::take(&mut stale.filter),
                );
                if let Some(parent) = stale.take_parent() {
                    picker = picker.with_parent(parent);
                }
                picker
            } else {
                self.refreshed_config_page(stale)
            };
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
    }

    pub(super) fn configure_daemon(&mut self, change: SessionChange) -> anyhow::Result<()> {
        let session = self
            .info
            .session
            .daemon
            .as_ref()
            .expect("daemon setting requires a session");
        match session.configure(change.clone()) {
            Ok(()) => {
                // Only cycling intent is projected through pending requests.
                // Visible state remains the daemon's acknowledged options.
                self.daemon_option_requests.push_back(change);
                if self.turn.is_provider_turn() {
                    self.set_status("the change applies after the current turn");
                }
            }
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not change the {}: {error}",
                setting_name(&change)
            ))),
        }
        Ok(())
    }

    pub(super) fn daemon_model_picker(&self) -> Option<UiPicker> {
        self.daemon_options.as_ref().map(model_picker)
    }

    pub(super) fn select_daemon_model(&mut self, reference: &str) -> anyhow::Result<()> {
        let Some(options) = &self.daemon_options else {
            return Ok(());
        };
        match model_choice(&options.model, reference) {
            Ok(model) => self.configure_daemon(SessionChange::Model { model }),
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
        let available: Vec<_> = options
            .model
            .available
            .iter()
            .filter(|choice| {
                favorites.iter().any(|favorite| {
                    favorite.provider == choice.model.provider
                        && favorite.model == choice.model.model
                })
            })
            .collect();
        if available.is_empty() {
            self.notify_status("no pinned models offered by the daemon session");
            return Ok(());
        }
        let current = self
            .daemon_option_requests
            .iter()
            .rev()
            .find_map(|change| match change {
                SessionChange::Model { model } => Some(model),
                _ => None,
            })
            .unwrap_or(&options.model.current);
        let index = available.iter().position(|choice| choice.model == *current);
        let next = match (direction, index) {
            (CycleDirection::Forward, Some(index)) => (index + 1) % available.len(),
            (CycleDirection::Backward, Some(index)) => {
                (index + available.len() - 1) % available.len()
            }
            (CycleDirection::Forward, None) => 0,
            (CycleDirection::Backward, None) => available.len() - 1,
        };
        let model = available[next].model.clone();
        self.configure_daemon(SessionChange::Model { model })
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
            _ => unreachable!("only daemon option commands reach option dispatch"),
        }
    }

    fn set_daemon_permission(&mut self, mode: PermissionMode) -> anyhow::Result<()> {
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

    fn set_daemon_fast(&mut self, args: &str) -> anyhow::Result<()> {
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
        if enabled && current.is_none() {
            self.insert_entry(&Entry::Error(
                "fast mode is not offered by the daemon session".into(),
            ));
            return Ok(());
        }
        self.configure_daemon(SessionChange::FastMode { enabled })
    }

    pub(super) fn open_daemon_permission_picker(&mut self) {
        if let Some(picker) = self.daemon_permission_picker() {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
    }

    pub(super) fn daemon_permission_picker(&self) -> Option<UiPicker> {
        let options = self.daemon_options.as_ref()?;
        let mut picker = config_picker::permission_mode_picker(self.info.runtime.permission_mode);
        picker.items.retain(|item| {
            item.value
                .strip_prefix(config_picker::PERMISSION_MODE_PREFIX)
                .and_then(|mode| mode.parse::<PermissionMode>().ok())
                .is_some_and(|mode| options.permission_mode.available.contains(&mode.into()))
        });
        Some(picker.with_config_page(config_picker::ConfigPage::SessionPermissionMode))
    }

    pub(super) fn open_daemon_config(&mut self) {
        if let Some(picker) = self.daemon_config_picker() {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
    }

    pub(super) fn daemon_config_picker(&self) -> Option<UiPicker> {
        let options = self.daemon_options.as_ref()?;
        let mut items = vec![PickerItem::new(
            "Model".into(),
            config_picker::CONVERSATION_MODEL_VALUE.into(),
        )];
        if options.reasoning.is_some() {
            items.push(PickerItem::new(
                "Reasoning".into(),
                config_picker::REASONING_VALUE.into(),
            ));
        }
        if options.fast_mode.is_some() {
            items.push(PickerItem::new("Fast mode".into(), FAST_MODE.into()));
        }
        items.push(PickerItem::new(
            "Permission mode".into(),
            config_picker::PERMISSION_MODE_VALUE.into(),
        ));
        items.push(PickerItem::new(
            "Theme".into(),
            config_picker::THEME_VALUE.into(),
        ));
        Some(
            UiPicker::config("Session options", items)
                .with_config_page(config_picker::ConfigPage::SessionOptions),
        )
    }

    pub(super) fn commit_daemon_config(&mut self, value: &str) -> anyhow::Result<()> {
        match value {
            config_picker::CONVERSATION_MODEL_VALUE => {
                if let Some(picker) = self.daemon_model_picker() {
                    self.input_ui.set_composer(ComposerMode::Picker(picker));
                }
            }
            config_picker::REASONING_VALUE => return self.cycle_daemon_reasoning(),
            FAST_MODE => return self.set_daemon_fast(""),
            config_picker::PERMISSION_MODE_VALUE => self.open_daemon_permission_picker(),
            config_picker::THEME_VALUE => return self.open_theme_picker(),
            _ => {
                if let Some(mode) = value.strip_prefix(config_picker::PERMISSION_MODE_PREFIX) {
                    self.set_daemon_permission(mode.parse()?)?;
                    self.input_ui.set_composer(ComposerMode::Input);
                } else {
                    self.daemon_command_unavailable("config");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "daemon_options_tests.rs"]
mod tests;
