//! Confirm a send when the conversation carries provider-native context the
//! active model cannot use.
//!
//! Model switches land immediately (see `context_handoff`); this gate moves the
//! warning to the next send, where the user can send anyway, compact first, or
//! not send at all.

use ratatui::DefaultTerminal;
use rho_sdk::model::handoff::HandoffReport;

use super::{
    App, ChatMedia, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending, InteractiveRuntime, PasteSegment, TurnPrompt,
};

pub(super) const ACTION_SEND: &str = "send";
pub(super) const ACTION_COMPACT_SEND: &str = "compact-send";
pub(super) const ACTION_DONT_SEND: &str = "dont-send";

#[derive(Debug)]
pub(super) struct PendingConfirmSend {
    turn: TurnPrompt,
    media: Vec<ChatMedia>,
    paste_segments: Vec<PasteSegment>,
    /// Whether compaction may run before the send; the compact option is only
    /// offered when this is set.
    can_compact: bool,
}

fn confirm_send_choice(
    target_label: &str,
    omissions: &HandoffReport,
    can_compact: bool,
) -> anyhow::Result<InlineChoice> {
    let blocks = omissions.omitted_provider_context;
    let kinds = omissions.omitted_kinds.join(", ");
    let mut options = vec![InlineChoiceOption::available(
        ACTION_SEND,
        '1',
        "Send anyway",
        format!(
            "{blocks} native block(s) will not be sent to {target_label}. Transcript and tool history remain."
        ),
    )];
    if can_compact {
        options.push(InlineChoiceOption::available(
            ACTION_COMPACT_SEND,
            '2',
            "Compact, then send",
            format!(
                "Summarize the conversation with {target_label}, then send. {blocks} native block(s) still will not be sent."
            ),
        ));
    }
    options.push(InlineChoiceOption::available(
        ACTION_DONT_SEND,
        '3',
        "Don't send",
        "Return the prompt to the composer.",
    ));
    InlineChoice::new(
        format!("Send to {target_label}?"),
        format!(
            "This conversation has {blocks} provider-native context block(s) ({kinds}). {target_label} cannot use them."
        ),
        options,
    )
}

impl App {
    /// Runs before every user-prompt turn start. When the live context holds
    /// provider-native blocks the current model cannot replay, the turn is
    /// parked behind a confirm-send modal. Returns the turn inputs when the
    /// send may proceed, `None` when the modal now owns it.
    pub(super) fn gate_send(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        agent: &mut InteractiveRuntime,
    ) -> Option<(TurnPrompt, Vec<ChatMedia>, Vec<PasteSegment>)> {
        if self.send_confirm_bypass {
            self.send_confirm_bypass = false;
            return Some((turn, media, paste_segments));
        }
        let target_identity = agent.provider_identity();
        let omissions = agent.provider_context_omissions(&target_identity);
        if !omissions.has_omissions() {
            return Some((turn, media, paste_segments));
        }

        let target_label = rho_providers::provider::model_reference(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        let can_compact = agent.can_compact();
        let choice = confirm_send_choice(&target_label, &omissions, can_compact)
            .inspect_err(|error| tracing::warn!(%error, "confirm-send choice unavailable"))
            .ok()?;
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ConfirmSend(Box::new(PendingConfirmSend {
                    turn,
                    media,
                    paste_segments,
                    can_compact,
                })),
                parent_picker: None,
            }));
        self.set_status("confirm send");
        None
    }

    pub(super) async fn resolve_send_confirm(
        &mut self,
        value: Option<&str>,
        pending: PendingConfirmSend,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let PendingConfirmSend {
            turn,
            media,
            paste_segments,
            can_compact,
        } = pending;
        match value {
            Some(ACTION_SEND) => {
                // The user just confirmed this send; do not re-gate it.
                self.send_confirm_bypass = true;
                self.run_turn_sequence_held(turn, media, paste_segments, terminal, agent)
                    .await?;
            }
            Some(ACTION_COMPACT_SEND) if can_compact => {
                match self.start_compact(agent, super::compact_work::CompactFollowUp::None) {
                    Ok(()) => {
                        // Compaction runs on the active model and the queued
                        // prompt restarts on its own, so arm the bypass to
                        // spare the user a second modal for this same send.
                        self.send_confirm_bypass = true;
                        self.queue_prompt(turn.model, turn.display, paste_segments, media)?;
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not compact before send: {err}"
                        )));
                        self.cancel_send(turn, media, paste_segments);
                    }
                }
            }
            _ => {
                self.cancel_send(turn, media, paste_segments);
            }
        }
        Ok(())
    }

    /// Don't send: unwind the gated turn back into the composer. Attachments
    /// cannot return once extracted, so only text and paste segments restore.
    fn cancel_send(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
    ) {
        self.restore_pending_prompt(super::QueuedPrompt {
            prompt: turn.model,
            display_prompt: turn.display,
            paste_segments,
            media: Vec::new(),
        });
        if media.is_empty() {
            self.set_status("send cancelled");
        } else {
            self.notify_status("send cancelled; attach the files again");
        }
    }
}

#[cfg(test)]
#[path = "send_confirm_tests.rs"]
mod tests;
