use super::{message_history, App, ComposerMode, Entry, InputSubmissionMode, QueuedPrompt};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug)]
pub(super) struct AcceptedSteering {
    pub(super) id: rho_sdk::SteeringId,
    pub(super) prompt: QueuedPrompt,
    pub(super) delivered: bool,
}

#[derive(Debug, Default)]
pub(super) struct PendingInputPanel {
    focused: bool,
    selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingInputRef {
    AcceptedSteering(usize),
    LocalSteering(usize),
    FollowUp(usize),
}

#[derive(Debug)]
enum PendingSelectionAnchor {
    Accepted(rho_sdk::SteeringId),
    Local(usize),
    FollowUp(usize),
}

#[derive(Debug)]
pub(super) enum PendingInputAction {
    EditAccepted {
        id: rho_sdk::SteeringId,
        prompt: QueuedPrompt,
    },
    DiscardAccepted {
        id: rho_sdk::SteeringId,
    },
}

impl App {
    pub(super) fn pending_input_focused(&self) -> bool {
        self.pending.input_panel().focused
    }

    pub(super) fn handle_pending_input_key(&mut self, key: KeyEvent) -> bool {
        if self
            .info
            .runtime
            .keybindings
            .manage_pending_input
            .matches(key)
        {
            if self.pending_input_count() == 0 {
                self.notify_status("no pending input");
            } else {
                self.pending.input_panel_mut().focused = !self.pending.input_panel().focused;
                if self.pending.input_panel().focused {
                    self.select_pending_recall_target();
                }
            }
            self.ctrl_c_streak = 0;
            return true;
        }

        if !self.pending.input_panel().focused {
            if self
                .info
                .runtime
                .keybindings
                .edit_pending_input
                .matches(key)
            {
                self.recall_latest_pending_input();
                self.ctrl_c_streak = 0;
                return true;
            }
            return false;
        }

        if self
            .info
            .runtime
            .keybindings
            .edit_pending_input
            .matches(key)
        {
            self.recall_latest_pending_input();
            self.ctrl_c_streak = 0;
            return true;
        }

        let count = self.pending_input_count();
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.pending.input_panel_mut().focused = false;
            }
            (_, KeyCode::Up) => {
                self.pending.input_panel_mut().selected =
                    self.pending.input_panel().selected.saturating_sub(1);
            }
            (_, KeyCode::Down) => {
                self.pending.input_panel_mut().selected =
                    (self.pending.input_panel().selected + 1).min(count.saturating_sub(1));
            }
            (_, KeyCode::Home) => self.pending.input_panel_mut().selected = 0,
            (_, KeyCode::End) => self.pending.input_panel_mut().selected = count.saturating_sub(1),
            (_, KeyCode::Enter) => self.edit_selected_pending_input(),
            (_, KeyCode::Backspace | KeyCode::Delete) => {
                self.discard_selected_pending_input();
            }
            (modifiers, KeyCode::Char(_))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.pending.input_panel_mut().focused = false;
                return false;
            }
            _ => {}
        }
        self.ctrl_c_streak = 0;
        true
    }

    /// Shows what withdrawing the steer `action` names did.
    pub(super) fn finish_steering_retraction(
        &mut self,
        action: PendingInputAction,
        result: Result<rho_sdk::SteeringRetraction, String>,
    ) {
        let (id, edit_prompt) = match action {
            PendingInputAction::EditAccepted { id, prompt } => (id, Some(prompt)),
            PendingInputAction::DiscardAccepted { id } => (id, None),
        };
        match result {
            Ok(rho_sdk::SteeringRetraction::Retracted) => {
                let _ = self.take_accepted_steering(std::slice::from_ref(&id));
                if let Some(prompt) = edit_prompt {
                    self.restore_pending_prompt(prompt);
                    self.notify_status("editing retracted steer");
                } else {
                    self.notify_status("steer discarded");
                }
            }
            Ok(rho_sdk::SteeringRetraction::Delivered) => {
                // The provider already received it mid-turn. It stays pending until
                // the SDK emits SteeringApplied, the authority for transcript insertion.
                self.notify_status("steer was already sent to the model");
            }
            Ok(rho_sdk::SteeringRetraction::AlreadyApplied) => {
                // SteeringApplied, not this receipt, inserts the steer into the transcript.
                self.notify_status("steer can no longer be changed");
            }
            Ok(rho_sdk::SteeringRetraction::NotFound) => {
                let _ = self.take_accepted_steering(std::slice::from_ref(&id));
                self.notify_status("steer is no longer pending");
            }
            Err(error) => {
                self.notify_status(format!(
                    "could not confirm steer retraction; it remains pending: {error}"
                ));
            }
        }
    }

    pub(super) fn record_delivered_steering(&mut self, id: &rho_sdk::SteeringId) {
        if let Some(entry) = self
            .pending
            .accepted_steering_mut()
            .iter_mut()
            .find(|entry| &entry.id == id)
        {
            entry.delivered = true;
        }
    }

    pub(super) fn record_applied_steering(&mut self, ids: &[rho_sdk::SteeringId]) {
        let prompts = self.take_accepted_steering(ids);
        if prompts.is_empty() {
            return;
        }
        self.finish_streams();
        for prompt in prompts {
            self.insert_entry(&Entry::User(message_history::render_user_entry(
                &prompt.display_prompt,
                &prompt.media,
            )));
        }
    }

    /// Drops the accepted steers `ids` from the pending panel and returns
    /// them.
    pub(super) fn take_accepted_steering(
        &mut self,
        ids: &[rho_sdk::SteeringId],
    ) -> Vec<QueuedPrompt> {
        let selection = self.pending_selection_anchor();
        let mut prompts = Vec::new();
        self.pending.accepted_steering_mut().retain(|entry| {
            if !ids.contains(&entry.id) {
                return true;
            }
            prompts.push(entry.prompt.clone());
            false
        });
        self.restore_pending_selection(selection);
        self.pending_input_changed();
        prompts
    }

    pub(super) fn clear_accepted_steering(&mut self) {
        self.pending.accepted_steering_mut().clear();
        self.pending.clear_input_action();
        self.pending_input_changed();
    }

    pub(super) fn select_pending_recall_target(&mut self) {
        let accepted = self.pending.accepted_steering().len();
        let local = self.pending.steering_prompts().len();
        let follow_up = self.pending.queued_prompts().len();
        if accepted + local + follow_up == 0 {
            self.pending.input_panel_mut().focused = false;
            self.pending.input_panel_mut().selected = 0;
            return;
        }
        self.pending.input_panel_mut().selected = if local > 0 {
            accepted + local - 1
        } else if accepted > 0 {
            accepted - 1
        } else {
            follow_up.saturating_sub(1)
        };
    }

    pub(super) fn pending_input_changed(&mut self) {
        let count = self.pending_input_count();
        self.clamp_pending_input_selection(count);
        if count == 0 {
            self.pending.input_panel_mut().focused = false;
        }
    }

    fn pending_input_count(&self) -> usize {
        self.pending.accepted_steering().len()
            + self.pending.steering_prompts().len()
            + self.pending.queued_prompts().len()
    }

    fn pending_input_refs(&self) -> Vec<PendingInputRef> {
        (0..self.pending.accepted_steering().len())
            .map(PendingInputRef::AcceptedSteering)
            .chain((0..self.pending.steering_prompts().len()).map(PendingInputRef::LocalSteering))
            .chain((0..self.pending.queued_prompts().len()).map(PendingInputRef::FollowUp))
            .collect()
    }

    fn pending_selection_anchor(&self) -> Option<PendingSelectionAnchor> {
        match self
            .pending_input_refs()
            .get(self.pending.input_panel().selected)?
        {
            PendingInputRef::AcceptedSteering(index) => self
                .pending
                .accepted_steering()
                .get(*index)
                .map(|entry| PendingSelectionAnchor::Accepted(entry.id.clone())),
            PendingInputRef::LocalSteering(index) => Some(PendingSelectionAnchor::Local(*index)),
            PendingInputRef::FollowUp(index) => Some(PendingSelectionAnchor::FollowUp(*index)),
        }
    }

    fn restore_pending_selection(&mut self, selection: Option<PendingSelectionAnchor>) {
        let Some(selection) = selection else {
            return;
        };
        let selected = match selection {
            PendingSelectionAnchor::Accepted(id) => self
                .pending
                .accepted_steering()
                .iter()
                .position(|entry| entry.id == id),
            PendingSelectionAnchor::Local(index) => {
                Some(self.pending.accepted_steering().len() + index)
            }
            PendingSelectionAnchor::FollowUp(index) => Some(
                self.pending.accepted_steering().len()
                    + self.pending.steering_prompts().len()
                    + index,
            ),
        };
        if let Some(selected) = selected {
            self.pending.input_panel_mut().selected = selected;
        }
    }

    fn clamp_pending_input_selection(&mut self, count: usize) {
        self.pending.input_panel_mut().selected = self
            .pending
            .input_panel()
            .selected
            .min(count.saturating_sub(1));
    }

    fn recall_latest_pending_input(&mut self) {
        if !self.composer_available_for_pending_edit() {
            self.notify_status("clear the composer before editing pending input");
            return;
        }
        self.select_pending_recall_target();
        self.edit_selected_pending_input();
    }

    fn edit_selected_pending_input(&mut self) {
        if !self.composer_available_for_pending_edit() {
            self.notify_status("clear the composer before editing pending input");
            return;
        }
        let Some(item) = self
            .pending_input_refs()
            .get(self.pending.input_panel().selected)
            .copied()
        else {
            return;
        };
        match item {
            PendingInputRef::AcceptedSteering(index) => {
                let entry = &self.pending.accepted_steering()[index];
                self.pending
                    .set_input_action(Some(PendingInputAction::EditAccepted {
                        id: entry.id.clone(),
                        prompt: entry.prompt.clone(),
                    }));
            }
            PendingInputRef::LocalSteering(index) => {
                if let Some(prompt) = self.pending.steering_prompts_mut().remove(index) {
                    self.restore_pending_prompt(prompt);
                    self.notify_status("editing queued steer");
                }
            }
            PendingInputRef::FollowUp(index) => {
                if let Some(prompt) = self.pending.remove_follow_up(index) {
                    self.restore_pending_prompt(prompt);
                    self.notify_status("editing queued follow-up");
                }
            }
        }
        self.pending.input_panel_mut().focused = false;
        self.pending_input_changed();
    }

    fn discard_selected_pending_input(&mut self) {
        let Some(item) = self
            .pending_input_refs()
            .get(self.pending.input_panel().selected)
            .copied()
        else {
            return;
        };
        match item {
            PendingInputRef::AcceptedSteering(index) => {
                let id = self.pending.accepted_steering()[index].id.clone();
                self.pending
                    .set_input_action(Some(PendingInputAction::DiscardAccepted { id }));
            }
            PendingInputRef::LocalSteering(index) => {
                self.pending.steering_prompts_mut().remove(index);
                self.notify_status("queued steer discarded");
            }
            PendingInputRef::FollowUp(index) => {
                self.pending.remove_follow_up(index);
                self.notify_status("queued follow-up discarded");
            }
        }
        self.pending_input_changed();
    }

    pub(super) fn composer_available_for_pending_edit(&self) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Input)
            && !self.input_ui.has_pending_draft()
            && self.input_ui.paste_segments().is_empty()
    }

    pub(super) fn restore_pending_prompt(&mut self, prompt: QueuedPrompt) {
        self.input_ui.set_shell_mode(None);
        self.input_ui.set_text(prompt.display_prompt);
        self.input_ui.set_paste_segments(prompt.paste_segments);
        for media in prompt.media {
            // Restore the owned model payload; image previews are optional
            // presentation state, so cancellation never needs to retain them.
            self.input_ui.push_ready_attachment(media, None);
        }
        self.input_ui
            .set_submission_mode(InputSubmissionMode::ParseCommands);
        self.input_ui.set_cursor(self.input_char_len());
        self.reset_input_history_navigation();
        self.input_changed();
    }
}

mod render;

#[cfg(test)]
#[path = "pending_input_tests.rs"]
mod tests;
