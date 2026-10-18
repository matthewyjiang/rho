//! Interactive plan approval records intent; the host installs policy after the turn.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use rho_sdk::{
    tool::{
        Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
        ToolSecurity,
    },
    HostChoice, HostInputRequest, HostQuestion, SelectionMode, ToolCallId,
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::permission::PermissionMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanExitDecision {
    pub(crate) target: PermissionMode,
    pub(crate) feedback: Option<String>,
}

/// How the turn that proposed a plan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProposalTurn {
    Completed,
    Unfinished,
}

/// What a host does with a decision once the proposing turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanExitResolution {
    /// Switch to this mode, then run the implementation turn.
    Apply(PermissionMode),
    /// Leave the mode alone and tell the user why.
    Keep(&'static str),
}

impl PlanExitDecision {
    /// The one policy every host applies: approval switches modes only when
    /// the proposing turn completed while the session is still in plan mode.
    pub(crate) fn resolve(
        &self,
        turn: ProposalTurn,
        current: PermissionMode,
    ) -> PlanExitResolution {
        if turn == ProposalTurn::Unfinished {
            return PlanExitResolution::Keep("plan approval discarded; turn did not complete");
        }
        if self.target == PermissionMode::Plan {
            return PlanExitResolution::Keep("plan not approved; permission mode stays plan");
        }
        if current != PermissionMode::Plan {
            return PlanExitResolution::Keep(
                "plan approval discarded; session is no longer in plan mode",
            );
        }
        PlanExitResolution::Apply(self.target)
    }
}

/// A one-shot intent shared by the tool and its interactive host, never a policy grant.
#[derive(Clone, Default)]
pub(crate) struct PlanExitSlot(Arc<Mutex<Option<PlanExitDecision>>>);

impl PlanExitSlot {
    pub(crate) fn set(&self, decision: PlanExitDecision) {
        *self.0.lock().expect("plan exit slot poisoned") = Some(decision);
    }

    pub(crate) fn take(&self) -> Option<PlanExitDecision> {
        self.0.lock().expect("plan exit slot poisoned").take()
    }
}

/// The approval prompt an interactive host draws, chosen when it installs
/// the tool; the tool set builds the matching [`PlanExitPrompt`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanExitHost {
    Questionnaire,
    Permission,
}

/// How the tool puts the approval question to the user.
pub(crate) enum PlanExitPrompt {
    /// A decision and optional-feedback questionnaire through the run's host
    /// input, which the TUI draws.
    Questionnaire,
    /// One permission prompt whose options are the approvable modes. ACP
    /// hosts answer it with `session/request_permission`, which every host
    /// implements, so the handoff does not depend on form elicitation.
    Permission(PlanPermissionSender),
}

/// One plan approval waiting on a permission-prompt host.
pub(crate) struct PlanPermissionRequest {
    /// The `exit_plan_mode` call, so the host shows the prompt on its card.
    pub(crate) call_id: ToolCallId,
    pub(crate) plan: String,
    /// The modes the user may approve into, in offer order.
    pub(crate) targets: Vec<PermissionMode>,
    /// Dropping it unanswered reports the call as cancelled.
    pub(crate) reply: oneshot::Sender<PlanPermissionAnswer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanPermissionAnswer {
    Approve(PermissionMode),
    KeepPlanning,
}

pub(crate) type PlanPermissionSender = mpsc::UnboundedSender<PlanPermissionRequest>;
pub(crate) type PlanPermissionReceiver = mpsc::UnboundedReceiver<PlanPermissionRequest>;

pub(crate) struct PlanExitTool {
    pub(crate) slot: Option<PlanExitSlot>,
    pub(crate) prompt: PlanExitPrompt,
    pub(crate) classifier_configured: Arc<AtomicBool>,
}

/// The modes a plan may be approved into. Auto needs the classifier model,
/// so it is offered only when one is configured.
fn approval_targets(classifier_configured: bool) -> Vec<PermissionMode> {
    let mut targets = vec![
        PermissionMode::AllowEdits,
        PermissionMode::Supervised,
        PermissionMode::Bypass,
    ];
    if classifier_configured {
        targets.push(PermissionMode::Auto);
    }
    targets
}

/// The label of the choice that approves into `mode`, shared by every host.
pub(crate) fn approval_label(mode: PermissionMode) -> String {
    format!("Approve → {}", mode.label().to_lowercase())
}

/// The decision for a questionnaire answer. Only an offered mode approves;
/// anything else, including "keep_planning", stays in plan mode.
fn decision(answer: &str, feedback: Option<&str>, targets: &[PermissionMode]) -> PlanExitDecision {
    let target = targets
        .iter()
        .copied()
        .find(|mode| mode.as_str() == answer)
        .unwrap_or(PermissionMode::Plan);
    PlanExitDecision {
        target,
        feedback: feedback
            .filter(|text| !text.trim().is_empty() && *text != "no_feedback")
            .map(str::to_owned),
    }
}

fn decision_output(decision: &PlanExitDecision) -> ToolOutput {
    if decision.target == PermissionMode::Plan {
        let feedback = decision.feedback.as_deref().unwrap_or("Keep planning.");
        ToolOutput::text(format!(
            "Not approved. {feedback} Revise the plan; remain in Plan mode."
        ))
    } else {
        let mut output = format!(
            "Plan approved. After this turn the host switches to {} and re-prompts you to implement the approved plan. The mode is not saved; a later session starts in the saved mode. End this turn now with a brief acknowledgement and no more tool calls.",
            decision.target.as_str()
        );
        if let Some(feedback) = &decision.feedback {
            output.push_str(&format!("\n\nUser feedback:\n{feedback}"));
        }
        ToolOutput::text(output)
    }
}

fn host_error(error: rho_sdk::Error) -> ToolError {
    match error {
        rho_sdk::Error::Cancelled => ToolError::cancelled(),
        error => ToolError::new(ToolErrorKind::Execution, error.to_string()),
    }
}

/// Asks the TUI's decision and feedback questionnaire.
async fn ask_questionnaire(
    context: &ToolContext,
    targets: &[PermissionMode],
) -> Result<PlanExitDecision, ToolError> {
    let mut choices = targets
        .iter()
        .map(|&mode| HostChoice::new(mode.as_str(), approval_label(mode)))
        .collect::<Vec<_>>();
    choices.push(HostChoice::new("keep_planning", "Keep planning"));
    let question = HostQuestion::new(
        "decision",
        "Plan ready. How should Rho continue?",
        choices,
        SelectionMode::One,
    )
    .map_err(host_error)?
    .header("Decision")
    .help("Review the plan in the tool card above. Approval lasts until Rho exits; it is not saved, and the next launch starts in your saved mode.");
    // Feedback can contain arbitrary text, including reserved decision values.
    // Only the closed decision question authorizes a permission change.
    let feedback = HostQuestion::new(
        "feedback",
        "Optional feedback for the plan",
        vec![HostChoice::new("no_feedback", "No feedback")],
        SelectionMode::One,
    )
    .map_err(host_error)?
    .header("Feedback")
    .allow_other()
    .optional();
    let request = HostInputRequest::questionnaire("Plan approval", vec![question, feedback])
        .map_err(host_error)?;
    let response = context
        .request_host_input(request)
        .await
        .map_err(host_error)?;
    let answer = response
        .answers()
        .get("decision")
        .and_then(|answers| answers.first())
        .ok_or_else(|| {
            ToolError::new(ToolErrorKind::Execution, "plan approval answer is missing")
        })?;
    let feedback = response
        .answers()
        .get("feedback")
        .and_then(|answers| answers.first());
    Ok(decision(answer, feedback.map(String::as_str), targets))
}

/// Asks a permission-prompt host, which offers no feedback field: the user
/// replies in their next message instead.
async fn ask_permission(
    sender: &PlanPermissionSender,
    call_id: ToolCallId,
    plan: &str,
    targets: Vec<PermissionMode>,
    context: &ToolContext,
) -> Result<PlanExitDecision, ToolError> {
    let (reply, answer) = oneshot::channel();
    sender
        .send(PlanPermissionRequest {
            call_id,
            plan: plan.to_owned(),
            targets: targets.clone(),
            reply,
        })
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::Execution,
                "the host stopped answering plan approvals",
            )
        })?;
    let answer = tokio::select! {
        biased;
        () = context.cancellation().cancelled() => return Err(ToolError::cancelled()),
        answer = answer => answer.map_err(|_| ToolError::cancelled())?,
    };
    // The one check that an answer names an offered mode.
    let target = match answer {
        PlanPermissionAnswer::Approve(mode) if targets.contains(&mode) => mode,
        PlanPermissionAnswer::Approve(_) | PlanPermissionAnswer::KeepPlanning => {
            PermissionMode::Plan
        }
    };
    Ok(PlanExitDecision {
        target,
        feedback: None,
    })
}

impl Tool for PlanExitTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: "exit_plan_mode".into(),
            description: "Present your completed markdown plan for approval. Approval applies only after this turn ends; do not implement or make more tool calls after approval.".into(),
            input_schema: json!({"type":"object","properties":{"plan":{"type":"string","description":"The complete markdown implementation plan"}},"required":["plan"],"additionalProperties":false}),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let call_id = invocation.id().clone();
            let arguments = invocation.into_arguments();
            let plan = arguments
                .get("plan")
                .and_then(serde_json::Value::as_str)
                .filter(|plan| !plan.trim().is_empty())
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "plan must be non-empty markdown",
                    )
                })?;
            let Some(slot) = &self.slot else {
                return Ok(ToolOutput::text(format!(
                    "This host cannot switch permission modes. Stop and report the plan to the user; do not implement it.\n\n{plan}"
                )).with_structured_content(json!({"status":"host_unavailable"})));
            };
            // A revision supersedes earlier intent, even if its new question is cancelled.
            slot.take();
            let targets = approval_targets(self.classifier_configured.load(Ordering::Relaxed));
            let decision = match &self.prompt {
                PlanExitPrompt::Questionnaire => ask_questionnaire(&context, &targets).await?,
                PlanExitPrompt::Permission(sender) => {
                    ask_permission(sender, call_id, plan, targets, &context).await?
                }
            };
            let output = decision_output(&decision);
            slot.set(decision);
            Ok(output)
        })
    }
}

#[cfg(test)]
#[path = "plan_exit_tests.rs"]
mod tests;
