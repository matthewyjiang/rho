//! `/compact` for ACP sessions: compact history between turns, persist the
//! result, and report it as an agent message.

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, PromptResponse, SessionNotification, SessionUpdate, StopReason,
};
use rho_sdk::CancellationToken;

use super::{send_notification, SessionHost};
use crate::app::acp::AcpClientPort;

impl SessionHost {
    /// Compacts with optional `instructions` on what to keep.
    ///
    /// The SDK commits replacement history before `compact` returns, so the
    /// work runs as a task: a cancel aborts it only while it is still running.
    /// A task that already finished has committed, and its outcome is saved
    /// like any other, as the TUI's abort does.
    pub(super) async fn compact(
        &mut self,
        instructions: Option<String>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
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
            Err(error) if error.is_cancelled() => {
                return Ok(PromptResponse::new(StopReason::Cancelled));
            }
            Err(error) => return Err(anyhow::anyhow!("could not compact context: {error}")),
        };
        if !self.is_replaced() {
            if let Err(error) =
                self.stored
                    .save_compaction_snapshot(&self.durable_snapshot(), &[], &outcome)
            {
                // Live history, compaction accounting, and metadata already
                // moved on; refuse later prompts rather than run against
                // state a reload would not see.
                self.unsaved_compaction = true;
                return Err(error.context("could not save compacted context"));
            }
        }
        // The context meter follows the committed history.
        let signals = self.signals();
        signals.send_usage(&self.acp_session_id, client).await;
        self.sent_plan = signals.into_sent_plan();
        send_notification(
            client,
            SessionNotification::new(
                self.acp_session_id.clone(),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
                    compaction_notice(&outcome),
                ))),
            ),
        )
        .await?;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }
}

fn compaction_notice(outcome: &rho_sdk::CompactionOutcome) -> String {
    if crate::compaction::outcome_reduced_context(outcome) {
        format!(
            "context compacted: {} → {} messages, {} → {} tokens",
            outcome.previous_messages(),
            outcome.current_messages(),
            outcome.previous_tokens(),
            outcome.current_tokens(),
        )
    } else {
        "context not compacted: compaction ran but did not reduce context".into()
    }
}
