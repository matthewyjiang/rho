//! Forks (`rho/session/fork`): a side conversation a client opens beside a
//! saved session, such as to ask about the work while that session goes on
//! with it.
//!
//! A fork is a new session that starts from the session's conversation as
//! last saved, which a turn under way saves at every step, and never writes
//! back to it. It runs in the session's workspace as the agent the session
//! was saved as, with the parts it leaves out, the directories it adds, its
//! model, and its prompt cache key, so its requests start with the same
//! tools, system prompt, and conversation, which a provider's prompt cache
//! can serve. A fork's own setup, such as its client's MCP servers, is its
//! request's, as for any new session.
//!
//! The session may still be working in the same workspace, so the fork
//! opens in supervised mode: reads are free, and an edit or a command waits
//! for the person's approval. A session saved in plan mode forks in plan
//! mode, which edits nothing either and keeps the session's tools. The
//! fork's first model turn also tells the model that it is a fork and must
//! leave the workspace alone. Neither changes the tools or system prompt
//! the cache serves: supervised mode adds no tool, and the word to the
//! model goes ahead of the person's first message. That word is held in
//! memory (see `held_context.rs`), so a fork unloaded before its first
//! model turn runs, as after a lone `/compact`, reopens without it, still
//! supervised.
//!
//! A fork's file records its origin as a fork, which person-facing
//! listings leave out, and the session as the one that started it, so
//! deleting the session deletes it (see `catalog.rs`). A fork is not forked
//! again, and neither are subagents', workflow nodes', or sessions rebound
//! to another agent with `/agent:`, whose files do not keep everything a
//! fork would need to run as they do. A session whose worktree the daemon
//! removed (see `worktree.rs`) forks once a reopen has restored it.

use rho_protocol::{ErrorCode, RpcError, SessionForkParams, SessionForkResponse};

use super::{
    grant,
    open::{config_for_new, open_session, OpenWith, Opening},
    reopen::{parse_session, saved_session, SavedFor},
    Admission, JobId, JobRecord, Keeping, SessionJobRecord, Sessions,
};
use crate::{
    app::{daemon_host::wire::session_to_wire, prompt_content::PromptContext},
    session::{Session as StoredSession, SessionOrigin},
};

/// What a fork's first model turn tells the model, ahead of the person's
/// message. Its model copy stays in the fork's conversation, so later turns
/// and a reopened fork keep it.
const MODEL_CONTEXT: &str = "\
You are in a side chat: a fork of the conversation above, which the user \
opened to ask about it. The original session may still be running in the same \
workspace, and nothing you do here reaches it. Answer the user's questions. Do \
not continue or redo the original session's task, and do not modify the \
workspace: no file edits and no commands that change files or state. Reading \
the workspace is fine.";

impl Sessions {
    /// Opens a fork of the saved session `params` names, with the client's
    /// `setup`, and registers the job of its turns, which start at once, as
    /// a new session's with no parent do.
    pub(in crate::app::daemon_host) async fn fork(
        &self,
        params: SessionForkParams,
    ) -> Result<SessionForkResponse, RpcError> {
        let SessionForkParams {
            session: source,
            environment,
            setup,
        } = params;
        self.refuse_when_stopping()?;
        let source = parse_session(&source)?;
        // Held until the fork is loaded, so a delete of the session, which
        // takes this lock, finds the fork and deletes it too.
        let _source = self.reopening.lock(&source).await;
        let stored = saved_session(&source, SavedFor::Fork).await?;
        let opening = grant::opening(grant::asked(None, &environment)?, None);
        let with = OpenWith::under(opening, &environment, setup)?;
        let global = config_for_new(&self.settings).await?;
        let job = JobId::generate();
        let opened = open_session(
            Opening::Fork(stored),
            &with,
            &self.subagents,
            &self.pull_requests,
            &self.settings,
            global,
            &job,
        )
        .await?;
        let session = opened.runtime.session_id().clone();
        let record = JobRecord {
            admission: Admission::Immediate,
            keeping: match opened.runtime.persistence() {
                crate::session::SessionPersistence::Saved => Keeping::Stored,
                crate::session::SessionPersistence::Ephemeral => Keeping::MemoryOnly,
            },
            grant_limit: (!with.grant.is_root()).then(|| with.grant.clone()),
            grant: with.grant,
            ..JobRecord::new_session(
                job.clone(),
                opened.runtime.workspace_root().to_path_buf(),
                session.clone(),
            )
        };
        let added = self.add(SessionJobRecord::Register(record), opened).await?;
        // Before the reply names the fork, so its first prompt finds it.
        added.hold_context(PromptContext::of(MODEL_CONTEXT));
        tracing::info!(%session, %job, %source, "forked session");
        Ok(SessionForkResponse {
            session: session_to_wire(session),
            options: added.feed.options(),
        })
    }
}

/// Refuses to fork `stored`, whose origin is `origin`, when it is a fork
/// itself or was rebound to another agent (see the module docs). Blocking.
pub(super) fn refuse_unforkable(
    stored: &StoredSession,
    origin: Option<SessionOrigin>,
) -> Result<(), RpcError> {
    let refused = |why: String| {
        Err(RpcError::new(
            ErrorCode::SessionOpenFailed,
            format!("could not fork session {}: {why}", stored.id()),
        ))
    };
    if origin == Some(SessionOrigin::Fork) {
        return refused("it is a fork itself; fork the session it forks".to_owned());
    }
    match stored.delegated_agent_id() {
        Ok(None) => Ok(()),
        Ok(Some(agent)) => refused(format!(
            "it was switched to agent {agent:?} with /agent:, which a fork cannot run as yet"
        )),
        Err(error) => refused(format!("{error:#}")),
    }
}
