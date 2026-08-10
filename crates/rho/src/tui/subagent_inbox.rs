//! Delegated-child traffic the parent has accepted but not yet delivered.

use std::collections::VecDeque;

use rho_sdk::SessionId;
use tokio::sync::mpsc::Receiver;

use crate::app::{
    subagent_host_input::SubagentHostInputRequest, subagent_messaging::SubagentNotice,
};

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
    queued_questionnaires: VecDeque<SubagentHostInputRequest>,
    queued_notices: VecDeque<SubagentNotice>,
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
    pub(super) fn bind(&mut self, manager: &crate::tools::agent::SubagentManager) {
        self.questionnaires = Some(manager.bind_host_input());
        self.notices = Some(manager.bind_notices());
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
            Incoming::Notice(notice) => self.queued_notices.push_back(notice),
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
        if let Some(receiver) = self.notices.as_mut() {
            while let Ok(notice) = receiver.try_recv() {
                self.queued_notices.push_back(notice);
                changed = true;
            }
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
        let notices_before = self.queued_notices.len();
        self.queued_notices
            .retain(|notice| &notice.parent_session_id == session_id);
        changed |= self.queued_notices.len() != notices_before;
        changed
    }

    /// Takes queued notices for this parent session in arrival order.
    ///
    /// Notices addressed to a session the parent has left can never be
    /// delivered, so they are dropped rather than queued forever.
    pub(super) fn take_notices(&mut self, session_id: &SessionId) -> Vec<SubagentNotice> {
        self.queued_notices
            .drain(..)
            .filter(|notice| &notice.parent_session_id == session_id)
            .collect()
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

    pub(super) fn has_pending_notices(&self) -> bool {
        !self.queued_notices.is_empty()
    }

    #[cfg(test)]
    pub(super) fn push_questionnaire_for_test(&mut self, request: SubagentHostInputRequest) {
        self.queued_questionnaires.push_back(request);
    }

    #[cfg(test)]
    pub(super) fn queued_questionnaire_count(&self) -> usize {
        self.queued_questionnaires.len()
    }

    #[cfg(test)]
    pub(super) fn push_notice_for_test(&mut self, notice: SubagentNotice) {
        self.queued_notices.push_back(notice);
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
