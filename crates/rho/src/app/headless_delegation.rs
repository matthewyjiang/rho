//! Background delivery at SDK input boundaries for hosts without an interactive
//! inbox (ACP and `rho run`). Finished subagents, workflows, and background
//! processes reach the model at the next boundary. Natural completion waits for
//! every undelivered child of the session, whichever prompt started it, and for
//! workflows and processes started during this prompt, standing in for the
//! TUI's idle wake. Workflows and processes from an earlier prompt, such as an
//! ACP `/workflow:<name>` start, only deliver; a later prompt does not wait for
//! them. The wait stays inside the original run, so cancellation and the SDK
//! step budget still govern parent continuations.
//! Host-orchestrated ACP children are not registered here: they outlive the
//! parent prompt and complete through the host's next prompt instead. Hosts
//! that show a transcript take [`HeadlessDelegation::offered_displays`] to
//! render what the model was given.
//!
//! In-process ACP children also outlive a cancelled prompt, so the ACP host
//! keeps their notice binding between prompts with
//! [`HeadlessDelegation::hold_notices`]. `rho run` drops it with the run.

use std::time::Instant;

use rho_sdk::{BoundaryInputRequest, InputBoundary, Session, UserInput};
use tokio::sync::mpsc;

use super::{
    boundary_results::BoundaryResults,
    subagent_manager::{SubagentManager, SubagentNotification},
    subagent_messaging::{NoticePermits, SubagentNotice},
};
use crate::{
    display_transcript::DisplayTranscript,
    tools::{
        process::{ProcessManager, ProcessNotification},
        sdk_registry::AppToolSet,
        workflow_tracker::{WorkflowNotification, WorkflowRunTracker},
    },
};

pub(crate) struct HeadlessDelegation {
    session_id: String,
    requests: mpsc::Receiver<BoundaryInputRequest>,
    subagents: Option<SubagentDelivery>,
    processes: Option<ProcessManager>,
    workflows: WorkflowRunTracker,
    /// Workflows and processes started from here on hold natural completion;
    /// older ones only deliver.
    prompt_started: Instant,
    /// Display rows for each delivery offered to the runtime, when a host asked.
    displays: Option<mpsc::UnboundedSender<(UserInput, DisplayTranscript)>>,
}

/// The parent's subagent notice binding for one headless prompt.
struct SubagentDelivery {
    manager: SubagentManager,
    notices: mpsc::Receiver<SubagentNotice>,
    permits: NoticePermits,
    pending: Vec<SubagentNotice>,
}

/// A notice binding kept between prompts. The binding stays installed, so a
/// child can still queue `message_parent` text or an action request while no
/// prompt runs, and notices a cancelled prompt never delivered reach the next.
pub(crate) struct HeldNotices(SubagentDelivery);

/// The background work a headless prompt delivers and waits for.
pub(crate) struct BackgroundSources<'a> {
    pub(crate) subagents: Option<&'a SubagentManager>,
    pub(crate) processes: Option<&'a ProcessManager>,
    pub(crate) workflows: &'a WorkflowRunTracker,
}

impl<'a> BackgroundSources<'a> {
    pub(crate) fn of(tools: &'a AppToolSet) -> Self {
        Self {
            subagents: tools.subagents(),
            processes: tools.processes(),
            workflows: tools.workflow_tracker(),
        }
    }
}

impl HeadlessDelegation {
    /// Installs boundary delivery for one prompt on an idle session. `held`
    /// resumes the previous prompt's notice binding; without it the prompt
    /// binds afresh, retiring any earlier binding.
    pub(crate) fn attach(
        session: &Session,
        sources: BackgroundSources<'_>,
        storage: Option<&crate::session::Session>,
        held: Option<HeldNotices>,
    ) -> Result<Self, rho_sdk::Error> {
        let (source, requests) = rho_sdk::boundary_input_channel();
        session.set_boundary_inputs(Some(source))?;
        let session_id = session.id().to_string();
        let mut held = held.map(|HeldNotices(delivery)| delivery);
        let subagents = sources.subagents.map(|manager| {
            manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
                session_id.clone(),
                storage.and_then(crate::session::Session::subagents_dir),
            ));
            held.take().unwrap_or_else(|| {
                let binding = manager.rebind_notices(None);
                SubagentDelivery {
                    manager: manager.clone(),
                    notices: binding.receiver,
                    permits: binding.permits,
                    pending: binding.retained,
                }
            })
        });
        let workflows = sources.workflows.clone();
        workflows.bind_parent_session(session_id.clone());
        Ok(Self {
            session_id,
            requests,
            subagents,
            processes: sources.processes.cloned(),
            workflows,
            prompt_started: Instant::now(),
            displays: None,
        })
    }

    /// Display rows for every delivery offered from now on. The host claims
    /// them as the run reports each input applied.
    pub(crate) fn offered_displays(&mut self) -> OfferedDisplays {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.displays = Some(sender);
        OfferedDisplays { receiver }
    }

    pub(crate) async fn drive<T>(host: &mut Self, run: impl std::future::Future<Output = T>) -> T {
        let service = host.serve();
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

    /// Ends this prompt but keeps its notice binding, with every notice not yet
    /// delivered, for the next [`Self::attach`].
    pub(crate) fn hold_notices(mut self) -> Option<HeldNotices> {
        self.subagents.take().map(HeldNotices)
    }

    async fn serve(&mut self) {
        while let Some(request) = self.requests.recv().await {
            self.respond(request).await;
        }
        std::future::pending().await
    }

    async fn respond(&mut self, request: BoundaryInputRequest) {
        let boundary = request.boundary();
        let mut notices_open = true;
        let (receipt, mut reservation) = loop {
            // Subscribe before collecting so a finish between the snapshot
            // and the wait below still wakes it.
            let process_exit = self.processes.as_ref().map(ProcessManager::notified_owned);
            let workflow_finish = self.workflows.notified_owned();
            {
                let _delivery = super::notification_delivery::lock();
                let batch = self.collect();
                if batch.has_results()
                    || self.subagents.as_ref().is_some_and(|subagents| {
                        subagents
                            .pending
                            .iter()
                            .any(|notice| notice.delivery.requires_parent_action())
                    })
                    || boundary == InputBoundary::BeforeProvider
                    || !self.has_outstanding_work()
                {
                    let input = self.render(&batch).map(|(model, display)| {
                        let input = UserInput::text(model);
                        if let Some(sink) = &self.displays {
                            // Queued before the reply, so its rows wait for
                            // the applied event. A closed host shows nothing.
                            let _ = sink.send((input.clone(), display));
                        }
                        input
                    });
                    break (request.respond(input), batch);
                }
            }
            let process_exit = async move {
                match process_exit {
                    Some(exit) => exit.await,
                    None => std::future::pending().await,
                }
            };
            match &mut self.subagents {
                Some(subagents) => tokio::select! {
                    notice = subagents.notices.recv(), if notices_open => {
                        if let Some(notice) = notice {
                            subagents.pending.push(notice);
                        } else {
                            notices_open = false;
                        }
                    }
                    () = subagents.manager.wait_for_notification(&self.session_id) => {}
                    () = process_exit => {}
                    () = workflow_finish => {}
                },
                None => tokio::select! {
                    () = process_exit => {}
                    () = workflow_finish => {}
                },
            }
        };
        if receipt.await {
            reservation.commit();
            if let Some(subagents) = &mut self.subagents {
                for notice in subagents.pending.drain(..) {
                    notice.acknowledge();
                    subagents.permits.release_notice(&notice);
                }
            }
        }
    }

    /// Drains finished work from every source. Call under the delivery lock.
    fn collect(&mut self) -> DeliveryReservation {
        let subagent_results = match &mut self.subagents {
            Some(subagents) => {
                while let Ok(notice) = subagents.notices.try_recv() {
                    subagents.pending.push(notice);
                }
                let permits = &subagents.permits;
                subagents.pending.retain(|notice| {
                    if notice.is_acknowledged() {
                        permits.release_notice(notice);
                        false
                    } else {
                        true
                    }
                });
                subagents.manager.take_notifications(&self.session_id)
            }
            None => Vec::new(),
        };
        DeliveryReservation {
            subagents: self
                .subagents
                .as_ref()
                .map(|subagents| subagents.manager.clone()),
            subagent_results,
            workflows: self.workflows.clone(),
            workflow_results: self.workflows.take_notifications(&self.session_id),
            processes: self.processes.clone(),
            process_results: self
                .processes
                .as_ref()
                .map(ProcessManager::take_notifications)
                .unwrap_or_default(),
        }
    }

    /// Model input and display rows for a batch, rendered as the TUI renders them.
    fn render(&self, batch: &DeliveryReservation) -> Option<(String, DisplayTranscript)> {
        BoundaryResults {
            notices: self
                .subagents
                .as_ref()
                .map(|subagents| subagents.pending.as_slice())
                .unwrap_or_default(),
            subagents: &batch.subagent_results,
            workflows: &batch.workflow_results,
            processes: &batch.process_results,
        }
        .render(self.subagents.as_ref().map(|subagents| &subagents.manager))
        .finish()
    }

    /// Work whose result could still arrive and should hold natural completion.
    fn has_outstanding_work(&self) -> bool {
        self.subagents.as_ref().is_some_and(|subagents| {
            subagents
                .manager
                .has_active_or_pending_notification(&self.session_id)
        }) || self.workflows.has_pending_notification(&self.session_id)
            || self
                .workflows
                .has_live_started_since(&self.session_id, self.prompt_started)
            || self.processes.as_ref().is_some_and(|processes| {
                processes.has_pending_notification()
                    || processes.has_live_started_since(self.prompt_started)
            })
    }
}

/// Display rows for deliveries offered to the runtime. Each waits for the
/// [`rho_sdk::RunEvent::BoundaryInputApplied`] that confirms it, so a host
/// shows a result exactly where it entered the run and never shows one the
/// runtime rejected.
pub(crate) struct OfferedDisplays {
    receiver: mpsc::UnboundedReceiver<(UserInput, DisplayTranscript)>,
}

impl OfferedDisplays {
    /// Rows for the `input` a run applied. A rejected offer can only precede
    /// it, so older offers that do not match are discarded.
    pub(crate) fn applied(&mut self, input: &UserInput) -> Option<DisplayTranscript> {
        while let Ok((offered, display)) = self.receiver.try_recv() {
            if offered == *input {
                return Some(display);
            }
        }
        None
    }
}

impl Drop for HeadlessDelegation {
    fn drop(&mut self) {
        let Some(subagents) = &mut self.subagents else {
            return;
        };
        let _delivery = super::notification_delivery::lock();
        subagents.manager.unbind_notices();
        // Cancellation can leave queued notices behind. A host that drops the
        // binding instead of holding it retires their receipts with it.
        while let Ok(notice) = subagents.notices.try_recv() {
            subagents.pending.push(notice);
        }
        for notice in subagents.pending.drain(..) {
            subagents.permits.release_notice(&notice);
        }
    }
}

/// Results drained for one boundary. A cancelled boundary must not consume
/// results it never committed, so dropping an uncommitted batch restores them.
struct DeliveryReservation {
    subagents: Option<SubagentManager>,
    subagent_results: Vec<SubagentNotification>,
    workflows: WorkflowRunTracker,
    workflow_results: Vec<WorkflowNotification>,
    processes: Option<ProcessManager>,
    process_results: Vec<ProcessNotification>,
}

impl DeliveryReservation {
    fn has_results(&self) -> bool {
        !self.subagent_results.is_empty()
            || !self.workflow_results.is_empty()
            || !self.process_results.is_empty()
    }

    fn commit(&mut self) {
        self.subagent_results.clear();
        self.workflow_results.clear();
        self.process_results.clear();
    }
}

impl Drop for DeliveryReservation {
    fn drop(&mut self) {
        if let Some(subagents) = &self.subagents {
            subagents.restore_notifications(&self.subagent_results);
        }
        self.workflows.restore_notifications(&self.workflow_results);
        if let Some(processes) = &self.processes {
            processes.restore_notifications(&self.process_results);
        }
    }
}

#[cfg(test)]
#[path = "headless_delegation_tests.rs"]
mod tests;
