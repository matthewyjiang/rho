//! Queued prompts, steering, and the pending-input panel.

use std::collections::VecDeque;

use crate::tui::{
    pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel},
    send_confirm::SendOmissions,
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
    /// The follow-up a daemon session compacts before, and the consent to
    /// omit provider-native context given for it (`send_confirm::daemon`).
    /// The consent goes with that prompt alone: taking the prompt off the
    /// queue hands it over or drops it.
    compact_send: Option<(QueuedPrompt, SendOmissions)>,
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
        self.pop_follow_up_with_consent().map(|(prompt, _)| prompt)
    }

    /// The next follow-up, with the consent given for it when it is the
    /// prompt [`Self::park_compact_send`] queued, unedited.
    pub(in crate::tui) fn pop_follow_up_with_consent(
        &mut self,
    ) -> Option<(QueuedPrompt, Option<SendOmissions>)> {
        let prompt = self.queued_prompts.pop_front()?;
        let consent = self
            .compact_send
            .take_if(|(parked, _)| *parked == prompt)
            .map(|(_, consent)| consent);
        self.drop_consent_of_a_prompt_no_longer_queued();
        Some((prompt, consent))
    }

    /// Queues `prompt` first, to send under `consent` once the compaction
    /// the person asked for before it is over.
    pub(in crate::tui) fn park_compact_send(
        &mut self,
        prompt: QueuedPrompt,
        consent: SendOmissions,
    ) {
        self.queued_prompts.push_front(prompt.clone());
        self.compact_send = Some((prompt, consent));
    }

    pub(in crate::tui) fn remove_follow_up(&mut self, index: usize) -> Option<QueuedPrompt> {
        let removed = if index < self.queued_prompts.len() {
            self.queued_prompts.remove(index)
        } else {
            None
        };
        self.drop_consent_of_a_prompt_no_longer_queued();
        removed
    }

    fn drop_consent_of_a_prompt_no_longer_queued(&mut self) {
        if let Some((parked, _)) = &self.compact_send {
            if !self.queued_prompts.contains(parked) {
                self.compact_send = None;
            }
        }
    }

    pub(in crate::tui) fn clear_follow_ups(&mut self) {
        self.queued_prompts.clear();
        self.compact_send = None;
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
        self.compact_send = None;
        self.queued_prompts.drain(..).map(|prompt| prompt.prompt)
    }
}
