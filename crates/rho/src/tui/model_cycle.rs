//! Composer cycle and session-scoped all/pinned model picker views.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rho_providers::model::favorites::CycleDirection;

use super::{
    catalog, favorites, model_picker, App, ComposerMode, Entry, InteractiveRuntime, PickerAction,
    UiPicker,
};

impl App {
    pub(super) fn resolved_model_picker_scope(&mut self) -> model_picker::ModelPickerScope {
        if let Some(scope) = self.model_picker_scope {
            return scope;
        }
        let scope = model_picker::default_model_picker_scope(
            &self.info.runtime.favorite_models,
            &self.available_auths,
        );
        self.model_picker_scope = Some(scope);
        scope
    }

    pub(super) fn conversation_model_picker(&mut self) -> UiPicker {
        self.refresh_available_auths();
        let scope = self.resolved_model_picker_scope();
        model_picker::model_picker(&self.info.runtime, &self.available_auths, scope)
    }

    pub(super) fn conversation_model_picker_during_run(&mut self) -> UiPicker {
        self.refresh_available_auths();
        let scope = self.resolved_model_picker_scope();
        model_picker::model_picker_during_run(
            &self.info.runtime,
            self.pending_model_selection
                .as_ref()
                .map(|pending| &pending.selection),
            &self.available_auths,
            scope,
        )
    }

    pub(super) fn rebuild_open_model_picker(
        &mut self,
        selected_value: &str,
        filter: String,
    ) -> bool {
        let Some((action, _)) = self.active_picker_selection() else {
            return false;
        };
        let parent = match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => picker.take_parent(),
            _ => None,
        };
        let mut picker = match action {
            PickerAction::SelectModel if self.is_provider_turn_ui() => {
                self.conversation_model_picker_during_run()
            }
            PickerAction::SelectModel => self.conversation_model_picker(),
            PickerAction::SelectInternalAgentModel => {
                let Some(target) = self.internal_agent_model_target.clone() else {
                    return false;
                };
                self.internal_agent_model_picker(&target.id, target.origin)
            }
            _ => return false,
        };
        if let Some(parent) = parent {
            picker = picker.with_parent(parent);
        }
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        true
    }

    pub(super) fn sync_model_picker_scope_after_pin_change(&mut self) {
        if self.resolved_model_picker_scope() == model_picker::ModelPickerScope::Pinned
            && model_picker::default_model_picker_scope(
                &self.info.runtime.favorite_models,
                &self.available_auths,
            ) != model_picker::ModelPickerScope::Pinned
        {
            self.model_picker_scope = Some(model_picker::ModelPickerScope::All);
        }
    }

    pub(super) fn toggle_model_picker_scope(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            return Ok(());
        };
        if !matches!(
            action,
            PickerAction::SelectModel | PickerAction::SelectInternalAgentModel
        ) {
            return Ok(());
        }
        let filter = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        self.refresh_available_auths();
        let current = self.resolved_model_picker_scope();
        let next = current.other();
        if next == model_picker::ModelPickerScope::Pinned
            && model_picker::default_model_picker_scope(
                &self.info.runtime.favorite_models,
                &self.available_auths,
            ) != model_picker::ModelPickerScope::Pinned
        {
            self.set_status("no pinned models");
            return Ok(());
        }
        self.model_picker_scope = Some(next);
        if self.rebuild_open_model_picker(&value, filter) {
            let view = match next {
                model_picker::ModelPickerScope::All => "all models",
                model_picker::ModelPickerScope::Pinned => "pinned models",
            };
            self.set_status(format!("showing {view}"));
        }
        Ok(())
    }

    pub(super) async fn cycle_favorite_model(
        &mut self,
        direction: CycleDirection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return Ok(false);
        }
        self.refresh_available_auths();
        let favorites = favorites::normalized_favorite_models(&self.info.runtime.favorite_models);
        let available = catalog::available_models_for_auths(&self.available_auths);
        let Some(next) = favorites::cycle_favorite(
            &favorites,
            &available,
            &self.info.runtime.provider,
            &self.info.runtime.model,
            direction,
        ) else {
            if favorites::available_favorites(&favorites, &available).is_empty() {
                self.notify_status("no pinned models");
            }
            return Ok(true);
        };
        match self.resolve_model_selection(
            &next.value(),
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) if self.is_provider_turn_ui() => self.queue_model_selection(selection)?,
            Ok(selection) => self.request_model_selection(selection, agent).await?,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("model switch failed");
            }
        }
        Ok(true)
    }

    fn favorite_cycle_direction(key: KeyEvent) -> Option<CycleDirection> {
        if !matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P')) {
            return None;
        }
        if !key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return None;
        }
        Some(if key.modifiers.contains(KeyModifiers::SHIFT) {
            CycleDirection::Backward
        } else {
            CycleDirection::Forward
        })
    }

    pub(super) async fn handle_favorite_cycle_key(
        &mut self,
        key: KeyEvent,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(direction) = Self::favorite_cycle_direction(key) else {
            return Ok(false);
        };
        let handled = self.cycle_favorite_model(direction, agent).await?;
        if handled {
            self.clear_transient_key_state();
        }
        Ok(handled)
    }

    pub(super) fn handle_running_favorite_cycle_key(
        &mut self,
        key: KeyEvent,
    ) -> anyhow::Result<bool> {
        let Some(direction) = Self::favorite_cycle_direction(key) else {
            return Ok(false);
        };
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return Ok(false);
        }
        self.refresh_available_auths();
        let favorites = favorites::normalized_favorite_models(&self.info.runtime.favorite_models);
        let available = catalog::available_models_for_auths(&self.available_auths);
        let Some(next) = favorites::cycle_favorite(
            &favorites,
            &available,
            &self.info.runtime.provider,
            &self.info.runtime.model,
            direction,
        ) else {
            if favorites::available_favorites(&favorites, &available).is_empty() {
                self.notify_status("no pinned models");
            }
            self.clear_transient_key_state();
            return Ok(true);
        };
        match self.resolve_model_selection(
            &next.value(),
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) => self.queue_model_selection(selection)?,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("model switch failed");
            }
        }
        self.clear_transient_key_state();
        Ok(true)
    }
}
