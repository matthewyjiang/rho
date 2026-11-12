//! The send gate for the daemon session (#1667).
//!
//! The daemon holds the conversation and runs the turn, so the TUI reads
//! what the session's model cannot be sent from the daemon
//! (`SessionReport::ProviderContext`) before each prompt and asks before
//! sending. A released submission is a prompt request.
//!
//! "Compact, then send" parks the prompt first in the follow-up queue and
//! sends `/compact`, which the daemon runs as a turn. The queue sends the
//! prompt when that turn ends, whether the compaction compacted or failed,
//! and the consent given here goes with that prompt alone, so it is not
//! asked for again and no other prompt is sent under it. A cancelled
//! compaction returns the prompt to the composer with the rest of the
//! pending work, without the consent. A compaction the daemon cannot start
//! parks nothing: the prompt returns to the composer.
//!
//! A prompt held while the session's MCP servers connect keeps the model it
//! was confirmed for (`HeldOrigin::Confirmed`), so its release asks again
//! only if the session's model changed meanwhile.

use rho_sdk::SessionId;

use super::{PendingConfirmSend, SendOmissions, SendSubmission};
use crate::app::provider_context_report::ProviderContextReport;
use crate::commands::CommandId;
use crate::tui::{
    idle_input::{HeldOrigin, HeldTurn},
    App, DefaultTerminal, Entry, QueuedPrompt, TurnPrompt,
};

impl SendSubmission {
    /// Sends this under `consent`, when the person gave one for this prompt
    /// before the session compacted for it.
    pub(in crate::tui) fn with_compact_consent(mut self, consent: Option<SendOmissions>) -> Self {
        self.compact_consent = consent;
        self
    }

    /// The turn this asks the daemon session for, and the same prompt as
    /// the follow-up queue holds one.
    pub(in crate::tui) fn into_daemon_turn(self) -> (TurnPrompt, QueuedPrompt) {
        let Self {
            turn,
            media,
            paste_segments,
            ..
        } = self;
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
    /// The omission gate for a prompt to the followed session.
    /// Returns ownership, approved for the model the session runs on, when
    /// no confirmation is needed, this exact submission was approved for
    /// that model, or consent already covers the omissions; otherwise the
    /// confirmation takes it.
    ///
    /// A report the daemon cannot give does not hold the prompt back: the
    /// daemon answers the prompt itself, and says why when it refuses it.
    pub(in crate::tui) async fn gate_daemon_send(
        &mut self,
        mut submission: SendSubmission,
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
        let compacted = submission.compact_consent.take();
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
            return Some(submission.approve_for(omissions.target));
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
        let session = &self.info.session.daemon;
        let report = session.provider_context().await?;
        let id = SessionId::from_string(
            session
                .id()
                .ok_or_else(|| anyhow::anyhow!("the session has not opened"))?,
        )?;
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
        let confirmed_for = submission.approved_for.clone();
        let (turn, prompt) = submission.into_daemon_turn();
        if self.daemon_tool_sources_loading() {
            self.hold(HeldTurn {
                turn,
                media: prompt.media,
                paste_segments: prompt.paste_segments,
                // Unread report: nothing was confirmed, so the release asks.
                origin: confirmed_for.map_or(HeldOrigin::Composer, HeldOrigin::Confirmed),
            });
            return Ok(());
        }
        self.send_daemon_turn(turn, prompt.media, prompt.paste_segments, terminal)
            .await
    }

    /// Compacts the daemon session, then sends `submission` as the next
    /// queued message under the consent `omissions` records. The prompt is
    /// parked only once the daemon took the compaction: one it cannot start
    /// returns the prompt to the composer under the reason.
    pub(super) async fn compact_then_send_to_daemon(
        &mut self,
        submission: SendSubmission,
        omissions: SendOmissions,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let (_, prompt) = submission.into_daemon_turn();
        let compact = TurnPrompt::command("/compact".into(), "/compact".into());
        let started = if self.daemon_tool_sources_loading() {
            // The daemon would start it only once they connected, and the
            // TUI would wait for it meanwhile.
            Err(anyhow::anyhow!(
                "the session's MCP servers are still connecting"
            ))
        } else {
            self.request_daemon_turn(&compact, &[]).await
        };
        if let Err(error) = started {
            self.show_taken_daemon_views(terminal).await?;
            let ended = error
                .downcast_ref::<rho_protocol::ClientError>()
                .and_then(rho_protocol::ClientError::turn_ended);
            // A compaction that ended before it began has the daemon's own
            // rows to say how.
            if ended.is_none() {
                self.insert_entry(&Entry::Error(format!(
                    "could not compact before send: {error:#}"
                )));
            }
            self.restore_pending_prompt(prompt);
            self.set_status("send cancelled");
            return Ok(());
        }
        self.pending.park_compact_send(prompt, omissions);
        self.pending_input_changed();
        self.select_pending_recall_target();
        self.show_taken_daemon_views(terminal).await
    }
}
