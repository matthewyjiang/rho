//! Composer cycle and session-scoped all/pinned model picker views.

use crossterm::event::KeyEvent;
use rho_providers::model::favorites::CycleDirection;

use super::{model_picker, App, ComposerMode, UiPicker};

impl App {
    /// Scope the next model picker over the signed-in catalogue should open
    /// on, as the internal-agent pickers are.
    ///
    /// An explicit Ctrl-O choice sticks until the user toggles again. Until
    /// then, prefer pinned and recompute on every open so a first-run picker
    /// that opened on all still shows pins after the user adds one.
    pub(super) fn resolved_model_picker_scope(&self) -> model_picker::ModelPickerScope {
        model_picker::effective_model_picker_scope(
            self.preferred_model_picker_scope(),
            &self.info.runtime.favorite_models,
            &self.available_auths,
        )
    }

    /// [`Self::resolved_model_picker_scope`] for the conversation's own
    /// picker, which a daemon session fills with the models it offers.
    pub(super) fn conversation_model_picker_scope(&self) -> model_picker::ModelPickerScope {
        self.usable_conversation_scope(self.preferred_model_picker_scope())
    }

    fn preferred_model_picker_scope(&self) -> model_picker::ModelPickerScope {
        self.model_picker_scope_override
            .unwrap_or(model_picker::ModelPickerScope::Pinned)
    }

    /// `scope` as the conversation's picker can show it: pinned only when a
    /// pin is among the models a daemon session offers, or else the
    /// signed-in catalogue.
    fn usable_conversation_scope(
        &self,
        scope: model_picker::ModelPickerScope,
    ) -> model_picker::ModelPickerScope {
        let favorite_models = &self.info.runtime.favorite_models;
        match &self.daemon_options {
            Some(options) => model_picker::offered_model_picker_scope(
                scope,
                favorite_models,
                &model_picker::offered_models(&options.model),
            ),
            None => model_picker::effective_model_picker_scope(
                scope,
                favorite_models,
                &self.available_auths,
            ),
        }
    }

    pub(super) fn conversation_model_picker(&mut self) -> UiPicker {
        if let Some(picker) = self.daemon_model_picker() {
            return picker;
        }
        self.refresh_available_auths();
        let scope = self.conversation_model_picker_scope();
        model_picker::model_picker(&self.info.runtime, &self.available_auths, scope)
    }

    pub(super) fn conversation_model_picker_during_run(&mut self) -> UiPicker {
        if let Some(picker) = self.daemon_model_picker() {
            return picker;
        }
        self.refresh_available_auths();
        let scope = self.conversation_model_picker_scope();
        model_picker::model_picker_during_run(
            &self.info.runtime,
            /*pending*/ None,
            &self.available_auths,
            scope,
        )
    }

    /// Rebuilds the open model picker in place, keeping filter, cursor, and
    /// parent. `None` when no model picker is open.
    fn rebuilt_model_picker(&mut self, selected_value: &str, filter: String) -> Option<UiPicker> {
        let mut picker = match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_conversation_model() => {
                if self.is_provider_turn_ui() {
                    self.conversation_model_picker_during_run()
                } else {
                    self.conversation_model_picker()
                }
            }
            ComposerMode::Picker(picker) if picker.is_internal_agent_model() => {
                let target = self.internal_agent_model_target.clone()?;
                if target.id == crate::permission_classifier::DECISION_SCREEN_ID {
                    self.screen_model_picker(target.origin)
                } else {
                    self.internal_agent_model_picker(target)
                }
            }
            _ => return None,
        };
        // Taken last: the arms above can bail, and dropping the parent on a
        // rebuild that never happens would strand the user's way back.
        let parent = match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => picker.take_parent(),
            _ => None,
        };
        if let Some(parent) = parent {
            picker = picker.with_parent(parent);
        }
        Self::restore_picker_position(&mut picker, selected_value, filter);
        Some(picker)
    }

    pub(super) fn rebuild_open_model_picker(&mut self, selected_value: &str, filter: String) {
        if let Some(picker) = self.rebuilt_model_picker(selected_value, filter) {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
    }

    pub(super) fn toggle_model_picker_scope(&mut self) -> anyhow::Result<()> {
        let (value, filter, conversation) = match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_model_list() => (
                picker
                    .selected_item()
                    .map(|item| item.value.clone())
                    .unwrap_or_default(),
                picker.filter.clone(),
                picker.is_conversation_model(),
            ),
            _ => return Ok(()),
        };
        if value.is_empty() {
            return Ok(());
        }
        self.refresh_available_auths();
        // Each picker toggles among the models it lists.
        let (current, usable) = if conversation {
            let current = self.conversation_model_picker_scope();
            (current, self.usable_conversation_scope(current.other()))
        } else {
            let current = self.resolved_model_picker_scope();
            let usable = model_picker::effective_model_picker_scope(
                current.other(),
                &self.info.runtime.favorite_models,
                &self.available_auths,
            );
            (current, usable)
        };
        let next = current.other();
        if usable != next {
            self.set_status("no pinned models");
            return Ok(());
        }
        // Scope and picker move together: a rebuild that cannot happen must not
        // leave the session flipped to a view the user never sees.
        let previous = self.model_picker_scope_override;
        self.model_picker_scope_override = Some(next);
        match self.rebuilt_model_picker(&value, filter) {
            Some(picker) => {
                self.input_ui.set_composer(ComposerMode::Picker(picker));
                self.set_status(format!("showing {}", next.status_label()));
            }
            None => self.model_picker_scope_override = previous,
        }
        Ok(())
    }

    fn favorite_cycle_direction(&self, key: KeyEvent) -> Option<CycleDirection> {
        let keybindings = &self.info.runtime.keybindings;
        if keybindings.cycle_pinned_model.matches(key) {
            Some(CycleDirection::Forward)
        } else if keybindings.cycle_pinned_model_back.matches(key) {
            Some(CycleDirection::Backward)
        } else {
            None
        }
    }

    /// Cycles the session's model through the pins on the cycle keys, idle
    /// or while a turn runs. Returns whether the key was one of them.
    pub(super) fn handle_favorite_cycle_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        let Some(direction) = self.favorite_cycle_direction(key) else {
            return Ok(false);
        };
        self.cycle_daemon_model(direction)?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "model_cycle_tests.rs"]
mod tests;
