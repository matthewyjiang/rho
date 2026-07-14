use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

use crate::{
    model::{build_provider, ContentBlock, Message, ModelRequest, ModelResponse},
    reasoning::ReasoningLevel,
};

pub(super) const MAX_GOAL_CHARS: usize = 4_000;
pub(super) const EVALUATION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_EVALUATION_TRANSCRIPT_CHARS: usize = 64_000;

#[derive(Clone, Debug)]
pub(super) struct GoalState {
    pub(super) condition: String,
    pub(super) turns: usize,
    pub(super) last_reason: Option<String>,
    started_at: std::time::Instant,
}

impl GoalState {
    pub(super) fn new(condition: String) -> Self {
        Self {
            condition,
            turns: 0,
            last_reason: None,
            started_at: std::time::Instant::now(),
        }
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GoalEvaluation {
    pub(super) met: bool,
    pub(super) reason: String,
}

pub(super) async fn evaluate(
    provider_name: &str,
    model: &str,
    condition: &str,
    messages: &[Message],
) -> anyhow::Result<GoalEvaluation> {
    let provider = build_provider(provider_name, model, ReasoningLevel::Low)?;
    let transcript = evaluation_transcript(messages);
    let request_messages = vec![
                Message::System(
                    "You are a conservative goal-completion evaluator. Decide whether the completion condition is fully satisfied using only evidence in the conversation transcript. Do not assume unreported work succeeded. Return only JSON in this exact shape: {\"met\":true|false,\"reason\":\"short explanation\"}."
                        .into(),
                ),
                Message::user_text(format!(
                    "Completion condition:\n{condition}\n\nConversation transcript:\n{transcript}"
                )),
            ];
    let response = provider
        .send_turn(ModelRequest {
            messages: &request_messages,
            tools: &[],
            cancellation: Default::default(),
            prompt_cache_key: None,
        })
        .await?;
    let ModelResponse::Assistant(blocks) = response;
    let text = blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
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
    met: bool,
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
    let reason = parsed.reason.trim().to_string();
    if reason.is_empty() {
        anyhow::bail!("evaluation reason is empty");
    }
    Ok(GoalEvaluation {
        met: parsed.met,
        reason,
    })
}

pub(super) fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_fenced_evaluations() {
        assert_eq!(
            parse_evaluation(r#"{"met":true,"reason":"tests pass"}"#).unwrap(),
            GoalEvaluation {
                met: true,
                reason: "tests pass".into(),
            }
        );
        assert_eq!(
            parse_evaluation("```json\n{\"met\":false,\"reason\":\"lint still fails\"}\n```")
                .unwrap(),
            GoalEvaluation {
                met: false,
                reason: "lint still fails".into(),
            }
        );
    }

    #[test]
    fn rejects_evaluation_without_a_reason() {
        assert!(parse_evaluation(r#"{"met":false,"reason":"  "}"#).is_err());
    }

    #[test]
    fn transcript_omits_opaque_provider_context() {
        let identity =
            crate::model::ModelIdentity::new("anthropic", "anthropic-messages", "claude-test");
        let transcript =
            evaluation_transcript(&[Message::assistant(crate::model::AssistantMessage {
                content: vec![ContentBlock::Text("answer".into())],
                provenance: Some(identity.clone()),
                reasoning_summary: Some("safe summary".into()),
                provider_context: vec![crate::model::ProviderContextBlock {
                    identity,
                    kind: "anthropic_content_block".into(),
                    position: Some(0),
                    data: serde_json::json!({"signature": "secret-signature"}),
                }],
            })]);

        assert!(transcript.contains("answer"));
        assert!(transcript.contains("safe summary"));
        assert!(!transcript.contains("secret-signature"));
        assert!(!transcript.contains("provider_context"));
    }

    #[test]
    fn transcript_tail_is_unicode_safe() {
        assert_eq!(
            tail_chars("a项目bc", 3),
            "[earlier transcript omitted]\n目bc"
        );
    }

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(format_elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 5s");
        assert_eq!(format_elapsed(Duration::from_secs(3_720)), "1h 2m");
    }
}
