//! The session host's side of T3 wake watchers (see [`super::super::t3_wake`]):
//! arming one as a parent prompt ends, recognizing the prompt T3 sends when
//! it fires, and running `/rho-wake` as the watcher, resuming a wait, or
//! repeating its result, after T3 Code restarts.

use agent_client_protocol::schema::v1::{PromptResponse, StopReason};
use rho_sdk::{model::ContentBlock, CancellationToken, UserInput};

use super::{
    super::t3_wake::{Background, PromptOrigin, WakeSignal},
    send_notification, EventMapper, SessionHost,
};
use crate::app::acp::AcpClientPort;

/// Added for the model to a prompt that mentions a watcher, normally T3's
/// notice that it finished. T3 asks the model to read the task, but a
/// watcher holds no result: the background results follow at the first
/// boundary. The notice stays whole, since T3 batches other tasks into it.
const WATCHER_NOTE: &str = "Delegated tasks titled `waiting: ...` are Rho's background-work \
                            watchers, not agents, and hold no result. Results of background work \
                            that finished, if any, follow.";

impl SessionHost {
    /// Call as a prompt starts, before anything it awaits. Returns the note
    /// to add to its model input when it mentions a watcher.
    pub(super) fn wake_prompt_started(&mut self, input: &UserInput) -> Option<&'static str> {
        let wake = self.t3_wake.as_mut()?;
        let text = match input.blocks().first() {
            Some(ContentBlock::Text(text)) => Some(text.as_str()),
            _ => None,
        };
        match wake.prompt_started(text) {
            PromptOrigin::WatcherNotice => Some(WATCHER_NOTE),
            PromptOrigin::Other => None,
        }
    }

    /// Call as a prompt ends: arms a watcher while background work runs. A
    /// cancelled prompt usually gives way to the next one, whose end arms
    /// instead, so only other stops report a failure to the client.
    pub(super) async fn wake_prompt_ended(
        &mut self,
        stop: Option<StopReason>,
        client: &dyn AcpClientPort,
    ) {
        let Some(wake) = self.t3_wake.as_mut() else {
            return;
        };
        let background = Background::of(&self.built.tools, self.built.session.id().to_string());
        let Err(error) = wake.prompt_ended(&self.built.tools, background).await else {
            return;
        };
        if stop == Some(StopReason::Cancelled) {
            tracing::debug!(%error, "could not arm T3 wake watcher after a cancelled prompt");
            return;
        }
        tracing::warn!(%error, "could not arm T3 wake watcher");
        let notice = EventMapper::agent_notice(
            &self.acp_session_id,
            &format!(
                "\n\ncould not watch background work: {error}. Its results arrive with your \
                 next message."
            ),
        );
        if let Err(error) = send_notification(client, notice).await {
            tracing::debug!(%error, "could not send T3 wake failure notice");
        }
    }

    /// The wait this session ran as a watcher in an earlier process, if
    /// any. Every prompt resumes it, or repeats its result, instead of
    /// running a model turn: T3 Code sends "continue" to each thread a
    /// restart cut off before it saw the thread's turn end. Other hosts
    /// never send it, so they never consult records.
    pub(super) fn recorded_wake(&self) -> Option<WakeSignal> {
        self.t3_wake.as_ref()?;
        WakeSignal::recorded(self.acp_session_id.0.as_ref())
    }

    /// `/rho-wake`: waits for the parent's signal without a model turn or a
    /// saved prompt, then ends the turn with the signal's summary as the
    /// watcher's result.
    pub(super) async fn await_wake(
        &self,
        signal: WakeSignal,
        cancel: &CancellationToken,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let session_id = self.acp_session_id.0.as_ref();
        let Some(summary) = signal.wait(session_id, cancel).await else {
            // A session that goes on elsewhere, as when T3 Code restarts or
            // reloads it, keeps the wait for the prompt that resumes it. A
            // cancel or close, as T3's task_cancel sends, ends it.
            if !self.prompt_gate.hands_off() {
                signal.abandon(session_id);
            }
            return Ok(PromptResponse::new(StopReason::Cancelled));
        };
        send_notification(
            client,
            EventMapper::agent_notice(&self.acp_session_id, &summary),
        )
        .await?;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }
}
