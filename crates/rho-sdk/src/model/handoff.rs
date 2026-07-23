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
/// Opaque provider context is replayed only to the exact provider/API/model that
/// produced it. When that context cannot replay, portable fallback text (then
/// reasoning summaries) is appended so foreign targets still receive a usable
/// handoff. Raw reasoning must be represented only as opaque context.
pub fn prepare_assistant(message: AssistantMessage, target: &ModelIdentity) -> PreparedAssistant {
    let mut content = message.content;
    let replay_context = message
        .provider_context
        .into_iter()
        .filter(|block| block.is_replayable_to(target))
        .collect::<Vec<_>>();
    if replay_context.is_empty() {
        if let Some(fallback) = message
            .portable_fallback
            .filter(|text| !text.trim().is_empty())
        {
            content.push(ContentBlock::Text(fallback));
        } else if let Some(summary) = message
            .reasoning_summary
            .filter(|summary| !summary.trim().is_empty())
        {
            content.push(ContentBlock::Text(format!(
                "{REASONING_SUMMARY_OPEN}\n{summary}\n{REASONING_SUMMARY_CLOSE}"
            )));
        }
    }
    PreparedAssistant {
        content,
        replay_context,
    }
}

pub fn report_message_omissions(messages: &[Message], target: &ModelIdentity) -> HandoffReport {
    let mut report = report_omissions(
        messages.iter().filter_map(|message| match message {
            Message::EnrichedAssistant(message) => Some(message.as_ref()),
            Message::System(_)
            | Message::User(_)
            | Message::Assistant(_)
            | Message::AbortedAssistant(_)
            | Message::ToolResult(_) => None,
        }),
        target,
    );
    for message in messages {
        let Message::AbortedAssistant(message) = message else {
            continue;
        };
        collect_omissions(&message.provider_context, target, &mut report);
    }
    report.omitted_kinds.sort();
    report
}

pub fn report_omissions<'a>(
    messages: impl IntoIterator<Item = &'a AssistantMessage>,
    target: &ModelIdentity,
) -> HandoffReport {
    let mut report = HandoffReport::default();
    for message in messages {
        collect_omissions(&message.provider_context, target, &mut report);
    }
    report.omitted_kinds.sort();
    report
}

fn collect_omissions(
    blocks: &[ProviderContextBlock],
    target: &ModelIdentity,
    report: &mut HandoffReport,
) {
    for block in blocks {
        if !block.is_replayable_to(target) {
            report.omitted_provider_context += 1;
            if !report.omitted_kinds.contains(&block.kind) {
                report.omitted_kinds.push(block.kind.clone());
            }
        }
    }
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
