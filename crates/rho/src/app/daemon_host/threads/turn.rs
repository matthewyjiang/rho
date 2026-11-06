//! One prompt's turn, run in a task the daemon owns, so it goes on when the
//! client that sent the prompt disconnects.
//!
//! The turn reports to a [`DaemonView`], which keeps the job's state in step
//! with it: `waiting_for_input` while an approval, plan approval, or question
//! is open, `running` otherwise. Nothing answers those requests yet: no
//! client can attach to a turn until #1657 and none answers until #1660, so
//! a turn that asks parks until `rho/thread/cancel` withdraws its requests.
//! A request is never denied because no client is attached. The events
//! themselves are dropped until #1657 buffers them for clients.

use std::{collections::HashSet, future::Future, sync::Arc};

use rho_sdk::{CancellationToken, HostInputId, UserInput};
use tokio::sync::{mpsc, oneshot};

use super::{
    super::jobs::{JobEvent, JobId, JobRegistry, TurnEnd},
    LiveThread,
};
use crate::{
    app::session_runtime::{
        ApprovalId, PlanApprovalId, PlanReport, SessionEvent, SessionRuntime, Turn, TurnControl,
        TurnIo, TurnOutcome, TurnView,
    },
    config::Config,
};

/// How a prompt's turn began, as the request that sent the prompt learns
/// it. A turn that completed saved its prompt, so it began `Saved`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PromptStart {
    /// The turn saved the prompt and runs on.
    Saved,
    /// A client cancelled the turn before it saved the prompt.
    Cancelled,
    /// The daemon stopped before the turn saved the prompt.
    Interrupted,
    /// The turn failed before it saved the prompt, for this reason.
    Failed(String),
}

impl PromptStart {
    /// How a turn that ended with `end` before it reported saving its
    /// prompt began.
    fn ended(end: &TurnEnd) -> Self {
        match end {
            TurnEnd::Completed => Self::Saved,
            TurnEnd::Cancelled => Self::Cancelled,
            TurnEnd::Interrupted => Self::Interrupted,
            TurnEnd::Failed(error) => Self::Failed(error.clone()),
        }
    }
}

/// One turn of a thread whose job already recorded the turn's start.
pub(super) struct TurnTask {
    pub(super) job: JobId,
    pub(super) jobs: Arc<JobRegistry>,
    pub(super) thread: Arc<LiveThread>,
    /// Cancelled once the daemon stops: a turn cancelled then was
    /// interrupted, not cancelled by a client.
    pub(super) stopping: CancellationToken,
    /// Tripped by a cancel sent for this turn, even one sent before the
    /// turn began its prompt.
    pub(super) cancel: CancellationToken,
}

impl TurnTask {
    /// Runs the turn of `text` in a daemon task. The receiver learns how
    /// the turn began; it closes without an answer only if the task
    /// panicked.
    pub(super) fn spawn(self, text: String) -> oneshot::Receiver<PromptStart> {
        let (started, start) = oneshot::channel();
        tokio::spawn(self.run(text, started));
        start
    }

    async fn run(self, text: String, started: oneshot::Sender<PromptStart>) {
        let Self {
            job,
            jobs,
            thread,
            stopping,
            cancel,
        } = self;
        // Locked for the whole turn and released only after the turn's end
        // is recorded, so the daemon's stop, which waits for the lock, finds
        // every turn it waited for recorded. Free once the thread's last
        // turn recorded its end, apart from the moment it takes to release.
        let mut runtime = thread.runtime.lock().await;
        // Declared after the lock, so even a panic records the end (see
        // `DaemonView`'s drop) before the lock is released.
        let mut view = DaemonView::new(job, jobs, started);
        let ended = match &mut *runtime {
            Some(runtime) => run_turn(runtime, &mut view, text, &thread.config, &cancel).await,
            // Only the daemon's stop takes the runtime, after its last turn.
            None => Ok(TurnOutcome::Cancelled),
        };
        view.end(match ended {
            Ok(TurnOutcome::Completed(_)) => TurnEnd::Completed,
            Ok(TurnOutcome::Cancelled) if stopping.is_cancelled() => TurnEnd::Interrupted,
            Ok(TurnOutcome::Cancelled) => TurnEnd::Cancelled,
            Err(error) => TurnEnd::Failed(format!("{error:#}")),
        });
    }
}

/// Begins a prompt of `text` on `runtime` and runs its model turns. Slash
/// commands are not expanded yet: the text goes to the model as written.
async fn run_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    text: String,
    config: &Config,
    cancel: &CancellationToken,
) -> anyhow::Result<TurnOutcome> {
    let scope = runtime.begin_prompt(config).await?;
    // A cancel from here on reaches the prompt through the thread's handle;
    // one sent before the prompt began only tripped `cancel`.
    if cancel.is_cancelled() {
        runtime.handle().cancel();
    }
    let input = UserInput::text(text);
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    // A turn builds a large future; keep it on the heap.
    Box::pin(runtime.run_prompt_turn(view, &scope, turn, config)).await
}

/// The message a turn records when its task stops without ending it.
const UNEXPECTED_STOP: &str = "the turn stopped unexpectedly; see the daemon log";

/// Shows a thread's turn to its job: the job's state follows the requests
/// the turn has open, and the request that sent the prompt learns how the
/// turn began.
///
/// Records the turn's end exactly once: through [`Self::end`], or, when the
/// view drops without it, as the turn task panicking, failed.
struct DaemonView {
    job: JobId,
    jobs: Arc<JobRegistry>,
    /// Answered `Saved` once the turn saved its prompt, or with how the
    /// turn ended if it ends first.
    started: Option<oneshot::Sender<PromptStart>>,
    /// Kept across the turn's `show` calls: a plan handoff opens the next
    /// model turn while the requests of the last may still be open.
    open: OpenInputs,
    /// Whether the turn's end is recorded.
    ended: bool,
}

impl DaemonView {
    fn new(job: JobId, jobs: Arc<JobRegistry>, started: oneshot::Sender<PromptStart>) -> Self {
        Self {
            job,
            jobs,
            started: Some(started),
            open: OpenInputs::default(),
            ended: false,
        }
    }

    /// Records that the turn ended with `end`, and tells a request still
    /// waiting for the turn to begin.
    fn end(mut self, end: TurnEnd) {
        let start = PromptStart::ended(&end);
        self.record(JobEvent::TurnEnded(end));
        self.ended = true;
        if let Some(started) = self.started.take() {
            let _ = started.send(start);
        }
    }

    fn record(&self, event: JobEvent) {
        if let Err(error) = self.jobs.record(&self.job, event) {
            tracing::warn!(job = %self.job, %error, "could not record a turn's progress");
        }
    }

    /// Takes the turn's events until it drops its sender. Holds
    /// `_control`, the turn's answer channel, until then: a turn whose
    /// control channel closes fails, and parked requests must stay open.
    async fn show(
        &mut self,
        mut events: mpsc::UnboundedReceiver<SessionEvent>,
        _control: mpsc::UnboundedSender<TurnControl>,
    ) {
        while let Some(event) = events.recv().await {
            if matches!(event, SessionEvent::MessageInserted(_)) {
                if let Some(started) = self.started.take() {
                    let _ = started.send(PromptStart::Saved);
                }
            }
            let was_waiting = self.open.waiting();
            self.open.note(&event);
            let event = match (was_waiting, self.open.waiting()) {
                (false, true) => JobEvent::InputOpened,
                (true, false) => JobEvent::InputSettled,
                (false, false) | (true, true) => continue,
            };
            self.record(event);
        }
    }
}

impl Drop for DaemonView {
    fn drop(&mut self) {
        if !self.ended {
            self.record(JobEvent::TurnEnded(TurnEnd::Failed(
                UNEXPECTED_STOP.to_owned(),
            )));
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

    /// The handoff reaches clients with the turn's events, in #1657.
    fn report_plan(
        &mut self,
        _runtime: &SessionRuntime,
        _process_config: &Config,
        _report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// The requests a turn has open, which only a person can answer. A request
/// leaves only when the turn withdraws it; #1660, which sends answers, must
/// remove the request it answers, since an answered request is not
/// withdrawn.
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
