use std::collections::BTreeSet;

use crate::{model::Message, SteeringId, UserInput};

/// Outcome of asking an active run to retract accepted steering input.
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
        self.staged.remove(index);
        SteeringRetraction::Retracted
    }

    pub(crate) fn has_staged(&self) -> bool {
        !self.staged.is_empty()
    }

    pub(crate) fn staged_ids(&self) -> Vec<SteeringId> {
        self.staged.iter().map(|entry| entry.id.clone()).collect()
    }

    pub(crate) fn apply(&mut self, history: &mut Vec<Message>) {
        for entry in self.staged.drain(..) {
            self.applied.insert(entry.id);
            history.push(entry.message);
        }
    }
}
