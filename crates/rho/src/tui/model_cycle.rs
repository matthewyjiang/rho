//! Composer cycle and session-scoped all/pinned model picker views.

use crossterm::event::KeyEvent;
use rho_providers::model::favorites::{CycleDirection, CycleOutcome};

use super::{
    catalog, favorites, model_picker, App, ComposerMode, Entry, InteractiveModelSelection,
    InteractiveRuntime, PickerAction, UiPicker,
};

/// What a composer pin cycle resolved to.
///
/// Idle and during-turn key handling differ only in how [`Self::Switch`] is
/// applied, so the decision is made once here and dispatched by the caller.
enum CycleTarget {
    /// Not in the composer, so the key is not ours to handle.
    NotComposing,
    /// No pin currently has auth.
    NoPins,
    /// The only usable pin is already active.
    Unchanged,
    /// The pin resolved, but not to a usable selection.
    Failed(String),
    /// Switch to this selection.
    Switch(Box<InteractiveModelSelection>),
}

impl App {
    /// Records the scope a model picker opens on for the rest of the session.
    ///
    /// Called only when a picker is opened, so browsing "all" and then pinning
    /// a model does not silently collapse the list under the user.
    pub(super) fn latch_model_picker_scope_on_open(&mut self) -> model_picker::ModelPickerScope {
        // With no explicit choice yet, prefer pinned; it degrades to all on its
        // own when no pin has auth.
        let requested = self
            .model_picker_scope
            .unwrap_or(model_picker::ModelPickerScope::Pinned);
        let scope = model_picker::effective_model_picker_scope(
            requested,
            &self.info.runtime.favorite_models,
            &self.available_auths,
        );
        self.model_picker_scope = Some(scope);
        scope
    }

    pub(super) fn conversation_model_picker(&mut self) -> UiPicker {
        self.refresh_available_auths();
        let scope = self.latch_model_picker_scope_on_open();
        model_picker::model_picker(&self.info.runtime, &self.available_auths, scope)
    }

    pub(super) fn conversation_model_picker_during_run(&mut self) -> UiPicker {
        self.refresh_available_auths();
        let scope = self.latch_model_picker_scope_on_open();
        model_picker::model_picker_during_run(
            &self.info.runtime,
            self.pending_model_selection
                .as_ref()
                .map(|pending| &pending.selection),
            &self.available_auths,
            scope,
        )
    }

    /// Rebuilds the open model picker in place, keeping filter, cursor, and
    /// parent. `None` when no model picker is open.
    fn rebuilt_model_picker(&mut self, selected_value: &str, filter: String) -> Option<UiPicker> {
        let (action, _) = self.active_picker_selection()?;
        let mut picker = match action {
            PickerAction::SelectModel if self.is_provider_turn_ui() => {
                self.conversation_model_picker_during_run()
            }
            PickerAction::SelectModel => self.conversation_model_picker(),
            PickerAction::SelectInternalAgentModel => {
                let target = self.internal_agent_model_target.clone()?;
                self.internal_agent_model_picker(&target.id, target.origin)
            }
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::SwitchAuthMode
            | PickerAction::RefreshModelList
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ResumeSession
            | PickerAction::ManageSessions
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::Config
            | PickerAction::SelectTheme
            | PickerAction::EditAgent
            | PickerAction::Workflow
            | PickerAction::AttachSubagent
            | PickerAction::Dismiss
            | PickerAction::ViewMcpServers => return None,
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
        let current = self.latch_model_picker_scope_on_open();
        let next = current.other();
        if model_picker::effective_model_picker_scope(
            next,
            &self.info.runtime.favorite_models,
            &self.available_auths,
        ) != next
        {
            self.set_status("no pinned models");
            return Ok(());
        }
        // Scope and picker move together: a rebuild that cannot happen must not
        // leave the session flipped to a view the user never sees.
        self.model_picker_scope = Some(next);
        match self.rebuilt_model_picker(&value, filter) {
            Some(picker) => {
                self.input_ui.set_composer(ComposerMode::Picker(picker));
                self.set_status(format!("showing {}", next.status_label()));
            }
            None => self.model_picker_scope = Some(current),
        }
        Ok(())
    }

    /// Resolves the pin the composer would cycle to, without applying it.
    fn next_pinned_selection(&mut self, direction: CycleDirection) -> CycleTarget {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return CycleTarget::NotComposing;
        }
        self.refresh_available_auths();
        let favorites = favorites::normalized_favorite_models(&self.info.runtime.favorite_models);
        let available = catalog::available_models_for_auths(&self.available_auths);
        let next = match favorites::cycle_favorite(
            &favorites,
            &available,
            &self.info.runtime.provider,
            &self.info.runtime.model,
            direction,
        ) {
            CycleOutcome::NoPins => return CycleTarget::NoPins,
            CycleOutcome::Unchanged => return CycleTarget::Unchanged,
            CycleOutcome::Switch(favorite) => favorite.value(),
        };
        match self.resolve_model_selection(
            &next,
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) => CycleTarget::Switch(Box::new(selection)),
            Err(err) => CycleTarget::Failed(err.to_string()),
        }
    }

    /// Reports the non-switching outcomes. Returns whether the key was consumed.
    fn report_cycle_target(&mut self, target: &CycleTarget) -> bool {
        match target {
            CycleTarget::NotComposing => return false,
            CycleTarget::NoPins => self.notify_status("no pinned models"),
            CycleTarget::Unchanged => {}
            CycleTarget::Failed(message) => {
                self.insert_entry(&Entry::Error(message.clone()));
                self.set_status("model switch failed");
            }
            CycleTarget::Switch(_) => {}
        }
        self.clear_transient_key_state();
        true
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

    pub(super) async fn handle_favorite_cycle_key(
        &mut self,
        key: KeyEvent,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(direction) = self.favorite_cycle_direction(key) else {
            return Ok(false);
        };
        let target = self.next_pinned_selection(direction);
        if !self.report_cycle_target(&target) {
            return Ok(false);
        }
        if let CycleTarget::Switch(selection) = target {
            // Compaction is busy UI without a live run, so an idle press still
            // applies immediately unless a provider turn actually owns the run.
            if self.is_provider_turn_ui() {
                self.queue_model_selection(*selection)?;
            } else {
                self.request_model_selection(*selection, agent).await?;
            }
        }
        Ok(true)
    }

    pub(super) fn handle_running_favorite_cycle_key(
        &mut self,
        key: KeyEvent,
    ) -> anyhow::Result<bool> {
        let Some(direction) = self.favorite_cycle_direction(key) else {
            return Ok(false);
        };
        let target = self.next_pinned_selection(direction);
        if !self.report_cycle_target(&target) {
            return Ok(false);
        }
        if let CycleTarget::Switch(selection) = target {
            self.queue_model_selection(*selection)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "model_cycle_tests.rs"]
mod tests;
