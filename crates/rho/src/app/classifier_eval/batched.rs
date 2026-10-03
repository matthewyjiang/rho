//! `rho __classifier_eval --batched`: screens each batch member alone, then
//! reviews the members that need a review twice, once each alone and once
//! together, and reports both verdicts per member. A flip between them is
//! what batching changed.

use std::time::Instant;

use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};
use serde::Serialize;

use super::{
    batch_cases::EvalBatch,
    cases::{CaseSource, Decision},
    elapsed_ms, input_digest, Outcome, ScreenReport,
};
use crate::permission_classifier::{BatchMember, BatchRequest, ClassifierModel, ScreenStep};

/// Which members a batch reviews.
#[derive(Clone, Copy)]
pub(super) enum ReviewScope {
    /// The ones production reviews: the screen escalated or failed.
    Escalated,
    /// Also the ones the screen allowed, so labeled members the screen
    /// allowed still test the review.
    All,
}

/// Classifies one batch: one report per member, in order.
pub(super) async fn classify_batch(
    model: &ClassifierModel,
    batch: &EvalBatch,
    scope: ReviewScope,
) -> Vec<MemberReport> {
    let session_id = SessionId::new();
    let members: Vec<BatchMember<'_>> = batch
        .members
        .iter()
        .map(|member| BatchMember {
            pending: &member.pending,
            call_id: Some(&member.call_id),
        })
        .collect();
    let request = BatchRequest {
        history: &batch.history,
        members: &members,
        cancellation: CancellationToken::new(),
        session_id: &session_id,
        workspace_path: &batch.workspace,
        usage_recording: ProviderRequestUsageRecording::default(),
    };

    let steps = model.screen_batch(&request).await;
    let reviewed: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, step)| match (step, scope) {
            (ScreenStep::Review { .. }, _) => true,
            // A transcript that did not render cannot be reviewed either.
            (ScreenStep::Done(trace), ReviewScope::All) => trace.result.is_ok(),
            (ScreenStep::Done(_), ReviewScope::Escalated) => false,
        })
        .map(|(index, _)| index)
        .collect();

    // Isolated first, one at a time, so neither run queues behind the other.
    let mut isolated = Vec::with_capacity(reviewed.len());
    for &index in &reviewed {
        let started = Instant::now();
        let result = model.review_each(&request, &[index]).await.remove(0);
        isolated.push(Outcome::of(result, elapsed_ms(started)));
    }
    let started = Instant::now();
    let together = model.review_batch(&request, &reviewed).await;
    let together_ms = elapsed_ms(started);
    let mut isolated = isolated.into_iter();
    let mut together = together.into_iter();

    batch
        .members
        .iter()
        .zip(steps)
        .enumerate()
        .map(|(index, (member, step))| {
            let (screen, done) = match step {
                ScreenStep::Done(trace) => (
                    ScreenReport::of(trace.screen, trace.screen_allow_probability),
                    Some(trace.result),
                ),
                ScreenStep::Review {
                    screen,
                    screen_allow_probability,
                    ..
                } => (ScreenReport::of(screen, screen_allow_probability), None),
            };
            let is_reviewed = reviewed.contains(&index);
            let (outcome, isolated) = if is_reviewed {
                let together = together.next().expect("one result per reviewed member");
                (
                    Outcome::of(together, together_ms),
                    isolated.next().expect("one result per reviewed member"),
                )
            } else {
                let result = done.expect("an unreviewed member's screen decided");
                let outcome = Outcome::of(result, /*latency_ms*/ 0);
                (outcome.clone(), outcome)
            };
            MemberReport {
                id: format!("{}/{}", batch.id, member.id),
                batch_id: batch.id.clone(),
                batch_size: reviewed.len(),
                source: batch.source,
                category: batch.category.clone(),
                label: member.label,
                pending: member.summary.clone(),
                input_digest: input_digest(
                    &batch.workspace,
                    &batch.history,
                    &member.call_id,
                    &member.summary,
                ),
                screen,
                reviewed: is_reviewed,
                outcome,
                isolated,
            }
        })
        .collect()
}

/// One batch member's result. Its flattened outcome is the batched review's,
/// or the screen's when the member was not reviewed.
#[derive(Serialize)]
pub(super) struct MemberReport {
    /// `<batch id>/<member id>`.
    id: String,
    batch_id: String,
    /// Members reviewed together in this member's batch.
    batch_size: usize,
    source: CaseSource,
    category: Option<String>,
    label: Option<Decision>,
    pending: String,
    input_digest: String,
    #[serde(flatten)]
    screen: ScreenReport,
    /// Whether a review decided, in both modes. When not, both outcomes are
    /// the screen's and take no review time.
    reviewed: bool,
    /// Latency is the whole batched review's, shared by its members.
    #[serde(flatten)]
    outcome: Outcome,
    /// The same member reviewed alone.
    isolated: Outcome,
}
