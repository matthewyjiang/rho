//! A daemon input request as the prompt the in-process TUI shows for the
//! same ask, and the person's reply as the daemon's answer (#1667).
//!
//! - An approval is the approval prompt, and its decision the matching
//!   answer.
//! - A plan approval is the plan-approval questionnaire's decision
//!   question, offering the request's modes this rho knows. The daemon
//!   takes no feedback with it, so the feedback field is left out.
//! - Questions are the questionnaire. A timeout's fallback answers show
//!   with its countdown, but the form never submits them: the daemon does.

use rho_protocol::{
    self as wire, Ask, DefaultStyle, InputAnswer, InputRequest, InputTimeout, QuestionAsk,
    Selection,
};
use rho_sdk::{
    ApprovalDecision, DefaultSelection, HostChoice, HostInputRequest, HostInputResponse,
    HostQuestion, SelectionMode,
};
use serde_json::Value;

use super::approval::ApprovalPrompt;
use crate::{permission::PermissionMode, tools::plan_exit};

/// How the TUI puts a daemon input request to the person.
pub(super) enum DaemonPrompt {
    Approval(ApprovalPrompt),
    PlanApproval {
        form: HostInputRequest,
        /// The modes it offers, in the request's order.
        modes: Vec<PermissionMode>,
    },
    Questions(HostInputRequest),
}

impl DaemonPrompt {
    /// The prompt for `request`, or why the TUI cannot show it.
    pub(super) fn of(request: &InputRequest) -> Result<Self, String> {
        match &request.ask {
            Ask::Approval(ask) => Ok(Self::Approval(ApprovalPrompt::of_daemon_ask(ask))),
            Ask::PlanApproval(ask) => {
                // A mode this rho does not know is left out: it could not
                // name it.
                let modes: Vec<_> = ask
                    .modes
                    .iter()
                    .filter_map(|&mode| PermissionMode::try_from(mode).ok())
                    .collect();
                let form =
                    plan_exit::questionnaire(&modes, "Review the plan in the tool card above.")
                        .map_err(|error| error.to_string())?;
                Ok(Self::PlanApproval { form, modes })
            }
            Ask::Question(ask) => questions(ask, request.timeout.as_ref())
                .map(Self::Questions)
                .map_err(|error| error.to_string()),
            // `Unknown`, or an ask a newer daemon sends.
            _ => Err("this rho does not know what it asks; answer it from another client".into()),
        }
    }
}

/// The answer an approval prompt's `decision` gives.
pub(super) fn approval_answer(decision: ApprovalDecision) -> InputAnswer {
    match decision {
        ApprovalDecision::AllowOnce => InputAnswer::AllowOnce,
        ApprovalDecision::AllowForSession => InputAnswer::AllowForSession,
        ApprovalDecision::Deny { reason } => InputAnswer::Deny {
            reason: Some(reason),
        },
        // A decision a newer SDK makes: refuse rather than guess.
        _ => InputAnswer::Deny { reason: None },
    }
}

/// The answer a plan approval's `response` gives: approve into the mode it
/// chose when it is one of `modes`, the offered ones; otherwise keep
/// planning, as the in-process tool decides. Either carries the feedback
/// the response gave.
pub(super) fn plan_answer(response: &HostInputResponse, modes: &[PermissionMode]) -> InputAnswer {
    match plan_exit::questionnaire_decision(response, modes) {
        Some(decision) if decision.target != PermissionMode::Plan => InputAnswer::ApprovePlan {
            mode: decision.target.into(),
            feedback: decision.feedback,
        },
        Some(decision) => InputAnswer::KeepPlanning {
            feedback: decision.feedback,
        },
        None => InputAnswer::KeepPlanning { feedback: None },
    }
}

/// The answer a questionnaire's `response` gives.
pub(super) fn questions_answer(response: &HostInputResponse) -> InputAnswer {
    InputAnswer::Answers {
        answers: response.answers().clone(),
    }
}

/// The questionnaire for `ask`, with `timeout`'s fallback answers to show.
fn questions(
    ask: &QuestionAsk,
    timeout: Option<&InputTimeout>,
) -> Result<HostInputRequest, rho_sdk::Error> {
    let questions = ask
        .questions
        .iter()
        .map(question)
        .collect::<Result<Vec<_>, _>>()?;
    let form = HostInputRequest::questionnaire(&ask.title, questions)?;
    let Some((answers, reason)) = timeout.and_then(|timeout| match &timeout.answer {
        InputAnswer::Answers { answers } => Some((answers, &timeout.reason)),
        _ => None,
    }) else {
        return Ok(form);
    };
    let fallback = answers
        .iter()
        .fold(HostInputResponse::new(), |fallback, (id, values)| {
            fallback.answer(id, values)
        });
    // A fallback the form rejects only goes unshown; the daemon still
    // answers with it at the deadline.
    Ok(form
        .clone()
        .with_timeout_fallback(fallback, reason)
        .unwrap_or(form))
}

fn question(question: &wire::Question) -> Result<HostQuestion, rho_sdk::Error> {
    let choices = question
        .choices
        .iter()
        .map(|choice| {
            let host = HostChoice::new(&choice.value, &choice.label);
            match &choice.description {
                Some(description) => host.with_description(description),
                None => host,
            }
        })
        .collect();
    let selection = match question.selection {
        Selection::One => SelectionMode::One,
        Selection::Many => SelectionMode::Many,
        // A selection a newer daemon sends: one choice answers any.
        _ => SelectionMode::One,
    };
    let mut host = HostQuestion::new(&question.id, &question.prompt, choices, selection)?;
    if let Some(header) = &question.header {
        host = host.with_header(header);
    }
    if let Some(help) = &question.help {
        host = host.with_help(help);
    }
    if question.allow_other {
        host = host.allow_other();
    }
    if !question.required {
        host = host.optional();
    }
    if let Some(default) = &question.default {
        let value = match default.values.as_slice() {
            [value] => Value::String(value.clone()),
            values => Value::Array(values.iter().cloned().map(Value::String).collect()),
        };
        let selection = match default.style {
            DefaultStyle::Selected => DefaultSelection::Selected,
            DefaultStyle::Focused => DefaultSelection::Focused,
            // A style a newer daemon sends: the person still picks.
            _ => DefaultSelection::Focused,
        };
        host = host
            .with_default_value(value)
            .with_default_selection(selection);
    }
    Ok(host)
}

#[cfg(test)]
#[path = "daemon_asks_tests.rs"]
mod tests;
