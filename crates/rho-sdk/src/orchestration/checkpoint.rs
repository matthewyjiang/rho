//! Opt-in step checkpoints and continuation of an interrupted run.
//!
//! A host that installs a checkpoint store can restore the last saved snapshot
//! after a crash and continue it with [`crate::Session::continue_history`].

use std::collections::BTreeSet;

use crate::{
    model::{ContentBlock, Message},
    session::{CommitMark, SessionCore},
    tool::{ToolInvocationSource, ToolOrigin, ToolSecurity},
    CapabilityKind, Error,
};

use super::{
    tool_settlement::interrupted_result,
    tool_turn::{StagedCall, StagedToolTurn},
    Rho,
};

/// A run's step checkpoints: what the session holds of its working history,
/// and whether the checkpoint store holds it too.
///
/// The loop saves before each provider request, after each model reply, and
/// after recording a host batch, in the last two cases before the tool calls
/// start. A restored snapshot therefore names every call that may have
/// started. Tool results appended since the last save are saved before the
/// next request, so a run killed while the model streams keeps them. A save
/// is skipped only when nothing changed since the last durable one.
pub(super) struct Checkpoints {
    committed: CommitMark,
    saved: bool,
}

impl Checkpoints {
    /// Starts from the commit the run copied its working history from. The
    /// store may not hold it, so the first save always runs.
    pub(super) fn new(committed: CommitMark) -> Self {
        Self {
            committed,
            saved: false,
        }
    }

    /// Commits the working history and saves it to the session's checkpoint
    /// store. Without a store this does nothing.
    pub(super) async fn save(
        &mut self,
        core: &SessionCore,
        runtime: &Rho,
        history: &[Message],
    ) -> Result<(), Error> {
        let Some(store) = &runtime.checkpoint_store else {
            return Ok(());
        };
        if self.saved && core.holds_unchanged(self.committed, history) {
            return Ok(());
        }
        self.saved = false;
        self.committed = core.commit_checkpoint(history, self.committed)?;
        store.save(core.persistence_snapshot()).await?;
        self.saved = true;
        Ok(())
    }
}

/// Settles tool calls that an interrupted run left without results.
///
/// A built-in tool that declares only read capabilities runs again in the
/// returned turn. Any other call may already have had effects, so it gets an
/// interrupted result instead of a second execution.
pub(super) fn settle_unanswered_calls(
    history: &mut Vec<Message>,
    runtime: &Rho,
) -> Option<StagedToolTurn> {
    let answered = history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.id.as_str()),
            Message::System(_)
            | Message::User(_)
            | Message::Assistant(_)
            | Message::EnrichedAssistant(_)
            | Message::AbortedAssistant(_) => None,
        })
        .collect::<BTreeSet<_>>();
    let unanswered = history
        .iter()
        .filter_map(Message::completed_assistant_content)
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if !answered.contains(call.id.as_str()) => {
                Some(call.clone())
            }
            ContentBlock::ToolCall(_) | ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>();
    let mut rerun = Vec::new();
    for call in unanswered {
        match runtime.tools.get(&call.name) {
            Some(tool) if reruns_safely(&tool.security()) => {
                rerun.push(StagedCall::new(
                    call,
                    ToolInvocationSource::Model,
                    Some(tool),
                ));
            }
            Some(_) | None => history.push(Message::ToolResult(interrupted_result(&call))),
        }
    }
    (!rerun.is_empty()).then(|| StagedToolTurn::model_requested(rerun))
}

fn reruns_safely(security: &ToolSecurity) -> bool {
    security.origin() == ToolOrigin::BuiltIn
        && !security.capabilities().is_empty()
        && security
            .capabilities()
            .iter()
            .all(|capability| *capability == CapabilityKind::Read)
}
