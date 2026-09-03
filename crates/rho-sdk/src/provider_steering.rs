use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use tokio::sync::mpsc;

use crate::{model::ContentBlock, SteeringId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimState {
    Available,
    Claimed,
    Retracted,
}

/// One user input the SDK offers to a provider while a turn streams.
pub struct ProviderSteeringRequest {
    id: SteeringId,
    content: Vec<ContentBlock>,
    claim: Arc<Mutex<ClaimState>>,
    outcomes: mpsc::UnboundedSender<(SteeringId, ProviderSteeringOutcome)>,
    settled: AtomicBool,
}

impl ProviderSteeringRequest {
    pub(crate) fn new(
        id: SteeringId,
        content: Vec<ContentBlock>,
        claim: Arc<Mutex<ClaimState>>,
        outcomes: mpsc::UnboundedSender<(SteeringId, ProviderSteeringOutcome)>,
    ) -> Self {
        Self {
            id,
            content,
            claim,
            outcomes,
            settled: AtomicBool::new(false),
        }
    }

    /// Builds an unclaimed request for tests and provider harnesses.
    #[doc(hidden)]
    pub fn test_unclaimed(
        content: Vec<ContentBlock>,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<(SteeringId, ProviderSteeringOutcome)>,
    ) {
        let (outcomes, receiver) = mpsc::unbounded_channel();
        (
            Self::new(
                SteeringId::new(),
                content,
                Arc::new(Mutex::new(ClaimState::Available)),
                outcomes,
            ),
            receiver,
        )
    }

    pub fn id(&self) -> &SteeringId {
        &self.id
    }

    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    /// Atomically claims the request for sending; `false` if the host retracted it first.
    pub fn claim(&self) -> bool {
        let mut state = self
            .claim
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *state == ClaimState::Available {
            *state = ClaimState::Claimed;
            true
        } else {
            false
        }
    }

    fn ensure_claimed(&self) -> bool {
        let mut state = self
            .claim
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            ClaimState::Available => {
                *state = ClaimState::Claimed;
                true
            }
            ClaimState::Claimed => true,
            ClaimState::Retracted => false,
        }
    }

    /// Backend acknowledged it will apply the input inside this turn.
    pub fn accept(self) {
        if self.ensure_claimed() {
            self.settle(ProviderSteeringOutcome::Accepted);
        } else {
            self.settle(ProviderSteeringOutcome::Released);
        }
    }

    /// Provider will not deliver it; the SDK applies it at the turn boundary.
    ///
    /// [`Drop`] does the same.
    pub fn release(self) {
        self.settle(ProviderSteeringOutcome::Released);
    }

    fn settle(&self, outcome: ProviderSteeringOutcome) {
        if self.settled.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.outcomes.send((self.id.clone(), outcome));
    }
}

impl Drop for ProviderSteeringRequest {
    fn drop(&mut self) {
        self.settle(ProviderSteeringOutcome::Released);
    }
}

/// Receiving end; dropping it releases every outstanding and future request.
pub struct ProviderSteeringReceiver {
    rx: mpsc::UnboundedReceiver<ProviderSteeringRequest>,
}

impl ProviderSteeringReceiver {
    pub async fn recv(&mut self) -> Option<ProviderSteeringRequest> {
        self.rx.recv().await
    }
}

impl Drop for ProviderSteeringReceiver {
    fn drop(&mut self) {
        self.rx.close();
        while let Ok(request) = self.rx.try_recv() {
            request.settle(ProviderSteeringOutcome::Released);
        }
    }
}

/// Outcome reported after a provider claims or declines a steering request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderSteeringOutcome {
    Accepted,
    Released,
}

pub(crate) type ProviderSteeringSender = mpsc::UnboundedSender<ProviderSteeringRequest>;
pub(crate) type ProviderSteeringOutcomes =
    mpsc::UnboundedSender<(SteeringId, ProviderSteeringOutcome)>;

/// Creates a mid-turn steering port.
///
/// The constructor is public so provider crates can inject requests in tests.
#[doc(hidden)]
pub fn provider_steering_channel() -> (
    mpsc::UnboundedSender<ProviderSteeringRequest>,
    ProviderSteeringReceiver,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, ProviderSteeringReceiver { rx })
}

pub(crate) fn try_retract_claim(claim: &Arc<Mutex<ClaimState>>) -> bool {
    let mut state = claim
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *state == ClaimState::Available {
        *state = ClaimState::Retracted;
        true
    } else {
        false
    }
}
