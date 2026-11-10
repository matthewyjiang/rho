//! A started session's turn in the daemon-wide agent budget (#1714).
//!
//! The turns of a session another session started share
//! `behavior.agent_concurrency` with subagent runs: each waits `queued` for
//! a slot before it runs. Unlike a subagent run, such a turn can wait on
//! others: on a person's answer, or on the sessions and subagents it
//! started, which may need a slot from the same budget. So it holds its slot
//! only while it runs alone (`JobRegistry::runs_alone`): `running`, with
//! nothing it started under way. Otherwise it lends the slot back to the
//! budget, and once it runs alone again it takes a slot again, in line, and
//! works on meanwhile. A slot is therefore only ever held by a turn that
//! waits on no other job's slot, and the budget cannot deadlock on itself.
//!
//! A turn that lent its slot is not paused: nothing gates its next model
//! request on a slot. So the budget bounds turns that run alone, not every
//! running agent: each started session with work of its own under way can
//! run one agent beyond it, as many as session control's depth and session
//! caps let agents start.

use std::{convert::Infallible, future::Future, pin::Pin};

use futures_util::FutureExt;
use rho_sdk::CancellationToken;

use super::{
    super::{jobs::JobRegistry, subagents::TurnBudget},
    turn::DaemonView,
};
use crate::app::{
    agent_binding::CapacityClass,
    agent_concurrency::{AgentConcurrency, RuntimePermits},
};

/// A turn cancelled, or stopped by the daemon's stop, while it waited for
/// its first slot.
pub(super) struct Withdrawn;

/// What a turn runs with in the budget.
pub(super) enum Slot {
    /// Nothing: a person's session's turn.
    Unbudgeted,
    /// The slot it was admitted with, and the budget to take one again.
    Budgeted {
        permits: RuntimePermits,
        concurrency: AgentConcurrency,
    },
}

/// Waits for the turn's first slot, its job `queued` until it has one and
/// then `running`; at once without a `budget`. The request that sent the
/// prompt learns the turn began as soon as it must wait, so it does not wait
/// for capacity; a slot free at once, or a cancel already sent, leaves that
/// to the turn, as for a person's session.
pub(super) async fn admit(
    view: &mut DaemonView,
    budget: Option<TurnBudget>,
    cancel: &CancellationToken,
    stopping: &CancellationToken,
) -> Result<Slot, Withdrawn> {
    let Some(TurnBudget {
        queued,
        concurrency,
        listing: _listing,
    }) = budget
    else {
        return Ok(Slot::Unbudgeted);
    };
    let acquire = queued.acquire(cancel);
    tokio::pin!(acquire);
    let permits = match acquire.as_mut().now_or_never() {
        Some(permits) => permits,
        None => {
            view.began();
            tokio::select! {
                biased;
                () = stopping.cancelled() => None,
                permits = &mut acquire => permits,
            }
        }
    }
    .ok_or(Withdrawn)?;
    view.session.admitted();
    Ok(Slot::Budgeted {
        permits,
        concurrency,
    })
}

impl Slot {
    /// Runs `turn`, the turn of `job`, lending the slot whenever the turn
    /// does not run alone and taking one again when it does.
    pub(super) async fn lend_while_waiting<T>(
        self,
        jobs: &JobRegistry,
        job: &super::super::jobs::JobId,
        turn: impl Future<Output = T>,
    ) -> T {
        match self {
            Self::Unbudgeted => turn.await,
            Self::Budgeted {
                permits,
                concurrency,
            } => {
                tokio::select! {
                    biased;
                    ended = turn => ended,
                    never = keep(permits, &concurrency, jobs, job) => match never {},
                }
            }
        }
    }
}

/// A wait for a slot taken again, which leaves the line when dropped.
type Retaking = Pin<Box<dyn Future<Output = Option<RuntimePermits>> + Send>>;

/// Holds `permits` while `job` runs alone, lends them while it does not,
/// and takes a slot again once it does. Runs until the turn drops it.
async fn keep(
    permits: RuntimePermits,
    concurrency: &AgentConcurrency,
    jobs: &JobRegistry,
    job: &super::super::jobs::JobId,
) -> Infallible {
    let mut held = Some(permits);
    let mut retaking: Option<Retaking> = None;
    loop {
        let changed = jobs.changed();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if !jobs.runs_alone(job) {
            if held.take().is_some() {
                tracing::debug!(%job, "lent a started session's slot while it waits");
            }
            retaking = None;
        } else if held.is_none() && retaking.is_none() {
            let queued = concurrency.join(CapacityClass::Rho);
            retaking = Some(Box::pin(async move {
                // Only dropping this wait ends it.
                queued.acquire(&CancellationToken::new()).await
            }));
        }
        match &mut retaking {
            Some(acquire) => tokio::select! {
                permits = acquire => {
                    held = permits;
                    retaking = None;
                }
                () = &mut changed => {}
            },
            None => changed.await,
        }
    }
}
