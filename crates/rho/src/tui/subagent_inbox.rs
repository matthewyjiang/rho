//! Delegated-child traffic the parent has accepted but not yet delivered.

use std::collections::VecDeque;

use rho_sdk::SessionId;
use tokio::sync::mpsc::Receiver;

use crate::app::{
    subagent_host_input::SubagentHostInputRequest,
    subagent_messaging::{NoticePermits, NoticeRebind, SubagentNotice},
};

#[cfg(test)]
use crate::app::subagent_messaging::SubagentNoticeBridge;

/// Every channel a delegated child can push at its parent session.
///
/// Questionnaires and notices keep separate channels, so a chatty child cannot
/// head-of-line block a question that is holding up its own run. They share one
/// receive point, one drain, and one stale-session purge, so an event loop
/// takes a single arm for all child traffic and a new channel does not have to
/// be threaded through every loop again.
#[derive(Default)]
pub(super) struct SubagentInbox {
    questionnaires: Option<Receiver<SubagentHostInputRequest>>,
    notices: Option<Receiver<SubagentNotice>>,
    /// Permit generation installed with the live notice receiver. New arrivals
    /// from that receiver are tagged with this handle.
    notice_permits: Option<NoticePermits>,
    queued_questionnaires: VecDeque<SubagentHostInputRequest>,
    queued_notices: VecDeque<QueuedNotice>,
    /// Permit handles for notices taken for turn-boundary delivery, in the same
    /// order as the returned `Vec`. Committed on successful provider start or
    /// re-attached by [`Self::return_notices`].
    pending_delivery_permits: Vec<Option<NoticePermits>>,
}

/// One accepted notice plus the generation that owns its end-to-end budget slot.
struct QueuedNotice {
    notice: SubagentNotice,
    /// Generation that accepted this notice. Released on deliver or discard.
    /// `None` only for test inserts that never reserved a slot.
    permits: Option<NoticePermits>,
}

/// What [`SubagentInbox::recv`] observed, resolved before any queue is touched.
enum Incoming {
    Questionnaire(SubagentHostInputRequest),
    Notice(SubagentNotice),
    QuestionnairesClosed,
    NoticesClosed,
}

impl SubagentInbox {
    /// Binds both channels to the delegated-run manager.
    pub(super) fn bind(&mut self, manager: &crate::app::subagent_manager::SubagentManager) {
        self.questionnaires = Some(manager.bind_host_input());
        let rebind = manager.rebind_notices(self.notices.take());
        self.install_notice_rebind(rebind);
    }

    /// Waits for the next child message on any bound channel and queues it.
    ///
    /// Cancel-safe: every branch is a `Receiver::recv`, an unbound channel
    /// parks instead of needing a guard at the call site, and no queue is
    /// touched until after the inner select resolves.
    pub(super) async fn recv(&mut self) {
        let incoming = {
            let Self {
                questionnaires,
                notices,
                ..
            } = self;
            tokio::select! {
                request = recv_next(questionnaires) => match request {
                    Some(request) => Incoming::Questionnaire(request),
                    None => Incoming::QuestionnairesClosed,
                },
                notice = recv_next(notices) => match notice {
                    Some(notice) => Incoming::Notice(notice),
                    None => Incoming::NoticesClosed,
                },
            }
        };
        match incoming {
            Incoming::Questionnaire(request) => self.queued_questionnaires.push_back(request),
            Incoming::Notice(notice) => {
                self.push_accepted_notice(notice, self.notice_permits.clone())
            }
            Incoming::QuestionnairesClosed => self.questionnaires = None,
            Incoming::NoticesClosed => self.notices = None,
        }
    }

    /// Moves everything already delivered on either channel into the queues.
    pub(super) fn drain(&mut self) -> bool {
        let mut changed = false;
        if let Some(receiver) = self.questionnaires.as_mut() {
            while let Ok(request) = receiver.try_recv() {
                self.queued_questionnaires.push_back(request);
                changed = true;
            }
        }
        let mut drained_notices = Vec::new();
        if let Some(receiver) = self.notices.as_mut() {
            while let Ok(notice) = receiver.try_recv() {
                drained_notices.push(notice);
            }
        }
        if !drained_notices.is_empty() {
            let current_permits = self.notice_permits.clone();
            for notice in drained_notices {
                self.push_accepted_notice(notice, current_permits.clone());
            }
            changed = true;
        }
        changed
    }

    /// Drops queued traffic that the current parent session can no longer take.
    ///
    /// A questionnaire is answered with an error so the child stops waiting; a
    /// notice has no waiting child, so it is dropped. Neither can be delivered
    /// once the parent has moved to another session, and keeping them would
    /// leave the parent permanently looking busy.
    pub(super) fn discard_stale(&mut self, session_id: &SessionId) -> bool {
        let mut changed = false;
        for pending in std::mem::take(&mut self.queued_questionnaires) {
            if pending.response.is_closed() {
                changed = true;
                continue;
            }
            if &pending.parent_session_id != session_id {
                let _ = pending.response.send(Err(rho_sdk::Error::Interrupted {
                    message: "parent session changed before the delegated questionnaire was shown"
                        .into(),
                }));
                changed = true;
                continue;
            }
            self.queued_questionnaires.push_back(pending);
        }
        let mut kept = VecDeque::new();
        for queued in self.queued_notices.drain(..) {
            if &queued.notice.parent_session_id == session_id {
                kept.push_back(queued);
            } else {
                release_one(queued.permits);
                changed = true;
            }
        }
        self.queued_notices = kept;
        changed
    }

    /// Takes queued notices for this parent session in arrival order.
    ///
    /// Notices addressed to a session the parent has left can never be
    /// delivered, so they are dropped rather than queued forever. Permit
    /// handles for kept notices stay with the inbox until
    /// [`Self::commit_delivered_notices`] or [`Self::return_notices`].
    pub(super) fn take_notices(&mut self, session_id: &SessionId) -> Vec<SubagentNotice> {
        debug_assert!(
            self.pending_delivery_permits.is_empty(),
            "previous take_notices was neither committed nor returned"
        );
        self.pending_delivery_permits.clear();
        let mut kept = Vec::new();
        for queued in self.queued_notices.drain(..) {
            if &queued.notice.parent_session_id == session_id {
                self.pending_delivery_permits.push(queued.permits);
                kept.push(queued.notice);
            } else {
                release_one(queued.permits);
            }
        }
        kept
    }

    /// Returns drained notices to the front of the queue so a failed turn
    /// setup preserves arrival order ahead of any newer arrivals.
    pub(super) fn return_notices(&mut self, notices: impl IntoIterator<Item = SubagentNotice>) {
        let notices = notices.into_iter().collect::<Vec<_>>();
        let mut permits = std::mem::take(&mut self.pending_delivery_permits);
        // Test helpers may return notices that never went through take_notices.
        if permits.len() != notices.len() {
            permits.resize_with(notices.len(), || None);
        }
        for (notice, permits) in notices.into_iter().zip(permits).rev() {
            self.queued_notices
                .push_front(QueuedNotice { notice, permits });
        }
    }

    /// Frees end-to-end notice budget after the parent delivered notices to the
    /// model. Restored batches must not call this.
    pub(super) fn commit_delivered_notices(&mut self, count: usize) {
        debug_assert_eq!(
            self.pending_delivery_permits.len(),
            count,
            "commit count must match the last take_notices batch"
        );
        let _ = count;
        for permits in self.pending_delivery_permits.drain(..) {
            release_one(permits);
        }
    }

    /// Hands the queued questionnaires to a turn's own interaction queue.
    pub(super) fn take_questionnaires(
        &mut self,
    ) -> impl Iterator<Item = SubagentHostInputRequest> + '_ {
        self.queued_questionnaires.drain(..)
    }

    /// Returns the questionnaires a turn parked back after it finished.
    pub(super) fn return_questionnaires(
        &mut self,
        requests: impl IntoIterator<Item = SubagentHostInputRequest>,
    ) {
        self.queued_questionnaires.extend(requests);
    }

    /// Next questionnaire to present, or `None` when the queue is empty.
    pub(super) fn next_questionnaire(&mut self) -> Option<SubagentHostInputRequest> {
        self.queued_questionnaires.pop_front()
    }

    pub(super) fn has_queued_questionnaires(&self) -> bool {
        !self.queued_questionnaires.is_empty()
    }

    #[cfg(test)]
    pub(super) fn has_pending_notices(&self) -> bool {
        !self.queued_notices.is_empty()
    }

    pub(super) fn has_parent_action_requests(&self) -> bool {
        self.queued_notices
            .iter()
            .any(|queued| match queued.notice.delivery {
                crate::app::subagent_messaging::NoticeDelivery::NextTurn => false,
                crate::app::subagent_messaging::NoticeDelivery::ParentActionRequired => true,
            })
    }

    #[cfg(test)]
    pub(super) fn push_notice_for_test(&mut self, notice: SubagentNotice) {
        self.push_accepted_notice(notice, /*permits*/ None);
    }

    #[cfg(test)]
    pub(super) fn bind_notices_for_test(&mut self, bridge: &SubagentNoticeBridge) {
        let rebind = bridge.rebind_parent(self.notices.take());
        self.install_notice_rebind(rebind);
    }

    #[cfg(any(test, debug_assertions))]
    pub(super) fn queued_notice_count(&self) -> usize {
        self.queued_notices.len()
    }

    #[cfg(test)]
    pub(super) fn push_questionnaire_for_test(&mut self, request: SubagentHostInputRequest) {
        self.queued_questionnaires.push_back(request);
    }

    #[cfg(test)]
    pub(super) fn queued_questionnaire_count(&self) -> usize {
        self.queued_questionnaires.len()
    }

    fn install_notice_rebind(&mut self, rebind: NoticeRebind) {
        // Retained channel notices keep the retired generation. Already-queued
        // notices already carry their own handles and must not be retagged.
        for notice in rebind.retained {
            self.push_accepted_notice(notice, rebind.retired_permits.clone());
        }
        self.notices = Some(rebind.receiver);
        self.notice_permits = Some(rebind.permits);
    }

    fn push_accepted_notice(&mut self, notice: SubagentNotice, permits: Option<NoticePermits>) {
        self.queued_notices
            .push_back(QueuedNotice { notice, permits });
    }
}

fn release_one(permits: Option<NoticePermits>) {
    if let Some(permits) = permits {
        permits.release(1);
    }
}

/// Awaits a channel that may not be bound. An unbound channel never resolves,
/// which disables its `select!` branch without a caller-side guard.
async fn recv_next<T>(slot: &mut Option<Receiver<T>>) -> Option<T> {
    match slot {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "subagent_inbox_tests.rs"]
mod tests;
