//! The send gate for a daemon session (#1667).
//!
//! The daemon holds the conversation and runs the turn, so the TUI reads
//! what the session's model cannot be sent from the daemon
//! (`SessionReport::ProviderContext`) before each prompt and asks as in
//! process. A released submission is a prompt request: it starts no provider
//! here, so it carries no [`super::SendAuthorization`].
//!
//! "Compact, then send" parks the prompt first in the follow-up queue and
//! sends `/compact`, which the daemon runs as a turn. The queue sends the
//! prompt when that turn completes, and the consent given here covers it
//! once, so it is not asked for again. A cancelled compaction returns the
//! prompt to the composer with the rest of the pending work; a failed one
//! leaves it parked.

use rho_sdk::SessionId;

use super::{PendingConfirmSend, SendOmissions, SendPayload, SendSubmission};
use crate::app::provider_context_report::ProviderContextReport;
use crate::commands::CommandId;
use crate::tui::{App, DefaultTerminal, QueuedPrompt, TurnPrompt};

impl SendSubmission {
    /// The turn this asks a daemon session for, and the same prompt as the
    /// follow-up queue holds one.
    pub(in crate::tui) fn into_daemon_turn(self) -> (TurnPrompt, QueuedPrompt) {
        let SendPayload::Turn {
            turn,
            media,
            paste_segments,
            ..
        } = self.into_payload()
        else {
            unreachable!("a daemon session's sends are turn submissions");
        };
        let prompt = QueuedPrompt {
            prompt: turn.model.clone(),
            display_prompt: turn.display.clone(),
            paste_segments,
            media,
        };
        (turn, prompt)
    }
}

impl SendOmissions {
    /// What `report` says session `session_id` cannot send its model.
    fn reported(session_id: SessionId, report: &ProviderContextReport) -> Self {
        Self {
            session_id,
            target: report.target.clone(),
            blocks: report.blocks.clone(),
        }
    }
}

impl App {
    /// The omission gate for a prompt to the followed daemon session.
    /// Returns ownership when no confirmation is needed, this exact
    /// submission was approved for the model the session still runs on, or
    /// consent already covers the omissions; otherwise the confirmation
    /// takes it.
    ///
    /// A report the daemon cannot give does not hold the prompt back: the
    /// daemon answers the prompt itself, and says why when it refuses it.
    pub(in crate::tui) async fn gate_daemon_send(
        &mut self,
        submission: SendSubmission,
    ) -> Option<SendSubmission> {
        let report = match self.read_daemon_provider_context().await {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!(
                    error = %format_args!("{error:#}"),
                    "could not read the daemon session's provider context; sending unconfirmed"
                );
                return Some(submission);
            }
        };
        let (omissions, report) = report;
        if submission.is_approved_for(&omissions.target) {
            return Some(submission);
        }
        // Taken by the first send it could cover, so it never outlives the
        // prompt it was given for.
        let compacted = self.pending.take_compact_send_consent();
        let consents = [
            self.acknowledged_send_omissions.as_ref(),
            compacted.as_ref(),
        ];
        if consents
            .into_iter()
            .flatten()
            .any(|consent| consent.covers(&omissions))
            || omissions.blocks.is_empty()
        {
            return Some(submission);
        }
        let can_compact = report.can_compact && self.daemon_offers_built_in(CommandId::Compact);
        self.confirm_send(
            PendingConfirmSend {
                submission,
                omissions,
                can_compact,
            },
            &report.omissions(),
        );
        None
    }

    async fn read_daemon_provider_context(
        &self,
    ) -> anyhow::Result<(SendOmissions, ProviderContextReport)> {
        let Some(session) = self.info.session.daemon.as_ref() else {
            anyhow::bail!("no daemon session is open");
        };
        let report = session.provider_context().await?;
        let id = SessionId::from_string(session.id())?;
        Ok((SendOmissions::reported(id, &report), report))
    }

    /// Sends `submission` to the daemon session once it passes the gate,
    /// which an approval for the model the session still runs on does.
    pub(in crate::tui) async fn submit_daemon_submission(
        &mut self,
        submission: SendSubmission,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(submission) = self.gate_daemon_send(submission).await else {
            return Ok(());
        };
        let (turn, prompt) = submission.into_daemon_turn();
        self.send_daemon_turn(turn, prompt.media, prompt.paste_segments, terminal)
            .await
    }

    /// Compacts the daemon session, then sends `submission` as the next
    /// queued message under the consent `omissions` records.
    pub(super) async fn compact_then_send_to_daemon(
        &mut self,
        submission: SendSubmission,
        omissions: SendOmissions,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let (_, prompt) = submission.into_daemon_turn();
        self.pending.push_follow_up_front(prompt);
        self.pending.set_compact_send_consent(omissions);
        self.pending_input_changed();
        self.select_pending_recall_target();
        let compact = TurnPrompt {
            // Not the person's input, so it stays out of the input history.
            history: String::new(),
            ..TurnPrompt::command("/compact".into(), "/compact".into())
        };
        self.send_daemon_turn(compact, Vec::new(), Vec::new(), terminal)
            .await
    }
}
