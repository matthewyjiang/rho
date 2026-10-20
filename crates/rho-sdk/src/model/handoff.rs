use super::{AssistantMessage, ContentBlock, Message, ModelIdentity, ProviderContextBlock};

const REASONING_SUMMARY_OPEN: &str = "<reasoning_summary>";
const REASONING_SUMMARY_CLOSE: &str = "</reasoning_summary>";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HandoffReport {
    pub omitted_provider_context: usize,
    pub omitted_kinds: Vec<String>,
}

impl HandoffReport {
    pub fn has_omissions(&self) -> bool {
        self.omitted_provider_context > 0
    }
}

pub struct PreparedAssistant {
    pub content: Vec<ContentBlock>,
    pub replay_context: Vec<ProviderContextBlock>,
}

/// Lowers a canonical assistant message for a target model.
///
/// Opaque provider context follows [`ProviderContextBlock::is_replayable_to`].
/// When that context cannot replay, portable fallback text (then
/// reasoning summaries) is appended so foreign targets still receive a usable
/// handoff. Raw reasoning must be represented only as opaque context.
pub fn prepare_assistant(message: AssistantMessage, target: &ModelIdentity) -> PreparedAssistant {
    let portable_fallback = message.portable_fallback().map(str::to_owned);
    let mut content = message.content;
    let only_sdk_metadata = !message.provider_context.is_empty()
        && message
            .provider_context
            .iter()
            .all(ProviderContextBlock::is_sdk_metadata);
    let replay_context = message
        .provider_context
        .into_iter()
        .filter(|block| block.is_replayable_to(target))
        .collect::<Vec<_>>();
    if replay_context.is_empty() {
        if let Some(fallback) = portable_fallback.filter(|text| !text.trim().is_empty()) {
            content.push(ContentBlock::Text(fallback));
        } else if !only_sdk_metadata {
            if let Some(summary) = message
                .reasoning_summary
                .filter(|summary| !summary.trim().is_empty())
            {
                content.push(ContentBlock::Text(format!(
                    "{REASONING_SUMMARY_OPEN}\n{summary}\n{REASONING_SUMMARY_CLOSE}"
                )));
            }
        }
    }
    PreparedAssistant {
        content,
        replay_context,
    }
}

pub fn report_message_omissions(messages: &[Message], target: &ModelIdentity) -> HandoffReport {
    collect_omissions(omitted_message_context(messages, target))
}

/// Yields the provider-native blocks in `messages` that `target` cannot
/// replay, in history order: exactly the blocks [`report_message_omissions`]
/// counts. SDK metadata blocks are skipped.
pub fn omitted_message_context<'a>(
    messages: &'a [Message],
    target: &'a ModelIdentity,
) -> impl Iterator<Item = &'a ProviderContextBlock> + 'a {
    messages
        .iter()
        .flat_map(|message| match message {
            Message::EnrichedAssistant(message) => message.provider_context.as_slice(),
            Message::AbortedAssistant(message) => message.provider_context.as_slice(),
            Message::System(_)
            | Message::User(_)
            | Message::Assistant(_)
            | Message::ToolResult(_) => &[],
        })
        .filter(|block| is_omitted(block, target))
}

pub fn report_omissions<'a>(
    messages: impl IntoIterator<Item = &'a AssistantMessage>,
    target: &ModelIdentity,
) -> HandoffReport {
    collect_omissions(
        messages
            .into_iter()
            .flat_map(|message| &message.provider_context)
            .filter(|block| is_omitted(block, target)),
    )
}

fn is_omitted(block: &ProviderContextBlock, target: &ModelIdentity) -> bool {
    !block.is_sdk_metadata() && !block.is_replayable_to(target)
}

fn collect_omissions<'a>(omitted: impl Iterator<Item = &'a ProviderContextBlock>) -> HandoffReport {
    let mut report = HandoffReport::default();
    for block in omitted {
        report.omitted_provider_context += 1;
        if !report.omitted_kinds.contains(&block.kind) {
            report.omitted_kinds.push(block.kind.clone());
        }
    }
    report.omitted_kinds.sort();
    report
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
