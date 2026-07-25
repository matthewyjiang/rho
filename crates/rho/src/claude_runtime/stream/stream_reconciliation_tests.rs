use pretty_assertions::assert_eq;

use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

use super::stream_test_support::*;
use super::*;

#[test]
fn mixed_partial_and_complete_envelopes_emit_presentation_once() {
    let effects = effects_from_fixture("mixed_partial_complete.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::StepStarted)
        }),
        1,
        "StepStarted once"
    );
    assert_eq!(joined_text(&effects), "Reading now.");
    assert_eq!(joined_reasoning(&effects), "plan step");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        1,
        "ToolStarted once"
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1,
        "ToolFinished once"
    );
    assert_no_terminal_attachment(&effects);

    let terminal = effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal");
    assert!(terminal.classification.is_success());
    assert_eq!(
        terminal
            .usage
            .as_ref()
            .and_then(ModelUsage::total_input_tokens),
        Some(3 + 10 + 7)
    );
    assert_eq!(
        terminal.context.as_ref().and_then(|context| context.tokens),
        Some(3 + 10 + 7)
    );
}

#[test]
fn partial_tool_only_plus_complete_only_text_and_reasoning() {
    // Tool streamed via deltas; text and reasoning arrive only on the complete
    // assistant envelope. Each presentation event must fire exactly once.
    let effects = effects_from_fixture("partial_tool_complete_text.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_reasoning(&effects), "think complete-only");
    assert_eq!(joined_text(&effects), "text complete-only");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { display_lines }
                if display_lines.iter().any(|line| line.contains("toolu_partial_1")))
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn message_start_without_deltas_emits_complete_blocks_once() {
    let effects = effects_from_fixture("message_start_no_deltas.ndjson");
    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_text(&effects), "only on complete");
    assert_eq!(joined_reasoning(&effects), "reason complete");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn missing_message_ids_do_not_share_fallback_or_duplicate() {
    let effects = effects_from_fixture("missing_message_id.ndjson");
    // Two distinct complete assistants without ids must each emit StepStarted
    // and their own text once. No shared "unknown" key merges them.
    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        2
    );
    assert_eq!(joined_text(&effects), "first second");
    assert_no_terminal_attachment(&effects);
}

#[test]
fn indexless_partials_do_not_duplicate_on_complete_envelope() {
    // Index-less text/thinking partials must still mark emission so the
    // complete assistant envelope does not replay the same presentation. An
    // indexed tool block in the same message keeps normal index bookkeeping.
    let effects = effects_from_fixture("indexless_partial_complete.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_reasoning(&effects), "plan first");
    assert_eq!(joined_text(&effects), "Hello indexless.");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { display_lines }
                if display_lines.iter().any(|line| line.contains("toolu_indexless_1")))
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::AssistantTextDelta(_))
        }),
        2,
        "two index-less text deltas only; complete envelope must not re-emit"
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ReasoningDelta(_))
        }),
        2,
        "two index-less reasoning deltas only; complete envelope must not re-emit"
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn indexless_partials_preserve_mixed_indexed_complete_only_blocks() {
    // Index-less reasoning streamed; indexed text arrives only on complete.
    let lines = [
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"id":"msg_idxless_mix","role":"assistant"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"thinking","thinking":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"streamed"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","session_id":"s","message":{"id":"msg_idxless_mix","role":"assistant","content":[{"type":"thinking","thinking":"streamed"},{"type":"text","text":"complete-only text"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
    ];
    let mut mapper = StreamMapper::new();
    let effects: Vec<_> = lines
        .iter()
        .flat_map(|line| mapper.push_line(line))
        .collect();

    assert_eq!(joined_reasoning(&effects), "streamed");
    assert_eq!(joined_text(&effects), "complete-only text");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ReasoningDelta(_))
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::AssistantTextDelta(_))
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn ordered_indexless_slots_preserve_complete_only_then_streamed_text() {
    // Two same-kind text blocks: first is complete-only (empty start, no deltas),
    // second streams indexless. Ordered slots must emit each once in order.
    let lines = [
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"id":"msg_ord_text","role":"assistant"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"second"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","session_id":"s","message":{"id":"msg_ord_text","role":"assistant","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
    ];
    let mut mapper = StreamMapper::new();
    let effects: Vec<_> = lines
        .iter()
        .flat_map(|line| mapper.push_line(line))
        .collect();

    assert_eq!(joined_text(&effects), "secondfirst");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::AssistantTextDelta(_))
        }),
        2
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn ordered_indexless_slots_preserve_complete_only_then_streamed_reasoning() {
    let lines = [
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"id":"msg_ord_reason","role":"assistant"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"thinking","thinking":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"thinking","thinking":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"streamed"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","session_id":"s","message":{"id":"msg_ord_reason","role":"assistant","content":[{"type":"thinking","thinking":"complete-only"},{"type":"thinking","thinking":"streamed"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
    ];
    let mut mapper = StreamMapper::new();
    let effects: Vec<_> = lines
        .iter()
        .flat_map(|line| mapper.push_line(line))
        .collect();

    assert_eq!(joined_reasoning(&effects), "streamedcomplete-only");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ReasoningDelta(_))
        }),
        2
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn ordered_indexless_slots_preserve_complete_only_then_streamed_tool() {
    // Two index-less tool starts both present on the stream; the complete
    // envelope must not re-emit either ToolStarted.
    let lines = [
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"id":"msg_ord_tool","role":"assistant"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"toolu_a","name":"Read","input":{}}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"toolu_b","name":"Bash","input":{}}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop"}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","session_id":"s","message":{"id":"msg_ord_tool","role":"assistant","content":[{"type":"tool_use","id":"toolu_a","name":"Read","input":{"path":"a"}},{"type":"tool_use","id":"toolu_b","name":"Bash","input":{"cmd":"true"}}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
    ];
    let mut mapper = StreamMapper::new();
    let effects: Vec<_> = lines
        .iter()
        .flat_map(|line| mapper.push_line(line))
        .collect();

    let started: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::ToolStarted { display_lines }) => {
                Some(display_lines.join(" | "))
            }
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 2, "both tools once: {started:?}");
    assert!(started[0].contains("toolu_a") && started[0].contains("Read"));
    assert!(started[1].contains("toolu_b") && started[1].contains("Bash"));
    // Complete envelope must not re-emit either tool.
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        2
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn some_blocks_streamed_others_complete_only_within_one_message() {
    // index 0 thinking streamed; index 1 text complete-only; index 2 tool streamed.
    let lines = [
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"id":"msg_mix","role":"assistant"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"streamed think"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop","index":0}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_x","name":"Read","input":{}}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop","index":2}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","session_id":"s","message":{"id":"msg_mix","role":"assistant","content":[{"type":"thinking","thinking":"streamed think"},{"type":"text","text":"complete-only text"},{"type":"tool_use","id":"toolu_x","name":"Read","input":{"path":"a"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_x","content":"ok"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
    ];
    let mut mapper = StreamMapper::new();
    let effects: Vec<_> = lines
        .iter()
        .flat_map(|line| mapper.push_line(line))
        .collect();

    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_reasoning(&effects), "streamed think");
    assert_eq!(joined_text(&effects), "complete-only text");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn maps_live_tool_roundtrip_capture() {
    let effects = effects_from_fixture("live_tool_roundtrip.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { display_lines }
                if display_lines.iter().any(|line| line.contains("Read"))
                    && display_lines.iter().any(|line| line.contains("toolu_0liveToolRoundtrip")))
        }),
        1,
        "tool start"
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1,
        "tool finish"
    );
    assert!(
        joined_text(&effects).contains("rho-tool-fixture-marker-42"),
        "assistant final text: {}",
        joined_text(&effects)
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::StepStarted)
        }),
        2,
        "exactly one StepStarted per assistant message despite mid-stream snapshots"
    );

    let terminal = effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal");
    assert!(terminal.classification.is_success());
    assert_eq!(terminal.num_turns, Some(2));
    assert_eq!(
        terminal.result_text.as_deref(),
        Some("rho-tool-fixture-marker-42")
    );
    assert_eq!(
        terminal.session_id.as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    let usage = terminal.usage.as_ref().expect("usage");
    assert_eq!(usage.input_tokens, Some(4));
    assert_eq!(usage.cache_read_tokens, Some(14452));
    assert_eq!(usage.cache_write_tokens, Some(5604));
    assert_eq!(usage.output_tokens, Some(102));
    assert_eq!(usage.total_input_tokens(), Some(4 + 14452 + 5604));

    let mut status = crate::subagent::RunStatus::default();
    for effect in &effects {
        if let StreamEffect::Status(patch) = effect {
            apply_status_patch(&mut status, patch.clone());
        }
    }
    assert_ne!(status.state, RunState::Ok);
    assert_ne!(status.state, RunState::Error);
    assert_eq!(status.turns, 2);
    assert_eq!(status.input_tokens, 4 + 14452 + 5604);
    assert_eq!(status.output_tokens, 102);
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    assert_eq!(status.result.as_deref(), Some("rho-tool-fixture-marker-42"));
    assert_no_terminal_attachment(&effects);

    // Heartbeat system/status frames from the live capture must stay quiet.
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::Notice(text))
                if text.contains("claude system: status")
        )),
        "live capture must not spam status notices"
    );
}
