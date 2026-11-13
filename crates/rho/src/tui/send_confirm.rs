//! Confirm a send when the conversation carries provider-native context the
//! active model cannot use.
//!
//! A [`SendSubmission`] owns the exact prompt while it moves through
//! confirmation and optional compaction. Approval is scoped to that value
//! and the model identity shown by the confirmation. The only state a later
//! send can consume is an explicit "Send for this session" acknowledgement,
//! which [`SendOmissions::covers`] scopes to one session, one target model,
//! and the exact native blocks the user was warned about.
//!
//! The daemon session holds the conversation, so sends pass the gate in
//! [`daemon`], on what the daemon reports.

use std::collections::BTreeSet;

use crate::tui::DefaultTerminal;
use rho_sdk::model::{handoff::HandoffReport, ModelIdentity};
use rho_sdk::SessionId;

mod daemon;

use super::{
    App, ChatMedia, ComposerMode, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending, PasteSegment, TurnPrompt,
};

pub(super) const ACTION_SEND: &str = "send";
pub(super) const ACTION_SEND_SESSION: &str = "send-session";
pub(super) const ACTION_COMPACT_SEND: &str = "compact-send";
pub(super) const ACTION_DONT_SEND: &str = "dont-send";

/// The provider-native blocks one session cannot send to one target. A
/// confirmation shows one of these; "Send for this session" keeps it on the
/// app so later sends can skip an unchanged confirmation.
#[derive(Debug)]
pub(super) struct SendOmissions {
    session_id: SessionId,
    target: ModelIdentity,
    /// Content fingerprints of the omitted blocks.
    blocks: BTreeSet<u64>,
}

impl SendOmissions {
    /// Whether consent to omit `self` also covers `current`: the same session
    /// and target, and no omitted block the user was not already warned about.
    /// Compaction only drops blocks, so it never invalidates consent.
    fn covers(&self, current: &Self) -> bool {
        self.session_id == current.session_id
            && self.target == current.target
            && current.blocks.is_subset(&self.blocks)
    }
}

/// A prompt on its way to the session, plus the model identity a
/// confirmation approved it for.
pub(super) struct SendSubmission {
    turn: TurnPrompt,
    media: Vec<ChatMedia>,
    paste_segments: Vec<PasteSegment>,
    approved_for: Option<ModelIdentity>,
    /// Consent given for this prompt before the session compacted for it;
    /// see [`daemon`].
    compact_consent: Option<SendOmissions>,
}

impl SendSubmission {
    pub(super) fn turn(
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
    ) -> Self {
        Self {
            turn,
            media,
            paste_segments,
            approved_for: None,
            compact_consent: None,
        }
    }

    pub(in crate::tui) fn approve_for(mut self, identity: ModelIdentity) -> Self {
        self.approved_for = Some(identity);
        self
    }

    fn is_approved_for(&self, identity: &ModelIdentity) -> bool {
        self.approved_for.as_ref() == Some(identity)
    }
}

pub(super) struct PendingConfirmSend {
    submission: SendSubmission,
    /// What the modal shows; the confirmed model identity is its target.
    omissions: SendOmissions,
    /// Whether compaction may run before the send; the compact option is only
    /// offered when this is set.
    can_compact: bool,
}

impl std::fmt::Debug for PendingConfirmSend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingConfirmSend")
            .field("target", &self.omissions.target)
            .field("can_compact", &self.can_compact)
            .finish_non_exhaustive()
    }
}

fn confirm_send_choice(
    target_label: &str,
    omissions: &HandoffReport,
    can_compact: bool,
) -> anyhow::Result<InlineChoice> {
    let blocks = omissions.omitted_provider_context;
    let kinds = omissions.omitted_kinds.join(", ");
    let mut shortcut = '1';
    let mut options = vec![InlineChoiceOption::available(
        ACTION_SEND,
        shortcut,
        "Send anyway",
        format!(
            "{blocks} native block(s) will not be sent to {target_label}. Transcript and tool history remain."
        ),
    )];
    shortcut = next_shortcut(shortcut);
    if can_compact {
        options.push(InlineChoiceOption::available(
            ACTION_COMPACT_SEND,
            shortcut,
            "Compact, then send",
            format!(
                "Summarize the conversation with {target_label}, then send. {blocks} native block(s) still will not be sent."
            ),
        ));
        shortcut = next_shortcut(shortcut);
    }
    options.push(InlineChoiceOption::available(
        ACTION_DONT_SEND,
        shortcut,
        "Don't send",
        "Return the prompt to the composer.",
    ));
    shortcut = next_shortcut(shortcut);
    // Last, so the established shortcuts keep their meaning; session-wide
    // consent is only selectable once its explanation is fully visible.
    options.push(
        InlineChoiceOption::available(
            ACTION_SEND_SESSION,
            shortcut,
            "Send for this session",
            format!(
                "Don't ask again for {target_label} in this session unless new native blocks would be omitted."
            ),
        )
        .require_full_visibility(),
    );
    InlineChoice::new(
        format!("Send to {target_label}?"),
        format!(
            "This conversation has {blocks} provider-native context block(s) ({kinds}). {target_label} cannot use them."
        ),
        options,
    )
}

impl App {
    /// Opens the confirmation that owns `pending` until the person answers
    /// it; `report` is what it says the model will not receive.
    fn confirm_send(&mut self, pending: PendingConfirmSend, report: &HandoffReport) {
        let target_label = rho_providers::provider::model_reference(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        let choice = match confirm_send_choice(&target_label, report, pending.can_compact) {
            Ok(choice) => choice,
            Err(error) => {
                tracing::warn!(%error, "confirm-send choice unavailable");
                return;
            }
        };
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ConfirmSend(Box::new(pending)),
                parent_picker: None,
            }));
        self.set_status("confirm send");
    }

    pub(super) async fn resolve_send_confirm(
        &mut self,
        value: Option<&str>,
        pending: PendingConfirmSend,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let PendingConfirmSend {
            submission,
            omissions,
            can_compact,
        } = pending;
        match value {
            Some(ACTION_SEND) => {
                self.submit_daemon_submission(submission.approve_for(omissions.target), terminal)
                    .await?;
            }
            Some(ACTION_SEND_SESSION) => {
                let submission = submission.approve_for(omissions.target.clone());
                self.acknowledged_send_omissions = Some(omissions);
                self.submit_daemon_submission(submission, terminal).await?;
            }
            Some(ACTION_COMPACT_SEND) if can_compact => {
                self.compact_then_send_to_daemon(submission, omissions, terminal)
                    .await?;
            }
            _ => self.cancel_send_submission(submission),
        }
        Ok(())
    }

    /// Returns a declined prompt to the composer.
    fn cancel_send_submission(&mut self, submission: SendSubmission) {
        let (_, prompt) = submission.into_daemon_turn();
        self.restore_pending_prompt(prompt);
        self.set_status("send cancelled");
    }
}

fn next_shortcut(current: char) -> char {
    char::from_u32(u32::from(current) + 1).unwrap_or(current)
}

#[cfg(test)]
#[path = "send_confirm_tests.rs"]
mod tests;
