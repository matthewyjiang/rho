use super::{App, Entry, InputSubmissionMode, InteractiveRuntime};

/// TUI session phase distinct from provider run controller state.
///
/// `ProviderTurn` should stay aligned with `InteractiveRuntime::is_run_active`
/// except for brief setup before `start` succeeds. `Compacting` is UI-only busy
/// work with no active provider run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SessionUiPhase {
    #[default]
    Idle,
    ProviderTurn,
    Compacting,
}

impl SessionUiPhase {
    pub(super) const fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(super) const fn is_provider_turn(self) -> bool {
        matches!(self, Self::ProviderTurn)
    }

    pub(super) const fn allows_idle_subagent_delivery(self) -> bool {
        matches!(self, Self::Idle)
    }

    pub(super) const fn uses_during_run_model_picker(self) -> bool {
        matches!(self, Self::ProviderTurn)
    }

    pub(super) const fn busy_status_label(self) -> &'static str {
        if self.is_busy() {
            "running"
        } else {
            "ready"
        }
    }
}

impl App {
    pub(super) fn is_ui_busy(&self) -> bool {
        self.session_ui.is_busy()
    }

    /// True only during an active provider turn, not compaction UI.
    pub(super) fn is_provider_turn_ui(&self) -> bool {
        self.session_ui.is_provider_turn()
    }

    pub(super) fn uses_during_run_model_picker(&self) -> bool {
        debug_assert_eq!(
            self.is_provider_turn_ui(),
            self.session_ui.uses_during_run_model_picker()
        );
        self.session_ui.uses_during_run_model_picker()
    }

    pub(super) fn busy_status_label(&self) -> &'static str {
        self.session_ui.busy_status_label()
    }

    pub(super) fn allows_idle_subagent_delivery(&self) -> bool {
        self.session_ui.allows_idle_subagent_delivery()
    }

    pub(super) fn begin_provider_turn_ui(&mut self) {
        self.session_ui = SessionUiPhase::ProviderTurn;
    }

    pub(super) fn begin_compact_ui(&mut self) {
        self.session_ui = SessionUiPhase::Compacting;
    }

    pub(super) fn end_busy_ui(&mut self) {
        self.session_ui = SessionUiPhase::Idle;
    }

    /// After a successful provider start or terminal finish, provider-turn UI
    /// must match the run controller. Compaction uses [`Self::begin_compact_ui`]
    /// and must not call this.
    pub(super) fn debug_assert_provider_turn_sync(&self, agent: &InteractiveRuntime) {
        debug_assert_eq!(
            self.session_ui.is_provider_turn(),
            agent.is_run_active(),
            "session_ui={:?} but InteractiveRuntime.is_run_active()={}",
            self.session_ui,
            agent.is_run_active()
        );
    }

    pub(super) fn preserve_unapplied_steering_as_follow_ups(&mut self) {
        self.pending.preserve_unapplied_steering_as_follow_ups();
        self.pending_input_changed();
        self.select_pending_recall_target();
    }

    pub(super) fn restore_pending_work_to_input(&mut self) {
        let mut messages = self
            .pending
            .accepted_steering
            .drain(..)
            .map(|entry| entry.prompt.prompt)
            .collect::<Vec<_>>();
        messages.extend(
            self.pending
                .steering_prompts
                .drain(..)
                .map(|prompt| prompt.prompt),
        );
        messages.extend(
            self.pending
                .queued_prompts
                .drain(..)
                .map(|prompt| prompt.prompt),
        );
        if messages.is_empty() {
            return;
        }
        if !self.expanded_input().trim().is_empty() {
            messages.push(self.expanded_input());
        }
        self.input_ui.text = messages.join("\n\n");
        self.input_ui.paste_segments.clear();
        self.input_ui.shell_mode = None;
        self.input_ui.cursor = self.input_char_len();
        self.input_ui.submission_mode = InputSubmissionMode::ParseCommands;
        self.reset_input_history_navigation();
        self.input_changed();
        self.pending_input_changed();
    }

    pub(super) fn clear_transient_key_state(&mut self) {
        self.input_ui.paste_burst.clear();
        self.ctrl_c_streak = 0;
    }

    pub(super) fn insert_runtime_notices(&mut self, agent: &mut InteractiveRuntime) {
        for notice in agent.take_notices() {
            self.insert_entry(&Entry::Notice(notice));
        }
    }
}

#[cfg(test)]
#[path = "run_lifecycle_tests.rs"]
mod tests;
