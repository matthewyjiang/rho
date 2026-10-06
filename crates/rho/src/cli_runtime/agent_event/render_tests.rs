use agent_client_protocol::schema::v1::{
    ContentChunk, Cost, ImageContent, PlanEntry, PlanEntryPriority, SessionInfoUpdate, ToolCall,
    ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolFact};

use super::*;

fn attachment(event: AttachmentEvent) -> StreamEffect {
    StreamEffect::Attachment(event)
}

fn first_step(event: AttachmentEvent) -> Vec<StreamEffect> {
    vec![attachment(AttachmentEvent::StepStarted), attachment(event)]
}

fn read_call() -> ToolCall {
    ToolCall::new("read-1", "Read config")
        .kind(ToolKind::Read)
        .locations(vec![ToolCallLocation::new("/workspace/config.toml")])
}

fn read_card(status: ToolStatus) -> ToolCard {
    ToolCard::new(
        status,
        ToolFamily::FileCommand,
        ToolHeader::call("Read", Some("config.toml".into())),
    )
}

// Covers: ACP updates must not lose text, tool identity, diffs, or usage at the
// protocol/artifact seam. No existing CLI mapper exercises this typed schema.
// Owner: pure protocol-to-artifact translation.
#[test]
fn session_updates_render_as_artifacts() {
    struct Case {
        name: &'static str,
        updates: Vec<SessionUpdate>,
        expected: Vec<Vec<StreamEffect>>,
        result: &'static str,
    }
    let running = AttachmentEvent::ToolStarted {
        key: Some("read-1".into()),
        card: read_card(ToolStatus::Running).into(),
    };
    let failed = read_card(ToolStatus::Error)
        .with_facts(vec![ToolFact::Error {
            text: "denied".into(),
        }])
        .with_body(ToolBody::Lines(vec!["denied".into()]));
    let plan = Plan::new(vec![PlanEntry::new(
        "inspect",
        PlanEntryPriority::Medium,
        PlanEntryStatus::InProgress,
    )]);
    let plan_card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Form,
        ToolHeader::call("Plan", /*primary*/ None),
    )
    .with_body(ToolBody::Lines(vec!["◐ inspect".into()]));
    let cases = vec![
        Case {
            name: "assistant chunks accumulate without replay",
            updates: vec![
                SessionUpdate::AgentMessageChunk(ContentChunk::new("hel".into())),
                SessionUpdate::AgentMessageChunk(ContentChunk::new("lo".into())),
            ],
            expected: vec![
                vec![
                    attachment(AttachmentEvent::StepStarted),
                    attachment(AttachmentEvent::AssistantTextDelta("hel".into())),
                    StreamEffect::Status(StatusPatch {
                        last_activity: Some("assistant text".into()),
                        append_text: Some("hel".into()),
                        ..StatusPatch::default()
                    }),
                ],
                vec![
                    attachment(AttachmentEvent::AssistantTextDelta("lo".into())),
                    StreamEffect::Status(StatusPatch {
                        last_activity: Some("assistant text".into()),
                        append_text: Some("lo".into()),
                        ..StatusPatch::default()
                    }),
                ],
            ],
            result: "hello",
        },
        Case {
            name: "thoughts do not become final result text",
            updates: vec![SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                "consider".into(),
            ))],
            expected: vec![vec![
                attachment(AttachmentEvent::StepStarted),
                attachment(AttachmentEvent::ReasoningDelta("consider".into())),
                StreamEffect::Status(StatusPatch {
                    last_activity: Some("reasoning".into()),
                    ..StatusPatch::default()
                }),
            ]],
            result: "",
        },
        Case {
            name: "opaque content becomes a notice, not encoded text",
            updates: vec![SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::Image(ImageContent::new("base64-data", "image/png")),
            ))],
            expected: vec![vec![attachment(AttachmentEvent::Notice("[image]".into()))]],
            result: "",
        },
        Case {
            name: "pending tools start running cards",
            updates: vec![SessionUpdate::ToolCall(read_call())],
            expected: vec![first_step(running.clone())],
            result: "",
        },
        Case {
            name: "a tool finishes under the key it started with, failures as errors",
            updates: vec![
                SessionUpdate::ToolCall(read_call()),
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    "read-1",
                    ToolCallUpdateFields::new()
                        .status(ToolCallStatus::Failed)
                        .content(vec!["denied".into()]),
                )),
            ],
            expected: vec![
                first_step(running.clone()),
                vec![attachment(AttachmentEvent::ToolFinished {
                    key: Some("read-1".into()),
                    presentation: failed.into(),
                })],
            ],
            result: "",
        },
        Case {
            name: "unknown tool id still renders and retains later partial updates",
            updates: vec![
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    "unknown",
                    ToolCallUpdateFields::new()
                        .title("Inspect")
                        .kind(ToolKind::Read),
                )),
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    "unknown",
                    ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
                )),
            ],
            expected: vec![
                first_step(AttachmentEvent::ToolUpdated {
                    key: Some("unknown".into()),
                    card: ToolCard::new(
                        ToolStatus::Running,
                        ToolFamily::FileCommand,
                        ToolHeader::call("Read", Some("Inspect".into())),
                    )
                    .into(),
                }),
                vec![attachment(AttachmentEvent::ToolFinished {
                    key: Some("unknown".into()),
                    presentation: ToolCard::new(
                        ToolStatus::Ok,
                        ToolFamily::FileCommand,
                        ToolHeader::call("Read", Some("Inspect".into())),
                    )
                    .into(),
                })],
            ],
            result: "",
        },
        Case {
            name: "plan snapshots share one live card",
            updates: vec![SessionUpdate::Plan(plan.clone()), SessionUpdate::Plan(plan)],
            expected: vec![
                first_step(AttachmentEvent::ToolStarted {
                    key: Some("acp-plan".into()),
                    card: plan_card.clone().into(),
                }),
                vec![attachment(AttachmentEvent::ToolUpdated {
                    key: Some("acp-plan".into()),
                    card: plan_card.into(),
                })],
            ],
            result: "",
        },
        Case {
            name: "context and USD cost retain their distinct semantics",
            updates: vec![SessionUpdate::UsageUpdate(
                UsageUpdate::new(42, 100).cost(Cost::new(0.25, "USD")),
            )],
            expected: vec![vec![
                attachment(AttachmentEvent::ContextUsage(
                    ContextUsage::provider_reported(42, Some(100)),
                )),
                StreamEffect::Status(StatusPatch {
                    total_cost_usd: Some(0.25),
                    ..StatusPatch::default()
                }),
            ]],
            result: "",
        },
        Case {
            // The pinned schema has no constructible unknown SessionUpdate;
            // this is its known-unhandled path, not an unsafe fake enum variant.
            name: "user echoes and unsupported metadata do not duplicate output",
            updates: vec![
                SessionUpdate::UserMessageChunk(ContentChunk::new("echo".into())),
                SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new()),
            ],
            expected: vec![vec![], vec![]],
            result: "",
        },
    ];
    for case in cases {
        let mut renderer = EventRenderer::new("/workspace".into());
        let effects = case
            .updates
            .into_iter()
            .map(|update| renderer.render(AgentEvent::from(update)))
            .collect::<Vec<_>>();
        assert_eq!(
            (effects, renderer.result_text()),
            (case.expected, case.result),
            "{}",
            case.name
        );
    }
}

// Covers: a turn end closes the presentation step, so the next turn's first
// event opens a new one instead of merging into the previous turn.
// Owner: pure protocol-to-artifact translation.
#[test]
fn turn_end_restarts_the_presentation_step() {
    let mut renderer = EventRenderer::new("/workspace".into());
    let running = AttachmentEvent::ToolStarted {
        key: Some("read-1".into()),
        card: read_card(ToolStatus::Running).into(),
    };
    let effects = [
        AgentEvent::from(SessionUpdate::ToolCall(read_call())),
        AgentEvent::TurnEnded(StopReason::MaxTokens),
        AgentEvent::from(SessionUpdate::ToolCall(read_call())),
    ]
    .map(|event| renderer.render(event));
    assert_eq!(
        effects,
        [
            first_step(running.clone()),
            vec![StreamEffect::Status(StatusPatch {
                last_activity: Some("turn ended: max_tokens".into()),
                ..StatusPatch::default()
            })],
            first_step(running),
        ]
    );
}

// Covers: saturation cannot drop a completion or grow retained state/result
// without bound. Later updates must render even when their start was not retained.
// Owner: pure protocol-to-artifact translation.
#[test]
fn saturated_state_remains_bounded_and_unknown_completion_renders() {
    use crate::cli_runtime::stream_effect::MAX_RESULT_CHARS;

    let mut renderer = EventRenderer::new("/workspace".into());
    for index in 0..MAX_ACTIVE_TOOLS {
        renderer.render(AgentEvent::from(SessionUpdate::ToolCall(ToolCall::new(
            index.to_string(),
            "Tool",
        ))));
    }
    let effects = renderer.render(AgentEvent::from(SessionUpdate::ToolCall(ToolCall::new(
        "overflow", "Tool",
    ))));
    assert_eq!(
        (renderer.active_tools.len(), effects),
        (MAX_ACTIVE_TOOLS, vec![
            attachment(AttachmentEvent::ToolStarted {
                key: Some("overflow".into()),
                card: ToolCard::new(ToolStatus::Running, ToolFamily::Default, ToolHeader::call("Tool", /*primary*/ None)).into(),
            }),
            attachment(AttachmentEvent::Notice(format!(
                "acp: active tool budget {MAX_ACTIVE_TOOLS}, requested {}; card renders without retained update state",
                MAX_ACTIVE_TOOLS + 1,
            ))),
        ]),
    );
    let completion = renderer.render(AgentEvent::from(SessionUpdate::ToolCallUpdate(
        ToolCallUpdate::new(
            "overflow",
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        ),
    )));
    assert_eq!(
        completion,
        vec![attachment(AttachmentEvent::ToolFinished {
            key: Some("overflow".into()),
            presentation: ToolCard::new(
                ToolStatus::Ok,
                ToolFamily::Default,
                ToolHeader::call("Tool", /*primary*/ None)
            )
            .into(),
        })]
    );
    for _ in 0..2 {
        renderer.render(AgentEvent::from(SessionUpdate::AgentMessageChunk(
            ContentChunk::new("界".repeat(MAX_RESULT_CHARS).into()),
        )));
    }
    assert_eq!(
        renderer.result_text(),
        format!("{}… [truncated result]", "界".repeat(MAX_RESULT_CHARS))
    );
}

// Covers: result.json must carry the final answer, not pre-tool narration or an
// earlier turn's reply (a head-truncated whole-session log could drop it).
// Owner: renderer result accounting.
#[test]
fn result_text_is_the_last_assistant_segment() {
    let text = |t: &str| {
        AgentEvent::from(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            t.into(),
        )))
    };
    let cases: Vec<(&str, Vec<AgentEvent>, &str)> = vec![
        (
            "narration before a tool call is replaced by the answer after it",
            vec![
                text("I'll read the config."),
                AgentEvent::from(SessionUpdate::ToolCall(read_call())),
                text("DONE"),
            ],
            "DONE",
        ),
        (
            "a tool call with no later text keeps the earlier segment",
            vec![
                text("partial"),
                AgentEvent::from(SessionUpdate::ToolCall(read_call())),
            ],
            "partial",
        ),
        (
            "a follow-up turn's reply replaces the previous turn's reply",
            vec![
                text("first"),
                AgentEvent::TurnEnded(StopReason::EndTurn),
                text("second"),
            ],
            "second",
        ),
    ];
    for (name, events, expected) in cases {
        let mut renderer = EventRenderer::new(PathBuf::from("/workspace"));
        for event in events {
            renderer.render(event);
        }
        assert_eq!(renderer.result_text(), expected, "{name}");
    }
}
