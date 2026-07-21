use rho_sdk::{Error, RunEvent};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InteractiveState {
    #[default]
    Idle,
    Run(RunState),
    Transition(TransitionState),
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunState {
    Running(RunPhase),
    WaitingForHostInput,
    Cancelling(RunPhase),
    Compacting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionState {
    SwitchingProvider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunPhase {
    Model,
    Tool,
    Steering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveRunCommand {
    Quit,
    SwitchSession,
    ReplaceProvider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveRunDisposition {
    CancelAndWait,
    RejectUntilFinished,
    DeferUntilFinished,
}

pub(crate) const fn active_run_disposition(command: ActiveRunCommand) -> ActiveRunDisposition {
    match command {
        ActiveRunCommand::Quit => ActiveRunDisposition::CancelAndWait,
        ActiveRunCommand::SwitchSession => ActiveRunDisposition::RejectUntilFinished,
        ActiveRunCommand::ReplaceProvider => ActiveRunDisposition::DeferUntilFinished,
    }
}

pub(crate) fn begin_provider_switch(current: InteractiveState) -> Result<InteractiveState, Error> {
    if current == InteractiveState::Idle {
        Ok(InteractiveState::Transition(
            TransitionState::SwitchingProvider,
        ))
    } else {
        Err(Error::SessionBusy)
    }
}

pub(crate) fn state_after_event(current: InteractiveState, event: &RunEvent) -> InteractiveState {
    match event {
        RunEvent::Started { .. } | RunEvent::StepStarted { .. } => {
            running_unless_cancelling(current, RunPhase::Model)
        }
        RunEvent::ToolStarted { .. } => running_unless_cancelling(current, RunPhase::Tool),
        RunEvent::ToolFinished { .. } => running_unless_cancelling(current, RunPhase::Model),
        RunEvent::HostInputRequested { .. } | RunEvent::ToolHostInputRequested { .. } => {
            if is_cancelling(current) {
                current
            } else {
                InteractiveState::Run(RunState::WaitingForHostInput)
            }
        }
        RunEvent::CompactionStarted { .. } => {
            if is_cancelling(current) {
                current
            } else {
                InteractiveState::Run(RunState::Compacting)
            }
        }
        RunEvent::CompactionCompleted { .. } => running_unless_cancelling(current, RunPhase::Model),
        RunEvent::Completed { .. } | RunEvent::Cancelled { .. } => InteractiveState::Completed,
        RunEvent::Failed { .. } => InteractiveState::Failed,
        _ => current,
    }
}

fn running_unless_cancelling(current: InteractiveState, phase: RunPhase) -> InteractiveState {
    if is_cancelling(current) {
        current
    } else {
        InteractiveState::Run(RunState::Running(phase))
    }
}

fn is_cancelling(state: InteractiveState) -> bool {
    matches!(state, InteractiveState::Run(RunState::Cancelling(_)))
}

#[cfg(test)]
#[path = "interactive_state_tests.rs"]
mod tests;
