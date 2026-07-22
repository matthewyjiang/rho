use std::time::Duration;

use anyhow::Context;
use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};
use serde::Deserialize;

use rho_providers::model::Message;

use crate::agent::{
    internal_definition, run_one_shot_agent, OneShotAgentRequest, GOAL_JUDGE_AGENT_ID,
};

pub(crate) const GOAL_JUDGE_PROMPT: &str = "You are a conservative goal-completion evaluator. Classify the goal using only evidence in the conversation transcript. Do not assume unreported work succeeded. Return only JSON in this exact shape: {\"state\":\"Met\"|\"Unmet\"|\"Blocked\",\"reason\":\"evidence-based explanation\",\"human_steps\":[{\"action\":\"specific action\",\"reason\":\"why it is outside this session's authority or capabilities\"}]}. Use Met only when the completion condition is fully satisfied. Use Unmet whenever meaningful work remains that the current agent can attempt, including work around missing dependencies, unavailable local tools, or transient network failures. Use Blocked only when no meaningful agent-actionable work remains and every remaining step requires user authority or capabilities unavailable in the current session. For Blocked, use reason to summarize what was completed and verified and why nothing agent-actionable remains, and list every remaining human-only step. Return an empty human_steps array for Met or Unmet.";

pub(super) const MAX_GOAL_CHARS: usize = 4_000;
pub(super) const EVALUATION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_EVALUATION_TRANSCRIPT_CHARS: usize = 64_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GoalLoopState {
    Active,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HumanStep {
    pub(super) action: String,
    pub(super) reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockedVerification {
    Waiting,
    InProgress,
}

#[derive(Clone, Debug)]
enum GoalPhase {
    Active,
    Blocked {
        pending_steps: Vec<HumanStep>,
        verification: BlockedVerification,
    },
}

#[derive(Clone, Debug)]
pub(super) struct GoalState {
    pub(super) condition: String,
    pub(super) turns: usize,
    pub(super) last_reason: Option<String>,
    phase: GoalPhase,
    started_at: std::time::Instant,
}

impl GoalState {
    pub(super) fn new(condition: String) -> Self {
        Self {
            condition,
            turns: 0,
            last_reason: None,
            phase: GoalPhase::Active,
            started_at: std::time::Instant::now(),
        }
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub(super) fn loop_state(&self) -> GoalLoopState {
        match self.phase {
            GoalPhase::Active => GoalLoopState::Active,
            GoalPhase::Blocked { .. } => GoalLoopState::Blocked,
        }
    }

    pub(super) fn is_blocked(&self) -> bool {
        matches!(self.phase, GoalPhase::Blocked { .. })
    }

    pub(super) fn pending_steps(&self) -> &[HumanStep] {
        match &self.phase {
            GoalPhase::Active => &[],
            GoalPhase::Blocked { pending_steps, .. } => pending_steps,
        }
    }

    pub(super) fn begin_verification(&mut self) -> bool {
        let GoalPhase::Blocked { verification, .. } = &mut self.phase else {
            return false;
        };
        *verification = BlockedVerification::InProgress;
        true
    }

    pub(super) fn complete_verification(&mut self) {
        if matches!(
            self.phase,
            GoalPhase::Blocked {
                verification: BlockedVerification::InProgress,
                ..
            }
        ) {
            self.phase = GoalPhase::Active;
        }
    }

    pub(super) fn interrupt_verification(&mut self) {
        if let GoalPhase::Blocked { verification, .. } = &mut self.phase {
            *verification = BlockedVerification::Waiting;
        }
    }

    pub(super) fn record_evaluation(&mut self, evaluation: &GoalEvaluation) -> GoalDisposition {
        self.turns += 1;
        self.last_reason = Some(evaluation.reason().to_string());
        match evaluation {
            GoalEvaluation::Met { .. } => GoalDisposition::Complete,
            GoalEvaluation::Unmet { .. } => {
                self.phase = GoalPhase::Active;
                GoalDisposition::Continue
            }
            GoalEvaluation::Blocked { pending_steps, .. } => {
                self.phase = GoalPhase::Blocked {
                    pending_steps: pending_steps.clone(),
                    verification: BlockedVerification::Waiting,
                };
                GoalDisposition::Pause
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GoalDisposition {
    Complete,
    Continue,
    Pause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum GoalEvaluation {
    Met {
        reason: String,
    },
    Unmet {
        reason: String,
    },
    Blocked {
        reason: String,
        pending_steps: Vec<HumanStep>,
    },
}

impl GoalEvaluation {
    pub(super) fn reason(&self) -> &str {
        match self {
            Self::Met { reason } | Self::Unmet { reason } | Self::Blocked { reason, .. } => reason,
        }
    }

    pub(super) fn pending_steps(&self) -> &[HumanStep] {
        match self {
            Self::Blocked { pending_steps, .. } => pending_steps,
            Self::Met { .. } | Self::Unmet { .. } => &[],
        }
    }
}

pub(super) struct EvaluationRequest<'a> {
    pub provider_name: &'a str,
    pub model: &'a str,
    pub condition: &'a str,
    pub messages: &'a [Message],
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a std::path::Path,
}

pub(super) async fn evaluate(
    request: EvaluationRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<GoalEvaluation> {
    let EvaluationRequest {
        provider_name,
        model,
        condition,
        messages,
        cancellation,
        session_id,
        workspace_path,
    } = request;
    let transcript = evaluation_transcript(messages);
    let blocks = run_one_shot_agent(
        OneShotAgentRequest {
            definition: internal_definition(GOAL_JUDGE_AGENT_ID),
            usage_purpose: "goal",
            provider_name,
            model,
            input: format!(
                "Completion condition:\n{condition}\n\nConversation transcript:\n{transcript}"
            ),
            cancellation,
            session_id,
            workspace_path,
        },
        usage_recording,
    )?
    .await?;
    let text = blocks.join("\n");
    parse_evaluation(&text).context("goal evaluator returned an invalid response")
}

fn evaluation_transcript(messages: &[Message]) -> String {
    let transcript = messages
        .iter()
        .filter(|message| !matches!(message, Message::System(_)))
        .map(safe_transcript_message)
        .map(|message| serde_json::to_string(&message).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    tail_chars(&transcript, MAX_EVALUATION_TRANSCRIPT_CHARS)
}

fn safe_transcript_message(message: &Message) -> Message {
    let mut message = message.clone();
    match &mut message {
        Message::EnrichedAssistant(assistant) => assistant.provider_context.clear(),
        Message::AbortedAssistant(assistant) => assistant.provider_context.clear(),
        Message::System(_) | Message::User(_) | Message::Assistant(_) | Message::ToolResult(_) => {}
    }
    message
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let start = text
        .char_indices()
        .nth(count - max_chars)
        .map(|(index, _)| index)
        .unwrap_or(0);
    format!("[earlier transcript omitted]\n{}", &text[start..])
}

#[derive(Deserialize)]
struct RawEvaluation {
    state: RawEvaluationState,
    reason: String,
    #[serde(default)]
    human_steps: Vec<RawHumanStep>,
}

#[derive(Deserialize)]
enum RawEvaluationState {
    Met,
    Unmet,
    Blocked,
}

#[derive(Deserialize)]
struct RawHumanStep {
    action: String,
    reason: String,
}

fn parse_evaluation(text: &str) -> anyhow::Result<GoalEvaluation> {
    let trimmed = text.trim();
    let json = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        trimmed
    } else {
        let start = trimmed
            .find('{')
            .ok_or_else(|| anyhow::anyhow!("missing JSON object"))?;
        let end = trimmed
            .rfind('}')
            .ok_or_else(|| anyhow::anyhow!("missing JSON object"))?;
        &trimmed[start..=end]
    };
    let parsed: RawEvaluation = serde_json::from_str(json)?;
    let reason = nonempty_field(parsed.reason, "evaluation reason")?;
    match parsed.state {
        RawEvaluationState::Met => Ok(GoalEvaluation::Met { reason }),
        RawEvaluationState::Unmet => Ok(GoalEvaluation::Unmet { reason }),
        RawEvaluationState::Blocked => {
            let pending_steps = parsed
                .human_steps
                .into_iter()
                .map(|step| {
                    Ok(HumanStep {
                        action: nonempty_field(step.action, "human step action")?,
                        reason: nonempty_field(step.reason, "human step reason")?,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            if pending_steps.is_empty() {
                anyhow::bail!("blocked evaluation has no human steps");
            }
            Ok(GoalEvaluation::Blocked {
                reason,
                pending_steps,
            })
        }
    }
}

fn nonempty_field(value: String, name: &str) -> anyhow::Result<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("{name} is empty");
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ElapsedPrecision {
    /// Whole seconds under one minute (`9s`).
    WholeSeconds,
    /// Tenths under one minute (`9.0s`), used by thought summaries.
    TenthsUnderMinute,
}

pub(super) fn format_elapsed(elapsed: Duration) -> String {
    format_elapsed_with(elapsed, ElapsedPrecision::WholeSeconds)
}

pub(super) fn format_elapsed_with(elapsed: Duration, precision: ElapsedPrecision) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        return match precision {
            ElapsedPrecision::WholeSeconds => format!("{seconds}s"),
            ElapsedPrecision::TenthsUnderMinute => {
                format!("{:.1}s", elapsed.as_secs_f64())
            }
        };
    }
    if seconds < 3_600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60)
    }
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod tests;
