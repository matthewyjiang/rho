//! Queued prompts, steering, and the pending-input panel.

use std::collections::VecDeque;

use crate::tui::{
    pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel},
    QueuedPrompt,
};

/// Queued prompts, steering, and the pending-input panel.
#[derive(Default)]
pub(in crate::tui) struct PendingWorkUi {
    steering_prompts: VecDeque<QueuedPrompt>,
    accepted_steering: VecDeque<AcceptedSteering>,
    retracting_steering: Option<rho_sdk::SteeringId>,
    input_panel: PendingInputPanel,
    input_action: Option<PendingInputAction>,
    queued_prompts: VecDeque<QueuedPrompt>,
}

impl PendingWorkUi {
    pub(in crate::tui) fn follow_up_len(&self) -> usize {
        self.queued_prompts.len()
    }

    pub(in crate::tui) fn has_follow_ups(&self) -> bool {
        !self.queued_prompts.is_empty()
    }

    /// Fold unapplied accepted and local steering into follow-up queue.
    pub(in crate::tui) fn preserve_unapplied_steering_as_follow_ups(&mut self) {
        let mut pending = self
            .accepted_steering
            .drain(..)
            .map(|entry| entry.prompt)
            .chain(self.steering_prompts.drain(..))
            .collect::<VecDeque<_>>();
        pending.append(&mut self.queued_prompts);
        self.queued_prompts = pending;
        self.retracting_steering = None;
        self.input_action = None;
    }

    pub(in crate::tui) fn steering_prompts(&self) -> &VecDeque<QueuedPrompt> {
        &self.steering_prompts
    }

    pub(in crate::tui) fn steering_prompts_mut(&mut self) -> &mut VecDeque<QueuedPrompt> {
        &mut self.steering_prompts
    }

    pub(in crate::tui) fn accepted_steering(&self) -> &VecDeque<AcceptedSteering> {
        &self.accepted_steering
    }

    pub(in crate::tui) fn accepted_steering_mut(&mut self) -> &mut VecDeque<AcceptedSteering> {
        &mut self.accepted_steering
    }

    pub(in crate::tui) fn queued_prompts(&self) -> &VecDeque<QueuedPrompt> {
        &self.queued_prompts
    }

    pub(in crate::tui) fn push_follow_up(&mut self, prompt: QueuedPrompt) {
        self.queued_prompts.push_back(prompt);
    }

    pub(in crate::tui) fn push_follow_up_front(&mut self, prompt: QueuedPrompt) {
        self.queued_prompts.push_front(prompt);
    }

    pub(in crate::tui) fn pop_follow_up(&mut self) -> Option<QueuedPrompt> {
        self.queued_prompts.pop_front()
    }

    pub(in crate::tui) fn remove_follow_up(&mut self, index: usize) -> Option<QueuedPrompt> {
        if index < self.queued_prompts.len() {
            self.queued_prompts.remove(index)
        } else {
            None
        }
    }

    pub(in crate::tui) fn clear_follow_ups(&mut self) {
        self.queued_prompts.clear();
    }

    pub(in crate::tui) fn clear_steering(&mut self) {
        self.steering_prompts.clear();
        self.accepted_steering.clear();
        self.retracting_steering = None;
    }

    pub(in crate::tui) fn retracting_steering(&self) -> Option<&rho_sdk::SteeringId> {
        self.retracting_steering.as_ref()
    }

    pub(in crate::tui) fn set_retracting_steering(&mut self, id: Option<rho_sdk::SteeringId>) {
        self.retracting_steering = id;
    }

    pub(in crate::tui) fn input_panel(&self) -> &PendingInputPanel {
        &self.input_panel
    }

    pub(in crate::tui) fn input_panel_mut(&mut self) -> &mut PendingInputPanel {
        &mut self.input_panel
    }

    pub(in crate::tui) fn input_action(&self) -> Option<&PendingInputAction> {
        self.input_action.as_ref()
    }

    pub(in crate::tui) fn set_input_action(&mut self, action: Option<PendingInputAction>) {
        self.input_action = action;
    }

    pub(in crate::tui) fn take_input_action(&mut self) -> Option<PendingInputAction> {
        self.input_action.take()
    }

    pub(in crate::tui) fn clear_input_action(&mut self) {
        self.input_action = None;
    }

    pub(in crate::tui) fn drain_accepted_steering_prompts(
        &mut self,
    ) -> impl Iterator<Item = String> + '_ {
        self.accepted_steering
            .drain(..)
            .map(|entry| entry.prompt.prompt)
    }

    pub(in crate::tui) fn drain_steering_prompt_texts(
        &mut self,
    ) -> impl Iterator<Item = String> + '_ {
        self.steering_prompts.drain(..).map(|prompt| prompt.prompt)
    }

    pub(in crate::tui) fn drain_queued_prompt_texts(
        &mut self,
    ) -> impl Iterator<Item = String> + '_ {
        self.queued_prompts.drain(..).map(|prompt| prompt.prompt)
    }
}
