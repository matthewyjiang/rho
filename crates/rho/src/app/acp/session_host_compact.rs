//! `/compact` for ACP sessions: the runtime compacts and saves; the host
//! reports it, as a compaction card on hosts that advertise compaction
//! updates, else as an agent message.

use agent_client_protocol::schema::v1::{
    PromptResponse, SessionNotification, SessionUpdate, StopReason,
};

use super::{send_notification, SessionHost};
use crate::app::acp::{
    compaction_card::{outcome_summary, CompactionCard},
    events::EventMapper,
    AcpClientPort,
};

impl SessionHost {
    /// Compacts with optional `instructions` on what to keep. A card opens
    /// before the work starts and closes with its outcome, error, or cancel.
    pub(super) async fn compact(
        &mut self,
        instructions: Option<String>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let card = self.host_features.compaction.then(CompactionCard::new);
        if let Some(card) = &card {
            self.send_update(client, card.in_progress()).await?;
        }
        let result = self.runtime.compact(instructions).await;
        if let Ok(Some(_)) = &result {
            // The context meter follows the committed history.
            let signals = self.signals();
            signals.send_usage(&self.acp_session_id, client).await;
            self.sent_plan = signals.into_sent_plan();
        }
        let report = match (card, &result) {
            (Some(card), Ok(Some(_))) => Some(card.completed()),
            (Some(card), Ok(None)) => Some(card.cancelled()),
            // Same text as the prompt's error reply.
            (Some(card), Err(error)) => Some(card.failed(error.to_string())),
            (None, Ok(Some(outcome))) => Some(EventMapper::agent_message(outcome_summary(outcome))),
            // The prompt's error reply or cancelled stop says the rest.
            (None, Ok(None) | Err(_)) => None,
        };
        if let Some(report) = report {
            let sent = self.send_update(client, report).await;
            match (&result, sent) {
                (Ok(Some(_)), sent) => sent?,
                // The prompt's error reply or cancelled stop is the answer
                // that matters; a lost closing card must not replace it.
                (Ok(None) | Err(_), Err(error)) => {
                    tracing::debug!(%error, "could not send closing compaction card");
                }
                (Ok(None) | Err(_), Ok(())) => {}
            }
        }
        let stop = match result? {
            Some(_) => StopReason::EndTurn,
            None => StopReason::Cancelled,
        };
        Ok(PromptResponse::new(stop))
    }

    async fn send_update(
        &self,
        client: &dyn AcpClientPort,
        update: SessionUpdate,
    ) -> anyhow::Result<()> {
        send_notification(
            client,
            SessionNotification::new(self.acp_session_id.clone(), update),
        )
        .await
    }
}
