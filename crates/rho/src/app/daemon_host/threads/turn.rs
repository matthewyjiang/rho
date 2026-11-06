//! One prompt's turn, run in a task the daemon owns, so it goes on when the
//! client that sent the prompt disconnects.
//!
//! The turn reports to a [`DaemonView`], which keeps the job's state in step
//! with it: `waiting_for_input` while an approval, plan approval, or question
//! is open, `running` otherwise. Nothing answers those requests yet: no
//! client can attach to a turn until 2c-2 and none answers until 2e, so a
//! turn that asks parks until `rho/thread/cancel` withdraws its requests. A
//! request is never denied because no client is attached. The events
//! themselves are dropped until 2c-2 buffers them for clients.

use std::{collections::HashSet, future::Future, sync::Arc};

use rho_sdk::{CancellationToken, HostInputId, UserInput};
use tokio::sync::{mpsc, oneshot, Mutex};

use super::super::jobs::{JobId, JobRegistry, JobState};
use crate::{
    app::session_runtime::{
        ApprovalId, PlanApprovalId, PlanReport, SessionEvent, SessionRuntime, Turn, TurnControl,
        TurnIo, TurnOutcome, TurnView,
    },
    config::Config,
};

/// What a turn task needs: the thread's runtime and where to report the
/// job's state.
pub(super) struct TurnTask {
    pub(super) job: JobId,
    pub(super) jobs: Arc<JobRegistry>,
    /// The thread's runtime, locked for the whole turn and released only
    /// after the turn's end is recorded, so the daemon's stop, which waits
    /// for the lock, finds every turn it waited for recorded. `None` once
    /// that stop took it.
    pub(super) runtime: Arc<Mutex<Option<SessionRuntime>>>,
    pub(super) config: Arc<Config>,
    /// Cancelled once the daemon stops: a turn cancelled then was
    /// interrupted, not cancelled by a client.
    pub(super) stopping: CancellationToken,
    /// Answered once the turn saved its prompt, so the request returns.
    pub(super) started: oneshot::Sender<()>,
}

impl TurnTask {
    /// Runs one turn of `text` and records how it ended. The job is already
    /// `running`.
    pub(super) async fn run(self, text: String) {
        let Self {
            job,
            jobs,
            runtime: slot,
            config,
            stopping,
            started,
        } = self;
        // Free once the thread's last turn recorded its end, apart from the
        // moment that turn takes to release it.
        let mut runtime = slot.lock().await;
        let mut view = DaemonView {
            job: job.clone(),
            jobs: Arc::clone(&jobs),
            started: Some(started),
        };
        let ended = match &mut *runtime {
            Some(runtime) => run_turn(runtime, &mut view, text, &config).await,
            // Only the daemon's stop takes the runtime, after its last turn.
            None => Ok(TurnOutcome::Cancelled),
        };
        let recorded = match ended {
            Ok(TurnOutcome::Completed(_)) => jobs.set_state(&job, JobState::Completed),
            Ok(TurnOutcome::Cancelled) if stopping.is_cancelled() => {
                jobs.set_state(&job, JobState::Interrupted)
            }
            Ok(TurnOutcome::Cancelled) => jobs.set_state(&job, JobState::Cancelled),
            Err(error) => jobs.fail(&job, format!("{error:#}")),
        };
        if let Err(error) = recorded {
            tracing::warn!(%job, %error, "could not record the end of a turn");
        }
        drop(runtime);
        // Dropped only now, so a request still waiting for the turn to start
        // reads the end just recorded.
        drop(view);
    }
}

/// Begins a prompt of `text` on `runtime` and runs its model turns. Slash
/// commands are not expanded yet: the text goes to the model as written.
async fn run_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    text: String,
    config: &Config,
) -> anyhow::Result<TurnOutcome> {
    let scope = runtime.begin_prompt(config).await?;
    let input = UserInput::text(text);
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    // A turn builds a large future; keep it on the heap.
    Box::pin(runtime.run_prompt_turn(view, &scope, turn, config)).await
}

/// Shows a thread's turns to its job: the job's state follows the requests
/// the turn has open.
struct DaemonView {
    job: JobId,
    jobs: Arc<JobRegistry>,
    started: Option<oneshot::Sender<()>>,
}

impl DaemonView {
    /// Takes the turn's events until it drops its sender. Holds
    /// `_control`, the turn's answer channel, until then: a turn whose
    /// control channel closes fails, and parked requests must stay open.
    async fn show(
        &mut self,
        mut events: mpsc::UnboundedReceiver<SessionEvent>,
        _control: mpsc::UnboundedSender<TurnControl>,
    ) {
        let mut open = OpenInputs::default();
        while let Some(event) = events.recv().await {
            if matches!(event, SessionEvent::MessageInserted(_)) {
                if let Some(started) = self.started.take() {
                    let _ = started.send(());
                }
            }
            let was_waiting = open.waiting();
            open.note(&event);
            let state = match (was_waiting, open.waiting()) {
                (false, true) => JobState::WaitingForInput,
                (true, false) => JobState::Running,
                (false, false) | (true, true) => continue,
            };
            if let Err(error) = self.jobs.set_state(&self.job, state) {
                tracing::warn!(job = %self.job, %error, "could not record a turn's input wait");
            }
        }
    }
}

impl TurnView for DaemonView {
    fn open_turn<'v>(
        &'v mut self,
        _runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v>) {
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (control_sender, control) = mpsc::unbounded_channel();
        (
            TurnIo { events, control },
            self.show(event_receiver, control_sender),
        )
    }

    /// The handoff reaches clients with the turn's events, in 2c-2.
    fn report_plan(
        &mut self,
        _runtime: &SessionRuntime,
        _process_config: &Config,
        _report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// The requests a turn has open, which only a person can answer.
#[derive(Debug, Default)]
struct OpenInputs {
    approvals: HashSet<ApprovalId>,
    plans: HashSet<PlanApprovalId>,
    questions: HashSet<HostInputId>,
}

impl OpenInputs {
    /// Notes the request `event` opens or closes, if any.
    fn note(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::ApprovalRequested { id, .. } => {
                self.approvals.insert(*id);
            }
            SessionEvent::ApprovalWithdrawn { id } => {
                self.approvals.remove(id);
            }
            SessionEvent::PlanApprovalRequested { id, .. } => {
                self.plans.insert(*id);
            }
            SessionEvent::PlanApprovalWithdrawn { id } => {
                self.plans.remove(id);
            }
            SessionEvent::QuestionAsked { id, .. } => {
                self.questions.insert(id.clone());
            }
            SessionEvent::QuestionWithdrawn { id } => {
                self.questions.remove(id);
            }
            SessionEvent::MessageInserted(_)
            | SessionEvent::RunStarted
            | SessionEvent::Run(_)
            | SessionEvent::BoundaryApplied { .. }
            | SessionEvent::Notice { .. }
            | SessionEvent::RunClosed { .. }
            | SessionEvent::RunFinished { .. } => {}
        }
    }

    /// Whether the turn waits on a person.
    fn waiting(&self) -> bool {
        !(self.approvals.is_empty() && self.plans.is_empty() && self.questions.is_empty())
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
