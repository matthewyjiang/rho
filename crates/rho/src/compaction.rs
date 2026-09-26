use std::ops::Range;

use rho_providers::model::{
    context::{estimate_context_tokens, estimate_message_tokens},
    ContentBlock, Message,
};
use rho_sdk::model::SemanticMessage;

use crate::history_message::HistoryMessage;
use rho_tools::tool::ToolSpec;

#[path = "compaction_elide.rs"]
mod elide;
#[path = "compaction_summary.rs"]
mod summary;
pub(crate) use elide::{elide_tool_results, recall_id, Elision};
pub(crate) use summary::{build_summary_request_messages, summary_replacement};

const SUMMARY_RESERVE_MIN_TOKENS: u64 = 512;
const SUMMARY_RESERVE_MAX_TOKENS: u64 = 8_192;
/// Largest first user turn, and largest latest user message, kept verbatim
/// through text-summary compaction; larger ones are only summarized. Typical
/// first turns are far below this.
const VERBATIM_USER_MAX_TOKENS: u64 = 2_048;

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

/// History split for text-summary compaction, as positions in the partitioned
/// history.
///
/// Replacement history keeps, in order: the leading system messages, the first
/// user turn when it fits [`VERBATIM_USER_MAX_TOKENS`], one new summary, the
/// latest user message when it would otherwise be only summarized, and the
/// recent tail. The summary an earlier compaction wrote is not summarized
/// again: the summarizer gets its text as the summary to update.
#[derive(Clone, Debug)]
pub(crate) struct CompactionPartition<'a> {
    messages: &'a [Message],
    first_turn: Range<usize>,
    previous_summary: Option<usize>,
    latest_user: Option<usize>,
    recent_start: usize,
}

impl<'a> CompactionPartition<'a> {
    /// The first user turn, kept verbatim ahead of the summary.
    pub(crate) fn first_turn(&self) -> &'a [Message] {
        &self.messages[self.first_turn.clone()]
    }

    /// Text of the summary an earlier compaction wrote.
    pub(crate) fn previous_summary(&self) -> Option<&'a str> {
        let summary = self.messages[self.previous_summary?].as_compaction_summary()?;
        Some(summary.text())
    }

    /// Messages the new summary covers, oldest first. The kept latest user
    /// message is included so the summary reads in order.
    pub(crate) fn summarized(&self) -> impl Iterator<Item = &'a Message> + '_ {
        self.compacted_range()
            .filter(|&index| Some(index) != self.previous_summary)
            .map(|index| &self.messages[index])
    }

    /// Positions between the first turn and the recent tail. Only tool
    /// results here may be elided.
    pub(crate) fn compacted_range(&self) -> Range<usize> {
        self.first_turn.end..self.recent_start
    }

    /// Replacement history around a newly written `summary`.
    pub(crate) fn replacement(&self, summary: Message) -> Vec<Message> {
        let mut replacement = self.messages[..self.first_turn.end].to_vec();
        replacement.push(summary);
        replacement.extend(self.latest_user.map(|index| self.messages[index].clone()));
        replacement.extend_from_slice(&self.messages[self.recent_start..]);
        replacement
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MessageGroup {
    start: usize,
    end: usize,
    tokens: u64,
}

/// Splits `messages` so the recent tail fits `target_tokens` alongside what
/// stays verbatim. Returns `None` when nothing would be summarized.
pub(crate) fn partition_messages_for_compaction<'a>(
    messages: &'a [Message],
    tools: &[ToolSpec],
    target_tokens: u64,
) -> Option<CompactionPartition<'a>> {
    partition(
        messages,
        tools,
        target_tokens,
        /*keep_user_turns*/ true,
    )
    // With nothing else to remove, summarize the kept user messages
    // rather than make compaction a no-op.
    .or_else(|| {
        partition(
            messages,
            tools,
            target_tokens,
            /*keep_user_turns*/ false,
        )
    })
}

fn partition<'a>(
    messages: &'a [Message],
    tools: &[ToolSpec],
    target_tokens: u64,
    keep_user_turns: bool,
) -> Option<CompactionPartition<'a>> {
    let leading_end = messages
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap_or(messages.len());
    let natural_first_turn_end = first_turn_end(messages, leading_end);
    // An earlier compaction put its summary right after the first turn. A
    // summary anywhere else is summarized like any other message.
    let previous_summary = messages
        .get(natural_first_turn_end)
        .is_some_and(|message| message.as_compaction_summary().is_some())
        .then_some(natural_first_turn_end);
    let first_turn_end = if keep_user_turns {
        natural_first_turn_end
    } else {
        leading_end
    };
    // Only messages after the earlier summary can stay in the recent tail.
    let history_start = previous_summary.map_or(first_turn_end, |index| index + 1);
    let groups = message_groups(messages, history_start);

    let fixed_tokens = estimate_context_tokens(&messages[..first_turn_end], tools);
    let mut recent_start = recent_tail_start(
        &groups,
        recent_tail_token_budget(fixed_tokens, target_tokens),
    )?;
    // The latest user message overall, when the tail does not already hold it.
    let latest_user = messages[history_start..]
        .iter()
        .rposition(is_user_input)
        .map(|offset| history_start + offset)
        .filter(|&index| {
            keep_user_turns
                && index < recent_start
                && estimate_message_tokens(&messages[index]) <= VERBATIM_USER_MAX_TOKENS
        });
    if let Some(index) = latest_user {
        // Less budget only moves the tail later, so `index` stays before it.
        let fixed_tokens = fixed_tokens.saturating_add(estimate_message_tokens(&messages[index]));
        recent_start = recent_tail_start(
            &groups,
            recent_tail_token_budget(fixed_tokens, target_tokens),
        )?;
    }
    // Rewriting the earlier summary around a restated latest user message
    // alone would remove nothing.
    (first_turn_end..recent_start)
        .any(|index| Some(index) != previous_summary && Some(index) != latest_user)
        .then_some(CompactionPartition {
            messages,
            first_turn: leading_end..first_turn_end,
            previous_summary,
            latest_user,
            recent_start,
        })
}

/// Input a person sent, as opposed to tool images or compaction summaries.
fn is_user_input(message: &Message) -> bool {
    matches!(HistoryMessage::of(message), HistoryMessage::User(_))
}

/// End of the contiguous user messages at `start`, or `start` when together
/// they exceed [`VERBATIM_USER_MAX_TOKENS`].
fn first_turn_end(messages: &[Message], start: usize) -> usize {
    let end = messages[start..]
        .iter()
        .position(|message| !is_user_input(message))
        .map_or(messages.len(), |offset| start + offset);
    let tokens: u64 = messages[start..end]
        .iter()
        .map(estimate_message_tokens)
        .sum();
    if tokens <= VERBATIM_USER_MAX_TOKENS {
        end
    } else {
        start
    }
}

fn recent_tail_token_budget(fixed_tokens: u64, target_tokens: u64) -> u64 {
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

    // Covers: the first user turn and the latest user message stay verbatim
    // only within their budget and only when something else is summarized; an
    // earlier summary is handed over for updating instead of being summarized.
    // Owner: compaction partition
    #[test]
    fn partition_keeps_user_turns_and_updates_previous_summary() {
        struct Case {
            name: &'static str,
            messages: Vec<Message>,
            previous_summary: Option<&'static str>,
            summarized: Vec<Message>,
            replacement: Vec<Message>,
        }
        let system = Message::System("system".into());
        let task = Message::user_text("x".repeat(1_000));
        let huge_task = Message::user_text("x".repeat(20_000));
        let old = Message::assistant_text("y".repeat(2_000));
        let follow_up = Message::user_text("now do the second part");
        let summary = Message::compaction_summary(rho_sdk::CompactionTrigger::Manual, "new");
        let earlier = Message::compaction_summary(rho_sdk::CompactionTrigger::Automatic, "earlier");
        let recent = [
            Message::user_text("recent user"),
            Message::assistant_text("recent assistant"),
        ];
        let history = |messages: &[&[Message]]| messages.concat();
        let cases = [
            Case {
                name: "first turn kept",
                messages: history(&[&[system.clone(), task.clone(), old.clone()], &recent]),
                previous_summary: None,
                summarized: vec![old.clone()],
                replacement: history(&[&[system.clone(), task.clone(), summary.clone()], &recent]),
            },
            Case {
                name: "oversized first turn is only summarized",
                messages: history(&[&[system.clone(), huge_task.clone(), old.clone()], &recent]),
                previous_summary: None,
                summarized: vec![huge_task.clone(), old.clone()],
                replacement: history(&[&[system.clone(), summary.clone()], &recent]),
            },
            Case {
                name: "latest user message outside the tail is restated",
                messages: vec![
                    system.clone(),
                    task.clone(),
                    old.clone(),
                    follow_up.clone(),
                    old.clone(),
                    recent[1].clone(),
                ],
                previous_summary: None,
                summarized: vec![old.clone(), follow_up.clone(), old.clone()],
                replacement: vec![
                    system.clone(),
                    task.clone(),
                    summary.clone(),
                    follow_up.clone(),
                    recent[1].clone(),
                ],
            },
            Case {
                name: "single turn falls back to summarizing it",
                messages: vec![system.clone(), task.clone(), old.clone()],
                previous_summary: None,
                summarized: vec![task.clone()],
                replacement: vec![system.clone(), summary.clone(), old.clone()],
            },
            Case {
                name: "earlier summary is updated, not summarized",
                messages: history(&[
                    &[system.clone(), task.clone(), earlier.clone(), old.clone()],
                    &recent,
                ]),
                previous_summary: Some("earlier"),
                summarized: vec![old.clone()],
                replacement: history(&[&[system.clone(), task.clone(), summary.clone()], &recent]),
            },
        ];

        for case in cases {
            let partition = partition_messages_for_compaction(&case.messages, &[], 1_000).unwrap();
            assert_eq!(
                partition.previous_summary(),
                case.previous_summary,
                "{}",
                case.name
            );
            assert_eq!(
                partition.summarized().cloned().collect::<Vec<_>>(),
                case.summarized,
                "{}",
                case.name
            );
            assert_eq!(
                partition.replacement(summary.clone()),
                case.replacement,
                "{}",
                case.name
            );
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
                // Over the verbatim first-turn budget, so it is summarized.
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

            assert_eq!(
                partition.summarized().cloned().collect::<Vec<_>>(),
                vec![messages[1].clone()],
                "{case}"
            );
            assert_eq!(
                partition.compacted_range(),
                1..2,
                "{case}: the call group stays in the tail"
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

        assert_eq!(
            partition.summarized().cloned().collect::<Vec<_>>(),
            vec![messages[1].clone()]
        );
        assert_eq!(partition.compacted_range(), 1..2);
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
