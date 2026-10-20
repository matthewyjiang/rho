//! Confirm a send when the conversation carries provider-native context the
//! active model cannot use.
//!
//! A [`SendSubmission`] owns the exact provider-starting work while it moves
//! through confirmation and optional compaction. Approval is scoped to that
//! value and the model identity shown by the confirmation. The only state a
//! later send can consume is an explicit "Send for this session"
//! acknowledgement, which [`SendOmissions::covers`] scopes to one session,
//! one target, and the exact native blocks the user was shown.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::tui::DefaultTerminal;
use rho_providers::model::Message;
use rho_sdk::model::{
    handoff::{self, HandoffReport},
    ModelIdentity, ProviderContextBlock,
};
use rho_sdk::SessionId;

use super::{
    prompt_turn::FailedTurn, subagent_delivery::TurnBoundaryDelivery, App, ChatMedia, ComposerMode,
    Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending,
    InteractiveRuntime, PasteSegment, TurnOutcome, TurnPrompt,
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
    fn new(session_id: SessionId, target: ModelIdentity, history: &[Message]) -> Self {
        let blocks = handoff::omitted_message_context(history, &target)
            .map(block_fingerprint)
            .collect();
        Self {
            session_id,
            target,
            blocks,
        }
    }

    /// Whether consent to omit `self` also covers `current`: the same session
    /// and target, and no omitted block the user has not already been shown.
    /// Compaction only drops blocks, so it never invalidates consent.
    fn covers(&self, current: &Self) -> bool {
        self.session_id == current.session_id
            && self.target == current.target
            && current.blocks.is_subset(&self.blocks)
    }
}

/// In-process identity of one native block. A collision could only skip a
/// warning, never change what is sent.
fn block_fingerprint(block: &ProviderContextBlock) -> u64 {
    let mut hasher = DefaultHasher::new();
    block.identity.hash(&mut hasher);
    block.kind.hash(&mut hasher);
    block.position.hash(&mut hasher);
    block.data.to_string().hash(&mut hasher);
    hasher.finish()
}

pub(super) enum SendPayload {
    Turn {
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        origin: TurnOrigin,
    },
    GoalRetry(FailedTurn),
    TurnBoundary(TurnBoundaryDelivery),
}

/// Origin of a submitted turn and the state transition to apply on cancellation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TurnOrigin {
    /// Return the exact user-authored prompt to the composer.
    User,
    /// Remove the goal that was created before its first turn was gated.
    InitialGoal,
    /// Put a blocked goal back into its pre-verification state.
    GoalResume,
    /// Drop a model-generated continuation; it was never composer input.
    GoalContinuation,
}

#[derive(Clone, Copy)]
enum CancellationSource {
    DirectConfirmation,
    Compact,
}

/// Move-only provider work plus authorization for one exact model identity.
pub(super) struct SendSubmission {
    payload: SendPayload,
    approved_for: Option<ModelIdentity>,
    allow_auto_compact: bool,
}

/// A submission released by the omission gate. Keeping this wrapper distinct
/// makes all provider-starting APIs require proof of the centralized policy.
pub(super) struct AuthorizedSendSubmission(SendSubmission);

/// Proof that omission policy admitted a provider start for one identity.
pub(super) struct SendAuthorization(ModelIdentity);

impl SendAuthorization {
    pub(super) fn matches(&self, identity: &ModelIdentity) -> bool {
        &self.0 == identity
    }
}

impl SendSubmission {
    pub(super) fn turn(
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
    ) -> Self {
        Self::turn_with_origin(turn, media, paste_segments, TurnOrigin::User)
    }

    pub(super) fn initial_goal(turn: TurnPrompt, media: Vec<ChatMedia>) -> Self {
        Self::turn_with_origin(turn, media, Vec::new(), TurnOrigin::InitialGoal)
    }

    pub(super) fn goal_resume(turn: TurnPrompt) -> Self {
        Self::turn_with_origin(turn, Vec::new(), Vec::new(), TurnOrigin::GoalResume)
    }

    pub(super) fn goal_continuation(turn: TurnPrompt) -> Self {
        Self::turn_with_origin(turn, Vec::new(), Vec::new(), TurnOrigin::GoalContinuation)
    }

    fn turn_with_origin(
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        origin: TurnOrigin,
    ) -> Self {
        Self {
            payload: SendPayload::Turn {
                turn,
                media,
                paste_segments,
                origin,
            },
            approved_for: None,
            allow_auto_compact: true,
        }
    }

    pub(super) fn goal_retry(failed_turn: FailedTurn) -> Self {
        Self {
            payload: SendPayload::GoalRetry(failed_turn),
            approved_for: None,
            allow_auto_compact: false,
        }
    }

    pub(super) fn turn_boundary(delivery: TurnBoundaryDelivery) -> Self {
        Self {
            payload: SendPayload::TurnBoundary(delivery),
            approved_for: None,
            allow_auto_compact: false,
        }
    }

    fn approve_for(mut self, identity: ModelIdentity) -> Self {
        self.approved_for = Some(identity);
        self
    }

    fn is_approved_for(&self, identity: &ModelIdentity) -> bool {
        self.approved_for.as_ref() == Some(identity)
    }

    fn after_compact(mut self) -> Self {
        self.allow_auto_compact = false;
        self
    }

    fn into_cancelled_payload(self) -> SendPayload {
        self.payload
    }

    #[cfg(test)]
    pub(super) fn allows_auto_compact(&self) -> bool {
        self.allow_auto_compact
    }

    #[cfg(test)]
    pub(super) fn turn_display(&self) -> Option<&str> {
        match &self.payload {
            SendPayload::Turn { turn, .. } => Some(&turn.display),
            SendPayload::GoalRetry(_) | SendPayload::TurnBoundary(_) => None,
        }
    }
}

impl AuthorizedSendSubmission {
    pub(super) fn into_authorized(self) -> (SendPayload, SendAuthorization, bool) {
        let SendSubmission {
            payload,
            approved_for,
            allow_auto_compact,
        } = self.0;
        let identity = approved_for.expect("the omission gate always records its identity");
        (payload, SendAuthorization(identity), allow_auto_compact)
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
            .field("confirmation_identity", &self.omissions.target)
            .field("can_compact", &self.can_compact)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl PendingConfirmSend {
    pub(super) fn for_test() -> Self {
        Self {
            submission: SendSubmission::turn(
                TurnPrompt::standard("model body".into(), "display body".into()),
                Vec::new(),
                Vec::new(),
            ),
            omissions: SendOmissions::new(
                SessionId::new(),
                ModelIdentity::new("openai", "responses", "gpt-5"),
                &[],
            ),
            can_compact: false,
        }
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
    options.push(InlineChoiceOption::available(
        ACTION_SEND_SESSION,
        shortcut,
        "Send for this session",
        "Don't ask again in this session unless the model or omitted blocks change.",
    ));
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
    InlineChoice::new(
        format!("Send to {target_label}?"),
        format!(
            "This conversation has {blocks} provider-native context block(s) ({kinds}). {target_label} cannot use them."
        ),
        options,
    )
}

impl App {
    /// The single omission-policy gate for every provider-starting main-session
    /// turn. Returns ownership only when no confirmation is needed, this exact
    /// submission was approved for the still-active model identity, or a
    /// session acknowledgement covers the current omissions.
    pub(super) fn gate_send(
        &mut self,
        submission: SendSubmission,
        agent: &mut InteractiveRuntime,
    ) -> Option<AuthorizedSendSubmission> {
        let target_identity = agent.provider_identity();
        if submission.is_approved_for(&target_identity) {
            return Some(AuthorizedSendSubmission(submission));
        }
        let history = agent.history();
        let report = handoff::report_message_omissions(&history, &target_identity);
        if !report.has_omissions() {
            return Some(AuthorizedSendSubmission(
                submission.approve_for(target_identity),
            ));
        }
        let omissions = SendOmissions::new(agent.session_id().clone(), target_identity, &history);
        if self
            .acknowledged_send_omissions
            .as_ref()
            .is_some_and(|acknowledged| acknowledged.covers(&omissions))
        {
            return Some(AuthorizedSendSubmission(
                submission.approve_for(omissions.target),
            ));
        }

        let target_label = rho_providers::provider::model_reference(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        let can_compact = agent.can_compact();
        let choice = confirm_send_choice(&target_label, &report, can_compact)
            .inspect_err(|error| tracing::warn!(%error, "confirm-send choice unavailable"))
            .ok()?;
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ConfirmSend(Box::new(PendingConfirmSend {
                    submission,
                    omissions,
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
            submission,
            omissions,
            can_compact,
        } = pending;
        match value {
            Some(ACTION_SEND) => {
                self.start_approved_submission(
                    submission.approve_for(omissions.target),
                    terminal,
                    agent,
                )
                .await?;
            }
            Some(ACTION_SEND_SESSION) => {
                let submission = submission.approve_for(omissions.target.clone());
                self.acknowledged_send_omissions = Some(omissions);
                self.start_approved_submission(submission, terminal, agent)
                    .await?;
            }
            Some(ACTION_COMPACT_SEND) if can_compact => {
                let submission = submission.approve_for(omissions.target).after_compact();
                if let Err((err, submission)) = self.start_compact_send(agent, Box::new(submission))
                {
                    self.insert_entry(&Entry::Error(format!(
                        "could not compact before send: {err}"
                    )));
                    self.cancel_send_submission(*submission, agent);
                }
            }
            _ => self.cancel_send_submission(submission, agent),
        }
        Ok(())
    }

    /// Starts a previously approved continuation. Calling the central gate
    /// again is intentional: a model change while the modal or compact job was
    /// active invalidates the old identity-scoped approval.
    pub(super) async fn start_approved_submission(
        &mut self,
        submission: SendSubmission,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(submission) = self.gate_send(submission, agent) else {
            return Ok(());
        };
        let (payload, authorization, allow_auto_compact) = submission.into_authorized();
        match payload {
            SendPayload::Turn {
                turn,
                media,
                paste_segments,
                origin,
            } => match origin {
                TurnOrigin::User => {
                    if allow_auto_compact {
                        self.run_turn_sequence_after_send_gate(
                            turn,
                            media,
                            paste_segments,
                            authorization,
                            terminal,
                            agent,
                        )
                        .await
                    } else {
                        self.run_turn_sequence_without_auto_compact(
                            turn,
                            media,
                            authorization,
                            terminal,
                            agent,
                        )
                        .await
                    }
                }
                TurnOrigin::InitialGoal | TurnOrigin::GoalContinuation => {
                    let outcome = self
                        .run_prompt_turn(turn, media, authorization, terminal, agent)
                        .await?;
                    self.resume_goal_after_confirmed_turn(outcome, terminal, agent)
                        .await
                }
                TurnOrigin::GoalResume => {
                    let outcome = self
                        .run_prompt_turn(turn, media, authorization, terminal, agent)
                        .await?;
                    self.finish_goal_resumption_turn(outcome.kind());
                    self.resume_goal_after_confirmed_turn(outcome, terminal, agent)
                        .await
                }
            },
            SendPayload::GoalRetry(failed_turn) => {
                let outcome = self
                    .retry_failed_prompt_turn(failed_turn, authorization, terminal, agent)
                    .await?;
                self.resume_goal_after_confirmed_turn(outcome, terminal, agent)
                    .await
            }
            SendPayload::TurnBoundary(delivery) => {
                let outcome = self
                    .run_turn_boundary_prompt_turn(delivery, authorization, terminal, agent)
                    .await?;
                self.resume_goal_after_confirmed_turn(outcome, terminal, agent)
                    .await
            }
        }
    }

    async fn resume_goal_after_confirmed_turn(
        &mut self,
        outcome: TurnOutcome,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let outcome_kind = outcome.kind();
        let pending_retries = match outcome {
            TurnOutcome::Failed(failed_turn) => std::collections::VecDeque::from([*failed_turn]),
            TurnOutcome::Completed | TurnOutcome::Interrupted | TurnOutcome::Cancelled => {
                std::collections::VecDeque::new()
            }
        };
        if super::goal_command::should_resume_goal_after_turn(
            outcome_kind,
            self.goal.as_ref().map(super::GoalState::loop_state),
            self.should_quit,
        ) {
            self.continue_goal(terminal, agent, pending_retries).await?;
        }
        Ok(())
    }

    pub(super) fn cancel_send_submission(
        &mut self,
        submission: SendSubmission,
        agent: &mut InteractiveRuntime,
    ) {
        self.cancel_send_submission_from(submission, CancellationSource::DirectConfirmation, agent);
    }

    pub(super) fn cancel_compact_send_submission(
        &mut self,
        submission: SendSubmission,
        agent: &mut InteractiveRuntime,
    ) {
        self.cancel_send_submission_from(submission, CancellationSource::Compact, agent);
    }

    fn cancel_send_submission_from(
        &mut self,
        submission: SendSubmission,
        source: CancellationSource,
        agent: &mut InteractiveRuntime,
    ) {
        match submission.into_cancelled_payload() {
            SendPayload::Turn {
                turn,
                media,
                paste_segments,
                origin,
            } => {
                let prompt = super::QueuedPrompt {
                    prompt: turn.model,
                    display_prompt: turn.display,
                    paste_segments,
                    media,
                };
                self.apply_turn_cancellation(origin, prompt, source);
            }
            SendPayload::GoalRetry(_) => self.set_status("goal retry cancelled"),
            SendPayload::TurnBoundary(delivery) => {
                self.restore_turn_boundary_batch(agent, delivery.batch);
                self.set_status("send cancelled");
            }
        }
    }

    fn apply_turn_cancellation(
        &mut self,
        origin: TurnOrigin,
        prompt: super::QueuedPrompt,
        source: CancellationSource,
    ) {
        match origin {
            TurnOrigin::InitialGoal => {
                self.goal = None;
            }
            TurnOrigin::GoalResume => {
                if let Some(goal) = self.goal.as_mut() {
                    goal.interrupt_verification();
                }
            }
            TurnOrigin::User | TurnOrigin::GoalContinuation => {}
        }

        if matches!(origin, TurnOrigin::GoalContinuation) {
            self.set_status("goal continuation cancelled");
            return;
        }

        // Goal-owned turns carry synthetic model prompts whose display text is
        // only a command label. If a compact-time draft occupies the composer,
        // putting that synthetic prompt into the ordinary editable queue would
        // erase its origin and later run it as a normal user message. Roll back
        // the goal state above and leave the newer draft intact instead.
        if matches!(source, CancellationSource::Compact)
            && !self.composer_available_for_pending_edit()
            && !matches!(origin, TurnOrigin::User)
        {
            self.set_status("goal send cancelled; current draft preserved");
            return;
        }

        match source {
            CancellationSource::DirectConfirmation => self.restore_pending_prompt(prompt),
            CancellationSource::Compact if self.composer_available_for_pending_edit() => {
                self.restore_pending_prompt(prompt);
            }
            CancellationSource::Compact => {
                self.pending.push_follow_up_front(prompt);
                self.pending_input_changed();
                self.select_pending_recall_target();
                self.set_status("send cancelled; prompt parked in pending input");
                return;
            }
        }
        self.set_status("send cancelled");
    }
}

fn next_shortcut(current: char) -> char {
    char::from_u32(u32::from(current) + 1).unwrap_or(current)
}

#[cfg(test)]
#[path = "send_confirm_tests.rs"]
mod tests;
