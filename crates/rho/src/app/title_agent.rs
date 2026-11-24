//! Runs the title agent, which names sessions and delegated runs.
//!
//! The agent gets the material to name (see `crate::title`) in a session of
//! its own, on the title model, where it may read the workspace when the
//! material alone does not say what the work is. The session is never
//! saved, is heard by no hook but the `before_tool_use` gate, and ends
//! with the title.

use std::{num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use rho_sdk::{SessionId, StopReason, UserInput};

use super::{
    agent_binding::{AgentBinder, BoundAgent},
    read_only_session::{self, ReadOnlySession},
    session_assembly::{BuiltSession, HookStart, ModelOwner},
};
use crate::{
    added_dirs::AddedDirs,
    agent::{internal_definition, AgentRuntimeSpec, SESSION_TITLE_AGENT_ID},
    config::{CodemodeConfig, Config},
    session_environment::SessionEnvironment,
    title::{title_from_answer, title_model_from_config},
};

const USAGE_PURPOSE: &str = "title";

/// The most model steps one title takes: a step reads, and the answer is
/// the last. A tripwire against an agent that keeps reading: of 40 titles
/// on claude-haiku-5-5 (text, file-reference, and image prompts,
/// 2026-11-23), 34 took one step and 6 took two, none more.
const TITLE_MAX_STEPS: NonZeroUsize = NonZeroUsize::new(8).unwrap();

/// How long one title may take, from building its session to its answer.
/// Of the same 40 titles, 39 landed 0.8 s to 4.2 s after their prompt and
/// one took 10.7 s; the rest is room for a slower title model.
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// What the title agent names, and where it may look to name it.
pub(crate) struct TitleRequest {
    /// The config of whoever asks, on its conversation model: the title
    /// agent's own model is that model unless the config names another.
    pub(crate) config: Config,
    pub(crate) config_path: PathBuf,
    /// The workspace the agent may read.
    pub(crate) cwd: PathBuf,
    pub(crate) added_dirs: AddedDirs,
    /// Whose credentials the title model signs in with.
    pub(crate) environment: SessionEnvironment,
    /// The session the title's usage counts toward: the one being named,
    /// or the one a named run works for.
    pub(crate) session: SessionId,
    /// The material to name: [`crate::title::session_input`] or
    /// [`crate::title::run_input`].
    pub(crate) input: UserInput,
}

/// Asks the title agent to name `request`'s material.
///
/// # Errors
///
/// Fails when the title model cannot be signed in to or reached, when the
/// agent runs out of [`TITLE_MAX_STEPS`] or [`TITLE_TIMEOUT`], and when its
/// answer is not a title.
pub(crate) async fn generate_title(request: TitleRequest) -> anyhow::Result<String> {
    let TitleRequest {
        config,
        config_path,
        cwd,
        added_dirs,
        environment,
        session,
        input,
    } = request;
    let deadline = tokio::time::Instant::now() + TITLE_TIMEOUT;
    let agent = bind_title_agent(&config, &environment)?;
    let built = tokio::time::timeout_at(
        deadline,
        read_only_session::assemble(ReadOnlySession {
            agent: &agent,
            config_path,
            cwd: &cwd,
            added_dirs: &added_dirs,
            environment,
            max_steps: TITLE_MAX_STEPS,
            usage_purpose: USAGE_PURPOSE,
            parent: session,
            // A title is the host's own bookkeeping, not work the user's
            // hooks asked to hear about; its reads still obey their
            // `before_tool_use` denials.
            hook_start: HookStart::Gate,
            model_owner: ModelOwner::InternalAgent,
            history: Vec::new(),
        }),
    )
    .await
    .map_err(|_| timed_out())??;
    let answer = ask(&built, input, deadline).await;
    built.teardown().await;
    title_from_answer(&answer?)
}

/// The title agent's answer to `input` on its session, by `deadline`.
async fn ask(
    built: &BuiltSession,
    input: UserInput,
    deadline: tokio::time::Instant,
) -> anyhow::Result<String> {
    let mut run = built.session.start(input).await?;
    let outcome = tokio::select! {
        outcome = run.outcome() => outcome,
        () = tokio::time::sleep_until(deadline) => {
            run.cancel();
            // Settled before the session is torn down under it.
            let _ = run.outcome().await;
            return Err(timed_out());
        }
    }?;
    // Any other stop is judged by the answer it left.
    anyhow::ensure!(
        outcome.stop_reason() != StopReason::MaxSteps,
        "title agent took all {TITLE_MAX_STEPS} of its steps without an answer (TITLE_MAX_STEPS)"
    );
    Ok(outcome.text().to_owned())
}

fn timed_out() -> anyhow::Error {
    anyhow::anyhow!(
        "title generation took over {}s (TITLE_TIMEOUT)",
        TITLE_TIMEOUT.as_secs()
    )
}

/// The title agent bound to its model on `config`, signed in with
/// `environment`'s credentials.
fn bind_title_agent(
    config: &Config,
    environment: &SessionEnvironment,
) -> anyhow::Result<BoundAgent> {
    let model = title_model_from_config(config);
    let mut host = config.clone();
    // The prompt names the read tools, and the step budget was measured
    // with them declared: `only` would hide them behind `codemode`.
    host.codemode = CodemodeConfig::default();
    host.provider = model.provider;
    host.model = model.model;
    host.auth = model.auth;
    let mut definition = internal_definition(SESSION_TITLE_AGENT_ID).clone();
    match &mut definition.runtime {
        // The config's reasoning for the title agent, else the definition's
        // own, fitted to what the title model takes.
        AgentRuntimeSpec::Rho { reasoning, .. } => *reasoning = Some(model.reasoning),
        AgentRuntimeSpec::ClaudeCli(_)
        | AgentRuntimeSpec::Cursor(_)
        | AgentRuntimeSpec::Antigravity(_) => {
            unreachable!("the title agent is defined on the Rho runtime")
        }
    }
    AgentBinder::bind_with_credentials(
        Arc::new(definition),
        read_only_session::invocation(),
        &host,
        /*model_override*/ None,
        environment.credentials().as_ref(),
    )
}

#[cfg(test)]
#[path = "title_agent_tests.rs"]
mod tests;
