use crate::model::{ContentBlock, Message};
use crate::tool::ToolResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionConfig {
    pub auto_compact: bool,
    pub threshold_percent: u8,
    pub recent_messages: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact: false,
            threshold_percent: 85,
            recent_messages: 8,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompactionPartition {
    pub leading_messages: Vec<Message>,
    pub compacted_messages: Vec<Message>,
    pub recent_messages: Vec<Message>,
}

pub fn should_compact(
    config: &CompactionConfig,
    estimated_tokens: Option<u64>,
    context_window: Option<u64>,
) -> bool {
    if !config.auto_compact {
        return false;
    }
    let Some(tokens) = estimated_tokens else {
        return false;
    };
    let Some(window) = context_window.filter(|window| *window > 0) else {
        return false;
    };
    let threshold = u64::from(config.threshold_percent.clamp(1, 100));
    tokens.saturating_mul(100) >= window.saturating_mul(threshold)
}

pub fn partition_messages_for_compaction(
    messages: &[Message],
    recent_messages: usize,
) -> Option<CompactionPartition> {
    let first_compactable = messages
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap_or(messages.len());
    let body_len = messages.len().saturating_sub(first_compactable);
    if body_len <= recent_messages {
        return None;
    }

    let desired_recent_start = messages.len().saturating_sub(recent_messages);
    let recent_start = tool_group_start_containing(messages, desired_recent_start)
        .unwrap_or(desired_recent_start)
        .max(first_compactable);
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
            "Summarize the conversation history for continuation. Preserve user goals, decisions, constraints, files changed, tool outcomes, and unresolved tasks. Be concise and factual."
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
        "Summary of earlier conversation:\n\n{}",
        summary.trim()
    )));
    replacement.extend(partition.recent_messages);
    replacement
}

fn tool_group_start_containing(messages: &[Message], index: usize) -> Option<usize> {
    for (candidate, message) in messages.iter().enumerate() {
        let Message::Assistant(blocks) = message else {
            continue;
        };
        let tool_call_count = blocks
            .iter()
            .filter(|block| matches!(block, ContentBlock::ToolCall(_)))
            .count();
        if tool_call_count == 0 {
            continue;
        }
        let group_end = candidate.saturating_add(1).saturating_add(tool_call_count);
        if candidate < index && index < group_end {
            return Some(candidate);
        }
    }
    None
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
        Message::ToolResult(result) => format!("tool result:\n{}", render_tool_result(result)),
    }
}

fn render_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.clone(),
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
    use crate::tool::ToolCall;

    #[test]
    fn compaction_decision_requires_enabled_config_and_window() {
        let config = CompactionConfig {
            auto_compact: true,
            threshold_percent: 80,
            recent_messages: 4,
        };

        assert!(should_compact(&config, Some(800), Some(1_000)));
        assert!(!should_compact(&config, Some(799), Some(1_000)));
        assert!(!should_compact(&config, Some(900), None));
        assert!(!should_compact(
            &CompactionConfig {
                auto_compact: false,
                ..config
            },
            Some(900),
            Some(1_000)
        ));
    }

    #[test]
    fn partitions_messages_around_recent_tail() {
        let messages = vec![
            Message::System("system".into()),
            Message::user_text("old user"),
            Message::assistant_text("old assistant"),
            Message::user_text("recent user"),
            Message::assistant_text("recent assistant"),
        ];

        let partition = partition_messages_for_compaction(&messages, 2).unwrap();

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
            Message::user_text("old"),
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

        let partition = partition_messages_for_compaction(&messages, 2).unwrap();

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
