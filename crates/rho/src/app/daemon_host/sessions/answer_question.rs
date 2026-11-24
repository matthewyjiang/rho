//! `rho/session/answer_question`: a question answered by the session that
//! started the asking one through session control, rather than by a person
//! (#1714). An agent's connection may call it, so the daemon checks the
//! answering session against its own job records before it answers: the
//! asking session's job must have been started by the job writing the
//! answering session, and that job's grant must allow acting on sessions.
//! Only then does it look at the request, which must be a question (see
//! `inputs`): approvals and plan approvals wait for a person whoever asks.
//!
//! The answering session is the one the caller declares, as a new session's
//! parent is: the daemon cannot tell who sends a request.

use rho_protocol::{
    ErrorCode, RpcError, SessionAnswerQuestionParams, SessionAnswerQuestionResponse,
};

use super::{inputs, reopen, JobId, Sessions};
use crate::{app::daemon_host::jobs::JobSubject, caller_context::Access};

impl Sessions {
    /// Hands `by`'s answer to one of a session's open questions, when `by`
    /// started the session and may act on sessions. The first answer wins,
    /// whoever sends it: a later one finds the request gone.
    pub(in crate::app::daemon_host) async fn answer_question(
        &self,
        params: SessionAnswerQuestionParams,
    ) -> Result<SessionAnswerQuestionResponse, RpcError> {
        let SessionAnswerQuestionParams {
            session,
            request,
            answer,
            by,
        } = params;
        let Some(asking) = self.loaded_session(&session).await? else {
            return Err(inputs::not_open(request));
        };
        self.may_answer(&asking.job, &session, &by)?;
        asking.change_inputs(|inputs| inputs.answer_question(request, answer, &by))?;
        Ok(SessionAnswerQuestionResponse {})
    }

    /// Refuses `by`'s answer to session `session`, whose turns job `asking`
    /// records, unless the job that started `asking` writes `by` and its
    /// grant allows acting on sessions.
    fn may_answer(
        &self,
        asking: &JobId,
        session: &rho_protocol::SessionId,
        by: &rho_protocol::SessionId,
    ) -> Result<(), RpcError> {
        let refused = |message: String| RpcError::new(ErrorCode::InvalidParams, message);
        let answering = reopen::parse_session(by)?;
        let started_by = self
            .jobs
            .get(asking)
            .and_then(|record| record.parent)
            .filter(|parent| {
                self.jobs
                    .get(parent)
                    .is_some_and(|record| match &record.subject {
                        JobSubject::Session { session: written }
                        | JobSubject::Subagent {
                            session: Some(written),
                            ..
                        } => *written == answering,
                        JobSubject::Subagent { session: None, .. }
                        | JobSubject::Workflow { .. } => false,
                    })
            })
            .ok_or_else(|| {
                refused(format!(
                    "session {by} did not start session {session}; only the session that started it may answer its questions"
                ))
            })?;
        let access = self
            .jobs
            .session_grant(&started_by)
            .map(|grant| grant.scope.access);
        if access != Some(Access::Write) {
            return Err(refused(format!(
                "session {by} may not act on sessions; answering a question needs write access"
            )));
        }
        Ok(())
    }
}
