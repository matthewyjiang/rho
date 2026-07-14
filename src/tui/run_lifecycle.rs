use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use super::{App, InputSubmissionMode};

#[cfg(test)]
use super::QueuedPrompt;

impl App {
    pub(super) fn restore_pending_work_to_input(
        &mut self,
        active_steering: &Arc<Mutex<VecDeque<String>>>,
    ) {
        let mut messages = active_steering
            .lock()
            .unwrap()
            .drain(..)
            .collect::<Vec<_>>();
        messages.extend(self.steering_prompts.drain(..));
        messages.extend(self.queued_prompts.drain(..).map(|prompt| prompt.prompt));
        if messages.is_empty() {
            return;
        }
        if !self.expanded_input().trim().is_empty() {
            messages.push(self.expanded_input());
        }
        self.input = messages.join("\n\n");
        self.paste_segments.clear();
        self.input_cursor = self.input_char_len();
        self.input_submission_mode = InputSubmissionMode::ParseCommands;
        self.reset_input_history_navigation();
        self.input_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::tests::test_app;

    #[test]
    fn interrupt_restores_all_pending_work_to_input() {
        let mut app = test_app();
        app.input = "draft".into();
        app.input_cursor = app.input_char_len();
        app.steering_prompts.push_back("local steer".into());
        app.queued_prompts.push_back(QueuedPrompt {
            prompt: "expanded next turn".into(),
            display_prompt: "next turn".into(),
            paste_segments: Vec::new(),
        });
        let active = Arc::new(Mutex::new(VecDeque::from(["active steer".into()])));

        app.restore_pending_work_to_input(&active);

        assert_eq!(
            app.input,
            "active steer\n\nlocal steer\n\nexpanded next turn\n\ndraft"
        );
        assert!(active.lock().unwrap().is_empty());
        assert!(app.steering_prompts.is_empty());
        assert!(app.queued_prompts.is_empty());
        assert_eq!(app.input_cursor, app.input_char_len());
    }

    #[test]
    fn interrupt_expands_pasted_draft_before_restoring_it() {
        let mut app = test_app();
        app.insert_pasted_input_text("alpha\nbeta");
        app.steering_prompts.push_back("steer".into());

        app.restore_pending_work_to_input(&Arc::default());

        assert_eq!(app.input, "steer\n\nalpha\nbeta");
        assert!(app.paste_segments.is_empty());
    }
}
