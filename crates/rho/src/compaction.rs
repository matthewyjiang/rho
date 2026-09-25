use rho_providers::model::{
    context::{estimate_context_tokens, estimate_message_tokens},
    ContentBlock, Message,
};
use rho_sdk::model::SemanticMessage;
use rho_tools::tool::ToolSpec;

#[path = "compaction_elide.rs"]
mod elide;
#[path = "compaction_summary.rs"]
mod summary;
pub(crate) use elide::{elide_tool_results, recall_id, Elision};
pub(crate) use summary::{
    build_summary_request_messages, replacement_history_from_summary, strip_analysis, ActiveGoal,
};

const SUMMARY_RESERVE_MIN_TOKENS: u64 = 512;
const SUMMARY_RESERVE_MAX_TOKENS: u64 = 8_192;
/// Largest first user turn kept verbatim through compaction; larger turns are
/// summarized instead. Measured over 701 local sessions: the first user turn
/// (contiguous user messages before the first reply) estimates at most 856
/// tokens at p99 and 1,537 at p99.5, and only 2 sessions exceed 2,048.
const FIRST_TURN_ANCHOR_MAX_TOKENS: u64 = 2_048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionConfig {
    pub auto_compact: bool,
    pub threshold_percent: u8,
    pub target_percent: u8,
}

/// Single source of truth for compaction defaults; `Config::default()` derives
/// its compaction fields from this.
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
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

    /// Translate the model-token target into the local units used to partition
    /// history. A provider-calibrated trigger must not retain an uncalibrated tail.
    /// Manual and context-overflow requests additionally cap retention at half
    /// the current context, so `/compact` can remove history below the
    /// automatic threshold and overflow recovery shrinks even when the
    /// configured window overstates the provider's real limit.
    pub(crate) fn target_tokens_for_context(
        &self,
        context_window: Option<u64>,
        trigger: rho_sdk::CompactionTrigger,
        context: rho_sdk::ContextEstimate,
    ) -> u64 {
        let configured = context_window
            .map(|window| self.target_tokens(window))
            .unwrap_or(u64::MAX / 2);
        let target = match trigger {
            rho_sdk::CompactionTrigger::Manual | rho_sdk::CompactionTrigger::ContextOverflow => {
                configured.min(context.tokens() / 2)
            }
            // `CompactionTrigger` is `#[non_exhaustive]`; unknown future
            // triggers keep the configured automatic budget.
            rho_sdk::CompactionTrigger::Automatic | _ => configured,
        };
        context.estimated_budget(target)
    }
}

/// True when a finished compact removed messages or estimated tokens.
pub(crate) fn outcome_reduced_context(outcome: &rho_sdk::CompactionOutcome) -> bool {
    outcome.current_messages() < outcome.previous_messages() || outcome.removed_tokens() > 0
}

/// History split for compaction, in order. Goal anchors written by an earlier
/// compaction sit between `anchor_messages` and the previous summary; they
/// belong to no field because the replacement re-adds the current goal.
#[derive(Clone, Debug)]
pub struct CompactionPartition {
    pub leading_messages: Vec<Message>,
    /// The first user turn, kept verbatim when it fits
    /// [`FIRST_TURN_ANCHOR_MAX_TOKENS`].
    pub anchor_messages: Vec<Message>,
    /// Text of the summary an earlier compaction wrote, so the summarizer
    /// updates it instead of summarizing it again as a user turn.
    pub previous_summary: Option<String>,
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
    let anchor_end = first_turn_anchor_end(messages, first_compactable);
    partition_after_anchor(
        messages,
        tools,
        target_tokens,
        first_compactable,
        anchor_end,
    )
    // With nothing else to remove, summarize the first turn rather than
    // make compaction a no-op.
    .or_else(|| {
        (anchor_end > first_compactable).then_some(())?;
        partition_after_anchor(
            messages,
            tools,
            target_tokens,
            first_compactable,
            first_compactable,
        )
    })
}

fn partition_after_anchor(
    messages: &[Message],
    tools: &[ToolSpec],
    target_tokens: u64,
    first_compactable: usize,
    anchor_end: usize,
) -> Option<CompactionPartition> {
    let mut compacted_start = anchor_end;
    while messages
        .get(compacted_start)
        .is_some_and(summary::is_goal_anchor)
    {
        compacted_start += 1;
    }
    let previous_summary = messages
        .get(compacted_start)
        .and_then(Message::as_compaction_summary)
        .map(|summary| summary.text().to_owned());
    if previous_summary.is_some() {
        compacted_start += 1;
    }

    let groups = message_groups(messages, compacted_start);
    let recent_token_budget =
        recent_tail_token_budget(&messages[..anchor_end], tools, target_tokens);
    let recent_start = recent_tail_start(&groups, recent_token_budget)?;
    if recent_start <= compacted_start {
        return None;
    }

    Some(CompactionPartition {
        leading_messages: messages[..first_compactable].to_vec(),
        anchor_messages: messages[first_compactable..anchor_end].to_vec(),
        previous_summary,
        compacted_messages: messages[compacted_start..recent_start].to_vec(),
        recent_messages: messages[recent_start..].to_vec(),
    })
}

/// End of the verbatim first-turn anchor starting at `start`: the contiguous
/// human user messages there, or `start` when they exceed the anchor budget.
fn first_turn_anchor_end(messages: &[Message], start: usize) -> usize {
    let end = messages[start..]
        .iter()
        .position(|message| {
            !matches!(message.semantic(), SemanticMessage::User(_))
                || message.as_compaction_summary().is_some()
                || summary::is_goal_anchor(message)
        })
        .map_or(messages.len(), |offset| start + offset);
    let tokens: u64 = messages[start..end]
        .iter()
        .map(estimate_message_tokens)
        .sum();
    if tokens <= FIRST_TURN_ANCHOR_MAX_TOKENS {
        end
    } else {
        start
    }
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
    if !blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
    {
        return None;
    }

    let mut end = index + 1;
    loop {
        let call_ids = messages[index..end]
            .iter()
            .filter_map(Message::completed_assistant_content)
            .flatten()
            .filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call.id.as_str()),
                ContentBlock::Text(_) | ContentBlock::Image(_) => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let Some(last_result_offset) = messages[end..]
            .iter()
            .enumerate()
            .filter_map(|(offset, message)| match message.semantic() {
                SemanticMessage::ToolResult(result) if call_ids.contains(result.id.as_str()) => {
                    Some(offset)
                }
                SemanticMessage::ToolImageSupplement(images)
                    if call_ids.contains(images.tool_call_id()) =>
                {
                    Some(offset)
                }
                SemanticMessage::System(_)
                | SemanticMessage::User(_)
                | SemanticMessage::Assistant(_)
                | SemanticMessage::EnrichedAssistant(_)
                | SemanticMessage::AbortedAssistant(_)
                | SemanticMessage::ToolResult(_)
                | SemanticMessage::ToolImageSupplement(_) => None,
            })
            .next_back()
        else {
            return Some(end);
        };
        end += last_result_offset + 1;
    }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use rho_tools::tool::{ToolCall, ToolResult};

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

    // Covers: manual compaction budget is capped below current usage so
    // `/compact` removes history even when the auto target is far away.
    // Owner: CompactionConfig target policy
    #[test]
    fn manual_trigger_caps_target_at_half_of_current_context() {
        let config = CompactionConfig {
            auto_compact: true,
            threshold_percent: 85,
            target_percent: 50,
        };
        let messages = vec![Message::user_text("x".repeat(4_000))];
        let current = estimate_context_tokens(&messages, &[]);
        let cases = [
            (rho_sdk::CompactionTrigger::Automatic, 500_000),
            (rho_sdk::CompactionTrigger::Manual, current / 2),
            (rho_sdk::CompactionTrigger::ContextOverflow, current / 2),
        ];

        for (trigger, expected) in cases {
            assert_eq!(
                config.target_tokens_for_context(
                    Some(1_000_000),
                    trigger,
                    rho_sdk::ContextEstimate::from_estimated_tokens(current),
                ),
                expected,
                "trigger={trigger:?}"
            );
        }
    }

    // Covers: the first user turn stays verbatim only within its budget and only
    // when something else is compacted; an earlier summary and its stale goal
    // anchor leave the compacted span so the summarizer can update the summary.
    // Owner: compaction partition
    #[test]
    fn partition_anchors_first_turn_and_splits_out_previous_summary() {
        struct Case {
            name: &'static str,
            messages: Vec<Message>,
            anchor: Vec<Message>,
            previous_summary: Option<&'static str>,
            compacted: Vec<Message>,
            recent: Vec<Message>,
        }
        let system = Message::System("system".into());
        let task = Message::user_text("x".repeat(1_000));
        let huge_task = Message::user_text("x".repeat(20_000));
        let old = Message::assistant_text("y".repeat(2_000));
        let recent = || {
            vec![
                Message::user_text("recent user"),
                Message::assistant_text("recent assistant"),
            ]
        };
        let stale_goal = |condition: &str| {
            let goal = ActiveGoal::default();
            goal.set(Some(condition));
            summary::replacement_history_from_summary(
                CompactionPartition {
                    leading_messages: Vec::new(),
                    anchor_messages: Vec::new(),
                    previous_summary: None,
                    compacted_messages: Vec::new(),
                    recent_messages: Vec::new(),
                },
                rho_sdk::CompactionTrigger::Manual,
                &goal,
                "earlier",
            )
        };
        let [goal_anchor, earlier_summary] = <[Message; 2]>::try_from(stale_goal("done")).unwrap();
        let cases = [
            Case {
                name: "first turn anchored",
                messages: [vec![system.clone(), task.clone(), old.clone()], recent()].concat(),
                anchor: vec![task.clone()],
                previous_summary: None,
                compacted: vec![old.clone()],
                recent: recent(),
            },
            Case {
                name: "oversized first turn is summarized",
                messages: [
                    vec![system.clone(), huge_task.clone(), old.clone()],
                    recent(),
                ]
                .concat(),
                anchor: Vec::new(),
                previous_summary: None,
                compacted: vec![huge_task.clone(), old.clone()],
                recent: recent(),
            },
            Case {
                name: "single turn falls back to summarizing it",
                messages: vec![system.clone(), task.clone(), old.clone()],
                anchor: Vec::new(),
                previous_summary: None,
                compacted: vec![task.clone()],
                recent: vec![old.clone()],
            },
            Case {
                name: "earlier summary and stale goal leave the compacted span",
                messages: [
                    vec![
                        system.clone(),
                        task.clone(),
                        goal_anchor,
                        earlier_summary,
                        old.clone(),
                    ],
                    recent(),
                ]
                .concat(),
                anchor: vec![task.clone()],
                previous_summary: Some("earlier"),
                compacted: vec![old.clone()],
                recent: recent(),
            },
        ];

        for case in cases {
            let partition = partition_messages_for_compaction(&case.messages, &[], 1_000).unwrap();
            assert_eq!(
                partition.leading_messages,
                vec![system.clone()],
                "{}",
                case.name
            );
            assert_eq!(partition.anchor_messages, case.anchor, "{}", case.name);
            assert_eq!(
                partition.previous_summary.as_deref(),
                case.previous_summary,
                "{}",
                case.name
            );
            assert_eq!(
                partition.compacted_messages, case.compacted,
                "{}",
                case.name
            );
            assert_eq!(partition.recent_messages, case.recent, "{}", case.name);
        }
    }

    // Covers: legacy and enriched assistant tool calls stay with their results
    // in the recent tail.
    // Owner: compaction partition
    #[test]
    fn partition_does_not_split_assistant_tool_call_group() {
        let tool_call = || {
            ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: json!({"command": "echo hi"}),
            })
        };
        let enriched = Message::assistant(rho_providers::model::AssistantMessage {
            content: vec![tool_call()],
            provenance: Some(rho_providers::model::ModelIdentity::new(
                "openai-codex",
                "openai-responses",
                "gpt-test",
            )),
            reasoning_summary: None,
            provider_context: Vec::new(),
        });
        let cases = [
            ("legacy", Message::Assistant(vec![tool_call()])),
            ("enriched", enriched),
        ];

        for (case, assistant) in cases {
            let messages = vec![
                Message::System("system".into()),
                // Over the first-turn anchor budget, so it is compacted.
                Message::user_text("x".repeat(20_000)),
                assistant.clone(),
                Message::ToolResult(ToolResult {
                    id: "call_1".into(),
                    ok: true,
                    content: "hi".into(),
                }),
                Message::user_text("new"),
            ];

            let partition = partition_messages_for_compaction(&messages, &[], 700).unwrap();

            assert!(
                matches!(partition.compacted_messages.as_slice(), [Message::User(_)]),
                "{case}"
            );
            assert!(
                matches!(
                    partition.recent_messages.as_slice(),
                    [a, Message::ToolResult(_), Message::User(_)] if *a == assistant
                ),
                "{case}"
            );
        }
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
}

#[cfg(test)]
#[path = "compaction_group_tests.rs"]
mod group_tests;
