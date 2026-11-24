//! `rho/session/answer_question`: a question answered by the session that
//! started the asking one through session control, rather than by a person
//! (#1714). An agent's connection may call it, so the daemon checks the
//! answering session against its own job records before it answers
//! (`JobRegistry::may_answer`, the one copy of the rule). Only then does it look at the request, which must be
//! a question anyone but the user may answer (see `inputs`): approvals,
//! plan approvals, and MCP servers' elicitations wait for a person whoever
//! asks.
//!
//! The answering session is the one the caller declares, as a new session's
//! parent is: the daemon cannot tell who sends a request.

use rho_protocol::{RpcError, SessionAnswerQuestionParams, SessionAnswerQuestionResponse};

use super::{inputs, reopen, Sessions};
use crate::app::daemon_host::jobs::AnswerRefused;

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
        let answering = reopen::parse_session(&by)?;
        self.jobs
            .may_answer(&asking.job, &answering)
            .map_err(|refused| {
                inputs::invalid(match refused {
                    AnswerRefused::NotStarter => format!(
                        "session {by} did not start session {session}; only the session that started it may answer its questions"
                    ),
                    AnswerRefused::ReadOnly => format!(
                        "session {by} may not act on sessions; answering a question needs write access"
                    ),
                    AnswerRefused::NoGrant => format!(
                        "session {by} runs under no grant the daemon knows, since the session that ran it is gone; answering a question needs write access"
                    ),
                })
            })?;
        asking.change_inputs(|inputs| inputs.answer_question(request, answer))?;
        Ok(SessionAnswerQuestionResponse {})
    }
}
