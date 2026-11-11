use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, CompactionId, CompactionStatus, CompactionUpdate,
    ConfigOptionUpdate, ContentBlock, ContentChunk, Cost, CurrentModeUpdate, Diff, MessageId,
    Notice, NoticeSeverity, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, ResourceLink,
    SessionId, SessionInfoUpdate, SessionModeId, SessionUpdate, ToolCall, ToolCallContent,
    ToolCallId, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    UsageUpdate,
};
use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, SessionEvent};
use serde_json::json;

use super::{notification, options_changed, Delivery, Updates};
use crate::app::acp::{host_features::HostFeatures, options};

fn features(compaction: bool, notices: bool) -> HostFeatures {
    HostFeatures {
        compaction,
        notices,
        ..HostFeatures::default()
    }
}

fn updates(features: HostFeatures) -> Updates {
    Updates::with_nonce(features, "n".to_owned())
}

fn message(id: &str, text: &str) -> ContentChunk {
    ContentChunk::new(ContentBlock::from(text)).message_id(MessageId::new(id))
}

fn tool_update(id: &str) -> wire::ToolCallUpdate {
    wire::ToolCallUpdate {
        id: wire::ToolCallId::new(id),
        title: None,
        kind: None,
        status: None,
        raw_input: None,
        content: None,
        locations: None,
        display: None,
    }
}

// Covers: each transcript event maps to the ACP update of the same meaning,
// field for field: message chunks keep the daemon's message ids, user
// messages drop blocks this rho cannot show, usage reports an unknown window
// as 0 and omits an unpriced cost, and cards and notices fall back to what a
// host without those features shows
// Owner: ACP event mapping
#[test]
fn transcript_events_map_to_acp_updates() {
    let plain = features(/*compaction*/ false, /*notices*/ false);
    let rich = features(/*compaction*/ true, /*notices*/ true);
    let cases = [
        (
            "user message",
            plain,
            SessionEvent::UserMessage {
                message: wire::MessageId::new("u1"),
                content: vec![
                    wire::ContentBlock::Text {
                        text: "read this".into(),
                    },
                    wire::ContentBlock::ResourceLink {
                        uri: "file:///a.md".into(),
                        name: "a.md".into(),
                        description: None,
                    },
                    wire::ContentBlock::Unknown,
                ],
                steer: None,
            },
            vec![
                SessionUpdate::UserMessageChunk(message("u1", "read this")),
                SessionUpdate::UserMessageChunk(
                    ContentChunk::new(ContentBlock::ResourceLink(ResourceLink::new(
                        "a.md",
                        "file:///a.md",
                    )))
                    .message_id(MessageId::new("u1")),
                ),
            ],
        ),
        (
            "agent chunk",
            plain,
            SessionEvent::AgentMessageChunk {
                message: wire::MessageId::new("m1"),
                text: "hi".into(),
            },
            vec![SessionUpdate::AgentMessageChunk(message("m1", "hi"))],
        ),
        (
            "thought chunk",
            plain,
            SessionEvent::ThoughtChunk {
                message: wire::MessageId::new("t1"),
                text: "hmm".into(),
            },
            vec![SessionUpdate::AgentThoughtChunk(message("t1", "hmm"))],
        ),
        (
            "plan",
            plain,
            SessionEvent::Plan {
                entries: vec![
                    wire::PlanEntry {
                        content: "write".into(),
                        status: wire::PlanStatus::InProgress,
                    },
                    wire::PlanEntry {
                        content: "later".into(),
                        status: wire::PlanStatus::Unknown,
                    },
                ],
            },
            vec![SessionUpdate::Plan(Plan::new(vec![
                PlanEntry::new(
                    "write",
                    PlanEntryPriority::Medium,
                    PlanEntryStatus::InProgress,
                ),
                PlanEntry::new("later", PlanEntryPriority::Medium, PlanEntryStatus::Pending),
            ]))],
        ),
        (
            "priced usage",
            plain,
            SessionEvent::Usage {
                used_tokens: 1200,
                window_tokens: Some(200_000),
                cost_usd_micros: Some(1_500_000),
            },
            vec![SessionUpdate::UsageUpdate(
                UsageUpdate::new(1200, 200_000).cost(Cost::new(1.5, "USD")),
            )],
        ),
        (
            "unpriced usage of an unknown window",
            plain,
            SessionEvent::Usage {
                used_tokens: 10,
                window_tokens: None,
                cost_usd_micros: None,
            },
            vec![SessionUpdate::UsageUpdate(UsageUpdate::new(10, 0))],
        ),
        (
            "title",
            plain,
            SessionEvent::TitleChanged {
                title: "Fix the build".into(),
            },
            vec![SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title("Fix the build".to_owned()),
            )],
        ),
        (
            "compaction to a host with cards",
            rich,
            SessionEvent::Compaction {
                id: wire::CompactionId::new("compaction-1"),
                status: wire::CompactionStatus::Failed {
                    error: "no provider".into(),
                },
            },
            vec![SessionUpdate::CompactionUpdate(
                CompactionUpdate::new(CompactionId::new("compaction-1"), CompactionStatus::Failed)
                    .error("no provider".to_owned()),
            )],
        ),
        (
            "compaction to a host without cards",
            plain,
            SessionEvent::Compaction {
                id: wire::CompactionId::new("compaction-1"),
                status: wire::CompactionStatus::Started {
                    trigger: Some(wire::CompactionTrigger::Command),
                },
            },
            vec![],
        ),
        (
            "discarded response",
            plain,
            SessionEvent::ResponseDiscarded,
            vec![SessionUpdate::AgentThoughtChunk(message(
                "rho-msg-n-1",
                "[provider response discarded; retrying]",
            ))],
        ),
        (
            "error card",
            plain,
            SessionEvent::Card {
                title: "Job failed".into(),
                markdown: "**exit 1**".into(),
                plain: "exit 1".into(),
                tone: wire::CardTone::Error,
                display: None,
            },
            vec![SessionUpdate::ToolCall(
                ToolCall::new(ToolCallId::new("rho-notice-n-1"), "Job failed")
                    .kind(ToolKind::Other)
                    .status(ToolCallStatus::Failed)
                    .content(vec![ToolCallContent::from("**exit 1**")])
                    .raw_output(json!("exit 1")),
            )],
        ),
        (
            "card without a body",
            plain,
            SessionEvent::Card {
                title: "Job done".into(),
                markdown: String::new(),
                plain: "  ".into(),
                tone: wire::CardTone::Success,
                display: None,
            },
            vec![SessionUpdate::ToolCall(
                ToolCall::new(ToolCallId::new("rho-notice-n-1"), "Job done")
                    .kind(ToolKind::Other)
                    .status(ToolCallStatus::Completed),
            )],
        ),
        (
            "notice to a host with notices",
            rich,
            SessionEvent::Notice {
                text: "plan approved".into(),
            },
            vec![SessionUpdate::Notice(Notice::new(
                NoticeSeverity::Info,
                "plan approved",
            ))],
        ),
        (
            "notice to a host without notices",
            plain,
            SessionEvent::Notice {
                text: "plan approved".into(),
            },
            vec![SessionUpdate::AgentMessageChunk(message(
                "rho-msg-n-1",
                "plan approved",
            ))],
        ),
        (
            "state is the pump's",
            rich,
            SessionEvent::State {
                state: wire::JobState::Running,
                turn: Some(wire::TurnId::new("t1")),
            },
            vec![],
        ),
        ("unknown event", rich, SessionEvent::Unknown, vec![]),
    ];
    for (label, features, event, expected) in cases {
        assert_eq!(updates(features).map(&event), expected, "{label}");
    }
}

// Covers: the first report of a tool call opens its card, defaulting what it
// leaves out; later reports patch only the fields they carry, so a status
// change does not blank the host's title or content
// Owner: ACP event mapping
#[test]
fn tool_calls_open_a_card_then_patch_it() {
    let mut mapping = updates(HostFeatures::default());
    let started = wire::ToolCallUpdate {
        title: Some("Edit src/lib.rs".into()),
        kind: Some(wire::ToolKind::Edit),
        raw_input: Some(json!({"path": "src/lib.rs"})),
        locations: Some(vec![wire::Location {
            path: "/w/src/lib.rs".into(),
            line: Some(3),
        }]),
        ..tool_update("c1")
    };
    let finished = wire::ToolCallUpdate {
        status: Some(wire::ToolStatus::Completed),
        content: Some(vec![
            wire::ToolContent::Diff {
                path: "/w/src/lib.rs".into(),
                old_text: Some("a".into()),
                new_text: "b".into(),
                deleted: false,
            },
            wire::ToolContent::Diff {
                path: "/w/src/gone.rs".into(),
                old_text: Some("c".into()),
                new_text: String::new(),
                deleted: true,
            },
            wire::ToolContent::Unknown,
        ]),
        ..tool_update("c1")
    };
    let unnamed = wire::ToolCallUpdate {
        kind: Some(wire::ToolKind::Unknown),
        status: Some(wire::ToolStatus::Unknown),
        ..tool_update("c2")
    };

    let mapped = [started, finished, unnamed]
        .into_iter()
        .flat_map(|update| mapping.map(&SessionEvent::tool_call(update)))
        .collect::<Vec<_>>();

    assert_eq!(
        mapped,
        [
            SessionUpdate::ToolCall(
                ToolCall::new(ToolCallId::new("c1"), "Edit src/lib.rs")
                    .kind(ToolKind::Edit)
                    .status(ToolCallStatus::Pending)
                    .raw_input(json!({"path": "src/lib.rs"}))
                    .locations(vec![ToolCallLocation::new("/w/src/lib.rs").line(3_u32)]),
            ),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("c1"),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .content(vec![
                        ToolCallContent::Diff(
                            Diff::new("/w/src/lib.rs", "b").old_text("a".to_owned()),
                        ),
                        // v1 cannot say a file is gone; `_meta` does.
                        ToolCallContent::Diff(
                            Diff::new("/w/src/gone.rs", "")
                                .old_text("c".to_owned())
                                .meta(serde_json::Map::from_iter([(
                                    "rho/fileDeleted".to_owned(),
                                    json!(true),
                                )])),
                        ),
                    ]),
            )),
            SessionUpdate::ToolCall(
                ToolCall::new(ToolCallId::new("c2"), "tool")
                    .kind(ToolKind::Other)
                    .status(ToolCallStatus::InProgress),
            ),
        ]
    );
}

fn session_options(mode: wire::PermissionMode, fast: bool) -> wire::SessionOptions {
    let current = wire::ModelRef {
        provider: "openai".into(),
        model: "gpt-5".into(),
    };
    wire::SessionOptions {
        model: wire::ModelOptions {
            auth: "api-key".into(),
            current: current.clone(),
            available: vec![wire::ModelChoice {
                model: current,
                label: "GPT-5".into(),
            }],
        },
        reasoning: None,
        fast_mode: Some(wire::FastModeOptions { enabled: fast }),
        permission_mode: wire::PermissionModeOptions {
            current: mode,
            available: vec![wire::PermissionMode::Plan, wire::PermissionMode::Auto],
        },
        commands: Vec::new(),
        added_dirs: Vec::new(),
    }
}

// Covers: an options change tells the host the whole config when an option
// moved, the mode when it switched, and the menu when commands changed, and
// nothing at all when nothing did, so repeated changes do not spam hosts
// Owner: ACP event mapping
#[test]
fn options_changes_send_only_what_moved() {
    let features = HostFeatures::default();
    let plan = session_options(wire::PermissionMode::Plan, /*fast*/ false);
    let auto = session_options(wire::PermissionMode::Auto, /*fast*/ false);
    let fast = session_options(wire::PermissionMode::Plan, /*fast*/ true);
    let mut with_command = plan.clone();
    with_command.commands.push(wire::SessionCommand {
        name: "init".into(),
        kind: wire::CommandKind::Slash,
        description: "Write AGENTS.md".into(),
        input_hint: None,
    });
    let config = |options: &wire::SessionOptions| {
        SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options::config_options(
            options,
            features.toggles,
        )))
    };
    let cases = [
        ("nothing", &plan, vec![]),
        (
            "mode",
            &auto,
            vec![
                config(&auto),
                SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(SessionModeId::new(
                    "auto",
                ))),
            ],
        ),
        ("fast mode", &fast, vec![config(&fast)]),
        (
            "commands",
            &with_command,
            vec![SessionUpdate::AvailableCommandsUpdate(
                AvailableCommandsUpdate::new(vec![AvailableCommand::new(
                    "init",
                    "Write AGENTS.md",
                )]),
            )],
        ),
    ];
    for (label, new, expected) in cases {
        assert_eq!(options_changed(&plan, new, features), expected, "{label}");
    }
}

// Covers: replayed history carries `_meta.isReplay` so hosts with their own
// transcript can skip it; live updates carry no marker
// Owner: ACP event mapping
#[test]
fn replayed_updates_are_marked() {
    let update = || SessionUpdate::AgentMessageChunk(message("m1", "hi"));
    let session = SessionId::new("s1");
    let meta = |delivery| {
        serde_json::to_value(notification(&session, update(), delivery)).unwrap()["_meta"].clone()
    };
    assert_eq!(
        [meta(Delivery::Replay), meta(Delivery::Live)],
        [json!({"isReplay": true}), json!(null)]
    );
}
