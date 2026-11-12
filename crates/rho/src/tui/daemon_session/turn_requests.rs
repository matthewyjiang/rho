//! Requests about the turn the followed session runs: cancelling it,
//! steering it, and answering and holding its input requests. Only an
//! opened session has a turn, so before the first prompt they ask nothing.

use rho_protocol::{
    methods::{SessionAnswer, SessionCancel, SessionHold, SessionSteer, SessionUnsteer},
    ClientError, ErrorCode, InputAnswer, InputRequestId, SessionAnswerParams, SessionCancelParams,
    SessionHoldParams, SessionSteerParams, SessionUnsteerParams, SteerId, UnsteerOutcome,
};
use rho_sdk::SteeringId;

use super::{DaemonSession, Following, LOST};
use crate::tui::{daemon_events::DaemonView, daemon_inputs::AnswerRefusal};

impl DaemonSession {
    /// Asks the turn under way, if any, to stop. A closed connection
    /// has already ended what the TUI followed, so it asks nothing.
    pub(in crate::tui) async fn cancel(&self) -> anyhow::Result<()> {
        // Only on the connection the session is attached on: one the
        // TUI reconnects on has no turn of its to cancel yet.
        let (Ok(link), Ok(session)) = (self.attached_link().await, self.opened_id()) else {
            return Ok(());
        };
        link.request::<SessionCancel>(&SessionCancelParams { session })
            .await?;
        Ok(())
    }

    /// Hands `text` to the running turn's next model request; returns
    /// the steer's id, which its user message carries once it applies.
    pub(in crate::tui) async fn steer(&self, text: String) -> anyhow::Result<SteeringId> {
        let session = self.opened_id()?;
        let steered = self
            .attached_link()
            .await?
            .request::<SessionSteer>(&SessionSteerParams { session, text })
            .await;
        let id = match steered {
            Ok(response) => response.id,
            Err(error) => return Err(steer_refused(error)),
        };
        SteeringId::from_string(id.as_str())
            .map_err(|_| anyhow::anyhow!("the rho daemon answered with an empty steer id"))
    }

    /// Withdraws steer `id` unless the turn already took it.
    pub(in crate::tui) async fn unsteer(&self, id: &SteeringId) -> anyhow::Result<UnsteerOutcome> {
        let session = self.opened_id()?;
        let withdrawn = self
            .attached_link()
            .await?
            .request::<SessionUnsteer>(&SessionUnsteerParams {
                session,
                id: SteerId::new(id.as_str()),
            })
            .await;
        match withdrawn {
            Ok(response) => Ok(response.outcome),
            Err(error) => Err(steer_refused(error)),
        }
    }

    /// Answers input request `id` with `answer`. Once this returns, a
    /// refusal's views wait for [`Self::take_views`]: an error row, then
    /// the request again while it is still open. An answer the daemon
    /// took shows nothing: the prompt already closed.
    pub(in crate::tui) async fn answer(&mut self, id: InputRequestId, answer: InputAnswer) {
        // Only an opened session has input requests to answer.
        let Following::Attached(attachment) = &mut self.following else {
            return;
        };
        attachment.events.answering(id);
        let params = SessionAnswerParams {
            session: attachment.id.clone(),
            request: id,
            answer,
        };
        let answered = match self.attached_link().await {
            Ok(link) => link.request::<SessionAnswer>(&params).await.map_err(Some),
            Err(_) => Err(None),
        };
        let refusal = match answered {
            Ok(_) => return,
            Err(Some(error)) if error.error_code() == Some(ErrorCode::InputNotOpen) => {
                AnswerRefusal::NotOpen
            }
            Err(Some(error)) => AnswerRefusal::Failed(format!("{:#}", anyhow::Error::new(error))),
            // Following ends once the TUI reads the connection's loss.
            Err(None) => AnswerRefusal::Failed(LOST.to_owned()),
        };
        let Following::Attached(attachment) = &mut self.following else {
            return;
        };
        let views = attachment.events.answer_refused(id, refusal);
        self.pending.extend(views);
    }

    /// Stops input request `id`'s timeout, as once the person starts on
    /// it, and says whether it needs no more holding: the daemon held
    /// it, it already closed (its resolution says how), or the
    /// connection is gone. Once a hold fails, its error row waits for
    /// [`Self::take_views`].
    pub(in crate::tui) async fn hold(&mut self, id: InputRequestId) -> bool {
        // Without the attached connection there is nothing to hold for:
        // following ends once the TUI reads its loss.
        let (Ok(link), Ok(session)) = (self.attached_link().await, self.opened_id()) else {
            return true;
        };
        let params = SessionHoldParams {
            session,
            request: id,
        };
        match link.request::<SessionHold>(&params).await {
            Ok(_) => true,
            Err(error) if error.error_code() == Some(ErrorCode::InputNotOpen) => true,
            Err(error) => {
                self.pending.push(DaemonView::Error(format!(
                    "could not hold the request: {:#}",
                    anyhow::Error::new(error)
                )));
                false
            }
        }
    }
}

/// Why a steer, or its withdrawal, was refused, as the TUI says it.
fn steer_refused(error: ClientError) -> anyhow::Error {
    if error.error_code() == Some(ErrorCode::NotSteerable) {
        anyhow::anyhow!("no model request is taking input")
    } else {
        anyhow::Error::new(error)
    }
}
