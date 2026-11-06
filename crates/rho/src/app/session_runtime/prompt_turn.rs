//! A prompt's model turns as one unit: the turn, the plan handoff it may
//! have asked for, and the implementation turn an approved plan starts.
//!
//! Every host runs a prompt's model turn through
//! [`SessionRuntime::run_prompt_turn`], so each gets the same sequence. The
//! runtime decides the handoff (see `plan_exit.rs`); the host only shows
//! each turn through IO of its own and reports the handoff, through its
//! [`TurnView`].

use std::future::Future;

use super::{plan_exit::PlanHandoff, PromptScope, SessionRuntime, Turn, TurnIo, TurnOutcome};
use crate::{config::Config, permission::PermissionMode};

/// How a host shows a prompt's turns. It does not own the runtime:
/// [`SessionRuntime::run_prompt_turn`] lends it the runtime to read as each
/// turn opens and as the handoff is reported.
///
/// [`Self::open_turn`] is called once per turn. It returns fresh [`TurnIo`]
/// for the turn and the future that shows the turn through it: one that
/// takes the turn's events and answers its requests until the turn drops its
/// event sender, as it does when it returns, and keeps the control sender
/// until then. That future runs beside the turn, so it may borrow the view
/// but not the runtime; read what it needs of the runtime up front. What
/// showing the turn came to, such as an update the client missed, stays in
/// the view; it never changes how the turn ended, since the turn is saved
/// either way.
pub(crate) trait TurnView {
    /// The IO the next turn reports through, and the future that shows it.
    fn open_turn<'v>(
        &'v mut self,
        runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v, Self>);

    /// Shows how the handoff went, before the prompt goes on. `runtime` is
    /// in the mode the handoff left it.
    fn report_plan(
        &mut self,
        runtime: &SessionRuntime,
        process_config: &Config,
        report: PlanReport,
    ) -> impl Future<Output = ()> + Send;
}

/// What a host shows of a plan handoff. A turn that answered no plan has
/// none.
#[derive(Debug)]
pub(crate) enum PlanReport {
    /// The session kept its mode; `notice` says why the answered plan did
    /// not switch it. The prompt ends with the turn.
    Kept { notice: String },
    /// The session is now in `mode`, as `notice` tells the user; the
    /// implementation turn runs next.
    Switched {
        mode: PermissionMode,
        notice: String,
    },
}

impl SessionRuntime {
    /// Runs `turn` within `prompt`, then hands off the plan it may have
    /// answered: an approved plan on an answered turn switches the session's
    /// mode and runs the implementation turn in the same prompt. `view`
    /// shows both turns and reports the handoff between them.
    ///
    /// Returns the prompt's end: the implementation turn's after a switch,
    /// else the first turn's, which a cancel that landed before the switch
    /// turns into `Ok(TurnOutcome::Cancelled)`.
    pub(crate) async fn run_prompt_turn(
        &mut self,
        view: &mut impl TurnView,
        prompt: &PromptScope,
        turn: Turn,
        process_config: &Config,
    ) -> anyhow::Result<TurnOutcome> {
        let ended = self.show_turn(view, prompt, turn, process_config).await;
        match self.hand_off_plan(prompt, ended, process_config).await {
            PlanHandoff::Ended { notice, ended } => {
                if let Some(notice) = notice {
                    view.report_plan(self, process_config, PlanReport::Kept { notice })
                        .await;
                }
                ended
            }
            PlanHandoff::Switched {
                mode,
                notice,
                implementation,
            } => {
                view.report_plan(self, process_config, PlanReport::Switched { mode, notice })
                    .await;
                self.show_turn(view, prompt, implementation, process_config)
                    .await
            }
        }
    }

    /// Runs `turn` beside `view` showing it.
    async fn show_turn(
        &mut self,
        view: &mut impl TurnView,
        prompt: &PromptScope,
        turn: Turn,
        process_config: &Config,
    ) -> anyhow::Result<TurnOutcome> {
        let (io, showing) = view.open_turn(self);
        let (ended, ()) = tokio::join!(self.run_turn(prompt, turn, io, process_config), showing);
        ended
    }
}
