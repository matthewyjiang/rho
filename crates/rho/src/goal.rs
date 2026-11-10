//! Shared goal state, evaluation, and prompt policy for session hosts.

use std::time::Duration;

use anyhow::Context;
use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};
use serde::Deserialize;

use rho_providers::model::Message;
use rho_sdk::model::SemanticMessage;

use crate::history_message::HistoryMessage;

use crate::agent::{
    internal_definition, run_one_shot_agent, OneShotAgentRequest, GOAL_JUDGE_AGENT_ID,
};

pub(crate) const GOAL_JUDGE_PROMPT: &str = "You are a conservative goal-completion evaluator. Classify the goal using only evidence in the conversation transcript. Do not assume unreported work succeeded. Return only JSON in this exact shape: {\"state\":\"Met\"|\"Unmet\"|\"Blocked\",\"reason\":\"evidence-based explanation\",\"human_steps\":[{\"action\":\"specific action\",\"reason\":\"why it is outside this session's authority or capabilities\"}]}. Use Met only when the completion condition is fully satisfied. Use Unmet whenever meaningful work remains that the current agent can attempt, including work around missing dependencies, unavailable local tools, or transient network failures. Use Blocked only when no meaningful agent-actionable work remains and every remaining step requires user authority or capabilities unavailable in the current session. For Blocked, use reason to summarize what was completed and verified and why nothing agent-actionable remains, and list every remaining human-only step. Return an empty human_steps array for Met or Unmet.";

pub(crate) const MAX_GOAL_CHARS: usize = 4_000;
pub(crate) const EVALUATION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_EVALUATION_TRANSCRIPT_CHARS: usize = 64_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoalLoopState {
    Active,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HumanStep {
    pub(crate) action: String,
    pub(crate) reason: String,
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
pub(crate) struct GoalState {
    pub(crate) condition: String,
    pub(crate) turns: usize,
    pub(crate) last_reason: Option<String>,
    phase: GoalPhase,
    started_at: std::time::Instant,
    /// When the goal was set, as frontends of a daemon goal count from.
    set_at: std::time::SystemTime,
}

impl GoalState {
    pub(crate) fn new(condition: String) -> Self {
        Self {
            condition,
            turns: 0,
            last_reason: None,
            phase: GoalPhase::Active,
            started_at: std::time::Instant::now(),
            set_at: std::time::SystemTime::now(),
        }
    }

    /// Unix time, in seconds, when the goal was set.
    pub(crate) fn set_at_unix_secs(&self) -> u64 {
        self.set_at
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub(crate) fn loop_state(&self) -> GoalLoopState {
        match self.phase {
            GoalPhase::Active => GoalLoopState::Active,
            GoalPhase::Blocked { .. } => GoalLoopState::Blocked,
        }
    }

    pub(crate) fn is_blocked(&self) -> bool {
        matches!(self.phase, GoalPhase::Blocked { .. })
    }

    pub(crate) fn pending_steps(&self) -> &[HumanStep] {
        match &self.phase {
            GoalPhase::Active => &[],
            GoalPhase::Blocked { pending_steps, .. } => pending_steps,
        }
    }

    pub(crate) fn begin_verification(&mut self) -> bool {
        let GoalPhase::Blocked { verification, .. } = &mut self.phase else {
            return false;
        };
        *verification = BlockedVerification::InProgress;
        true
    }

    pub(crate) fn complete_verification(&mut self) {
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

    pub(crate) fn interrupt_verification(&mut self) {
        if let GoalPhase::Blocked { verification, .. } = &mut self.phase {
            *verification = BlockedVerification::Waiting;
        }
    }

    pub(crate) fn record_evaluation(&mut self, evaluation: &GoalEvaluation) -> GoalDisposition {
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
pub(crate) enum GoalDisposition {
    Complete,
    Continue,
    Pause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GoalEvaluation {
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
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Met { reason } | Self::Unmet { reason } | Self::Blocked { reason, .. } => reason,
        }
    }

    pub(crate) fn pending_steps(&self) -> &[HumanStep] {
        match self {
            Self::Blocked { pending_steps, .. } => pending_steps,
            Self::Met { .. } | Self::Unmet { .. } => &[],
        }
    }
}

pub(crate) struct EvaluationRequest<'a> {
    pub provider_name: &'a str,
    pub model: &'a str,
    pub auth: &'a str,
    pub reasoning: rho_providers::reasoning::ReasoningLevel,
    pub condition: &'a str,
    pub messages: &'a [Message],
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a std::path::Path,
    /// The credentials of the session the goal belongs to.
    pub credentials: std::sync::Arc<dyn rho_providers::credentials::CredentialStore>,
}

pub(crate) async fn evaluate(
    request: EvaluationRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<GoalEvaluation> {
    let EvaluationRequest {
        provider_name,
        model,
        auth,
        reasoning,
        condition,
        messages,
        cancellation,
        session_id,
        workspace_path,
        credentials,
    } = request;
    let transcript = evaluation_transcript(messages);
    let result = run_one_shot_agent(
        OneShotAgentRequest {
            definition: internal_definition(GOAL_JUDGE_AGENT_ID),
            usage_purpose: "goal",
            reasoning: Some(reasoning),
            input: vec![rho_sdk::model::ContentBlock::Text(format!(
                "Completion condition:\n{condition}\n\nConversation transcript:\n{transcript}"
            ))],
            cancellation,
            session_id,
            workspace_path,
        },
        provider_name,
        model,
        auth,
        credentials,
        usage_recording,
    )
    .await?;
    let text = result.texts.join("\n");
    parse_evaluation(&text).context("goal evaluator returned an invalid response")
}

fn evaluation_transcript(messages: &[Message]) -> String {
    let transcript = messages
        .iter()
        .filter(|message| !matches!(message, Message::System(_)))
        .map(|message| match HistoryMessage::of(message) {
            // A model-written summary is not user input; label it so the
            // judge weighs it as a recap, not as a user claim.
            HistoryMessage::Semantic(SemanticMessage::CompactionSummary(summary)) => {
                serde_json::json!({ "CompactionSummary": summary.text }).to_string()
            }
            HistoryMessage::ChecklistRestatement(blocks) => serde_json::json!({
                "ChecklistRestatement": {
                    "source": "host context, not human authorization",
                    "content": blocks,
                }
            })
            .to_string(),
            // Other entries serialize under their typed variant name, which
            // already labels runtime input and host context.
            HistoryMessage::Semantic(
                SemanticMessage::System(_)
                | SemanticMessage::User(_)
                | SemanticMessage::Assistant(_)
                | SemanticMessage::EnrichedAssistant(_)
                | SemanticMessage::AbortedAssistant(_)
                | SemanticMessage::ToolResult(_)
                | SemanticMessage::ModelContext(_)
                | SemanticMessage::BoundaryInput(_)
                | SemanticMessage::ReasoningShift(_),
            ) => serde_json::to_string(&safe_transcript_message(message)).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    tail_chars(&transcript, MAX_EVALUATION_TRANSCRIPT_CHARS)
}

fn safe_transcript_message(message: &Message) -> Message {
    let mut message = message.clone();
    match &mut message {
        Message::EnrichedAssistant(assistant) => assistant.retain_portable_context(),
        Message::AbortedAssistant(assistant) => assistant.provider_context.clear(),
        Message::System(_)
        | Message::User(_)
        | Message::Assistant(_)
        | Message::ToolResult(_)
        | Message::CompactionSummary(_)
        | Message::ModelContext(_)
        | Message::BoundaryInput(_)
        | Message::ReasoningShift(_) => {}
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

// Same retry cadence as the interactive goal loop.
pub(crate) const GOAL_RETRY_DELAY: Duration = Duration::from_secs(3);

pub(crate) fn initial_goal_prompt(condition: &str) -> String {
    format!(
        "The user invoked Rho's `/goal` command to set the following completion goal. Treat this as a goal-setting action, not as an ordinary conversational message or a claim that the goal is already complete.\n\nGoal:\n{condition}\n\nBegin working toward the goal now. Make concrete progress, use tools as needed, and verify the completion condition before stopping."
    )
}

pub(crate) fn blocked_goal_resumption_prompt(
    condition: &str,
    pending_steps: &[HumanStep],
    user_message: Option<&str>,
) -> String {
    let user_message = user_message
        .map(|message| format!("\n\nThe user's new message is:\n{message}"))
        .unwrap_or_default();
    format!(
        "Resume the following goal after it was blocked on steps requiring the user:\n\n{condition}\n\nPreviously blocked steps:\n{}\n\nFirst verify whether each relevant external condition has changed. Do not repeat implementation work unless verification shows that more agent-actionable work is needed.{user_message}",
        format_human_steps(pending_steps)
    )
}

pub(crate) fn format_human_steps(steps: &[HumanStep]) -> String {
    steps
        .iter()
        .map(|step| format!("- {}: {}", step.action, step.reason))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod tests;
