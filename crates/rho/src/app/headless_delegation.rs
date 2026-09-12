//! Child delivery at SDK input boundaries for hosts without an interactive inbox.
//! A natural completion waits for children, but remains inside the original run
//! so cancellation and the SDK step budget still govern parent continuations.

use rho_sdk::{BoundaryInputRequest, InputBoundary, Session, UserInput};
use tokio::sync::mpsc;

use super::{
    subagent_manager::{SubagentManager, SubagentNotification},
    subagent_messaging::{NoticePermits, SubagentNotice},
};

pub(crate) struct HeadlessDelegation {
    manager: SubagentManager,
    session_id: String,
    requests: mpsc::Receiver<BoundaryInputRequest>,
    notices: mpsc::Receiver<SubagentNotice>,
    permits: NoticePermits,
    pending: Vec<SubagentNotice>,
}

impl HeadlessDelegation {
    pub(crate) fn attach(
        session: &Session,
        manager: Option<&SubagentManager>,
        storage: Option<&crate::session::Session>,
    ) -> Result<Option<Self>, rho_sdk::Error> {
        let Some(manager) = manager else {
            return Ok(None);
        };
        let (source, requests) = rho_sdk::boundary_input_channel();
        session.set_boundary_inputs(Some(source))?;
        manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
            session.id().to_string(),
            storage.and_then(crate::session::Session::subagents_dir),
        ));
        // Each headless prompt drops and retires its previous notice binding.
        let binding = manager.rebind_notices(None);
        Ok(Some(Self {
            manager: manager.clone(),
            session_id: session.id().to_string(),
            requests,
            notices: binding.receiver,
            permits: binding.permits,
            pending: binding.retained,
        }))
    }

    pub(crate) async fn drive<T>(
        slot: &mut Option<Self>,
        run: impl std::future::Future<Output = T>,
    ) -> T {
        let service = Self::serve(slot);
        tokio::pin!(service);
        let result = tokio::select! {
            biased;
            () = &mut service => unreachable!(),
            result = run => result,
        };
        // The SDK checkpoints input before acknowledging it. Reap any ready
        // acknowledgement before dropping reservations, including when the
        // event pump notices cancellation in that same scheduling turn.
        let _ = futures_util::poll!(&mut service);
        result
    }

    async fn serve(slot: &mut Option<Self>) {
        let Some(host) = slot else {
            return std::future::pending().await;
        };
        while let Some(request) = host.requests.recv().await {
            host.respond(request).await;
        }
        std::future::pending().await
    }

    async fn respond(&mut self, request: BoundaryInputRequest) {
        let boundary = request.boundary();
        let mut notices_open = true;
        let (receipt, mut reservation) = loop {
            {
                let _delivery = super::notification_delivery::lock();
                while let Ok(notice) = self.notices.try_recv() {
                    self.pending.push(notice);
                }
                self.pending.retain(|notice| {
                    if notice.is_acknowledged() {
                        self.permits.release_notice(notice);
                        false
                    } else {
                        true
                    }
                });
                let terminals = self.manager.take_notifications(&self.session_id);
                let mut sections = Vec::new();
                if !self.pending.is_empty() {
                    sections.push(format!(
                        "Earlier child messages in send order. Any terminal result below supersedes that child's planning and progress; preserve substantive findings.\n\n{}",
                        super::subagent_messaging::notice_prompt(&self.pending),
                    ));
                }
                if !terminals.is_empty() {
                    sections.push(crate::tools::agent::notification_prompt(&terminals));
                }
                if !terminals.is_empty()
                    || self
                        .pending
                        .iter()
                        .any(|notice| notice.delivery.requires_parent_action())
                    || boundary == InputBoundary::BeforeProvider
                    || !self
                        .manager
                        .has_active_or_pending_notification(&self.session_id)
                {
                    let input =
                        (!sections.is_empty()).then(|| UserInput::text(sections.join("\n\n")));
                    break (
                        request.respond(input),
                        TerminalReservation {
                            manager: self.manager.clone(),
                            terminals,
                        },
                    );
                }
            }
            tokio::select! {
                notice = self.notices.recv(), if notices_open => {
                    if let Some(notice) = notice {
                        self.pending.push(notice);
                    } else {
                        notices_open = false;
                    }
                }
                () = self.manager.wait_for_notification(&self.session_id) => {}
            }
        };
        if receipt.await {
            reservation.terminals.clear();
            for notice in self.pending.drain(..) {
                notice.acknowledge();
                self.permits.release_notice(&notice);
            }
        }
    }
}

impl Drop for HeadlessDelegation {
    fn drop(&mut self) {
        let _delivery = super::notification_delivery::lock();
        self.manager.unbind_notices();
        // Cancellation can leave queued notices behind. Retire their receipts
        // along with this binding rather than leaking them into later prompts.
        while let Ok(notice) = self.notices.try_recv() {
            self.pending.push(notice);
        }
        for notice in self.pending.drain(..) {
            self.permits.release_notice(&notice);
        }
    }
}

/// A cancelled boundary must not consume a terminal result it never committed.
struct TerminalReservation {
    manager: SubagentManager,
    terminals: Vec<SubagentNotification>,
}

impl Drop for TerminalReservation {
    fn drop(&mut self) {
        self.manager.restore_notifications(&self.terminals);
    }
}

#[cfg(test)]
#[path = "headless_delegation_tests.rs"]
mod tests;
