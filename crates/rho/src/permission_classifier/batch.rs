//! Classifies several requests the agent made at once, such as the calls of
//! one codemode `call_tools` batch, with one review for all of them.
//!
//! Each request is screened on its own, exactly as a lone request is, because
//! the screen is cheap and a decision-model screen was measured only on single
//! requests. The requests the screen escalates are then reviewed in one model
//! request that asks one question per request, under [`BATCH_POLICY`]. A
//! batch of one escalation takes the single-request review unchanged.
//!
//! Siblings are concurrent, so none has seen another's result, and every
//! verdict still names one concrete capability: batching shares the review's
//! cost, not its judgment.

use std::path::Path;

use futures_util::future::join_all;
use rho_providers::model::Message;
use rho_sdk::{
    decision::{
        text::{questions_block, system_prompt, AnswerStyle},
        DecisionRequest,
    },
    model::context::estimate_text_tokens,
    ApprovalRequest, CancellationToken, ProviderRequestUsageRecording, SessionId,
};

use super::{
    batch_review_questions,
    classify::{
        ask, review, screen_step, text_model, CallScope, ClassifierModel, ClassifyRequest,
        ScreenStep, REVIEW_STAGE,
    },
    review_verdict,
    transcript::{render_with_pending_call, render_with_pending_calls, LabeledPending},
    ClassifierVerdict, TranscriptBudget, TranscriptOverBudget, BATCH_POLICY, CLASSIFIER_POLICY,
};

/// Usage ledger purpose of a batched review, apart from single reviews so
/// their latency and tokens can be compared.
const BATCH_REVIEW_PURPOSE: &str = "permission-classifier-batch-review";

/// One request of a [`BatchRequest`].
pub(crate) struct BatchMember<'a> {
    pub pending: &'a ApprovalRequest,
    /// ID of the call that asked, which the SDK attaches to the request in
    /// production; the eval supplies replayed IDs.
    pub call_id: Option<&'a str>,
}

/// Requests the agent made at once, with the history they share.
pub(crate) struct BatchRequest<'a> {
    pub history: &'a [Message],
    pub members: &'a [BatchMember<'a>],
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
    pub usage_recording: ProviderRequestUsageRecording,
}

impl BatchRequest<'_> {
    /// Member `index` as a lone request.
    fn member(&self, index: usize) -> ClassifyRequest<'_> {
        ClassifyRequest {
            history: self.history,
            pending: self.members[index].pending,
            cancellation: self.cancellation.clone(),
            session_id: self.session_id,
            workspace_path: self.workspace_path,
            usage_recording: self.usage_recording.clone(),
        }
    }

    fn scope(&self) -> CallScope<'_> {
        CallScope {
            cancellation: &self.cancellation,
            session_id: self.session_id,
            workspace_path: self.workspace_path,
            usage_recording: &self.usage_recording,
        }
    }
}

/// The question ID member `index` is asked under.
fn label(index: usize) -> String {
    format!("call_{}", index + 1)
}

impl ClassifierModel {
    /// Stage 1 for every member, concurrently. One step per member, in order.
    pub(crate) async fn screen_batch(&self, batch: &BatchRequest<'_>) -> Vec<ScreenStep> {
        let requests: Vec<_> = (0..batch.members.len())
            .map(|index| batch.member(index))
            .collect();
        join_all(requests.iter().zip(batch.members).map(|(request, member)| {
            screen_step(
                self.provider.as_ref(),
                &self.screen,
                self.budget,
                member.call_id,
                request,
            )
        }))
        .await
    }

    /// Stage 2 for the members at `members`: one review for all of them, or
    /// the single-request review when there is only one. One result per
    /// member, in order.
    pub(crate) async fn review_batch(
        &self,
        batch: &BatchRequest<'_>,
        members: &[usize],
    ) -> Vec<anyhow::Result<ClassifierVerdict>> {
        match members {
            [] => Vec::new(),
            [_] => self.review_each(batch, members).await,
            _ => match self.review_together(batch, members).await {
                Ok(verdicts) => verdicts.into_iter().map(Ok).collect(),
                // Every member shares the request, so every member fails.
                Err(error) => members.iter().map(|_| Err(copy_error(&error))).collect(),
            },
        }
    }

    /// Reviews each of `members` alone, in turn, as if the agent had asked
    /// for each on its own.
    pub(crate) async fn review_each(
        &self,
        batch: &BatchRequest<'_>,
        members: &[usize],
    ) -> Vec<anyhow::Result<ClassifierVerdict>> {
        let scope = batch.scope();
        let mut results = Vec::with_capacity(members.len());
        for &index in members {
            let member = &batch.members[index];
            let transcript = render_with_pending_call(
                batch.history,
                member.pending,
                member.call_id,
                self.budget,
            );
            results.push(match transcript {
                Ok(transcript) => {
                    review(self.provider.as_ref(), self.reasoning, &scope, &transcript).await
                }
                Err(error) => Err(error),
            });
        }
        results
    }

    /// One review request asking one question per member.
    async fn review_together(
        &self,
        batch: &BatchRequest<'_>,
        members: &[usize],
    ) -> anyhow::Result<Vec<ClassifierVerdict>> {
        let labels: Vec<String> = members.iter().map(|&index| label(index)).collect();
        let questions = batch_review_questions(&labels);
        let pendings: Vec<LabeledPending<'_>> = members
            .iter()
            .zip(&labels)
            .map(|(&index, label)| LabeledPending {
                label,
                pending: batch.members[index].pending,
                call_id: batch.members[index].call_id,
            })
            .collect();
        let budget = batch_budget(self.budget, &questions);
        let transcript = render_with_pending_calls(batch.history, &pendings, budget)?;
        let scope = batch.scope();
        let model = text_model(
            self.provider.as_ref(),
            &scope,
            BATCH_REVIEW_PURPOSE,
            REVIEW_STAGE.style,
            self.reasoning,
        );
        let decision = DecisionRequest::new(BATCH_POLICY, &transcript, &questions);
        let answers = ask(&model, scope.cancellation, decision).await?;
        answers
            .iter()
            .map(|answer| review_verdict(std::slice::from_ref(answer)))
            .collect()
    }
}

/// `budget`, which fits a single review, shrunk by how much more a batched
/// review's system prompt and questions take than a single review's.
fn batch_budget(
    budget: TranscriptBudget,
    questions: &[rho_sdk::decision::Question<'_>],
) -> TranscriptBudget {
    let tokens = |text: &str| estimate_text_tokens(text);
    let batch = tokens(&system_prompt(BATCH_POLICY))
        + tokens(&questions_block(questions, AnswerStyle::Reasoned));
    let single = tokens(&system_prompt(CLASSIFIER_POLICY))
        + tokens(&questions_block(
            REVIEW_STAGE.questions,
            AnswerStyle::Reasoned,
        ));
    budget.less(batch.saturating_sub(single))
}

/// A copy of a failed batch's error for each member. An over-budget error
/// stays typed, so production still shows its token counts.
fn copy_error(error: &anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<TranscriptOverBudget>() {
        Some(over_budget) => (*over_budget).into(),
        None => anyhow::anyhow!("{error:#}"),
    }
}
