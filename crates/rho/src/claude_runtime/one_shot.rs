//! Ask Claude one no-tools question over ACP and return its text.
//!
//! Delegated subagents go through [`super::session`], which owns the run
//! directory, status file, and attachment contract. Rho's own internal agents
//! (the advisor) need none of that: they ask one question, stream the answer
//! into a tool card, and keep nothing, so the session is not persisted.

use std::path::PathBuf;

use rho_sdk::{model::ModelUsage, CancellationToken};
use tokio::sync::watch;

use crate::{
    acp_runtime::one_shot::{self, OneShotError, OneShotProgress},
    agent::{OneShotPhase, OneShotUpdate, PromptPolicy},
    session_environment::Ambient,
};

use super::policy::{ClaudeAcpPolicy, ClaudeSessionShape, SessionPersistence};

/// A single Claude question with no tools and no follow-up turn.
pub(crate) struct ClaudeOneShotRequest {
    /// Replaces Claude's system prompt.
    pub(crate) system_prompt: &'static str,
    /// The user turn.
    pub(crate) input: String,
    /// Pass-through Claude model; `None` inherits Claude's.
    pub(crate) model: Option<String>,
    /// Bound reasoning; `None` inherits Claude's effort.
    pub(crate) reasoning: Option<rho_providers::reasoning::ReasoningLevel>,
    pub(crate) cwd: PathBuf,
    /// The environment Claude runs in: the asking session's (#1659).
    pub(crate) environment: Ambient,
    pub(crate) cancellation: CancellationToken,
}

/// Text and usage from a finished one-shot Claude call.
pub(crate) struct ClaudeOneShotResult {
    pub(crate) text: String,
    /// Claude reports spend, not token counts, over ACP.
    pub(crate) usage: ModelUsage,
}

/// Run the request to completion. Assistant text streams into `updates`;
/// reasoning only moves the phase to [`OneShotPhase::Thinking`].
pub(crate) async fn run_one_shot(
    request: ClaudeOneShotRequest,
    updates: Option<watch::Sender<OneShotUpdate>>,
) -> Result<ClaudeOneShotResult, OneShotError> {
    let publish = |phase, text: &str| {
        if let Some(updates) = &updates {
            let _ = updates.send(OneShotUpdate::new(phase, text));
        }
    };
    publish(OneShotPhase::WaitingForProvider, "");
    let policy = ClaudeAcpPolicy::headless(
        ClaudeSessionShape {
            system_prompt: PromptPolicy::Replace(request.system_prompt.to_owned()),
            model: request.model,
            reasoning: request.reasoning,
            // Parity with the Rho one-shot path, which exposes no tools.
            tools: Vec::new(),
            max_turns: 1,
            persistence: SessionPersistence::Discard,
        },
        request.cwd,
    );
    let mut answer_so_far = String::new();
    let answer = one_shot::run_one_shot(
        policy,
        &request.input,
        &request.environment,
        &request.cancellation,
        |progress| match progress {
            OneShotProgress::Thinking => publish(OneShotPhase::Thinking, &answer_so_far),
            OneShotProgress::Answer(text) => {
                answer_so_far = text.to_owned();
                publish(OneShotPhase::Responding, text);
            }
        },
    )
    .await?;
    Ok(ClaudeOneShotResult {
        text: answer.text.trim().to_owned(),
        usage: ModelUsage {
            // The usage ledger and the parent session total count micros.
            cost_usd_micros: answer
                .cost_usd
                .filter(|cost| *cost > 0.0)
                .map(|cost| (cost * 1_000_000.0).round() as u64),
            ..ModelUsage::default()
        },
    })
}
