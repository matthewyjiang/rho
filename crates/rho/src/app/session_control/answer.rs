//! `answer`: the caller answers a question that a session it started asks
//! (#1714). Approvals and plan approvals are never answered here: they wait
//! for a person, and the daemon refuses them on this path whatever the
//! caller.
//!
//! The check refuses a session the caller did not start, as the daemon's
//! job records tell it, so the model learns why before anyone is asked to
//! approve the call. The daemon checks the same again, and the grant, when
//! it answers (`rho/session/answer_question`), naming the calling session
//! as the one that answers.

use rho_protocol::{
    methods::SessionAnswerQuestion, ClientError, ErrorCode, JobKind, JobSummary,
    SessionAnswerQuestionParams, SessionId,
};

use super::{
    jobs_of, AnswerParams, AnswerResult, Caller, ControlError, DaemonError, Refusal, SessionControl,
};

impl SessionControl {
    /// Refuses `session` unless `caller`'s session started it.
    pub(super) async fn check_started(
        &self,
        caller: &Caller,
        session: &SessionId,
    ) -> Result<(), ControlError> {
        let jobs = jobs_of(self.daemon_for(caller.daemon)).await?;
        if started_by(&jobs, session, caller.session.as_ref()) {
            return Ok(());
        }
        Err(not_started(caller, session).into())
    }

    /// Hands the call's answer to the question it names, as the calling
    /// session's. An answer that lost, to a person's or the timeout's, or
    /// to a request the turn withdrew, says the question is no longer open.
    pub(super) async fn answer(
        &self,
        caller: &Caller,
        params: AnswerParams,
    ) -> Result<AnswerResult, ControlError> {
        let answer = params.answer()?;
        let by = caller
            .session
            .clone()
            .ok_or_else(|| not_started(caller, &params.session))?;
        let answered = self
            .daemon
            .request::<SessionAnswerQuestion>(&SessionAnswerQuestionParams {
                session: params.session.clone(),
                request: params.request,
                answer,
                by,
            })
            .await;
        match answered {
            Ok(_) => Ok(AnswerResult {}),
            Err(DaemonError::Request(ClientError::Rpc { error, .. }))
                if error.error_code() == Some(ErrorCode::InputNotOpen) =>
            {
                Err(ControlError::Invalid(format!(
                    "input request {} of session {} is no longer open: it was already answered, it timed out, or its turn withdrew it",
                    params.request, params.session
                )))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// Whether `jobs` record that session `by` started `session`: a job of
/// `session`'s turns whose parent is a job writing `by`. No calling
/// session started nothing.
fn started_by(jobs: &[JobSummary], session: &SessionId, by: Option<&SessionId>) -> bool {
    let Some(by) = by else {
        return false;
    };
    jobs.iter()
        .filter(|job| job.kind == JobKind::Session && job.session.as_ref() == Some(session))
        .filter_map(|job| job.parent.as_ref())
        .any(|parent| {
            jobs.iter()
                .any(|job| &job.id == parent && job.session.as_ref() == Some(by))
        })
}

fn not_started(caller: &Caller, session: &SessionId) -> Refusal {
    Refusal::NotStarted {
        holder: caller.grant.holder.clone(),
        session: session.clone(),
    }
}

#[cfg(test)]
#[path = "answer_tests.rs"]
mod tests;
