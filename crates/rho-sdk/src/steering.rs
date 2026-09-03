use std::{collections::BTreeSet, sync::Arc};

use crate::{
    model::{ContentBlock, Message},
    provider_steering::{
        try_retract_claim, ClaimState, ProviderSteeringOutcomes, ProviderSteeringRequest,
        ProviderSteeringSender,
    },
    SteeringId, UserInput,
};

/// Outcome of asking an active run to retract accepted steering input.
///
/// # Next major
///
/// NEXT_MAJOR(rho-sdk): add SteeringRetraction::Delivered for steering already forwarded to the provider mid-turn.
///
/// This minor keeps claimed or delivered input on [`SteeringRetraction::AlreadyApplied`]
/// so hosts matching the current enum remain source-compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteeringRetraction {
    /// The input was still staged and was removed before reaching history.
    Retracted,
    /// The input was already appended to the run's conversation history.
    AlreadyApplied,
    /// The identifier was not accepted by this run.
    NotFound,
}

pub(crate) struct SteeringQueue {
    staged: Vec<StagedSteering>,
    applied: BTreeSet<SteeringId>,
}

struct StagedSteering {
    id: SteeringId,
    message: Message,
    delivery: Delivery,
}

enum Delivery {
    Staged,
    Offered(Arc<std::sync::Mutex<ClaimState>>),
    Delivered,
}

impl SteeringQueue {
    pub(crate) fn new() -> Self {
        Self {
            staged: Vec::new(),
            applied: BTreeSet::new(),
        }
    }

    pub(crate) fn accept(&mut self, input: UserInput) -> SteeringId {
        let id = SteeringId::new();
        self.staged.push(StagedSteering {
            id: id.clone(),
            message: Message::User(input.into_blocks()),
            delivery: Delivery::Staged,
        });
        id
    }

    pub(crate) fn retract(&mut self, id: &SteeringId) -> SteeringRetraction {
        let Some(index) = self.staged.iter().position(|entry| &entry.id == id) else {
            return if self.applied.contains(id) {
                SteeringRetraction::AlreadyApplied
            } else {
                SteeringRetraction::NotFound
            };
        };
        match &self.staged[index].delivery {
            Delivery::Staged => {
                self.staged.remove(index);
                SteeringRetraction::Retracted
            }
            Delivery::Offered(claim) => {
                if try_retract_claim(claim) {
                    self.staged.remove(index);
                    SteeringRetraction::Retracted
                } else {
                    SteeringRetraction::AlreadyApplied
                }
            }
            Delivery::Delivered => SteeringRetraction::AlreadyApplied,
        }
    }

    pub(crate) fn has_staged(&self) -> bool {
        !self.staged.is_empty()
    }

    pub(crate) fn staged_ids(&self) -> Vec<SteeringId> {
        self.staged.iter().map(|entry| entry.id.clone()).collect()
    }

    pub(crate) fn planned_apply_ids(&self) -> Vec<SteeringId> {
        let delivered_only = self.has_delivered();
        self.staged
            .iter()
            .filter(|entry| should_apply(entry, delivered_only))
            .map(|entry| entry.id.clone())
            .collect()
    }

    fn has_delivered(&self) -> bool {
        self.staged
            .iter()
            .any(|entry| matches!(entry.delivery, Delivery::Delivered))
    }

    /// Applies delivered entries when any exist; otherwise applies every staged entry.
    pub(crate) fn apply(&mut self, history: &mut Vec<Message>) -> Vec<SteeringId> {
        let delivered_only = self.has_delivered();
        let mut applied = Vec::new();
        let mut kept = Vec::new();
        for entry in self.staged.drain(..) {
            if should_apply(&entry, delivered_only) {
                applied.push(entry.id.clone());
                self.applied.insert(entry.id);
                history.push(entry.message);
            } else {
                kept.push(entry);
            }
        }
        self.staged = kept;
        applied
    }

    pub(crate) fn offer_unoffered(
        &mut self,
        outcomes: ProviderSteeringOutcomes,
    ) -> Vec<ProviderSteeringRequest> {
        let mut requests = Vec::new();
        for entry in &mut self.staged {
            if !matches!(entry.delivery, Delivery::Staged) {
                continue;
            }
            let claim = Arc::new(std::sync::Mutex::new(ClaimState::Available));
            entry.delivery = Delivery::Offered(Arc::clone(&claim));
            requests.push(ProviderSteeringRequest::new(
                entry.id.clone(),
                user_blocks(&entry.message),
                claim,
                outcomes.clone(),
            ));
        }
        requests
    }

    pub(crate) fn offer_into(
        &mut self,
        tx: &mut Option<ProviderSteeringSender>,
        outcomes: &ProviderSteeringOutcomes,
    ) {
        let Some(sender) = tx else {
            return;
        };
        for request in self.offer_unoffered(outcomes.clone()) {
            if sender.send(request).is_err() {
                *tx = None;
                return;
            }
        }
    }

    pub(crate) fn mark_delivered(&mut self, id: &SteeringId) {
        if let Some(entry) = self.staged.iter_mut().find(|entry| &entry.id == id) {
            entry.delivery = Delivery::Delivered;
        }
    }

    pub(crate) fn mark_released(&mut self, id: &SteeringId) {
        if let Some(entry) = self.staged.iter_mut().find(|entry| &entry.id == id) {
            if matches!(entry.delivery, Delivery::Delivered) {
                return;
            }
            entry.delivery = Delivery::Staged;
        }
    }

    pub(crate) fn reset_delivery(&mut self) {
        for entry in &mut self.staged {
            entry.delivery = Delivery::Staged;
        }
    }
}

fn should_apply(entry: &StagedSteering, delivered_only: bool) -> bool {
    if delivered_only {
        matches!(entry.delivery, Delivery::Delivered)
    } else {
        true
    }
}

fn user_blocks(message: &Message) -> Vec<ContentBlock> {
    match message {
        Message::User(blocks) => blocks.clone(),
        Message::System(_)
        | Message::Assistant(_)
        | Message::EnrichedAssistant(_)
        | Message::AbortedAssistant(_)
        | Message::ToolResult(_) => Vec::new(),
    }
}

#[cfg(test)]
#[path = "steering_tests.rs"]
mod tests;
