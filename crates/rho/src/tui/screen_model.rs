//! The permission screen's model: the picker that sets
//! `[internal_agents.permission-classifier-screen]`, its allow threshold, and
//! the config row badges.
//!
//! The screen is answered by the classifier's own model (no entry), by a
//! decision model discovered on a decision-model host, or by another chat
//! model asked as text. A picked row records its kind, so a model on Ollama,
//! which serves both, is asked the way it was listed.

use std::collections::BTreeMap;

use rho_providers::{
    model::decision_models::{cached_decision_models, lists_decision_models},
    provider,
};

use super::{
    agent_picker::{InternalAgentModelPickerOrigin, InternalAgentModelTarget},
    model_picker::{self, InternalAgentModelRow, ScreenPickerInputs, ScreenSelection},
    App, ComposerMode, Entry, PickerBadge, PickerBadgeTone, RuntimeModelView, UiPicker,
};
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    config::{InternalAgentModelConfig, ModelKind},
    decision,
    permission_classifier::DECISION_SCREEN_ID,
};

impl App {
    /// Opens the screen-model picker, as a child of the config picker or
    /// alone in the composer by `origin`.
    pub(super) fn open_screen_model_picker(&mut self, origin: InternalAgentModelPickerOrigin) {
        let picker = self.screen_model_picker(origin);
        if origin.opens_standalone() {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        } else {
            self.open_child_picker(picker);
        }
        self.set_status(match origin {
            InternalAgentModelPickerOrigin::PermissionScreenSetupConfigRow
            | InternalAgentModelPickerOrigin::PermissionScreenSetupCommand => {
                "auto is on. Choose a screen model, or Esc to keep the current one"
            }
            _ => "select a permission screen model",
        });
    }

    /// Builds the screen-model picker and records what a selection means.
    pub(super) fn screen_model_picker(
        &mut self,
        origin: InternalAgentModelPickerOrigin,
    ) -> UiPicker {
        self.refresh_available_auths();
        let picker = model_picker::screen_model_picker(ScreenPickerInputs {
            current: screen_selection(&self.info.runtime),
            classifier: self
                .info
                .runtime
                .internal_agents
                .get(PERMISSION_CLASSIFIER_AGENT_ID)
                .map(InternalAgentModelConfig::display_reference),
            decision_models: discovered_decision_models(&self.available_auths),
            favorite_models: &self.info.runtime.favorite_models,
            available_auths: &self.available_auths,
            scope: self.resolved_model_picker_scope(),
            keybindings: &self.info.runtime.keybindings,
        });
        self.internal_agent_model_target = Some(InternalAgentModelTarget {
            id: DECISION_SCREEN_ID.into(),
            origin,
        });
        picker
    }

    /// Saves the picked screen row. Returns whether the entry changed.
    pub(super) fn commit_screen_model(
        &mut self,
        row: InternalAgentModelRow,
    ) -> anyhow::Result<bool> {
        let selection = match row {
            InternalAgentModelRow::Conversation => None,
            InternalAgentModelRow::Decision { provider, model } => {
                let auth = self.screen_auth(&provider);
                let mut selection = InternalAgentModelConfig::new(provider, model, auth);
                set_kind(&mut selection, ModelKind::Decision);
                Some(selection)
            }
            InternalAgentModelRow::RhoModel(reference) => {
                let current = self.internal_agent_rho_model_or_conversation(DECISION_SCREEN_ID);
                let picked = match self.resolve_model_selection(
                    &reference,
                    &current.provider,
                    &current.auth,
                ) {
                    Ok(picked) => picked.selection,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.set_status("permission screen model unchanged");
                        return Ok(false);
                    }
                };
                // The classifier's own model is the default, which also
                // follows it when the classifier changes.
                let classifier = self
                    .info
                    .runtime
                    .internal_agents
                    .get(PERMISSION_CLASSIFIER_AGENT_ID)
                    .and_then(InternalAgentModelConfig::rho);
                if classifier.is_some_and(|classifier| {
                    classifier.provider == picked.provider && classifier.model == picked.model
                }) {
                    None
                } else {
                    let mut selection =
                        InternalAgentModelConfig::new(picked.provider, picked.model, picked.auth);
                    set_kind(&mut selection, ModelKind::Text);
                    Some(selection)
                }
            }
            InternalAgentModelRow::ClaudeCode { .. } => {
                self.set_status("the permission screen runs on Rho models only");
                return Ok(false);
            }
        };
        self.store_screen_model(selection);
        Ok(true)
    }

    /// Dismisses an open screen picker, leaving the screen entry as it was.
    /// Returns whether one was open.
    pub(super) fn cancel_screen_model_prompt(&mut self, restore_input: bool) -> bool {
        let Some(target) = self.internal_agent_model_target.as_ref() else {
            return false;
        };
        if target.id != DECISION_SCREEN_ID {
            return false;
        }
        let origin = target.origin;
        self.internal_agent_model_target = None;
        if restore_input {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        let status = match origin {
            InternalAgentModelPickerOrigin::PermissionScreenSetupConfigRow
            | InternalAgentModelPickerOrigin::PermissionScreenSetupCommand => {
                match self.info.runtime.internal_agents.get(DECISION_SCREEN_ID) {
                    Some(screen) => format!(
                        "permission mode: auto. The screen keeps {}",
                        screen.display_reference()
                    ),
                    None => "permission mode: auto. The classifier model answers the screen".into(),
                }
            }
            _ => "permission screen model unchanged".into(),
        };
        self.set_status(status);
        true
    }

    /// The auth a decision row is saved with: a stored credential when the
    /// host has one, else its default mode.
    fn screen_auth(&self, provider: &str) -> String {
        provider::provider_descriptor(provider)
            .map(|descriptor| {
                super::model_actions::refresh_auth_for_provider(
                    descriptor,
                    &self.info.runtime.auth,
                    &self.available_auths,
                )
            })
            .unwrap_or(provider::KEYLESS_AUTH)
            .to_string()
    }

    /// Saves `percent` as the allow threshold of the session's screen entry,
    /// writing that whole entry so the saved config names the same screen the
    /// session uses. On a save failure nothing changes.
    pub(super) fn store_allow_threshold(&mut self, percent: u8) {
        let Some(mut selection) = self
            .info
            .runtime
            .internal_agents
            .get(DECISION_SCREEN_ID)
            .filter(|selection| selection.rho().is_some())
            .cloned()
        else {
            self.insert_entry(&Entry::Error(
                "could not save the screen allow threshold: the permission screen has no decision model".into(),
            ));
            return;
        };
        if let crate::config::InternalAgentTarget::Rho(rho) = &mut selection.target {
            rho.allow_threshold_percent = Some(percent);
        }
        let saved = self.info.services.config_repository.update(|config| {
            config.set_internal_agent_model_config(DECISION_SCREEN_ID, selection.clone());
        });
        match saved {
            Ok(()) => {
                self.info
                    .runtime
                    .internal_agents
                    .insert(DECISION_SCREEN_ID.into(), selection);
                // The number editor has no agent; the next idle pass or turn
                // start hands the classifier the new value.
                self.classifier_config_sync_pending = true;
                self.set_status(format!("screen allow threshold set to {percent}%"));
            }
            Err(err) => self.insert_entry(&Entry::Error(format!(
                "could not save the screen allow threshold: {err}"
            ))),
        }
    }

    fn store_screen_model(&mut self, mut selection: Option<InternalAgentModelConfig>) {
        // A threshold the user set is their policy, not the old model's, so
        // it carries over to the new one.
        let kept = screen_entry(&self.info.runtime.internal_agents)
            .and_then(|screen| screen.allow_threshold_percent);
        if let Some(crate::config::InternalAgentTarget::Rho(rho)) =
            selection.as_mut().map(|selection| &mut selection.target)
        {
            rho.allow_threshold_percent = kept;
        }
        let label = selection
            .as_ref()
            .map(InternalAgentModelConfig::display_reference)
            .unwrap_or_else(|| "the classifier model".into());
        let saved = match selection {
            Some(selection) => {
                self.info
                    .runtime
                    .internal_agents
                    .insert(DECISION_SCREEN_ID.into(), selection.clone());
                self.info.services.config_repository.update(|config| {
                    config.set_internal_agent_model_config(DECISION_SCREEN_ID, selection);
                })
            }
            None => {
                self.info.runtime.internal_agents.remove(DECISION_SCREEN_ID);
                self.info
                    .services
                    .config_repository
                    .update(|config| config.clear_internal_agent_model(DECISION_SCREEN_ID))
            }
        };
        match saved {
            Ok(()) => self.set_status(format!("permission screen now uses {label}")),
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save the permission screen model: {err}"
                )));
                self.set_status(format!(
                    "permission screen uses {label} for this session only"
                ));
            }
        }
    }
}

fn set_kind(selection: &mut InternalAgentModelConfig, kind: ModelKind) {
    if let crate::config::InternalAgentTarget::Rho(rho) = &mut selection.target {
        rho.kind = Some(kind);
    }
}

/// The screen entry in `agents`, when it names a model on Rho's providers.
fn screen_entry(
    agents: &BTreeMap<String, InternalAgentModelConfig>,
) -> Option<&crate::config::RhoInternalAgentModel> {
    agents.get(DECISION_SCREEN_ID)?.rho()
}

/// The allow threshold row's badge, or `None` when the row is hidden: the
/// screen is not a decision model, so no answer carries a probability.
pub(super) fn allow_threshold_badge(info: &RuntimeModelView) -> Option<PickerBadge> {
    let screen = screen_entry(&info.internal_agents)?;
    if decision::entry_kind(screen) != ModelKind::Decision {
        return None;
    }
    Some(
        match crate::permission_classifier::screen_allow_percent(screen) {
            Ok(percent) => PickerBadge {
                text: format!("{percent}%"),
                tone: PickerBadgeTone::Selected,
            },
            Err(err) => PickerBadge {
                text: err.to_string(),
                tone: PickerBadgeTone::Warning,
            },
        },
    )
}

/// The picker row the configured screen entry names.
fn screen_selection(info: &RuntimeModelView) -> ScreenSelection {
    let Some(selection) = screen_entry(&info.internal_agents) else {
        return ScreenSelection::Classifier;
    };
    let provider = selection.provider.clone();
    match decision::entry_kind(selection) {
        ModelKind::Decision => ScreenSelection::Decision {
            model: decision::discovered_decision_model(&provider, &selection.model)
                .unwrap_or_else(|| selection.model.clone()),
            provider,
        },
        ModelKind::Text => ScreenSelection::Text {
            provider,
            model: selection.model.clone(),
        },
    }
}

/// Every discovered decision model on a host with a usable auth mode, as
/// `(provider, model)`. After `/logout typesafe`, Jev's cached rows stay
/// hidden rather than saving an entry that cannot sign in.
fn discovered_decision_models(available_auths: &[String]) -> Vec<(String, String)> {
    provider::providers()
        .iter()
        .filter(|descriptor| lists_decision_models(descriptor.name))
        .filter(|descriptor| {
            descriptor
                .auth_modes()
                .any(|mode| available_auths.iter().any(|auth| auth == mode.id))
        })
        .flat_map(|descriptor| {
            cached_decision_models(descriptor.name)
                .into_iter()
                .map(|model| (descriptor.name.to_string(), model))
        })
        .collect()
}

/// The config row's badge: the screen's model, warning when the entry's kind
/// disagrees with what its provider listed.
pub(super) fn screen_model_badge(info: &RuntimeModelView) -> PickerBadge {
    let Some(configured) = info.internal_agents.get(DECISION_SCREEN_ID) else {
        return PickerBadge {
            text: "same as classifier".into(),
            tone: PickerBadgeTone::Selected,
        };
    };
    match configured
        .rho()
        .and_then(crate::permission_classifier::screen_warning)
    {
        Some(mismatch) => PickerBadge {
            text: mismatch,
            tone: PickerBadgeTone::Warning,
        },
        None => PickerBadge {
            text: configured.display_reference(),
            tone: PickerBadgeTone::Selected,
        },
    }
}

#[cfg(test)]
#[path = "screen_model_tests.rs"]
mod tests;
