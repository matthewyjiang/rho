//! `answer`: the caller answers a question that a session it started asks
//! (#1714). Approvals and plan approvals are never answered here: they wait
//! for a person, and the daemon refuses them on this path whatever the
//! caller.
//!
//! The daemon alone decides whether the calling session may answer
//! (`rho/session/answer_question`, naming the calling session as the one
//! that answers): it refuses a session that did not start the asker, or
//! whose grant may not act on sessions, and its refusal is the call's
//! error.

use rho_protocol::{
    methods::SessionAnswerQuestion, ClientError, ErrorCode, InputRequestId, QuestionAnswer,
    SessionAnswerQuestionParams, SessionId,
};

use super::{AnswerResult, ControlError, DaemonError, SessionControl};

impl SessionControl {
    /// Hands `answer` to question `request` of `session`, as session
    /// `by`'s. An answer that lost, to a person's or the timeout's, or to a
    /// request the turn withdrew, says the question is not open.
    pub(super) async fn answer(
        &self,
        session: SessionId,
        request: InputRequestId,
        answer: QuestionAnswer,
        by: SessionId,
    ) -> Result<AnswerResult, ControlError> {
        let answered = self
            .daemon
            .request::<SessionAnswerQuestion>(&SessionAnswerQuestionParams {
                session: session.clone(),
                request,
                answer,
                by,
            })
            .await;
        match answered {
            Ok(_) => Ok(AnswerResult {}),
            Err(DaemonError::Request(ClientError::Rpc { error, .. }))
                if error.error_code() == Some(ErrorCode::InputNotOpen) =>
            {
                Err(not_open(&session, request))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// The refusal of an answer to request `request` of `session`, which is not
/// open: from the daemon, or from the check, which reads the open requests
/// first and also sees an id that never opened.
pub(super) fn not_open(session: &SessionId, request: InputRequestId) -> ControlError {
    ControlError::Invalid(format!(
        "input request {request} of session {session} is not open: it was already answered, it timed out, or its turn withdrew it"
    ))
}
