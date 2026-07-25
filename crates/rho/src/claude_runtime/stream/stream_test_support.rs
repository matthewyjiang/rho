use pretty_assertions::assert_eq;

use crate::run_artifacts::AttachmentEvent;

use super::*;

pub(super) fn effects_from_fixture(name: &str) -> Vec<StreamEffect> {
    let body = match name {
        "success.ndjson" => include_str!("../fixtures/success.ndjson"),
        "live_success.ndjson" => include_str!("../fixtures/live_success.ndjson"),
        "tool_call.ndjson" => include_str!("../fixtures/tool_call.ndjson"),
        "error_result.ndjson" => include_str!("../fixtures/error_result.ndjson"),
        "unknown_and_partial.ndjson" => include_str!("../fixtures/unknown_and_partial.ndjson"),
        "mixed_partial_complete.ndjson" => {
            include_str!("../fixtures/mixed_partial_complete.ndjson")
        }
        "partial_tool_complete_text.ndjson" => {
            include_str!("../fixtures/partial_tool_complete_text.ndjson")
        }
        "message_start_no_deltas.ndjson" => {
            include_str!("../fixtures/message_start_no_deltas.ndjson")
        }
        "missing_message_id.ndjson" => include_str!("../fixtures/missing_message_id.ndjson"),
        "indexless_partial_complete.ndjson" => {
            include_str!("../fixtures/indexless_partial_complete.ndjson")
        }
        "live_tool_roundtrip.ndjson" => include_str!("../fixtures/live_tool_roundtrip.ndjson"),
        other => panic!("unknown fixture {other}"),
    };
    let mut mapper = StreamMapper::new();
    body.lines()
        .flat_map(|line| mapper.push_line(line))
        .collect()
}

pub(super) fn count_attachments(
    effects: &[StreamEffect],
    predicate: impl Fn(&AttachmentEvent) -> bool,
) -> usize {
    effects
        .iter()
        .filter(|effect| match effect {
            StreamEffect::Attachment(event) => predicate(event),
            _ => false,
        })
        .count()
}

pub(super) fn joined_text(effects: &[StreamEffect]) -> String {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::AssistantTextDelta(text)) => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect()
}

pub(super) fn joined_reasoning(effects: &[StreamEffect]) -> String {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::ReasoningDelta(text)) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub(super) fn assert_no_terminal_attachment(effects: &[StreamEffect]) {
    assert_eq!(
        count_attachments(effects, |event| {
            matches!(
                event,
                AttachmentEvent::Completed | AttachmentEvent::Failed(_)
            )
        }),
        0,
        "result mapping must not emit Completed/Failed"
    );
}
