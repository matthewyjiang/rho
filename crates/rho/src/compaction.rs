use rho_providers::model::{
    context::{estimate_context_tokens, estimate_message_tokens},
    ContentBlock, Message,
};
use rho_tools::tool::{ToolResult, ToolSpec};

const SUMMARY_RESERVE_MIN_TOKENS: u64 = 512;
const SUMMARY_RESERVE_MAX_TOKENS: u64 = 8_192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionConfig {
    pub auto_compact: bool,
    pub threshold_percent: u8,
    pub target_percent: u8,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact: false,
            threshold_percent: 85,
            target_percent: 50,
        }
    }
}

impl CompactionConfig {
    pub fn threshold_tokens(&self, context_window: u64) -> Option<u64> {
        (self.auto_compact && context_window > 0)
            .then(|| percent_tokens(context_window, normalized_percent(self.threshold_percent)))
    }

    pub fn target_tokens(&self, context_window: u64) -> u64 {
        percent_tokens(
            context_window,
            normalized_target_percent(self.threshold_percent, self.target_percent),
        )
    }
}

#[derive(Clone, Debug)]
pub struct CompactionPartition {
    pub leading_messages: Vec<Message>,
    pub compacted_messages: Vec<Message>,
    pub recent_messages: Vec<Message>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MessageGroup {
    start: usize,
    end: usize,
    tokens: u64,
}

pub fn partition_messages_for_compaction(
    messages: &[Message],
    tools: &[ToolSpec],
    target_tokens: u64,
) -> Option<CompactionPartition> {
    let first_compactable = messages
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap_or(messages.len());
    let groups = message_groups(messages, first_compactable);
    if groups.len() <= 1 {
        return None;
    }

    let recent_token_budget =
        recent_tail_token_budget(&messages[..first_compactable], tools, target_tokens);
    let recent_start = recent_tail_start(&groups, recent_token_budget)?;
    if recent_start <= first_compactable {
        return None;
    }

    Some(CompactionPartition {
        leading_messages: messages[..first_compactable].to_vec(),
        compacted_messages: messages[first_compactable..recent_start].to_vec(),
        recent_messages: messages[recent_start..].to_vec(),
    })
}

pub fn build_summary_request_messages(compacted_messages: &[Message]) -> Vec<Message> {
    vec![
        Message::System(
            "Summarize the compacted conversation history for continuation. The original transcript is still stored separately; this summary replaces only older model context. Preserve user goals, constraints, decisions, files changed, tool calls and results, test results, unresolved tasks, and continuation-critical details such as paths, commands, errors, IDs, and pending next steps. Be concise and factual."
                .into(),
        ),
        Message::user_text(render_messages_for_summary(compacted_messages)),
    ]
}

pub fn replacement_history_from_summary(
    partition: CompactionPartition,
    summary: String,
) -> Vec<Message> {
    let mut replacement = partition.leading_messages;
    replacement.push(Message::user_text(format!(
        "Automatic compaction summary of earlier conversation for model context only:\n\n{}",
        summary.trim()
    )));
    replacement.extend(partition.recent_messages);
    replacement
}

fn recent_tail_token_budget(
    leading_messages: &[Message],
    tools: &[ToolSpec],
    target_tokens: u64,
) -> u64 {
    let fixed_tokens = estimate_context_tokens(leading_messages, tools);
    target_tokens
        .saturating_sub(fixed_tokens)
        .saturating_sub(summary_reserve_tokens(target_tokens))
}

fn recent_tail_start(groups: &[MessageGroup], token_budget: u64) -> Option<usize> {
    let mut tail_start = None;
    let mut tail_tokens = 0_u64;
    for group in groups.iter().rev() {
        let next_tokens = tail_tokens.saturating_add(group.tokens);
        if tail_start.is_some() && next_tokens > token_budget {
            break;
        }
        tail_tokens = next_tokens;
        tail_start = Some(group.start);
    }
    tail_start
}

fn message_groups(messages: &[Message], start: usize) -> Vec<MessageGroup> {
    let mut groups = Vec::new();
    let mut index = start;
    while index < messages.len() {
        let end = completed_tool_group_end(messages, index).unwrap_or(index + 1);
        let tokens = messages[index..end]
            .iter()
            .map(estimate_message_tokens)
            .sum();
        groups.push(MessageGroup {
            start: index,
            end,
            tokens,
        });
        index = end;
    }
    groups
}

fn completed_tool_group_end(messages: &[Message], index: usize) -> Option<usize> {
    let blocks = messages[index].completed_assistant_content()?;
    let tool_call_ids = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call.id.as_str()),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>();
    if tool_call_ids.is_empty() {
        return None;
    }

    let results_start = index + 1;
    let results_end = results_start + tool_call_ids.len();
    if results_end > messages.len() {
        return Some(messages.len());
    }
    let complete = tool_call_ids.iter().enumerate().all(|(offset, id)| {
        matches!(
            &messages[results_start + offset],
            Message::ToolResult(result) if result.id == *id
        )
    });
    Some(if complete {
        results_end
    } else {
        messages.len()
    })
}

fn summary_reserve_tokens(target_tokens: u64) -> u64 {
    if target_tokens == 0 {
        return 0;
    }
    (target_tokens / 10)
        .clamp(SUMMARY_RESERVE_MIN_TOKENS, SUMMARY_RESERVE_MAX_TOKENS)
        .min(target_tokens)
}

fn percent_tokens(total: u64, percent: u8) -> u64 {
    total.saturating_mul(u64::from(percent)).div_ceil(100)
}

fn normalized_percent(percent: u8) -> u8 {
    percent.clamp(1, 100)
}

fn normalized_target_percent(threshold_percent: u8, target_percent: u8) -> u8 {
    let threshold_percent = normalized_percent(threshold_percent);
    let target_percent = normalized_percent(target_percent);
    if threshold_percent == 1 {
        1
    } else {
        target_percent.min(threshold_percent - 1)
    }
}

fn render_messages_for_summary(messages: &[Message]) -> String {
    messages
        .iter()
        .map(render_message_for_summary)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_message_for_summary(message: &Message) -> String {
    match message {
        Message::System(text) => format!("system:\n{text}"),
        Message::User(blocks) => format!("user:\n{}", render_blocks(blocks)),
        Message::Assistant(blocks) => format!("assistant:\n{}", render_blocks(blocks)),
        Message::EnrichedAssistant(message) => {
            let mut rendered = render_blocks(&message.content);
            if let Some(summary) = &message.reasoning_summary {
                rendered.push_str(&format!("\nreasoning summary:\n{summary}"));
            }
            format!("assistant:\n{rendered}")
        }
        Message::AbortedAssistant(message) => {
            format!("assistant [aborted]:\n{}", render_blocks(&message.content))
        }
        Message::ToolResult(result) => format!("tool result:\n{}", render_tool_result(result)),
    }
}

fn render_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.clone(),
            ContentBlock::Image(image) => format!("[image: {}]", image.mime_type),
            ContentBlock::ToolCall(call) => serde_json::to_string(call).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_result(result: &ToolResult) -> String {
    serde_json::to_string(result).unwrap_or_else(|_| result.content.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use rho_tools::tool::ToolCall;

    #[test]
    fn compaction_summary_retains_portable_reasoning_context() {
        let source = rho_providers::model::ModelIdentity::new(
            "openai-codex",
            "openai-responses",
            "gpt-test",
        );
        let messages = vec![Message::assistant(rho_providers::model::AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(source),
            reasoning_summary: Some("verified it".into()),
            portable_fallback: None,
            provider_context: Vec::new(),
        })];

        let rendered = render_messages_for_summary(&messages);

        assert!(rendered.contains("reasoning summary:\nverified it"));
    }

    #[test]
    fn compaction_threshold_requires_enabled_config_and_window() {
        let config = CompactionConfig {
            auto_compact: true,
            threshold_percent: 80,
            target_percent: 50,
        };

        assert_eq!(config.threshold_tokens(1_000), Some(800));
        assert_eq!(config.threshold_tokens(0), None);
        assert_eq!(
            CompactionConfig {
                auto_compact: false,
                ..config
            }
            .threshold_tokens(1_000),
            None
        );
    }

    #[test]
    fn target_percent_stays_below_threshold_when_possible() {
        let config = CompactionConfig {
            auto_compact: true,
            threshold_percent: 85,
            target_percent: 99,
        };

        assert_eq!(config.target_tokens(1_000), 840);
    }

    #[test]
    fn partitions_messages_around_token_budgeted_recent_tail() {
        let messages = vec![
            Message::System("system".into()),
            Message::user_text("x".repeat(1_000)),
            Message::assistant_text("y".repeat(1_000)),
            Message::user_text("recent user"),
            Message::assistant_text("recent assistant"),
        ];

        let partition = partition_messages_for_compaction(&messages, &[], 700).unwrap();

        assert_eq!(partition.leading_messages.len(), 1);
        assert!(matches!(
            partition.compacted_messages.as_slice(),
            [Message::User(_), Message::Assistant(_)]
        ));
        assert!(matches!(
            partition.recent_messages.as_slice(),
            [Message::User(_), Message::Assistant(_)]
        ));
    }

    #[test]
    fn partition_does_not_split_assistant_tool_call_group() {
        let messages = vec![
            Message::System("system".into()),
            Message::user_text("x".repeat(1_000)),
            Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: json!({"command": "echo hi"}),
            })]),
            Message::ToolResult(ToolResult {
                id: "call_1".into(),
                ok: true,
                content: "hi".into(),
            }),
            Message::user_text("new"),
        ];

        let partition = partition_messages_for_compaction(&messages, &[], 700).unwrap();

        assert!(matches!(
            partition.compacted_messages.as_slice(),
            [Message::User(_)]
        ));
        assert!(matches!(
            partition.recent_messages.as_slice(),
            [
                Message::Assistant(_),
                Message::ToolResult(_),
                Message::User(_)
            ]
        ));
    }

    #[test]
    fn partition_does_not_split_enriched_assistant_tool_call_group() {
        let identity = rho_providers::model::ModelIdentity::new(
            "openai-codex",
            "openai-responses",
            "gpt-test",
        );
        let messages = vec![
            Message::System("system".into()),
            Message::user_text("x".repeat(1_000)),
            Message::assistant(rho_providers::model::AssistantMessage {
                content: vec![ContentBlock::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "echo hi"}),
                })],
                provenance: Some(identity),
                reasoning_summary: None,
                portable_fallback: None,
                provider_context: Vec::new(),
            }),
            Message::ToolResult(ToolResult {
                id: "call_1".into(),
                ok: true,
                content: "hi".into(),
            }),
            Message::user_text("new"),
        ];

        let partition = partition_messages_for_compaction(&messages, &[], 700).unwrap();

        assert!(matches!(
            partition.recent_messages.as_slice(),
            [
                Message::EnrichedAssistant(_),
                Message::ToolResult(_),
                Message::User(_)
            ]
        ));
    }

    #[test]
    fn partition_keeps_last_group_even_when_it_exceeds_budget() {
        let messages = vec![
            Message::System("system".into()),
            Message::user_text("old"),
            Message::assistant_text("z".repeat(2_000)),
        ];

        let partition = partition_messages_for_compaction(&messages, &[], 1).unwrap();

        assert!(matches!(
            partition.compacted_messages.as_slice(),
            [Message::User(_)]
        ));
        assert!(matches!(
            partition.recent_messages.as_slice(),
            [Message::Assistant(_)]
        ));
    }

    #[test]
    fn partition_skips_when_everything_fits_in_recent_tail() {
        let messages = vec![
            Message::System("system".into()),
            Message::user_text("old user"),
            Message::assistant_text("old assistant"),
        ];

        assert!(partition_messages_for_compaction(&messages, &[], 10_000).is_none());
    }

    #[test]
    fn builds_replacement_history_with_summary_between_system_and_recent_messages() {
        let partition = CompactionPartition {
            leading_messages: vec![Message::System("system".into())],
            compacted_messages: vec![Message::user_text("old")],
            recent_messages: vec![Message::user_text("new")],
        };

        let replacement = replacement_history_from_summary(partition, "remembered".into());

        assert_eq!(replacement.len(), 3);
        assert!(matches!(replacement[0], Message::System(_)));
        assert!(
            matches!(&replacement[1], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text.contains("remembered")))
        );
        assert!(matches!(replacement[2], Message::User(_)));
    }
}
