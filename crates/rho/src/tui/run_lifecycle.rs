use super::{App, Entry, InputSubmissionMode, InteractiveRuntime};

impl App {
    pub(super) fn is_ui_busy(&self) -> bool {
        self.turn.is_busy()
    }

    /// True only during an active provider turn, not compaction UI.
    pub(super) fn is_provider_turn_ui(&self) -> bool {
        self.turn.is_provider_turn()
    }

    pub(super) fn busy_status_label(&self) -> &'static str {
        self.turn.session_ui().busy_status_label()
    }

    pub(super) fn allows_idle_subagent_delivery(&self) -> bool {
        self.turn.session_ui().allows_idle_subagent_delivery()
    }

    pub(super) fn begin_provider_turn_ui(&mut self) {
        self.turn.enter_provider_turn();
    }

    pub(super) fn begin_compact_ui(&mut self) {
        self.turn.enter_compact();
    }

    pub(super) fn end_busy_ui(&mut self) {
        self.turn.end_busy();
    }

    /// After a successful provider start or terminal finish, provider-turn UI
    /// must match the run controller. Compaction uses [`Self::begin_compact_ui`]
    /// and must not call this.
    pub(super) fn debug_assert_provider_turn_sync(&self, agent: &InteractiveRuntime) {
        debug_assert_eq!(
            self.turn.is_provider_turn(),
            agent.is_run_active(),
            "session_ui={:?} but InteractiveRuntime.is_run_active()={}",
            self.turn.session_ui(),
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
            .drain_accepted_steering_prompts()
            .collect::<Vec<_>>();
        messages.extend(self.pending.drain_steering_prompt_texts());
        messages.extend(self.pending.drain_queued_prompt_texts());
        if messages.is_empty() {
            return;
        }
        if !self.expanded_input().trim().is_empty() {
            messages.push(self.expanded_input());
        }
        let text = messages.join("\n\n");
        let cursor = text.chars().count();
        self.input_ui.set_text_and_cursor(text, cursor);
        self.input_ui.clear_paste_segments();
        self.input_ui.set_shell_mode(None);
        self.input_ui
            .set_submission_mode(InputSubmissionMode::ParseCommands);
        self.reset_input_history_navigation();
        self.input_changed();
        self.pending_input_changed();
    }

    pub(super) fn clear_transient_key_state(&mut self) {
        self.input_ui.clear_transient_edit_state();
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
