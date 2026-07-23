use super::{App, InputSubmissionMode};

impl App {
    pub(super) fn preserve_unapplied_steering_as_follow_ups(&mut self) {
        let mut pending = self
            .accepted_steering
            .drain(..)
            .map(|entry| entry.prompt)
            .chain(self.steering_prompts.drain(..))
            .collect::<std::collections::VecDeque<_>>();
        pending.append(&mut self.queued_prompts);
        self.queued_prompts = pending;
        self.retracting_steering = None;
        self.pending_input_action = None;
        self.pending_input_changed();
        self.select_pending_recall_target();
    }

    pub(super) fn restore_pending_work_to_input(&mut self) {
        let mut messages = self
            .accepted_steering
            .drain(..)
            .map(|entry| entry.prompt.prompt)
            .collect::<Vec<_>>();
        messages.extend(self.steering_prompts.drain(..).map(|prompt| prompt.prompt));
        messages.extend(self.queued_prompts.drain(..).map(|prompt| prompt.prompt));
        if messages.is_empty() {
            return;
        }
        if !self.expanded_input().trim().is_empty() {
            messages.push(self.expanded_input());
        }
        self.input = messages.join("\n\n");
        self.paste_segments.clear();
        self.shell_mode = None;
        self.input_cursor = self.input_char_len();
        self.input_submission_mode = InputSubmissionMode::ParseCommands;
        self.reset_input_history_navigation();
        self.input_changed();
        self.pending_input_changed();
    }
}

#[cfg(test)]
#[path = "run_lifecycle_tests.rs"]
mod tests;
