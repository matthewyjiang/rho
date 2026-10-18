//! `/compact` for ACP sessions: compact history between turns, persist the
//! result, and report it: as a compaction card on hosts that advertise
//! compaction updates, else as an agent message.

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, PromptResponse, SessionNotification, SessionUpdate, StopReason,
};
use rho_sdk::{CancellationToken, CompactionOutcome};

use super::{send_notification, SessionHost};
use crate::app::acp::{
    compaction_card::{outcome_summary, CompactionCard},
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
        let card = self.host_updates.compaction.then(CompactionCard::new);
        if let Some(card) = &card {
            self.send_update(client, card.in_progress()).await?;
        }
        let result = self.run_compaction(instructions, client).await;
        let report = match (card, &result) {
            (Some(card), Ok(Some(_))) => Some(card.completed()),
            (Some(card), Ok(None)) => Some(card.cancelled()),
            // Same text as the prompt's error reply.
            (Some(card), Err(error)) => Some(card.failed(error.to_string())),
            (None, Ok(Some(outcome))) => Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::from(outcome_summary(outcome)),
            ))),
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

    /// Runs and saves the compaction; `None` when it was cancelled first.
    ///
    /// The SDK commits replacement history before `compact` returns, so the
    /// work runs as a task: a cancel aborts it only while it is still running.
    /// A task that already finished has committed, and its outcome is saved
    /// like any other, as the TUI's abort does.
    async fn run_compaction(
        &mut self,
        instructions: Option<String>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<Option<CompactionOutcome>> {
        let cancel = CancellationToken::new();
        self.prompt_gate.activate(cancel.clone());
        let session = self.built.session.clone();
        let mut task = tokio::spawn(async move {
            match instructions {
                Some(instructions) => session.compact_with_instructions(instructions).await,
                None => session.compact().await,
            }
        });
        let joined = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                task.abort();
                (&mut task).await
            }
            joined = &mut task => joined,
        };
        let outcome = match joined {
            Ok(outcome) => {
                outcome.map_err(|error| anyhow::anyhow!("could not compact context: {error}"))?
            }
            Err(error) if error.is_cancelled() => return Ok(None),
            Err(error) => return Err(anyhow::anyhow!("could not compact context: {error}")),
        };
        if self.is_replaced() {
            // Load and shutdown cancel this prompt after snapshotting the
            // file. The rewrite was not saved, so do not report it as done.
            return Ok(None);
        }
        let snapshot = self.durable_snapshot();
        let saved = self
            .stored()
            .and_then(|stored| stored.save_compaction_snapshot(&snapshot, &[], &outcome));
        if let Err(error) = saved {
            // Live history, compaction accounting, and metadata already
            // moved on; refuse later prompts rather than run against
            // state a reload would not see.
            self.unsaved_compaction = true;
            return Err(error.context("could not save compacted context"));
        }
        // The context meter follows the committed history.
        let signals = self.signals();
        signals.send_usage(&self.acp_session_id, client).await;
        self.sent_plan = signals.into_sent_plan();
        Ok(Some(outcome))
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
