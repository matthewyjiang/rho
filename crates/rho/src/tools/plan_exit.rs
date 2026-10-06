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
    HostChoice, HostInputRequest, HostQuestion, SelectionMode,
};
use serde_json::json;

use crate::permission::PermissionMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanExitDecision {
    pub(crate) target: PermissionMode,
    pub(crate) feedback: Option<String>,
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

pub(crate) struct PlanExitTool {
    pub(crate) slot: Option<PlanExitSlot>,
    pub(crate) classifier_configured: Arc<AtomicBool>,
}

fn decision(answer: &str, feedback: Option<&str>, classifier_configured: bool) -> PlanExitDecision {
    let target = match answer {
        "allow_edits" => PermissionMode::AllowEdits,
        "supervised" => PermissionMode::Supervised,
        "bypass" => PermissionMode::Bypass,
        "auto" if classifier_configured => PermissionMode::Auto,
        _ => PermissionMode::Plan,
    };
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
            "Plan approved. After this turn the host switches to {} and re-prompts you to implement the approved plan. The mode lasts until Rho exits; it is not saved, and the next launch starts in your saved mode. End this turn now with a brief acknowledgement and no more tool calls.",
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
            let classifier_configured = self.classifier_configured.load(Ordering::Relaxed);
            let mut choices = vec![
                HostChoice::new("allow_edits", "Approve → allow edits"),
                HostChoice::new("supervised", "Approve → supervised"),
                HostChoice::new("bypass", "Approve → bypass"),
            ];
            if classifier_configured {
                choices.push(HostChoice::new("auto", "Approve → auto"));
            }
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
            let request =
                HostInputRequest::questionnaire("Plan approval", vec![question, feedback])
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
            let decision = decision(answer, feedback.map(String::as_str), classifier_configured);
            let output = decision_output(&decision);
            slot.set(decision);
            Ok(output)
        })
    }
}

#[cfg(test)]
#[path = "plan_exit_tests.rs"]
mod tests;
