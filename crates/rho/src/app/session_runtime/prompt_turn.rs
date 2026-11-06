//! A prompt's model turns as one unit: the turn, the plan handoff it may
//! have asked for, and the implementation turn an approved plan starts.
//!
//! Every host runs a prompt's model turn through [`run_prompt_turn`], so
//! each gets the same sequence. The runtime decides the handoff (see
//! `plan_exit.rs`); the host only shows each turn through IO of its own
//! and reports the handoff, through [`TurnHost`].

use std::future::Future;

use super::{plan_exit::PlanHandoff, PromptScope, SessionRuntime, Turn, TurnIo, TurnOutcome};
use crate::{config::Config, permission::PermissionMode};

/// How a host shows a prompt's model turns. The host owns its runtime and
/// lends it out, so it can show a turn while the runtime runs it.
///
/// [`Self::open_turn`] is called once per turn. It returns the runtime with
/// fresh [`TurnIo`] for the turn and the future that shows the turn through
/// it: one that takes the turn's events and answers its requests until the
/// turn drops its event sender, as it does when it returns, and keeps the
/// control sender until then. Its output is whatever the host needs from
/// showing the turn, such as an update its client missed; it never changes
/// how the turn ended, since the turn is saved either way.
pub(crate) trait TurnHost {
    /// What showing one turn came to.
    type Shown;

    /// The runtime, for the handoff between turns.
    fn runtime(&mut self) -> &mut SessionRuntime;

    /// The runtime to run the next turn on, the IO the turn reports
    /// through, and the future that shows it.
    fn open_turn(
        &mut self,
    ) -> (
        &mut SessionRuntime,
        TurnIo,
        impl Future<Output = Self::Shown> + Send,
    );

    /// Shows how the handoff went, before the prompt goes on.
    fn report_plan(&mut self, report: PlanReport) -> impl Future<Output = ()> + Send;
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

/// How a prompt's model turns ended.
pub(crate) struct PromptTurnEnd<S> {
    /// The prompt's end: the implementation turn's after a switch, else the
    /// first turn's, which a cancel that landed before the switch turns
    /// into `Ok(TurnOutcome::Cancelled)`.
    pub(crate) ended: anyhow::Result<TurnOutcome>,
    /// What showing the first turn came to.
    pub(crate) turn: S,
    /// What showing the implementation turn came to, when one ran.
    pub(crate) implementation: Option<S>,
}

/// Runs `turn` within `prompt` on `host`'s runtime, then hands off the plan
/// it may have answered: an approved plan on an answered turn switches the
/// session's mode and runs the implementation turn in the same prompt.
/// `host` shows both turns and reports the handoff between them.
pub(crate) async fn run_prompt_turn<H: TurnHost>(
    host: &mut H,
    prompt: &PromptScope,
    turn: Turn,
    process_config: &Config,
) -> PromptTurnEnd<H::Shown> {
    let (ended, shown) = show_turn(host, prompt, turn, process_config).await;
    let handoff = host
        .runtime()
        .hand_off_plan(prompt, ended, process_config)
        .await;
    match handoff {
        PlanHandoff::Ended { notice, ended } => {
            if let Some(notice) = notice {
                host.report_plan(PlanReport::Kept { notice }).await;
            }
            PromptTurnEnd {
                ended,
                turn: shown,
                implementation: None,
            }
        }
        PlanHandoff::Switched {
            mode,
            notice,
            implementation,
        } => {
            host.report_plan(PlanReport::Switched { mode, notice })
                .await;
            let (ended, implemented) =
                show_turn(host, prompt, implementation, process_config).await;
            PromptTurnEnd {
                ended,
                turn: shown,
                implementation: Some(implemented),
            }
        }
    }
}

/// Runs `turn` beside the host showing it.
async fn show_turn<H: TurnHost>(
    host: &mut H,
    prompt: &PromptScope,
    turn: Turn,
    process_config: &Config,
) -> (anyhow::Result<TurnOutcome>, H::Shown) {
    let (runtime, io, showing) = host.open_turn();
    tokio::join!(runtime.run_turn(prompt, turn, io, process_config), showing)
}
