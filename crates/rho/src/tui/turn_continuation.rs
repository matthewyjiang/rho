//! Successful turn completion owns follow-ups, regardless of the turn driver.
//!
//! Goal evaluation and idle background delivery only see the outcome after the
//! queued follow-ups have run. Failed goal turns retain their driver's retry
//! handling; cancellation and interruption never auto-continue.

use super::{App, DefaultTerminal, InteractiveRuntime, PromptTurnRequest, TurnOutcome, TurnPrompt};
use crate::tui::send_confirm::{SendAuthorization, SendPayload, SendSubmission};

impl App {
    pub(super) async fn run_prompt_turn_request(
        &mut self,
        request: PromptTurnRequest,
        authorization: SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        let mut outcome = self
            .run_single_prompt_turn_request(request, authorization, terminal, agent)
            .await?;
        while matches!(outcome, TurnOutcome::Completed)
            && !self.should_quit
            && !self.input_ui.composer().blocks_auto_continue()
        {
            let Some(prompt) = self.pending.pop_follow_up() else {
                break;
            };
            self.pending_input_changed();
            self.select_pending_recall_target();
            let submission = SendSubmission::turn(
                TurnPrompt::standard(prompt.prompt, prompt.display_prompt),
                prompt.media,
                prompt.paste_segments,
            );
            let Some(submission) = self.gate_send(submission, agent) else {
                // Confirmation owns continuation from here. Approval drains the
                // remaining queue only if its turn succeeds; rejection or failure
                // must leave it parked, without an independent idle-runner arm.
                return Ok(outcome);
            };
            let (payload, authorization, _allow_auto_compact) = submission.into_authorized();
            let SendPayload::Turn { turn, media, .. } = payload else {
                unreachable!("queued prompts are turn submissions");
            };
            outcome = self
                .run_single_prompt_turn_request(
                    PromptTurnRequest::New {
                        prompt: turn,
                        media,
                    },
                    authorization,
                    terminal,
                    agent,
                )
                .await?;
        }
        // An independent overlay may defer continuation before a prompt reaches
        // the send gate. Resume via idle once that overlay releases the composer.
        if matches!(outcome, TurnOutcome::Completed)
            && !self.should_quit
            && !self.pending.queued_prompts().is_empty()
            && self.start_follow_ups.is_none()
        {
            self.start_follow_ups = Some(crate::tui::compact_work::ReadyFollowUp::Queued {
                allow_auto_compact: true,
            });
        }
        Ok(outcome)
    }
}
