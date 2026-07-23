use super::{App, ComposerMode, InputSubmissionMode, InteractiveRuntime, QueuedPrompt};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug)]
pub(super) struct AcceptedSteering {
    pub(super) id: rho_sdk::SteeringId,
    pub(super) prompt: QueuedPrompt,
}

pub(super) enum PendingInputRequest {
    Accept {
        prompt: QueuedPrompt,
        receipt: crate::app::interactive_runtime::SteeringAcceptanceFuture,
    },
    Retract {
        action: PendingInputAction,
        receipt: crate::app::interactive_runtime::SteeringRetractionFuture,
    },
}

pub(super) enum PendingInputCompletion {
    Accepted(Result<rho_sdk::SteeringId, rho_sdk::Error>),
    Retracted(Result<rho_sdk::SteeringRetraction, rho_sdk::Error>),
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
        self.pending.input_panel.focused
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
                self.pending.input_panel.focused = !self.pending.input_panel.focused;
                if self.pending.input_panel.focused {
                    self.select_pending_recall_target();
                }
            }
            self.ctrl_c_streak = 0;
            return true;
        }

        if !self.pending.input_panel.focused {
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
                self.pending.input_panel.focused = false;
            }
            (_, KeyCode::Up) => {
                self.pending.input_panel.selected =
                    self.pending.input_panel.selected.saturating_sub(1);
            }
            (_, KeyCode::Down) => {
                self.pending.input_panel.selected =
                    (self.pending.input_panel.selected + 1).min(count.saturating_sub(1));
            }
            (_, KeyCode::Home) => self.pending.input_panel.selected = 0,
            (_, KeyCode::End) => self.pending.input_panel.selected = count.saturating_sub(1),
            (_, KeyCode::Enter) => self.edit_selected_pending_input(),
            (_, KeyCode::Backspace | KeyCode::Delete) => {
                self.discard_selected_pending_input();
            }
            (modifiers, KeyCode::Char(_))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.pending.input_panel.focused = false;
                return false;
            }
            _ => {}
        }
        self.ctrl_c_streak = 0;
        true
    }

    pub(super) fn start_pending_input_request(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> Option<PendingInputRequest> {
        if let Some(action) = self.pending.input_action.take() {
            let id = match &action {
                PendingInputAction::EditAccepted { id, .. }
                | PendingInputAction::DiscardAccepted { id } => id.clone(),
            };
            return match agent.request_steering_retraction(id.clone()) {
                Ok(receipt) => {
                    self.pending.retracting_steering = Some(id);
                    Some(PendingInputRequest::Retract { action, receipt })
                }
                Err(error) => {
                    self.notify_status(format!("could not request steer retraction: {error}"));
                    None
                }
            };
        }

        let prompt = self.pending.steering_prompts.pop_front()?;
        match agent.request_steer(rho_sdk::UserInput::text(prompt.prompt.clone())) {
            Ok(receipt) => Some(PendingInputRequest::Accept { prompt, receipt }),
            Err(error) => {
                self.pending.steering_prompts.push_front(prompt);
                self.notify_status(format!("could not submit steer: {error}"));
                None
            }
        }
    }

    pub(super) fn finish_pending_input_request(
        &mut self,
        request: PendingInputRequest,
        completion: PendingInputCompletion,
    ) -> Option<String> {
        match (request, completion) {
            (
                PendingInputRequest::Accept { prompt, .. },
                PendingInputCompletion::Accepted(Ok(id)),
            ) => {
                self.pending
                    .accepted_steering
                    .push_back(AcceptedSteering { id, prompt });
                self.select_pending_recall_target();
                None
            }
            (
                PendingInputRequest::Accept { prompt, .. },
                PendingInputCompletion::Accepted(Err(error)),
            ) => {
                self.pending.queued_prompts.push_front(prompt);
                self.select_pending_recall_target();
                self.notify_status(format!("steer queued as follow-up: {error}"));
                None
            }
            (
                PendingInputRequest::Retract { action, .. },
                PendingInputCompletion::Retracted(result),
            ) => {
                self.pending.retracting_steering = None;
                self.finish_steering_retraction(action, result);
                None
            }
            _ => Some("pending-input request completed with the wrong response type".into()),
        }
    }

    fn finish_steering_retraction(
        &mut self,
        action: PendingInputAction,
        result: Result<rho_sdk::SteeringRetraction, rho_sdk::Error>,
    ) {
        let (id, edit_prompt) = match action {
            PendingInputAction::EditAccepted { id, prompt } => (id, Some(prompt)),
            PendingInputAction::DiscardAccepted { id } => (id, None),
        };
        match result {
            Ok(rho_sdk::SteeringRetraction::Retracted) => {
                self.remove_accepted_steering(&id);
                if let Some(prompt) = edit_prompt {
                    self.restore_pending_prompt(prompt);
                    self.notify_status("editing retracted steer");
                } else {
                    self.notify_status("steer discarded");
                }
            }
            Ok(rho_sdk::SteeringRetraction::AlreadyApplied) => {
                self.remove_accepted_steering(&id);
                self.notify_status("steer was already applied and can no longer be changed");
            }
            Ok(rho_sdk::SteeringRetraction::NotFound) => {
                self.remove_accepted_steering(&id);
                self.notify_status("steer is no longer pending");
            }
            Err(error) => {
                self.notify_status(format!(
                    "could not confirm steer retraction; it remains pending: {error}"
                ));
            }
        }
    }

    pub(super) fn mark_steering_applied(&mut self, ids: &[rho_sdk::SteeringId]) {
        let selection = self.pending_selection_anchor();
        self.pending
            .accepted_steering
            .retain(|entry| !ids.contains(&entry.id));
        self.restore_pending_selection(selection);
        self.pending_input_changed();
    }

    pub(super) fn clear_accepted_steering(&mut self) {
        self.pending.accepted_steering.clear();
        self.pending.retracting_steering = None;
        self.pending.input_action = None;
        self.pending_input_changed();
    }

    pub(super) fn select_pending_recall_target(&mut self) {
        let accepted = self.pending.accepted_steering.len();
        let local = self.pending.steering_prompts.len();
        let follow_up = self.pending.queued_prompts.len();
        if accepted + local + follow_up == 0 {
            self.pending.input_panel.focused = false;
            self.pending.input_panel.selected = 0;
            return;
        }
        self.pending.input_panel.selected = if local > 0 {
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
            self.pending.input_panel.focused = false;
        }
    }

    fn pending_input_count(&self) -> usize {
        self.pending.accepted_steering.len()
            + self.pending.steering_prompts.len()
            + self.pending.queued_prompts.len()
    }

    fn pending_input_refs(&self) -> Vec<PendingInputRef> {
        (0..self.pending.accepted_steering.len())
            .map(PendingInputRef::AcceptedSteering)
            .chain((0..self.pending.steering_prompts.len()).map(PendingInputRef::LocalSteering))
            .chain((0..self.pending.queued_prompts.len()).map(PendingInputRef::FollowUp))
            .collect()
    }

    fn pending_selection_anchor(&self) -> Option<PendingSelectionAnchor> {
        match self
            .pending_input_refs()
            .get(self.pending.input_panel.selected)?
        {
            PendingInputRef::AcceptedSteering(index) => self
                .pending
                .accepted_steering
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
                .accepted_steering
                .iter()
                .position(|entry| entry.id == id),
            PendingSelectionAnchor::Local(index) => {
                Some(self.pending.accepted_steering.len() + index)
            }
            PendingSelectionAnchor::FollowUp(index) => Some(
                self.pending.accepted_steering.len() + self.pending.steering_prompts.len() + index,
            ),
        };
        if let Some(selected) = selected {
            self.pending.input_panel.selected = selected;
        }
    }

    fn clamp_pending_input_selection(&mut self, count: usize) {
        self.pending.input_panel.selected = self
            .pending
            .input_panel
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
            .get(self.pending.input_panel.selected)
            .copied()
        else {
            return;
        };
        match item {
            PendingInputRef::AcceptedSteering(index) => {
                let entry = &self.pending.accepted_steering[index];
                if self.pending.retracting_steering.as_ref() == Some(&entry.id) {
                    self.notify_status("steer retraction is already in progress");
                    return;
                }
                self.pending.input_action = Some(PendingInputAction::EditAccepted {
                    id: entry.id.clone(),
                    prompt: entry.prompt.clone(),
                });
            }
            PendingInputRef::LocalSteering(index) => {
                if let Some(prompt) = self.pending.steering_prompts.remove(index) {
                    self.restore_pending_prompt(prompt);
                    self.notify_status("editing queued steer");
                }
            }
            PendingInputRef::FollowUp(index) => {
                if let Some(prompt) = self.pending.queued_prompts.remove(index) {
                    self.restore_pending_prompt(prompt);
                    self.notify_status("editing queued follow-up");
                }
            }
        }
        self.pending.input_panel.focused = false;
        self.pending_input_changed();
    }

    fn discard_selected_pending_input(&mut self) {
        let Some(item) = self
            .pending_input_refs()
            .get(self.pending.input_panel.selected)
            .copied()
        else {
            return;
        };
        match item {
            PendingInputRef::AcceptedSteering(index) => {
                let id = self.pending.accepted_steering[index].id.clone();
                if self.pending.retracting_steering.as_ref() == Some(&id) {
                    self.notify_status("steer retraction is already in progress");
                    return;
                }
                self.pending.input_action = Some(PendingInputAction::DiscardAccepted { id });
            }
            PendingInputRef::LocalSteering(index) => {
                self.pending.steering_prompts.remove(index);
                self.notify_status("queued steer discarded");
            }
            PendingInputRef::FollowUp(index) => {
                self.pending.queued_prompts.remove(index);
                self.notify_status("queued follow-up discarded");
            }
        }
        self.pending_input_changed();
    }

    fn composer_available_for_pending_edit(&self) -> bool {
        matches!(self.input_ui.composer, ComposerMode::Input)
            && self.input_ui.text.is_empty()
            && self.input_ui.paste_segments.is_empty()
            && self.input_ui.pending_images.is_empty()
            && self.input_ui.shell_mode.is_none()
    }

    pub(super) fn restore_pending_prompt(&mut self, prompt: QueuedPrompt) {
        self.input_ui.shell_mode = None;
        self.input_ui.text = prompt.display_prompt;
        self.input_ui.paste_segments = prompt.paste_segments;
        self.input_ui.submission_mode = InputSubmissionMode::ParseCommands;
        self.input_ui.cursor = self.input_char_len();
        self.reset_input_history_navigation();
        self.input_changed();
    }

    fn remove_accepted_steering(&mut self, id: &rho_sdk::SteeringId) {
        self.pending
            .accepted_steering
            .retain(|entry| &entry.id != id);
        self.pending_input_changed();
    }
}

pub(super) async fn pending_input_completion(
    request: &mut Option<PendingInputRequest>,
) -> Option<PendingInputCompletion> {
    match request.as_mut()? {
        PendingInputRequest::Accept { receipt, .. } => {
            Some(PendingInputCompletion::Accepted(receipt.as_mut().await))
        }
        PendingInputRequest::Retract { receipt, .. } => {
            Some(PendingInputCompletion::Retracted(receipt.as_mut().await))
        }
    }
}

mod render;

#[cfg(test)]
#[path = "pending_input_tests.rs"]
mod tests;
